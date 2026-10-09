use super::*;
use crate::test_support::{login_server, response};

fn offline_server(read_only: bool) -> WanderlogServer {
    WanderlogServer::new(|| Err(anyhow!("no Wanderlog session stored")), read_only)
}

#[tokio::test]
async fn forty_five_source_places_read_one_trip_and_make_no_place_requests() {
    let mut doc = crate::test_support::fixture::doc();
    doc["overallVersion"] = json!(12);
    doc["itinerary"]["sections"][1]["blocks"] = json!((1..=45).map(|id| json!({
            "id":id + 1000, "type":"place", "place":{"place_id":format!("P{id}"), "name":format!("Place {id}")},
            "imageKeys":[format!("photo{id}")], "text":{"ops":[{"insert":"Private source note\n"}]}
        })).collect::<Vec<_>>());
    let (base, requests) = login_server(vec![response(
        "200 OK",
        "",
        &json!({"success":true,"tripPlan":doc}).to_string(),
    )])
    .await;
    let server = WanderlogServer::new(|| Ok("dummy-cookie".into()), false);
    let mut session = server.session().await.unwrap();
    session.rest = Rest::with_base_url("", &base).unwrap();
    *server.state.session.lock().await = Some(session.clone());
    server.state.trips.lock().await.insert(
        1,
        TripSummary {
            id: 1,
            key: "test-source-key".into(),
            editable: false,
            title: "Source".into(),
            start_date: None,
            end_date: None,
            place_count: 45,
            edited_at: None,
            relation: "own",
        },
    );
    let edits: Vec<Edit> = (1..=45).map(|id| serde_json::from_value(json!({
            "op":"add_place", "section":"day:1", "source":{"trip_id":1,"block":format!("b:{}",id+1000),"revision":12}
        })).unwrap()).collect();
    let resolved = server.places_for(&session, &edits).await.unwrap();
    let plan = edit::plan(
        &crate::test_support::fixture::doc(),
        &edits,
        &PlanContext {
            user_id: 7,
            places: &resolved,
        },
    )
    .unwrap();
    assert_eq!(plan.components.len(), 45);
    assert_eq!(resolved.len(), 45);
    assert_eq!(session.places.lock().await.len(), 45);
    let cached = server
        .get_place_impl(PlaceArgs {
            place_id: "P1".into(),
            trip_id: None,
        })
        .await
        .unwrap();
    assert!(cached.contains("Place 1"));
    let requests = requests.await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET /api/tripPlans/test-source-key?clientSchemaVersion=2 "));
    assert!(
        plan.components
            .iter()
            .all(|op| op["li"]["text"] != doc["itinerary"]["sections"][1]["blocks"][0]["text"])
    );
}

#[tokio::test]
async fn rate_limit_results_distinguish_before_write_from_unreported_state() {
    let server = offline_server(false);
    let make = || {
        anyhow::Error::new(RateLimited {
            retry_after_seconds: None,
            retry_in_seconds: 60,
        })
    };
    let result = server
        .respond(Err(make().context(BeforeWrite("resolve_places"))))
        .await;
    assert_eq!(result.is_error, Some(true));
    let data = result.structured_content.unwrap();
    assert_eq!(data["code"], "RATE_LIMITED");
    assert_eq!(data["write_state"], "not_started");
    assert_eq!(data["stage"], "resolve_places");
    assert_eq!(data["retry_in_seconds"], 60);
    assert!(data["retry_after_seconds"].is_null());
    let result = server.respond(Err(make())).await;
    assert_eq!(
        result.structured_content.unwrap()["write_state"],
        "not_reported"
    );
}

#[tokio::test]
async fn changing_login_does_not_share_place_cache_with_inflight_old_reads() {
    let cookie = Arc::new(std::sync::Mutex::new("first".to_owned()));
    let shared = cookie.clone();
    let server = WanderlogServer::new(move || Ok(shared.lock().unwrap().clone()), true);
    let old = server.session().await.unwrap();
    *cookie.lock().unwrap() = "second".into();
    let new = server.session().await.unwrap();
    remember_places(
        &mut *old.places.lock().await,
        &crate::test_support::fixture::doc(),
    );
    assert!(!old.places.lock().await.is_empty());
    assert!(new.places.lock().await.is_empty());
}

#[test]
fn tool_schemas_are_flat_objects() {
    let tools = offline_server(false).tool_router.list_all();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(names.len(), 7, "{names:?}");
    for tool in &tools {
        let schema = serde_json::to_value(&tool.input_schema).unwrap();
        assert_eq!(schema["type"], "object", "{}", tool.name);
        let text = schema.to_string();
        assert!(
            !text.contains("$ref") && !text.contains("oneOf"),
            "{} schema not flat: {text}",
            tool.name
        );
    }
    let create = tools.iter().find(|t| t.name == "create_trip").unwrap();
    assert!(
        !serde_json::to_string(&create.input_schema)
            .unwrap()
            .contains("privacy")
    );
    let apply = tools.iter().find(|t| t.name == "apply_edits").unwrap();
    assert_eq!(
        apply.annotations.as_ref().unwrap().destructive_hint,
        Some(true)
    );
}

#[tokio::test]
async fn missing_auth_surfaces_as_tool_error() {
    let server = offline_server(false);
    let result = server.respond(server.list_trips_impl().await).await;
    assert_eq!(result.is_error, Some(true));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("no Wanderlog session stored"), "{text}");
}

#[test]
fn read_only_mode_hides_write_tools() {
    let names: Vec<String> = offline_server(true)
        .tool_router
        .list_all()
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    assert_eq!(names.len(), 5, "{names:?}");
    assert!(
        !names
            .iter()
            .any(|n| n == "apply_edits" || n == "create_trip")
    );
}

#[test]
fn blind_retries_are_refused() {
    let mut log = ApplyLog::default();
    assert!(log.check_retry(1, 42, None).is_ok());
    log.record(1, 42, Some(8));
    let err = log.check_retry(1, 42, None).unwrap_err().to_string();
    assert!(
        err.contains("identical batch") && err.contains("revision 8"),
        "{err}"
    );
    assert!(
        log.check_retry(1, 43, None).is_ok(),
        "a different batch is fine"
    );
    assert!(
        log.check_retry(1, 42, Some(8)).is_ok(),
        "base_revision proves a re-read"
    );
    log.record(2, 7, None);
    assert!(
        log.check_retry(2, 99, None)
            .unwrap_err()
            .to_string()
            .contains("unknown outcome")
    );
    assert!(log.check_retry(2, 99, Some(5)).is_ok());
    log.record(2, 99, Some(6));
    assert!(
        log.check_retry(2, 100, None).is_ok(),
        "a confirmed apply clears the unknown state"
    );
}

#[tokio::test]
async fn redaction_ignores_degenerate_keys_and_handles_case() {
    let server = offline_server(true);
    let summary = |id: u64, key: &str| crate::rest::TripSummary {
        id,
        key: key.into(),
        editable: true,
        title: String::new(),
        start_date: None,
        end_date: None,
        place_count: 0,
        edited_at: None,
        relation: "own",
    };
    server
        .state
        .trips
        .lock()
        .await
        .extend([(1, summary(1, "")), (2, summary(2, "abcdefghijklmnop"))]);
    let out = server
        .redact("key abcdefghijklmnop and HTTPS://WANDERLOG.COM/Plan/QWERTYUIOPASDFGH/x".into())
        .await;
    assert_eq!(out, "key <trip-key> and HTTPS://WANDERLOG.COM/Plan/<key>/x");
}

#[test]
fn share_links_are_masked() {
    let text = "see https://wanderlog.com/plan/abcdefghijklmnop/kyoto and wanderlog.com/view/qwertyuiop \
                    but keep wanderlog.com/plan/create/plan and wanderlog.com/home";
    let masked = redact_share_links(text);
    assert_eq!(
        masked,
        "see https://wanderlog.com/plan/<key>/kyoto and wanderlog.com/view/<key> \
             but keep wanderlog.com/plan/create/plan and wanderlog.com/home"
    );
}

#[test]
fn create_trip_validates_before_creating() {
    let args = |s: Option<&str>, e: Option<&str>, t: Option<&str>| CreateTripArgs {
        destination: "Kyoto".into(),
        start_date: s.map(Into::into),
        end_date: e.map(Into::into),
        title: t.map(Into::into),
    };
    assert!(create_trip_follow_up(&args(Some("2026-11-12"), Some("2026-11-10"), None)).is_err());
    assert!(create_trip_follow_up(&args(Some("2026-11-10"), None, None)).is_err());
    assert!(create_trip_follow_up(&args(None, None, Some("  "))).is_err());
    assert!(create_trip_follow_up(&args(Some("2026-01-01"), Some("2026-12-31"), None)).is_err());
    assert_eq!(
        create_trip_follow_up(&args(Some("2026-11-10"), Some("2026-11-12"), Some("Kyoto")))
            .unwrap()
            .len(),
        2
    );
}
