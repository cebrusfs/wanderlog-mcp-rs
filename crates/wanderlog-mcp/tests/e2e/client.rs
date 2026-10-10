//! Library round trip on a new trip: every edit op family in three atomic batches, each verified
//! through a fresh connection. A failure here points at the Wanderlog protocol, not at the tools.

use std::collections::HashMap;

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use wanderlog_mcp::edit::{self, Edit, PlanContext};
use wanderlog_mcp::sharedb::{Connection, Snapshot};
use wanderlog_mcp::{dates, trip};

use crate::{Account, END, START};

struct Live<'a> {
    account: &'a Account,
    key: String,
}

impl Live<'_> {
    async fn snapshot(&self) -> Result<Snapshot> {
        let mut conn = Connection::open(&self.account.cookie, &self.key).await?;
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
        let places = edit::prefetch_places(
            &self.account.rest,
            &edits,
            &mut HashMap::new(),
            HashMap::new(),
        )
        .await?;
        let mut conn = Connection::open(&self.account.cookie, &self.key).await?;
        let snap = conn.subscribe().await?;
        let plan = edit::plan(
            &snap.doc,
            &edits,
            &PlanContext {
                user_id: self.account.user_id,
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
#[ignore = "talks to wanderlog.com with the stored session"]
async fn edit_ops_round_trip() -> Result<()> {
    let account = Account::load().await?;
    let geo_id = account
        .rest
        .geo_search("Tokyo")
        .await?
        .first()
        .and_then(|g| g["id"].as_u64())
        .context("destination search returned nothing")?;
    let created = account.rest.create_trip(geo_id, "friends").await?;
    let key = created["key"]
        .as_str()
        .context("trip without key")?
        .to_owned();
    let live = Live {
        account: &account,
        key: key.clone(),
    };
    account.delete_after(&key, round_trip(&live)).await
}

async fn round_trip(live: &Live<'_>) -> Result<()> {
    let title = crate::title();
    let (start, end) = (START, END);
    let before = live
        .apply(vec![
            json!({"op": "set_dates", "start_date": start, "end_date": end}),
            json!({"op": "rename_trip", "title": title}),
        ])
        .await?;
    ensure!(
        before.doc["title"] == json!(title) && before.doc["endDate"] == json!(end),
        "new trip not set up"
    );
    let day_index = *trip::day_indices(&before.doc)
        .first()
        .context("trip needs a day")?;
    let day = &trip::sections(&before.doc)[day_index];
    let day_ref = format!("s:{}", trip::id_of(day).context("day without id")?);
    let day_heading = day["heading"].as_str().unwrap_or("").to_owned();
    let marker = format!("e2e-{}", std::process::id());
    let place_id = live
        .account
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
            json!({"op": "add_place", "section": marker, "place_id": place_id, "include_photos": true, "start_time": "10:00", "end_time": "11:00", "text": marker}),
            json!({"op": "add_checklist", "section": day_ref, "heading": marker, "items": ["a", "b"]}),
            json!({"op": "update_section", "section": day_ref, "heading": format!("{marker} day")}),
            json!({"op": "rename_trip", "title": format!("{title} {marker}")}),
            json!({"op": "set_dates", "start_date": start, "end_date": dates::add(end, 1)?}),
        ])
        .await?;
    ensure!(
        after.doc["title"] == json!(format!("{title} {marker}")),
        "title not renamed"
    );
    ensure!(
        after.doc["endDate"] == json!(dates::add(end, 1)?),
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

    // Batch 3: undo batches 1 and 2.
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
