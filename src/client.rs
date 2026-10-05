//! Client extension: while a save is open, reads match records a little every frame, rebuilds
//! the model every couple of seconds, publishes the draft tables and asks the server to write
//! the tier list (only the server may change records).
//!
//! Everything runs in `post_update` under one lock that tolerates poisoning, and nothing in it
//! indexes a list it did not just measure, so one bad record or a save switch cannot stop it.

use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use mod_api_stable::{ClientSceneKindV1, RecordKindV1, StableClient, StableExtension};
use serde_json::Value;

use crate::config::{self, Config};
use crate::model::{self, Input, Sample, Tier};
use crate::patchnotes::{self, PatchNote};
use crate::records::{compare_versions, same_version_shape};
use crate::scan::{Scanner, Source, Step, VersionStats};
use crate::server::{Request, APPLY_TIERS, FIELD, RESULT_EVENT};
use crate::{diag, draft, shared};

/// Everything the client half needs from the game (the client context, or a test double).
pub trait Game: Source {
    fn player_team(&mut self) -> Option<usize>;
    fn team_name(&mut self, team: usize) -> Option<String>;
    fn champion_names(&mut self) -> Vec<String>;
    fn game_date(&mut self) -> Option<(i32, u32, u32)>;
    fn send_command(&mut self, command: &str, payload: &[u8]) -> bool;
    fn take_events(&mut self) -> Vec<(String, Vec<u8>)>;
    /// The screen inside the save (`None` when the game does not say).
    fn client_scene(&mut self) -> Option<ClientSceneKindV1> {
        None
    }
}

/// The mod only works on the management screens. Everything around a match - lineup, stadium
/// entrance, the match itself (`InGame`/`Match`), result, locker room - is left alone so not a
/// single frame there is spent on it.
fn idle_scene(scene: Option<ClientSceneKindV1>) -> bool {
    !matches!(scene, None | Some(ClientSceneKindV1::Main))
}

struct ClientGame<'a, 'b> {
    ctx: &'a mut StableClient<'b>,
}

impl Source for ClientGame<'_, '_> {
    fn record_ids(&mut self, kind: RecordKindV1) -> Vec<usize> {
        self.ctx.record_ids(kind)
    }
    fn record_json(&mut self, kind: RecordKindV1, id: usize, path: &str) -> Option<String> {
        self.ctx.record_get_json(kind, id, path)
    }
}

impl Game for ClientGame<'_, '_> {
    fn player_team(&mut self) -> Option<usize> {
        self.ctx.player_team_id()
    }
    fn team_name(&mut self, team: usize) -> Option<String> {
        self.ctx.team_name(team)
    }
    fn champion_names(&mut self) -> Vec<String> {
        self.ctx.champion_names()
    }
    fn game_date(&mut self) -> Option<(i32, u32, u32)> {
        self.ctx.game_date()
    }
    fn send_command(&mut self, command: &str, payload: &[u8]) -> bool {
        self.ctx.send_command(command, payload)
    }
    fn take_events(&mut self) -> Vec<(String, Vec<u8>)> {
        self.ctx.take_events().into_iter().map(|e| (e.event, e.payload)).collect()
    }
    fn client_scene(&mut self) -> Option<ClientSceneKindV1> {
        self.ctx.client_scene_kind()
    }
}

pub struct ClientExt;

impl StableExtension for ClientExt {
    fn post_update(&self, ctx: &mut StableClient<'_>, _dt_micros: u64) {
        let in_game = ctx.is_in_game();
        tick(&mut ClientGame { ctx }, in_game, Instant::now());
    }
}

const IDENTITY_EVERY: Duration = Duration::from_secs(2);
const LIST_EVERY: Duration = Duration::from_secs(10);
/// Unplayed solo-rank matches looked at again once per in-game day, oldest first.
const RECHECK_UNPLAYED: usize = 200;
const REBUILD_EVERY: Duration = Duration::from_secs(5);
const NEWS_EVERY: Duration = Duration::from_secs(300);
/// Records read per save at most (newest first); older ones add little to the current patch.
const BACKLOG_COMPETITION: u32 = 3000;
const BACKLOG_SOLO: u32 = 2000;
const TABLE_EVERY: Duration = Duration::from_secs(10);
const REPLY_TIMEOUT: Duration = Duration::from_secs(20);
/// Average reading time per frame. A record that takes longer is paid back over the next
/// frames (no reading until then), so a heavy replay costs one short hitch, not a slow game.
const FRAME_BUDGET: Duration = Duration::from_micros(1000);
const MAX_READS_PER_FRAME: u32 = 20;

struct State {
    out_of_game: bool,
    save: Option<(usize, String)>,
    next_identity: Option<Instant>,
    next_list: Option<Instant>,
    unplayed_day: Option<(i32, u32, u32)>,
    comp: Scanner,
    solo: Scanner,
    /// Reading time spent beyond the per-frame budget, paid back by frames that do not read.
    debt: Duration,
    scene: Option<ClientSceneKindV1>,
    reported_backfill: bool,
    dirty: bool,
    next_rebuild: Option<Instant>,
    rebuilt_with: Option<Arc<Config>>,
    notes: Vec<PatchNote>,
    news_read_at: Option<Instant>,
    /// The newest played version when the news was last read.
    news_version: Option<String>,
    table_written_at: Option<Instant>,
    table: Option<String>,
    last_summary: String,
    /// The tier plan was built after every listed record had been read.
    plan_complete: bool,
    tiers: TierSync,
}

impl State {
    fn new() -> Self {
        Self {
            out_of_game: true,
            save: None,
            next_identity: None,
            next_list: None,
            unplayed_day: None,
            comp: Scanner::new(RecordKindV1::MatchReplay, false),
            solo: Scanner::new(RecordKindV1::SoloRankMatch, true),
            debt: Duration::ZERO,
            scene: None,
            reported_backfill: false,
            dirty: false,
            next_rebuild: None,
            rebuilt_with: None,
            notes: Vec::new(),
            news_read_at: None,
            news_version: None,
            table_written_at: None,
            table: None,
            last_summary: String::new(),
            plan_complete: false,
            tiers: TierSync::default(),
        }
    }
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn lock() -> MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn reset_for_tests() {
    *lock() = None;
}

fn due(at: Option<Instant>, now: Instant) -> bool {
    at.is_none_or(|at| now >= at)
}

/// One frame of the client half. `in_game` = a save is open (scene `InGame`).
pub fn tick(game: &mut impl Game, in_game: bool, now: Instant) {
    let mut guard = lock();
    let st = guard.get_or_insert_with(State::new);
    if !in_game {
        if !st.out_of_game {
            if st.save.is_some() {
                diag::log("save closed; statistics are rebuilt when a save is opened again");
            }
            *st = State::new();
            shared::clear();
        }
        return;
    }
    st.out_of_game = false;
    config::refresh(now);
    let cfg = config::get();
    if !cfg.ban_pick_on() && !cfg.tier_list_on() {
        if shared::get().is_some() {
            shared::clear();
        }
        return;
    }
    run(st, game, &cfg, now);
}

fn run(st: &mut State, game: &mut impl Game, cfg: &Arc<Config>, now: Instant) {
    let scene = game.client_scene();
    if scene != st.scene {
        diag::log(&format!("screen: {scene:?}{}", if idle_scene(scene) { " (mod paused)" } else { "" }));
        st.scene = scene;
    }
    if idle_scene(scene) {
        return;
    }
    // which save is open (another one may have been loaded without leaving the game scene)
    if due(st.next_identity, now) {
        st.next_identity = Some(now + IDENTITY_EVERY);
        let Some(team) = game.player_team() else { return };
        let identity = (team, game.team_name(team).unwrap_or_default());
        if st.save.as_ref() != Some(&identity) {
            if st.save.is_some() {
                diag::log("another save is open: starting over");
            }
            *st = State::new();
            st.out_of_game = false;
            st.next_identity = Some(now + IDENTITY_EVERY);
            shared::clear();
            diag::reset_once();
            diag::log(&format!("save open: team #{} {:?}", identity.0, identity.1));
            st.save = Some(identity);
        }
    }
    let Some((team, _)) = st.save.clone() else { return };

    if due(st.next_list, now) {
        st.next_list = Some(now + LIST_EVERY);
        let day = game.game_date();
        let new_day = day.is_some() && day != st.unplayed_day;
        if new_day {
            st.unplayed_day = day;
        }
        st.comp.refresh(game, 0);
        st.comp.refresh_new(game);
        if cfg.solo_weight > 0.0 {
            st.solo.refresh(game, if new_day { RECHECK_UNPLAYED } else { 0 });
            st.solo.refresh_new(game);
        }
        diag::log_once(
            "listed",
            &format!(
                "records listed: {} competition (MatchReplay), {} solo rank (SoloRankMatch)",
                st.comp.listed, st.solo.listed
            ),
        );
    }

    read_records(st, game, cfg);
    st.comp.cap_backlog(BACKLOG_COMPETITION);
    st.solo.cap_backlog(BACKLOG_SOLO);
    report_probes(st);

    let caught_up = st.comp.pending() == 0 && st.solo.pending() == 0;
    let config_changed = st.rebuilt_with.as_ref().is_none_or(|c| !Arc::ptr_eq(c, cfg));
    if (st.dirty || config_changed) && due(st.next_rebuild, now) {
        st.next_rebuild = Some(now + REBUILD_EVERY);
        st.dirty = false;
        st.rebuilt_with = Some(Arc::clone(cfg));
        rebuild(st, game, cfg, team, now);
        st.plan_complete = caught_up;
    }
    if st.table.is_some() && due(st.table_written_at.map(|t| t + TABLE_EVERY), now) {
        st.table_written_at = Some(now);
        if let Some(text) = st.table.take() {
            diag::write_table(&text);
        }
    }

    // the tier list goes out once it was built from the whole backlog, not after every
    // partial rebuild while the save is still being read
    if cfg.tier_list_on() {
        let complete = st.plan_complete;
        sync_tiers(st, game, team, now, complete);
    }
}

fn read_records(st: &mut State, game: &mut impl Game, cfg: &Config) {
    if st.debt > Duration::ZERO {
        st.debt = st.debt.saturating_sub(FRAME_BUDGET);
        return;
    }
    let frame = Instant::now();
    for _ in 0..MAX_READS_PER_FRAME {
        let step = if st.comp.pending() > 0 {
            st.comp.step(game)
        } else if cfg.solo_weight > 0.0 && st.solo.pending() > 0 {
            st.solo.step(game)
        } else {
            Step::Idle
        };
        if let Step::Idle = step {
            break;
        }
        st.dirty = true;
        if frame.elapsed() >= FRAME_BUDGET {
            break;
        }
    }
    st.debt = frame.elapsed().saturating_sub(FRAME_BUDGET).min(Duration::from_secs(2));
}

fn report_probes(st: &mut State) {
    for (name, scan) in [("competition", &st.comp), ("solo rank", &st.solo)] {
        if let Some(text) = &scan.first_match {
            diag::log_once(&format!("probe-match-{name}"), &format!("[probe] {name} {text}"));
        }
        if let Some(text) = &scan.first_problem {
            diag::log_once(&format!("probe-problem-{name}"), &format!("[probe] {name} unusable {text}"));
        }
    }
    if !st.reported_backfill && st.comp.listed > 0 && st.comp.pending() == 0 && st.solo.pending() == 0 {
        st.reported_backfill = true;
        let c = &st.comp.counts;
        let s = &st.solo.counts;
        diag::log(&format!(
            "caught up: competition {} matches of {} read (unusable {}, unreadable {}, older patches skipped {}, \
             fields searched {}), solo rank {} matches of {} read (not played yet {}, unusable {}); versions {:?}",
            c.matches,
            c.read,
            c.invalid,
            c.fetch_failed,
            c.skipped_old,
            c.searched,
            s.matches,
            s.read,
            s.not_played,
            s.invalid,
            st.comp.versions_newest_first()
        ));
    }
}

/// A patch version as the patch notes give it: the first `title_bind`/`version` string shaped
/// like the record versions.
fn note_version(note: &PatchNote, like: Option<&str>) -> Option<String> {
    note.versions
        .iter()
        .find(|v| match like {
            Some(like) => same_version_shape(v, like),
            None => v.contains('.') && v.chars().next().is_some_and(|c| c.is_ascii_digit()),
        })
        .cloned()
}

/// The current patch: the newest version with games, or a newer one the patch notes already
/// announced (no games yet). A note's version only counts when the notes and the match records
/// number patches the same way - some note names a version that has games - otherwise
/// `Err(explanation)`.
fn current_version(
    notes: &[PatchNote],
    record_current: Option<&str>,
    played: &BTreeMap<String, VersionStats>,
) -> Result<Option<String>, String> {
    let newest_note = notes
        .iter()
        .filter_map(|n| note_version(n, record_current))
        .max_by(|a, b| compare_versions(a, b));
    let (Some(record), Some(note)) = (record_current, newest_note) else {
        return Ok(record_current.map(str::to_string));
    };
    if !compare_versions(&note, record).is_gt() {
        return Ok(Some(record.to_string()));
    }
    let consistent = notes.iter().any(|n| n.versions.iter().any(|v| played.contains_key(v)));
    if consistent {
        Ok(Some(note))
    } else {
        Err(format!(
            "patch notes name versions {:?} but matches were played in {:?}: notes only used once they match",
            notes.iter().filter_map(|n| n.versions.first()).take(5).collect::<Vec<_>>(),
            played.keys().take(5).collect::<Vec<_>>()
        ))
    }
}

fn rebuild(st: &mut State, game: &mut impl Game, cfg: &Config, team: usize, now: Instant) {
    // the patch notes (re-read every 30 s; they change once per in-game patch)
    let played_newest = st.comp.current_version().or_else(|| st.solo.current_version());
    if due(st.news_read_at.map(|t| t + NEWS_EVERY), now) || played_newest != st.news_version {
        st.news_read_at = Some(now);
        st.news_version = played_newest;
        match game.record_json(RecordKindV1::Team, team, "news") {
            Some(json) => match patchnotes::parse_news(&json) {
                Ok(notes) => {
                    diag::log_once(
                        "probe-news",
                        &format!(
                            "[probe] news: {} patch notes; latest {:?}",
                            notes.len(),
                            notes.last().map(|n| (&n.versions, n.changes.len()))
                        ),
                    );
                    st.notes = notes;
                }
                Err(why) => diag::log_once("probe-news", &format!("[probe] news unusable: {why}")),
            },
            None => diag::log_once("probe-news", "[probe] the team record has no readable news"),
        }
    }

    let record_current = st.comp.current_version().or_else(|| st.solo.current_version());
    let current = match current_version(&st.notes, record_current.as_deref(), &st.comp.versions) {
        Ok(version) => version,
        Err(mismatch) => {
            diag::log_once("probe-note-versions", &format!("[probe] {mismatch}"));
            record_current.clone()
        }
    };
    let Some(current) = current else { return };
    let previous = if Some(&current) != record_current.as_ref() {
        record_current.clone()
    } else {
        st.comp.versions_newest_first().into_iter().find(|v| compare_versions(v, &current).is_lt())
    };
    let note = st.notes.iter().rev().find(|n| n.is_for(&current));

    let empty = VersionStats::default();
    let cur_stats = st.comp.versions.get(&current).unwrap_or(&empty);
    let prev_stats = previous.as_ref().and_then(|p| st.comp.versions.get(p)).unwrap_or(&empty);
    let solo_stats = st.solo.versions.get(&current).unwrap_or(&empty);

    let mut champions = game.champion_names();
    if champions.is_empty() {
        champions = cur_stats.champs.keys().chain(prev_stats.champs.keys()).cloned().collect();
        champions.sort();
        champions.dedup();
    }
    let unknown: Vec<&String> =
        cur_stats.champs.keys().filter(|c| !champions.contains(c)).take(5).collect();
    if !unknown.is_empty() {
        diag::log_once(
            "probe-unknown",
            &format!("[probe] champions in matches but not selectable (ignored), e.g. {unknown:?}"),
        );
    }

    let sample = |stats: &VersionStats, name: &str| {
        stats.champs.get(name).map_or(Sample::default(), |c| Sample::new(c.m, c.w))
    };
    let matches = (cur_stats.matches + prev_stats.matches) as f32;
    let mut rows = Vec::with_capacity(champions.len());
    for name in &champions {
        let input = Input {
            champion: name,
            cur: sample(cur_stats, name),
            prev: sample(prev_stats, name),
            solo: sample(solo_stats, name),
            patch_dir: note.map_or(0, |n| n.direction(name)),
        };
        let est = model::estimate(cfg, &input);
        let presence = if matches > 0.0 { (input.cur.m + input.prev.m) / matches } else { 0.0 };
        rows.push(Row { name: name.clone(), input_cur: input.cur, input_prev: input.prev, input_solo: input.solo, patch_dir: input.patch_dir, est, presence, pick: 0.0, ban: 0.0, tier: None });
    }
    let typical = if rows.is_empty() { 0.0 } else { rows.iter().map(|r| r.presence).sum::<f32>() / rows.len() as f32 };
    let mut tables = shared::Tables::default();
    for row in &mut rows {
        let (pick, ban) = draft::amounts(cfg, row.est.value, row.presence, typical);
        row.pick = pick;
        row.ban = ban;
        if pick.abs() > 1e-4 {
            tables.pick.insert(row.name.clone(), pick);
        }
        if ban.abs() > 1e-4 {
            tables.ban.insert(row.name.clone(), ban);
        }
    }
    shared::publish(tables);

    let mut ranked: Vec<(String, f32)> = rows
        .iter()
        .filter(|r| r.est.evidence >= cfg.min_games && r.est.evidence > 0.0)
        .map(|r| (r.name.clone(), r.est.score()))
        .collect();
    let tiers: HashMap<String, Tier> = model::tiers_by_share(cfg, &mut ranked).into_iter().collect();
    for row in &mut rows {
        row.tier = tiers.get(&row.name).copied();
    }

    let mut plan = BTreeMap::new();
    for row in &rows {
        match row.tier {
            Some(tier) => {
                plan.insert(row.name.clone(), tier.as_str().to_string());
            }
            None if cfg.clear_unranked => {
                plan.insert(row.name.clone(), Tier::NoTier.as_str().to_string());
            }
            None => {}
        }
    }
    // nothing ranked yet (a new save): leave the team's list alone
    if !ranked.is_empty() {
        st.tiers.set_plan(team, plan, !cfg.clear_unranked);
    }

    let summary = format!(
        "patch {current} (previous {}): competition {} matches this patch, {} previous; solo rank {}; \
         patch notes {}; ranked {}/{} champions{}",
        previous.as_deref().unwrap_or("-"),
        cur_stats.matches,
        prev_stats.matches,
        solo_stats.matches,
        note.map_or("none".to_string(), |n| {
            let buffs = n.changes.values().filter(|v| **v > 0).count();
            let nerfs = n.changes.values().filter(|v| **v < 0).count();
            format!("{buffs} buffed, {nerfs} nerfed")
        }),
        ranked.len(),
        rows.len(),
        tier_counts(&rows)
    );
    if summary != st.last_summary {
        diag::log(&format!("[rebuild] {summary}"));
        st.last_summary = summary;
        if cfg.verbose {
            let mut by_pick: Vec<&Row> = rows.iter().collect();
            by_pick.sort_by(|a, b| b.pick.partial_cmp(&a.pick).unwrap_or(std::cmp::Ordering::Equal));
            let show = |r: &&Row| format!("{} {:+.2}", r.name, r.pick);
            let top: Vec<String> = by_pick.iter().take(5).map(show).collect();
            let bottom: Vec<String> = by_pick.iter().rev().take(5).map(show).collect();
            diag::log(&format!("[rebuild] picks up: {}; down: {}", top.join(", "), bottom.join(", ")));
        }
    }
    st.table = Some(table(&current, previous.as_deref(), &st.last_summary, &mut rows));
}

struct Row {
    name: String,
    input_cur: Sample,
    input_prev: Sample,
    input_solo: Sample,
    patch_dir: i32,
    est: model::Estimate,
    presence: f32,
    pick: f32,
    ban: f32,
    tier: Option<Tier>,
}

fn tier_counts(rows: &[Row]) -> String {
    let mut counts = [0usize; 5];
    for row in rows {
        if let Some(tier) = row.tier {
            if let Some(slot) = counts.get_mut(tier as usize) {
                *slot += 1;
            }
        }
    }
    if counts.iter().all(|c| *c == 0) {
        return String::new();
    }
    format!(" (S {} A {} B {} C {} D {})", counts[0], counts[1], counts[2], counts[3], counts[4])
}

fn table(current: &str, previous: Option<&str>, summary: &str, rows: &mut [Row]) -> String {
    rows.sort_by(|a, b| b.est.score().partial_cmp(&a.est.score()).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = format!(
        "{} {} - patch {current} (previous {})\n{summary}\n\n\
         tier   = tier written to your team (- = not enough games)\n\
         win%   = estimated win rate this patch, +- its standard error; start = where it started\n\
         games  = this patch / previous patch / solo rank; carried = previous-patch games counted\n\
         pick, ban = added to the AI's draft scores\n\n",
        crate::MOD_ID,
        crate::VERSION,
        previous.unwrap_or("-")
    );
    out.push_str(&format!(
        "{:<22} {:>4} {:>13} {:>6} {:>15} {:>7} {:>5} {:>6} {:>6} {:>6}\n",
        "champion", "tier", "win%", "start", "games c/p/s", "carried", "notes", "sure", "pick", "ban"
    ));
    for r in rows.iter() {
        out.push_str(&format!(
            "{:<22} {:>4} {:>7.1}+-{:<4.1} {:>6.1} {:>15} {:>7.0} {:>5} {:>5.0}% {:>+6.2} {:>+6.2}\n",
            diag::clip(&r.name, 22),
            r.tier.map_or("-", Tier::as_str),
            r.est.p * 100.0,
            r.est.se * 100.0,
            r.est.start * 100.0,
            format!("{}/{}/{}", r.input_cur.m, r.input_prev.m, r.input_solo.m),
            r.est.carried,
            match r.patch_dir.signum() {
                1 => "buff",
                -1 => "nerf",
                _ => "",
            },
            r.est.certainty * 100.0,
            r.pick,
            r.ban
        ));
    }
    out
}

/// Keeps the team's tier list in line with the plan: sends it when it changes, waits for the
/// server's answer, retries with a growing pause, and checks after each in-game day that the
/// game did not put an older list back.
#[derive(Default)]
struct TierSync {
    plan: Option<Request>,
    plan_hash: u64,
    applied: Option<u64>,
    waiting: Option<(u64, Instant)>,
    failures: u32,
    retry_at: Option<Instant>,
    day: Option<(i32, u32, u32)>,
    resends: u32,
    sent_at: Option<Instant>,
}

impl TierSync {
    fn set_plan(&mut self, team: usize, tiers: BTreeMap<String, String>, keep_others: bool) {
        if tiers.is_empty() {
            return;
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (team, &tiers, keep_others).hash(&mut hasher);
        let hash = hasher.finish();
        if self.plan.is_some() && hash == self.plan_hash {
            return;
        }
        self.plan_hash = hash;
        self.plan = Some(Request { team, hash: format!("{hash:016x}"), keep_others, tiers });
        self.failures = 0;
        self.retry_at = None;
        self.resends = 0;
    }
}

fn backoff(failures: u32) -> Duration {
    Duration::from_secs((30u64 << failures.min(4)).min(600))
}

/// Times the same list is written again after something else changed it.
const MAX_RESENDS: u32 = 3;

/// Shortest time between two tier lists sent to the server.
const SEND_GAP: Duration = Duration::from_secs(10);

fn sync_tiers(st: &mut State, game: &mut impl Game, team: usize, now: Instant, plan_complete: bool) {
    let sync = &mut st.tiers;
    for (event, payload) in game.take_events() {
        if event != RESULT_EVENT {
            continue;
        }
        let text = String::from_utf8_lossy(&payload);
        let mut parts = text.splitn(3, '\t');
        let (hash, ok, detail) = (parts.next().unwrap_or(""), parts.next() == Some("ok"), parts.next().unwrap_or(""));
        let Some((sent, _)) = sync.waiting else { continue };
        if hash != format!("{sent:016x}") {
            continue;
        }
        sync.waiting = None;
        if ok {
            sync.applied = Some(sent);
            sync.failures = 0;
            diag::log(&format!("tier list accepted by the server ({detail})"));
        } else {
            sync.failures += 1;
            sync.retry_at = Some(now + backoff(sync.failures));
            diag::log(&format!("tier list refused by the server ({detail}); trying again later"));
        }
    }
    if let Some((_, at)) = sync.waiting {
        if now.duration_since(at) > REPLY_TIMEOUT {
            sync.waiting = None;
            sync.failures += 1;
            sync.retry_at = Some(now + backoff(sync.failures));
            diag::log("no answer from the server about the tier list; trying again later");
        }
    }
    let Some(plan) = &sync.plan else { return };

    // after each in-game day: is the list we wrote still there?
    let day = game.game_date();
    if day.is_some() && day != sync.day {
        let first_look = sync.day.is_none();
        sync.day = day;
        if !first_look && sync.applied == Some(sync.plan_hash) && sync.resends < MAX_RESENDS {
            if let Some(stale) = differs(game, team, plan) {
                sync.applied = None;
                sync.resends += 1;
                if sync.resends < MAX_RESENDS {
                    diag::log(&format!("tier list changed in the game ({stale}); writing it again"));
                } else {
                    // written back again and again: another mod (or the player) owns the list
                    sync.applied = Some(sync.plan_hash);
                    diag::log(&format!(
                        "tier list changed in the game again ({stale}): another mod seems to write it too                          (e.g. Drafter's Toolbox); leaving it alone until the list here changes.                          Set tier_list=off in settings.ini to stop for good."
                    ));
                }
            }
        }
    }

    if !plan_complete
        || sync.applied == Some(sync.plan_hash)
        || sync.waiting.is_some()
        || !due(sync.retry_at, now)
        || !due(sync.sent_at.map(|t| t + SEND_GAP), now)
    {
        return;
    }
    let payload = plan.encode();
    sync.sent_at = Some(now);
    if game.send_command(APPLY_TIERS, payload.as_bytes()) {
        sync.waiting = Some((sync.plan_hash, now));
    } else {
        sync.failures += 1;
        sync.retry_at = Some(now + backoff(sync.failures));
        diag::log_once("send-failed", "could not send the tier list to the server (will retry)");
    }
}

/// `Some(example)` when the team's tier list, as the client sees it, is not the plan.
fn differs(game: &mut impl Game, team: usize, plan: &Request) -> Option<String> {
    let json = game.record_json(RecordKindV1::Team, team, FIELD)?;
    let Ok(Value::Object(seen)) = serde_json::from_str::<Value>(&json) else { return None };
    plan.tiers.iter().find_map(|(name, tier)| {
        let now = seen.get(name).and_then(Value::as_str).unwrap_or("NoTier");
        let expected_missing = tier == "NoTier" && !seen.contains_key(name);
        (now != tier && !expected_missing).then(|| format!("{name} is {now}, should be {tier}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::tests::game as replay;
    use crate::server::{self, TeamDocs};
    use std::collections::BTreeMap as Map;

    /// A save: records, the team record (whose `champion_tiers` the "server" below edits),
    /// commands sent, events waiting.
    #[derive(Default)]
    struct FakeGame {
        team: usize,
        team_name: String,
        replays: Map<usize, String>,
        solo: Map<usize, String>,
        news: String,
        champions: Vec<String>,
        tiers: Value,
        date: (i32, u32, u32),
        sent: Vec<Request>,
        events: Vec<(String, Vec<u8>)>,
        accept: bool,
        read_delay: Duration,
        scene: Option<ClientSceneKindV1>,
        reads: u32,
    }

    impl Source for FakeGame {
        fn record_ids(&mut self, kind: RecordKindV1) -> Vec<usize> {
            match kind {
                RecordKindV1::MatchReplay => self.replays.keys().copied().collect(),
                RecordKindV1::SoloRankMatch => self.solo.keys().copied().collect(),
                _ => vec![],
            }
        }
        fn record_json(&mut self, kind: RecordKindV1, id: usize, path: &str) -> Option<String> {
            if kind == RecordKindV1::MatchReplay {
                self.reads += 1;
                std::thread::sleep(self.read_delay);
            }
            match (kind, path) {
                (RecordKindV1::MatchReplay, "") => self.replays.get(&id).cloned(),
                (RecordKindV1::SoloRankMatch, "") => self.solo.get(&id).cloned(),
                (RecordKindV1::Team, "news") if id == self.team => Some(self.news.clone()),
                (RecordKindV1::Team, "champion_tiers") if id == self.team => Some(self.tiers.to_string()),
                _ => None,
            }
        }
    }

    impl TeamDocs for FakeGame {
        fn team_json(&self, _team: usize, path: &str) -> Option<String> {
            (path == FIELD).then(|| self.tiers.to_string())
        }
        fn set_team_json(&mut self, _team: usize, path: &str, json: &str) -> bool {
            if path != FIELD || !self.accept {
                return false;
            }
            match serde_json::from_str(json) {
                Ok(v) => {
                    self.tiers = v;
                    true
                }
                Err(_) => false,
            }
        }
    }

    impl Game for FakeGame {
        fn player_team(&mut self) -> Option<usize> {
            Some(self.team)
        }
        fn team_name(&mut self, _team: usize) -> Option<String> {
            Some(self.team_name.clone())
        }
        fn champion_names(&mut self) -> Vec<String> {
            self.champions.clone()
        }
        fn game_date(&mut self) -> Option<(i32, u32, u32)> {
            Some(self.date)
        }
        fn send_command(&mut self, command: &str, payload: &[u8]) -> bool {
            assert_eq!(command, APPLY_TIERS);
            let req = Request::parse(payload, None).unwrap();
            // what the server does with it
            let out = server::apply(self, &req);
            let msg = format!("{}\t{}\t{}", out.hash, if out.ok { "ok" } else { "fail" }, out.detail);
            self.events.push((RESULT_EVENT.to_string(), msg.into_bytes()));
            self.sent.push(req);
            true
        }
        fn take_events(&mut self) -> Vec<(String, Vec<u8>)> {
            std::mem::take(&mut self.events)
        }
        fn client_scene(&mut self) -> Option<ClientSceneKindV1> {
            self.scene
        }
    }

    const NAMES: [&str; 10] = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"];

    /// Deterministic pseudo-random numbers in [0, 1).
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    /// 300 matches of patch 1.3, random line-ups: a side with "a" wins 25% more often, one with
    /// "j" 25% less - "a" ends near 70%, "j" near 30%, everyone else near 50%.
    fn save() -> FakeGame {
        let mut g = FakeGame {
            team: 4,
            team_name: "Mods FC".into(),
            champions: NAMES.iter().map(|s| s.to_string()).collect(),
            tiers: serde_json::json!({"z": "B"}),
            date: (2026, 3, 1),
            accept: true,
            news: r#"[{"kind":"PatchNote","title_bind":["1.3"],"champion_patch_data":[["e",[{"is_buff":true}]]]}]"#.into(),
            ..Default::default()
        };
        let mut rng = Lcg(7);
        for id in 0..300usize {
            let mut lineup: Vec<&str> = NAMES.to_vec();
            for i in (1..lineup.len()).rev() {
                let j = (rng.next() * (i + 1) as f64) as usize;
                lineup.swap(i, j);
            }
            let (blue, red) = (&lineup[..5], &lineup[5..]);
            let edge = |side: &[&str]| side.contains(&"a") as i32 as f64 * 0.25 - side.contains(&"j") as i32 as f64 * 0.25;
            let p_blue = (0.5 + edge(blue) - edge(red)).clamp(0.05, 0.95);
            g.replays.insert(id + 1, replay("1.3", blue, red, rng.next() < p_blue));
        }
        g
    }

    fn frames(g: &mut FakeGame, count: usize, start: Instant) -> Instant {
        let mut now = start;
        for _ in 0..count {
            tick(g, true, now);
            now += Duration::from_millis(100);
        }
        now
    }

    fn with_temp_dir() {
        let dir = std::env::temp_dir().join(format!("patch_meta_ai_client_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var(crate::paths::DIR_ENV, &dir);
        crate::reset_for_tests();
    }

    #[test]
    fn reads_rebuilds_publishes_and_writes_tiers() {
        let _serial = crate::tests::serial();
        with_temp_dir();
        let mut g = save();
        let now = frames(&mut g, 200, Instant::now());
        let tables = shared::get().expect("tables published");
        assert!(tables.pick["a"] > 0.3, "{:?}", tables.pick);
        assert!(tables.pick["j"] < -0.3);
        assert!(tables.ban["a"] > 0.0 && tables.ban["j"] < 0.0);
        // tiers were sent once, accepted, and merged into the existing list ("z" kept)
        assert_eq!(g.sent.len(), 1, "{:?}", g.sent);
        assert_eq!(g.tiers["a"], "S");
        assert_eq!(g.tiers["j"], "D");
        assert_eq!(g.tiers["z"], "B");
        // nothing new: nothing sent again
        let now = frames(&mut g, 50, now);
        assert_eq!(g.sent.len(), 1);
        // the game puts an old list back; after the next day the list is written again
        g.tiers = serde_json::json!({"a": "C"});
        g.date = (2026, 3, 2);
        frames(&mut g, 5, now);
        assert_eq!(g.sent.len(), 2);
        assert_eq!(g.tiers["a"], "S");
    }

    #[test]
    fn another_tier_writer_is_left_alone() {
        let _serial = crate::tests::serial();
        with_temp_dir();
        let mut g = save();
        let mut now = frames(&mut g, 200, Instant::now());
        assert_eq!(g.sent.len(), 1);
        // every in-game day another mod puts its own list back
        for day in 2..10 {
            g.tiers = serde_json::json!({"a": "C"});
            g.date = (2026, 3, day);
            now = frames(&mut g, 300, now);
        }
        assert_eq!(g.sent.len(), MAX_RESENDS as usize, "stops fighting: {:?}", g.sent.len());
        assert_eq!(g.tiers["a"], "C");
    }

    #[test]
    fn a_tier_mod_listed_in_mods_json_takes_over() {
        let _serial = crate::tests::serial();
        with_temp_dir();
        crate::compat::set(crate::compat::Others::from_mods_json(r#"{"enabled_mods":["drafters_toolkit"]}"#));
        let mut g = save();
        frames(&mut g, 200, Instant::now());
        assert!(g.sent.is_empty(), "tier_list=auto leaves the list to the Toolbox");
        assert!(shared::get().is_some(), "ban/pick still works");
        crate::compat::set(crate::compat::Others::default());
    }

    #[test]
    fn another_save_or_fewer_records_never_breaks_it() {
        let _serial = crate::tests::serial();
        with_temp_dir();
        let mut g = save();
        let now = frames(&mut g, 3, Instant::now());
        // a much smaller save is loaded in the same session
        g.replays.retain(|id, _| *id <= 3);
        g.team_name = "Other".into();
        let now = frames(&mut g, 100, now);
        let tables = shared::get();
        assert!(tables.is_some(), "rebuilt from the new save");
        // leaving the game clears everything
        tick(&mut g, false, now);
        assert!(shared::get().is_none());
    }

    #[test]
    fn slow_records_cost_about_a_millisecond_per_frame() {
        let _serial = crate::tests::serial();
        with_temp_dir();
        let mut g = save();
        g.read_delay = Duration::from_millis(20); // a heavy replay
        let started = Instant::now();
        let mut now = Instant::now();
        let mut worst = Duration::ZERO;
        for _ in 0..400 {
            let t = Instant::now();
            tick(&mut g, true, now);
            worst = worst.max(t.elapsed());
            now += Duration::from_millis(16);
        }
        let per_frame = started.elapsed() / 400;
        assert!(g.reads >= 15, "still reading: {}", g.reads);
        assert!(per_frame < Duration::from_micros(1800), "{per_frame:?} per frame");
        assert!(worst < Duration::from_millis(45), "one record per frame at most: {worst:?}");
    }

    #[test]
    fn nothing_happens_around_a_match() {
        let _serial = crate::tests::serial();
        with_temp_dir();
        for scene in [
            ClientSceneKindV1::Lineup,
            ClientSceneKindV1::StadiumEntrance,
            ClientSceneKindV1::InGame,
            ClientSceneKindV1::Match,
            ClientSceneKindV1::MatchResult,
            ClientSceneKindV1::LockerRoom,
        ] {
            crate::reset_for_tests();
            let mut g = save();
            g.scene = Some(scene);
            frames(&mut g, 100, Instant::now());
            assert_eq!(g.reads, 0, "{scene:?}");
            assert!(g.sent.is_empty());
        }
        let mut g = save();
        g.scene = Some(ClientSceneKindV1::Main);
        frames(&mut g, 5, Instant::now());
        assert!(g.reads > 0, "works on the management screen");
    }

    #[test]
    fn current_patch_from_records_and_notes() {
        let note = |v: &str| PatchNote { versions: vec![v.to_string()], ..Default::default() };
        let mut played = BTreeMap::new();
        played.insert("1.3".to_string(), VersionStats::default());
        // announced 1.4, no games yet: 1.4 is current (the notes number patches like the records)
        let notes = [note("1.3"), note("1.4")];
        assert_eq!(current_version(&notes, Some("1.3"), &played), Ok(Some("1.4".into())));
        // only older notes: the records decide
        assert_eq!(current_version(&[note("1.2")], Some("1.3"), &played), Ok(Some("1.3".into())));
        // notes that number patches differently are not trusted
        assert!(current_version(&[note("26.1")], Some("1.3"), &played).is_err());
        assert_eq!(current_version(&[], None, &played), Ok(None));
        assert_eq!(current_version(&notes, None, &played), Ok(None), "no games at all yet");
    }

    #[test]
    fn a_new_save_keeps_its_tier_list() {
        let _serial = crate::tests::serial();
        with_temp_dir();
        let mut g = save();
        g.replays.retain(|id, _| *id <= 1); // one match: nobody has min_games yet
        frames(&mut g, 100, Instant::now());
        assert!(g.sent.is_empty());
        assert_eq!(g.tiers, serde_json::json!({"z": "B"}));
    }

    #[test]
    fn a_refused_list_is_retried_later() {
        let _serial = crate::tests::serial();
        with_temp_dir();
        let mut g = save();
        g.accept = false;
        let now = frames(&mut g, 200, Instant::now());
        let tries = g.sent.len();
        assert!(tries >= 1);
        g.accept = true;
        frames(&mut g, 3000, now); // 300 s later
        assert!(g.sent.len() > tries);
        assert_eq!(g.tiers["a"], "S");
    }
}
