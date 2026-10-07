//! Live round-trip against a real, throwaway trip. Ignored by default; run with
//! `WANDERLOG_E2E_TRIP_ID=<id> mise run e2e` (uses the stored session). Exercises every edit op
//! family against Wanderlog in three atomic batches, verifying each through a fresh connection,
//! and restores the trip at the end. The trip needs dates and at least one day.

use std::collections::HashMap;

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use wanderlog_mcp::edit::{self, Edit, PlanContext};
use wanderlog_mcp::sharedb::{Connection, Snapshot};
use wanderlog_mcp::{auth, dates, rest::Rest, trip};

struct Live {
    cookie: String,
    key: String,
    rest: Rest,
    user_id: u64,
}

impl Live {
    async fn snapshot(&self) -> Result<Snapshot> {
        let mut conn = Connection::open(&self.cookie, &self.key).await?;
        let snap = conn.subscribe().await;
        conn.close().await;
        snap
    }

    /// Plan `edits` on a fresh snapshot, submit them as one op, return the new snapshot.
    async fn apply(&self, edits: Vec<Value>) -> Result<Snapshot> {
        let edits: Vec<Edit> = edits
            .into_iter()
            .map(serde_json::from_value)
            .collect::<Result<_, _>>()?;
        let places = edit::prefetch_places(&self.rest, &edits, &HashMap::new()).await?;
        let mut conn = Connection::open(&self.cookie, &self.key).await?;
        let snap = conn.subscribe().await?;
        let plan = edit::plan(
            &snap.doc,
            &edits,
            &PlanContext {
                user_id: self.user_id,
                places: &places,
            },
        )?;
        let applied = conn.submit(snap.version, &plan.components).await;
        conn.close().await;
        applied?;
        self.snapshot().await
    }
}

fn find_block(doc: &Value, pred: impl Fn(&Value) -> bool) -> Option<(usize, &Value)> {
    trip::sections(doc)
        .iter()
        .enumerate()
        .find_map(|(si, s)| trip::blocks(s).iter().find(|b| pred(b)).map(|b| (si, b)))
}

fn section_ids(doc: &Value) -> Vec<u64> {
    trip::sections(doc).iter().filter_map(trip::id_of).collect()
}

#[tokio::test]
#[ignore = "talks to wanderlog.com; needs WANDERLOG_E2E_TRIP_ID and a stored session"]
async fn live_round_trip_restores_the_trip() -> Result<()> {
    let trip_id: u64 = std::env::var("WANDERLOG_E2E_TRIP_ID")
        .context("set WANDERLOG_E2E_TRIP_ID to a throwaway trip id")?
        .parse()?;
    let (cookie, _) = auth::load()?;
    let rest = Rest::new(&cookie)?;
    let user = rest
        .current_user()
        .await?
        .context("stored session is not logged in")?;
    let summary = rest
        .trips()
        .await?
        .into_iter()
        .find(|t| t.id == trip_id)
        .context("trip not in this account")?;
    ensure!(summary.editable, "trip is view-only");
    let live = Live {
        user_id: user["id"].as_u64().context("user without id")?,
        cookie,
        key: summary.key.clone(),
        rest,
    };

    let before = live.snapshot().await?;
    let title = before.doc["title"]
        .as_str()
        .context("trip without title")?
        .to_owned();
    let start = before.doc["startDate"]
        .as_str()
        .context("trip needs dates")?
        .to_owned();
    let end = before.doc["endDate"]
        .as_str()
        .context("trip needs dates")?
        .to_owned();
    let day_index = *trip::day_indices(&before.doc)
        .first()
        .context("trip needs a day")?;
    let day = &trip::sections(&before.doc)[day_index];
    let day_ref = format!("s:{}", trip::id_of(day).context("day without id")?);
    let day_heading = day["heading"].as_str().unwrap_or("").to_owned();
    let marker = format!("e2e-{}", std::process::id());
    let place_id = live
        .rest
        .autocomplete("Tokyo Tower", None)
        .await?
        .first()
        .and_then(|p| p["place_id"].as_str().map(str::to_owned))
        .context("place search returned nothing")?;

    // Batch 1: list + place with times, checklist, headings, title, one extra day.
    let after = live
        .apply(vec![
            json!({"op": "add_list", "heading": marker}),
            json!({"op": "add_place", "section": marker, "place_id": place_id, "start_time": "10:00", "end_time": "11:00", "text": marker}),
            json!({"op": "add_checklist", "section": day_ref, "heading": marker, "items": ["a", "b"]}),
            json!({"op": "update_section", "section": day_ref, "heading": format!("{marker} day")}),
            json!({"op": "rename_trip", "title": format!("{title} {marker}")}),
            json!({"op": "set_dates", "start_date": start, "end_date": dates::add(&end, 1)?}),
        ])
        .await?;
    ensure!(
        after.doc["title"] == json!(format!("{title} {marker}")),
        "title not renamed"
    );
    ensure!(
        after.doc["endDate"] == json!(dates::add(&end, 1)?),
        "dates not extended"
    );
    let (_, place) = find_block(&after.doc, |b| {
        b["text"].get("ops").is_some() && trip::delta_text(&b["text"]) == marker
    })
    .context("added place missing")?;
    ensure!(
        place["startTime"] == "10:00" && place["endTime"] == "11:00",
        "times not set"
    );
    ensure!(
        place["imageKeys"].as_array().is_some_and(|k| !k.is_empty()),
        "imageKeys missing"
    );
    let place_ref = format!("b:{}", trip::id_of(place).context("place id")?);
    let (_, checklist) =
        find_block(&after.doc, |b| b["title"] == json!(marker)).context("checklist missing")?;
    let checklist_ref = format!("b:{}", trip::id_of(checklist).context("checklist id")?);

    // Batch 2: append to the note, change the end time, move the place to the top of the day.
    let after = live
        .apply(vec![
            json!({"op": "update_block", "block": place_ref, "text": "more", "text_mode": "append", "end_time": "12:00"}),
            json!({"op": "move_block", "block": place_ref, "section": day_ref, "position": 1}),
        ])
        .await?;
    let day_now = trip::resolve_section(&after.doc, &day_ref)?;
    let first = &trip::blocks(&trip::sections(&after.doc)[day_now])[0];
    ensure!(
        format!("b:{}", trip::id_of(first).unwrap_or(0)) == place_ref,
        "place not moved to the top of the day"
    );
    ensure!(first["endTime"] == "12:00", "end time not updated");
    ensure!(
        trip::delta_text(&first["text"]) == format!("{marker}\nmore"),
        "note not appended"
    );

    // Batch 3: restore everything.
    let restored = live
        .apply(vec![
            json!({"op": "remove_block", "block": place_ref}),
            json!({"op": "remove_block", "block": checklist_ref}),
            json!({"op": "remove_section", "section": marker}),
            json!({"op": "update_section", "section": day_ref, "heading": day_heading}),
            json!({"op": "rename_trip", "title": title}),
            json!({"op": "set_dates", "start_date": start, "end_date": end}),
        ])
        .await?;
    ensure!(restored.doc["title"] == json!(title), "title not restored");
    ensure!(
        restored.doc["endDate"] == json!(end) && restored.doc["days"] == before.doc["days"],
        "dates not restored"
    );
    ensure!(
        section_ids(&restored.doc) == section_ids(&before.doc),
        "sections differ after restore"
    );
    ensure!(
        trip::sections(&restored.doc)
            .iter()
            .zip(trip::sections(&before.doc))
            .all(|(a, b)| a["blocks"] == b["blocks"] && a["heading"] == b["heading"]),
        "section contents differ after restore"
    );
    Ok(())
}
