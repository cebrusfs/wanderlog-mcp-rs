use super::*;
use crate::rest::tests::{login_server, response};
use crate::trip::{fixture, is_day};

fn day_dates(doc: &Value) -> Vec<String> {
    sections(doc)
        .iter()
        .filter(|s| is_day(s))
        .filter_map(|s| str_of(s, "date").map(str::to_owned))
        .collect()
}

fn run(doc: &Value, edits: Vec<Value>) -> Result<Plan> {
    let mut places = HashMap::new();
    places.insert(
        PlaceKey::Id("P9".to_owned()),
        PlaceInfo {
            details: json!({"name": "Skytree", "place_id": "P9"}),
            image_keys: vec!["k1".into()],
            photos_loaded: true,
        },
    );
    let edits: Vec<Edit> = edits
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()?;
    plan(
        doc,
        &edits,
        &PlanContext {
            user_id: 7,
            places: &places,
        },
    )
}

fn place_edits(ids: &[&str]) -> Vec<Edit> {
    ids.iter()
        .map(|id| {
            serde_json::from_value(json!({"op":"add_place", "section":"day:1", "place_id":id}))
                .unwrap()
        })
        .collect()
}

#[tokio::test]
async fn partial_batches_survive_rate_limiting_and_retry_only_missing_ids() {
    let first: Vec<Value> = (1..=5)
        .map(|id| json!({"place_id":format!("P{id}"), "name":"Place"}))
        .collect();
    let (base, requests) = login_server(vec![
        response(
            "200 OK",
            "",
            &json!({"success":true,"data":first}).to_string(),
        ),
        response(
            "429 Too Many Requests",
            "Retry-After: 0\r\n",
            "<html>limited</html>",
        ),
        response(
            "200 OK",
            "",
            r#"{"success":true,"data":[{"place_id":"P6","name":"Six"}]}"#,
        ),
    ])
    .await;
    let rest = Rest::for_test(&base).unwrap();
    let edits = place_edits(&["P1", "P2", "P3", "P4", "P5", "P6", "P1"]);
    let mut cache = HashMap::new();
    let error = prefetch_places(&rest, &edits, &mut cache, HashMap::new())
        .await
        .unwrap_err();
    assert!(error.downcast_ref::<crate::rest::RateLimited>().is_some());
    assert_eq!(cache.len(), 5);
    let resolved = prefetch_places(&rest, &edits, &mut cache, HashMap::new())
        .await
        .unwrap();
    assert_eq!(resolved.len(), 6);
    let requests = requests.await.unwrap();
    assert_eq!(requests.len(), 3);
    assert!(
        requests
            .iter()
            .all(|request| request.starts_with("GET /api/placesAPI/getMultiplePlaceDetails?"))
    );
    assert_eq!(requests[0].matches("placeIds%5B%5D=").count(), 5);
    for request in &requests[1..] {
        assert!(request.contains("placeIds%5B%5D=P6"));
        assert!(!request.contains("=P1"));
    }
}

#[tokio::test]
async fn missing_bulk_entries_keep_successes_without_silent_fallback() {
    let (base, requests) = login_server(vec![response("200 OK", "",
            r#"{"success":true,"data":[{"place_id":"P1","name":"One"},null,{"place_id":"P2"},{"place_id":"UNREQUESTED","name":"Other"}]}"#)]).await;
    let mut cache = HashMap::new();
    let error = prefetch_places(
        &Rest::for_test(&base).unwrap(),
        &place_edits(&["P1", "P2"]),
        &mut cache,
        HashMap::new(),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("missing for P2"));
    assert_eq!(cache.len(), 1);
    assert!(cache.contains_key("P1"));
    assert_eq!(requests.await.unwrap().len(), 1);
}

#[tokio::test]
async fn photos_are_explicit_and_retained_for_preview_apply_reuse() {
    let (base, requests) = login_server(vec![
        response(
            "200 OK",
            "",
            r#"{"success":true,"data":[{"place_id":"P1","name":"One"}]}"#,
        ),
        response("200 OK", "", r#"{"success":true,"data":["photo-key"]}"#),
    ])
    .await;
    let rest = Rest::for_test(&base).unwrap();
    let mut cache = HashMap::new();
    let mut edits = place_edits(&["P1"]);
    prefetch_places(&rest, &edits, &mut cache, HashMap::new())
        .await
        .unwrap();
    assert!(!cache["P1"].photos_loaded);
    edits[0].include_photos = Some(true);
    prefetch_places(&rest, &edits, &mut cache, HashMap::new())
        .await
        .unwrap();
    prefetch_places(&rest, &edits, &mut cache, HashMap::new())
        .await
        .unwrap();
    assert_eq!(cache["P1"].image_keys, ["photo-key"]);
    let requests = requests.await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].starts_with("POST /api/placePhotos/P1 "));
}

#[tokio::test]
async fn source_photos_survive_a_later_failure_and_fresh_source_reads() {
    let (base, requests) = login_server(vec![
        response("200 OK", "", r#"{"success":true,"data":["photo-one"]}"#),
        response("429 Too Many Requests", "Retry-After: 0\r\n", "limited"),
        response("200 OK", "", r#"{"success":true,"data":["photo-two"]}"#),
    ])
    .await;
    let rest = Rest::for_test(&base).unwrap();
    let mut cache = HashMap::new();
    let mut sources = HashMap::new();
    let edits: Vec<Edit> = (1..=2).map(|id| {
            let edit: Edit = serde_json::from_value(json!({"op":"add_place","section":"day:1","source":{"trip_id":1,"block":format!("b:{id}")},"include_photos":true})).unwrap();
            let info = PlaceInfo::from_block(&json!({"type":"place","place":{"place_id":format!("P{id}"),"name":"Place"}})).unwrap();
            cache.insert(format!("P{id}"), info.clone());
            sources.insert(edit.place_key().unwrap(), info);
            edit
        }).collect();
    assert!(
        prefetch_places(&rest, &edits, &mut cache, sources.clone())
            .await
            .is_err()
    );
    assert!(cache["P1"].photos_loaded);
    assert!(!cache["P2"].photos_loaded);
    prefetch_places(&rest, &edits, &mut cache, sources.clone())
        .await
        .unwrap();
    let reused = prefetch_places(&rest, &edits, &mut cache, sources)
        .await
        .unwrap();
    assert!(reused.values().all(|info| info.photos_loaded));
    let requests = requests.await.unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests[0].starts_with("POST /api/placePhotos/P1 "));
    assert!(
        requests[1..]
            .iter()
            .all(|request| request.starts_with("POST /api/placePhotos/P2 "))
    );
}

#[tokio::test]
async fn malformed_photo_results_keep_details_and_do_not_mark_photos_loaded() {
    for data in [json!(null), json!(["valid", 123])] {
        let (base, requests) = login_server(vec![response(
            "200 OK",
            "",
            &json!({"success":true,"data":data}).to_string(),
        )])
        .await;
        let mut cache = HashMap::from([(
            "P1".to_owned(),
            PlaceInfo {
                details: json!({"place_id":"P1","name":"One"}),
                image_keys: Vec::new(),
                photos_loaded: false,
            },
        )]);
        let mut edits = place_edits(&["P1"]);
        edits[0].include_photos = Some(true);
        let error = prefetch_places(
            &Rest::for_test(&base).unwrap(),
            &edits,
            &mut cache,
            HashMap::new(),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("place photos:"));
        assert_eq!(cache.len(), 1);
        assert!(!cache["P1"].photos_loaded);
        assert!(cache["P1"].image_keys.is_empty());
        assert_eq!(requests.await.unwrap().len(), 1);
    }
}

#[test]
fn source_places_preserve_place_data_but_not_private_block_fields() {
    let mut doc = fixture::doc();
    doc["overallVersion"] = json!(8);
    let block = &mut doc["itinerary"]["sections"][1]["blocks"][0];
    block["imageKeys"] = json!(["photo"]);
    block["attachments"] = json!([{"secret":"reservation"}]);
    let original = block.clone();
    let source = PlaceSource {
        trip_id: 1,
        block: format!("b:{}", original["id"]),
        revision: Some(8),
    };
    let info = source_place(&doc, &source).unwrap();
    let edit: Edit = serde_json::from_value(json!({"op":"add_place", "section":"day:1", "source":{"trip_id":1,"block":source.block,"revision":8}, "text":"New note"})).unwrap();
    let places = HashMap::from([(PlaceKey::Source(source.clone()), info)]);
    let planned = plan(
        &doc,
        &[edit],
        &PlanContext {
            user_id: 7,
            places: &places,
        },
    )
    .unwrap();
    let new = &planned.components[0]["li"];
    assert_eq!(new["place"], original["place"]);
    assert_eq!(new["imageKeys"], original["imageKeys"]);
    assert_ne!(new["id"], original["id"]);
    assert_eq!(new["attachments"], json!([]));
    assert_eq!(delta_text(&new["text"]), "New note");
    assert!(
        source_place(
            &doc,
            &PlaceSource {
                revision: Some(7),
                ..source.clone()
            }
        )
        .is_err()
    );
    assert!(
        source_place(
            &doc,
            &PlaceSource {
                block: "b:9999999".into(),
                ..source
            }
        )
        .is_err()
    );
    for bad in [
        json!({"op":"add_place","section":"day:1"}),
        json!({"op":"add_place","section":"day:1","place_id":"P1","source":{"trip_id":1,"block":"b:1"}}),
        json!({"op":"add_note","section":"day:1","text":"note","include_photos":true}),
    ] {
        assert!(check_batch(&[serde_json::from_value(bad).unwrap()]).is_err());
    }
}

#[test]
fn add_place_matches_web_client_block_shape() {
    let doc = fixture::doc();
    let plan = run(&doc, vec![json!({"op": "add_place", "section": "day:2", "place_id": "P9",
                                         "position": 1, "start_time": "9:30", "text": "Book ahead"})]).unwrap();
    let c = &plan.components[0];
    assert_eq!(c["p"], json!(["itinerary", "sections", 3, "blocks", 0]));
    let block = &c["li"];
    let mut keys: Vec<&str> = block
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "addedBy",
            "attachments",
            "endTime",
            "id",
            "imageKeys",
            "imageSize",
            "place",
            "startTime",
            "text",
            "travelMode",
            "type",
            "upvotedBy"
        ]
    );
    assert_eq!(block["startTime"], "09:30");
    assert_eq!(block["text"], json!({"ops": [{"insert": "Book ahead\n"}]}));
    assert_eq!(block["addedBy"], json!({"type": "user", "userId": 7}));
    assert!(block["id"].as_u64().unwrap() < 1_000_000_000);
    assert!(plan.summary[0].contains("«Skytree» → Day 2 (2026-11-11) #1"));
}

#[test]
fn batch_sees_earlier_edits() {
    let doc = fixture::doc();
    let plan = run(
        &doc,
        vec![
            json!({"op": "add_list", "heading": "Food"}),
            json!({"op": "add_note", "section": "Food", "text": "ramen"}),
        ],
    )
    .unwrap();
    // New list lands after the last user list ("Places to visit" at 1) → index 2.
    assert_eq!(plan.components[0]["p"], json!(["itinerary", "sections", 2]));
    assert_eq!(plan.components[0]["li"]["mode"], "placeList");
    assert!(plan.components[0]["li"].get("date").is_none());
    assert_eq!(
        plan.components[1]["p"],
        json!(["itinerary", "sections", 2, "blocks", 0])
    );
    assert_eq!(sections(&plan.after)[2]["blocks"][0]["type"], "note");
}

#[test]
fn moves_use_lm_within_and_ld_li_across_sections() {
    let doc = fixture::doc();
    let within = run(
        &doc,
        vec![json!({"op": "move_block", "block": "b:3", "section": "day:2", "position": 1})],
    )
    .unwrap();
    assert_eq!(
        within.components,
        vec![json!({"p": ["itinerary", "sections", 3, "blocks", 1], "lm": 0})]
    );
    let across = run(
        &doc,
        vec![json!({"op": "move_block", "block": "b:2", "section": "s:101"})],
    )
    .unwrap();
    assert_eq!(
        across.components[0]["p"],
        json!(["itinerary", "sections", 3, "blocks", 0])
    );
    assert_eq!(across.components[0]["ld"]["id"], 2);
    assert_eq!(
        across.components[1]["p"],
        json!(["itinerary", "sections", 1, "blocks", 1])
    );
    assert_eq!(across.components[1]["li"]["id"], 2, "block id is preserved");
    assert!(
        run(
            &doc,
            vec![json!({"op": "move_block", "block": "b:2", "section": "s:100"})]
        )
        .is_err()
    );
}

#[test]
fn update_block_note_time_and_validation() {
    let doc = fixture::doc();
    let plan = run(
        &doc,
        vec![
            json!({"op": "update_block", "block": "b:2", "text": "Bring a camera",
                                         "text_mode": "append", "end_time": "19:00"}),
        ],
    )
    .unwrap();
    assert_eq!(
        plan.components[0],
        json!({"p": ["itinerary", "sections", 3, "blocks", 0, "text"], "t": "rich-text",
                                              "o": [{"retain": 14}, {"insert": "Bring a camera\n"}]})
    );
    assert_eq!(
        plan.components[1],
        json!({"p": ["itinerary", "sections", 3, "blocks", 0, "endTime"], "od": null, "oi": "19:00"})
    );
    assert_eq!(
        trip::delta_text(&sections(&plan.after)[3]["blocks"][0]["text"]),
        "Go at sunset.\nBring a camera"
    );
    let replace = run(
        &doc,
        vec![json!({"op": "update_block", "block": "b:2", "text": "New"})],
    )
    .unwrap();
    assert_eq!(
        replace.components[0]["o"],
        json!([{"insert": "New"}, {"delete": 13}])
    );
    assert!(
        run(
            &doc,
            vec![json!({"op": "update_block", "block": "b:3", "start_time": "10:00"})]
        )
        .is_err()
    );
    assert!(
        run(
            &doc,
            vec![json!({"op": "update_block", "block": "b:2", "start_time": "25:00"})]
        )
        .is_err()
    );
    assert!(
        run(
            &doc,
            vec![json!({"op": "update_block", "block": "b:2", "bogus": 1})]
        )
        .is_err()
    );
}

#[test]
fn review_fixes_guard_edge_cases() {
    let doc = fixture::doc();
    // Unchanged text is a no-op line, not a batch-aborting error.
    let same = run(
        &doc,
        vec![json!({"op": "update_block", "block": "b:2", "text": "Go at sunset."})],
    )
    .unwrap();
    assert!(same.components.is_empty());
    assert!(same.summary[0].contains("note unchanged"));
    assert!(run(&doc, vec![json!({"op": "update_block", "block": "b:2"})]).is_err());
    // Reservations are read-only.
    let mut with_hotel = fixture::doc();
    with_hotel["itinerary"]["sections"].as_array_mut().unwrap().insert(1, json!({
            "heading": "Hotels and lodging", "text": {"ops": [{"insert": "\n"}]}, "id": 200, "type": "hotels",
            "mode": "placeList", "blocks": [{"id": 9, "type": "place", "place": {"name": "Hotel"}, "hotel": {"checkIn": "2026-11-10"}}]}));
    let err = run(
        &with_hotel,
        vec![json!({"op": "remove_block", "block": "b:9"})],
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("Lodging"), "{err:#}");
    // A list that still has notes text is not dropped silently.
    let mut noted = fixture::doc();
    noted["itinerary"]["sections"][1]["blocks"] = json!([]);
    noted["itinerary"]["sections"][1]["text"] = json!({"ops": [{"insert": "keep me\n"}]});
    assert!(
        run(
            &noted,
            vec![json!({"op": "remove_section", "section": "s:101"})]
        )
        .is_err()
    );
    // A null title is replaced as a value (text0 needs a string).
    let mut untitled = fixture::doc();
    untitled["title"] = Value::Null;
    let plan = run(
        &untitled,
        vec![json!({"op": "rename_trip", "title": "Kyoto"})],
    )
    .unwrap();
    assert_eq!(
        plan.components[0],
        json!({"p": ["title"], "od": null, "oi": "Kyoto"})
    );
    // Append keeps the last line's format (stored on its newline) and adds a plain line.
    let mut bulleted = fixture::doc();
    bulleted["itinerary"]["sections"][3]["blocks"][1]["text"] =
        json!({"ops": [{"insert": "item"}, {"insert": "\n", "attributes": {"list": "bullet"}}]});
    let plan = run(
        &bulleted,
        vec![json!({"op": "update_block", "block": "b:3", "text": "more", "text_mode": "append"})],
    )
    .unwrap();
    let ops = &sections(&plan.after)[3]["blocks"][1]["text"]["ops"];
    assert_eq!(
        *ops,
        json!([{"insert": "item"}, {"insert": "\n", "attributes": {"list": "bullet"}}, {"insert": "more\n"}])
    );
}

#[test]
fn reservations_are_read_only_and_days_with_headings_are_kept() {
    let mut doc = fixture::doc();
    doc["itinerary"]["sections"].as_array_mut().unwrap().insert(1, json!({
            "heading": "Hotels and lodging", "text": {"ops": [{"insert": "\n"}]}, "id": 200, "type": "hotels",
            "mode": "placeList", "blocks": [{"id": 9, "type": "place", "place": {"name": "Hotel"},
                                             "text": {"ops": [{"insert": "\n"}]}, "hotel": {"checkIn": "2026-11-10"}}]}));
    for edit in [
        json!({"op": "update_block", "block": "b:9", "text": "x"}),
        json!({"op": "update_block", "block": "b:9", "start_time": "10:00"}),
        json!({"op": "update_section", "section": "s:200", "text": "x"}),
    ] {
        let err = run(&doc, vec![edit.clone()]).unwrap_err();
        assert!(format!("{err:#}").contains("read-only"), "{edit}: {err:#}");
    }
    assert!(
        run(
            &doc,
            vec![json!({"op": "update_section", "section": "s:100", "text": "trip notes"})]
        )
        .is_ok()
    );
    let mut headed = fixture::doc();
    headed["itinerary"]["sections"][3]["blocks"] = json!([]);
    headed["itinerary"]["sections"][3]["heading"] = json!("Museums");
    assert!(
        run(
            &headed,
            vec![json!({"op": "set_dates", "start_date": "2026-11-10", "end_date": "2026-11-10"})]
        )
        .is_err()
    );
    let mut linked = fixture::doc();
    linked["itinerary"]["budget"]["expenses"] =
        json!([{"id": 50, "blockId": 2, "amount": {"amount": 1, "currencyCode": "JPY"}}]);
    let plan = run(&linked, vec![json!({"op": "remove_block", "block": "b:2"})]).unwrap();
    assert!(
        plan.summary[0].contains("1 budget expense(s) stay"),
        "{}",
        plan.summary[0]
    );
}

#[test]
fn sections_title_and_removals() {
    let doc = fixture::doc();
    let plan = run(
        &doc,
        vec![
            json!({"op": "update_section", "section": "day:1", "heading": "Arrival"}),
            json!({"op": "rename_trip", "title": "Tokyo 2026"}),
            json!({"op": "remove_block", "block": "b:1"}),
            json!({"op": "remove_section", "section": "Places to visit"}),
        ],
    )
    .unwrap();
    assert_eq!(
        plan.components[0],
        json!({"p": ["itinerary", "sections", 2, "heading"], "od": "", "oi": "Arrival"})
    );
    assert_eq!(
        plan.components[1]["o"],
        json!([{"p": 0, "d": "Test trip"}, {"p": 0, "i": "Tokyo 2026"}])
    );
    assert_eq!(plan.after["title"], "Tokyo 2026");
    assert_eq!(sections(&plan.after).len(), 3);
    assert!(
        run(
            &doc,
            vec![json!({"op": "remove_section", "section": "Places to visit"})]
        )
        .is_err(),
        "non-empty list"
    );
}

#[test]
fn set_dates_shift_extend_and_guarded_shrink() {
    let doc = fixture::doc();
    let plan = run(
        &doc,
        vec![json!({"op": "set_dates", "start_date": "2026-11-11", "end_date": "2026-11-13"})],
    )
    .unwrap();
    assert_eq!(
        day_dates(&plan.after),
        ["2026-11-11", "2026-11-12", "2026-11-13"]
    );
    assert_eq!(plan.after["days"], 3);
    assert_eq!(
        plan.components[0],
        json!({"p": ["startDate"], "od": "2026-11-10", "oi": "2026-11-11"})
    );
    // Day 2 holds items, so shrinking to one day must refuse instead of deleting them.
    assert!(
        run(
            &doc,
            vec![json!({"op": "set_dates", "start_date": "2026-11-10", "end_date": "2026-11-10"})]
        )
        .is_err()
    );
    let mut dateless = fixture::doc();
    dateless["itinerary"]["sections"]
        .as_array_mut()
        .unwrap()
        .truncate(2);
    for key in ["startDate", "endDate", "days"] {
        dateless.as_object_mut().unwrap().remove(key);
    }
    let plan = run(
        &dateless,
        vec![json!({"op": "set_dates", "start_date": "2026-11-10", "end_date": "2026-11-11"})],
    )
    .unwrap();
    assert_eq!(
        plan.components[0],
        json!({"p": ["startDate"], "od": null, "oi": "2026-11-10"})
    );
    assert_eq!(day_dates(&plan.after), ["2026-11-10", "2026-11-11"]);
}
#[test]
fn seeded_ids_are_reproducible_and_failed_batches_leave_input_untouched() {
    use rand::SeedableRng;
    let doc = trip::fixture::doc();
    let edits: Vec<Edit> = vec![
        json!({"op":"add_note","section":"day:1","text":"A"}),
        json!({"op":"add_list","heading":"List"}),
    ]
    .into_iter()
    .map(|v| serde_json::from_value(v).unwrap())
    .collect();
    let places = HashMap::new();
    let ctx = PlanContext {
        user_id: 7,
        places: &places,
    };
    let mut a = rand::rngs::StdRng::seed_from_u64(42);
    let mut b = rand::rngs::StdRng::seed_from_u64(42);
    let first = plan_with_rng(&doc, &edits, &ctx, &mut a).unwrap();
    let second = plan_with_rng(&doc, &edits, &ctx, &mut b).unwrap();
    assert_eq!(first.components, second.components);
    assert_eq!(first.after, second.after);
    let mut invalid = edits.clone();
    invalid.push(serde_json::from_value(json!({"op":"remove_block","block":"b:999999"})).unwrap());
    assert!(plan_with_rng(&doc, &invalid, &ctx, &mut a).is_err());
    assert_eq!(doc, trip::fixture::doc());
}
#[test]
fn checklists_in_section_moves_and_updates_generate_valid_atomic_ops() {
    let doc = trip::fixture::doc();
    let added=run(&doc,vec![json!({"op":"add_checklist","section":"day:1","heading":"Pack","items":["Passport","Camera"]})]).unwrap();
    let block = &added.after["itinerary"]["sections"][2]["blocks"][0];
    let id = block["id"].as_u64().unwrap();
    assert_eq!(block["items"].as_array().unwrap().len(), 2);
    assert_eq!(block["items"][0]["checked"], false);
    assert!(added.summary[0].contains("checklist"));
    let updated = run(
        &added.after,
        vec![
            json!({"op":"update_block","block":format!("b:{id}"),"heading":"Travel"}),
            json!({"op":"move_block","block":"b:2","section":"day:2","position":2}),
            json!({"op":"update_block","block":"b:2","start_time":"18:00","end_time":"19:00"}),
        ],
    )
    .unwrap();
    assert_eq!(
        updated.after["itinerary"]["sections"][2]["blocks"][0]["title"],
        "Travel"
    );
    assert_eq!(
        updated.after["itinerary"]["sections"][3]["blocks"][1]["id"],
        2
    );
    assert_eq!(
        updated.after["itinerary"]["sections"][3]["blocks"][1]["endTime"],
        "19:00"
    );
    let noop = run(
        &doc,
        vec![json!({"op":"move_block","block":"b:3","section":"day:2"})],
    )
    .unwrap();
    assert!(noop.components.is_empty());
    let cleared = run(
        &doc,
        vec![json!({"op":"update_block","block":"b:2","start_time":"","end_time":""})],
    )
    .unwrap();
    assert!(cleared.summary[0].contains("times cleared"));
    assert!(
        run(
            &doc,
            vec![json!({"op":"add_checklist","section":"day:1","items":[]})]
        )
        .is_err()
    );
    assert!(
        run(
            &doc,
            vec![json!({"op":"move_block","block":"b:2","section":"day:2","position":0})]
        )
        .is_err()
    );
}
#[test]
fn section_notes_missing_fields_and_readonly_reservations_are_checked() {
    let mut doc = trip::fixture::doc();
    let edits = vec![
        json!({"op":"update_section","section":"s:100","text":"Bring cash"}),
        json!({"op":"update_block","block":"b:3","text":"Buy tickets"}),
        json!({"op":"update_section","section":"day:2","heading":"Museums","text":"Visit","text_mode":"append"}),
    ];
    let plan = run(&doc, edits).unwrap();
    assert!(plan.summary[0].contains("unchanged"));
    assert!(plan.summary[1].contains("unchanged"));
    doc["itinerary"]["sections"][2]
        .as_object_mut()
        .unwrap()
        .remove("text");
    let plan = run(
        &doc,
        vec![json!({"op":"update_section","section":"day:1","text":"New"})],
    )
    .unwrap();
    assert_eq!(
        trip::delta_text(&plan.after["itinerary"]["sections"][2]["text"]),
        "New"
    );
    doc["itinerary"]["sections"][2]["type"] = json!("hotels");
    for op in [
        json!({"op":"update_section","section":"s:102","heading":"X"}),
        json!({"op":"move_block","block":"b:2","section":"s:102"}),
        json!({"op":"update_block","block":"b:2","heading":"X"}),
    ] {
        assert!(run(&doc, vec![op]).is_err());
    }
    let mut doc = trip::fixture::doc();
    doc["itinerary"]["sections"] = json!([]);
    assert_eq!(
        run(&doc, vec![json!({"op":"add_list","heading":"First"})])
            .unwrap()
            .after["itinerary"]["sections"][0]["heading"],
        "First"
    );
    assert!(
        check_batch(&vec![
            serde_json::from_value::<Edit>(
                json!({"op":"rename_trip","title":"X"})
            )
            .unwrap();
            MAX_EDITS + 1
        ])
        .is_err()
    );
}
