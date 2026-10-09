//! Read helpers over a Wanderlog trip document (the ShareDB `TripPlans` doc).
//!
//! The document stays raw JSON so fields we do not model survive edits untouched. Sections and
//! blocks are addressed by their stable numeric ids (`[s:<id>]`, `[b:<id>]`), never by cached
//! indices: reservations insert new sections near the top, shifting every index after them.

use std::collections::HashSet;

use anyhow::{Result, anyhow, bail};
use serde_json::Value;

use crate::json0::utf16_len;

pub fn sections(doc: &Value) -> &[Value] {
    doc.pointer("/itinerary/sections")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

pub fn blocks(section: &Value) -> &[Value] {
    section
        .get("blocks")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

pub fn id_of(v: &Value) -> Option<u64> {
    v.get("id").and_then(Value::as_u64)
}

pub fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

pub fn is_day(section: &Value) -> bool {
    str_of(section, "mode") == Some("dayPlan")
}

/// A user place list ("Places to visit", custom lists).
pub fn is_list(section: &Value) -> bool {
    str_of(section, "mode") == Some("placeList") && str_of(section, "type") == Some("normal")
}

/// Lists and days hold places/notes/checklists; notes, lodging, flight and transit sections do not.
pub fn holds_items(section: &Value) -> bool {
    str_of(section, "type") == Some("normal")
}

/// Indices of day sections in document order.
pub fn day_indices(doc: &Value) -> Vec<usize> {
    sections(doc)
        .iter()
        .enumerate()
        .filter(|(_, s)| is_day(s))
        .map(|(i, _)| i)
        .collect()
}

/// 1-based day number of the section at `index`, if it is a day.
pub fn day_number(doc: &Value, index: usize) -> Option<usize> {
    day_indices(doc)
        .iter()
        .position(|&i| i == index)
        .map(|n| n + 1)
}

fn parse_ref(selector: &str, prefix: &str) -> Option<u64> {
    let s = selector
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']');
    s.strip_prefix(prefix).unwrap_or(s).trim().parse().ok()
}

/// Human label for a section, used in summaries and errors.
pub fn section_label(doc: &Value, index: usize) -> String {
    let section = &sections(doc)[index];
    let heading = str_of(section, "heading").unwrap_or("").trim();
    if is_day(section) {
        let date = crate::render::date_value(str_of(section, "date").unwrap_or("?"));
        let day = day_number(doc, index).unwrap_or(0);
        return format!("Day {day} ({date})");
    }
    // Headings are written by tripmates: quote them like any other trip content.
    match str_of(section, "type").unwrap_or("") {
        "textOnly" => "Notes".to_owned(),
        "hotels" => "Lodging".to_owned(),
        "flights" => "Flights".to_owned(),
        "transit" => "Transit".to_owned(),
        "normal" if heading.is_empty() => "untitled list".to_owned(),
        "normal" => format!("list {}", crate::render::quote(heading, Default::default())),
        other => format!("{} section", crate::render::field(other)),
    }
}

/// Resolve a section selector: `s:<id>`, a day date `YYYY-MM-DD`, `day:<n>`, or an exact heading.
pub fn resolve_section(doc: &Value, selector: &str) -> Result<usize> {
    let secs = sections(doc);
    let wanted = selector
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim();
    let by_id = |id: u64| secs.iter().position(|s| id_of(s) == Some(id));
    if let Some(rest) = wanted.strip_prefix("s:") {
        let id: u64 = rest
            .trim()
            .parse()
            .map_err(|_| anyhow!("bad section ref {selector:?}"))?;
        return by_id(id).ok_or_else(|| anyhow!("no section [s:{id}] in this trip"));
    }
    if crate::dates::parse(wanted).is_ok() {
        return secs
            .iter()
            .position(|s| is_day(s) && str_of(s, "date") == Some(wanted))
            .ok_or_else(|| anyhow!("this trip has no day {wanted}"));
    }
    let lower = wanted.to_lowercase();
    let days = day_indices(doc);
    let day_of = |n: usize| n.checked_sub(1).and_then(|k| days.get(k).copied());
    // `day:<n>` is unambiguous; `Day <n>` could also be a list's heading.
    if let Some(n) = lower
        .strip_prefix("day:")
        .and_then(|r| r.trim().parse::<usize>().ok())
    {
        return day_of(n).ok_or_else(|| anyhow!("this trip has {} days; no day {n}", days.len()));
    }
    let day_n = lower
        .strip_prefix("day ")
        .and_then(|r| r.trim().parse::<usize>().ok());
    let mut candidates: Vec<usize> = secs
        .iter()
        .enumerate()
        .filter(|(_, s)| str_of(s, "heading").is_some_and(|h| h.trim().to_lowercase() == lower))
        .map(|(i, _)| i)
        .collect();
    // A bare number counts as a section id only when such a section exists.
    for extra in [
        day_n.and_then(day_of),
        wanted.parse::<u64>().ok().and_then(by_id),
    ]
    .into_iter()
    .flatten()
    {
        if !candidates.contains(&extra) {
            candidates.push(extra);
        }
    }
    match candidates.as_slice() {
        [one] => Ok(*one),
        [] => match day_n {
            Some(n) => bail!("this trip has {} days; no day {n}", days.len()),
            None => bail!("no section matches {wanted:?}; use an [s:<id>] ref from get_trip"),
        },
        many => bail!(
            "{wanted:?} matches {} sections ({}); use an [s:<id>] ref",
            many.len(),
            many.iter()
                .filter_map(|&i| id_of(&secs[i]).map(|id| format!("s:{id}")))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Resolve a block selector `b:<id>` to (section index, block index).
pub fn resolve_block(doc: &Value, selector: &str) -> Result<(usize, usize)> {
    let id = parse_ref(selector, "b:")
        .ok_or_else(|| anyhow!("bad block ref {selector:?}; expected b:<id>"))?;
    for (si, section) in sections(doc).iter().enumerate() {
        if let Some(bi) = blocks(section).iter().position(|b| id_of(b) == Some(id)) {
            return Ok((si, bi));
        }
    }
    bail!(
        "no item [b:{id}] in this trip (it may have been moved or removed; re-read with get_trip)"
    )
}

/// Every numeric `id` inside the itinerary, so new ids never collide.
pub fn collect_ids(doc: &Value) -> HashSet<u64> {
    fn walk(v: &Value, out: &mut HashSet<u64>) {
        match v {
            Value::Object(map) => {
                if let Some(id) = map.get("id").and_then(Value::as_u64) {
                    out.insert(id);
                }
                map.values().for_each(|child| walk(child, out));
            }
            Value::Array(list) => list.iter().for_each(|child| walk(child, out)),
            _ => {}
        }
    }
    let mut ids = HashSet::new();
    if let Some(itinerary) = doc.get("itinerary") {
        walk(itinerary, &mut ids);
    }
    ids
}

/// Plain text of a Quill document (embeds shown as `[image]`), without the final newline.
pub fn delta_text(delta: &Value) -> String {
    let mut text = String::new();
    for op in delta
        .get("ops")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match op.get("insert") {
            Some(Value::String(s)) => text.push_str(s),
            Some(_) => text.push_str("[image]"),
            None => {}
        }
    }
    text.trim_end_matches('\n').to_owned()
}

/// Quill length of a document (UTF-16 units; embeds count 1), including the final newline.
pub fn delta_len(delta: &Value) -> usize {
    delta
        .get("ops")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|op| match op.get("insert") {
            Some(Value::String(s)) => utf16_len(s),
            Some(_) => 1,
            None => 0,
        })
        .sum()
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod fixture {
    use serde_json::{Value, json};

    /// A small synthetic trip shaped like captured Wanderlog documents (no real data).
    pub fn doc() -> Value {
        json!({
            "title": "Test trip",
            "startDate": "2026-11-10", "endDate": "2026-11-11", "days": 2,
            "itinerary": {
                "options": {},
                "budget": {"amount": {"amount": 0, "currencyCode": "USD"}, "expenses": [], "payments": [], "simplifyDebt": false},
                "journal": {"stops": [], "summary": ""},
                "sections": [
                    {"heading": "Notes", "text": {"ops": [{"insert": "Bring cash\n"}]}, "blocks": [],
                     "placeMarkerColor": "#000000", "placeMarkerIcon": "map-marker", "id": 100, "type": "textOnly", "mode": "placeList"},
                    {"heading": "Places to visit", "text": {"ops": [{"insert": "\n"}]}, "placeMarkerColor": "#3f52e3",
                     "placeMarkerIcon": "map-marker", "id": 101, "type": "normal", "mode": "placeList", "date": null,
                     "blocks": [{"id": 1, "type": "place", "place": {"name": "Sensō-ji", "place_id": "P1", "rating": 4.5, "types": ["tourist_attraction"]},
                                 "text": {"ops": [{"insert": "\n"}]}, "addedBy": {"type": "user", "userId": 7}, "imageSize": "small",
                                 "upvotedBy": [], "travelMode": null, "attachments": []}]},
                    {"heading": "", "text": {"ops": [{"insert": "\n"}]}, "blocks": [], "placeMarkerColor": "#46cdcf",
                     "placeMarkerIcon": "map-marker", "id": 102, "type": "normal", "mode": "dayPlan", "date": "2026-11-10"},
                    {"heading": "Museums", "text": {"ops": [{"insert": "\n"}]}, "placeMarkerColor": "#7045af",
                     "placeMarkerIcon": "map-marker", "id": 103, "type": "normal", "mode": "dayPlan", "date": "2026-11-11",
                     "blocks": [
                        {"id": 2, "type": "place", "place": {"name": "Tokyo Tower", "place_id": "P2"}, "text": {"ops": [{"insert": "Go at sunset.\n"}]},
                         "startTime": "17:00", "endTime": null, "addedBy": {"type": "user", "userId": 7}, "imageSize": "small",
                         "upvotedBy": [], "travelMode": null, "attachments": []},
                        {"id": 3, "type": "note", "text": {"ops": [{"insert": "Buy tickets\n"}]}, "addedBy": {"type": "user", "userId": 7}, "attachments": []}
                     ]}
                ]
            }
        })
    }
}

#[cfg(test)]
mod tests {
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
}
