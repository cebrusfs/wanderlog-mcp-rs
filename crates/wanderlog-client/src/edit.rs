//! High-level itinerary edits → json0 components.
//!
//! Component shapes mirror what the official web client sends (captured 2026-10-06; see
//! docs/protocol.md). Edits run in order against a working copy, so each edit sees the effects of
//! earlier ones (e.g. add a list, then add places to it by heading). The whole batch is submitted
//! as ONE ShareDB op, which the server applies atomically.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, anyhow, bail, ensure};
use schemars::JsonSchema;
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

/// An existing place whose saved data can be reused without a Places API lookup.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct PlaceSource {
    pub trip_id: u64,
    /// A b:<id> ref from the source trip's get_trip response.
    pub block: String,
    /// Source revision from get_trip; rejects a changed source when supplied.
    pub revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PlaceKey {
    Id(String),
    Source(PlaceSource),
}

/// One itinerary edit. Fields used per op:
/// add_place{section, place_id OR source, include_photos?, position?, text?, start_time?, end_time?};
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
    /// add_place: real Google place_id from any trusted place tool, or use source instead. Never invent one.
    pub place_id: Option<String>,
    /// add_place: reuse an existing place and its saved photos. Supply either source or place_id.
    /// Notes, times, reservations and attachments are not copied; supply text/times explicitly.
    pub source: Option<PlaceSource>,
    /// add_place: fetch missing photo keys (default false). Existing cached/source photos are reused.
    pub include_photos: Option<bool>,
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
    pub photos_loaded: bool,
}

impl PlaceInfo {
    pub fn id(&self) -> Option<&str> {
        str_of(&self.details, "place_id").or_else(|| str_of(&self.details, "placeId"))
    }

    /// Keep only place data and photo keys, never a source trip's notes or booking information.
    pub fn from_block(block: &Value) -> Option<Self> {
        if str_of(block, "type") != Some("place") {
            return None;
        }
        let info = Self {
            details: block.get("place")?.clone(),
            image_keys: block
                .get("imageKeys")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            photos_loaded: block.get("imageKeys").is_some_and(Value::is_array),
        };
        if info.id().is_none_or(|id| id.trim().is_empty())
            || str_of(&info.details, "name").is_none()
        {
            return None;
        }
        Some(info)
    }
}

impl Edit {
    pub fn place_key(&self) -> Result<PlaceKey> {
        match (&self.place_id, &self.source) {
            (Some(id), None) if !id.trim().is_empty() => Ok(PlaceKey::Id(id.trim().to_owned())),
            (None, Some(source)) if !source.block.trim().is_empty() => {
                Ok(PlaceKey::Source(source.clone()))
            }
            _ => bail!("add_place requires exactly one of place_id or source (trip_id, block)"),
        }
    }
}

pub fn source_place(doc: &Value, source: &PlaceSource) -> Result<PlaceInfo> {
    if let Some(revision) = source.revision {
        ensure!(
            doc.get("overallVersion").and_then(Value::as_u64) == Some(revision),
            "source trip {} changed since revision {revision}; re-read it before copying",
            source.trip_id
        );
    }
    let (section, block) = trip::resolve_block(doc, &source.block)?;
    PlaceInfo::from_block(&blocks(&sections(doc)[section])[block])
        .context("source block is not a reusable place")
}

pub struct PlanContext<'a> {
    pub user_id: u64,
    pub places: &'a HashMap<PlaceKey, PlaceInfo>,
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
    for edit in edits {
        if edit.op == EditOp::AddPlace {
            edit.place_key()?;
        } else {
            ensure!(
                edit.source.is_none() && edit.include_photos.is_none(),
                "source and include_photos are only valid for add_place"
            );
        }
    }
    Ok(())
}

/// Resolve missing IDs in small sequential batches. Save each successful response immediately,
/// so a later failure does not discard progress. The caller serialises access to `known`.
pub async fn prefetch_places(
    rest: &Rest,
    edits: &[Edit],
    known: &mut HashMap<String, PlaceInfo>,
    mut resolved: HashMap<PlaceKey, PlaceInfo>,
) -> Result<HashMap<PlaceKey, PlaceInfo>> {
    check_batch(edits)?;
    let mut wanted: Vec<String> = Vec::new();
    for edit in edits.iter().filter(|e| e.op == EditOp::AddPlace) {
        if let PlaceKey::Id(id) = edit.place_key()? {
            if !wanted.contains(&id) {
                wanted.push(id);
            }
        } else {
            ensure!(
                resolved.contains_key(&edit.place_key()?),
                "source place has not been resolved"
            );
        }
    }
    let missing: Vec<String> = wanted
        .iter()
        .filter(|id| !known.contains_key(*id))
        .cloned()
        .collect();
    // Five is our conservative request size, not a claimed server limit.
    for ids in missing.chunks(5) {
        for details in rest.multiple_place_details(ids).await? {
            if let Some(id) =
                str_of(&details, "place_id").filter(|id| ids.iter().any(|wanted| wanted == id))
                && str_of(&details, "name").is_some_and(|name| !name.trim().is_empty())
            {
                known.insert(
                    id.to_owned(),
                    PlaceInfo {
                        details,
                        image_keys: Vec::new(),
                        photos_loaded: false,
                    },
                );
            }
        }
        let absent: Vec<&str> = ids
            .iter()
            .filter(|id| !known.contains_key(*id))
            .map(String::as_str)
            .collect();
        ensure!(
            absent.is_empty(),
            "place details missing for {}; nothing written; successful lookups remain cached",
            absent.join(", ")
        );
    }
    for id in wanted {
        resolved.insert(PlaceKey::Id(id.clone()), known[&id].clone());
    }
    for edit in edits.iter().filter(|e| e.op == EditOp::AddPlace) {
        let key = edit.place_key()?;
        let info = resolved.get_mut(&key).context("place data unavailable")?;
        if !info.photos_loaded
            && let Some(cached) = info
                .id()
                .and_then(|id| known.get(id))
                .filter(|p| p.photos_loaded)
        {
            info.image_keys.clone_from(&cached.image_keys);
            info.photos_loaded = true;
        }
        if !info.photos_loaded && edit.include_photos == Some(true) {
            info.image_keys = rest.place_photos(&info.details).await?;
            info.photos_loaded = true;
            if let Some(id) = info.id() {
                known.insert(id.to_owned(), info.clone());
            }
        }
    }
    Ok(resolved)
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
        let key = edit.place_key()?;
        let info = ctx
            .places
            .get(&key)
            .ok_or_else(|| anyhow!("place data could not be resolved"))?;
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
        let name = str_of(&info.details, "name").unwrap_or("?");
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
    use crate::test_support::{login_server, response};
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
