use super::*;
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

#[tokio::test]
async fn login_verifies_the_new_cookie_without_forwarding_other_cookies() {
    let (base, server) = login_server(vec![
            response(
                "200 OK",
                "Set-Cookie: analytics=other; Path=/\r\nSet-Cookie: connect.sid=s%3Anew.session%3D; Path=/; HttpOnly; Secure\r\n",
                r#"{"success":true,"user":{"id":1,"username":"unverified"}}"#,
            ),
            response("200 OK", "", r#"{"success":true,"user":{"id":2,"username":"verified"}}"#),
        ])
        .await;
    let (cookie, user) = Rest::login_with_base_url(&base, " person@example.com ", " password ")
        .await
        .unwrap();
    assert_eq!(cookie, "s%3Anew.session%3D");
    assert_eq!(user["username"], "verified");
    let requests = server.await.unwrap();
    let (headers, body) = requests[0].split_once("\r\n\r\n").unwrap();
    assert!(headers.starts_with("POST /api/user/login HTTP/1.1\r\n"));
    assert!(!headers.to_ascii_lowercase().contains("\r\ncookie:"));
    assert_eq!(
        serde_json::from_str::<Value>(body).unwrap(),
        json!({"email":"person@example.com", "password":" password ", "platform":"web"})
    );
    assert!(requests[1].starts_with("GET /api/user HTTP/1.1\r\n"));
    assert!(requests[1].contains("connect.sid=s%3Anew.session%3D\r\n"));
    for private in ["analytics", "person@example.com", "password"] {
        assert!(!requests[1].contains(private));
    }
}

#[tokio::test]
async fn login_failures_never_echo_credentials_or_server_messages() {
    let sensitive_body =
        r#"{"success":false,"messages":["person@example.com secret-password s%3Asecret.cookie"]}"#;
    for (status, body, expected) in [
        ("401 Unauthorized", sensitive_body, "rejected"),
        ("403 Forbidden", sensitive_body, "rejected"),
        ("429 Too Many Requests", sensitive_body, "rate limited"),
        ("500 Internal Server Error", sensitive_body, "HTTP 500"),
        ("302 Found", sensitive_body, "HTTP 302"),
        ("200 OK", sensitive_body, "did not accept"),
        ("200 OK", "secret-password", "unreadable response"),
    ] {
        let (base, server) = login_server(vec![response(
            status,
            "Set-Cookie: connect.sid=s%3Asecret.cookie; Path=/\r\nLocation: /must-not-follow\r\n",
            body,
        )])
        .await;
        let err = Rest::login_with_base_url(&base, "person@example.com", "secret-password")
            .await
            .unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains(expected), "{message}");
        for secret in ["person@example.com", "secret-password", "s%3Asecret.cookie"] {
            assert!(!message.contains(secret), "{message}");
        }
        assert_eq!(server.await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn login_requires_a_session_cookie_and_authenticated_user() {
    for headers in [
        "",
        "Set-Cookie: analytics=connect.sid=unrelated\r\n",
        "Set-Cookie: connect.sid=; Path=/\r\n",
        "Set-Cookie: connect.sid=invalid value; Path=/\r\n",
    ] {
        let (base, server) = login_server(vec![response(
            "200 OK",
            headers,
            r#"{"success":true,"user":{"id":1}}"#,
        )])
        .await;
        let err = Rest::login_with_base_url(&base, "person@example.com", "password")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("session cookie"));
        assert_eq!(server.await.unwrap().len(), 1);
    }
    for (status, body, expected) in [
        ("200 OK", r#"{"success":true,"user":null}"#, "not logged in"),
        ("200 OK", r#"{"success":true,"user":{}}"#, "not logged in"),
        (
            "401 Unauthorized",
            "secret-password s%3Anew.session",
            "rejected",
        ),
    ] {
        let (base, server) = login_server(vec![
            response(
                "200 OK",
                "Set-Cookie: connect.sid=s%3Anew.session; Path=/\r\n",
                r#"{"success":true,"user":{"id":1}}"#,
            ),
            response(status, "", body),
        ])
        .await;
        let err = Rest::login_with_base_url(&base, "person@example.com", "secret-password")
            .await
            .unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains(expected), "{message}");
        assert!(!message.contains("secret-password"));
        assert!(!message.contains("s%3Anew.session"));
        assert_eq!(server.await.unwrap().len(), 2);
    }
}

#[tokio::test]
async fn login_rejects_empty_credentials_before_network_access() {
    for (email, password) in [("  ", "password"), ("person@example.com", "")] {
        let err = Rest::login_with_base_url("invalid-url", email, password)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("cannot be empty"));
    }
}

#[test]
fn retry_after_parses_seconds_dates_and_defaults() {
    for (raw, expected) in [
        (None, None),
        (Some("garbage"), None),
        (Some("-1"), None),
        (Some("12"), Some(12)),
        (Some("0"), Some(0)),
        (Some("184467440737095516160"), Some(u64::MAX)),
        (Some("Sun, 06 Nov 1994 08:49:37 GMT"), Some(0)),
    ] {
        let mut headers = HeaderMap::new();
        if let Some(raw) = raw {
            headers.insert(
                reqwest::header::RETRY_AFTER,
                HeaderValue::from_str(raw).unwrap(),
            );
        }
        let error = rate_limit_at(&headers, SystemTime::now());
        assert_eq!(error.retry_after_seconds, expected);
        assert_eq!(error.retry_in_seconds, expected.unwrap_or(60));
    }
    let mut headers = HeaderMap::new();
    headers.insert(
        reqwest::header::RETRY_AFTER,
        HeaderValue::from_str(&httpdate::fmt_http_date(
            SystemTime::now() + Duration::from_secs(120),
        ))
        .unwrap(),
    );
    assert!(matches!(
        rate_limit_at(&headers, SystemTime::now()).retry_after_seconds,
        Some(119..=120)
    ));
}

#[tokio::test]
async fn html_rate_limit_blocks_clones_without_replaying_post_or_leaking_secrets() {
    let (base, server) = login_server(vec![response(
        "429 Too Many Requests",
        "Retry-After: 120\r\n",
        "<html>private-cookie trip-secret</html>",
    )])
    .await;
    let rest = Rest::for_test(&base).unwrap();
    let clone = rest.clone();
    let error = rest
        .post(
            "/api/tripPlans/trip-secret",
            &json!({"secret":"private-cookie"}),
            "write",
        )
        .await
        .unwrap_err();
    let limit = error.downcast_ref::<RateLimited>().unwrap();
    assert_eq!(limit.retry_after_seconds, Some(120));
    for secret in [base.as_str(), "trip-secret", "private-cookie", "<html>"] {
        assert!(!format!("{error:#}").contains(secret));
    }
    let error = clone.current_user().await.unwrap_err();
    assert!(error.downcast_ref::<RateLimited>().is_some());
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("POST "));
}

#[tokio::test]
async fn rate_limit_invalid_and_absent_headers_use_default_before_json() {
    for headers in ["", "Retry-After: invalid\r\n"] {
        let (base, server) = login_server(vec![response(
            "429 Too Many Requests",
            headers,
            "<html>limited</html>",
        )])
        .await;
        let rest = Rest::for_test(&base).unwrap();
        let error = rest.current_user().await.unwrap_err();
        let limit = error.downcast_ref::<RateLimited>().unwrap();
        assert_eq!(limit.retry_after_seconds, None);
        assert_eq!(limit.retry_in_seconds, 60);
        assert_eq!(server.await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn expired_cooldown_allows_one_new_request() {
    let (base, server) = login_server(vec![
        response("429 Too Many Requests", "Retry-After: 1\r\n", "limited"),
        response("200 OK", "", r#"{"success":true,"user":{"id":1}}"#),
    ])
    .await;
    let rest = Rest::for_test(&base).unwrap();
    assert!(
        rest.current_user()
            .await
            .unwrap_err()
            .downcast_ref::<RateLimited>()
            .is_some()
    );
    {
        let mut cooldown = rest.cooldown.lock().unwrap();
        cooldown.as_mut().unwrap().started = Instant::now() - Duration::from_secs(2);
    }
    assert_eq!(rest.current_user().await.unwrap().unwrap()["id"], 1);
    assert_eq!(server.await.unwrap().len(), 2);
}

#[tokio::test]
async fn bulk_details_encode_repeated_query_and_validate_response() {
    let (base, server) = login_server(vec![response(
        "200 OK",
        "",
        r#"{"success":true,"data":[{"place_id":"first"},{"place_id":"second"}]}"#,
    )])
    .await;
    let rest = Rest::for_test(&base).unwrap();
    assert!(rest.multiple_place_details(&[]).await.unwrap().is_empty());
    let details = rest
        .multiple_place_details(&["first/東京".into(), "second&value".into()])
        .await
        .unwrap();
    assert_eq!(details.len(), 2);
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET /api/placesAPI/getMultiplePlaceDetails?placeIds%5B%5D=first%2F%E6%9D%B1%E4%BA%AC&placeIds%5B%5D=second%26value&language=en HTTP/1.1\r\n"));
    assert!(!requests[0].to_ascii_lowercase().contains("\r\ncookie:"));
    for data in [json!(null), json!({})] {
        let (base, server) = login_server(vec![response(
            "200 OK",
            "",
            &json!({"success":true,"data":data}).to_string(),
        )])
        .await;
        assert!(
            Rest::for_test(&base)
                .unwrap()
                .multiple_place_details(&["first".into()])
                .await
                .is_err()
        );
        server.await.unwrap();
    }
}

#[test]
fn debug_never_prints_the_trip_key() {
    let t = TripSummary {
        id: 1,
        key: "abcdefghijklmnop".into(),
        editable: true,
        title: "t".into(),
        start_date: None,
        end_date: None,
        place_count: 0,
        edited_at: None,
        relation: "own",
    };
    assert!(!format!("{t:?}").contains("abcdefghijklmnop"));
}

#[test]
fn segments_and_uuids() {
    assert_eq!(encode_segment("ChIJ_x-1.~"), "ChIJ_x-1.~");
    assert_eq!(
        encode_segment("東京 tower/1"),
        "%E6%9D%B1%E4%BA%AC%20tower%2F1"
    );
    let id = uuid_v4();
    assert_eq!(id.len(), 36);
    assert_eq!(&id[14..15], "4");
    assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"));
}
