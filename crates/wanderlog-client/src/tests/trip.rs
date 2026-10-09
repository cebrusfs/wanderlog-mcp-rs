use super::*;
use serde_json::json;

#[test]
fn resolves_selectors() {
    let doc = fixture::doc();
    assert_eq!(resolve_section(&doc, "s:101").unwrap(), 1);
    assert_eq!(resolve_section(&doc, "[s:103]").unwrap(), 3);
    assert_eq!(resolve_section(&doc, "2026-11-10").unwrap(), 2);
    assert_eq!(resolve_section(&doc, "day:2").unwrap(), 3);
    assert_eq!(resolve_section(&doc, "Day 1").unwrap(), 2);
    assert_eq!(resolve_section(&doc, "places to visit").unwrap(), 1);
    assert!(resolve_section(&doc, "2026-11-12").is_err());
    assert!(resolve_section(&doc, "day:3").is_err());
    assert_eq!(resolve_block(&doc, "b:3").unwrap(), (3, 1));
    assert!(resolve_block(&doc, "b:99").is_err());
    assert_eq!(section_label(&doc, 3), "Day 2 (2026-11-11)");
    // A list named like a day selector is ambiguous unless `day:<n>` or an id is used.
    let mut named = fixture::doc();
    named["itinerary"]["sections"][1]["heading"] = json!("Day 1");
    assert!(
        resolve_section(&named, "Day 1")
            .unwrap_err()
            .to_string()
            .contains("matches 2 sections")
    );
    assert_eq!(resolve_section(&named, "day:1").unwrap(), 2);
    assert_eq!(resolve_section(&doc, "101").unwrap(), 1);
    assert!(resolve_section(&doc, "999").is_err());
}

#[test]
fn ids_and_text() {
    let doc = fixture::doc();
    let ids = collect_ids(&doc);
    assert!(ids.contains(&100) && ids.contains(&3) && !ids.contains(&7));
    let text = &sections(&doc)[3]["blocks"][0]["text"];
    assert_eq!(delta_text(text), "Go at sunset.");
    assert_eq!(delta_len(text), 14);
}
