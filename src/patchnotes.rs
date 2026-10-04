//! Buffs and nerfs from the in-game patch notes: the player team's `news` (a Team record field).
//!
//! Game 0.6: a news item mentioning `PatchNote`, whose `title_bind` list starts with the patch
//! version, and a `champion_patch_data` list of `["champion", [ {..., "is_buff": true}, ... ]]`
//! entries. A champion's direction is buffed lines minus nerfed lines (positive = buffed).
//! Game 0.6.2 stopped listing champions whose numbers did not actually change, which only means
//! fewer entries. Also accepted: an object `{"champion": [...]}` and
//! `{"champion": "...", "changes": [...]}` entries.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::records::{find_key, version_string};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PatchNote {
    /// Every string of `title_bind` (the first one is the version in 0.6.0), plus a `version`
    /// field if the item has one.
    pub versions: Vec<String>,
    /// Champion -> buffed lines minus nerfed lines.
    pub changes: BTreeMap<String, i32>,
}

impl PatchNote {
    pub fn is_for(&self, version: &str) -> bool {
        self.versions.iter().any(|v| v == version)
    }

    /// Buff (+1), nerf (-1) or neither (0) for one champion.
    pub fn direction(&self, champion: &str) -> i32 {
        self.changes.get(champion).copied().unwrap_or(0).signum()
    }
}

/// Every patch note in the news list, newest last as the game stores them.
pub fn parse_news(json: &str) -> Result<Vec<PatchNote>, String> {
    let doc: Value = serde_json::from_str(json).map_err(|e| format!("news is not JSON ({e})"))?;
    let items: Vec<&Value> = match &doc {
        Value::Array(items) => items.iter().collect(),
        Value::Object(map) => match map.values().find(|v| v.is_array()) {
            Some(Value::Array(items)) => items.iter().collect(),
            _ => vec![&doc],
        },
        _ => return Err(format!("news is a {}", crate::records::kind(&doc))),
    };
    Ok(items.into_iter().filter_map(patch_note).collect())
}

fn patch_note(item: &Value) -> Option<PatchNote> {
    let data = find_key(item, "champion_patch_data");
    if data.is_none() && !mentions(item, "PatchNote", 0) {
        return None;
    }
    let mut versions = Vec::new();
    if let Some(bind) = find_key(item, "title_bind") {
        strings(bind, &mut versions, 0);
    }
    if let Some(v) = find_key(item, "version").and_then(version_string) {
        versions.push(v);
    }
    let mut changes = BTreeMap::new();
    if let Some(data) = data {
        champion_changes(data, &mut changes);
    }
    Some(PatchNote { versions, changes })
}

/// Whether a key or a string value contains `needle`.
fn mentions(v: &Value, needle: &str, depth: usize) -> bool {
    if depth > 8 {
        return false;
    }
    match v {
        Value::String(s) => s.contains(needle),
        Value::Array(items) => items.iter().any(|i| mentions(i, needle, depth + 1)),
        Value::Object(map) => {
            map.iter().any(|(k, v)| k.contains(needle) || mentions(v, needle, depth + 1))
        }
        _ => false,
    }
}

fn strings(v: &Value, out: &mut Vec<String>, depth: usize) {
    if depth > 6 {
        return;
    }
    match v {
        Value::String(s) => out.push(s.trim().to_string()),
        Value::Number(n) => out.push(n.to_string()),
        Value::Array(items) => items.iter().for_each(|i| strings(i, out, depth + 1)),
        Value::Object(map) => map.values().for_each(|i| strings(i, out, depth + 1)),
        _ => {}
    }
}

fn champion_changes(data: &Value, out: &mut BTreeMap<String, i32>) {
    match data {
        Value::Array(entries) => {
            for entry in entries {
                match entry {
                    // ["champion", [changes...]] (0.6.0)
                    Value::Array(parts) => {
                        if let Some(Value::String(name)) = parts.first() {
                            let lines: i32 = parts.iter().skip(1).map(|p| buff_count(p, 0)).sum();
                            *out.entry(name.clone()).or_insert(0) += lines;
                        }
                    }
                    // {"champion": "...", "changes": [...]}
                    Value::Object(map) => {
                        let name = ["champion", "name", "id", "key"]
                            .iter()
                            .find_map(|k| map.get(*k).and_then(Value::as_str));
                        if let Some(name) = name {
                            *out.entry(name.to_string()).or_insert(0) += buff_count(entry, 0);
                        }
                    }
                    _ => {}
                }
            }
        }
        // {"champion": [changes...]}
        Value::Object(map) => {
            for (name, lines) in map {
                *out.entry(name.clone()).or_insert(0) += buff_count(lines, 0);
            }
        }
        _ => {}
    }
}

/// Buffed lines minus nerfed lines below `v`.
fn buff_count(v: &Value, depth: usize) -> i32 {
    if depth > 8 {
        return 0;
    }
    match v {
        Value::Object(map) => {
            let own = match map.get("is_buff") {
                Some(Value::Bool(true)) => 1,
                Some(Value::Bool(false)) => -1,
                _ => 0,
            };
            own + map
                .iter()
                .filter(|(k, _)| k.as_str() != "is_buff")
                .map(|(_, v)| buff_count(v, depth + 1))
                .sum::<i32>()
        }
        Value::Array(items) => items.iter().map(|i| buff_count(i, depth + 1)).sum(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_060_news_layout() {
        let news = r#"[
            {"kind": "Transfer", "title_bind": ["x"], "content": "hello"},
            {"kind": "PatchNote", "title_bind": ["1.2"], "champion_patch_data": [
                ["fighter", [{"stat": "attack", "is_buff": false}]]]},
            {"kind": "PatchNote", "title_bind": ["1.3", "Spring"], "champion_patch_data": [
                ["fighter", [{"stat": "attack", "is_buff": true}, {"stat": "hp", "is_buff": true},
                             {"stat": "range", "is_buff": false}]],
                ["ninja", [{"stat": "attack", "is_buff": false}]],
                ["monk", []]]}
        ]"#;
        let notes = parse_news(news).unwrap();
        assert_eq!(notes.len(), 2);
        let latest = notes.iter().rev().find(|n| n.is_for("1.3")).unwrap();
        assert_eq!(latest.direction("fighter"), 1);
        assert_eq!(latest.direction("ninja"), -1);
        assert_eq!(latest.direction("monk"), 0);
        assert_eq!(latest.direction("archer"), 0);
        assert!(notes[0].is_for("1.2") && !notes[0].is_for("1.3"));
    }

    #[test]
    fn reads_other_shapes() {
        let news = r#"{"items": [{"type": "patch", "version": "2.0", "champion_patch_data": {
            "fighter": [{"is_buff": false}, {"is_buff": false}],
            "ninja": {"lines": [{"is_buff": true}]}}},
            {"type": "patch", "title_bind": {"v": "2.1"}, "champion_patch_data": [
                {"champion": "monk", "changes": [{"is_buff": true}]}]}]}"#;
        let notes = parse_news(news).unwrap();
        assert_eq!(notes.len(), 2);
        assert!(notes[0].is_for("2.0"));
        assert_eq!(notes[0].direction("fighter"), -1);
        assert_eq!(notes[0].direction("ninja"), 1);
        assert!(notes[1].is_for("2.1"));
        assert_eq!(notes[1].direction("monk"), 1);
        assert!(parse_news("{").is_err());
        assert_eq!(parse_news("[]").unwrap(), vec![]);
    }
}
