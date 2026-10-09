use super::tests::{login_server, response};
use super::*;
use std::sync::Mutex;

struct FakeClock(Mutex<(Instant, SystemTime)>);
impl Clock for FakeClock {
    fn now(&self) -> Instant {
        self.0.lock().unwrap().0
    }
    fn system_time(&self) -> SystemTime {
        self.0.lock().unwrap().1
    }
}
#[tokio::test]
async fn cooldown_is_shared_expires_without_sleep_and_never_replays() {
    let now = Instant::now();
    let wall = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    let clock = Arc::new(FakeClock(Mutex::new((now, wall))));
    let (base, requests) = login_server(vec![
        response("429 Too Many Requests", "Retry-After: 10\r\n", ""),
        response("200 OK", "", r#"{"success":true,"user":{"id":7}}"#),
    ])
    .await;
    let rest = Rest::with_base_url("synthetic-session", &base)
        .unwrap()
        .with_clock(clock.clone());
    let error = rest.current_user().await.unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<RateLimited>()
            .unwrap()
            .retry_in_seconds,
        10
    );
    clock.0.lock().unwrap().0 = now + Duration::from_secs(4);
    assert_eq!(
        rest.clone()
            .current_user()
            .await
            .unwrap_err()
            .downcast_ref::<RateLimited>()
            .unwrap()
            .retry_in_seconds,
        6
    );
    clock.0.lock().unwrap().0 = now + Duration::from_secs(10);
    assert_eq!(rest.current_user().await.unwrap().unwrap()["id"], 7);
    assert_eq!(requests.await.unwrap().len(), 2);
}
#[tokio::test]
async fn auth_failures_and_server_echoes_are_sanitized() {
    for status in ["401 Unauthorized", "403 Forbidden"] {
        let (base, requests) = login_server(vec![response(status, "", "not json")]).await;
        let err = Rest::with_base_url("secret-session", &base)
            .unwrap()
            .current_user()
            .await
            .unwrap_err();
        assert!(err.downcast_ref::<AuthenticationFailed>().is_some());
        assert!(!err.to_string().contains("secret-session"));
        requests.await.unwrap();
    }
    let key = "secret/key?";
    let encoded = encode_segment(key);
    let (base, requests) = login_server(vec![response(
        "500 Internal Server Error",
        "",
        &json!({"success":false,"messages":[format!("{key} {encoded} secret-session")]})
            .to_string(),
    )])
    .await;
    let err = Rest::with_base_url("secret-session", &base)
        .unwrap()
        .trip(key)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("<redacted>"),
        "server echo was not exercised: {err}"
    );
    for secret in [key, &encoded, "secret-session"] {
        assert!(!err.contains(secret), "{err}");
    }
    let wire = requests.await.unwrap();
    assert!(wire[0].starts_with(&format!("GET /api/tripPlans/{encoded}?")));
    assert!(wire[0].contains("cookie: connect.sid=secret-session"));
}
#[tokio::test]
async fn write_confirmation_errors_are_unknown_and_not_replayed() {
    for (status, body, unknown) in [
        ("200 OK", "not json", true),
        ("503 Service Unavailable", "not json", true),
        (
            "503 Service Unavailable",
            r#"{"success":false,"message":"failed"}"#,
            true,
        ),
        ("200 OK", r#"{"success":true,"data":null}"#, true),
        ("200 OK", r#"{"success":true,"data":{"id":9}}"#, true),
        ("400 Bad Request", r#"{"success":false}"#, false),
    ] {
        let (base, requests) = login_server(vec![response(status, "", body)]).await;
        let err = Rest::for_test(&base)
            .unwrap()
            .create_trip(1, "friends")
            .await
            .unwrap_err();
        assert_eq!(
            err.downcast_ref::<OutcomeUnknown>().is_some(),
            unknown,
            "{status}: {err}"
        );
        assert_eq!(requests.await.unwrap().len(), 1);
    }
    let (base, requests) = login_server(vec![response(
        "200 OK",
        "",
        r#"{"success":true,"data":{"id":9,"key":"new-trip-key"}}"#,
    )])
    .await;
    assert_eq!(
        Rest::for_test(&base)
            .unwrap()
            .create_trip(1, "friends")
            .await
            .unwrap()["id"],
        9
    );
    let request = &requests.await.unwrap()[0];
    let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["privacy"], "friends");
    assert_eq!(body["geoIds"], json!([1]));
}
#[tokio::test]
async fn redirects_never_forward_credentials_and_network_failures_have_no_url() {
    let (base, requests) = login_server(vec![response(
        "302 Found",
        "Location: http://127.0.0.1:1/stolen\r\n",
        "",
    )])
    .await;
    assert!(
        Rest::with_base_url("secret-session", &base)
            .unwrap()
            .current_user()
            .await
            .is_err()
    );
    assert_eq!(requests.await.unwrap().len(), 1);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let rest = Rest::with_base_url("secret-session", &base).unwrap();
    let err = rest.trip("secret-trip-key").await.unwrap_err();
    assert!(!format!("{err:#}").contains("secret-trip-key"));
    assert!(!format!("{err:#}").contains(&base));
    assert!(
        rest.create_trip(1, "friends")
            .await
            .unwrap_err()
            .downcast_ref::<OutcomeUnknown>()
            .is_some()
    );
}
#[tokio::test]
async fn listing_places_and_query_encoding_follow_wire_contract() {
    let bodies = vec![
        json!({"success":true,"ownTripPlans":[{"id":1,"key":"own-secret","keyType":"edit","title":"One","startDate":"2026-11-10","endDate":"2026-11-11","placeCount":4,"editedAt":"2026"},{"id":2}],"friendsPrivateSharedTripPlans":[{"id":1,"key":"duplicate"},{"id":2,"key":"view-secret"}],"friendsTripPlans":[{"id":3,"key":"friend-secret"}]}),
        json!({"success":true,"data":[{"place_id":"P1"},{"place_id":""},{"description":"search"}]}),
        json!({"success":true,"data":{"place_id":"P1","name":"One"}}),
        json!({"success":true,"data":[{"place_id":"P1"},null]}),
        json!({"success":true,"data":["photo"]}),
        json!({"success":true,"data":[{"description":"meta"}]}),
        json!({"success":true,"data":[{"id":5}]}),
        json!({"success":true,"user":null}),
    ];
    let (base, requests) = login_server(
        bodies
            .into_iter()
            .map(|v| response("200 OK", "", &v.to_string()))
            .collect(),
    )
    .await;
    let rest = Rest::for_test(&base).unwrap();
    let trips = rest.trips().await.unwrap();
    assert_eq!(trips.len(), 3);
    assert!(trips[0].editable);
    assert_eq!(trips[1].relation, "shared with you");
    assert_eq!(trips[2].relation, "friend");
    assert!(!format!("{:?}", trips[0]).contains("own-secret"));
    assert_eq!(
        rest.autocomplete("A/B", Some((35.0, 139.0, 7000.0)))
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(rest.place_details("P1").await.unwrap()["name"], "One");
    assert!(rest.multiple_place_details(&[]).await.unwrap().is_empty());
    assert_eq!(
        rest.multiple_place_details(&["P1".into()])
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        rest.place_photos(&json!({"place_id":"A/B"})).await.unwrap(),
        ["photo"]
    );
    assert!(
        rest.place_metadata("P1", "secret-key")
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(rest.geo_search("A/B 東京").await.unwrap()[0]["id"], 5);
    assert!(rest.current_user().await.unwrap().is_none());
    let wires = requests.await.unwrap();
    assert!(wires[4].starts_with("POST /api/placePhotos/A%2FB "));
    let url = reqwest::Url::parse(&format!(
        "{base}{}",
        wires[1].split_whitespace().nth(1).unwrap()
    ))
    .unwrap();
    let payload: Value =
        serde_json::from_str(&url.query_pairs().find(|(k, _)| k == "request").unwrap().1).unwrap();
    assert_eq!(payload["location"]["latitude"], 35.0);
    assert_eq!(payload["radius"], 7000.0);
    let token = payload["sessiontoken"].as_str().unwrap();
    assert_eq!(token.len(), 36);
    assert_eq!(&token[14..15], "4");
}
#[tokio::test]
async fn malformed_read_shapes_fail_cleanly() {
    let (base, requests) =
        login_server(vec![response("200 OK", "", r#"{"success":true}"#); 5]).await;
    let rest = Rest::for_test(&base).unwrap();
    assert!(rest.place_details("P1").await.is_err());
    assert!(rest.multiple_place_details(&["P1".into()]).await.is_err());
    assert!(rest.place_photos(&json!({"place_id":"P1"})).await.is_err());
    assert!(rest.place_metadata("P1", "key").await.unwrap().is_none());
    assert!(rest.geo_search("x").await.unwrap().is_empty());
    assert!(rest.place_photos(&json!({})).await.is_err());
    requests.await.unwrap();
}
#[test]
fn endpoints_dates_and_redaction_have_deterministic_boundaries() {
    for url in [
        "http://example.com",
        "http://localhost",
        "https://u:p@example.com",
        "https://example.com/#secret",
        "broken",
    ] {
        assert!(Rest::with_base_url("", url).is_err());
    }
    assert!(Rest::with_base_url("", "http://[::1]:1").is_ok());
    assert!(Rest::with_base_url("", "https://example.com/path").is_err());
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
    let mut headers = HeaderMap::new();
    headers.insert(
        "retry-after",
        HeaderValue::from_str(&httpdate::fmt_http_date(now + Duration::from_secs(15))).unwrap(),
    );
    assert_eq!(rate_limit_at(&headers, now).retry_in_seconds, 15);
    assert_eq!(
        rate_limit_at(&headers, now + Duration::from_secs(20)).retry_in_seconds,
        0
    );
    assert_eq!(
        redact("s%3Asecret s%253Asecret s:secret", &["s%3Asecret".into()]),
        "<redacted> <redacted> <redacted>"
    );
}
