use super::*;
use tokio_tungstenite::accept_hdr_async;

// The handshake callback's error type is fixed by tungstenite.
#[allow(clippy::result_large_err)]
async fn mock(
    frames: Vec<Value>,
    reply: Option<Value>,
) -> (String, tokio::task::JoinHandle<Vec<Value>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/trip", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(3), async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = accept_hdr_async(
                stream,
                |request: &tungstenite::handshake::server::Request,
                 response: tungstenite::handshake::server::Response| {
                    assert_eq!(request.headers()["cookie"], "connect.sid=synthetic-session");
                    assert_eq!(request.headers()["origin"], crate::rest::BASE);
                    Ok(response)
                },
            )
            .await
            .unwrap();
            let read = |message: Message| {
                serde_json::from_str::<Value>(message.to_text().unwrap()).unwrap()
            };
            let hs = read(ws.next().await.unwrap().unwrap());
            assert_eq!(hs["a"], "hs");
            ws.send(Message::text(
                json!({"a":"init","id":"session"}).to_string(),
            ))
            .await
            .unwrap();
            ws.send(Message::text(json!({"a":"hs","id":"session"}).to_string()))
                .await
                .unwrap();
            let subscribe = read(ws.next().await.unwrap().unwrap());
            for frame in frames {
                ws.send(Message::text(frame.to_string())).await.unwrap();
            }
            let mut seen = vec![hs, subscribe];
            if let Some(reply) = reply {
                let op = read(ws.next().await.unwrap().unwrap());
                seen.push(op);
                for frame in reply.as_array().unwrap() {
                    ws.send(Message::text(frame.to_string())).await.unwrap();
                }
            }
            while let Some(Ok(message)) = ws.next().await {
                if message.is_close() {
                    break;
                }
            }
            seen
        })
        .await
        .unwrap()
    });
    (url, task)
}
fn snapshot() -> Value {
    json!({"a":"s","c":"TripPlans","d":"trip-key","data":{"v":8,"data":{"title":"Before"}}})
}
#[tokio::test]
async fn snapshot_and_ack_ignore_other_documents_sources_and_ops() {
    let (url,task)=mock(vec![json!({"a":"s","d":"other","data":{"v":1,"data":{}}}),snapshot()],Some(json!([
 {"a":"op","seq":1,"v":2,"src":"other","error":"ignore"}, {"a":"op","seq":1,"v":2,"d":"other"}, {"a":"op","seq":1,"v":2,"src":4}, {"a":"op","seq":1,"v":8,"src":"session","op":[]}, {"a":"op","seq":1,"v":9,"src":"session","d":"trip-key"}]))).await;
    let mut conn = Connection::open_url(
        &url,
        "synthetic-session",
        "trip-key",
        Duration::from_secs(1),
    )
    .await
    .unwrap();
    let snap = conn.subscribe().await.unwrap();
    assert_eq!(snap.version, 8);
    snap.validate_revision(8).unwrap();
    assert!(
        snap.validate_revision(7)
            .unwrap_err()
            .downcast_ref::<RevisionConflict>()
            .is_some()
    );
    assert_eq!(
        conn.submit(8, &[json!({"p":["title"],"oi":"After"})])
            .await
            .unwrap(),
        9
    );
    conn.close().await;
    let seen = task.await.unwrap();
    assert_eq!(seen[2]["op"].as_array().unwrap().len(), 1);
    assert_eq!(seen[2]["v"], 8);
}
#[tokio::test]
async fn missing_stale_and_overflow_ack_versions_are_unknown() {
    for frame in [
        json!({"a":"op","seq":1}),
        json!({"a":"op","seq":1,"v":7}),
        json!({"a":"op","seq":1,"v":u64::MAX}),
        json!({"code":4001,"message":"trip-key synthetic-session"}),
    ] {
        let (url, task) = mock(vec![snapshot()], Some(json!([frame]))).await;
        let mut conn = Connection::open_url(
            &url,
            "synthetic-session",
            "trip-key",
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        conn.subscribe().await.unwrap();
        let err = conn.submit(8, &[]).await.unwrap_err();
        assert!(err.downcast_ref::<OutcomeUnknown>().is_some());
        let text = err.to_string();
        assert!(!text.contains("trip-key"));
        assert!(!text.contains("synthetic-session"));
        conn.close().await;
        task.await.unwrap();
    }
}
#[tokio::test]
async fn explicit_own_rejection_is_known_and_redacted() {
    let (url, task) = mock(
        vec![snapshot()],
        Some(json!([{"a":"op","seq":1,"error":{"message":"trip-key synthetic-session"}}])),
    )
    .await;
    let mut conn = Connection::open_url(
        &url,
        "synthetic-session",
        "trip-key",
        Duration::from_secs(1),
    )
    .await
    .unwrap();
    conn.subscribe().await.unwrap();
    let err = conn.submit(8, &[]).await.unwrap_err();
    assert!(err.downcast_ref::<OutcomeUnknown>().is_none());
    assert!(err.to_string().contains("nothing applied"));
    assert!(!err.to_string().contains("trip-key"));
    conn.close().await;
    task.await.unwrap();
}
#[tokio::test]
async fn malformed_subscriptions_fail_before_write() {
    for frame in [
        json!({"a":"s"}),
        json!({"a":"s","data":{"data":{}}}),
        json!({"a":"s","data":{"v":8,"data":null}}),
        json!({"error":"trip-key synthetic-session"}),
    ] {
        let (url, task) = mock(vec![frame], None).await;
        let mut conn = Connection::open_url(
            &url,
            "synthetic-session",
            "trip-key",
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        let err = conn.subscribe().await.err().unwrap();
        assert!(!err.to_string().contains("trip-key"));
        conn.close().await;
        task.await.unwrap();
    }
}
#[tokio::test]
async fn transport_validation_and_connection_failure_are_safe() {
    for (url, timeout) in [
        ("ws://example.com/key", Duration::from_secs(1)),
        ("ws://127.0.0.1:1/key", Duration::ZERO),
    ] {
        assert!(
            Connection::open_url(url, "synthetic-session", "trip-key", timeout)
                .await
                .is_err()
        );
    }
    assert!(
        !Connection::open_url(
            "ws://127.0.0.1:1/trip-key",
            "synthetic-session",
            "trip-key",
            Duration::from_secs(1)
        )
        .await
        .err()
        .unwrap()
        .to_string()
        .contains("trip-key")
    );
}
#[tokio::test]
async fn defaults_reject_invalid_cookie_before_any_production_connection() {
    assert!(Connection::open("bad\ncookie", "trip/key?").await.is_err());
}
#[tokio::test]
async fn close_reason_and_malformed_frames_are_redacted() {
    for malformed in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/trip", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            ws.next().await.unwrap().unwrap();
            ws.send(Message::text(json!({"a":"hs","id":"session"}).to_string()))
                .await
                .unwrap();
            ws.next().await.unwrap().unwrap();
            if malformed {
                ws.send(Message::text("{invalid trip-key synthetic-session"))
                    .await
                    .unwrap();
            } else {
                ws.send(Message::Close(Some(tungstenite::protocol::CloseFrame {
                    code: tungstenite::protocol::frame::coding::CloseCode::Policy,
                    reason: "trip-key synthetic-session".into(),
                })))
                .await
                .unwrap();
            }
        });
        let mut conn = Connection::open_url(
            &url,
            "synthetic-session",
            "trip-key",
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        let err = conn.subscribe().await.err().unwrap().to_string();
        assert!(!err.contains("trip-key"));
        assert!(!err.contains("synthetic-session"));
        conn.close().await;
        task.await.unwrap();
    }
}
#[tokio::test]
async fn handshake_requires_id_and_handles_server_errors_and_init() {
    for frame in [
        json!({"a":"hs"}),
        json!({"error":"trip-key synthetic-session"}),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            ws.next().await.unwrap().unwrap();
            ws.send(Message::text(frame.to_string())).await.unwrap();
        });
        let err = Connection::open_url(
            &url,
            "synthetic-session",
            "trip-key",
            Duration::from_secs(1),
        )
        .await
        .err()
        .unwrap();
        assert!(!err.to_string().contains("trip-key"));
        task.await.unwrap();
    }
    let (url, task) = mock(
        vec![snapshot()],
        Some(json!([{"error":"trip-key synthetic-session"}])),
    )
    .await;
    let mut conn = Connection::open_url(
        &url,
        "synthetic-session",
        "trip-key",
        Duration::from_secs(1),
    )
    .await
    .unwrap();
    conn.subscribe().await.unwrap();
    assert!(
        conn.submit(8, &[])
            .await
            .unwrap_err()
            .downcast_ref::<OutcomeUnknown>()
            .is_some()
    );
    conn.close().await;
    task.await.unwrap();
}
#[test]
fn protocol_errors_never_render_request_urls() {
    let response = tungstenite::http::Response::builder()
        .status(401)
        .body(None)
        .unwrap();
    assert_eq!(
        describe(&tungstenite::Error::Http(Box::new(response))),
        "HTTP 401 Unauthorized"
    );
    assert_eq!(
        describe(&tungstenite::Error::Url(
            tungstenite::error::UrlError::NoHostName
        )),
        "could not connect"
    );
    assert_eq!(
        describe(&tungstenite::Error::ConnectionClosed),
        "Connection closed normally"
    );
}
