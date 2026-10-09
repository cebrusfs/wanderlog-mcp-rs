use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
pub(crate) fn response(status: &str, headers: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
        body.len()
    )
}

pub(crate) async fn login_server(
    responses: Vec<String>,
) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(5), async move {
            let mut requests = Vec::new();
            for response in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut buf = [0; 1024];
                    let n = stream.read(&mut buf).await.unwrap();
                    assert!(n > 0, "client closed before completing the request");
                    request.extend_from_slice(&buf[..n]);
                    let Some(end) = request.windows(4).position(|b| b == b"\r\n\r\n") else {
                        continue;
                    };
                    let headers = std::str::from_utf8(&request[..end]).unwrap();
                    let length = headers
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
                requests.push(String::from_utf8(request).unwrap());
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        })
        .await
        .expect("login requests exceeded the test deadline")
    });
    (base, server)
}

#[cfg(test)]
#[path = "tests/test_support_fixture.rs"]
pub(crate) mod fixture;

/// Mock ShareDB scripts: an optional subscription error and optional op acknowledgement.
pub(crate) async fn websocket_server(
    doc: serde_json::Value,
    scripts: Vec<(bool, Option<serde_json::Value>)>,
) -> (String, tokio::task::JoinHandle<Vec<serde_json::Value>>) {
    use futures_util::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    use tokio_tungstenite::tungstenite::Message;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("ws://{}/", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(5), async move {
            let mut ops = vec![];
            for (reject, ack) in scripts {
                let (stream, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                let read =
                    |m: Message| serde_json::from_str::<Value>(m.to_text().unwrap()).unwrap();
                assert_eq!(read(ws.next().await.unwrap().unwrap())["a"], "hs");
                ws.send(Message::text(json!({"a":"hs","id":"session"}).to_string()))
                    .await
                    .unwrap();
                let request = read(ws.next().await.unwrap().unwrap());
                assert_eq!(request["a"], "s");
                let frame = if reject {
                    json!({"a":"s","error":"mock denied"})
                } else {
                    json!({"a":"s","c":"TripPlans","d":request["d"],"data":{"v":12,"data":doc}})
                };
                ws.send(Message::text(frame.to_string())).await.unwrap();
                while let Some(Ok(m)) = ws.next().await {
                    if m.is_close() {
                        break;
                    }
                    if m.is_text() {
                        let op = read(m);
                        assert_eq!(op["a"], "op");
                        ops.push(op.clone());
                        if let Some(reply) = &ack {
                            let mut frame = reply.clone();
                            frame["seq"] = op["seq"].clone();
                            ws.send(Message::text(frame.to_string())).await.unwrap();
                        } else {
                            ws.close(None).await.unwrap();
                            break;
                        }
                    }
                }
            }
            ops
        })
        .await
        .expect("mock WebSocket deadline")
    });
    (base, task)
}
