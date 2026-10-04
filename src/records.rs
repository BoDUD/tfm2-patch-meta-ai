//! Reading one match out of a `MatchReplay` (competition) or `SoloRankMatch` record.
//!
//! The documents are JSON from `record_get_json(kind, id, "")`. In game 0.6 they carry a
//! top-level `version` (the in-game patch, e.g. `"1.3"`), `blue_team_win`, and `blue_team` /
//! `red_team` arrays whose player entries have `champion` and `position`; solo-rank records
//! also carry `played`. That layout is read directly (unknown fields are skipped without being
//! built). When a field is not where it used to be, the whole document is searched for it, so a
//! reshuffled record still parses, and the probe in `diag.log` says which layout was found.
//! Only real player entries count - a `champion` key in bans or other nested data does not.

use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Player {
    pub champion: String,
    pub position: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchSummary {
    pub version: String,
    pub blue_win: bool,
    pub blue: Vec<Player>,
    pub red: Vec<Player>,
    /// The layout differed from 0.6.0's and the fields were found by searching.
    pub searched: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    Match(MatchSummary),
    /// A scheduled solo-rank match that has not been played yet (look again later).
    NotPlayed,
    /// Not usable; the reason goes to `diag.log`.
    Invalid(String),
}

#[derive(Deserialize)]
struct Layout {
    #[serde(default)]
    version: Option<Value>,
    #[serde(default)]
    blue_team_win: Option<Value>,
    #[serde(default)]
    blue_team: Option<Value>,
    #[serde(default)]
    red_team: Option<Value>,
    #[serde(default)]
    played: Option<Value>,
}

struct Fields<'a> {
    version: Option<&'a Value>,
    blue_win: Option<&'a Value>,
    blue: Option<&'a Value>,
    red: Option<&'a Value>,
    played: Option<&'a Value>,
}

/// Parses one record. `solo` records must say they were played (`"played": true`); unplayed
/// ones come back as [`Parsed::NotPlayed`].
pub fn parse_record(json: &str, solo: bool) -> Parsed {
    if let Ok(layout) = serde_json::from_str::<Layout>(json) {
        let fields = Fields {
            version: layout.version.as_ref(),
            blue_win: layout.blue_team_win.as_ref(),
            blue: layout.blue_team.as_ref(),
            red: layout.red_team.as_ref(),
            played: layout.played.as_ref(),
        };
        if let Ok(parsed) = from_fields(&fields, solo, false) {
            return parsed;
        }
    }
    // Not the 0.6.0 layout: parse everything and look for the fields anywhere.
    let doc: Value = match serde_json::from_str(json) {
        Ok(doc) => doc,
        Err(err) => return Parsed::Invalid(format!("not JSON ({err})")),
    };
    let fields = Fields {
        version: find_key(&doc, "version"),
        blue_win: find_key(&doc, "blue_team_win"),
        blue: find_key(&doc, "blue_team"),
        red: find_key(&doc, "red_team"),
        played: find_key(&doc, "played"),
    };
    match from_fields(&fields, solo, true) {
        Ok(parsed) => parsed,
        Err(missing) => Parsed::Invalid(format!("no {missing}; top-level keys: {}", top_keys(&doc))),
    }
}

/// `Err(field)` when a required field is missing.
fn from_fields(f: &Fields<'_>, solo: bool, searched: bool) -> Result<Parsed, &'static str> {
    if solo {
        match f.played.and_then(as_bool) {
            Some(true) => {}
            Some(false) => return Ok(Parsed::NotPlayed),
            None => return Err("played"),
        }
    }
    let version = f.version.and_then(version_string).ok_or("version")?;
    let blue = players(f.blue.ok_or("blue_team")?);
    let red = players(f.red.ok_or("red_team")?);
    let blue_win = f.blue_win.and_then(as_bool).ok_or("blue_team_win")?;
    if blue.is_empty() && red.is_empty() {
        return Ok(Parsed::Invalid("no champions in blue_team / red_team".to_string()));
    }
    Ok(Parsed::Match(MatchSummary { version, blue_win, blue, red, searched }))
}

fn as_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_i64().map(|n| n != 0),
        Value::String(s) => match s.as_str() {
            "true" | "True" => Some(true),
            "false" | "False" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// `"1.3"`, `13`, `[1, 3]` or `{"major": 1, "minor": 3}` -> `"1.3"`.
pub fn version_string(v: &Value) -> Option<String> {
    let text = match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(parts) => {
            let parts: Option<Vec<String>> = parts.iter().map(scalar_text).collect();
            parts?.join(".")
        }
        Value::Object(map) => {
            let parts: Vec<String> = ["major", "minor", "patch"]
                .iter()
                .filter_map(|k| map.get(*k).and_then(scalar_text))
                .collect();
            if parts.is_empty() {
                return None;
            }
            parts.join(".")
        }
        _ => return None,
    };
    (!text.is_empty()).then_some(text)
}

fn scalar_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The players of one side: every object with a `champion` field, not looking inside a player.
/// An array of player objects (0.6.0) is the common case; a `players`/`members` list inside an
/// object is preferred over other lists (bans and the like).
pub fn players(team: &Value) -> Vec<Player> {
    if let Value::Object(map) = team {
        for key in ["players", "members", "lineup", "athletes", "picks"] {
            if let Some(list) = map.get(key) {
                let found = players(list);
                if !found.is_empty() {
                    return found;
                }
            }
        }
    }
    let mut out = Vec::new();
    collect_players(team, &mut out, 0);
    out
}

fn collect_players(v: &Value, out: &mut Vec<Player>, depth: usize) {
    if depth > 6 {
        return;
    }
    match v {
        Value::Object(map) => {
            if let Some(player) = player(map) {
                out.push(player);
                return;
            }
            for (key, child) in map {
                if key.contains("ban") {
                    continue;
                }
                collect_players(child, out, depth + 1);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_players(item, out, depth + 1);
            }
        }
        _ => {}
    }
}

fn player(map: &Map<String, Value>) -> Option<Player> {
    let champion = match map.get("champion")? {
        Value::String(s) if !s.is_empty() => s.clone(),
        Value::Object(inner) => ["name", "id", "key"]
            .iter()
            .find_map(|k| inner.get(*k).and_then(Value::as_str))
            .filter(|s| !s.is_empty())?
            .to_string(),
        _ => return None,
    };
    let position = ["position", "lane", "role"]
        .iter()
        .find_map(|k| map.get(*k))
        .and_then(label);
    Some(Player { champion, position })
}

/// Lane labels: `"Top"`, or a unit enum written as `{"Top": null}`, or an index.
fn label(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Object(map) if map.len() == 1 => map.keys().next().cloned(),
        _ => None,
    }
}

/// The value of the shallowest `key` anywhere in the document (breadth first).
pub fn find_key<'a>(doc: &'a Value, key: &str) -> Option<&'a Value> {
    let mut level: Vec<&Value> = vec![doc];
    for _ in 0..10 {
        let mut next = Vec::new();
        for v in level {
            match v {
                Value::Object(map) => {
                    if let Some(found) = map.get(key) {
                        return Some(found);
                    }
                    next.extend(map.values());
                }
                Value::Array(items) => next.extend(items.iter()),
                _ => {}
            }
        }
        if next.is_empty() {
            return None;
        }
        level = next;
    }
    None
}

/// `a, b, c` - the document's top-level keys, for the probe lines in `diag.log`.
pub fn top_keys(doc: &Value) -> String {
    match doc {
        Value::Object(map) => map.keys().take(40).cloned().collect::<Vec<_>>().join(", "),
        Value::Array(items) => format!("(array of {})", items.len()),
        other => format!("({})", kind(other)),
    }
}

pub fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Orders in-game patch versions: numeric parts compared as numbers (`1.10` > `1.9`), then the
/// text itself.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    fn parts(v: &str) -> Vec<u64> {
        v.split(|c: char| !c.is_ascii_digit())
            .filter(|p| !p.is_empty())
            .map(|p| p.parse::<u64>().unwrap_or(u64::MAX))
            .collect()
    }
    let (pa, pb) = (parts(a), parts(b));
    let len = pa.len().max(pb.len());
    for i in 0..len {
        let (x, y) = (pa.get(i).copied().unwrap_or(0), pb.get(i).copied().unwrap_or(0));
        if x != y {
            return x.cmp(&y);
        }
    }
    a.cmp(b)
}

/// Whether two version strings have the same shape (`1.3` vs `12.1`, not `1.3` vs `Season 2`).
pub fn same_version_shape(a: &str, b: &str) -> bool {
    let shape = |v: &str| -> String {
        v.chars().map(|c| if c.is_ascii_digit() { '0' } else { c }).collect::<String>()
    };
    let squash = |s: String| -> String {
        let mut out = String::new();
        for c in s.chars() {
            if !(c == '0' && out.ends_with('0')) {
                out.push(c);
            }
        }
        out
    };
    let (sa, sb) = (squash(shape(a)), squash(shape(b)));
    sa == sb && sa.contains('0')
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLAY_060: &str = r#"{
        "version": "1.3", "seed": 99, "blue_team_win": true,
        "blue_team": [
            {"athlete": 1, "champion": "fighter", "position": "Top", "items": [{"champion": "x"}]},
            {"athlete": 2, "champion": "ninja", "position": "Jungle"},
            {"athlete": 3, "champion": "fire_mage", "position": "Mid"},
            {"athlete": 4, "champion": "archer", "position": "Bottom"},
            {"athlete": 5, "champion": "priest", "position": "Support"}],
        "red_team": [
            {"champion": "monk", "position": "Top"},
            {"champion": "vampire", "position": "Jungle"},
            {"champion": "ice_mage", "position": "Mid"},
            {"champion": "sniper", "position": "Bottom"},
            {"champion": "shaman", "position": "Support"}],
        "inputs": [[1, 2, 3]], "events": {"kills": [{"champion": "ninja"}]}
    }"#;

    #[test]
    fn reads_the_060_layout() {
        let Parsed::Match(m) = parse_record(REPLAY_060, false) else { panic!() };
        assert_eq!(m.version, "1.3");
        assert!(m.blue_win && !m.searched);
        assert_eq!(m.blue.len(), 5);
        assert_eq!(m.red.len(), 5);
        assert_eq!(m.blue[0], Player { champion: "fighter".into(), position: Some("Top".into()) });
        assert_eq!(m.red[4].champion, "shaman");
    }

    #[test]
    fn finds_moved_fields_and_skips_bans() {
        let json = r#"{"info": {"version": {"major": 2, "minor": 10}, "result": {"blue_team_win": false}},
            "teams": {"blue_team": {"bans": [{"champion": "monk"}],
                                     "players": [{"champion": {"name": "fighter"}, "lane": {"Top": null}}]},
                      "red_team": [{"champion": "ninja", "role": 1}]}}"#;
        let Parsed::Match(m) = parse_record(json, false) else { panic!() };
        assert_eq!(m.version, "2.10");
        assert!(!m.blue_win && m.searched);
        assert_eq!(m.blue, [Player { champion: "fighter".into(), position: Some("Top".into()) }]);
        assert_eq!(m.red, [Player { champion: "ninja".into(), position: Some("1".into()) }]);
    }

    #[test]
    fn solo_rank_needs_played() {
        let unplayed = r#"{"played": false, "version": "1.3", "blue_team_win": false,
                           "blue_team": [{"champion": "a"}], "red_team": [{"champion": "b"}]}"#;
        assert_eq!(parse_record(unplayed, true), Parsed::NotPlayed);
        let played = unplayed.replace("\"played\": false", "\"played\": true");
        assert!(matches!(parse_record(&played, true), Parsed::Match(_)));
        assert!(matches!(parse_record(unplayed, false), Parsed::Match(_)), "competition ignores it");
        // the flag may sit deeper in a reshuffled record; no flag at all = not usable
        let nested = r#"{"info": {"played": false}, "version": "1.3", "blue_team_win": true,
                         "blue_team": [{"champion": "a"}], "red_team": [{"champion": "b"}]}"#;
        assert_eq!(parse_record(nested, true), Parsed::NotPlayed);
        let none = r#"{"version": "1.3", "blue_team_win": true,
                       "blue_team": [{"champion": "a"}], "red_team": [{"champion": "b"}]}"#;
        assert!(matches!(parse_record(none, true), Parsed::Invalid(why) if why.starts_with("no played")));
    }

    #[test]
    fn reports_what_is_missing() {
        let Parsed::Invalid(why) = parse_record(r#"{"date": 1, "teams": []}"#, false) else { panic!() };
        assert!(why.contains("no version") && why.contains("top-level keys: date, teams"), "{why}");
        let Parsed::Invalid(why) = parse_record(
            r#"{"version": "1.3", "blue_team": [{"champion": "a"}], "red_team": []}"#,
            false,
        ) else {
            panic!()
        };
        assert!(why.starts_with("no blue_team_win"), "{why}");
        assert!(matches!(parse_record("not json", false), Parsed::Invalid(_)));
    }

    #[test]
    fn version_order_and_shape() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_versions("1.10", "1.9"), Greater);
        assert_eq!(compare_versions("2.0", "1.12.3"), Greater);
        assert_eq!(compare_versions("1.3", "1.3"), Equal);
        assert!(same_version_shape("1.3", "12.10"));
        assert!(!same_version_shape("1.3", "1.3.1"));
        assert!(!same_version_shape("1.3", "Season"));
    }
}
