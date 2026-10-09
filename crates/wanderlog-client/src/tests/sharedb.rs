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
