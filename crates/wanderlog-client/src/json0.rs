//! Local application of the json0 components this crate generates.
//!
//! Wanderlog's ShareDB server stays the source of truth (it also transforms our op against
//! concurrent edits). We apply our own components to a working copy so later edits in one batch
//! resolve indices against the state earlier edits leave behind, and so previews can render the
//! result. Only the subset we emit is supported: li/ld/lm/oi/od plus the `text0` and `rich-text`
//! subtypes. Offsets in both subtypes count UTF-16 code units, as the JavaScript client does.

use anyhow::{Result, anyhow, bail, ensure};
use serde_json::{Map, Value};

/// Apply one json0 component to `doc`.
pub fn apply(doc: &mut Value, component: &Value) -> Result<()> {
    let path = component
        .get("p")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("json0 component without a path"))?;

    if let Some(subtype) = component.get("t").and_then(Value::as_str) {
        let target = walk_mut(doc, path)?;
        let op = component
            .get("o")
            .ok_or_else(|| anyhow!("subtype op without `o`"))?;
        return match subtype {
            "text0" => apply_text0(target, op),
            "rich-text" => apply_rich_text(target, op),
            other => bail!("unsupported json0 subtype {other}"),
        };
    }

    let (last, parent_path) = path
        .split_last()
        .ok_or_else(|| anyhow!("empty json0 path"))?;
    let parent = walk_mut(doc, parent_path)?;

    if let Some(to) = component.get("lm") {
        let list = parent
            .as_array_mut()
            .ok_or_else(|| anyhow!("lm on a non-list"))?;
        let from = index(last)?;
        let to = to
            .as_u64()
            .ok_or_else(|| anyhow!("lm target must be an index"))? as usize;
        ensure!(
            from < list.len() && to < list.len(),
            "lm index out of range"
        );
        let item = list.remove(from);
        list.insert(to, item);
        return Ok(());
    }

    let (li, ld) = (component.get("li"), component.get("ld"));
    if li.is_some() || ld.is_some() {
        let list = parent
            .as_array_mut()
            .ok_or_else(|| anyhow!("li/ld on a non-list"))?;
        let i = index(last)?;
        if ld.is_some() {
            ensure!(i < list.len(), "ld index {i} out of range");
            list.remove(i);
        }
        if let Some(item) = li {
            ensure!(i <= list.len(), "li index {i} out of range");
            list.insert(i, item.clone());
        }
        return Ok(());
    }

    let (oi, od) = (component.get("oi"), component.get("od"));
    if oi.is_some() || od.is_some() {
        let object = parent
            .as_object_mut()
            .ok_or_else(|| anyhow!("oi/od on a non-object"))?;
        let key = last
            .as_str()
            .ok_or_else(|| anyhow!("oi/od key must be a string"))?;
        match oi {
            Some(value) => object.insert(key.to_owned(), value.clone()),
            None => object.remove(key),
        };
        return Ok(());
    }

    bail!("unsupported json0 component {component}")
}

fn index(segment: &Value) -> Result<usize> {
    segment
        .as_u64()
        .map(|i| i as usize)
        .ok_or_else(|| anyhow!("expected a list index in path, got {segment}"))
}

fn walk_mut<'a>(mut node: &'a mut Value, path: &[Value]) -> Result<&'a mut Value> {
    for segment in path {
        node = match segment {
            Value::Number(_) => {
                let i = index(segment)?;
                node.as_array_mut()
                    .and_then(|list| list.get_mut(i))
                    .ok_or_else(|| anyhow!("path index {i} not found"))?
            }
            Value::String(key) => node
                .as_object_mut()
                .and_then(|object| object.get_mut(key))
                .ok_or_else(|| anyhow!("path key {key:?} not found"))?,
            other => bail!("invalid path segment {other}"),
        };
    }
    Ok(node)
}

/// Length in UTF-16 code units (JavaScript string length).
pub fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

fn apply_text0(target: &mut Value, op: &Value) -> Result<()> {
    let current = target
        .as_str()
        .ok_or_else(|| anyhow!("text0 target is not a string"))?;
    let mut units: Vec<u16> = current.encode_utf16().collect();
    for part in op
        .as_array()
        .ok_or_else(|| anyhow!("text0 op must be a list"))?
    {
        let at = part
            .get("p")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("text0 part without p"))? as usize;
        ensure!(at <= units.len(), "text0 offset {at} out of range");
        if let Some(insert) = part.get("i").and_then(Value::as_str) {
            units.splice(at..at, insert.encode_utf16());
        } else if let Some(delete) = part.get("d").and_then(Value::as_str) {
            let del: Vec<u16> = delete.encode_utf16().collect();
            ensure!(
                units.get(at..at + del.len()) == Some(&del[..]),
                "text0 delete does not match document"
            );
            units.drain(at..at + del.len());
        } else {
            bail!("text0 part needs i or d");
        }
    }
    *target = Value::String(String::from_utf16(&units)?);
    Ok(())
}

/// Quill-delta length of one document op (strings count UTF-16 units, embeds count 1).
fn insert_len(op: &Value) -> usize {
    match op.get("insert") {
        Some(Value::String(s)) => utf16_len(s),
        Some(_) => 1,
        None => 0,
    }
}

/// Split a document insert op after `n` units; returns (head, tail).
fn split_insert(op: &Value, n: usize) -> (Value, Option<Value>) {
    let len = insert_len(op);
    if n >= len {
        return (op.clone(), None);
    }
    let text = op["insert"].as_str().unwrap_or_default();
    let units: Vec<u16> = text.encode_utf16().collect();
    let with_text = |s: &[u16]| {
        let mut part = op.clone();
        part["insert"] = Value::String(String::from_utf16_lossy(s));
        part
    };
    (with_text(&units[..n]), Some(with_text(&units[n..])))
}

/// Compose a Quill delta change onto a rich-text document `{ops: [...]}`.
fn apply_rich_text(target: &mut Value, change: &Value) -> Result<()> {
    let object = target
        .as_object_mut()
        .ok_or_else(|| anyhow!("rich-text target is not an object"))?;
    let mut source: std::collections::VecDeque<Value> = object
        .get("ops")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into();
    let mut out: Vec<Value> = Vec::new();

    let mut take = |mut n: usize, keep: bool, out: &mut Vec<Value>| -> Result<()> {
        while n > 0 {
            let op = source
                .pop_front()
                .ok_or_else(|| anyhow!("rich-text change longer than document"))?;
            let (head, tail) = split_insert(&op, n);
            n -= insert_len(&head);
            if let Some(tail) = tail {
                source.push_front(tail);
            }
            if keep {
                out.push(head);
            }
        }
        Ok(())
    };

    for op in change
        .as_array()
        .ok_or_else(|| anyhow!("rich-text change must be a list"))?
    {
        if let Some(n) = op.get("retain").and_then(Value::as_u64) {
            ensure!(
                op.get("attributes").is_none(),
                "attribute retains are not supported"
            );
            take(n as usize, true, &mut out)?;
        } else if let Some(n) = op.get("delete").and_then(Value::as_u64) {
            take(n as usize, false, &mut out)?;
        } else if op.get("insert").is_some() {
            out.push(op.clone());
        } else {
            bail!("unsupported rich-text op {op}");
        }
    }
    out.extend(source);
    object.insert("ops".into(), Value::Array(normalize(out)));
    Ok(())
}

/// Merge adjacent string inserts that carry identical attributes.
fn normalize(ops: Vec<Value>) -> Vec<Value> {
    let mut merged: Vec<Value> = Vec::with_capacity(ops.len());
    for op in ops {
        if insert_len(&op) == 0 {
            continue;
        }
        if let (Some(prev), Some(Value::String(text))) = (merged.last_mut(), op.get("insert")) {
            let same_attrs = prev.get("attributes") == op.get("attributes");
            if let (true, Some(Value::String(prev_text))) = (same_attrs, prev.get_mut("insert")) {
                prev_text.push_str(text);
                continue;
            }
        }
        merged.push(op);
    }
    merged
}

/// A Quill document holding `text` (Quill documents always end with a newline).
pub fn rich_text_doc(text: &str) -> Value {
    let mut ops = Map::new();
    ops.insert("insert".into(), Value::String(format!("{text}\n")));
    serde_json::json!({ "ops": [Value::Object(ops)] })
}

#[cfg(test)]
#[path = "tests/json0.rs"]
mod tests;
