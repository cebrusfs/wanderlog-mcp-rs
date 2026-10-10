//! Minimal ShareDB client (wire protocol 1.2, json0 document type) for one Wanderlog trip.
//!
//! Wanderlog edits trips through ShareDB, the realtime engine behind its live collaboration.
//! We hold one short-lived WebSocket per tool call: handshake → subscribe (snapshot + version) →
//! submit a single op → wait for its acknowledgement → close. An op submitted at an older version
//! is transformed against concurrent edits by the server, so no client-side OT is needed.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream, connect_async_tls_with_config,
};

pub const WS_BASE: &str = "wss://wanderlog.com/api/tripPlans/wsOverall/";
const COLLECTION: &str = "TripPlans";
/// Budget for each whole step (handshake, subscribe, acknowledgement), not per frame: pings and
/// tripmates' ops keep arriving and must not extend the wait.
pub const TIMEOUT: Duration = Duration::from_secs(20);

pub use crate::errors::{OutcomeUnknown, RevisionConflict};

/// A trip document at a known ShareDB version.
pub struct Snapshot {
    pub version: u64,
    pub doc: Value,
}

impl Snapshot {
    pub fn validate_revision(&self, expected: u64) -> Result<()> {
        if self.version != expected {
            return Err(RevisionConflict {
                expected,
                actual: self.version,
            }
            .into());
        }
        Ok(())
    }
}

pub struct Connection {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    /// Trip key (document id). Bearer secret: never put it in errors.
    key: String,
    cookie: String,
    session: String,
    seq: u64,
    timeout: Duration,
}

impl Connection {
    pub async fn open(cookie: &str, key: &str) -> Result<Self> {
        // clientSchemaVersion=2 and an Origin header are both required, or the server drops us.
        Self::open_url(
            &format!(
                "{WS_BASE}{}?clientSchemaVersion=2",
                crate::rest::encode_segment(key)
            ),
            cookie,
            key,
            TIMEOUT,
        )
        .await
    }

    pub async fn open_url(url: &str, cookie: &str, key: &str, timeout: Duration) -> Result<Self> {
        let url = crate::rest::validate_endpoint(url, "wss", "ws")?;
        ensure!(!timeout.is_zero(), "WebSocket timeout must be nonzero");
        let cookie = crate::session::normalize(cookie)?;
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|_| anyhow!("invalid trip key"))?;
        let mut session = HeaderValue::from_str(&format!("connect.sid={cookie}"))
            .context("invalid session cookie")?;
        session.set_sensitive(true);
        let headers = request.headers_mut();
        headers.insert("cookie", session);
        headers.insert("origin", HeaderValue::from_static(crate::rest::BASE));
        headers.insert("user-agent", HeaderValue::from_static(crate::USER_AGENT));

        let connector = if url.scheme() == "wss" {
            tls_connector()?
        } else {
            Connector::Plain
        };
        let connect = connect_async_tls_with_config(request, None, false, Some(connector));
        let (ws, _) = tokio::time::timeout(timeout, connect)
            .await
            .map_err(|_| anyhow!("timed out connecting to Wanderlog"))?
            .map_err(|e| {
                anyhow!(
                    "could not open Wanderlog's edit channel: {}",
                    crate::rest::redact(&describe(&e), &[key.to_owned(), cookie.clone()])
                )
            })?;
        let mut conn = Self {
            ws,
            key: key.to_owned(),
            cookie,
            session: String::new(),
            seq: 0,
            timeout,
        };

        conn.send(json!({"a": "hs", "id": null, "protocol": 1, "protocolMinor": 2}))
            .await?;
        let deadline = Instant::now() + conn.timeout;
        loop {
            let frame = conn.recv(deadline).await?;
            conn.check_error(&frame)?;
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
            self.check_error(&frame)?;
            if frame.get("a").and_then(Value::as_str) != Some("s") || !self.matches_document(&frame)
            {
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
                .is_none_or(|src| src.as_str() == Some(self.session.as_str()));
            let own_reply = is_op && same_seq && from_us && self.matches_document(&frame);
            if let Some(error) = frame.get("error") {
                if own_reply {
                    bail!(
                        "Wanderlog rejected the edit (nothing applied): {}",
                        self.sanitize(&error_text(error))
                    );
                }
                if frame.get("seq").is_none() {
                    return Err(unknown(anyhow!(
                        "Wanderlog error: {}",
                        self.sanitize(&error_text(error))
                    )));
                }
                continue;
            }
            self.check_error(&frame).map_err(unknown)?;
            if own_reply && frame.get("op").is_none() {
                return frame
                    .get("v")
                    .and_then(Value::as_u64)
                    .filter(|v| *v >= version && *v < u64::MAX)
                    .ok_or_else(|| unknown(anyhow!("acknowledgement without a valid version")));
            }
            // Ops from tripmates arrive here too; the server has already transformed ours.
        }
    }

    pub async fn close(mut self) {
        let _ = tokio::time::timeout(self.timeout, self.ws.close(None)).await;
    }

    async fn send(&mut self, frame: Value) -> Result<()> {
        tokio::time::timeout(self.timeout, self.ws.send(Message::text(frame.to_string())))
            .await
            .map_err(|_| anyhow!("timed out sending to Wanderlog"))?
            .map_err(|e| {
                anyhow!(
                    "sending to Wanderlog failed: {}",
                    self.sanitize(&describe(&e))
                )
            })
    }

    fn matches_document(&self, frame: &Value) -> bool {
        frame
            .get("c")
            .is_none_or(|c| c.as_str() == Some(COLLECTION))
            && frame
                .get("d")
                .is_none_or(|d| d.as_str() == Some(self.key.as_str()))
    }
    fn sanitize(&self, text: &str) -> String {
        crate::rest::redact(text, &[self.key.clone(), self.cookie.clone()])
    }
    fn check_error(&self, frame: &Value) -> Result<()> {
        check_error(frame).map_err(|e| anyhow!("{}", self.sanitize(&format!("{e:#}"))))
    }
    async fn recv(&mut self, deadline: Instant) -> Result<Value> {
        loop {
            let next = tokio::time::timeout_at(deadline, self.ws.next())
                .await
                .map_err(|_| anyhow!("Wanderlog did not answer in time"))?;
            match next {
                None => bail!("Wanderlog closed the connection"),
                Some(Err(e)) => bail!("connection error: {}", self.sanitize(&describe(&e))),
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
                                format!(": {}", self.sanitize(&quoted(&f.reason)))
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

/// TLS settings shared by every edit-channel connection, trusting the system root certificates.
/// tungstenite would otherwise reload the roots for each connection (60–100 ms on macOS).
fn tls_connector() -> Result<Connector> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    if let Some(config) = CONFIG.get() {
        return Ok(Connector::Rustls(config.clone()));
    }
    let mut roots = rustls::RootCertStore::empty();
    let (added, _) =
        roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
    ensure!(
        added > 0,
        "no usable system root certificates for Wanderlog's edit channel"
    );
    let config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    );
    Ok(Connector::Rustls(CONFIG.get_or_init(|| config).clone()))
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
#[path = "tests/sharedb.rs"]
mod tests;

#[cfg(test)]
#[path = "sharedb_tests.rs"]
mod coverage_tests;
