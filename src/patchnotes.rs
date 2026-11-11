//! Buffs and nerfs from the in-game patch notes: the player team's `news` (a Team record field).
//!
//! Game 0.6: a news item mentioning `PatchNote`, whose `title_bind` list starts with the patch
//! version, and a `champion_patch_data` list of `["champion", [ {..., "is_buff": true}, ... ]]`
//! entries. A champion's direction is buffed lines minus nerfed lines (positive = buffed).
//! Game 0.7 (2027.x saves): `ty: {"PatchNote": {"version", "added_champion", "champion_patch_data",
//! "champion_patch_reasons", "game_rule_patch_data", "game_rule_patch_reasons"}}`, `title_bind` `[["Version", ...]]`;
//! the balance patches (2.8.0) write theirs in the same form, author [`crate::balance::AUTHOR`].
//! Game 0.6.2 stopped listing champions whose numbers did not actually change, which only means
//! fewer entries. Also accepted: an object `{"champion": [...]}` and
//! `{"champion": "...", "changes": [...]}` entries.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::records::{find_key, version_string};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PatchNote {
    /// Every string of `title_bind` (the first one is the version in 0.6.0), plus a `version`
    /// field if the item has one.
    pub versions: Vec<String>,
    /// The note's own `version` field (the game's patch it is for).
    pub version: Option<String>,
    /// Written by this mod's balance patches (a hotfix's own notes).
    pub ours: bool,
    /// Champion -> buffed lines minus nerfed lines.
    pub changes: BTreeMap<String, i32>,
    /// Champions whose lines the balance patches wrote (their reason a `pma_` code), in the game's notes too.
    pub balance: BTreeSet<String>,
}

impl PatchNote {
    pub fn is_for(&self, version: &str) -> bool {
        self.versions.iter().any(|v| v == version)
    }

    /// The note without the balance patches' lines (the game's own changes only).
    pub fn game_only(&self) -> PatchNote {
        let mut note = self.clone();
        note.changes.retain(|c, _| !self.balance.contains(c));
        note.balance.clear();
        note
    }

    /// Buff (+1), nerf (-1) or neither (0) for one champion.
    pub fn direction(&self, champion: &str) -> i32 {
        self.changes.get(champion).copied().unwrap_or(0).signum()
    }
}

/// Every patch note in the news list, newest last as the game stores them.
pub fn parse_news(json: &str) -> Result<Vec<PatchNote>, String> {
    parse_all(json).map(|n| n.notes)
}

/// The news list: its patch notes and how many items it has.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct News {
    pub notes: Vec<PatchNote>,
    pub len: usize,
}

impl News {
    /// The game's newest patch: the newest note's `version`.
    pub fn newest_version(&self) -> Option<String> {
        self.notes.iter().rev().find_map(|n| n.version.clone())
    }
}

/// The news list as [`News`].
pub fn parse_all(json: &str) -> Result<News, String> {
    let doc: Value = serde_json::from_str(json).map_err(|e| format!("news is not JSON ({e})"))?;
    let items: Vec<&Value> = match &doc {
        Value::Array(items) => items.iter().collect(),
        Value::Object(map) => match map.values().find(|v| v.is_array()) {
            Some(Value::Array(items)) => items.iter().collect(),
            _ => vec![&doc],
        },
        _ => return Err(format!("news is a {}", crate::records::kind(&doc))),
    };
    let len = items.len();
    Ok(News { notes: items.into_iter().filter_map(patch_note).collect(), len })
}

/// One news item's patch notes (None: not patch notes).
pub fn parse_item(json: &str) -> Option<PatchNote> {
    serde_json::from_str::<Value>(json).ok().as_ref().and_then(patch_note)
}

/// The notes of each patch as one (the game's own and the balance patches' of the same patch): a champion's
/// direction is the sum of each note's. Notes without a `version` field stay as they are.
pub fn combine(notes: Vec<PatchNote>) -> Vec<PatchNote> {
    let mut out: Vec<PatchNote> = Vec::new();
    for note in notes {
        let Some(version) = note.version.clone() else {
            out.push(note);
            continue;
        };
        let at = match out.iter().position(|n| n.version.as_deref() == Some(version.as_str())) {
            Some(at) => at,
            None => {
                out.push(PatchNote { versions: vec![version.clone()], version: Some(version), ..PatchNote::default() });
                out.len() - 1
            }
        };
        let merged = &mut out[at];
        for v in note.versions {
            if !merged.versions.contains(&v) {
                merged.versions.push(v);
            }
        }
        merged.ours |= note.ours;
        merged.balance.extend(note.balance);
        for (champion, lines) in note.changes {
            *merged.changes.entry(champion).or_insert(0) += lines.signum();
        }
    }
    out
}

fn patch_note(item: &Value) -> Option<PatchNote> {
    let data = find_key(item, "champion_patch_data");
    // patch notes that listed nothing, made plain news by the balance patches (`server::unchanged`): the patch's
    // notes all the same, the game's patch in the `Patch` bind
    let unchanged = data.is_none() && mentions(item, "patch.pma_none", 0);
    if data.is_none() && !unchanged && !mentions(item, "PatchNote", 0) {
        return None;
    }
    let mut versions = Vec::new();
    if let Some(bind) = find_key(item, "title_bind") {
        strings(bind, &mut versions, 0);
    }
    let bound = || {
        let binds = item.get("ty")?.get("Simple")?.get("content_bind")?.as_array()?;
        binds.iter().find(|b| b.get(0).and_then(Value::as_str) == Some("Patch"))?.get(1)?.as_str().map(str::to_string)
    };
    let version = find_key(item, "version").and_then(version_string).or_else(|| if unchanged { bound() } else { None });
    if let Some(v) = &version {
        versions.push(v.clone());
    }
    let ours = item.get("author").and_then(Value::as_str).is_some_and(crate::balance::is_ours);
    let mut changes = BTreeMap::new();
    if let Some(data) = data {
        champion_changes(data, &mut changes);
    }
    let mut balance = BTreeSet::new();
    if let Some(reasons) = find_key(item, "champion_patch_reasons") {
        balance_champions(reasons, &mut balance);
    }
    Some(PatchNote { versions, version, ours, changes, balance })
}

/// The champions of `champion_patch_reasons` (a list of `[champion, "code,code"]` or a map) with a balance-patch
/// reason (`pma_` codes, `balance::reason`).
fn balance_champions(reasons: &Value, out: &mut BTreeSet<String>) {
    let ours = |codes: &Value| codes.as_str().is_some_and(|s| s.split(',').any(|c| c.trim().starts_with("pma_")));
    match reasons {
        Value::Array(entries) => {
            for entry in entries {
                if let (Some(name), Some(codes)) = (entry.get(0).and_then(Value::as_str), entry.get(1)) {
                    if ours(codes) {
                        out.insert(name.to_string());
                    }
                }
            }
        }
        Value::Object(map) => {
            for (name, codes) in map {
                if ours(codes) {
                    out.insert(name.clone());
                }
            }
        }
        _ => {}
    }
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
    fn reads_the_07_layout_and_adds_up_a_patchs_notes() {
        let news = r##"[
            {"author": "Team Samoyed", "title": "#asset/base/text/news?patch.title", "title_bind": [["Version", "2027.4.1"]],
             "ty": {"PatchNote": {"version": "2027.4.1", "added_champion": [], "champion_patch_data": [
                ["league_brand", [{"key": "#asset/base/text/champion?stat.attack", "target_action": null,
                                   "pre_value": "94", "after_value": "75", "is_buff": false}]],
                ["tft_maddie", [{"key": "k", "target_action": null, "pre_value": "1", "after_value": "2", "is_buff": true}]]],
                "champion_patch_reasons": [["league_brand", "early_game_dominant_2"]],
                "game_rule_patch_data": [], "game_rule_patch_reasons": []}}},
            {"author": "Bark Knights", "title": "x", "title_bind": [], "ty": "StartTeam"},
            {"author": "智能BP / Patch Meta AI", "title": "#asset/base/text/news?patch.title", "title_bind": [["Version", "2027.4.1b"]],
             "ty": {"PatchNote": {"version": "2027.4.1", "added_champion": [], "champion_patch_data": [
                ["tft_maddie", [{"key": "k", "target_action": null, "pre_value": "2", "after_value": "1", "is_buff": false},
                                {"key": "k2", "target_action": null, "pre_value": "2", "after_value": "1", "is_buff": false}]]],
                "champion_patch_reasons": [], "game_rule_patch_data": [], "game_rule_patch_reasons": []}}}
        ]"##;
        let news = parse_all(news).unwrap();
        assert_eq!((news.len, news.notes.len()), (3, 2));
        assert_eq!(news.newest_version().as_deref(), Some("2027.4.1"));
        assert!(!news.notes[0].ours && news.notes[1].ours);
        assert!(news.notes[1].is_for("2027.4.1b") && news.notes[1].is_for("2027.4.1"));
        let combined = combine(news.notes);
        assert_eq!(combined.len(), 1);
        // the game buffed maddie, the balance patch nerfed her: no direction left; brand stays nerfed
        assert_eq!((combined[0].direction("tft_maddie"), combined[0].direction("league_brand")), (0, -1));
        assert!(combined[0].ours && combined[0].is_for("2027.4.1b"));
    }

    #[test]
    fn the_balance_lines_in_the_games_notes_are_told_apart() {
        // the game's 2027.4.2 notes (its champion changes off) with a round merged in, next to one of its own
        let item = r##"{"author": "Team Samoyed", "title": "#asset/base/text/news?patch.title", "title_bind": [["Version", "2027.4.2"]],
            "ty": {"PatchNote": {"version": "2027.4.2", "added_champion": [], "champion_patch_data": [
                ["league_brand", [{"key": "k", "target_action": null, "pre_value": "94", "after_value": "75", "is_buff": false}]],
                ["tft_maddie", [{"key": "k", "target_action": null, "pre_value": "2", "after_value": "1", "is_buff": false}]],
                ["league_kaisa", [{"key": "k", "target_action": null, "pre_value": "1", "after_value": "2", "is_buff": true}]]],
                "champion_patch_reasons": [["league_brand", "early_game_dominant_2"], ["tft_maddie", "pma_wr_77,pma_cap"],
                                           ["league_kaisa", "pma_wr_28"]],
                "game_rule_patch_data": [], "game_rule_patch_reasons": []}}}"##;
        let note = parse_item(item).unwrap();
        assert!(!note.ours, "the game's own notes");
        assert_eq!(note.balance.iter().map(String::as_str).collect::<Vec<_>>(), ["league_kaisa", "tft_maddie"]);
        let game = note.game_only();
        assert_eq!(game.changes.keys().map(String::as_str).collect::<Vec<_>>(), ["league_brand"]);
        assert!(game.balance.is_empty() && game.is_for("2027.4.2"));
        // a map of reasons, and the mod's notes under its new name
        let item = r#"{"author": "电竞大脑 / Patch Meta AI", "ty": {"PatchNote": {"version": "1.1",
            "champion_patch_data": {"a": [{"is_buff": true}]}, "champion_patch_reasons": {"a": "pma_wr_30", "b": "x"}}}}"#;
        let note = parse_item(item).unwrap();
        assert!(note.ours && note.balance.contains("a") && !note.balance.contains("b"));
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
