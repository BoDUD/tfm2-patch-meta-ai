//! Reading one match out of a `MatchReplay` (competition) or `SoloRankMatch` record.
//!
//! The documents are JSON from `record_get_json(kind, id, "")`. In game 0.6 they carry a
//! top-level `version` (the in-game patch, e.g. `"1.3"`), `blue_team_win`, and `blue_team` /
//! `red_team` arrays whose player entries have `champion` and `position`; solo-rank records
//! also carry `played`. That layout is read directly (unknown fields are skipped without being
//! built). When a field is not where it used to be, the whole document is searched for it, so a
//! reshuffled record still parses, and the probe in `diag.log` says which layout was found.
//! Only real player entries count - a `champion` key in bans or other nested data does not.
//!
//! Also read when present (game 0.6 `MatchReplayData`): `seed` (the same match keeps it when
//! the game hands the record a new id), `blue_team_id` / `red_team_id`, `blue_ban` / `red_ban`,
//! and each player's athlete id (`athlete` or `athlete_id`, a number or an object with `id`).
//!
//! Also from competition records: each side's team strategy (`blue_strategy`: `early_jungle`,
//! `minion_wave`, `game_finish`... -> the chosen option, or the variant name of an option that
//! carries data, e.g. `{"Split131": {...}}` -> `Split131`), each player's gold at the end of the
//! lane phase (`blue_performance.gold_line_phase`, in team-list order) and the game's length in
//! ticks (`game_tick`).
//!
//! Solo-rank records name no lanes. Each player's `stat` holds their rating in every position
//! (`top`, `jungle`, `mid`, `bottom`, `support`), so a side without lanes gets the one-to-one
//! assignment of its players to lanes with the highest total rating.

use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Player {
    pub champion: String,
    pub position: Option<String>,
    pub athlete: Option<u32>,
    /// Gold at the end of the lane phase.
    pub lane_gold: Option<i32>,
}

const LANES: [&str; 5] = ["Top", "Jungle", "Mid", "Bottom", "Support"];

/// A player's rating in each lane (`stat.top` ... `stat.support`), when the record has them.
fn lane_ratings(map: &Map<String, Value>) -> Option<[f64; 5]> {
    let stat = map.get("stat")?.as_object()?;
    let mut out = [0.0; 5];
    for (slot, lane) in out.iter_mut().zip(LANES) {
        *slot = stat.get(&lane.to_ascii_lowercase())?.as_f64()?;
    }
    Some(out)
}

/// Lanes for a side whose record names none: the assignment with the highest total rating.
fn assign_lanes(players: &mut [Player], ratings: &[Option<[f64; 5]>]) {
    if players.len() != 5 || players.iter().any(|p| p.position.is_some()) || ratings.iter().any(Option::is_none) {
        return;
    }
    let ratings: Vec<[f64; 5]> = ratings.iter().flatten().copied().collect();
    let mut best = (f64::MIN, [0usize; 5]);
    let mut lanes = [0usize, 1, 2, 3, 4];
    // every permutation of five lanes (Heap's algorithm)
    fn permute(k: usize, lanes: &mut [usize; 5], ratings: &[[f64; 5]], best: &mut (f64, [usize; 5])) {
        if k == 1 {
            let total: f64 = lanes.iter().enumerate().map(|(p, l)| ratings[p][*l]).sum();
            if total > best.0 {
                *best = (total, *lanes);
            }
            return;
        }
        for i in 0..k {
            permute(k - 1, lanes, ratings, best);
            if k.is_multiple_of(2) {
                lanes.swap(i, k - 1);
            } else {
                lanes.swap(0, k - 1);
            }
        }
    }
    permute(5, &mut lanes, &ratings, &mut best);
    for (p, l) in players.iter_mut().zip(best.1) {
        p.position = Some(LANES[l].to_string());
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MatchSummary {
    pub version: String,
    pub blue_win: bool,
    pub blue: Vec<Player>,
    pub red: Vec<Player>,
    /// The layout differed from 0.6.0's and the fields were found by searching.
    pub searched: bool,
    pub seed: Option<u64>,
    /// Blue, red.
    pub teams: [Option<u32>; 2],
    pub bans: [Vec<String>; 2],
    /// Each side's team strategy: (setting, chosen option).
    pub strategies: [Vec<(String, String)>; 2],
    /// Game length in ticks.
    pub length: Option<u32>,
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
    #[serde(default)]
    seed: Option<Value>,
    #[serde(default)]
    blue_team_id: Option<Value>,
    #[serde(default)]
    red_team_id: Option<Value>,
    #[serde(default)]
    blue_ban: Option<Value>,
    #[serde(default)]
    red_ban: Option<Value>,
    #[serde(default)]
    blue_strategy: Option<Value>,
    #[serde(default)]
    red_strategy: Option<Value>,
    #[serde(default)]
    blue_performance: Option<Value>,
    #[serde(default)]
    red_performance: Option<Value>,
    #[serde(default)]
    game_tick: Option<Value>,
}

struct Fields<'a> {
    version: Option<&'a Value>,
    blue_win: Option<&'a Value>,
    blue: Option<&'a Value>,
    red: Option<&'a Value>,
    played: Option<&'a Value>,
    seed: Option<&'a Value>,
    teams: [Option<&'a Value>; 2],
    bans: [Option<&'a Value>; 2],
    strategies: [Option<&'a Value>; 2],
    performance: [Option<&'a Value>; 2],
    length: Option<&'a Value>,
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
            seed: layout.seed.as_ref(),
            teams: [layout.blue_team_id.as_ref(), layout.red_team_id.as_ref()],
            bans: [layout.blue_ban.as_ref(), layout.red_ban.as_ref()],
            strategies: [layout.blue_strategy.as_ref(), layout.red_strategy.as_ref()],
            performance: [layout.blue_performance.as_ref(), layout.red_performance.as_ref()],
            length: layout.game_tick.as_ref(),
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
        seed: find_key(&doc, "seed"),
        teams: [find_key(&doc, "blue_team_id"), find_key(&doc, "red_team_id")],
        bans: [find_key(&doc, "blue_ban"), find_key(&doc, "red_ban")],
        strategies: [find_key(&doc, "blue_strategy"), find_key(&doc, "red_strategy")],
        performance: [find_key(&doc, "blue_performance"), find_key(&doc, "red_performance")],
        length: find_key(&doc, "game_tick"),
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
    let mut blue = players(f.blue.ok_or("blue_team")?);
    let mut red = players(f.red.ok_or("red_team")?);
    lane_gold(&mut blue, f.performance[0]);
    lane_gold(&mut red, f.performance[1]);
    let blue_win = f.blue_win.and_then(as_bool).ok_or("blue_team_win")?;
    if blue.is_empty() && red.is_empty() {
        return Ok(Parsed::Invalid("no champions in blue_team / red_team".to_string()));
    }
    Ok(Parsed::Match(MatchSummary {
        version,
        blue_win,
        blue,
        red,
        searched,
        seed: f.seed.and_then(as_u64),
        teams: [f.teams[0].and_then(id), f.teams[1].and_then(id)],
        bans: [f.bans[0].map(names).unwrap_or_default(), f.bans[1].map(names).unwrap_or_default()],
        strategies: [f.strategies[0].map(strategy).unwrap_or_default(), f.strategies[1].map(strategy).unwrap_or_default()],
        length: f.length.and_then(as_u64).and_then(|n| u32::try_from(n).ok()),
    }))
}

/// A team strategy as (setting, chosen option): a plain option, or the variant name of an
/// option that carries data (`{"Split131": {...}}`).
pub fn strategy(v: &Value) -> Vec<(String, String)> {
    let Value::Object(map) = v else { return Vec::new() };
    let mut out: Vec<(String, String)> = map
        .iter()
        .filter_map(|(k, v)| {
            let option = match v {
                Value::String(s) if !s.is_empty() => s.clone(),
                Value::Object(inner) if inner.len() == 1 => inner.keys().next()?.clone(),
                _ => return None,
            };
            Some((k.clone(), option))
        })
        .collect();
    out.sort();
    out
}

/// Each player's gold at the end of the lane phase: `gold_line_phase`, one number per player in
/// the order of the team's list (only when the counts agree).
fn lane_gold(players: &mut [Player], performance: Option<&Value>) {
    let Some(list) = performance.and_then(|p| p.get("gold_line_phase")).and_then(Value::as_array) else { return };
    if list.len() != players.len() {
        return;
    }
    for (p, g) in players.iter_mut().zip(list) {
        p.lane_gold = g.as_i64().and_then(|g| i32::try_from(g).ok());
    }
}

fn as_u64(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64().or_else(|| n.as_i64().map(|i| i as u64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// An id: a number, a numeric string, `{"id": n}` or a one-variant enum `{"Some": n}`.
fn id(v: &Value) -> Option<u32> {
    match v {
        Value::Object(map) => ["id", "athlete_id", "team_id", "Some"]
            .iter()
            .find_map(|k| map.get(*k))
            .and_then(id),
        other => as_u64(other).and_then(|n| u32::try_from(n).ok()),
    }
}

/// Champion names in a ban list: strings, `{"champion": ...}` / `{"name": ...}` objects, or
/// `null` for an empty ban, at any nesting.
fn names(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(v: &Value, out: &mut Vec<String>, depth: usize) {
        if depth > 4 {
            return;
        }
        match v {
            Value::String(s) if !s.is_empty() => out.push(s.clone()),
            Value::Array(items) => items.iter().for_each(|i| walk(i, out, depth + 1)),
            Value::Object(map) => {
                if let Some(name) = ["champion", "name", "key"].iter().find_map(|k| map.get(*k)) {
                    walk(name, out, depth + 1);
                } else {
                    map.values().for_each(|i| walk(i, out, depth + 1));
                }
            }
            _ => {}
        }
    }
    walk(v, &mut out, 0);
    out
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
    let mut ratings = Vec::new();
    collect_players(team, &mut out, &mut ratings, 0);
    assign_lanes(&mut out, &ratings);
    out
}

fn collect_players(v: &Value, out: &mut Vec<Player>, ratings: &mut Vec<Option<[f64; 5]>>, depth: usize) {
    if depth > 6 {
        return;
    }
    match v {
        Value::Object(map) => {
            if let Some(player) = player(map) {
                out.push(player);
                ratings.push(lane_ratings(map));
                return;
            }
            for (key, child) in map {
                if key.contains("ban") {
                    continue;
                }
                collect_players(child, out, ratings, depth + 1);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_players(item, out, ratings, depth + 1);
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
    let athlete = ["athlete", "athlete_id"].iter().find_map(|k| map.get(*k)).and_then(id);
    Some(Player { champion, position, athlete, lane_gold: None })
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
        "blue_team_id": 7, "red_team_id": {"id": 12},
        "blue_ban": ["monk", null, {"champion": "ninja"}], "red_ban": [{"name": "sniper"}],
        "game_tick": 50848,
        "blue_strategy": {"early_jungle": "CounterJungle", "morgard_use": {"Split131": {"position1": "Top"}}, "odd": 3},
        "red_strategy": {"early_jungle": "GrowthAndCover"},
        "blue_performance": {"gold_line_phase": [3101, 2104, 2301, 2502, 1326], "kills": [1, 2, 3, 4, 5]},
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
        assert_eq!(
            m.blue[0],
            Player { champion: "fighter".into(), position: Some("Top".into()), athlete: Some(1), lane_gold: Some(3101) }
        );
        assert_eq!(m.blue[4].lane_gold, Some(1326));
        assert_eq!(m.red[0].lane_gold, None, "no performance for red");
        assert_eq!(m.length, Some(50848));
        assert_eq!(
            m.strategies[0],
            [("early_jungle".to_string(), "CounterJungle".to_string()), ("morgard_use".to_string(), "Split131".to_string())]
        );
        assert_eq!(m.strategies[1], [("early_jungle".to_string(), "GrowthAndCover".to_string())]);
        assert_eq!(m.red[4].champion, "shaman");
        assert_eq!(m.red[4].athlete, None);
        assert_eq!(m.seed, Some(99));
        assert_eq!(m.teams, [Some(7), Some(12)]);
        assert_eq!(m.bans, [vec!["monk".to_string(), "ninja".to_string()], vec!["sniper".to_string()]]);
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
        assert_eq!(m.blue, [Player { champion: "fighter".into(), position: Some("Top".into()), athlete: None, lane_gold: None }]);
        assert_eq!(m.red, [Player { champion: "ninja".into(), position: Some("1".into()), athlete: None, lane_gold: None }]);
        assert_eq!(m.bans, [Vec::<String>::new(), Vec::new()], "bans inside a team are not read");
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
    fn solo_rank_lanes_come_from_the_players_ratings() {
        let player = |c: &str, t: u32, j: u32, m: u32, b: u32, s: u32| {
            format!(r#"{{"champion":"{c}","athlete_id":1,"stat":{{"top":{t},"jungle":{j},"mid":{m},"bottom":{b},"support":{s},"ego":50}}}}"#)
        };
        let json = format!(
            r#"{{"played":true,"version":"1.3","blue_team_win":true,"blue_team":[{},{},{},{},{}],"red_team":[{}]}}"#,
            player("a", 0, 100, 0, 0, 0),
            player("b", 90, 0, 0, 0, 10),
            player("c", 0, 0, 0, 60, 70),
            player("d", 0, 0, 100, 0, 0),
            player("e", 0, 0, 0, 80, 75),
            player("x", 100, 0, 0, 0, 0),
        );
        let Parsed::Match(m) = parse_record(&json, true) else { panic!() };
        let lanes: Vec<&str> = m.blue.iter().map(|p| p.position.as_deref().unwrap()).collect();
        // c and e both lean bottom; the best total gives e bottom and c support
        assert_eq!(lanes, ["Jungle", "Top", "Support", "Mid", "Bottom"]);
        assert_eq!(m.red[0].position, None, "an incomplete side gets no guess");
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
