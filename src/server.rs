//! Feature 2, server half: writes the tier list the client computed into the player team's
//! `champion_tiers` (records can only be changed on the management server; the change reaches
//! the client with the next in-game day).
//!
//! The balance patches' server half (2.8.0, `apply_balance`): with `balance` on, the save's own auto-patcher makes no
//! champion change (every count zero; its values kept in the save to put back when `balance` is turned off), and
//! each new round's notes go to the player's inbox once.
//!
//! Game 0.6 stores `champion_tiers` as `{"<champion>": "S" | "A" | "B" | "C" | "D" | "NoTier"}`.
//! A write the game's schema rejects changes nothing, so when the whole object is refused this
//! tries the same list without `NoTier`, then champion by champion, and says in `diag.log` what
//! the field looked like - enough to adapt the mod if a game update changes it.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Mutex, PoisonError};

use mod_api_stable::{CommandResultV1, SettingTargetV1, StableCommand, StableServerCtx, StableServerExtension};
use serde_json::{Map, Value};

use crate::diag;
use crate::records::{kind, top_keys};

pub const APPLY_TIERS: &str = "apply_tiers";
pub const RESULT_EVENT: &str = "apply_tiers_result";
pub const FIELD: &str = "champion_tiers";
/// The balance patches (2.8.0): the request, the answer.
pub const APPLY_BALANCE: &str = "apply_balance";
pub const BALANCE_RESULT: &str = "apply_balance_result";
/// The save's own auto-patcher values while the balance has them off, and the newest round put in the news.
const AUTOPATCH_KEPT: &str = "pma_balance_autopatch";
const NEWS_DONE: &str = "pma_balance_news";
/// The news' author.
const AUTHOR: &str = "智能BP / Patch Meta AI";

pub struct ServerExt;

/// Teams whose `champion_tiers` layout was already described in `diag.log`.
static PROBED: Mutex<Option<HashSet<usize>>> = Mutex::new(None);

pub fn reset_for_tests() {
    *PROBED.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

impl StableServerExtension for ServerExt {
    fn handle_command(&self, ctx: &mut StableServerCtx<'_>, cmd: &StableCommand<'_>) -> CommandResultV1 {
        if cmd.command == APPLY_BALANCE {
            let reply = cmd.reply_target();
            let (state, ok, detail) = match BalanceRequest::parse(cmd.payload, cmd.sender_team_id) {
                Ok(req) => {
                    let (ok, detail) = balance(ctx, &req);
                    (format!("{}/{}", u8::from(req.off), req.round), ok, detail)
                }
                Err(why) => (String::new(), false, why),
            };
            let message = format!("{state}\t{}\t{detail}", if ok { "ok" } else { "fail" });
            ctx.emit_event(reply, BALANCE_RESULT, message.as_bytes());
            return CommandResultV1::Handled;
        }
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

// ===================== the balance patches (2.8.0) =====================

/// What the client sends: `v1`, team id, `1` (the game's champion patches off) or `0` (put back), the newest round,
/// its news title, then the news lines.
#[derive(Debug, PartialEq)]
pub struct BalanceRequest {
    pub team: usize,
    pub off: bool,
    pub round: u32,
    pub title: String,
    pub content: String,
}

impl BalanceRequest {
    pub fn parse(payload: &[u8], sender_team: Option<usize>) -> Result<BalanceRequest, String> {
        let text = String::from_utf8_lossy(payload);
        let mut lines = text.split('\n');
        if lines.next().map(str::trim) != Some("v1") {
            return Err("unknown format".to_string());
        }
        let team = lines.next().and_then(|l| l.trim().parse::<usize>().ok()).or(sender_team).ok_or("no team id")?;
        let off = lines.next().map(str::trim) == Some("1");
        let round = lines.next().and_then(|l| l.trim().parse::<u32>().ok()).unwrap_or(0);
        let title = lines.next().unwrap_or("").trim().to_string();
        let content = lines.collect::<Vec<_>>().join("\n");
        Ok(BalanceRequest { team, off, round, title, content })
    }
}

/// The part of the server context the balance needs (so tests can run it without a game).
pub trait BalanceHost {
    fn autopatch(&self) -> Option<String>;
    fn set_autopatch(&mut self, json: &str) -> bool;
    fn saved(&self, key: &str) -> Option<String>;
    fn set_saved(&mut self, key: &str, value: Option<&str>) -> bool;
    fn push_news(&mut self, team: usize, title: &str, content: &str) -> bool;
}

impl BalanceHost for StableServerCtx<'_> {
    fn autopatch(&self) -> Option<String> {
        self.setting_get_json(SettingTargetV1::AutoPatch, "")
    }
    fn set_autopatch(&mut self, json: &str) -> bool {
        self.setting_set_json(SettingTargetV1::AutoPatch, "", json)
    }
    fn saved(&self, key: &str) -> Option<String> {
        self.save_get_string(key)
    }
    fn set_saved(&mut self, key: &str, value: Option<&str>) -> bool {
        match value {
            Some(v) => self.save_set_string(key, v),
            None => self.save_remove_key(key),
        }
    }
    fn push_news(&mut self, team: usize, title: &str, content: &str) -> bool {
        self.news_push(team, title, content, AUTHOR)
    }
}

/// The auto-patcher's settings with no champion change: every `*_count` zero. A document of zeros reads as unset (the
/// game's defaults), so a threshold stays as it was - or, all of them zero too, one is set where it changes nothing.
pub fn champion_patches_off(doc: &Value) -> Option<Value> {
    let Value::Object(map) = doc else { return None };
    let mut out = map.clone();
    for (k, v) in out.iter_mut() {
        if k.ends_with("_count") && v.is_number() {
            *v = Value::from(0);
        }
    }
    let all_zero = out.values().all(|v| v.as_f64() == Some(0.0));
    if all_zero {
        let key = ["repeat_high_streak_threshold", "repeat_medium_streak_threshold", "meta_stale_high_threshold"]
            .into_iter()
            .find(|k| out.contains_key(*k))
            .map(str::to_string)
            .or_else(|| out.keys().find(|k| !k.ends_with("_count")).cloned())?;
        out.insert(key, Value::from(99));
    }
    Some(Value::Object(out))
}

/// One request: the game's champion patches off (its values kept) or put back, and the round's news once.
pub fn balance(host: &mut impl BalanceHost, req: &BalanceRequest) -> (bool, String) {
    let mut said = Vec::new();
    let mut ok = true;
    if req.off {
        match host.autopatch().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            Some(doc) => match champion_patches_off(&doc) {
                Some(off) if off == doc => said.push("the game's champion patches already off".to_string()),
                Some(off) => {
                    if host.saved(AUTOPATCH_KEPT).is_none() {
                        host.set_saved(AUTOPATCH_KEPT, Some(&doc.to_string()));
                    }
                    if host.set_autopatch(&off.to_string()) {
                        diag::log(&format!("[balance] the game's auto-patcher was {doc}; its champion counts now zero: {off}"));
                        said.push("the game's champion patches turned off".to_string());
                    } else {
                        ok = false;
                        said.push(format!("the game refused the auto-patcher settings {off}"));
                    }
                }
                None => {
                    ok = false;
                    said.push(format!("the auto-patcher settings are not an object: {doc}"));
                }
            },
            None => {
                ok = false;
                said.push("the auto-patcher settings are not readable".to_string());
            }
        }
        let done = host.saved(NEWS_DONE).and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(0);
        if req.round > done && !req.title.is_empty() {
            if host.push_news(req.team, &req.title, &req.content) {
                host.set_saved(NEWS_DONE, Some(&req.round.to_string()));
                said.push(format!("round {} in the news", req.round));
            } else {
                ok = false;
                said.push(format!("the news of round {} was refused", req.round));
            }
        }
    } else if let Some(kept) = host.saved(AUTOPATCH_KEPT) {
        if host.set_autopatch(&kept) {
            host.set_saved(AUTOPATCH_KEPT, None);
            diag::log(&format!("[balance] the game's auto-patcher put back: {kept}"));
            said.push("the game's champion patches put back".to_string());
        } else {
            ok = false;
            said.push("the game refused its own auto-patcher settings back".to_string());
        }
    } else {
        said.push("nothing to put back".to_string());
    }
    (ok, said.join("; "))
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

    #[derive(Default)]
    struct Host {
        autopatch: Option<String>,
        saved: BTreeMap<String, String>,
        news: Vec<(usize, String, String)>,
        refuse: bool,
    }

    impl BalanceHost for Host {
        fn autopatch(&self) -> Option<String> {
            self.autopatch.clone()
        }
        fn set_autopatch(&mut self, json: &str) -> bool {
            if self.refuse {
                return false;
            }
            self.autopatch = Some(json.to_string());
            true
        }
        fn saved(&self, key: &str) -> Option<String> {
            self.saved.get(key).cloned()
        }
        fn set_saved(&mut self, key: &str, value: Option<&str>) -> bool {
            match value {
                Some(v) => self.saved.insert(key.to_string(), v.to_string()),
                None => self.saved.remove(key),
            };
            true
        }
        fn push_news(&mut self, team: usize, title: &str, content: &str) -> bool {
            self.news.push((team, title.to_string(), content.to_string()));
            true
        }
    }

    fn breq(off: bool, round: u32) -> BalanceRequest {
        BalanceRequest { team: 3, off, round, title: format!("round {round}"), content: "a\nb".into() }
    }

    #[test]
    fn the_balance_request_round_trips() {
        let payload = "v1\n3\n1\n2\n智能BP 平衡补丁 2027.4.1（第 2 轮）\n【削弱】\n麦迪：…";
        let req = BalanceRequest::parse(payload.as_bytes(), None).unwrap();
        assert_eq!((req.team, req.off, req.round), (3, true, 2));
        assert_eq!(req.title, "智能BP 平衡补丁 2027.4.1（第 2 轮）");
        assert_eq!(req.content, "【削弱】\n麦迪：…");
        assert!(BalanceRequest::parse(b"v9\n3", None).is_err());
    }

    #[test]
    fn the_counts_go_to_zero_and_a_threshold_stays() {
        let doc: Value = serde_json::from_str(
            r#"{"major_strong_nerf_count":2,"minor_weak_buff_count":3,"meta_stale_high_threshold":4,"repeat_high_streak_threshold":0}"#,
        )
        .unwrap();
        let off = champion_patches_off(&doc).unwrap();
        assert_eq!(off["major_strong_nerf_count"], 0);
        assert_eq!(off["minor_weak_buff_count"], 0);
        assert_eq!(off["meta_stale_high_threshold"], 4, "kept: the document is not all zeros");
        // all zeros (the game's defaults): a threshold is set so it does not read as unset
        let zeros: Value = serde_json::from_str(r#"{"major_strong_nerf_count":0,"repeat_high_streak_threshold":0}"#).unwrap();
        let off = champion_patches_off(&zeros).unwrap();
        assert_eq!(off["repeat_high_streak_threshold"], 99);
        assert!(champion_patches_off(&Value::from(3)).is_none());
    }

    #[test]
    fn on_keeps_the_games_values_and_news_once_off_puts_them_back() {
        let original = r#"{"major_strong_nerf_count":2,"meta_stale_high_threshold":4}"#;
        let mut host = Host { autopatch: Some(original.into()), ..Host::default() };
        let (ok, said) = balance(&mut host, &breq(true, 1));
        assert!(ok, "{said}");
        assert!(host.autopatch.as_deref().unwrap().contains(r#""major_strong_nerf_count":0"#));
        let kept: Value = serde_json::from_str(host.saved.get(AUTOPATCH_KEPT).unwrap()).unwrap();
        assert_eq!(kept, serde_json::from_str::<Value>(original).unwrap(), "the game's own values kept");
        assert_eq!(host.news.len(), 1);
        // the same round again (a new session): no second news, the values kept from the first time
        let (ok, said) = balance(&mut host, &breq(true, 1));
        assert!(ok && said.contains("already off"), "{said}");
        assert_eq!(host.news.len(), 1);
        // the next round: its news
        balance(&mut host, &breq(true, 2));
        assert_eq!(host.news.len(), 2);
        assert_eq!(host.news[1].1, "round 2");
        // off: the game's own values back, nothing kept any more
        let (ok, said) = balance(&mut host, &breq(false, 2));
        assert!(ok, "{said}");
        let back: Value = serde_json::from_str(host.autopatch.as_deref().unwrap()).unwrap();
        assert_eq!(back["major_strong_nerf_count"], 2);
        assert!(!host.saved.contains_key(AUTOPATCH_KEPT));
        let (_, said) = balance(&mut host, &breq(false, 2));
        assert_eq!(said, "nothing to put back");
        // a refusal is said and retried
        let mut host = Host { autopatch: Some(original.into()), refuse: true, ..Host::default() };
        assert!(!balance(&mut host, &breq(true, 1)).0);
    }
}
