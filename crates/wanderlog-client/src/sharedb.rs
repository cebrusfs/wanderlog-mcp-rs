//! Minimal ShareDB client (wire protocol 1.2, json0 document type) for one Wanderlog trip.
//!
//! Wanderlog edits trips through ShareDB, the realtime engine behind its live collaboration.
//! We hold one short-lived WebSocket per tool call: handshake → subscribe (snapshot + version) →
//! submit a single op → wait for its acknowledgement → close. An op submitted at an older version
//! is transformed against concurrent edits by the server, so no client-side OT is needed.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

const WS_BASE: &str = "wss://wanderlog.com/api/tripPlans/wsOverall/";
const COLLECTION: &str = "TripPlans";
/// Budget for each whole step (handshake, subscribe, acknowledgement), not per frame: pings and
/// tripmates' ops keep arriving and must not extend the wait.
const TIMEOUT: Duration = Duration::from_secs(20);

/// The op was sent but Wanderlog never confirmed or rejected it, so it may or may not be applied.
#[derive(Debug)]
pub struct OutcomeUnknown(pub String);

impl std::fmt::Display for OutcomeUnknown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}. The edit may or may not have been applied: re-read the trip before retrying",
            self.0
        )
    }
}

impl std::error::Error for OutcomeUnknown {}

/// A trip document at a known ShareDB version.
pub struct Snapshot {
    pub version: u64,
    pub doc: Value,
}

pub struct Connection {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    /// Trip key (document id). Bearer secret: never put it in errors.
    key: String,
    session: String,
    seq: u64,
    timeout: Duration,
}

impl Connection {
    pub async fn open(cookie: &str, key: &str) -> Result<Self> {
        // clientSchemaVersion=2 and an Origin header are both required, or the server drops us.
        Self::open_url(
            &format!("{WS_BASE}{key}?clientSchemaVersion=2"),
            cookie,
            key,
            TIMEOUT,
        )
        .await
    }

    async fn open_url(url: &str, cookie: &str, key: &str, timeout: Duration) -> Result<Self> {
        let mut request = url
            .into_client_request()
            .map_err(|_| anyhow!("invalid trip key"))?;
        let mut session = HeaderValue::from_str(&format!("connect.sid={cookie}"))
            .context("invalid session cookie")?;
        session.set_sensitive(true);
        let headers = request.headers_mut();
        headers.insert("cookie", session);
        headers.insert("origin", HeaderValue::from_static(crate::rest::BASE));
        headers.insert("user-agent", HeaderValue::from_static(crate::USER_AGENT));

        let (ws, _) = tokio::time::timeout(timeout, connect_async(request))
            .await
            .map_err(|_| anyhow!("timed out connecting to Wanderlog"))?
            .map_err(|e| anyhow!("could not open Wanderlog's edit channel: {}", describe(&e)))?;
        let mut conn = Self {
            ws,
            key: key.to_owned(),
            session: String::new(),
            seq: 0,
            timeout,
        };

        conn.send(json!({"a": "hs", "id": null, "protocol": 1, "protocolMinor": 2}))
            .await?;
        let deadline = Instant::now() + conn.timeout;
        loop {
            let frame = conn.recv(deadline).await?;
            check_error(&frame)?;
            let id = frame.get("id").and_then(Value::as_str);
            match frame.get("a").and_then(Value::as_str) {
                Some("init") => conn.session = id.unwrap_or_default().to_owned(),
                Some("hs") => {
                    if let Some(id) = id {
                        conn.session = id.to_owned();
                    }
                    break;
                }
                _ => {}
            }
        }
        ensure!(
            !conn.session.is_empty(),
            "Wanderlog handshake returned no session id"
        );
        Ok(conn)
    }

    /// Subscribe to the trip and return its current snapshot.
    pub async fn subscribe(&mut self) -> Result<Snapshot> {
        self.send(json!({"a": "s", "c": COLLECTION, "d": self.key}))
            .await?;
        let deadline = Instant::now() + self.timeout;
        loop {
            let frame = self.recv(deadline).await?;
            check_error(&frame)?;
            if frame.get("a").and_then(Value::as_str) != Some("s") {
                continue;
            }
            let data = frame
                .get("data")
                .ok_or_else(|| anyhow!("trip not found or not accessible"))?;
            let version = data
                .get("v")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("snapshot without version"))?;
            let doc = data
                .get("data")
                .filter(|d| d.is_object())
                .cloned()
                .ok_or_else(|| anyhow!("trip not found or not accessible"))?;
            return Ok(Snapshot { version, doc });
        }
    }

    /// Submit one op built against `version`; returns the version the server applied it at.
    pub async fn submit(&mut self, version: u64, components: &[Value]) -> Result<u64> {
        self.seq += 1;
        let seq = self.seq;
        let unknown = |e: anyhow::Error| anyhow::Error::new(OutcomeUnknown(format!("{e:#}")));
        self.send(json!({"a": "op", "c": COLLECTION, "d": self.key, "v": version, "seq": seq, "x": {}, "op": components}))
            .await
            .map_err(unknown)?;
        let deadline = Instant::now() + self.timeout;
        loop {
            let frame = self.recv(deadline).await.map_err(unknown)?;
            let is_op = frame.get("a").and_then(Value::as_str) == Some("op");
            let same_seq = frame.get("seq").and_then(Value::as_u64) == Some(seq);
            let from_us = frame
                .get("src")
                .and_then(Value::as_str)
                .is_none_or(|src| src == self.session);
            if let Some(error) = frame.get("error") {
                if is_op && same_seq {
                    bail!(
                        "Wanderlog rejected the edit (nothing applied): {}",
                        error_text(error)
                    );
                }
                if frame.get("seq").is_none() {
                    return Err(unknown(anyhow!("Wanderlog error: {}", error_text(error))));
                }
                continue;
            }
            check_error(&frame).map_err(unknown)?;
            if is_op && same_seq && from_us && frame.get("op").is_none() {
                return frame
                    .get("v")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| anyhow!("acknowledgement without version"));
            }
            // Ops from tripmates arrive here too; the server has already transformed ours.
        }
    }

    pub async fn close(mut self) {
        let _ = self.ws.close(None).await;
    }

    async fn send(&mut self, frame: Value) -> Result<()> {
        self.ws
            .send(Message::text(frame.to_string()))
            .await
            .map_err(|e| anyhow!("sending to Wanderlog failed: {}", describe(&e)))
    }

    async fn recv(&mut self, deadline: Instant) -> Result<Value> {
        loop {
            let next = tokio::time::timeout_at(deadline, self.ws.next())
                .await
                .map_err(|_| anyhow!("Wanderlog did not answer in time"))?;
            match next {
                None => bail!("Wanderlog closed the connection"),
                Some(Err(e)) => bail!("connection error: {}", describe(&e)),
                Some(Ok(Message::Text(text))) => {
                    return serde_json::from_str(text.as_str())
                        .context("malformed frame from Wanderlog");
                }
                Some(Ok(Message::Close(frame))) => {
                    let detail = frame.map(|f| {
                        format!(
                            " (code {}{})",
                            f.code,
                            if f.reason.is_empty() {
                                String::new()
                            } else {
                                format!(": {}", quoted(&f.reason))
                            }
                        )
                    });
                    bail!(
                        "Wanderlog closed the connection{}",
                        detail.unwrap_or_default()
                    )
                }
                Some(Ok(_)) => {} // ping/pong/binary: tungstenite answers pings itself
            }
        }
    }
}

/// Fail on ShareDB error frames, including the bare `{code, message}` form (e.g. rate limits).
fn check_error(frame: &Value) -> Result<()> {
    if let Some(error) = frame.get("error") {
        bail!("Wanderlog error: {}", error_text(error));
    }
    if frame.get("a").is_none()
        && let Some(code) = frame.get("code")
    {
        let message = frame
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        bail!(
            "Wanderlog refused the request ({}): {}",
            crate::render::field(&code.to_string()),
            quoted(message)
        );
    }
    Ok(())
}

/// Server-supplied text is untrusted (it can echo user content): always quote it.
fn quoted(text: &str) -> String {
    crate::render::quote(text, crate::render::Options::default())
}

fn error_text(error: &Value) -> String {
    match error {
        Value::String(s) => quoted(s),
        other => quoted(
            &other
                .get("message")
                .and_then(Value::as_str)
                .map_or_else(|| other.to_string(), str::to_owned),
        ),
    }
}

/// Error text without URLs (connection errors can embed the request URI, which holds the key).
fn describe(error: &tungstenite::Error) -> String {
    match error {
        tungstenite::Error::Url(_) => "could not connect".to_owned(),
        tungstenite::Error::Http(response) => format!("HTTP {}", response.status()),
        tungstenite::Error::Io(io) => format!("I/O error ({})", io.kind()),
        tungstenite::Error::Tls(_) => "TLS error".to_owned(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_frames() {
        assert!(check_error(&json!({"a": "hs", "id": "x"})).is_ok());
        let err = check_error(&json!({"code": 4001, "message": "Too many requests"})).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Wanderlog refused the request (4001): «Too many requests»"
        );
        let err = check_error(&json!({"a": "s", "error": {"code": 4022, "message": "Forbidden"}}))
            .unwrap_err();
        assert_eq!(err.to_string(), "Wanderlog error: «Forbidden»");
    }

    /// A server that keeps the socket busy (pings, tripmates' ops) but never acknowledges our op
    /// must not keep `submit` waiting past its deadline.
    #[tokio::test]
    async fn ack_wait_has_a_hard_deadline() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            while let Some(Ok(msg)) = ws.next().await {
                let Message::Text(text) = msg else { continue };
                let frame: Value = serde_json::from_str(text.as_str()).unwrap();
                let reply = match frame["a"].as_str() {
                    Some("hs") => json!({"a": "hs", "id": "me", "protocol": 1, "protocolMinor": 2}),
                    Some("s") => {
                        json!({"a": "s", "c": COLLECTION, "d": "k", "data": {"v": 3, "data": {"title": "t"}}})
                    }
                    Some("op") => loop {
                        let _ = ws.send(Message::Ping(vec![1].into())).await;
                        let foreign = json!({"a": "op", "c": COLLECTION, "d": "k", "v": 3, "src": "other", "seq": 1, "op": []});
                        let _ = ws.send(Message::text(foreign.to_string())).await;
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    },
                    _ => continue,
                };
                ws.send(Message::text(reply.to_string())).await.unwrap();
            }
        });
        let url = format!("ws://{addr}/");
        let mut conn = Connection::open_url(&url, "cookie", "k", Duration::from_millis(800))
            .await
            .unwrap();
        let snapshot = conn.subscribe().await.unwrap();
        let started = std::time::Instant::now();
        let err = conn
            .submit(
                snapshot.version,
                &[json!({"p": ["title"], "od": "t", "oi": "x"})],
            )
            .await
            .unwrap_err();
        assert!(err.downcast_ref::<OutcomeUnknown>().is_some(), "{err:#}");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "waited {:?}",
            started.elapsed()
        );
    }
}
