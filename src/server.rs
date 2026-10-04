//! Feature 2, server half: writes the tier list the client computed into the player team's
//! `champion_tiers` (records can only be changed on the management server; the change reaches
//! the client with the next in-game day).
//!
//! Game 0.6 stores `champion_tiers` as `{"<champion>": "S" | "A" | "B" | "C" | "D" | "NoTier"}`.
//! A write the game's schema rejects changes nothing, so when the whole object is refused this
//! tries the same list without `NoTier`, then champion by champion, and says in `diag.log` what
//! the field looked like - enough to adapt the mod if a game update changes it.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Mutex, PoisonError};

use mod_api_stable::{CommandResultV1, StableCommand, StableServerCtx, StableServerExtension};
use serde_json::{Map, Value};

use crate::diag;
use crate::records::{kind, top_keys};

pub const APPLY_TIERS: &str = "apply_tiers";
pub const RESULT_EVENT: &str = "apply_tiers_result";
pub const FIELD: &str = "champion_tiers";

pub struct ServerExt;

/// Teams whose `champion_tiers` layout was already described in `diag.log`.
static PROBED: Mutex<Option<HashSet<usize>>> = Mutex::new(None);

pub fn reset_for_tests() {
    *PROBED.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

impl StableServerExtension for ServerExt {
    fn handle_command(&self, ctx: &mut StableServerCtx<'_>, cmd: &StableCommand<'_>) -> CommandResultV1 {
        if cmd.command != APPLY_TIERS {
            return CommandResultV1::Pass;
        }
        let reply = cmd.reply_target();
        let outcome = match Request::parse(cmd.payload, cmd.sender_team_id) {
            Ok(request) => apply(ctx, &request),
            Err(why) => {
                diag::log(&format!("[server] bad {APPLY_TIERS} payload: {why}"));
                Outcome { hash: String::new(), ok: false, detail: why }
            }
        };
        let message = format!(
            "{}\t{}\t{}",
            outcome.hash,
            if outcome.ok { "ok" } else { "fail" },
            outcome.detail
        );
        ctx.emit_event(reply, RESULT_EVENT, message.as_bytes());
        CommandResultV1::Handled
    }
}

/// What the client sends: `v2`, team id, a hash of the list, `keep`/`clear`, then
/// `champion<TAB>tier` lines.
#[derive(Debug, PartialEq)]
pub struct Request {
    pub team: usize,
    pub hash: String,
    pub keep_others: bool,
    pub tiers: BTreeMap<String, String>,
}

impl Request {
    pub fn encode(&self) -> String {
        let mut out = format!(
            "v2\n{}\n{}\n{}\n",
            self.team,
            self.hash,
            if self.keep_others { "keep" } else { "clear" }
        );
        for (name, tier) in &self.tiers {
            out.push_str(name);
            out.push('\t');
            out.push_str(tier);
            out.push('\n');
        }
        out
    }

    pub fn parse(payload: &[u8], sender_team: Option<usize>) -> Result<Request, String> {
        let text = String::from_utf8_lossy(payload);
        let mut lines = text.lines();
        if lines.next() != Some("v2") {
            return Err("unknown format".to_string());
        }
        let team = lines
            .next()
            .and_then(|l| l.trim().parse::<usize>().ok())
            .or(sender_team)
            .ok_or("no team id")?;
        let hash = lines.next().unwrap_or("").trim().to_string();
        let keep_others = lines.next().map(str::trim) != Some("clear");
        let mut tiers = BTreeMap::new();
        for line in lines {
            if let Some((name, tier)) = line.split_once('\t') {
                let (name, tier) = (name.trim(), tier.trim());
                if !name.is_empty() && !tier.is_empty() {
                    tiers.insert(name.to_string(), tier.to_string());
                }
            }
        }
        if tiers.is_empty() {
            return Err("empty tier list".to_string());
        }
        Ok(Request { team, hash, keep_others, tiers })
    }
}

pub struct Outcome {
    pub hash: String,
    pub ok: bool,
    pub detail: String,
}

/// The part of the server context this needs (so tests can run it without a game).
pub trait TeamDocs {
    fn team_json(&self, team: usize, path: &str) -> Option<String>;
    fn set_team_json(&mut self, team: usize, path: &str, json: &str) -> bool;
}

impl TeamDocs for StableServerCtx<'_> {
    fn team_json(&self, team: usize, path: &str) -> Option<String> {
        self.team_get_json(team, path)
    }
    fn set_team_json(&mut self, team: usize, path: &str, json: &str) -> bool {
        self.team_set_json(team, path, json)
    }
}

pub fn apply(docs: &mut impl TeamDocs, req: &Request) -> Outcome {
    let existing: Option<Value> =
        docs.team_json(req.team, FIELD).and_then(|s| serde_json::from_str(&s).ok());
    probe(docs, req.team, existing.as_ref());

    let mut object = Map::new();
    if req.keep_others {
        if let Some(Value::Object(old)) = &existing {
            object = old.clone();
        }
    }
    for (name, tier) in &req.tiers {
        object.insert(name.clone(), Value::String(tier.clone()));
    }
    let summary = summarize(&req.tiers);
    let done = |ok: bool, how: &str| {
        let detail = format!("{how}; {summary}");
        diag::log(&format!(
            "[server] team {} {}: {detail}",
            req.team,
            if ok { "tier list written" } else { "tier list NOT written" }
        ));
        Outcome { hash: req.hash.clone(), ok, detail }
    };

    if docs.set_team_json(req.team, FIELD, &Value::Object(object.clone()).to_string()) {
        return done(true, "whole list");
    }
    // the schema may not know "NoTier": leave those champions out
    let without: Map<String, Value> =
        object.iter().filter(|(_, v)| v.as_str() != Some("NoTier")).map(|(k, v)| (k.clone(), v.clone())).collect();
    if without.len() != object.len()
        && docs.set_team_json(req.team, FIELD, &Value::Object(without).to_string())
    {
        return done(true, "whole list without NoTier");
    }
    // one champion at a time: whatever the schema accepts goes in
    let mut written = 0;
    let mut refused = Vec::new();
    for (name, tier) in &req.tiers {
        if name.contains('.') {
            continue;
        }
        let path = format!("{FIELD}.{name}");
        if docs.set_team_json(req.team, &path, &Value::String(tier.clone()).to_string()) {
            written += 1;
        } else if refused.len() < 5 {
            refused.push(format!("{name}={tier}"));
        }
    }
    let how = format!(
        "whole list refused; {written}/{} written one by one{}",
        req.tiers.len(),
        if refused.is_empty() { String::new() } else { format!(" (refused e.g. {})", refused.join(", ")) }
    );
    done(written > 0, &how)
}

fn summarize(tiers: &BTreeMap<String, String>) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for tier in tiers.values() {
        *counts.entry(tier.as_str()).or_insert(0) += 1;
    }
    let order = ["S", "A", "B", "C", "D", "NoTier"];
    let mut parts: Vec<String> = order
        .iter()
        .filter_map(|t| counts.get(t).map(|n| format!("{t} {n}")))
        .collect();
    parts.extend(counts.iter().filter(|(t, _)| !order.contains(t)).map(|(t, n)| format!("{t} {n}")));
    format!("{} champions ({})", tiers.len(), parts.join(", "))
}

/// Describes the team's `champion_tiers` once per team and session.
fn probe(docs: &impl TeamDocs, team: usize, existing: Option<&Value>) {
    {
        let mut guard = PROBED.lock().unwrap_or_else(PoisonError::into_inner);
        if !guard.get_or_insert_with(HashSet::new).insert(team) {
            return;
        }
    }
    let text = match existing {
        Some(Value::Object(map)) => {
            let mut values: Vec<String> = map
                .values()
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => kind(other).to_string(),
                })
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            values.sort();
            format!("object with {} entries, values: {}", map.len(), values.join(", "))
        }
        Some(other) => format!("{} ({})", kind(other), diag::clip(&other.to_string(), 120)),
        None => {
            let keys = docs
                .team_json(team, "")
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .map(|doc| top_keys(&doc))
                .unwrap_or_else(|| "(team record not readable)".to_string());
            format!("missing; team record keys: {keys}")
        }
    };
    diag::log(&format!("[server] probe: team {team} {FIELD} is {text}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A team record whose `champion_tiers` accepts only the given tier names.
    struct Team {
        doc: HashMap<String, Value>,
        allowed: Vec<&'static str>,
        writes: u32,
    }

    impl TeamDocs for Team {
        fn team_json(&self, _team: usize, path: &str) -> Option<String> {
            if path.is_empty() {
                return serde_json::to_string(&self.doc).ok();
            }
            self.doc.get(path).map(|v| v.to_string())
        }
        fn set_team_json(&mut self, _team: usize, path: &str, json: &str) -> bool {
            self.writes += 1;
            let Ok(value) = serde_json::from_str::<Value>(json) else { return false };
            let valid = |v: &Value| v.as_str().is_some_and(|s| self.allowed.contains(&s));
            if let Some(champion) = path.strip_prefix("champion_tiers.") {
                if !valid(&value) {
                    return false;
                }
                let Some(Value::Object(map)) = self.doc.get_mut(FIELD) else { return false };
                map.insert(champion.to_string(), value);
                return true;
            }
            match &value {
                Value::Object(map) if map.values().all(valid) => {
                    self.doc.insert(path.to_string(), value);
                    true
                }
                _ => false,
            }
        }
    }

    fn team(allowed: Vec<&'static str>) -> Team {
        let mut doc = HashMap::new();
        doc.insert("name".to_string(), Value::String("T1".into()));
        doc.insert(FIELD.to_string(), serde_json::json!({"fighter": "S", "monk": "C"}));
        Team { doc, allowed, writes: 0 }
    }

    fn request(keep: bool, tiers: &[(&str, &str)]) -> Request {
        Request {
            team: 3,
            hash: "h1".into(),
            keep_others: keep,
            tiers: tiers.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
        }
    }

    #[test]
    fn payload_round_trip() {
        let req = request(true, &[("fighter", "A"), ("ninja", "D")]);
        assert_eq!(Request::parse(req.encode().as_bytes(), None).unwrap(), req);
        let clear = request(false, &[("fighter", "A")]);
        assert_eq!(Request::parse(clear.encode().as_bytes(), None).unwrap(), clear);
        assert!(Request::parse(b"v1\n3\n", None).is_err());
        assert!(Request::parse(b"v2\nx\nh\nkeep\n", Some(1)).is_err(), "empty list");
        assert_eq!(Request::parse(b"v2\n\nh\nkeep\nfighter\tS\n", Some(9)).unwrap().team, 9);
    }

    #[test]
    fn keep_merges_into_the_existing_list() {
        let mut t = team(vec!["S", "A", "B", "C", "D", "NoTier"]);
        let out = apply(&mut t, &request(true, &[("fighter", "B"), ("ninja", "A")]));
        assert!(out.ok && out.hash == "h1");
        assert_eq!(t.doc[FIELD], serde_json::json!({"fighter": "B", "monk": "C", "ninja": "A"}));
        assert_eq!(t.writes, 1);
    }

    #[test]
    fn clear_replaces_and_falls_back_without_notier() {
        let mut t = team(vec!["S", "A", "B", "C", "D"]);
        let out = apply(&mut t, &request(false, &[("fighter", "B"), ("ninja", "NoTier")]));
        assert!(out.ok && out.detail.starts_with("whole list without NoTier"), "{}", out.detail);
        assert_eq!(t.doc[FIELD], serde_json::json!({"fighter": "B"}));
    }

    #[test]
    fn one_by_one_when_the_whole_list_is_refused() {
        let mut t = team(vec!["A", "B"]);
        let out = apply(&mut t, &request(true, &[("fighter", "B"), ("ninja", "S"), ("odd.name", "A")]));
        assert!(out.ok, "{}", out.detail);
        assert!(out.detail.contains("1/3 written one by one") && out.detail.contains("ninja=S"), "{}", out.detail);
        assert_eq!(t.doc[FIELD]["fighter"], "B");
    }
}
