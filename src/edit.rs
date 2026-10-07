//! High-level itinerary edits → json0 components.
//!
//! Component shapes mirror what the official web client sends (captured 2026-10-06; see
//! docs/protocol.md). Edits run in order against a working copy, so each edit sees the effects of
//! earlier ones (e.g. add a list, then add places to it by heading). The whole batch is submitted
//! as ONE ShareDB op, which the server applies atomically.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, anyhow, bail, ensure};
use futures_util::{StreamExt, TryStreamExt, stream};
use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::render::{self, Options};
use crate::rest::Rest;
use crate::trip::{self, blocks, delta_text, id_of, sections, str_of};
use crate::{dates, json0};

pub const MAX_EDITS: usize = 100;
pub const MAX_TRIP_DAYS: i64 = 90;
/// Marker colours seen on sections created by the web client; cycled for new sections.
const MARKER_COLORS: [&str; 8] = [
    "#46cdcf", "#7045af", "#3498db", "#f75940", "#ec9b3b", "#3f52e3", "#e23e57", "#2ecc71",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(inline)]
pub enum EditOp {
    AddPlace,
    AddNote,
    AddChecklist,
    UpdateBlock,
    MoveBlock,
    RemoveBlock,
    AddList,
    UpdateSection,
    RemoveSection,
    RenameTrip,
    SetDates,
}

impl EditOp {
    pub fn name(self) -> &'static str {
        match self {
            Self::AddPlace => "add_place",
            Self::AddNote => "add_note",
            Self::AddChecklist => "add_checklist",
            Self::UpdateBlock => "update_block",
            Self::MoveBlock => "move_block",
            Self::RemoveBlock => "remove_block",
            Self::AddList => "add_list",
            Self::UpdateSection => "update_section",
            Self::RemoveSection => "remove_section",
            Self::RenameTrip => "rename_trip",
            Self::SetDates => "set_dates",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(inline)]
pub enum TextMode {
    #[default]
    Replace,
    Append,
}

/// One itinerary edit. Fields used per op:
/// add_place{section, place_id, position?, text?, start_time?, end_time?};
/// add_note{section, text, position?}; add_checklist{section, items, heading?, position?};
/// update_block{block, text?, text_mode?, start_time?, end_time?, heading? (checklist title)};
/// move_block{block, section, position?}; remove_block{block};
/// add_list{heading}; update_section{section, heading?, text?, text_mode?};
/// remove_section{section} (empty lists only); rename_trip{title}; set_dates{start_date, end_date}.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct Edit {
    /// The operation to perform.
    pub op: EditOp,
    /// Target section: an `s:<id>` ref from get_trip, a day date `YYYY-MM-DD`, `day:<n>` (1-based), or a list's exact heading.
    pub section: Option<String>,
    /// Target item: a `b:<id>` ref from get_trip.
    pub block: Option<String>,
    /// add_place: Google place_id returned by search_places. Never invent one.
    pub place_id: Option<String>,
    /// Plain-text note (add_place/add_note/update_block) or section text (update_section).
    pub text: Option<String>,
    /// How `text` updates an existing note: `replace` (default) or `append` as a new line.
    pub text_mode: Option<TextMode>,
    /// Start time `HH:MM` (24h) for a place; empty string clears both times.
    pub start_time: Option<String>,
    /// End time `HH:MM` (24h) for a place; requires a start time; empty string clears it.
    pub end_time: Option<String>,
    /// 1-based item position inside the target section (default: append at the end).
    pub position: Option<u32>,
    /// Section heading (add_list/update_section; day subheading for days) or checklist title.
    pub heading: Option<String>,
    /// add_checklist: item texts.
    pub items: Option<Vec<String>>,
    /// rename_trip: new trip title.
    pub title: Option<String>,
    /// set_dates: first day `YYYY-MM-DD`.
    pub start_date: Option<String>,
    /// set_dates: last day `YYYY-MM-DD` (inclusive).
    pub end_date: Option<String>,
}

/// Place data fetched before planning (REST), keyed by place_id.
#[derive(Debug, Clone)]
pub struct PlaceInfo {
    pub details: Value,
    pub image_keys: Vec<String>,
}

pub struct PlanContext<'a> {
    pub user_id: u64,
    pub places: &'a HashMap<String, PlaceInfo>,
}

#[derive(Debug)]
pub struct Plan {
    /// json0 components to submit as one op.
    pub components: Vec<Value>,
    /// One human-readable line per edit.
    pub summary: Vec<String>,
    /// The document after all edits (for previews).
    pub after: Value,
    /// Ids of sections touched, in first-touch order.
    pub touched: Vec<u64>,
}

/// Reject empty or oversized batches before any network work.
pub fn check_batch(edits: &[Edit]) -> Result<()> {
    ensure!(!edits.is_empty(), "no edits given");
    ensure!(
        edits.len() <= MAX_EDITS,
        "at most {MAX_EDITS} edits per batch"
    );
    Ok(())
}

/// Place data for every add_place target: entries already in `known` are reused, the rest are
/// fetched four at a time (details, plus photo keys best effort like the web client).
pub async fn prefetch_places(
    rest: &Rest,
    edits: &[Edit],
    known: &HashMap<String, PlaceInfo>,
) -> Result<HashMap<String, PlaceInfo>> {
    check_batch(edits)?;
    let mut wanted: Vec<String> = Vec::new();
    for edit in edits.iter().filter(|e| e.op == EditOp::AddPlace) {
        let place_id = edit.place_id.as_deref().map(str::trim).unwrap_or_default();
        if !place_id.is_empty() && !wanted.iter().any(|w| w == place_id) {
            wanted.push(place_id.to_owned());
        }
    }
    let mut places: HashMap<String, PlaceInfo> = wanted
        .iter()
        .filter_map(|id| known.get(id).map(|info| (id.clone(), info.clone())))
        .collect();
    let missing: Vec<String> = wanted
        .into_iter()
        .filter(|id| !places.contains_key(id))
        .collect();
    let fetched: Vec<(String, PlaceInfo)> = stream::iter(missing)
        .map(|place_id| async move {
            let details = rest
                .place_details(&place_id)
                .await
                .with_context(|| format!("look up place_id {place_id}"))?;
            let image_keys = rest.place_photos(&details).await.unwrap_or_default();
            Ok::<_, anyhow::Error>((
                place_id,
                PlaceInfo {
                    details,
                    image_keys,
                },
            ))
        })
        .buffer_unordered(4)
        .try_collect()
        .await?;
    places.extend(fetched);
    Ok(places)
}

/// Plan a batch of edits against `doc`.
pub fn plan(doc: &Value, edits: &[Edit], ctx: &PlanContext<'_>) -> Result<Plan> {
    check_batch(edits)?;
    let mut ids = trip::collect_ids(doc);
    let mut work = doc.clone();
    let mut plan = Plan {
        components: Vec::new(),
        summary: Vec::new(),
        after: Value::Null,
        touched: Vec::new(),
    };
    for (n, edit) in edits.iter().enumerate() {
        let mut step = Step {
            doc: work,
            components: Vec::new(),
            touched: Vec::new(),
        };
        let summary = step
            .run(edit, ctx, &mut ids)
            .with_context(|| format!("edit {} ({})", n + 1, edit.op.name()))?;
        for id in step.touched {
            if !plan.touched.contains(&id) {
                plan.touched.push(id);
            }
        }
        plan.components.extend(step.components);
        plan.summary.push(summary);
        work = step.doc;
    }
    plan.after = work;
    Ok(plan)
}

/// Builds the components of one edit, applying each to its working copy as it goes so later
/// indices are computed against the current state.
struct Step {
    doc: Value,
    components: Vec<Value>,
    touched: Vec<u64>,
}

impl Step {
    fn push(&mut self, component: Value) -> Result<()> {
        json0::apply(&mut self.doc, &component)
            .context("internal: generated component does not apply")?;
        self.components.push(component);
        Ok(())
    }

    fn touch(&mut self, index: usize) {
        if let Some(id) = sections(&self.doc).get(index).and_then(id_of)
            && !self.touched.contains(&id)
        {
            self.touched.push(id);
        }
    }

    fn section(&self, index: usize) -> &Value {
        &sections(&self.doc)[index]
    }

    fn label(&self, index: usize) -> String {
        trip::section_label(&self.doc, index)
    }

    fn run(
        &mut self,
        edit: &Edit,
        ctx: &PlanContext<'_>,
        ids: &mut HashSet<u64>,
    ) -> Result<String> {
        match edit.op {
            EditOp::AddPlace => self.add_place(edit, ctx, ids),
            EditOp::AddNote => self.add_note(edit, ctx, ids),
            EditOp::AddChecklist => self.add_checklist(edit, ctx, ids),
            EditOp::UpdateBlock => self.update_block(edit),
            EditOp::MoveBlock => self.move_block(edit),
            EditOp::RemoveBlock => self.remove_block(edit),
            EditOp::AddList => self.add_list(edit, ids),
            EditOp::UpdateSection => self.update_section(edit),
            EditOp::RemoveSection => self.remove_section(edit),
            EditOp::RenameTrip => self.rename_trip(edit),
            EditOp::SetDates => self.set_dates(edit, ids),
        }
    }

    /// Resolve `section` and require that it can hold places/notes/checklists.
    fn item_section(&self, edit: &Edit) -> Result<usize> {
        let index = trip::resolve_section(&self.doc, required(&edit.section, "section")?)?;
        ensure!(
            trip::holds_items(self.section(index)),
            "{} cannot hold places or notes; pick a list or a day",
            self.label(index)
        );
        Ok(index)
    }

    fn insert_block(
        &mut self,
        section: usize,
        position: Option<u32>,
        block: Value,
    ) -> Result<usize> {
        let at = insert_index(position, blocks(self.section(section)).len())?;
        self.push(json!({"p": ["itinerary", "sections", section, "blocks", at], "li": block}))?;
        self.touch(section);
        Ok(at)
    }

    fn add_place(
        &mut self,
        edit: &Edit,
        ctx: &PlanContext<'_>,
        ids: &mut HashSet<u64>,
    ) -> Result<String> {
        let section = self.item_section(edit)?;
        let place_id = required(&edit.place_id, "place_id")?.trim();
        let info = ctx
            .places
            .get(place_id)
            .ok_or_else(|| anyhow!("place_id {place_id} could not be looked up"))?;
        let id = new_id(ids);
        let mut block = json!({
            "id": id, "type": "place", "place": info.details,
            "text": text_doc(edit.text.as_deref()),
            "addedBy": added_by(ctx), "imageSize": "small", "upvotedBy": [], "travelMode": null, "attachments": [],
        });
        if !info.image_keys.is_empty() {
            block["imageKeys"] = json!(info.image_keys);
        }
        let (start, end) = times(edit, None, None)?;
        if let Some(start) = start {
            block["startTime"] = json!(start);
            block["endTime"] = json!(end);
        }
        let at = self.insert_block(section, edit.position, block)?;
        let name = str_of(&info.details, "name").unwrap_or(place_id);
        Ok(format!(
            "+ place {} → {} #{} [b:{id}]",
            q(name),
            self.label(section),
            at + 1
        ))
    }

    fn add_note(
        &mut self,
        edit: &Edit,
        ctx: &PlanContext<'_>,
        ids: &mut HashSet<u64>,
    ) -> Result<String> {
        let section = self.item_section(edit)?;
        let text = required(&edit.text, "text")?;
        ensure!(!text.trim().is_empty(), "text must not be empty");
        let id = new_id(ids);
        let block = json!({"id": id, "type": "note", "text": json0::rich_text_doc(text),
                           "addedBy": added_by(ctx), "attachments": []});
        let at = self.insert_block(section, edit.position, block)?;
        Ok(format!(
            "+ note {} → {} #{} [b:{id}]",
            q(text),
            self.label(section),
            at + 1
        ))
    }

    fn add_checklist(
        &mut self,
        edit: &Edit,
        ctx: &PlanContext<'_>,
        ids: &mut HashSet<u64>,
    ) -> Result<String> {
        let section = self.item_section(edit)?;
        let texts = edit.items.as_deref().unwrap_or_default();
        ensure!(!texts.is_empty(), "items must list at least one entry");
        let items: Vec<Value> = texts
            .iter()
            .map(|t| json!({"id": new_id(ids), "checked": false, "text": json0::rich_text_doc(t)}))
            .collect();
        let id = new_id(ids);
        let title = edit.heading.as_deref().unwrap_or("");
        let block = json!({"id": id, "type": "checklist", "items": items, "attachments": [],
                           "addedBy": added_by(ctx), "title": title});
        let at = self.insert_block(section, edit.position, block)?;
        Ok(format!(
            "+ checklist {} ({} items) → {} #{} [b:{id}]",
            q(title),
            texts.len(),
            self.label(section),
            at + 1
        ))
    }

    fn update_block(&mut self, edit: &Edit) -> Result<String> {
        let (s, b) = trip::resolve_block(&self.doc, required(&edit.block, "block")?)?;
        let block = blocks(self.section(s))[b].clone();
        let kind = str_of(&block, "type").unwrap_or("?").to_owned();
        ensure!(
            matches!(kind.as_str(), "place" | "note" | "checklist")
                && trip::holds_items(self.section(s)),
            "only places, notes and checklists in lists or days can be edited (this is {} in {}); reservations are read-only",
            render::field(&kind),
            self.label(s)
        );
        let path = json!(["itinerary", "sections", s, "blocks", b]);
        ensure!(
            edit.text.is_some()
                || edit.start_time.is_some()
                || edit.end_time.is_some()
                || edit.heading.is_some(),
            "nothing to update: give text, start_time/end_time or heading"
        );
        let mut changes = Vec::new();

        if let Some(text) = &edit.text {
            ensure!(
                block.get("text").is_some_and(Value::is_object),
                "this {kind} has no note to edit"
            );
            let mode = edit.text_mode.unwrap_or_default();
            match rich_text_change(at(&path, "text"), &block["text"], text, mode) {
                Some(c) => {
                    self.push(c)?;
                    changes.push(text_change_line("note", &block["text"], text, mode));
                }
                None => changes.push("note unchanged".to_owned()),
            }
        }
        if edit.start_time.is_some() || edit.end_time.is_some() {
            ensure!(
                kind == "place",
                "times can only be set on places, not on a {kind}"
            );
            let (cur_start, cur_end) = (str_of(&block, "startTime"), str_of(&block, "endTime"));
            let (start, end) = times(edit, cur_start, cur_end)?;
            for (key, old, new) in [
                ("startTime", cur_start, start.clone()),
                ("endTime", cur_end, end.clone()),
            ] {
                if old.map(str::to_owned) != new || block.get(key).is_none() {
                    self.push(json!({"p": at(&path, key), "od": old, "oi": new}))?;
                }
            }
            changes.push(match (start, end) {
                (Some(s), Some(e)) => format!("time {s}–{e}"),
                (Some(s), None) => format!("time {s}"),
                _ => "times cleared".to_owned(),
            });
        }
        if let Some(title) = &edit.heading {
            ensure!(
                kind == "checklist",
                "heading on update_block sets a checklist title; this is a {kind}"
            );
            self.push(json!({"p": at(&path, "title"), "od": str_of(&block, "title").unwrap_or(""), "oi": title}))?;
            changes.push(format!("title {}", q(title)));
        }
        self.touch(s);
        Ok(format!(
            "~ {} [b:{}]: {}",
            describe(&block),
            id_of(&block).unwrap_or(0),
            changes.join(", ")
        ))
    }

    fn move_block(&mut self, edit: &Edit) -> Result<String> {
        let (s, b) = trip::resolve_block(&self.doc, required(&edit.block, "block")?)?;
        let target = self.item_section(edit)?;
        let block = blocks(self.section(s))[b].clone();
        let kind = str_of(&block, "type").unwrap_or("?");
        ensure!(
            matches!(kind, "place" | "note" | "checklist") && trip::holds_items(self.section(s)),
            "only places, notes and checklists in lists or days can be moved (this is a {kind})"
        );
        let what = describe(&block);
        if s == target {
            let len = blocks(self.section(s)).len();
            let to = match edit.position {
                None => len - 1,
                Some(p) => {
                    ensure!(
                        p >= 1 && p as usize <= len,
                        "position must be 1..={len} within the same section"
                    );
                    p as usize - 1
                }
            };
            if to == b {
                return Ok(format!(
                    "= {what} already at #{} in {}",
                    b + 1,
                    self.label(s)
                ));
            }
            self.push(json!({"p": ["itinerary", "sections", s, "blocks", b], "lm": to}))?;
            self.touch(s);
            return Ok(format!("↕ {what} → #{} in {}", to + 1, self.label(s)));
        }
        // Like the web client: one op deleting from the source and inserting into the target.
        // Unlike it we keep the block id, so budget expenses linked by blockId stay attached.
        let at = insert_index(edit.position, blocks(self.section(target)).len())?;
        let from_label = self.label(s);
        self.touch(s);
        self.push(json!({"p": ["itinerary", "sections", s, "blocks", b], "ld": block}))?;
        self.push(json!({"p": ["itinerary", "sections", target, "blocks", at], "li": block}))?;
        self.touch(target);
        Ok(format!(
            "→ {what}: {from_label} ⇒ {} #{}",
            self.label(target),
            at + 1
        ))
    }

    fn remove_block(&mut self, edit: &Edit) -> Result<String> {
        let (s, b) = trip::resolve_block(&self.doc, required(&edit.block, "block")?)?;
        let block = blocks(self.section(s))[b].clone();
        let kind = str_of(&block, "type").unwrap_or("?");
        // Reservations stay read-only: removing one would orphan linked expenses and bookings.
        ensure!(
            matches!(kind, "place" | "note" | "checklist") && trip::holds_items(self.section(s)),
            "only places, notes and checklists in lists or days can be removed (this is {} in {})",
            render::field(kind),
            self.label(s)
        );
        let label = self.label(s);
        let linked = self
            .doc
            .pointer("/itinerary/budget/expenses")
            .and_then(Value::as_array)
            .map_or(0, |expenses| {
                expenses
                    .iter()
                    .filter(|e| e.get("blockId").and_then(Value::as_u64) == id_of(&block))
                    .count()
            });
        self.touch(s);
        self.push(json!({"p": ["itinerary", "sections", s, "blocks", b], "ld": block}))?;
        let note = if linked > 0 {
            format!(" ({linked} budget expense(s) stay, still pointing at it)")
        } else {
            String::new()
        };
        Ok(format!("- {} from {label}{note}", describe(&block)))
    }

    fn add_list(&mut self, edit: &Edit, ids: &mut HashSet<u64>) -> Result<String> {
        let heading = required(&edit.heading, "heading")?.trim();
        let secs = sections(&self.doc);
        // After the last user list, else after the Notes section, else first.
        let at = secs
            .iter()
            .rposition(trip::is_list)
            .or_else(|| {
                secs.iter()
                    .position(|s| str_of(s, "type") == Some("textOnly"))
            })
            .map_or(0, |i| i + 1);
        let id = new_id(ids);
        let section = json!({
            "heading": heading, "text": json0::rich_text_doc(""), "blocks": [],
            "placeMarkerColor": MARKER_COLORS[secs.len() % MARKER_COLORS.len()], "placeMarkerIcon": "map-marker",
            "id": id, "type": "normal", "mode": "placeList",
        });
        self.push(json!({"p": ["itinerary", "sections", at], "li": section}))?;
        self.touch(at);
        Ok(format!("+ list {} [s:{id}]", q(heading)))
    }

    fn update_section(&mut self, edit: &Edit) -> Result<String> {
        let index = trip::resolve_section(&self.doc, required(&edit.section, "section")?)?;
        let section = self.section(index).clone();
        let label = self.label(index);
        ensure!(
            edit.heading.is_some() || edit.text.is_some(),
            "nothing to update: give heading or text"
        );
        ensure!(
            trip::holds_items(&section) || str_of(&section, "type") == Some("textOnly"),
            "{label} is read-only; only lists, days and the trip Notes can be edited"
        );
        let mut changes = Vec::new();
        if let Some(heading) = &edit.heading {
            ensure!(
                trip::holds_items(&section),
                "only lists and days have editable headings"
            );
            let old = str_of(&section, "heading").unwrap_or("");
            if old != heading {
                self.push(json!({"p": ["itinerary", "sections", index, "heading"], "od": old, "oi": heading}))?;
            }
            changes.push(format!("heading {}", q(heading)));
        }
        if let Some(text) = &edit.text {
            let path = json!(["itinerary", "sections", index, "text"]);
            let mode = edit.text_mode.unwrap_or_default();
            if section.get("text").is_some_and(Value::is_object) {
                match rich_text_change(path, &section["text"], text, mode) {
                    Some(c) => {
                        self.push(c)?;
                        changes.push(text_change_line("text", &section["text"], text, mode));
                    }
                    None => changes.push("text unchanged".to_owned()),
                }
            } else {
                self.push(json!({"p": path, "oi": json0::rich_text_doc(text)}))?;
                changes.push(format!("text {}", q(text)));
            }
        }
        self.touch(index);
        Ok(format!("~ {label}: {}", changes.join(", ")))
    }

    fn remove_section(&mut self, edit: &Edit) -> Result<String> {
        let index = trip::resolve_section(&self.doc, required(&edit.section, "section")?)?;
        let section = self.section(index).clone();
        let label = self.label(index);
        ensure!(
            trip::is_list(&section),
            "only lists can be removed (use set_dates to change days)"
        );
        ensure!(
            blocks(&section).is_empty(),
            "{label} still has {} items; move or remove them first (in the same batch is fine)",
            blocks(&section).len()
        );
        ensure!(
            section
                .get("text")
                .map(delta_text)
                .unwrap_or_default()
                .trim()
                .is_empty(),
            "{label} still has notes text; clear it first with update_section text \"\""
        );
        self.push(json!({"p": ["itinerary", "sections", index], "ld": section}))?;
        Ok(format!("- {label}"))
    }

    fn rename_trip(&mut self, edit: &Edit) -> Result<String> {
        let title = required(&edit.title, "title")?.trim();
        ensure!(!title.is_empty(), "title must not be empty");
        let Some(old) = str_of(&self.doc, "title").map(str::to_owned) else {
            // text0 needs an existing string; a missing/null title is replaced as a value.
            let current = self.doc.get("title").cloned().unwrap_or(Value::Null);
            self.push(json!({"p": ["title"], "od": current, "oi": title}))?;
            return Ok(format!("~ trip title set to {}", q(title)));
        };
        if old == title {
            return Ok(format!("= title already {}", q(title)));
        }
        // The web client sends a text0 diff; delete-all + insert is the same op type.
        let mut parts = Vec::new();
        if !old.is_empty() {
            parts.push(json!({"p": 0, "d": old}));
        }
        parts.push(json!({"p": 0, "i": title}));
        self.push(json!({"p": ["title"], "t": "text0", "o": parts}))?;
        Ok(format!("~ trip title {} ⇒ {}", q(&old), q(title)))
    }

    /// Mirror the web client's date picker: day k becomes start+k (order kept), new days are
    /// appended, surplus trailing days are removed (only when empty — never silently drop items).
    fn set_dates(&mut self, edit: &Edit, ids: &mut HashSet<u64>) -> Result<String> {
        let start = required(&edit.start_date, "start_date")?.trim().to_owned();
        let end = required(&edit.end_date, "end_date")?.trim().to_owned();
        let span = dates::parse(&end)? - dates::parse(&start)? + 1;
        ensure!(span >= 1, "end_date must not be before start_date");
        ensure!(
            span <= MAX_TRIP_DAYS,
            "trips longer than {MAX_TRIP_DAYS} days are not supported"
        );
        let wanted = span as usize;

        let old = |key: &str| self.doc.get(key).cloned().unwrap_or(Value::Null);
        let (old_start, old_end, old_days) = (old("startDate"), old("endDate"), old("days"));
        for (key, before, after) in [
            ("startDate", &old_start, json!(start)),
            ("endDate", &old_end, json!(end)),
            ("days", &old_days, json!(wanted)),
        ] {
            if *before != after {
                self.push(json!({"p": [key], "od": before, "oi": after}))?;
            }
        }

        let days = trip::day_indices(&self.doc);
        let (mut redated, mut added, mut removed) = (0, 0, 0);
        for (k, &index) in days.iter().take(wanted).enumerate() {
            let date = dates::add(&start, k as i64)?;
            let current = self
                .section(index)
                .get("date")
                .cloned()
                .unwrap_or(Value::Null);
            if current != json!(date) {
                self.push(json!({"p": ["itinerary", "sections", index, "date"], "od": current, "oi": date}))?;
                redated += 1;
            }
        }
        for &index in days.iter().skip(wanted).rev() {
            let section = self.section(index).clone();
            let busy = !blocks(&section).is_empty()
                || !str_of(&section, "heading").unwrap_or("").trim().is_empty()
                || !section
                    .get("text")
                    .map(delta_text)
                    .unwrap_or_default()
                    .trim()
                    .is_empty();
            ensure!(
                !busy,
                "shrinking would delete {} which still has content; move or remove its items first",
                self.label(index)
            );
            self.push(json!({"p": ["itinerary", "sections", index], "ld": section}))?;
            removed += 1;
        }
        for k in days.len()..wanted {
            let at = sections(&self.doc).len();
            let section = json!({
                "heading": "", "text": json0::rich_text_doc(""), "blocks": [],
                "placeMarkerColor": MARKER_COLORS[k % MARKER_COLORS.len()], "placeMarkerIcon": "map-marker",
                "id": new_id(ids), "type": "normal", "mode": "dayPlan", "date": dates::add(&start, k as i64)?,
            });
            self.push(json!({"p": ["itinerary", "sections", at], "li": section}))?;
            self.touch(at);
            added += 1;
        }
        Ok(format!(
            "~ dates {} → {} ({wanted} days): {redated} days re-dated, {added} added, {removed} removed \
             (reservations, expenses and journal dates are not shifted, as in the web app)",
            start, end
        ))
    }
}

fn required<'a>(field: &'a Option<String>, name: &str) -> Result<&'a str> {
    field.as_deref().ok_or_else(|| anyhow!("missing `{name}`"))
}

/// 1-based insert position → list index (None appends).
fn insert_index(position: Option<u32>, len: usize) -> Result<usize> {
    match position {
        None => Ok(len),
        Some(0) => bail!("position is 1-based"),
        Some(p) => {
            let i = p as usize - 1;
            ensure!(
                i <= len,
                "position {p} is past the end ({len} items; use {} to append)",
                len + 1
            );
            Ok(i)
        }
    }
}

fn new_id(ids: &mut HashSet<u64>) -> u64 {
    // The web client uses random integers below 1e9.
    loop {
        let id = rand::random_range(1..1_000_000_000_u64);
        if ids.insert(id) {
            return id;
        }
    }
}

fn added_by(ctx: &PlanContext<'_>) -> Value {
    json!({"type": "user", "userId": ctx.user_id})
}

fn text_doc(text: Option<&str>) -> Value {
    json0::rich_text_doc(text.unwrap_or("").trim_end_matches('\n'))
}

/// Normalise `H:MM`/`HH:MM` to `HH:MM`; empty means "clear".
fn parse_time(raw: &str) -> Result<Option<String>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let (h, m) = raw
        .split_once(':')
        .ok_or_else(|| anyhow!("time {raw:?} must be HH:MM"))?;
    let (h, m): (u32, u32) = (h.parse()?, m.parse()?);
    ensure!(
        h < 24 && m < 60 && raw.len() >= 4,
        "time {raw:?} must be HH:MM (24h)"
    );
    Ok(Some(format!("{h:02}:{m:02}")))
}

/// Resulting (start, end) after applying the edit's time fields to the current values.
fn times(
    edit: &Edit,
    current_start: Option<&str>,
    current_end: Option<&str>,
) -> Result<(Option<String>, Option<String>)> {
    let mut start = current_start.map(str::to_owned);
    let mut end = current_end.map(str::to_owned);
    if let Some(raw) = &edit.start_time {
        start = parse_time(raw)?;
        if start.is_none() {
            end = None;
        }
    }
    if let Some(raw) = &edit.end_time {
        end = parse_time(raw)?;
    }
    ensure!(
        end.is_none() || start.is_some(),
        "end_time needs a start_time"
    );
    Ok((start, end))
}

/// Rich-text change replacing or appending to a Quill document (None if nothing changes).
fn rich_text_change(path: Value, current: &Value, text: &str, mode: TextMode) -> Option<Value> {
    let body = trip::delta_len(current).saturating_sub(1);
    let text = text.trim_end_matches('\n');
    let ops: Vec<Value> = match mode {
        TextMode::Replace => {
            if delta_text(current) == text {
                return None;
            }
            let mut ops = Vec::new();
            if !text.is_empty() {
                ops.push(json!({"insert": text}));
            }
            if body > 0 {
                ops.push(json!({"delete": body}));
            }
            ops
        }
        TextMode::Append => {
            if text.is_empty() {
                return None;
            }
            if body == 0 {
                vec![json!({"insert": text})]
            } else {
                // Keep the existing final newline (it carries the last line's format, e.g. a
                // bullet) and add the new line after it, ending with its own plain newline.
                vec![
                    json!({"retain": body + 1}),
                    json!({"insert": format!("{text}\n")}),
                ]
            }
        }
    };
    (!ops.is_empty()).then(|| json!({"p": path, "t": "rich-text", "o": ops}))
}

/// Summary of a text change; replacements show what they overwrite.
fn text_change_line(label: &str, current: &Value, text: &str, mode: TextMode) -> String {
    let old = delta_text(current);
    match mode {
        TextMode::Replace if !old.trim().is_empty() => {
            format!("{label} {} (was {})", q(text), q(&old))
        }
        TextMode::Replace => format!("{label} {}", q(text)),
        TextMode::Append => format!("{label} += {}", q(text)),
    }
}

fn at(path: &Value, key: &str) -> Value {
    let mut p = path.as_array().cloned().unwrap_or_default();
    p.push(json!(key));
    Value::Array(p)
}

fn q(text: &str) -> String {
    render::quote(text, Options::default())
}

fn describe(block: &Value) -> String {
    let kind = str_of(block, "type").unwrap_or("item");
    let name = match kind {
        "place" => block
            .pointer("/place/name")
            .and_then(Value::as_str)
            .map(str::to_owned),
        "checklist" => str_of(block, "title").map(str::to_owned),
        _ => block.get("text").map(delta_text),
    };
    let id = id_of(block)
        .map(|id| format!(" [b:{id}]"))
        .unwrap_or_default();
    match name {
        Some(n) if !n.trim().is_empty() => format!("{kind} {}{id}", q(&n)),
        _ => format!("{kind}{id}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            "P9".to_owned(),
            PlaceInfo {
                details: json!({"name": "Skytree", "place_id": "P9"}),
                image_keys: vec!["k1".into()],
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
        bulleted["itinerary"]["sections"][3]["blocks"][1]["text"] = json!({"ops": [{"insert": "item"}, {"insert": "\n", "attributes": {"list": "bullet"}}]});
        let plan = run(&bulleted, vec![json!({"op": "update_block", "block": "b:3", "text": "more", "text_mode": "append"})]).unwrap();
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
                vec![
                    json!({"op": "set_dates", "start_date": "2026-11-10", "end_date": "2026-11-10"})
                ]
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
                vec![
                    json!({"op": "set_dates", "start_date": "2026-11-10", "end_date": "2026-11-10"})
                ]
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
}
