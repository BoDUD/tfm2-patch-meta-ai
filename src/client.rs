//! Client extension: while a save is open, reads match records a little every frame, has the
//! model refitted in the background every few seconds (`worker`), publishes the result for the
//! draft hook, writes the report files and asks the server to write the tier list (only the
//! server may change records).
//!
//! Everything runs in `post_update` under one lock that tolerates poisoning, and nothing in it
//! indexes a list it did not just measure, so one bad record or a save switch cannot stop it.

use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use mod_api_stable::{ClientSceneKindV1, RecordKindV1, StableClient, StableExtension};
use serde_json::Value;

use crate::advisor::Damage;
use crate::config::{self, Config};
use crate::history::{Game as Match, Names};
use crate::meta::Backtest;
use crate::model::Tier;
use crate::patchnotes::{self, PatchNote};
use crate::records::{compare_versions, same_version_shape};
use crate::report::{self, Labels};
use crate::scan::{Scanner, Source, Step, VersionStats};
use crate::server::{Request, APPLY_TIERS, FIELD, RESULT_EVENT};
use crate::worker::{Job, Worker};
use crate::{diag, meta, shared};

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
    /// Physical or magic damage, from the champion's tags.
    fn champion_damage(&mut self, _name: &str) -> Option<Damage> {
        None
    }
    /// The champion's display name.
    fn champion_label(&mut self, _name: &str) -> Option<String> {
        None
    }
    fn athlete_name(&mut self, _athlete: u32) -> Option<String> {
        None
    }
    /// The season schedule document (or part of it).
    fn schedule_json(&mut self, _path: &str) -> Option<String> {
        None
    }
    /// Fit in a background thread (tests fit in the frame, to stay deterministic).
    fn threaded(&self) -> bool {
        false
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
    fn champion_damage(&mut self, name: &str) -> Option<Damage> {
        use mod_api_stable::ChampionTagV1;
        let tags = self.ctx.champion_brief(name)?.tags;
        let (ad, ap) = (tags.contains(&ChampionTagV1::Ad), tags.contains(&ChampionTagV1::Ap));
        Some(match (ad, ap) {
            (true, false) => Damage::Physical,
            (false, true) => Damage::Magic,
            _ => Damage::Mixed,
        })
    }
    fn champion_label(&mut self, name: &str) -> Option<String> {
        let key = format!("#asset/base/text/champion?description.{name}.name");
        self.ctx.i18n(&key).filter(|s| !s.is_empty() && !s.starts_with('#'))
    }
    fn athlete_name(&mut self, athlete: u32) -> Option<String> {
        self.ctx.athlete_name(athlete as usize)
    }
    fn schedule_json(&mut self, path: &str) -> Option<String> {
        self.ctx.schedule_get_json(path)
    }
    fn threaded(&self) -> bool {
        true
    }
}

pub struct ClientExt;

impl StableExtension for ClientExt {
    fn post_update(&self, ctx: &mut StableClient<'_>, _dt_micros: u64) {
        let in_game = ctx.is_in_game();
        tick(&mut ClientGame { ctx }, in_game, Instant::now());
        if in_game {
            let scene = ctx.client_scene_kind();
            crate::ui::tick(ctx, scene, &config::get());
        } else {
            crate::ui::reset();
        }
    }
}

const IDENTITY_EVERY: Duration = Duration::from_secs(2);
const LIST_EVERY: Duration = Duration::from_secs(10);
/// Unplayed solo-rank matches looked at again once per in-game day, oldest first.
const RECHECK_UNPLAYED: usize = 200;
const REBUILD_EVERY: Duration = Duration::from_secs(5);
const REPORT_EVERY: Duration = Duration::from_secs(30);
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
    report_written_at: Option<Instant>,
    report: Option<String>,
    last_summary: String,
    /// The tier plan was built after every listed record had been read.
    plan_complete: bool,
    tiers: TierSync,
    names: Names,
    worker: Option<Worker>,
    /// Rebuilds submitted, and the one a result has to match to count.
    serial: u64,
    /// Whether the newest submitted rebuild had read every listed record.
    submitted_complete: bool,
    backtest: Option<Backtest>,
    backtest_done: bool,
    labels: Labels,
    damage: HashMap<u16, Damage>,
    champions: Vec<String>,
    /// Team names by id (asked once each).
    team_names: HashMap<u32, String>,
    probes_written: bool,
    records_probed: bool,
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
            report_written_at: None,
            report: None,
            last_summary: String::new(),
            plan_complete: false,
            tiers: TierSync::default(),
            names: Names::default(),
            worker: None,
            serial: 0,
            submitted_complete: false,
            backtest: None,
            backtest_done: false,
            labels: Labels::default(),
            damage: HashMap::new(),
            champions: Vec::new(),
            team_names: HashMap::new(),
            probes_written: false,
            records_probed: false,
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
    // the model is needed by any of the features, not only the two that change the game
    if !cfg.ban_pick_on() && !cfg.tier_list_on() && !cfg.draft_overlay && !cfg.report {
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
        rebuild(st, game, cfg, team, now, caught_up);
    }
    collect(st, cfg, team);
    if st.table.is_some() && due(st.table_written_at.map(|t| t + TABLE_EVERY), now) {
        st.table_written_at = Some(now);
        if let Some(text) = st.table.take() {
            diag::write_table(&text);
        }
    }
    if st.report.is_some() && due(st.report_written_at.map(|t| t + REPORT_EVERY), now) {
        st.report_written_at = Some(now);
        if let Some(text) = st.report.take() {
            diag::write_file(diag::REPORT_FILE, &text);
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
            st.comp.step(game, &mut st.names)
        } else if cfg.solo_weight > 0.0 && st.solo.pending() > 0 {
            st.solo.step(game, &mut st.names)
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
    if !st.probes_written {
        if let Some(raw) = &st.comp.first_raw {
            st.probes_written = true;
            diag::write_file("probe_competition.json", &pretty(raw));
            diag::log("wrote probe_competition.json (one competition record as the game gives it)");
        }
    }
    if let Some(raw) = st.solo.first_raw.take() {
        diag::write_file("probe_solo_rank.json", &pretty(&raw));
    }
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

/// What the screens know about every team, by lower-case team name.
#[derive(Default)]
struct Teams {
    rosters: HashMap<String, Vec<(u32, Option<crate::history::Role>)>>,
    labels: HashMap<String, String>,
    picks: HashMap<String, Vec<(String, u32, u32)>>,
    last_opponent: Option<String>,
}

/// Matches per team the "most played" lists count.
const RECENT_PER_TEAM: usize = 20;

/// Each team's players and lanes (its newest competition match) and its most played champions
/// lately; the player's last opponent.
fn teams(st: &mut State, game: &mut impl Game, own: usize) -> Teams {
    let mut by_team: HashMap<u32, Vec<(usize, &Match, usize)>> = HashMap::new();
    for g in st.comp.games.values() {
        for (side, team) in g.teams.iter().enumerate() {
            if let Some(team) = team {
                by_team.entry(*team).or_default().push((g.record, g, side));
            }
        }
    }
    let mut out = Teams::default();
    let mut wanted_athletes = Vec::new();
    for (team, mut list) in by_team {
        list.sort_by_key(|x| std::cmp::Reverse(x.0));
        let label = st
            .team_names
            .entry(team)
            .or_insert_with(|| game.team_name(team as usize).unwrap_or_default())
            .trim()
            .to_string();
        let key = label.to_lowercase();
        if key.is_empty() {
            continue;
        }
        let (_, newest, side) = list[0];
        let players: Vec<(u32, Option<crate::history::Role>)> =
            newest.sides[side].iter().filter_map(|s| Some((s.athlete?, s.role))).collect();
        wanted_athletes.extend(players.iter().map(|(a, _)| *a));
        if team as usize == own {
            let other = newest.teams[1 - side];
            out.last_opponent = other.and_then(|o| st.team_names.get(&o)).map(|n| n.trim().to_lowercase());
        }
        let mut counts: HashMap<u16, (u32, u32)> = HashMap::new();
        for (_, g, side) in list.iter().take(RECENT_PER_TEAM) {
            for s in &g.sides[*side] {
                let e = counts.entry(s.champ).or_default();
                e.0 += 1;
                e.1 += g.won(*side) as u32;
            }
        }
        let mut picks: Vec<(String, u32, u32)> =
            counts.into_iter().map(|(c, (g, w))| (st.names.name(c).to_string(), g, w)).collect();
        picks.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
        picks.truncate(12);
        if !players.is_empty() {
            out.rosters.insert(key.clone(), players);
        }
        out.picks.insert(key.clone(), picks);
        out.labels.insert(key, label);
    }
    for a in wanted_athletes {
        if let std::collections::hash_map::Entry::Vacant(slot) = st.labels.athletes.entry(a) {
            if let Some(name) = game.athlete_name(a) {
                slot.insert(name);
            }
        }
    }
    out
}

/// Once per save: what the team, athlete, fixture and schedule records look like, for the
/// features that need them (`probe_records.txt`).
fn probe_records(game: &mut impl Game, team: usize) {
    let mut out = format!("{} {} - record layouts of this save (long lists cut short)\n", crate::MOD_ID, crate::VERSION);
    let mut section = |title: &str, json: Option<String>| {
        out.push_str(&format!("\n===== {title} =====\n"));
        out.push_str(&json.map_or("(nothing)".to_string(), |j| pretty(&j)));
        out.push('\n');
    };
    section(&format!("Team #{team}"), game.record_json(RecordKindV1::Team, team, ""));
    let athletes = game.record_ids(RecordKindV1::Athlete);
    section("Athlete (first)", athletes.first().and_then(|id| game.record_json(RecordKindV1::Athlete, *id, "")));
    for kind in [RecordKindV1::MatchNormal, RecordKindV1::Match, RecordKindV1::YearSchedule, RecordKindV1::LeagueCompetition] {
        let ids = game.record_ids(kind);
        let mut picked: Vec<usize> = ids.first().copied().into_iter().chain(ids.last().copied()).collect();
        picked.dedup();
        for id in picked {
            section(&format!("{kind:?} #{id} (of {})", ids.len()), game.record_json(kind, id, ""));
        }
    }
    section("schedule", game.schedule_json(""));
    diag::write_file("probe_records.txt", &out);
    diag::log("wrote probe_records.txt (team, athlete, fixture and schedule layouts)");
}

/// A record as indented JSON, with very long arrays (replay inputs) cut short.
fn pretty(raw: &str) -> String {
    fn trim(v: &mut Value) {
        match v {
            Value::Array(items) => {
                if items.len() > 12 {
                    let n = items.len();
                    items.truncate(12);
                    items.push(Value::String(format!("... {} more", n - 12)));
                }
                items.iter_mut().for_each(trim);
            }
            Value::Object(map) => map.values_mut().for_each(trim),
            _ => {}
        }
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(mut v) => {
            trim(&mut v);
            serde_json::to_string_pretty(&v).unwrap_or_else(|_| raw.to_string())
        }
        Err(_) => raw.to_string(),
    }
}

fn rebuild(st: &mut State, game: &mut impl Game, cfg: &Config, team: usize, now: Instant, caught_up: bool) {
    // the patch notes (re-read every 5 minutes; they change once per in-game patch)
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

    // the champion list, names and damage types (once per save)
    if st.champions.is_empty() {
        let mut champions = game.champion_names();
        if champions.is_empty() {
            // the host does not list champions: rate the ones seen in matches
            champions = st.comp.versions.values().flat_map(|v| v.champs.keys().cloned()).collect();
            champions.sort();
            champions.dedup();
        }
        for name in &champions {
            let id = st.names.id(name);
            if let Some(d) = game.champion_damage(name) {
                st.damage.insert(id, d);
            }
            if let Some(label) = game.champion_label(name) {
                st.labels.champions.insert(name.clone(), label);
            }
        }
        st.labels.team_name = game.team_name(team).unwrap_or_default();
        st.champions = champions;
    }
    let unknown: Vec<&str> = (0..st.names.len() as u16)
        .map(|id| st.names.name(id))
        .filter(|n| !st.champions.iter().any(|c| c == n))
        .take(5)
        .collect();
    if !unknown.is_empty() {
        diag::log_once(
            "probe-unknown",
            &format!("[probe] champions in matches but not selectable (rated anyway), e.g. {unknown:?}"),
        );
    }

    // the player's line-up: the athletes of their newest competition match
    let newest_own = st
        .comp
        .games
        .values()
        .filter(|g| g.teams.contains(&Some(team as u32)))
        .max_by_key(|g| g.record);
    if let Some(g) = newest_own {
        let side = if g.teams[0] == Some(team as u32) { 0 } else { 1 };
        let roster: Vec<u32> = g.sides[side].iter().filter_map(|s| s.athlete).collect();
        if roster != st.labels.roster {
            for a in &roster {
                if !st.labels.athletes.contains_key(a) {
                    if let Some(name) = game.athlete_name(*a) {
                        st.labels.athletes.insert(*a, name);
                    }
                }
            }
            st.labels.roster = roster;
        }
    }
    st.labels.date = game.game_date();
    let teams = teams(st, game, team);
    crate::ui::set_context(crate::ui::Context {
        team_name: st.labels.team_name.clone(),
        champions: st.champions.clone(),
        english: st.labels.champions.iter().map(|(id, label)| (label.clone(), id.clone())).collect(),
        rosters: teams.rosters,
        team_labels: teams.labels,
        team_picks: teams.picks,
        athletes: st.labels.athletes.clone(),
        last_opponent: teams.last_opponent,
        backtest: st.backtest,
    });
    if !st.records_probed {
        st.records_probed = true;
        probe_records(game, team);
    }

    let mut games: Vec<Match> = st.comp.games.values().cloned().collect();
    if cfg.solo_weight > 0.0 {
        games.extend(st.solo.games.values().cloned());
    }
    st.serial += 1;
    st.submitted_complete = caught_up;
    // the backtest once per save, when everything has been read
    let backtest = caught_up && !st.backtest_done;
    st.backtest_done |= backtest;
    let job = Job {
        serial: st.serial,
        games,
        names: st.names.clone(),
        champions: st.champions.clone(),
        notes: st.notes.clone(),
        current,
        settings: cfg.model(),
        backtest,
    };
    let threaded = game.threaded();
    st.worker.get_or_insert_with(|| Worker::new(threaded)).submit(job);
}

/// Picks up a finished fit: publishes it for the draft hook, plans the tier list, prepares the
/// report files.
fn collect(st: &mut State, cfg: &Config, team: usize) {
    let Some(done) = st.worker.as_mut().and_then(Worker::take) else { return };
    // a result of an older rebuild is still the best there is until the newest one lands
    let complete = done.serial == st.serial && st.submitted_complete;
    if done.backtest.is_some() {
        st.backtest = done.backtest;
        if let Some(bt) = &st.backtest {
            diag::log(&format!("[model] {}", report::backtest_line(bt)));
        }
    }
    let meta = done.meta;

    let shares = [cfg.s_percent, cfg.a_percent, cfg.b_percent, cfg.c_percent];
    let tiers = meta::tiers(&meta, cfg.min_games, shares);
    let mut plan = BTreeMap::new();
    for c in &meta.champions {
        match tiers.get(&c.name) {
            Some(tier) => {
                plan.insert(c.name.clone(), tier.as_str().to_string());
            }
            None if cfg.clear_unranked && st.champions.contains(&c.name) => {
                plan.insert(c.name.clone(), Tier::NoTier.as_str().to_string());
            }
            None => {}
        }
    }
    // nothing ranked yet (a new save): leave the team's list alone
    if !tiers.is_empty() {
        st.tiers.set_plan(team, plan, !cfg.clear_unranked);
    }
    st.plan_complete = complete;

    let note = st.notes.iter().rev().find(|n| n.is_for(&meta.current));
    let summary = format!(
        "patch {} (kept {}): competition {} matches this patch, {} kept; solo rank {}; patch notes {}; \
         ranked {}/{} champions{}",
        meta.current,
        meta.versions.len(),
        meta.current_matches,
        meta.matches,
        meta.solo_matches,
        note.map_or("none".to_string(), |n| {
            let buffs = n.changes.values().filter(|v| **v > 0).count();
            let nerfs = n.changes.values().filter(|v| **v < 0).count();
            format!("{buffs} buffed, {nerfs} nerfed")
        }),
        tiers.len(),
        st.champions.len(),
        tier_counts(&tiers)
    );
    if summary != st.last_summary {
        diag::log(&format!("[rebuild] {summary} (fit {} ms, {} iterations)", done.millis, meta.sweeps));
        if cfg.verbose {
            let mut by: Vec<&meta::Champion> = meta.champions.iter().filter(|c| c.window.games > 0).collect();
            by.sort_by(|a, b| b.strength.partial_cmp(&a.strength).unwrap_or(std::cmp::Ordering::Equal));
            let show = |c: &&meta::Champion| format!("{} {:.1}%", c.name, c.win_rate() * 100.0);
            let top: Vec<String> = by.iter().take(5).map(show).collect();
            let bottom: Vec<String> = by.iter().rev().take(5).map(show).collect();
            diag::log(&format!("[rebuild] strongest: {}; weakest: {}", top.join(", "), bottom.join(", ")));
        }
        st.last_summary = summary;
    }
    let tier_map: HashMap<String, Tier> = tiers.into_iter().collect();
    st.table = Some(report::table(&meta, &tier_map, &st.labels, &st.last_summary, st.backtest.as_ref()));
    if cfg.report {
        st.report = Some(report::html(&meta, &tier_map, &st.labels, st.backtest.as_ref()));
    }
    shared::publish(shared::Snapshot { meta, damage: st.damage.clone() });
}

fn tier_counts(tiers: &BTreeMap<String, Tier>) -> String {
    let mut counts = [0usize; 5];
    for tier in tiers.values() {
        if let Some(slot) = counts.get_mut(*tier as usize) {
            *slot += 1;
        }
    }
    if counts.iter().all(|c| *c == 0) {
        return String::new();
    }
    format!(" (S {} A {} B {} C {} D {})", counts[0], counts[1], counts[2], counts[3], counts[4])
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
        let snapshot = shared::get().expect("model published");
        let wr = |n: &str| snapshot.meta.champion(n).unwrap().win_rate();
        assert!(wr("a") > 0.6 && wr("j") < 0.4, "a {} j {}", wr("a"), wr("j"));
        let a = snapshot.meta.names.get("a").unwrap();
        assert!(crate::draft::value(&snapshot, a, &[], &[], false) > 0.3);
        assert!(crate::draft::value(&snapshot, a, &[], &[], true) > 0.0);
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
    fn the_model_runs_for_the_screens_when_other_mods_do_the_rest() {
        let _serial = crate::tests::serial();
        with_temp_dir();
        crate::compat::set(crate::compat::Others::from_mods_json(
            r#"{"enabled_mods":["drafters_toolkit","bows_terminator_draft"]}"#,
        ));
        let mut g = save();
        frames(&mut g, 200, Instant::now());
        assert!(g.sent.is_empty(), "the tier list is the Toolbox's");
        assert!(shared::get().is_some(), "the overlay and the report still have a model");
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
        assert!(shared::get().is_some(), "rebuilt from the new save");
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
