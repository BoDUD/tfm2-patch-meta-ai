//! Feature 2, server half: writes the tier list the client computed into the player team's
//! `champion_tiers` (records can only be changed on the management server; the change reaches
//! the client with the next in-game day).
//!
//! The balance patches' server half (2.8.0, `apply_balance`): with `balance` on, the save's own auto-patcher makes no
//! champion change (every count zero; its values kept in the save to put back when `balance` is turned off), and
//! each new round's notes go into the player's news once, as the game's own patch notes (into the game's notes of
//! the patch, or patch notes of their own for a hotfix), with the patch's news cards.
//!
//! Game 0.6 stores `champion_tiers` as `{"<champion>": "S" | "A" | "B" | "C" | "D" | "NoTier"}`.
//! A write the game's schema rejects changes nothing, so when the whole object is refused this
//! tries the same list without `NoTier`, then champion by champion, and says in `diag.log` what
//! the field looked like - enough to adapt the mod if a game update changes it.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use mod_api_stable::{
    CommandResultV1, SettingTargetV1, StableCommand, StableEventTarget, StableServerCtx, StableServerExtension,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::balance::{Article, Row};
use crate::diag;
use crate::records::{compare_versions, kind, top_keys};

pub const APPLY_TIERS: &str = "apply_tiers";
pub const RESULT_EVENT: &str = "apply_tiers_result";
pub const FIELD: &str = "champion_tiers";
/// The balance patches (2.8.0): the request, the answer.
pub const APPLY_BALANCE: &str = "apply_balance";
pub const BALANCE_RESULT: &str = "apply_balance_result";
/// The round made ahead for the game's next patch (2.8.1, [`Prepared`]; an empty payload withdraws it), and the
/// server's word that it wrote it (`<round>\tok|fail\t<what it did>`).
pub const PREPARE_BALANCE: &str = "prepare_balance";
pub const PREPARED_WRITTEN: &str = "prepared_balance_written";
/// The patch a prepared round carries until the game names it.
pub const NEXT_PATCH: &str = "{NEXT_PATCH}";
/// The save's book (2.8.2): the client asks for its id (payload `1`: make one, the setting is on) and gets
/// `<id>\t<newest round in the save's news>` (no id: the save has no book).
pub const BALANCE_BOOK: &str = "balance_book";
pub const BOOK_RESULT: &str = "balance_book_result";
/// The save's own auto-patcher values while the balance has them off, the newest round put in the news, the author
/// the mod's news items were last renamed to, the id of the save's book, and the news fix-ups done.
const AUTOPATCH_KEPT: &str = "pma_balance_autopatch";
const NEWS_DONE: &str = "pma_balance_news";
const AUTHOR_DONE: &str = "pma_balance_author";
const BOOK_ID: &str = "pma_balance_book";
const FIXUPS_DONE: &str = "pma_balance_fixups";
/// The news fix-ups this version knows (2: the mod's own patch notes without a champion say so).
const FIXUPS: &str = "2";
/// The text of patch notes without a champion change (`text/news.i18n`).
pub const UNCHANGED_TEXT: &str = "#asset/base/text/news?patch.pma_none";

pub struct ServerExt;

/// Teams whose `champion_tiers` layout was already described in `diag.log`.
static PROBED: Mutex<Option<HashSet<usize>>> = Mutex::new(None);

pub fn reset_for_tests() {
    *PROBED.lock().unwrap_or_else(PoisonError::into_inner) = None;
    *WATCH.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

impl StableServerExtension for ServerExt {
    fn on_server_start(&self, _ctx: &mut StableServerCtx<'_>) {
        // a new server (another save): nothing prepared for it yet
        *WATCH.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn after_management_tick(&self, ctx: &mut StableServerCtx<'_>) {
        if let Some((target, message)) = watch(ctx) {
            ctx.emit_event(target, PREPARED_WRITTEN, message.as_bytes());
        }
    }

    fn handle_command(&self, ctx: &mut StableServerCtx<'_>, cmd: &StableCommand<'_>) -> CommandResultV1 {
        if cmd.command == BALANCE_BOOK {
            let (id, done) = book(ctx, cmd.payload == b"1");
            let message = format!("{}\t{done}", id.unwrap_or_default());
            ctx.emit_event(cmd.reply_target(), BOOK_RESULT, message.as_bytes());
            return CommandResultV1::Handled;
        }
        if cmd.command == PREPARE_BALANCE {
            match (cmd.payload.is_empty(), Prepared::parse(cmd.payload)) {
                (true, _) => arm(None, cmd.reply_target()),
                (false, Ok(prepared)) => arm(Some(prepared), cmd.reply_target()),
                (false, Err(why)) => diag::log(&format!("[balance] a prepared round the server cannot read: {why}")),
            }
            return CommandResultV1::Handled;
        }
        if cmd.command == APPLY_BALANCE {
            let reply = cmd.reply_target();
            let (state, ok, detail) = match BalanceRequest::parse(cmd.payload) {
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

/// What the client sends (JSON): the team, whether the game's champion patches are to be off (else put back), the
/// newest round, and that round's notes and news cards. The client sends it again until the server answers; the
/// notes and cards are written once (the newest round written is kept in the save).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BalanceRequest {
    pub team: usize,
    pub off: bool,
    pub round: u32,
    #[serde(default)]
    pub note: Option<NoteRequest>,
    #[serde(default)]
    pub articles: Vec<Article>,
}

/// A round's notes: for the game's patch `version`, named `label`; `merge`d into the game's own notes of that patch
/// (else patch notes of their own); each champion's lines and reason codes; and the plain news (`title`, `content`)
/// the inbox keeps should the game take the notes in neither form.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NoteRequest {
    pub version: String,
    pub label: String,
    pub merge: bool,
    pub champions: Vec<(String, Vec<Row>, String)>,
    pub title: String,
    pub content: String,
}

impl BalanceRequest {
    pub fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn parse(payload: &[u8]) -> Result<BalanceRequest, String> {
        serde_json::from_slice(payload).map_err(|e| format!("not a balance request ({e})"))
    }
}

/// The part of the server context the balance needs (so tests can run it without a game).
pub trait BalanceHost {
    fn autopatch(&self) -> Option<String>;
    fn set_autopatch(&mut self, json: &str) -> bool;
    fn saved(&self, key: &str) -> Option<String>;
    fn set_saved(&mut self, key: &str, value: Option<&str>) -> bool;
    /// A plain news item at the end of the team's news, dated now.
    fn push_news(&mut self, team: usize, title: &str, content: &str, author: &str) -> bool;
    /// The team's news list (JSON).
    fn news(&self, team: usize) -> Option<String>;
    /// One item of the team's news (None past its end).
    fn news_item(&self, team: usize, index: usize) -> Option<String>;
    /// Replaces one item of the team's news (the game reads the whole record again and refuses what it cannot
    /// read, changing nothing).
    fn set_news_item(&mut self, team: usize, index: usize, json: &str) -> bool;
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
    fn push_news(&mut self, team: usize, title: &str, content: &str, author: &str) -> bool {
        self.news_push(team, title, content, author)
    }
    fn news(&self, team: usize) -> Option<String> {
        self.team_get_json(team, "news")
    }
    fn news_item(&self, team: usize, index: usize) -> Option<String> {
        self.team_get_json(team, &format!("news.{index}"))
    }
    fn set_news_item(&mut self, team: usize, index: usize, json: &str) -> bool {
        self.team_set_json(team, &format!("news.{index}"), json)
    }
}

// ===================== the round made ahead (2.8.1) =====================
//
// The server's changes to records reach the client's screens with a later in-game day: notes changed after the
// management tick the game wrote them in show the inbox the game's own notes first - with its champion changes off,
// empty (the user's 2027.4.2: 「这里看起来有bug？」). So while the game's next patch is near, the client makes the
// round ahead and hands it over, and the server writes it in the tick the game's notes appear in.

/// A round made ahead: for the game's first patch newer than `after`, its version and label [`NEXT_PATCH`] until then.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Prepared {
    /// The save's book it was made on (`balance::book`): written on that save only.
    pub book: String,
    pub after: String,
    pub round: crate::balance::Round,
    pub request: BalanceRequest,
}

impl Prepared {
    pub fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn parse(payload: &[u8]) -> Result<Prepared, String> {
        serde_json::from_slice(payload).map_err(|e| format!("not a prepared round ({e})"))
    }

    /// The round and its request for the game's patch `version`.
    pub fn named(&self, version: &str) -> Result<(crate::balance::Round, BalanceRequest), String> {
        let named = Prepared::parse(self.encode().replace(NEXT_PATCH, version).as_bytes())?;
        Ok((named.round, named.request))
    }
}

/// The prepared round the server watches the news for, and what watching costs.
struct Watch {
    prepared: Prepared,
    reply: StableEventTarget,
    /// The news list's length when last looked at (None: to be read whole).
    len: Option<usize>,
    ticks: u32,
    spent: Duration,
    worst: Duration,
    since: Instant,
    logged_tick: bool,
}

static WATCH: Mutex<Option<Watch>> = Mutex::new(None);
/// How often the cost of watching is written to `diag.log` while a round is prepared.
const WATCH_LOG_EVERY: Duration = Duration::from_secs(60);
/// New news items read one by one in a tick at most (a tick adds a few).
const NEW_ITEMS_MAX: usize = 64;

/// A round handed over (None: withdrawn - no game patch near).
pub fn arm(prepared: Option<Prepared>, reply: StableEventTarget) {
    let mut guard = WATCH.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(prepared) = prepared else {
        if guard.take().is_some() {
            diag::log("[balance] the prepared round withdrawn (no game patch near)");
        }
        return;
    };
    let old = guard.take();
    let same_team = old.as_ref().is_some_and(|w| w.prepared.request.team == prepared.request.team);
    if old.as_ref().is_none_or(|w| w.prepared.round.id != prepared.round.id || w.prepared.after != prepared.after) {
        let list: Vec<String> = prepared.round.changes.iter().map(|c| format!("{} {:+}", c.champion, c.step)).collect();
        diag::log(&format!(
            "[balance] round {} prepared for the game's first patch after {}: {}; written in the tick its notes appear",
            prepared.round.id,
            prepared.after,
            if list.is_empty() { "no changes".to_string() } else { list.join(", ") }
        ));
    }
    let mut watch = Watch {
        prepared,
        reply,
        len: None,
        ticks: 0,
        spent: Duration::ZERO,
        worst: Duration::ZERO,
        since: Instant::now(),
        logged_tick: false,
    };
    if let Some(old) = old.filter(|_| same_team) {
        (watch.len, watch.ticks, watch.spent, watch.worst, watch.since, watch.logged_tick) =
            (old.len, old.ticks, old.spent, old.worst, old.since, old.logged_tick);
    }
    *guard = Some(watch);
}

/// Every management tick: with a round prepared, the news' end looked at, and the game's patch notes newer than the
/// round's `after` given it now - the book, the notes, the cards. Some(the client's event) when that happened.
pub fn watch(host: &mut impl BalanceHost) -> Option<(StableEventTarget, String)> {
    let mut guard = WATCH.lock().unwrap_or_else(PoisonError::into_inner);
    let w = guard.as_mut()?;
    if !w.logged_tick {
        w.logged_tick = true;
        diag::log("[balance] the server watches the news for the game's patch (management ticks reach the mod)");
    }
    let started = Instant::now();
    let found = find_patch(host, w.prepared.request.team, &w.prepared.after, &mut w.len);
    let spent = started.elapsed();
    w.ticks += 1;
    w.spent += spent;
    w.worst = w.worst.max(spent);
    if w.since.elapsed() >= WATCH_LOG_EVERY {
        diag::log(&format!(
            "[balance] watching for the game's patch: {} management ticks in {:.0} s, the news looked at in {:.2} ms on average ({:.2} ms at worst)",
            w.ticks,
            w.since.elapsed().as_secs_f32(),
            w.spent.as_secs_f64() * 1000.0 / f64::from(w.ticks.max(1)),
            w.worst.as_secs_f64() * 1000.0
        ));
        (w.ticks, w.spent, w.worst, w.since) = (0, Duration::ZERO, Duration::ZERO, Instant::now());
    }
    let version = found?;
    let watch = guard.take()?;
    drop(guard);
    let (ok, detail) = write_prepared(host, &watch.prepared, &version);
    diag::log(&format!("[balance] {detail}"));
    Some((watch.reply, format!("{}\t{}\t{detail}", watch.prepared.round.id, if ok { "ok" } else { "fail" })))
}

/// The version of the game's patch notes newer than `after` among the news items from `len` on (the whole list read
/// the first time: notes there already count too - the patch came before the round did).
fn find_patch(host: &impl BalanceHost, team: usize, after: &str, len: &mut Option<usize>) -> Option<String> {
    let newer = |item: &Value| {
        patch_version(item).filter(|v| compare_versions(v, after) == std::cmp::Ordering::Greater).map(str::to_string)
    };
    let Some(mut at) = *len else {
        let items = news_items(host, team).ok()?;
        *len = Some(items.len());
        return items.iter().rev().find_map(newer);
    };
    let mut found = None;
    for _ in 0..NEW_ITEMS_MAX {
        let Some(json) = host.news_item(team, at) else { break };
        at += 1;
        if found.is_none() {
            found = serde_json::from_str::<Value>(&json).ok().as_ref().and_then(newer);
        }
    }
    *len = Some(at);
    found
}

/// The prepared round named for the game's patch `version`: in the book, then its notes and cards in the news.
fn write_prepared(host: &mut impl BalanceHost, prepared: &Prepared, version: &str) -> (bool, String) {
    let (round, req) = match prepared.named(version) {
        Ok(named) => named,
        Err(why) => return (false, format!("the prepared round for {version} unusable: {why}")),
    };
    let id = round.id;
    if let Err(why) = crate::balance::publish_prepared(round, &prepared.book) {
        return (false, format!("the prepared round {id} not written into {version}: {why}"));
    }
    let (ok, said) = balance(host, &req);
    (ok, format!("round {id} written in the tick the game's patch notes {version} appeared: {said}"))
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

/// One request: the game's champion patches off (its values kept) or put back, and the round's notes and news cards
/// once.
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
        if req.round > done {
            let mut written = true;
            if let Some(note) = &req.note {
                match write_note(host, req.team, note) {
                    Ok(how) => said.push(format!("round {} {how}", req.round)),
                    Err(why) => {
                        ok = false;
                        written = false;
                        said.push(format!("the notes of round {} not written: {why}", req.round));
                    }
                }
            }
            if written {
                if !req.articles.is_empty() {
                    said.push(write_articles(host, req.team, &req.articles));
                }
                host.set_saved(NEWS_DONE, Some(&req.round.to_string()));
            }
        }
        // the mod's news items put right, once a save and version
        if host.saved(AUTHOR_DONE).as_deref() != Some(crate::balance::AUTHOR) || host.saved(FIXUPS_DONE).as_deref() != Some(FIXUPS) {
            if let Some((renamed, said_so)) = fix_news(host, req.team) {
                if renamed > 0 {
                    said.push(format!("{renamed} news items now by {}", crate::balance::AUTHOR));
                }
                if said_so > 0 {
                    said.push(format!("{said_so} of the mod's patch notes without a champion now say so"));
                }
                host.set_saved(AUTHOR_DONE, Some(crate::balance::AUTHOR));
                host.set_saved(FIXUPS_DONE, Some(FIXUPS));
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

/// The mod's news items put right: those by one of its old names (`balance::OLD_AUTHORS`) get its name now, and its
/// own patch notes without a champion (2.8.1 wrote a first round that changed nobody as an empty hotfix: 「看不到任何
/// 内容」) say that nothing changed. How many of each (None: the news not readable).
fn fix_news(host: &mut impl BalanceHost, team: usize) -> Option<(usize, usize)> {
    let items = news_items(host, team).ok()?;
    let (mut renamed, mut said_so) = (0, 0);
    for (i, item) in items.into_iter().enumerate() {
        let author = item.get("author").and_then(Value::as_str).unwrap_or_default();
        if !crate::balance::is_ours(author) {
            continue;
        }
        let old_name = author != crate::balance::AUTHOR;
        let empty = says_nothing(&item);
        if !old_name && !empty {
            continue;
        }
        let mut fixed = if empty { unchanged(&item) } else { item };
        if let Value::Object(map) = &mut fixed {
            map.insert("author".to_string(), json!(crate::balance::AUTHOR));
        }
        if host.set_news_item(team, i, &fixed.to_string()) {
            renamed += usize::from(old_name);
            said_so += usize::from(empty);
        }
    }
    Some((renamed, said_so))
}

/// Whether a news item is patch notes that list nothing: no champion, no new champion, no rule.
fn says_nothing(item: &Value) -> bool {
    let Some(body) = patch_body(item) else { return false };
    ["champion_patch_data", "added_champion", "game_rule_patch_data"].iter().all(|key| match body.get(*key) {
        None | Some(Value::Null) => true,
        Some(Value::Array(list)) => list.is_empty(),
        Some(Value::Object(map)) => map.is_empty(),
        Some(_) => false,
    })
}

/// Patch notes that list nothing as plain news that says so: the same title, the text [`UNCHANGED_TEXT`], the
/// game's patch kept in the binds (`patchnotes` reads it there).
fn unchanged(item: &Value) -> Value {
    let mut out = item.clone();
    let version = patch_version(item).unwrap_or_default().to_string();
    if let Value::Object(map) = &mut out {
        map.insert(
            "ty".to_string(),
            json!({"Simple": {"content": UNCHANGED_TEXT, "content_bind": [["Patch", version], ["Scope", "patch"]]}}),
        );
    }
    out
}

/// The save's book id and the newest round its news has. The id is made the first time the setting is on (`create`),
/// or when the save already has rounds written (a save from before the ids, 2.8.0-2.8.1); None: the save has no book.
pub fn book(host: &mut impl BalanceHost, create: bool) -> (Option<String>, u32) {
    let done = host.saved(NEWS_DONE).and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(0);
    let valid = |id: &String| (8..=32).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric());
    let id = match host.saved(BOOK_ID).filter(valid) {
        Some(id) => Some(id),
        None if create || done > 0 => {
            let id = new_book_id();
            host.set_saved(BOOK_ID, Some(&id)).then_some(id)
        }
        None => None,
    };
    (id, done)
}

/// A book id no other save has: the time and the process's random hasher.
fn new_book_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    format!("{:016x}", h.finish())
}

/// The team's news items.
fn news_items(host: &impl BalanceHost, team: usize) -> Result<Vec<Value>, String> {
    let json = host.news(team).ok_or("the team's news is not readable")?;
    match serde_json::from_str::<Value>(&json) {
        Ok(Value::Array(items)) => Ok(items),
        Ok(other) => Err(format!("the team's news is a {}", kind(&other))),
        Err(e) => Err(format!("the team's news is not JSON ({e})")),
    }
}

fn patch_body(item: &Value) -> Option<&Value> {
    item.get("ty")?.get("PatchNote")
}

fn patch_version(item: &Value) -> Option<&str> {
    patch_body(item)?.get("version")?.as_str()
}

fn rows_json(rows: &[Row]) -> Vec<Value> {
    rows.iter().filter_map(|r| serde_json::to_value(r).ok()).collect()
}

/// Adds a champion's lines to `champion_patch_data` (a list of `[champion, lines]`, or a map): after the lines the
/// champion already has.
fn add_lines(field: &mut Value, champion: &str, rows: &[Row]) {
    let lines = rows_json(rows);
    match field {
        Value::Array(entries) => {
            let at = entries
                .iter()
                .position(|e| e.get(0).and_then(Value::as_str) == Some(champion) && e.get(1).is_some_and(Value::is_array));
            match at {
                Some(at) => {
                    if let Some(Value::Array(old)) = entries[at].get_mut(1) {
                        old.extend(lines);
                    }
                }
                None => entries.push(json!([champion, lines])),
            }
        }
        Value::Object(map) => {
            if map.get(champion).is_some_and(Value::is_array) {
                if let Some(Value::Array(old)) = map.get_mut(champion) {
                    old.extend(lines);
                }
            } else {
                map.insert(champion.to_string(), Value::Array(lines));
            }
        }
        other => *other = json!([[champion, lines]]),
    }
}

/// Adds a champion's reason codes to `champion_patch_reasons` (a list of `[champion, codes]`, or a map): after the
/// codes it already has, comma-separated as the game writes them.
fn add_reason(field: &mut Value, champion: &str, codes: &str) {
    let join = |old: &mut String| {
        if !old.is_empty() {
            old.push(',');
        }
        old.push_str(codes);
    };
    match field {
        Value::Array(entries) => {
            let at = entries
                .iter()
                .position(|e| e.get(0).and_then(Value::as_str) == Some(champion) && e.get(1).is_some_and(Value::is_string));
            match at {
                Some(at) => {
                    if let Some(Value::String(old)) = entries[at].get_mut(1) {
                        join(old);
                    }
                }
                None => entries.push(json!([champion, codes])),
            }
        }
        Value::Object(map) => {
            if map.get(champion).is_some_and(Value::is_string) {
                if let Some(Value::String(old)) = map.get_mut(champion) {
                    join(old);
                }
            } else {
                map.insert(champion.to_string(), Value::String(codes.to_string()));
            }
        }
        other => *other = json!([[champion, codes]]),
    }
}

/// Puts the round's champions into a patch-notes item (its own lines kept), unread again.
fn merge_note(item: &mut Value, note: &NoteRequest) -> bool {
    {
        let Some(Value::Object(body)) = item.get_mut("ty").and_then(|t| t.get_mut("PatchNote")) else { return false };
        let data = body.entry("champion_patch_data").or_insert_with(|| json!([]));
        for (champion, rows, _) in &note.champions {
            add_lines(data, champion, rows);
        }
        let reasons = body.entry("champion_patch_reasons").or_insert_with(|| json!([]));
        for (champion, _, codes) in &note.champions {
            if !codes.is_empty() {
                add_reason(reasons, champion, codes);
            }
        }
    }
    if let Value::Object(map) = item {
        map.insert("is_read".to_string(), Value::Bool(false));
    }
    true
}

/// The pushed plain news made patch notes: the game's title (`补丁公告 v<label>`), the balance's author, and the body
/// in each form the game might want - the newest patch notes' own (`template`) first, then the 0.7 form and others.
fn own_note_items(pushed: &Value, note: &NoteRequest, template: Option<&Value>) -> Vec<Value> {
    let Value::Object(head) = pushed else { return Vec::new() };
    let mut head = head.clone();
    head.insert("title".to_string(), json!("#asset/base/text/news?patch.title"));
    head.insert("title_bind".to_string(), json!([["Version", note.label]]));
    head.insert("author".to_string(), json!(crate::balance::AUTHOR));
    head.insert("is_read".to_string(), Value::Bool(false));
    let empty_like = |v: &Value| match v {
        Value::Array(_) => json!([]),
        Value::Object(_) => json!({}),
        Value::String(_) => json!(""),
        _ => Value::Null,
    };
    let mut bodies: Vec<Value> = Vec::new();
    if let Some(Value::Object(t)) = template {
        let mut body: serde_json::Map<String, Value> = t.iter().map(|(k, v)| (k.clone(), empty_like(v))).collect();
        body.insert("version".to_string(), json!(note.version));
        bodies.push(Value::Object(body));
    }
    for (data, reasons) in [(json!([]), json!([])), (json!({}), json!({}))] {
        for rule_reasons in [json!([]), json!({}), json!("")] {
            bodies.push(json!({
                "version": note.version,
                "added_champion": [],
                "champion_patch_data": data,
                "champion_patch_reasons": reasons,
                "game_rule_patch_data": [],
                "game_rule_patch_reasons": rule_reasons,
            }));
        }
    }
    let mut out: Vec<Value> = Vec::new();
    for body in bodies {
        let mut item = Value::Object(head.clone());
        if let Value::Object(map) = &mut item {
            map.insert("ty".to_string(), json!({ "PatchNote": body }));
        }
        merge_note(&mut item, note);
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

/// A round's notes into the team's news: into the game's own notes of the patch (`merge`), else as patch notes of
/// their own - first a plain news item (it stays, readable, should the game refuse the patch-notes form), then made
/// patch notes. A round that changed nobody writes none (2.8.2: a first round on a new save was an empty hotfix,
/// 「看不到任何内容」); the game's notes of its patch, when they list nothing at all (its champion changes are off),
/// are made plain news that says so. Ok says how; Err when nothing could be written.
pub fn write_note(host: &mut impl BalanceHost, team: usize, note: &NoteRequest) -> Result<String, String> {
    if note.champions.is_empty() {
        if !note.merge {
            return Ok(format!("no champion changed: no patch notes {}", note.label));
        }
        let items = news_items(host, team)?;
        let at = items.iter().rposition(|it| patch_version(it) == Some(note.version.as_str()));
        return Ok(match at.filter(|i| says_nothing(&items[*i])) {
            Some(i) if host.set_news_item(team, i, &unchanged(&items[i]).to_string()) => {
                format!("no champion changed: the game's empty patch notes {} say so", note.version)
            }
            Some(_) => format!("no champion changed (the game refused its empty patch notes {} as plain news)", note.version),
            None => format!("no champion changed: the game's patch notes {} left as they are", note.version),
        });
    }
    let items = news_items(host, team)?;
    if note.merge {
        if let Some(i) = items.iter().rposition(|it| patch_version(it) == Some(note.version.as_str())) {
            let mut item = items[i].clone();
            if merge_note(&mut item, note) && host.set_news_item(team, i, &item.to_string()) {
                return Ok(format!("written into the game's patch notes {}", note.version));
            }
            diag::log(&format!(
                "[balance] the game refused its patch notes {} with the round's champions in them: {}",
                note.version,
                diag::clip(&item.to_string(), 400)
            ));
        }
    }
    let template = items.iter().rev().find_map(|it| patch_body(it).cloned());
    if !host.push_news(team, &note.title, &note.content, crate::balance::AUTHOR) {
        return Err("the game refused the news".to_string());
    }
    let items = news_items(host, team)?;
    let Some(i) = items.iter().rposition(|it| it.get("title").and_then(Value::as_str) == Some(note.title.as_str())) else {
        return Ok(format!("as plain news {} (not found again to make it patch notes)", note.label));
    };
    let tries = own_note_items(&items[i], note, template.as_ref());
    for item in &tries {
        if host.set_news_item(team, i, &item.to_string()) {
            return Ok(format!("written as patch notes {}", note.label));
        }
    }
    diag::log(&format!(
        "[balance] the game refused {} forms of patch notes {}; they stay plain news. The pushed item: {}; the newest patch notes: {}",
        tries.len(),
        note.label,
        diag::clip(&items[i].to_string(), 300),
        template.map_or("none".to_string(), |t| diag::clip(&t.to_string(), 300))
    ));
    Ok(format!("as plain news {}; the game refused the patch-notes form", note.label))
}

fn binds(list: &[(String, String)]) -> Value {
    Value::Array(list.iter().map(|(k, v)| json!([k, v])).collect())
}

/// The round's news cards: each a plain news item made the game's card (its texts and binds).
fn write_articles(host: &mut impl BalanceHost, team: usize, articles: &[Article]) -> String {
    let (mut cards, mut plain, mut refused) = (0, 0, 0);
    for a in articles {
        if !host.push_news(team, &a.plain_title, &a.plain_content, &a.author) {
            refused += 1;
            continue;
        }
        let item = news_items(host, team).ok().and_then(|items| {
            let i = items.iter().rposition(|it| it.get("title").and_then(Value::as_str) == Some(a.plain_title.as_str()))?;
            Some((i, items[i].clone()))
        });
        let Some((i, Value::Object(mut map))) = item else {
            plain += 1;
            continue;
        };
        map.insert("title".to_string(), json!(a.title));
        map.insert("title_bind".to_string(), binds(&a.title_bind));
        map.insert("author".to_string(), json!(a.author));
        map.insert("ty".to_string(), json!({"Simple": {"content": a.content, "content_bind": binds(&a.content_bind)}}));
        if host.set_news_item(team, i, &Value::Object(map).to_string()) {
            cards += 1;
        } else {
            plain += 1;
        }
    }
    let mut out = format!("{cards} news cards");
    if plain > 0 {
        out.push_str(&format!(", {plain} as plain news"));
    }
    if refused > 0 {
        out.push_str(&format!(", {refused} refused"));
    }
    out
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

    /// A save's news as the server sees it, and the game's reading of a news item (`strict`: what 0.7 saves hold).
    #[derive(Default)]
    struct Host {
        autopatch: Option<String>,
        saved: BTreeMap<String, String>,
        news: Vec<Value>,
        refuse: bool,
        strict: bool,
        refuse_items: bool,
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
        fn push_news(&mut self, _team: usize, title: &str, content: &str, author: &str) -> bool {
            self.news.push(json!({
                "author": author, "date": "2027-09-20T13:00:00", "is_favorite": false, "is_read": false, "is_sent": true,
                "title": title, "title_bind": [], "ty": {"Simple": {"content": content, "content_bind": []}}
            }));
            true
        }
        fn news(&self, _team: usize) -> Option<String> {
            Some(Value::Array(self.news.clone()).to_string())
        }
        fn news_item(&self, _team: usize, index: usize) -> Option<String> {
            self.news.get(index).map(Value::to_string)
        }
        fn set_news_item(&mut self, _team: usize, index: usize, json: &str) -> bool {
            let Ok(item) = serde_json::from_str::<Value>(json) else { return false };
            if self.refuse_items || (self.strict && !game_reads(&item)) {
                return false;
            }
            match self.news.get_mut(index) {
                Some(slot) => {
                    *slot = item;
                    true
                }
                None => false,
            }
        }
    }

    /// The news item layout of 0.7 saves: `[key, value]` binds, a `PatchNote` with exactly its six fields as lists.
    fn game_reads(item: &Value) -> bool {
        let pairs = |v: &Value| {
            v.as_array().is_some_and(|a| a.iter().all(|p| p.as_array().is_some_and(|p| p.len() == 2 && p.iter().all(Value::is_string))))
        };
        let row = |r: &Value| {
            r.as_object().is_some_and(|o| {
                o.len() == 5
                    && o["key"].is_string()
                    && (o["target_action"].is_null() || o["target_action"].is_string())
                    && o["pre_value"].is_string()
                    && o["after_value"].is_string()
                    && o["is_buff"].is_boolean()
            })
        };
        if !pairs(&item["title_bind"]) || !item["author"].is_string() || !item["date"].is_string() || !item["title"].is_string() {
            return false;
        }
        match &item["ty"] {
            Value::String(_) => true,
            Value::Object(ty) if ty.len() == 1 => match ty.iter().next() {
                Some((k, body)) if k == "PatchNote" => {
                    body.as_object().is_some_and(|b| b.len() == 6)
                        && body["version"].is_string()
                        && body["added_champion"].is_array()
                        && body["champion_patch_data"]
                            .as_array()
                            .is_some_and(|e| e.iter().all(|p| p[0].is_string() && p[1].as_array().is_some_and(|rows| rows.iter().all(row))))
                        && pairs(&body["champion_patch_reasons"])
                        && body["game_rule_patch_data"].is_array()
                        && body["game_rule_patch_reasons"].is_array()
                }
                Some((k, body)) if k == "Simple" => body["content"].is_string() && pairs(&body["content_bind"]),
                _ => false,
            },
            _ => false,
        }
    }

    fn line(key: &str, pre: &str, after: &str) -> Row {
        Row { key: key.into(), target_action: None, pre_value: pre.into(), after_value: after.into(), is_buff: after > pre }
    }

    fn note(version: &str, label: &str, merge: bool) -> NoteRequest {
        NoteRequest {
            version: version.into(),
            label: label.into(),
            merge,
            champions: vec![
                ("tft_maddie".into(), vec![line("#asset/base/text/champion?stat.attack", "94", "89.3")], "pma_wr_74,pma_cap".into()),
                ("league_kaisa".into(), vec![line("#asset/base/text/champion?stat.hp", "900", "945")], "pma_wr_35".into()),
            ],
            title: format!("电竞大脑平衡补丁 {label}"),
            content: "【削弱】\n麦迪：…".into(),
        }
    }

    /// The game's own patch notes of a patch (one champion of its own; its rule changes).
    fn official(version: &str) -> Value {
        json!({
            "author": "Team Samoyed", "date": "2027-10-01T00:00:00", "is_favorite": false, "is_read": true, "is_sent": true,
            "title": "#asset/base/text/news?patch.title", "title_bind": [["Version", version]],
            "ty": {"PatchNote": {
                "version": version, "added_champion": [],
                "champion_patch_data": [["league_brand", [
                    {"key": "#asset/base/text/champion?stat.attack", "target_action": null, "pre_value": "94", "after_value": "75", "is_buff": false}]]],
                "champion_patch_reasons": [["league_brand", "early_game_dominant_2"]],
                "game_rule_patch_data": [{"category": "x", "pre_value": "1", "after_value": "2", "is_buff": true, "key": "kill_gold"}],
                "game_rule_patch_reasons": ["game_too_long_1"]
            }}
        })
    }

    fn breq(off: bool, round: u32) -> BalanceRequest {
        BalanceRequest { team: 3, off, round, note: Some(note("2027.4.1", "2027.4.1b", false)), articles: Vec::new() }
    }

    #[test]
    fn the_balance_request_round_trips() {
        let mut req = breq(true, 2);
        req.articles.push(Article { title: "t".into(), title_bind: vec![("Champion".into(), "x".into())], ..Article::default() });
        assert_eq!(BalanceRequest::parse(req.encode().as_bytes()).unwrap(), req);
        assert!(BalanceRequest::parse(b"v1\n3\n1").is_err());
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
    fn on_keeps_the_games_values_and_writes_each_round_once_off_puts_them_back() {
        let original = r#"{"major_strong_nerf_count":2,"meta_stale_high_threshold":4}"#;
        let mut host = Host { autopatch: Some(original.into()), news: vec![official("2027.4.1")], ..Host::default() };
        let (ok, said) = balance(&mut host, &breq(true, 1));
        assert!(ok, "{said}");
        assert!(host.autopatch.as_deref().unwrap().contains(r#""major_strong_nerf_count":0"#));
        let kept: Value = serde_json::from_str(host.saved.get(AUTOPATCH_KEPT).unwrap()).unwrap();
        assert_eq!(kept, serde_json::from_str::<Value>(original).unwrap(), "the game's own values kept");
        assert_eq!(host.news.len(), 2, "{said}");
        // the same round again (a new session): nothing written twice, the values kept from the first time
        let (ok, said) = balance(&mut host, &breq(true, 1));
        assert!(ok && said.contains("already off"), "{said}");
        assert_eq!(host.news.len(), 2);
        // the next round: its notes
        balance(&mut host, &breq(true, 2));
        assert_eq!(host.news.len(), 3);
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

    #[test]
    fn a_patch_round_goes_into_the_games_own_notes() {
        let mut host = Host { strict: true, news: vec![official("2027.4.1"), official("2027.5.0")], ..Host::default() };
        let how = write_note(&mut host, 3, &note("2027.5.0", "2027.5.0", true)).unwrap();
        assert_eq!(how, "written into the game's patch notes 2027.5.0");
        assert_eq!(host.news.len(), 2, "no item of its own");
        let item = &host.news[1];
        assert_eq!(item["is_read"], false, "unread again");
        let body = &item["ty"]["PatchNote"];
        let champions: Vec<&str> = body["champion_patch_data"].as_array().unwrap().iter().map(|e| e[0].as_str().unwrap()).collect();
        assert_eq!(champions, ["league_brand", "tft_maddie", "league_kaisa"], "the game's own champion kept");
        assert_eq!(body["champion_patch_reasons"][1], json!(["tft_maddie", "pma_wr_74,pma_cap"]));
        assert_eq!(body["game_rule_patch_reasons"], json!(["game_too_long_1"]), "its rule changes kept");
        assert_eq!(host.news[0], official("2027.4.1"), "an older patch untouched");
        // a champion already in the game's notes gets its lines added to them
        let mut host = Host { news: vec![official("2027.5.0")], ..Host::default() };
        let mut n = note("2027.5.0", "2027.5.0", true);
        n.champions[0].0 = "league_brand".into();
        write_note(&mut host, 3, &n).unwrap();
        let body = &host.news[0]["ty"]["PatchNote"];
        assert_eq!(body["champion_patch_data"][0][1].as_array().unwrap().len(), 2);
        assert_eq!(body["champion_patch_reasons"][0][1], "early_game_dominant_2,pma_wr_74,pma_cap");
        // no notes of the game for the patch: notes of its own
        let mut host = Host { strict: true, news: vec![official("2027.4.1")], ..Host::default() };
        assert_eq!(write_note(&mut host, 3, &note("2027.5.0", "2027.5.0", true)).unwrap(), "written as patch notes 2027.5.0");
        assert_eq!(host.news.len(), 2);
    }

    #[test]
    fn a_hotfix_is_patch_notes_of_its_own_in_the_games_form() {
        // the newest notes of the game give the form (here: its own rule reasons a list)
        let mut host = Host { strict: true, news: vec![official("2027.4.1")], ..Host::default() };
        let how = write_note(&mut host, 3, &note("2027.4.1", "2027.4.1b", false)).unwrap();
        assert_eq!(how, "written as patch notes 2027.4.1b");
        let item = &host.news[1];
        assert!(game_reads(item), "{item}");
        assert_eq!(item["title"], "#asset/base/text/news?patch.title");
        assert_eq!(item["title_bind"], json!([["Version", "2027.4.1b"]]));
        assert_eq!(item["author"], crate::balance::AUTHOR);
        assert_eq!(item["date"], "2027-09-20T13:00:00", "dated as the game dated the news");
        let body = &item["ty"]["PatchNote"];
        assert_eq!(body["version"], "2027.4.1", "the game's own patch");
        assert_eq!(body["game_rule_patch_data"], json!([]));
        assert_eq!(body["champion_patch_data"][0][1][0]["after_value"], "89.3");
        // no notes of the game in the news yet: the 0.7 form is found among the others
        let mut host = Host { strict: true, ..Host::default() };
        assert_eq!(write_note(&mut host, 3, &note("2027.4.1", "2027.4.1b", false)).unwrap(), "written as patch notes 2027.4.1b");
        assert!(game_reads(&host.news[0]));
        // the patch-notes form refused: the plain news stays, readable
        let mut host = Host { refuse_items: true, ..Host::default() };
        let how = write_note(&mut host, 3, &note("2027.4.1", "2027.4.1b", false)).unwrap();
        assert!(how.starts_with("as plain news 2027.4.1b"), "{how}");
        assert_eq!(host.news[0]["title"], "电竞大脑平衡补丁 2027.4.1b");
        assert_eq!(host.news[0]["ty"]["Simple"]["content"], "【削弱】\n麦迪：…");
    }

    #[test]
    fn a_prepared_round_is_written_in_the_tick_the_games_notes_appear() {
        use crate::balance::{Change, Round};
        let _serial = crate::tests::serial();
        crate::balance::reset_for_tests();
        reset_for_tests();
        let file = std::env::temp_dir().join(format!("pma_prepared_test_{}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        crate::balance::open_for_tests(file.clone());
        crate::balance::publish(Round { id: 1, version: "2027.4.1".into(), label: "2027.4.1b".into(), ..Round::default() });
        let mut host = Host {
            strict: true,
            autopatch: Some(r#"{"major_strong_nerf_count":0,"meta_stale_high_threshold":4}"#.into()),
            news: vec![official("2027.4.1")],
            ..Host::default()
        };
        // made ahead for the game's first patch after 2027.4.1
        let prepared = |id: u32, after: &str| {
            let mut request = breq(true, id);
            request.note = Some(note(NEXT_PATCH, NEXT_PATCH, true));
            let round = Round {
                id,
                version: NEXT_PATCH.into(),
                label: NEXT_PATCH.into(),
                merged: true,
                changes: vec![Change { champion: "tft_maddie".into(), win: 74.0, evidence: 99.0, step: -10, total: -20 }],
                power: [("tft_maddie".to_string(), -20)].into_iter().collect(),
                ..Round::default()
            };
            Prepared { book: crate::balance::book().unwrap(), after: after.into(), round, request }
        };
        let p = prepared(2, "2027.4.1");
        assert_eq!(Prepared::parse(p.encode().as_bytes()).unwrap(), p, "through the command");
        arm(Some(p), StableEventTarget::Team(3));
        // ticks with nothing of the patch: nothing written (the first reads the whole list)
        assert!(watch(&mut host).is_none());
        host.news.push(json!({"author": "x", "date": "2027-10-02T00:00:00", "is_favorite": false, "is_read": false,
                              "is_sent": true, "title": "t", "title_bind": [], "ty": "StartTeam"}));
        assert!(watch(&mut host).is_none());
        assert_eq!(crate::balance::latest().map(|r| r.id), Some(1));
        // the game's patch notes appear: the round goes into the book and into them in the same tick
        host.news.push(official("2027.4.2"));
        let (target, message) = watch(&mut host).expect("written");
        assert_eq!(target, StableEventTarget::Team(3));
        assert!(message.starts_with("2\tok\tround 2 written in the tick the game's patch notes 2027.4.2 appeared"), "{message}");
        let r = crate::balance::latest().unwrap();
        assert_eq!((r.id, r.version.as_str(), r.label.as_str(), r.merged), (2, "2027.4.2", "2027.4.2", true));
        let item = &host.news[2];
        assert!(game_reads(item), "{item}");
        let champions: Vec<&str> = item["ty"]["PatchNote"]["champion_patch_data"].as_array().unwrap().iter().map(|e| e[0].as_str().unwrap()).collect();
        assert_eq!(champions, ["league_brand", "tft_maddie", "league_kaisa"]);
        assert_eq!(host.saved.get(NEWS_DONE).map(String::as_str), Some("2"));
        assert!(watch(&mut host).is_none(), "nothing prepared any more");
        // made before the book moved on: not written
        arm(Some(prepared(2, "2027.4.2")), StableEventTarget::Team(3));
        host.news.push(official("2027.4.3"));
        let (_, message) = watch(&mut host).expect("said");
        assert!(message.starts_with("2\tfail\t") && message.contains("the book's next is 3"), "{message}");
        assert_eq!(crate::balance::latest().map(|r| r.id), Some(2));
        // made on another save: not written into this one
        let mut other = prepared(3, "2027.4.3");
        other.book = "another save's book".into();
        arm(Some(other), StableEventTarget::Team(3));
        host.news.push(official("2027.4.4"));
        let (_, message) = watch(&mut host).expect("said");
        assert!(message.starts_with("3\tfail\t") && message.contains("made on another save"), "{message}");
        assert_eq!(host.saved.get(NEWS_DONE).map(String::as_str), Some("2"), "nothing written");
        // withdrawn: nothing watched
        arm(Some(prepared(3, "2027.4.3")), StableEventTarget::Team(3));
        arm(None, StableEventTarget::Team(3));
        host.news.push(official("2027.4.5"));
        assert!(watch(&mut host).is_none());
        crate::balance::reset_for_tests();
        reset_for_tests();
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn the_mods_news_under_its_old_name_get_its_name_once() {
        let old = json!({"author": crate::balance::OLD_AUTHORS[0], "date": "2027-10-01T00:00:00", "is_favorite": false,
                         "is_read": true, "is_sent": true, "title": "t", "title_bind": [], "ty": "StartTeam"});
        let mut host = Host {
            autopatch: Some(r#"{"major_strong_nerf_count":0,"meta_stale_high_threshold":4}"#.into()),
            news: vec![official("2027.4.1"), old.clone(), old],
            ..Host::default()
        };
        let (ok, said) = balance(&mut host, &BalanceRequest { team: 3, off: true, ..BalanceRequest::default() });
        assert!(ok && said.contains("2 news items now by 电竞大脑 / Patch Meta AI"), "{said}");
        assert_eq!((host.news[1]["author"].as_str(), host.news[2]["author"].as_str()), (Some(crate::balance::AUTHOR), Some(crate::balance::AUTHOR)));
        assert_eq!(host.news[0]["author"], "Team Samoyed", "the game's own left alone");
        host.news[1]["author"] = json!(crate::balance::OLD_AUTHORS[0]);
        let (_, said) = balance(&mut host, &BalanceRequest { team: 3, off: true, ..BalanceRequest::default() });
        assert!(!said.contains("news items now by"), "once a save: {said}");
    }

    #[test]
    fn a_save_gets_a_book_id_once() {
        // the setting off on a save that never had a round: no book
        let mut host = Host::default();
        assert_eq!(book(&mut host, false), (None, 0));
        // on: an id, kept in the save from then on
        let (id, done) = book(&mut host, true);
        let id = id.expect("an id");
        assert!(id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit()), "{id}");
        assert_eq!(done, 0);
        assert_eq!(book(&mut host, false), (Some(id.clone()), 0));
        // a save from before the ids with rounds in its news: an id of its own, the setting on or off
        let mut old = Host::default();
        old.saved.insert(NEWS_DONE.into(), "2".into());
        let (other, done) = book(&mut old, false);
        assert!(other.is_some() && other != Some(id) && done == 2, "{other:?} {done}");
    }

    /// The game's notes of a patch that list nothing (its champion changes off, no rule change).
    fn bare(version: &str) -> Value {
        let mut item = official(version);
        for key in ["champion_patch_data", "champion_patch_reasons", "game_rule_patch_data", "game_rule_patch_reasons"] {
            item["ty"]["PatchNote"][key] = json!([]);
        }
        item
    }

    #[test]
    fn a_round_that_changed_nobody_writes_no_notes() {
        let nobody = |version: &str, label: &str, merge: bool| NoteRequest { champions: Vec::new(), ..note(version, label, merge) };
        // a hotfix: nothing
        let mut host = Host { strict: true, news: vec![official("2027.4.1")], ..Host::default() };
        let how = write_note(&mut host, 3, &nobody("2027.4.1", "2027.4.1b", false)).unwrap();
        assert_eq!(how, "no champion changed: no patch notes 2027.4.1b");
        assert_eq!(host.news, vec![official("2027.4.1")]);
        // the game's notes of its patch with something in them (a rule): left as they are
        let mut host = Host { strict: true, news: vec![official("2027.5.0")], ..Host::default() };
        write_note(&mut host, 3, &nobody("2027.5.0", "2027.5.0", true)).unwrap();
        assert_eq!(host.news, vec![official("2027.5.0")]);
        // the game's notes that list nothing at all: plain news that says so, its title and the patch kept
        let mut host = Host { strict: true, news: vec![official("2027.4.1"), bare("2027.5.0")], ..Host::default() };
        let how = write_note(&mut host, 3, &nobody("2027.5.0", "2027.5.0", true)).unwrap();
        assert_eq!(how, "no champion changed: the game's empty patch notes 2027.5.0 say so");
        let item = &host.news[1];
        assert!(game_reads(item), "{item}");
        assert_eq!((item["title"].as_str(), &item["title_bind"]), (Some("#asset/base/text/news?patch.title"), &json!([["Version", "2027.5.0"]])));
        assert_eq!(item["author"], "Team Samoyed");
        assert_eq!(item["ty"], json!({"Simple": {"content": UNCHANGED_TEXT, "content_bind": [["Patch", "2027.5.0"], ["Scope", "patch"]]}}));
        // the client still takes it for the patch's notes (the game's newest patch)
        let read = crate::patchnotes::parse_all(&Value::Array(host.news.clone()).to_string()).unwrap();
        assert_eq!(read.newest_version().as_deref(), Some("2027.5.0"));
        assert!(read.notes[1].changes.is_empty() && read.notes[1].is_for("2027.5.0"));
        // a whole request of such a round: marked done, no card
        let mut host = Host {
            strict: true,
            autopatch: Some(r#"{"major_strong_nerf_count":0,"meta_stale_high_threshold":4}"#.into()),
            news: vec![official("2027.4.1")],
            ..Host::default()
        };
        let req = BalanceRequest { team: 3, off: true, round: 1, note: Some(nobody("2027.4.1", "2027.4.1b", false)), articles: Vec::new() };
        let (ok, said) = balance(&mut host, &req);
        assert!(ok && said.contains("round 1 no champion changed"), "{said}");
        assert_eq!((host.news.len(), host.saved.get(NEWS_DONE).map(String::as_str)), (1, Some("1")));
    }

    #[test]
    fn the_mods_own_empty_patch_notes_say_that_nothing_changed() {
        // 2.8.1 wrote a first round that changed nobody as an empty hotfix
        let mut own = bare("2026.0.0");
        own["author"] = json!(crate::balance::AUTHOR);
        own["title_bind"] = json!([["Version", "2026.0.0b"]]);
        let mut host = Host {
            strict: true,
            autopatch: Some(r#"{"major_strong_nerf_count":0,"meta_stale_high_threshold":4}"#.into()),
            news: vec![bare("2026.0.0"), own],
            ..Host::default()
        };
        host.saved.insert(AUTHOR_DONE.into(), crate::balance::AUTHOR.into());
        let request = BalanceRequest { team: 3, off: true, ..BalanceRequest::default() };
        let (ok, said) = balance(&mut host, &request);
        assert!(ok && said.contains("1 of the mod's patch notes without a champion now say so"), "{said}");
        let item = &host.news[1];
        assert!(game_reads(item), "{item}");
        assert_eq!(item["ty"]["Simple"]["content"], UNCHANGED_TEXT);
        assert_eq!(item["ty"]["Simple"]["content_bind"][0], json!(["Patch", "2026.0.0"]));
        assert_eq!((item["title_bind"].clone(), item["author"].as_str()), (json!([["Version", "2026.0.0b"]]), Some(crate::balance::AUTHOR)));
        assert_eq!(host.news[0], bare("2026.0.0"), "the game's own notes are not looked at here");
        // once a save
        host.news[1] = bare("2026.0.0");
        host.news[1]["author"] = json!(crate::balance::AUTHOR);
        let (_, said) = balance(&mut host, &request);
        assert!(!said.contains("now say so"), "{said}");
    }

    #[test]
    fn the_cards_are_written_as_the_games() {
        let card = Article {
            title: "#asset/base/text/news?article.meta.pc02.title.0".into(),
            title_bind: vec![("Champion".into(), "tft_maddie".into()), ("PatchChanges".into(), "[]".into())],
            content: "#asset/base/text/news?article.meta.pc02.content.0".into(),
            content_bind: vec![("Champion".into(), "tft_maddie".into()), ("ContentId".into(), "PC02".into())],
            author: crate::balance::AUTHOR.into(),
            plain_title: "麦迪在本次补丁中被削弱".into(),
            plain_content: "…".into(),
        };
        let mut host = Host { strict: true, autopatch: Some(r#"{"major_strong_nerf_count":2,"meta_stale_high_threshold":4}"#.into()), news: vec![official("2027.4.1")], ..Host::default() };
        let mut req = breq(true, 1);
        req.articles = vec![card.clone(), Article { plain_title: "second".into(), ..card.clone() }];
        let (ok, said) = balance(&mut host, &req);
        assert!(ok && said.contains("2 news cards"), "{said}");
        assert_eq!(host.news.len(), 4);
        let item = &host.news[2];
        assert!(game_reads(item), "{item}");
        assert_eq!(item["title"], card.title);
        assert_eq!(item["title_bind"], json!([["Champion", "tft_maddie"], ["PatchChanges", "[]"]]));
        assert_eq!(item["ty"]["Simple"]["content"], card.content);
        assert_eq!(item["ty"]["Simple"]["content_bind"][1], json!(["ContentId", "PC02"]));
        // refused as a card: the plain news stays
        let mut host = Host { refuse_items: true, ..Host::default() };
        assert_eq!(write_articles(&mut host, 3, &[card]), "0 news cards, 1 as plain news");
        assert_eq!(host.news[0]["title"], "麦迪在本次补丁中被削弱");
    }
}
