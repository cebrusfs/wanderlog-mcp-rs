use super::*;
use crate::test_support::{fixture, login_server, response, websocket_server};
fn ok(body: Value) -> String {
    response("200 OK", "", &body.to_string())
}
fn listing() -> Value {
    json!({"success":true,"ownTripPlans":[{"id":1,"key":"mock-trip-key","keyType":"edit","title":"Test trip","startDate":"2026-11-10","endDate":"2026-11-11","editedAt":"2026"}],"friendsTripPlans":[{"id":2,"key":"mock-view-key","title":"View"}]})
}
fn mock_server(base: String, ws: String, readonly: bool) -> WanderlogServer {
    WanderlogServer::with_transports(
        || Ok("synthetic-session".into()),
        readonly,
        TransportOptions {
            rest_base: base,
            websocket_base: ws,
            timeout: Duration::from_secs(1),
        },
    )
}
fn text(result: &CallToolResult) -> String {
    serde_json::to_string(&result.content).unwrap()
}
#[tokio::test]
async fn read_tools_execute_http_requests_cache_places_and_render_all_metadata() {
    let mut doc = fixture::doc();
    doc["overallVersion"] = json!(12);
    let payload = json!({"success":true,"tripPlan":doc,"resources":{"geo":{"latitude":35.0,"longitude":139.0,"bounds":[138.0,34.0,140.0,36.0]}}});
    let detail = json!({"place_id":"PX","name":"Test place","formatted_address":"Road","rating":4.5,"user_ratings_total":10,"types":["museum"],"business_status":"OPERATIONAL","opening_hours":{"weekday_text":["Monday: open",null]},"website":"https://example.test","international_phone_number":"000","url":"https://maps.example.test"});
    let(base,requests)=login_server(vec![ok(listing()),ok(payload),ok(json!({"success":true,"data":[{"place_id":"PX","structured_formatting":{"main_text":"Museum","secondary_text":"Tokyo"}},{"place_id":"PY","structured_formatting":{"main_text":"Main"}},{"place_id":"PZ","description":"Description"}]})),ok(json!({"success":true,"data":detail})),ok(json!({"success":true,"data":[{"generatedDescription":"Visit","minMinutesSpent":30,"maxMinutesSpent":60,"categories":["museum",null]}]})),ok(json!({"success":true,"data":[]})),ok(json!({"success":true,"user":{"id":7}})),ok(json!({"success":true,"ownTripPlans":[]}))]).await;
    let server = mock_server(base, "ws://127.0.0.1:1/".into(), true);
    assert!(text(&server.list_trips().await.unwrap()).contains("view-only"));
    assert!(
        text(
            &server
                .get_trip(Parameters(TripArgs {
                    trip_id: 1,
                    detail: Some(Detail::Full)
                }))
                .await
                .unwrap()
        )
        .contains("revision 12")
    );
    let result = server
        .search_places(Parameters(SearchArgs {
            query: " Museum ".into(),
            trip_id: Some(1),
        }))
        .await
        .unwrap();
    assert!(text(&result).contains("Tokyo"));
    let result = server
        .get_place(Parameters(PlaceArgs {
            place_id: "PX".into(),
            trip_id: Some(1),
        }))
        .await
        .unwrap();
    for expected in [
        "Road",
        "rating 4.5",
        "Monday",
        "website",
        "Visit",
        "30–60",
        "museum",
    ] {
        assert!(text(&result).contains(expected));
    }
    assert!(
        server
            .get_place_impl(PlaceArgs {
                place_id: "PX".into(),
                trip_id: None
            })
            .await
            .unwrap()
            .contains("Test place")
    );
    assert!(
        server
            .search_places_impl(SearchArgs {
                query: "x".into(),
                trip_id: None
            })
            .await
            .unwrap()
            .contains("No places")
    );
    assert_eq!(server.user_id().await.unwrap(), 7);
    assert_eq!(server.user_id().await.unwrap(), 7);
    assert!(server.list_trips_impl().await.unwrap().contains("No trips"));
    let requests = requests.await.unwrap();
    assert_eq!(requests.len(), 8);
}
#[tokio::test]
async fn missing_fields_auth_and_validation_fail_without_writes() {
    let (base, requests) = login_server(vec![
        ok(json!({"success":true,"user":null})),
        ok(json!({"success":true,"user":{}})),
        ok(listing()),
        ok(json!({"success":true})),
        ok(listing()),
    ])
    .await;
    let server = mock_server(base, "ws://127.0.0.1:1/".into(), false);
    assert!(
        server
            .user_id()
            .await
            .unwrap_err()
            .to_string()
            .contains("not logged in")
    );
    assert!(
        server
            .user_id()
            .await
            .unwrap_err()
            .to_string()
            .contains("without id")
    );
    assert!(
        server
            .get_trip_impl(TripArgs {
                trip_id: 1,
                detail: None
            })
            .await
            .unwrap_err()
            .to_string()
            .contains("tripPlan")
    );
    assert!(server.editable_trip(2).await.is_err());
    assert!(server.trip(999).await.is_err());
    assert!(
        server
            .search_places_impl(SearchArgs {
                query: " ".into(),
                trip_id: None
            })
            .await
            .is_err()
    );
    assert!(
        server
            .get_place_impl(PlaceArgs {
                place_id: " ".into(),
                trip_id: None
            })
            .await
            .is_err()
    );
    requests.await.unwrap();
    let ro = WanderlogServer::new(|| panic!("read-only write must not load credentials"), true);
    let args = EditsArgs {
        trip_id: 1,
        edits: vec![],
        base_revision: None,
    };
    assert!(
        ro.apply_edits(Parameters(args))
            .await
            .unwrap()
            .is_error
            .unwrap()
    );
    assert!(
        ro.create_trip(Parameters(CreateTripArgs {
            destination: "X".into(),
            title: None,
            start_date: None,
            end_date: None
        }))
        .await
        .unwrap()
        .is_error
        .unwrap()
    );
}
async fn seed(server: &WanderlogServer) {
    server.session().await.unwrap();
    server.state.session.lock().await.as_mut().unwrap().user_id = Some(7);
    let t = TripSummary {
        id: 1,
        key: "mock-trip-key".into(),
        editable: true,
        title: "Test trip".into(),
        start_date: None,
        end_date: None,
        place_count: 0,
        edited_at: None,
        relation: "own",
    };
    server.state.trips.lock().await.insert(1, t);
}
fn edits(title: &str, revision: Option<u64>) -> EditsArgs {
    EditsArgs {
        trip_id: 1,
        edits: vec![serde_json::from_value(json!({"op":"rename_trip","title":title})).unwrap()],
        base_revision: revision,
    }
}
#[tokio::test]
async fn preview_atomic_apply_revision_conflict_noop_and_duplicate_guard() {
    let (ws, frames) = websocket_server(
        fixture::doc(),
        vec![
            (false, None),
            (false, None),
            (false, Some(json!({"a":"op","v":12}))),
            (false, None),
        ],
    )
    .await;
    let server = mock_server("http://127.0.0.1:1".into(), ws, false);
    seed(&server).await;
    let preview = server
        .preview_edits(Parameters(edits("New", None)))
        .await
        .unwrap();
    assert!(text(&preview).contains("nothing written"));
    let conflict = server
        .apply_edits(Parameters(edits("New", Some(11))))
        .await
        .unwrap();
    assert_eq!(
        conflict.structured_content.unwrap()["code"],
        "REVISION_CONFLICT"
    );
    let applied = server
        .apply_edits(Parameters(edits("New", Some(12))))
        .await
        .unwrap();
    assert!(text(&applied).contains("revision 13"));
    let duplicate = server
        .apply_edits(Parameters(edits("New", None)))
        .await
        .unwrap();
    assert!(text(&duplicate).contains("identical batch"));
    assert!(
        server
            .edits_impl(edits("Test trip", Some(12)), true)
            .await
            .unwrap()
            .contains("Nothing to change")
    );
    let ops = frames.await.unwrap();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0]["op"].as_array().unwrap().len(), 1);
    assert_eq!(ops[0]["v"], 12);
}
#[tokio::test]
async fn unknown_write_blocks_new_batches_and_explicit_rejection_does_not() {
    let (ws, frames) = websocket_server(
        fixture::doc(),
        vec![
            (false, None),
            (false, Some(json!({"a":"op","error":"rejected"}))),
        ],
    )
    .await;
    let server = mock_server("http://127.0.0.1:1".into(), ws, false);
    seed(&server).await;
    let unknown = server
        .apply_edits(Parameters(edits("New", Some(12))))
        .await
        .unwrap();
    assert_eq!(
        unknown.structured_content.unwrap()["write_state"],
        "unknown"
    );
    assert!(
        server
            .edits_impl(edits("Different", None), true)
            .await
            .unwrap_err()
            .to_string()
            .contains("unknown outcome")
    );
    let rejected = server
        .edits_impl(edits("Different", Some(12)), true)
        .await
        .unwrap_err();
    assert!(rejected.downcast_ref::<OutcomeUnknown>().is_none());
    assert!(rejected.to_string().contains("nothing applied"));
    assert_eq!(frames.await.unwrap().len(), 2);
}
#[tokio::test]
async fn subscription_retries_once_before_any_op() {
    let (ws, frames) = websocket_server(fixture::doc(), vec![(true, None), (false, None)]).await;
    let server = mock_server("http://127.0.0.1:1".into(), ws, true);
    let (conn, snapshot) = server
        .connect("synthetic-session", "mock-trip-key")
        .await
        .unwrap();
    assert_eq!(snapshot.version, 12);
    conn.close().await;
    assert!(frames.await.unwrap().is_empty());
}
#[tokio::test]
async fn creation_with_followup_and_late_failure_names_existing_trip() {
    let (ws, frames) = websocket_server(
        fixture::doc(),
        vec![(false, Some(json!({"a":"op","v":12})))],
    )
    .await;
    let (base, requests) = login_server(vec![
        ok(json!({"success":true,"data":[{"id":5,"name":"Tokyo"}]})),
        ok(json!({"success":true,"data":{"id":1,"key":"mock-trip-key"}})),
        ok(listing()),
        ok(json!({"success":true,"user":{"id":7}})),
        ok(json!({"success":true,"tripPlan":fixture::doc()})),
    ])
    .await;
    let server = mock_server(base, ws, false);
    assert!(
        server
            .create_trip_impl(CreateTripArgs {
                destination: " Tokyo ".into(),
                title: Some("New title".into()),
                start_date: Some("2026-11-10".into()),
                end_date: Some("2026-11-11".into())
            })
            .await
            .unwrap()
            .contains("Created trip 1")
    );
    assert_eq!(frames.await.unwrap().len(), 1);
    let requests = requests.await.unwrap();
    assert!(requests[1].starts_with("POST /api/tripPlans "));
    assert!(requests[1].contains("\"privacy\":\"friends\""));
    let (base, requests) = login_server(vec![
        ok(json!({"success":true,"data":[{"id":5}]})),
        ok(json!({"success":true,"data":{"id":1,"key":"mock-trip-key"}})),
        response("503 Service Unavailable", "", r#"{"success":false}"#),
    ])
    .await;
    let server = mock_server(base, "ws://127.0.0.1:1/".into(), false);
    let err = server
        .create_trip_impl(CreateTripArgs {
            destination: "Tokyo".into(),
            title: None,
            start_date: None,
            end_date: None,
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("trip 1 was created"));
    assert_eq!(requests.await.unwrap().len(), 3);
}
#[tokio::test]
async fn creation_invalid_destinations_geo_bounds_and_retry_expiry() {
    let (base, requests) = login_server(vec![
        ok(json!({"success":true,"data":[]})),
        ok(json!({"success":true,"data":[{}]})),
    ])
    .await;
    let server = mock_server(base, "ws://127.0.0.1:1/".into(), false);
    for destination in ["", "No match", "Missing id"] {
        assert!(
            server
                .create_trip_impl(CreateTripArgs {
                    destination: destination.into(),
                    title: None,
                    start_date: None,
                    end_date: None
                })
                .await
                .is_err()
        );
    }
    requests.await.unwrap();
    server
        .remember_geo(
            1,
            &json!({"resources":{"geo":{"latitude":0,"longitude":0}}}),
        )
        .await;
    assert_eq!(server.state.geo.lock().await[&1].2, 50_000.0);
    server
        .remember_geo(2, &json!({"resources":{"geo":{"latitude":0}}}))
        .await;
    assert!(!server.state.geo.lock().await.contains_key(&2));
    server
        .remember_geo(
            1,
            &json!({"resources":{"geo":{"latitude":0,"longitude":0,"bounds":[0,0,0,0]}}}),
        )
        .await;
    assert_eq!(server.state.geo.lock().await[&1].2, 5_000.0);
    let now = Instant::now();
    let mut log = ApplyLog::default();
    log.recent.insert((1, 42), (now, Some(8)));
    assert!(log.check_retry_at(1, 42, None, now).is_err());
    assert!(
        log.check_retry_at(1, 42, None, now + DUPLICATE_WINDOW)
            .is_ok()
    );
    log.uncertain.insert(1);
    assert!(
        log.check_retry_at(1, 43, None, now + DUPLICATE_WINDOW * 2)
            .is_err()
    );
    assert_eq!(encode_key("a/b?"), "a%2Fb%3F");
}

#[tokio::test]
async fn encoded_secrets_and_login_scoped_geo_are_redacted_or_cleared() {
    let secret = Arc::new(std::sync::Mutex::new("s%3Amock-session".to_owned()));
    let shared = secret.clone();
    let server = WanderlogServer::new(move || Ok(shared.lock().unwrap().clone()), true);
    server.session().await.unwrap();
    server
        .state
        .geo
        .lock()
        .await
        .insert(1, (35.0, 139.0, 5000.0));
    assert_eq!(
        server
            .redact("s%3Amock-session s%253Amock-session s:mock-session".into())
            .await,
        "<session> <session> <session>"
    );
    *secret.lock().unwrap() = "other-session".into();
    server.session().await.unwrap();
    assert!(server.state.geo.lock().await.is_empty());
}
