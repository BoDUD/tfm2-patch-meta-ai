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
use crate::server::{BalanceRequest, NoteRequest, Request, APPLY_TIERS, FIELD, RESULT_EVENT};
use crate::worker::{Job, Worker};
use crate::{diag, meta, shared};

/// Everything the client half needs from the game (the client context, or a test double).
pub trait Game: Source {
    fn player_team(&mut self) -> Option<usize>;
    fn team_name(&mut self, team: usize) -> Option<String>;
    fn champion_names(&mut self) -> Vec<String>;
    /// How many champions the game lists, and the name of one - one question to the game each (`champion_names` asks
    /// 2 x champions + 1 in one call, and a question about the list takes tens of milliseconds on a save with hundreds
    /// of champions: `champions`). None: this host is not asked that way.
    fn champion_count(&mut self) -> Option<usize> {
        None
    }
    fn champion_name_at(&mut self, _index: usize) -> Option<String> {
        None
    }
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
    /// The champion's category and stat block.
    fn champion_stats(&mut self, _name: &str) -> Option<crate::stats::Stats> {
        None
    }
    /// Both of the above from one question to the game.
    fn champion_data(&mut self, name: &str) -> (Option<Damage>, Option<crate::stats::Stats>) {
        (self.champion_damage(name), self.champion_stats(name))
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
    fn champion_count(&mut self) -> Option<usize> {
        crate::champions::count(self.ctx)
    }
    fn champion_name_at(&mut self, index: usize) -> Option<String> {
        crate::champions::name_at(self.ctx, index)
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
    fn champion_stats(&mut self, name: &str) -> Option<crate::stats::Stats> {
        let brief = self.ctx.champion_brief(name)?;
        let category = brief.category.map(|c| c.code() as u8);
        Some(crate::stats::Stats::from_game(category, &brief.stat, &brief.growth))
    }
    fn champion_data(&mut self, name: &str) -> (Option<Damage>, Option<crate::stats::Stats>) {
        use mod_api_stable::ChampionTagV1;
        let Some(brief) = self.ctx.champion_brief(name) else { return (None, None) };
        let (ad, ap) = (brief.tags.contains(&ChampionTagV1::Ad), brief.tags.contains(&ChampionTagV1::Ap));
        let damage = match (ad, ap) {
            (true, false) => Damage::Physical,
            (false, true) => Damage::Magic,
            _ => Damage::Mixed,
        };
        let category = brief.category.map(|c| c.code() as u8);
        (Some(damage), Some(crate::stats::Stats::from_game(category, &brief.stat, &brief.growth)))
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
        crate::perf::time(crate::perf::Part::Client, || tick(&mut ClientGame { ctx }, in_game, Instant::now()));
        if in_game {
            let scene = ctx.client_scene_kind();
            crate::ui::tick(ctx, scene, &config::get());
            crate::perf::end_frame(&format!("{scene:?}"));
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
/// meta_table.txt and meta_report.html are rewritten at most this often.
const FILES_EVERY: Duration = Duration::from_secs(30);
const NEWS_EVERY: Duration = Duration::from_secs(300);
/// With the balance patches on, the end of the news list is looked at this often: a new item (the game's patch notes
/// on the day it patches, a round's own notes) has the list read again.
const NEWS_PROBE_EVERY: Duration = Duration::from_secs(1);
/// Records read per save at most (newest first); older ones add little to the current patch.
const BACKLOG_COMPETITION: u32 = 3000;
const BACKLOG_SOLO: u32 = 2000;
const REPLY_TIMEOUT: Duration = Duration::from_secs(20);
/// Average reading time per frame. A record that takes longer is paid back over the next
/// frames (no reading until then), so a heavy replay costs one short hitch, not a slow game.
const FRAME_BUDGET: Duration = Duration::from_micros(1000);

/// The reading budget per frame: [`FRAME_BUDGET`], but tests that count frames get more (a
/// loaded test machine must not make them read fewer records).
static BUDGET_MICROS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(FRAME_BUDGET.as_micros() as u64);

fn frame_budget() -> Duration {
    Duration::from_micros(BUDGET_MICROS.load(std::sync::atomic::Ordering::Relaxed))
}
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
    /// The newest fit's files (table, report) are waiting to be written.
    files: Option<Files>,
    files_written_at: Option<Instant>,
    last_summary: String,
    /// The tier plan was built after every listed record had been read.
    plan_complete: bool,
    tiers: TierSync,
    /// The balance patches' state on the server (2.8.0).
    balance: BalanceSync,
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
    /// Every champion's stats (read with the champion list).
    stats: Arc<HashMap<String, crate::stats::Stats>>,
    champions: Vec<String>,
    /// The game's champions while they are being read, a little a frame (`read_champions`); since when the save
    /// has had no team name yet; and the file the lists are kept in.
    champion_read: Option<ChampionRead>,
    nameless_since: Option<Instant>,
    champions_file: std::path::PathBuf,
    /// When this state began (a save opened), and when the game is next asked for champions it did not list.
    opened: Instant,
    champions_retry_at: Option<Instant>,
    /// The save's own id (kept in the save, `server::save_id`), once the server said it: its history file's name.
    save_id: Option<String>,
    /// This save's matches from earlier sessions (`cache`), until the save's own records confirm
    /// the file is this save's; whether that was settled; when the file was last written and
    /// with how many matches.
    cache_pending: Option<Vec<(u64, Match)>>,
    cache_checked: bool,
    cache_written: Option<(Instant, usize)>,
    /// Athletes whose records are to be read (a couple per frame), and the ones already tried.
    athlete_queue: std::collections::VecDeque<u32>,
    athletes_tried: std::collections::HashSet<u32>,
    /// Team names by id (asked once each).
    team_names: HashMap<u32, String>,
    probes_written: bool,
    records_probed: bool,
    /// The player's last starting five (their signature champions get the patch's news cards).
    starting: Vec<u32>,
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
            files: None,
            files_written_at: None,
            last_summary: String::new(),
            plan_complete: false,
            balance: BalanceSync::default(),
            tiers: TierSync::default(),
            names: Names::default(),
            worker: None,
            serial: 0,
            submitted_complete: false,
            backtest: None,
            backtest_done: false,
            labels: Labels::default(),
            damage: HashMap::new(),
            stats: Arc::new(HashMap::new()),
            champions: Vec::new(),
            champion_read: None,
            nameless_since: None,
            champions_file: crate::champions::file(),
            opened: Instant::now(),
            champions_retry_at: None,
            save_id: None,
            cache_pending: None,
            cache_checked: false,
            cache_written: None,
            athlete_queue: std::collections::VecDeque::new(),
            athletes_tried: std::collections::HashSet::new(),
            team_names: HashMap::new(),
            probes_written: false,
            records_probed: false,
            starting: Vec::new(),
        }
    }
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn lock() -> MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The game's champions being read: their names (the list kept from an earlier session, or the game's own as it
/// is asked name by name), how far the reading of their data is, and what was read.
struct ChampionRead {
    names: Vec<String>,
    /// The game's count, when it said one.
    count: Option<usize>,
    /// The next name to ask the game for (None: the names are all there).
    listing: Option<usize>,
    /// The names are the list kept from an earlier session, and whether the game's count and its first and last
    /// name agreed with it.
    kept: bool,
    agreed: bool,
    /// The game's own list is asked behind a list already in use (the kept one, which did not agree).
    behind: bool,
    next: usize,
    /// Names the game had no data for.
    lacking: Vec<String>,
    stats: HashMap<String, crate::stats::Stats>,
    /// What the game's answers took: about the list, about the champions.
    list_time: Duration,
    data_time: Duration,
}

impl ChampionRead {
    fn new(names: Vec<String>, count: Option<usize>, listing: Option<usize>, list_time: Duration) -> Self {
        Self {
            names,
            count,
            listing,
            kept: false,
            agreed: false,
            behind: false,
            next: 0,
            lacking: Vec::new(),
            stats: HashMap::new(),
            list_time,
            data_time: Duration::ZERO,
        }
    }

    /// The game's own list, asked name by name.
    fn listing(count: usize, behind: bool, list_time: Duration) -> Self {
        Self { behind, ..Self::new(Vec::with_capacity(count), Some(count), Some(0), list_time) }
    }
}

/// A frame's time for reading champions (one champion at least): they are read once, when a save opens, and the
/// model waits for them.
const CHAMPION_BUDGET: Duration = Duration::from_millis(8);
/// How long the champions wait for the save's team name (it comes a moment after the team, and the state starts
/// over with it).
const NAME_WAIT: Duration = Duration::from_secs(5);

fn millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Where the champions come from when a save opens (2.8.3; what each way costs is in `diag.log`). The game is asked
/// how many it lists - one question. The list kept from an earlier session of this save is taken, so the model has
/// its champions at once; when the game's count or its first or last name are not the list's (a mod added a
/// champion), the game's own names are asked behind it, one a frame. A save without a kept list waits for the
/// game's names. A host that is not asked that way lists them all at once, as before. None: the game lists none
/// (`rebuild` rates the ones seen in matches).
fn start_champions(st: &State, game: &mut impl Game) -> Option<ChampionRead> {
    let asked = Instant::now();
    let Some(count) = game.champion_count() else {
        let names = game.champion_names();
        let took = asked.elapsed();
        if !names.is_empty() {
            diag::log(&format!("[probe] champion list: {} names, all asked at once ({:.0} ms)", names.len(), millis(took)));
        }
        return (!names.is_empty()).then(|| ChampionRead::new(names, None, None, took));
    };
    let counted = asked.elapsed();
    if count == 0 {
        return None;
    }
    let save = st.save.as_ref().map(|(team, name)| crate::cache::stem(*team, name)).unwrap_or_default();
    let kept = crate::champions::load(&st.champions_file, &save);
    // the kept list is the game's when it has as many names and the game's first and last are its own (two
    // questions more)
    let agreed = kept.as_ref().is_some_and(|names| {
        names.len() == count && [0, count - 1].iter().all(|i| game.champion_name_at(*i).as_ref() == names.get(*i))
    });
    let took = asked.elapsed();
    diag::log(&format!(
        "[probe] champion list: the game counts {count} (that question took {:.0} ms, {:.0} ms with the look at the kept list); {}",
        millis(counted),
        millis(took),
        match (&kept, agreed) {
            (Some(_), true) => "the list kept from an earlier session is taken".to_string(),
            (Some(names), false) => format!("the kept list ({} names) is taken for now, the game's names are asked behind it", names.len()),
            (None, _) => "its names are asked one a frame".to_string(),
        }
    ));
    Some(match kept {
        Some(names) => ChampionRead { kept: true, agreed, ..ChampionRead::new(names, Some(count), None, took) },
        None => ChampionRead::listing(count, false, took),
    })
}

/// The champions' names, damage types and stats when a save opens, a little a frame (2.8.2 spread the stats; 2.8.3
/// the list itself, which was the slow part - `champions`). True while the model has to wait for them.
fn read_champions(st: &mut State, game: &mut impl Game, team: usize) -> bool {
    let first = st.champions.is_empty();
    if !first && st.champion_read.is_none() {
        return false;
    }
    // the save's name comes a moment after its team, and the state starts over with it: not before
    if st.champion_read.is_none() && st.save.as_ref().is_some_and(|(_, name)| name.is_empty()) {
        let since = *st.nameless_since.get_or_insert_with(Instant::now);
        if since.elapsed() < NAME_WAIT {
            return true;
        }
    }
    let mut read = match st.champion_read.take() {
        Some(read) => read,
        None => {
            // the game lists none (yet): asked again a second later, not every frame
            if !due(st.champions_retry_at, Instant::now()) {
                return false;
            }
            match start_champions(st, game) {
                Some(read) => read,
                None => {
                    st.champions_retry_at = Some(Instant::now() + Duration::from_secs(1));
                    return false;
                }
            }
        }
    };
    let frame = Instant::now();
    // the game's own list: one name a frame (a question about the list is the slow one)
    if let (Some(at), Some(count)) = (read.listing, read.count) {
        if at < count {
            let asked = Instant::now();
            if let Some(name) = game.champion_name_at(at) {
                read.names.push(name);
            }
            read.list_time += asked.elapsed();
            read.listing = Some(at + 1);
        }
        if read.listing.is_some_and(|at| at >= count) {
            read.listing = None;
            if read.names.len() < count {
                // a name the game did not give this way: its list all at once, as the SDK asks it
                let asked = Instant::now();
                let names = game.champion_names();
                read.list_time += asked.elapsed();
                diag::log(&format!(
                    "[probe] champion list: {} of {count} names came one by one; all asked at once ({} names, {:.0} ms)",
                    read.names.len(),
                    names.len(),
                    millis(asked.elapsed())
                ));
                if names.len() > read.names.len() {
                    read = ChampionRead { behind: read.behind, ..ChampionRead::new(names, None, None, read.list_time) };
                }
            }
        }
    }
    while read.next < read.names.len() {
        let name = &read.names[read.next];
        read.next += 1;
        let asked = Instant::now();
        let (damage, stats) = game.champion_data(name);
        let label = game.champion_label(name);
        read.data_time += asked.elapsed();
        if damage.is_none() && stats.is_none() {
            read.lacking.push(name.clone());
        }
        if let Some(d) = damage {
            st.damage.insert(st.names.id(name), d);
        }
        if let Some(s) = stats {
            read.stats.insert(name.clone(), s);
        }
        if let Some(label) = label {
            st.labels.champions.insert(name.clone(), label);
        }
        if frame.elapsed() >= CHAMPION_BUDGET {
            break;
        }
    }
    if read.listing.is_some() || read.next < read.names.len() {
        st.champion_read = Some(read);
        return first;
    }
    use_champions(st, game, team, read);
    false
}

/// A list whose champions were all read: in use from now on. A kept list that did not agree with the game, or has
/// names the game has no data for (while it has for others), is behind the game's: its own names are asked next,
/// behind the kept ones.
fn use_champions(st: &mut State, game: &mut impl Game, team: usize, mut read: ChampionRead) {
    st.labels.team_name = game.team_name(team).unwrap_or_default();
    let save = st.save.as_ref().map(|(team, name)| crate::cache::stem(*team, name));
    if read.kept {
        // names the game knows nothing of any more are left out (unless it has data for none: a host without)
        let gone = if read.lacking.len() < read.names.len() { std::mem::take(&mut read.lacking) } else { Vec::new() };
        read.names.retain(|n| !gone.contains(n));
        diag::log(&format!(
            "[probe] stats read for {} of {} champions of the kept list (the game took {:.0} ms about the list, {:.0} ms about the champions)",
            read.stats.len(),
            read.names.len(),
            millis(read.list_time),
            millis(read.data_time)
        ));
        if let (true, Some(count)) = (!read.agreed || !gone.is_empty(), read.count) {
            diag::log(&format!(
                "[probe] champion list: the kept list is behind the game's ({} of its names gone); the game's names are asked one a frame",
                gone.len()
            ));
            st.champion_read = Some(ChampionRead::listing(count, true, Duration::ZERO));
        }
    } else {
        diag::log(&format!(
            "[probe] stats read for {} of {} champions (the game took {:.0} ms about the list, {:.0} ms about the champions)",
            read.stats.len(),
            read.names.len(),
            millis(read.list_time),
            millis(read.data_time)
        ));
        // the game's own list, asked name by name: kept for the next session of this save
        if let (Some(_), Some(save)) = (read.count, &save) {
            if let Some(before) = crate::champions::load(&st.champions_file, save) {
                let (added, gone) = crate::champions::difference(&before, &read.names);
                let some = |list: &[String]| list.iter().take(6).cloned().collect::<Vec<_>>().join(", ");
                diag::log(&format!(
                    "[probe] champion list: {} new since the kept one ({}), {} gone ({})",
                    added.len(),
                    some(&added),
                    gone.len(),
                    some(&gone)
                ));
            }
            crate::champions::store(&st.champions_file, save, &read.names);
        }
        if read.behind && read.names == st.champions {
            // the list in use was the game's after all
            return;
        }
    }
    st.stats = Arc::new(read.stats);
    st.champions = read.names;
    st.dirty = true;
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
            crate::balance::close();
        }
        return;
    }
    st.out_of_game = false;
    config::refresh(now);
    let cfg = config::get();
    // the model is needed by any of the features, not only the two that change the game
    if !cfg.ban_pick_on() && !cfg.tier_list_on() && !cfg.draft_overlay && !cfg.report && !cfg.balance {
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
    // athletes' records, a couple per frame: on the management and line-up screens (the line-up
    // screen asks for the squad it shows), never around the match itself
    if matches!(scene, None | Some(ClientSceneKindV1::Main) | Some(ClientSceneKindV1::Lineup)) && st.save.is_some() {
        read_athletes(st, game);
    }
    if idle_scene(scene) {
        // a match begun right after a save was opened: the first model is carried on with on the line-up screens,
        // and a fit that ends on any screen is published - the draft that follows needs it
        if let Some((team, _)) = st.save.clone() {
            let before_match = matches!(scene, Some(ClientSceneKindV1::Lineup) | Some(ClientSceneKindV1::StadiumEntrance));
            if before_match && shared::get().is_none() {
                first_model(st, game, cfg, team, now);
            }
            collect(st, cfg, team);
        }
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
            crate::ui::forget_save();
            diag::reset_once();
            diag::log(&format!("save open: team #{} {:?}", identity.0, identity.1));
            shared::set_player_team(Some(identity.0));
            // this save's matches from earlier sessions, if any (checked against the save later)
            let file = cache_on().then(|| crate::cache::path(identity.0, &identity.1));
            if let Some(Ok(text)) = file.map(std::fs::read_to_string) {
                let games = crate::cache::decode(&text, &mut st.names);
                if !games.is_empty() {
                    diag::log(&format!("[cache] {} matches from earlier sessions, to be checked against this save", games.len()));
                    st.cache_pending = Some(games);
                }
            }
            // this save's balance patches: its book is opened once the server said which it is (`sync_balance`)
            crate::balance::close();
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
        // a save may list no match for a while after it opens (no model until it does: 2.8.2's log had a draft
        // "with no model ready" a minute after a load): when its first ones come is said once
        if st.comp.listed + st.solo.listed > 0 {
            diag::log_once(
                "listed-some",
                &format!(
                    "records listed: {} competition, {} solo rank - the first {:.0} s after the save opened",
                    st.comp.listed,
                    st.solo.listed,
                    st.opened.elapsed().as_secs_f32()
                ),
            );
        }
    }

    read_records(st, game, cfg);
    settle_cache(st);
    st.comp.cap_backlog(BACKLOG_COMPETITION);
    st.solo.cap_backlog(BACKLOG_SOLO);
    report_probes(st);

    let caught_up = st.comp.pending() == 0 && st.solo.pending() == 0;
    let config_changed = st.rebuilt_with.as_ref().is_none_or(|c| !Arc::ptr_eq(c, cfg));
    // the champions' data first, a few a frame: the model is fitted with their stats
    let reading = read_champions(st, game, team);
    if (st.dirty || config_changed) && due(st.next_rebuild, now) && !reading {
        st.next_rebuild = Some(now + REBUILD_EVERY);
        st.dirty = false;
        st.rebuilt_with = Some(Arc::clone(cfg));
        if !rebuild(st, game, cfg, team, now, caught_up) {
            // nothing to fit yet: the first matches (or the kept history) are not waited another five seconds for
            st.next_rebuild = None;
        }
    }
    collect(st, cfg, team);
    save_cache(st, team, now, caught_up);

    // the files are built and written on a thread of their own: a big report is a few
    // milliseconds of formatting the frame need not wait for
    if st.files.is_some() && due(st.files_written_at.map(|t| t + FILES_EVERY), now) {
        st.files_written_at = Some(now);
        if let (Some(files), Some(snapshot)) = (st.files.take(), shared::get()) {
            let write = move || files.write(&snapshot);
            if std::thread::Builder::new().name(format!("{}-files", crate::MOD_ID)).spawn(write).is_err() {
                diag::log_once("files-thread", "could not start the thread that writes meta_table.txt / meta_report.html");
            }
        }
    }

    // the tier list goes out once it was built from the whole backlog, not after every
    // partial rebuild while the save is still being read
    let events = game.take_events();
    if cfg.tier_list_on() {
        let complete = st.plan_complete;
        sync_tiers(st, game, team, now, complete, &events);
    }
    sync_balance(st, game, cfg, team, now, &events);
}

/// The save's first model where the management screens were left before it was there: the champions read on, the
/// fit begun (`run` does the same on its own screens).
fn first_model(st: &mut State, game: &mut impl Game, cfg: &Arc<Config>, team: usize, now: Instant) {
    let reading = read_champions(st, game, team);
    if st.dirty && !reading && due(st.next_rebuild, now) {
        st.next_rebuild = Some(now + REBUILD_EVERY);
        st.dirty = false;
        st.rebuilt_with = Some(Arc::clone(cfg));
        let caught_up = st.comp.pending() == 0 && st.solo.pending() == 0;
        if !rebuild(st, game, cfg, team, now, caught_up) {
            st.next_rebuild = None;
        }
    }
}

fn read_records(st: &mut State, game: &mut impl Game, cfg: &Config) {
    if st.debt > Duration::ZERO {
        st.debt = st.debt.saturating_sub(frame_budget());
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
        if frame.elapsed() >= frame_budget() {
            break;
        }
    }
    st.debt = frame.elapsed().saturating_sub(frame_budget()).min(Duration::from_secs(2));
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

/// Merges the matches of earlier sessions once the save's first matches confirm the file.
fn settle_cache(st: &mut State) {
    if st.cache_checked {
        return;
    }
    let keys: Vec<u64> = st.comp.first_keys.iter().chain(&st.solo.first_keys).copied().collect();
    let caught_up = st.comp.listed > 0 && st.comp.pending() == 0 && st.solo.pending() == 0;
    if keys.len() < crate::cache::CONFIRM_MATCHES && !caught_up {
        return;
    }
    st.cache_checked = true;
    let Some(pending) = st.cache_pending.take() else { return };
    let file_keys: std::collections::HashSet<u64> = pending.iter().map(|(k, _)| *k).collect();
    if !crate::cache::confirms(&file_keys, &keys) {
        diag::log("[cache] the history file is not this save's (or the save has too few matches yet): not used");
        return;
    }
    let mut merged = 0;
    for (key, game) in pending {
        let scanner = if game.solo { &mut st.solo } else { &mut st.comp };
        merged += scanner.adopt(key, game, &st.names) as usize;
    }
    diag::log(&format!("[cache] {merged} matches from earlier sessions added (the save no longer lists them)"));
    if merged > 0 {
        st.dirty = true;
    }
}

/// The save's matches from earlier sessions kept under its own id (2.8.3): the save's for sure, so they are taken
/// at once - a save just opened may list no match of its own for a while, and a match played right then was
/// drafted with no model (`the AI drafted with no model ready`). The file named by team and name (2.8.2 and
/// before), still waiting for the save's own matches to confirm it, is not needed then.
fn adopt_history(st: &mut State) {
    let Some(id) = st.save_id.clone() else { return };
    if !cache_on() {
        return;
    }
    let Ok(text) = std::fs::read_to_string(crate::cache::path_of(&id)) else { return };
    let mut merged = 0;
    for (key, game) in crate::cache::decode(&text, &mut st.names) {
        let scanner = if game.solo { &mut st.solo } else { &mut st.comp };
        merged += scanner.adopt(key, game, &st.names) as usize;
    }
    st.cache_pending = None;
    st.cache_checked = true;
    diag::log(&format!("[cache] {merged} matches of this save from earlier sessions (kept under its id {id}): taken at once"));
    if merged > 0 {
        st.dirty = true;
    }
}

/// The history file is rewritten at most this often.
const CACHE_EVERY: Duration = Duration::from_secs(120);

/// History files are read and written (tests switch them off, but for the one about them: a
/// file one test's thread writes late must not reach the next test's save).
static CACHE_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

fn cache_on() -> bool {
    CACHE_ON.load(std::sync::atomic::Ordering::Relaxed)
}

/// Writes this save's matches to its history file (on a thread of its own), once everything is
/// read and the old file was checked - never a half-read save over a full history.
fn save_cache(st: &mut State, team: usize, now: Instant, caught_up: bool) {
    if !st.cache_checked || !caught_up || !cache_on() {
        return;
    }
    let count = st.comp.games.len() + st.solo.games.len();
    if count == 0 || st.cache_written.is_some_and(|(at, n)| n == count || now < at + CACHE_EVERY) {
        return;
    }
    st.cache_written = Some((now, count));
    // under the save's own id when it has one (taken at once next time), else by its team and name
    let path = match &st.save_id {
        Some(id) => crate::cache::path_of(id),
        None => crate::cache::path(team, st.save.as_ref().map_or("", |s| s.1.as_str())),
    };
    let mut games: Vec<(u64, Arc<Match>)> = st.comp.games.iter().chain(&st.solo.games).map(|(k, g)| (*k, g.clone())).collect();
    // oldest first, so the file keeps the newest when it is trimmed
    // matches kept from earlier sessions have no record id (0): their patch orders them
    games.sort_by(|(_, a), (_, b)| a.record.cmp(&b.record).then_with(|| crate::records::compare_versions(&a.version, &b.version)));
    let names = st.names.clone();
    let write = move || {
        let text = crate::cache::encode(games.iter().map(|(k, g)| (k, &**g)), &names);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    };
    if std::thread::Builder::new().name(format!("{}-history", crate::MOD_ID)).spawn(write).is_err() {
        diag::log_once("cache-thread", "could not start the thread that writes the history file");
    }
}

/// Athletes per frame whose records are read (stat, proficiency: a few hundred numbers each).
const ATHLETES_PER_FRAME: usize = 2;

/// Reads the records of athletes the screens asked for, a couple per frame.
fn read_athletes(st: &mut State, game: &mut impl Game) {
    for id in crate::ui::take_wanted() {
        if st.athletes_tried.insert(id) {
            st.athlete_queue.push_back(id);
        }
    }
    for _ in 0..ATHLETES_PER_FRAME {
        let Some(id) = st.athlete_queue.pop_front() else { return };
        let read = |path: &str, game: &mut dyn FnMut(&str) -> Option<String>| -> Value {
            game(path).and_then(|j| serde_json::from_str(&j).ok()).unwrap_or(Value::Null)
        };
        let mut get = |path: &str| game.record_json(RecordKindV1::Athlete, id as usize, path);
        let stat = read("stat", &mut get);
        let proficiency = read("champion_proficiency", &mut get);
        if stat.is_null() && proficiency.is_null() {
            diag::log_once("athlete-unreadable", &format!("[probe] athlete #{id}: no stat / champion_proficiency in its record"));
            continue;
        }
        let name = game.athlete_name(id).unwrap_or_else(|| format!("#{id}"));
        st.labels.athletes.entry(id).or_insert_with(|| name.clone());
        crate::ui::learn_athlete(id, crate::plan::AthleteInfo::from_record(name, &stat, &proficiency));
    }
}

/// What the screens know about every team, by lower-case team name.
#[derive(Default)]
struct Teams {
    rosters: HashMap<String, Vec<(u32, Option<crate::history::Role>)>>,
    labels: HashMap<String, String>,
    picks: HashMap<String, Vec<(String, u32, u32)>>,
    /// Matches the `picks` lists count, per team.
    matches: HashMap<String, u32>,
    last_opponent: Option<String>,
    /// Per team id: the share of its recent matches it played each champion in (`ai_bp`).
    habits: HashMap<u32, HashMap<u16, f32>>,
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
        let key = crate::ui::team_key(&label);
        if key.is_empty() {
            continue;
        }
        let (_, newest, side) = list[0];
        let players: Vec<(u32, Option<crate::history::Role>)> =
            newest.sides[side].iter().filter_map(|s| Some((s.athlete?, s.role))).collect();
        wanted_athletes.extend(players.iter().map(|(a, _)| *a));
        if team as usize == own {
            let other = newest.teams[1 - side];
            out.last_opponent = other.and_then(|o| st.team_names.get(&o)).map(|n| crate::ui::team_key(n));
        }
        let mut counts: HashMap<u16, (u32, u32)> = HashMap::new();
        for (_, g, side) in list.iter().take(RECENT_PER_TEAM) {
            for s in &g.sides[*side] {
                let e = counts.entry(s.champ).or_default();
                e.0 += 1;
                e.1 += g.won(*side) as u32;
            }
        }
        let recent = list.len().clamp(1, RECENT_PER_TEAM) as f32;
        out.habits.insert(team, counts.iter().map(|(c, (g, _))| (*c, *g as f32 / recent)).collect());
        let mut picks: Vec<(String, u32, u32)> =
            counts.into_iter().map(|(c, (g, w))| (st.names.name(c).to_string(), g, w)).collect();
        picks.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
        picks.truncate(12);
        if !players.is_empty() {
            out.rosters.insert(key.clone(), players);
        }
        out.picks.insert(key.clone(), picks);
        out.matches.insert(key.clone(), list.len().min(RECENT_PER_TEAM) as u32);
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

/// Reads the team's news: the patch notes (the model's, the balance patches' game patch), how many items it has and
/// its last one. True when the patch notes changed.
fn read_news(st: &mut State, game: &mut impl Game, team: usize) -> bool {
    match game.record_json(RecordKindV1::Team, team, "news") {
        Some(json) => match patchnotes::parse_all(&json) {
            Ok(news) => {
                diag::log_once(
                    "probe-news",
                    &format!(
                        "[probe] news: {} items, {} patch notes; latest {:?}",
                        news.len,
                        news.notes.len(),
                        news.notes.last().map(|n| (&n.versions, n.changes.len()))
                    ),
                );
                let patch = news.newest_version();
                if patch.is_some() && patch != st.balance.patch && st.balance.patch.is_some() {
                    diag::log(&format!("[balance] the game's newest patch notes: {}", patch.as_deref().unwrap_or("")));
                }
                st.balance.patch = patch;
                st.balance.news_len = Some(news.len);
                st.balance.news_tail = news.len.checked_sub(1).and_then(|i| game.record_json(RecordKindV1::Team, team, &format!("news.{i}")));
                let changed = st.notes != news.notes;
                st.notes = news.notes;
                return changed;
            }
            Err(why) => diag::log_once("probe-news", &format!("[probe] news unusable: {why}")),
        },
        None => diag::log_once("probe-news", "[probe] the team record has no readable news"),
    }
    false
}

/// New news items at most read one by one; more than that and the whole list is read.
const NEW_ITEMS_MAX: usize = 32;

/// Looks at the news list's end: its last item as it was (else the whole list is read again) and the items after it,
/// one by one (a whole list is megabytes of a two-year save; a day adds a few items). True when the patch notes
/// changed.
fn probe_news(st: &mut State, game: &mut impl Game, team: usize) -> bool {
    let Some(len) = st.balance.news_len else { return read_news(st, game, team) };
    if let Some(last) = len.checked_sub(1) {
        if game.record_json(RecordKindV1::Team, team, &format!("news.{last}")) != st.balance.news_tail {
            return read_news(st, game, team);
        }
    }
    let mut at = len;
    let mut changed = false;
    while let Some(json) = game.record_json(RecordKindV1::Team, team, &format!("news.{at}")) {
        if at >= len + NEW_ITEMS_MAX {
            return read_news(st, game, team);
        }
        if let Some(note) = patchnotes::parse_item(&json) {
            st.notes.push(note);
            changed = true;
        }
        st.balance.news_tail = Some(json);
        at += 1;
    }
    st.balance.news_len = Some(at);
    if changed {
        let patch = st.notes.iter().rev().find_map(|n| n.version.clone());
        if patch.is_some() && patch != st.balance.patch {
            diag::log(&format!("[balance] the game's newest patch notes: {}", patch.as_deref().unwrap_or("")));
        }
        st.balance.patch = patch;
    }
    changed
}

/// True when a model was sent to be fitted (false: no patch known yet - nothing to fit).
fn rebuild(st: &mut State, game: &mut impl Game, cfg: &Config, team: usize, now: Instant, caught_up: bool) -> bool {
    // the patch notes (re-read every 5 minutes; they change once per in-game patch)
    let played_newest = st.comp.current_version().or_else(|| st.solo.current_version());
    if due(st.news_read_at.map(|t| t + NEWS_EVERY), now) || played_newest != st.news_version {
        st.news_read_at = Some(now);
        st.news_version = played_newest;
        read_news(st, game, team);
    }

    let record_current = st.comp.current_version().or_else(|| st.solo.current_version());
    let current = match current_version(&st.notes, record_current.as_deref(), &st.comp.versions) {
        Ok(version) => version,
        Err(mismatch) => {
            diag::log_once("probe-note-versions", &format!("[probe] {mismatch}"));
            record_current.clone()
        }
    };
    // no match known (a save just opened may list none for a while): the patch its notes or its balance book name,
    // so that a model - League's knowledge, the champions' stats - is there for a match played right away
    let current = current.or_else(|| {
        let noted = st.notes.iter().filter_map(|n| n.version.clone()).max_by(|a, b| compare_versions(a, b));
        noted.or_else(|| crate::balance::latest().map(|r| r.version))
    });
    let Some(current) = current else { return false };

    // the game lists no champions (`read_champions` reads its list when it does): the ones seen in matches are rated
    if st.champions.is_empty() {
        let mut champions: Vec<String> = st.comp.versions.values().flat_map(|v| v.champs.keys().cloned()).collect();
        champions.sort();
        champions.dedup();
        let mut stats = HashMap::new();
        for name in &champions {
            let id = st.names.id(name);
            if let Some(d) = game.champion_damage(name) {
                st.damage.insert(id, d);
            }
            if let Some(s) = game.champion_stats(name) {
                stats.insert(name.clone(), s);
            }
            if let Some(label) = game.champion_label(name) {
                st.labels.champions.insert(name.clone(), label);
            }
        }
        st.labels.team_name = game.team_name(team).unwrap_or_default();
        diag::log(&format!("[probe] stats read for {} of {} champions", stats.len(), champions.len()));
        st.stats = Arc::new(stats);
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
    shared::set_habits(teams.habits.clone());
    // the squad: every athlete of the player's team in the kept matches, and the last starting
    // five; their records are read in the background
    let mut squad: Vec<u32> = Vec::new();
    for g in st.comp.games.values() {
        for (side, t) in g.teams.iter().enumerate() {
            if *t == Some(team as u32) {
                squad.extend(g.sides[side].iter().filter_map(|s| s.athlete));
            }
        }
    }
    if let Some(Value::Array(last)) = game.record_json(RecordKindV1::Team, team, "last_starting").and_then(|j| serde_json::from_str(&j).ok()) {
        st.starting = last.iter().filter_map(|v| v.as_u64()).map(|v| v as u32).collect();
        squad.extend(st.starting.iter().copied());
    }
    squad.sort_unstable();
    squad.dedup();
    crate::ui::want_athletes(&squad);
    let current_strategy = game
        .record_json(RecordKindV1::Team, team, "strategy")
        .and_then(|j| serde_json::from_str::<Value>(&j).ok())
        .map(|v| crate::records::strategy(&v))
        .unwrap_or_default();
    crate::ui::set_context(crate::ui::Context {
        team_name: st.labels.team_name.clone(),
        champions: st.champions.clone(),
        english: st.labels.champions.iter().map(|(id, label)| (label.clone(), id.clone())).collect(),
        rosters: teams.rosters,
        team_labels: teams.labels,
        team_picks: teams.picks,
        team_matches: teams.matches,
        athletes: st.labels.athletes.clone(),
        last_opponent: teams.last_opponent,
        backtest: st.backtest,
        squad,
        current_strategy,
    });
    if !st.records_probed {
        st.records_probed = true;
        probe_records(game, team);
    }

    // pointers only: the worker copies them (a copy of every game here took milliseconds)
    let mut games: Vec<Arc<Match>> = st.comp.games.values().cloned().collect();
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
        stats: st.stats.clone(),
        notes: model_notes(&st.notes),
        current,
        settings: cfg.model(),
        backtest,
    };
    let threaded = game.threaded();
    st.worker.get_or_insert_with(|| Worker::new(threaded)).submit(job);
    true
}

/// The patch notes the model learns from, one note a patch: the game's own from the news, and this save's balance
/// rounds from its book - their lines in the news (a hotfix's own notes, the lines merged into the game's) left out
/// so a round counts once. A save with no book keeps the news as it is.
fn model_notes(news: &[PatchNote]) -> Vec<PatchNote> {
    let rounds = crate::balance::notes();
    let mut notes: Vec<PatchNote> = if rounds.is_empty() {
        news.to_vec()
    } else {
        news.iter().filter(|n| !n.ours).map(PatchNote::game_only).collect()
    };
    notes.extend(rounds);
    patchnotes::combine(notes)
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

    let notes = model_notes(&st.notes);
    let note = notes.iter().rev().find(|n| n.is_for(&meta.current));
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
    st.files = Some(Files {
        tiers: tiers.into_iter().collect(),
        labels: st.labels.clone(),
        summary: st.last_summary.clone(),
        backtest: st.backtest,
        html: cfg.report,
    });
    let mut snapshot = shared::Snapshot::new(meta, st.damage.clone());
    snapshot.athlete_names = st.labels.athletes.clone();
    snapshot.team_names = st.team_names.clone();
    shared::publish(snapshot);
}

/// What the report files are built from, besides the published model.
struct Files {
    tiers: HashMap<String, Tier>,
    labels: Labels,
    summary: String,
    backtest: Option<Backtest>,
    html: bool,
}

impl Files {
    fn write(&self, snapshot: &shared::Snapshot) {
        let meta = &snapshot.meta;
        diag::write_table(&report::table(meta, &self.tiers, &self.labels, &self.summary, self.backtest.as_ref()));
        if self.html {
            diag::write_file(diag::REPORT_FILE, &report::html(meta, &self.tiers, &self.labels, self.backtest.as_ref()));
        }
    }
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

fn sync_tiers(st: &mut State, game: &mut impl Game, team: usize, now: Instant, plan_complete: bool, events: &[(String, Vec<u8>)]) {
    let sync = &mut st.tiers;
    for (event, payload) in events {
        if event != RESULT_EVENT {
            continue;
        }
        let text = String::from_utf8_lossy(payload);
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

/// The balance patches on the server (2.8.0): what it confirmed (the game's champion patches off, the newest round
/// written), the request waiting for its answer, retries; and what the news says (its length, the game's patch).
#[derive(Default)]
struct BalanceSync {
    acked: Option<(bool, u32)>,
    waiting: Option<(bool, u32, Instant)>,
    failures: u32,
    retry_at: Option<Instant>,
    /// The news list's length and last item when it was last read, when its end is looked at next, and the game's
    /// newest patch (its newest patch notes' version).
    news_len: Option<usize>,
    news_tail: Option<String>,
    probe_at: Option<Instant>,
    patch: Option<String>,
    /// The round made ahead for the game's next patch as last handed to the server (2.8.1), what it was made from (the
    /// model's snapshot, the game day), and the next patch day in the schedule as of a day.
    prepared: Option<String>,
    prepared_from: Option<(usize, Date)>,
    next_patch: Option<(Date, Option<Date>)>,
    /// The last request (`off`, round): its answer counts after the wait for it was given up, too (2.8.2 - a save
    /// being opened stalls the game for tens of seconds, and the answer that came after them was thrown away).
    sent: Option<(bool, u32)>,
    /// The save's book (2.8.2): when the server was last asked which it is, and its id once it said ("": the save has
    /// none; `book_wanted`: it was asked to make one). Nothing of the balance happens before.
    book_asked: Option<Instant>,
    book_wanted: bool,
    book: Option<String>,
}

type Date = (i32, u32, u32);

/// How long the server is given to say which book the save has before it is asked again.
const BOOK_RETRY: Duration = Duration::from_secs(5);

/// Game days before the game's scheduled patch that its round is made ahead and handed to the server.
const PREPARE_DAYS: i64 = 2;
/// The year schedule's events that are game patches.
const PATCH_EVENTS: [&str; 3] = ["MinorPatch", "MajorPatch", "SeasonPatch"];

/// The first day on or after `today` with a game patch in the year schedules (`[{"schedules": [["2027-10-03",
/// "MinorPatch"], ...]}, ...]`; an event may be a name or a one-key object).
fn next_patch_day(schedule: &str, today: Date) -> Option<Date> {
    let doc: Value = serde_json::from_str(schedule).ok()?;
    let years: Vec<&Value> = match &doc {
        Value::Array(years) => years.iter().collect(),
        other => vec![other],
    };
    let from = crate::balance::day_number(today);
    years
        .iter()
        .filter_map(|y| y.get("schedules").and_then(Value::as_array))
        .flatten()
        .filter_map(|entry| {
            let is_patch = match entry.get(1)? {
                Value::String(name) => PATCH_EVENTS.contains(&name.as_str()),
                Value::Object(map) => map.keys().any(|k| PATCH_EVENTS.contains(&k.as_str())),
                _ => false,
            };
            let mut parts = entry.get(0)?.as_str()?.get(..10)?.split('-');
            let day: Date = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
            (is_patch && crate::balance::day_number(day) >= from).then_some(day)
        })
        .min_by_key(|d| crate::balance::day_number(*d))
}

/// The round for the game's next scheduled patch, made ahead while that patch is [`PREPARE_DAYS`] days off or nearer
/// and handed to the server, which writes it in the management tick the game's patch notes appear in (its changes
/// made later reach the screens only with a later day: the inbox showed the game's own empty notes first). Withdrawn
/// when no patch is near.
fn prepare_next(st: &mut State, game: &mut impl Game, team: usize, snapshot: &Arc<shared::Snapshot>, rules: crate::balance::Rules, patch: Option<&str>) {
    let today = game.game_date();
    let next = match (today, st.balance.next_patch) {
        (Some(t), Some((day, next))) if day == t => next,
        (Some(t), _) => {
            let next = game.schedule_json("").and_then(|s| next_patch_day(&s, t));
            st.balance.next_patch = Some((t, next));
            next
        }
        (None, _) => None,
    };
    let near = match (today, next) {
        (Some(t), Some(d)) => crate::balance::day_number(d) - crate::balance::day_number(t) <= PREPARE_DAYS,
        _ => false,
    };
    let (Some(today), Some(day), true) = (today, next, near) else {
        withdraw_prepared(st, game);
        return;
    };
    let from = (Arc::as_ptr(snapshot) as usize, today);
    if st.balance.prepared_from == Some(from) {
        return;
    }
    st.balance.prepared_from = Some(from);
    let round = crate::balance::make(&snapshot.meta, rules, &crate::balance::Due::Patch(crate::server::NEXT_PATCH.into()), Some(day));
    // the patch it is for: the first newer than every patch known (the book's, the news', the matches')
    let mut after = crate::balance::latest().map(|r| r.version).unwrap_or_default();
    if let Some(p) = patch.filter(|p| compare_versions(p, &after) == std::cmp::Ordering::Greater) {
        after = p.to_string();
    }
    let Some(book) = crate::balance::book() else { return };
    let request = balance_request(st, team, &round, rules);
    let payload = crate::server::Prepared { book, after, round, request }.encode();
    if st.balance.prepared.as_deref() != Some(payload.as_str()) && game.send_command(crate::server::PREPARE_BALANCE, payload.as_bytes()) {
        st.balance.prepared = Some(payload);
    }
}

/// The prepared round taken back from the server (no patch near, the setting off).
fn withdraw_prepared(st: &mut State, game: &mut impl Game) {
    st.balance.prepared_from = None;
    if st.balance.prepared.is_some() && game.send_command(crate::server::PREPARE_BALANCE, b"") {
        st.balance.prepared = None;
    }
}

/// The balance patches (`balance`): the news watched for the game's patches, a new round when one is due (from a
/// model built on the whole backlog), and the server told - the game's own champion patches off while this is on
/// (put back when it is off), and each new round's notes and news cards written into the player's news.
fn sync_balance(st: &mut State, game: &mut impl Game, cfg: &Config, team: usize, now: Instant, events: &[(String, Vec<u8>)]) {
    for (event, payload) in events {
        if event == crate::server::PREPARED_WRITTEN {
            // the server wrote the round made ahead into the game's patch: in the book, the news and the screens
            let text = String::from_utf8_lossy(payload);
            let mut parts = text.splitn(3, '\t');
            let (round, ok, detail) = (parts.next().and_then(|r| r.parse::<u32>().ok()), parts.next() == Some("ok"), parts.next().unwrap_or(""));
            diag::log(&format!("[balance] the server: {detail}"));
            st.balance.prepared = None;
            st.balance.prepared_from = None;
            if let (true, Some(round)) = (ok, round) {
                st.balance.acked = Some((true, round));
                st.balance.failures = 0;
            }
            st.balance.probe_at = None;
            st.balance.news_len = None;
            st.dirty = true;
            continue;
        }
        if event == crate::server::BOOK_RESULT {
            // which book the save has, and its own id (`<book>\t<newest round in its news>\t<save>`; no book: none)
            let text = String::from_utf8_lossy(payload);
            let mut parts = text.split('\t');
            let (id, done, save) = (parts.next().unwrap_or(""), parts.next().unwrap_or("0"), parts.next().unwrap_or(""));
            if st.save_id.is_none() && !save.is_empty() {
                st.save_id = Some(save.to_string());
                adopt_history(st);
            }
            if st.balance.book.is_none() {
                if !id.is_empty() {
                    let name = st.save.as_ref().map(|s| s.1.clone()).unwrap_or_default();
                    crate::balance::open_book(id, team, &name, done.trim().parse().unwrap_or(0));
                }
                st.balance.book = Some(id.to_string());
            }
            continue;
        }
        if event != crate::server::BALANCE_RESULT {
            continue;
        }
        let text = String::from_utf8_lossy(payload);
        let mut parts = text.splitn(3, '\t');
        let (state, ok, detail) = (parts.next().unwrap_or(""), parts.next() == Some("ok"), parts.next().unwrap_or(""));
        // the request waited for, or the last one sent: its answer may come after the wait was given up
        let Some((off, round)) = st.balance.waiting.map(|(off, round, _)| (off, round)).or(st.balance.sent) else { continue };
        if state != format!("{}/{round}", u8::from(off)) {
            continue;
        }
        st.balance.waiting = None;
        st.balance.sent = None;
        if ok {
            st.balance.acked = Some((off, round));
            st.balance.failures = 0;
            diag::log(&format!("[balance] the server: {detail}"));
            // the round's notes are in the news now (maybe in the game's own notes): read it again
            st.balance.probe_at = None;
            st.balance.news_len = None;
        } else {
            st.balance.failures += 1;
            st.balance.retry_at = Some(now + backoff(st.balance.failures));
            diag::log(&format!("[balance] the server refused ({detail}); trying again later"));
        }
    }
    if let Some((_, _, at)) = st.balance.waiting {
        if now.duration_since(at) > REPLY_TIMEOUT {
            st.balance.waiting = None;
            st.balance.failures += 1;
            st.balance.retry_at = Some(now + backoff(st.balance.failures));
            diag::log("[balance] no answer from the server; trying again later");
        }
    }
    // the save's book first: the server says which it is (the id is kept in the save)
    match st.balance.book.as_deref() {
        None => {
            let create: &[u8] = if cfg.balance { b"1" } else { b"0" };
            if due(st.balance.book_asked.map(|t| t + BOOK_RETRY), now) && game.send_command(crate::server::BALANCE_BOOK, create) {
                st.balance.book_asked = Some(now);
                st.balance.book_wanted = cfg.balance;
            }
            return;
        }
        // the save has none, and the setting is on (turned on since): asked again, for one to be made
        Some("") if cfg.balance && !st.balance.book_wanted => {
            st.balance.book = None;
            st.balance.book_asked = None;
            return;
        }
        Some("") => {
            if cfg.balance {
                diag::log_once("balance-no-book", "[balance] the save keeps no id for its book: no balance patches on it");
            }
            return;
        }
        Some(_) => {}
    }
    let rules = crate::balance::Rules { count: cfg.balance_count, max_step: cfg.balance_max };
    if cfg.balance {
        // the news' end: a new item (the game's patch notes, a round's own) has the list read again
        if due(st.balance.probe_at, now) {
            st.balance.probe_at = Some(now + NEWS_PROBE_EVERY);
            if probe_news(st, game, team) {
                st.dirty = true;
            }
        }
        // a new round when one is due: the game's new patch, the first one, or a late hotfix - from a model that has
        // matches (one of League's knowledge alone, right after a save opened, would use the patch up on nobody), the
        // kept history taken or turned down
        let settled = st.cache_checked || st.cache_pending.is_none();
        if st.plan_complete && settled {
            if let Some(snapshot) = shared::get().filter(|s| s.meta.matches + s.meta.solo_matches > 0) {
                let today = game.game_date();
                // the game's patch: its newest patch notes, or the newest patch matches were played in when that is newer
                // (a patch the game made without notes)
                let played = (!snapshot.meta.current.is_empty()).then(|| snapshot.meta.current.clone());
                let patch = match (st.balance.patch.clone(), played) {
                    (Some(notes), Some(played)) if compare_versions(&played, &notes) == std::cmp::Ordering::Greater => Some(played),
                    (notes, played) => notes.or(played),
                };
                if let Some(due) = crate::balance::due_now(patch.as_deref(), today) {
                    let round = crate::balance::make(&snapshot.meta, rules, &due, today);
                    let list: Vec<String> = round.changes.iter().map(|c| format!("{} {:+} ({:.1}%)", c.champion, c.step, c.win)).collect();
                    diag::log(&format!(
                        "[balance] round {} {} ({:?}): {}",
                        round.id,
                        round.label,
                        due,
                        if list.is_empty() { "no changes".to_string() } else { list.join(", ") }
                    ));
                    log_held(&round);
                    crate::balance::publish(round);
                    // the model takes the round as the patch's notes; a round made ahead is made again after it
                    st.dirty = true;
                    st.balance.prepared_from = None;
                }
                prepare_next(st, game, team, &snapshot, rules, patch.as_deref());
            }
        }
    } else {
        withdraw_prepared(st, game);
    }
    // the server (a save with a book: the setting was on once). Off with no round yet: the game's champion patches,
    // turned off when it was on, are put back all the same
    let newest = crate::balance::latest();
    let want = match (&newest, cfg.balance) {
        (None, false) => (false, 0),
        // on, no round yet: the game's champion patches off already
        (None, true) => (true, 0),
        (Some(r), true) => (true, r.id),
        (Some(r), false) => (false, st.balance.acked.map_or(r.id, |a| a.1)),
    };
    if st.balance.acked == Some(want) || st.balance.waiting.is_some() || !due(st.balance.retry_at, now) {
        return;
    }
    // a round's lines are the champions' numbers: not before they are read (a few a frame after a save opens)
    if newest.is_some() && st.champions.is_empty() {
        return;
    }
    let req = match (&newest, want.0) {
        (Some(round), true) => balance_request(st, team, round, rules),
        _ => BalanceRequest { team, off: want.0, round: want.1, ..BalanceRequest::default() },
    };
    if game.send_command(crate::server::APPLY_BALANCE, req.encode().as_bytes()) {
        st.balance.waiting = Some((want.0, want.1, now));
        st.balance.sent = Some((want.0, want.1));
    } else {
        st.balance.failures += 1;
        st.balance.retry_at = Some(now + backoff(st.balance.failures));
        diag::log_once("balance-send-failed", "[balance] could not send to the server (will retry)");
    }
}

/// The champions a round left for later (outside the band, too few games with their new numbers), in the log.
fn log_held(round: &crate::balance::Round) {
    if !round.held.is_empty() {
        let held: Vec<String> = round.held.iter().map(|(c, g)| format!("{c} ({g:.0} games)")).collect();
        diag::log(&format!("[balance] round {} leaves for later (too few games since their last change): {}", round.id, held.join(", ")));
    }
}

/// The server's request for a round: its notes (each champion's lines - its numbers as it plays with them, from
/// the game's stats - and reason) and the patch's news cards.
fn balance_request(st: &State, team: usize, round: &crate::balance::Round, rules: crate::balance::Rules) -> BalanceRequest {
    let label = |n: &str| st.labels.champion(n);
    let mut lines = BTreeMap::new();
    let mut champions = Vec::new();
    for c in &round.changes {
        let Some(stats) = st.stats.get(&c.champion) else {
            diag::log_once(&format!("balance-no-stats-{}", c.champion), &format!("[balance] no stats for {}: no lines in the notes", c.champion));
            continue;
        };
        let rows = crate::balance::rows(stats, c.total - c.step, c.total);
        let why = crate::balance::reason(c, rules);
        champions.push((c.champion.clone(), rows.clone(), why.clone()));
        lines.insert(c.champion.clone(), (rows, why));
    }
    let (title, content) = crate::balance::news(round, &label);
    let articles = crate::balance::articles(round, &lines, &st.labels.team_name, &signatures(st), &label);
    let name = if round.label.is_empty() { round.version.clone() } else { round.label.clone() };
    BalanceRequest {
        team,
        off: true,
        round: round.id,
        note: Some(NoteRequest { version: round.version.clone(), label: name, merge: round.merged, champions, title, content }),
        articles,
    }
}

/// The player's starting five (the squad when not known) and each one's signature champion: the one they are most
/// proficient with, from their records.
fn signatures(st: &State) -> Vec<crate::balance::Signature> {
    let infos = crate::ui::athlete_infos();
    st.starting
        .iter()
        .filter_map(|a| {
            let info = infos.get(a)?;
            let (champion, _) = info.proficiency.iter().max_by(|x, y| x.1.cmp(y.1).then_with(|| y.0.cmp(x.0)))?;
            let name = if info.name.is_empty() { st.labels.athletes.get(a).cloned().unwrap_or_default() } else { info.name.clone() };
            Some(crate::balance::Signature { athlete: *a, name, champion: champion.clone() })
        })
        .collect()
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
        /// Fit on a worker thread, as in the game (the frame-timing test).
        threaded: bool,
        /// The save's book as the server says it (id, newest round in its news), the balance commands sent, and how
        /// long the game takes to answer about a champion.
        book: (String, u32),
        balance_sent: Vec<String>,
        champion_delay: Duration,
        champions_asked: u32,
        /// The game answers about its champion list a question at a time (as the real one), and how many names it
        /// was asked for.
        counts: bool,
        names_asked: u32,
        /// It counts its champions but gives no name one by one.
        nameless: bool,
        /// The save's own id as the server says it (none: a server that gives none).
        save_id: String,
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
                // one news item: `news.<index>`
                (RecordKindV1::Team, p) if id == self.team && p.starts_with("news.") => {
                    let at: usize = p["news.".len()..].parse().ok()?;
                    serde_json::from_str::<Value>(&self.news).ok()?.get(at).map(Value::to_string)
                }
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
            if command == server::BALANCE_BOOK {
                // which book the save has (none unless a test gives it one)
                let answer = format!("{}\t{}\t{}", self.book.0, self.book.1, self.save_id);
                self.events.push((server::BOOK_RESULT.to_string(), answer.into_bytes()));
                return true;
            }
            if command == server::APPLY_BALANCE || command == server::PREPARE_BALANCE {
                self.balance_sent.push(command.to_string());
                return true;
            }
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
        fn threaded(&self) -> bool {
            self.threaded
        }
        fn champion_data(&mut self, name: &str) -> (Option<Damage>, Option<crate::stats::Stats>) {
            self.champions_asked += 1;
            if !self.champion_delay.is_zero() {
                std::thread::sleep(self.champion_delay);
            }
            // (data only where a test looks at it: for the champions the game lists)
            let known = self.counts && self.champions.iter().any(|c| c == name);
            (known.then_some(Damage::Mixed), None)
        }
        fn champion_count(&mut self) -> Option<usize> {
            self.counts.then_some(self.champions.len())
        }
        fn champion_name_at(&mut self, index: usize) -> Option<String> {
            self.names_asked += 1;
            self.champions.get(index).filter(|_| !self.nameless).cloned()
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
        CACHE_ON.store(false, std::sync::atomic::Ordering::Relaxed);
        BUDGET_MICROS.store(50_000, std::sync::atomic::Ordering::Relaxed);
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
        assert!(crate::draft::value(&snapshot, a, &[], &[], false, &[]) > 0.3);
        assert!(crate::draft::value(&snapshot, a, &[], &[], true, &[]) > 0.0);
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
    fn matches_from_earlier_sessions_come_back_for_the_same_save() {
        let _serial = crate::tests::serial();
        with_temp_dir();
        CACHE_ON.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(crate::paths::mod_dir().join(crate::cache::DIR));
        // session 1: 300 matches read and written to the history file
        let mut g = save();
        let mut now = frames(&mut g, 400, Instant::now());
        now += Duration::from_secs(1);
        frames(&mut g, 5, now);
        let file = crate::cache::path(g.team, &g.team_name);
        for _ in 0..200 {
            if file.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(file.exists(), "history written");
        // session 2: the game pruned the 200 oldest records
        crate::reset_for_tests();
        tick(&mut g, false, now);
        g.replays.retain(|id, _| *id > 200);
        let now = frames(&mut g, 400, now + Duration::from_secs(10));
        let _ = now;
        let snapshot = shared::get().unwrap();
        assert_eq!(snapshot.meta.matches, 300, "the pruned matches came back from the file");
        // another save with the same team: the file is not used
        crate::reset_for_tests();
        let mut other = save();
        other.replays = other.replays.into_iter().map(|(id, r)| (id, r.replace("\"1.3\"", "\"1.4\""))).collect();
        frames(&mut other, 400, Instant::now());
        assert_eq!(shared::get().unwrap().meta.matches, 300, "only its own 300");
        CACHE_ON.store(false, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn a_saves_history_under_its_own_id_is_there_as_soon_as_it_opens() {
        let _serial = crate::tests::serial();
        with_temp_dir();
        CACHE_ON.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(crate::paths::mod_dir().join(crate::cache::DIR));
        // session 1: the save says its id; its 300 matches are read and written under it
        let mut g = save();
        g.save_id = "abcdef0123456789".into();
        let mut now = frames(&mut g, 400, Instant::now());
        now += Duration::from_secs(1);
        frames(&mut g, 5, now);
        let file = crate::cache::path_of("abcdef0123456789");
        for _ in 0..200 {
            if file.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(file.exists(), "history written under the save's id");
        assert!(!crate::cache::path(g.team, &g.team_name).exists(), "not by team and name any more");
        // session 2: the save lists no match at all (as one just opened does): the model is there within frames
        crate::reset_for_tests();
        tick(&mut g, false, now);
        g.replays.clear();
        frames(&mut g, 5, now + Duration::from_secs(10));
        assert_eq!(shared::get().expect("a model").meta.matches, 300, "its matches taken at once");
        // another save with the same team and name has an id of its own: not its matches
        crate::reset_for_tests();
        tick(&mut g, false, now);
        g.save_id = "ffff000011112222".into();
        frames(&mut g, 30, now + Duration::from_secs(20));
        assert!(shared::get().is_none(), "no match of its own, none taken");
        CACHE_ON.store(false, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn a_save_that_lists_no_match_has_a_model_from_the_patch_its_notes_name() {
        let _serial = crate::tests::serial();
        with_temp_dir();
        let mut g = save();
        g.replays.clear();
        // no match and no patch known: nothing a model could be of
        frames(&mut g, 20, Instant::now());
        assert!(shared::get().is_none());
        // the news names the patch (the game's notes of 0.7): League's knowledge and the stats, for that patch
        crate::reset_for_tests();
        tick(&mut g, false, Instant::now());
        g.news = Value::Array(vec![patch_item("2027.4.0", "e")]).to_string();
        frames(&mut g, 20, Instant::now());
        let snapshot = shared::get().expect("a model");
        assert_eq!((snapshot.meta.current.as_str(), snapshot.meta.matches), ("2027.4.0", 0));
        assert!(g.sent.is_empty(), "no tier list of nothing");
    }

    #[test]
    fn a_match_begun_right_after_a_save_opened_still_gets_its_model() {
        let _serial = crate::tests::serial();
        with_temp_dir();
        let mut g = save();
        // the champions take ten frames to read; the player is on the line-up screen after three
        g.champion_delay = CHAMPION_BUDGET * 2;
        let now = frames(&mut g, 3, Instant::now());
        assert!(shared::get().is_none(), "the champions are still being read");
        g.scene = Some(ClientSceneKindV1::Lineup);
        let now = frames(&mut g, 4, now);
        assert!(shared::get().is_none());
        let asked = g.champions_asked;
        assert!(asked > 3, "read on with on the line-up screen: {asked}");
        frames(&mut g, 12, now);
        assert!(shared::get().is_some_and(|s| s.meta.matches > 0), "the model was fitted and published there");
        // around the match itself nothing is begun (a model that is not there yet is left for after it)
        crate::reset_for_tests();
        let mut g = save();
        g.champion_delay = CHAMPION_BUDGET * 2;
        let now = frames(&mut g, 3, Instant::now());
        g.scene = Some(ClientSceneKindV1::Match);
        frames(&mut g, 30, now);
        assert!(shared::get().is_none());
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
        BUDGET_MICROS.store(FRAME_BUDGET.as_micros() as u64, std::sync::atomic::Ordering::Relaxed); // the real one
        g.read_delay = Duration::from_millis(20); // a heavy replay
        g.threaded = true; // the fit runs off the frame, as in the game
        let started = Instant::now();
        let mut now = Instant::now();
        // counted, not timed: a 20 ms sleep can take 30 under a loaded test run
        let mut most = 0;
        for _ in 0..400 {
            let before = g.reads;
            tick(&mut g, true, now);
            most = most.max(g.reads - before);
            now += Duration::from_millis(16);
        }
        let per_frame = started.elapsed() / 400;
        assert!(g.reads >= 8, "still reading: {}", g.reads); // about 20, fewer when sleeps run long
        assert!(per_frame < Duration::from_micros(1800), "{per_frame:?} per frame");
        assert!(most <= 1, "one record per frame at most: {most}");
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

    fn patch_item(version: &str, champion: &str) -> Value {
        serde_json::json!({
            "author": "Team Samoyed", "title": "#asset/base/text/news?patch.title", "title_bind": [["Version", version]],
            "ty": {"PatchNote": {"version": version, "added_champion": [],
                "champion_patch_data": [[champion, [{"key": "k", "target_action": null, "pre_value": "1", "after_value": "2", "is_buff": true}]]],
                "champion_patch_reasons": [], "game_rule_patch_data": [], "game_rule_patch_reasons": []}}
        })
    }

    #[test]
    fn only_new_news_items_are_read_and_a_changed_list_is_read_again() {
        let mut g = save();
        let mut items = vec![patch_item("2027.4.0", "e"), serde_json::json!({"title": "x", "ty": "StartTeam"})];
        g.news = Value::Array(items.clone()).to_string();
        let mut st = State::new();
        // the first look reads the whole list
        assert!(probe_news(&mut st, &mut g, 4));
        assert_eq!((st.balance.news_len, st.balance.patch.as_deref(), st.notes.len()), (Some(2), Some("2027.4.0"), 1));
        // a day's news: read one by one, no patch notes among them
        items.push(serde_json::json!({"title": "y", "ty": "StartTeam"}));
        g.news = Value::Array(items.clone()).to_string();
        assert!(!probe_news(&mut st, &mut g, 4));
        assert_eq!(st.balance.news_len, Some(3));
        // the game patches: its notes come in, the newest patch known
        items.push(patch_item("2027.5.0", "f"));
        g.news = Value::Array(items.clone()).to_string();
        assert!(probe_news(&mut st, &mut g, 4));
        assert_eq!((st.balance.news_len, st.balance.patch.as_deref(), st.notes.len()), (Some(4), Some("2027.5.0"), 2));
        // nothing new: nothing read
        assert!(!probe_news(&mut st, &mut g, 4));
        // the last item changed (a round written into the game's notes): the whole list again
        items[3] = patch_item("2027.5.0", "g");
        g.news = Value::Array(items.clone()).to_string();
        assert!(probe_news(&mut st, &mut g, 4));
        assert!(st.notes.last().unwrap().changes.contains_key("g"));
        // the list got shorter: read again too
        items.remove(1);
        g.news = Value::Array(items.clone()).to_string();
        probe_news(&mut st, &mut g, 4);
        assert_eq!(st.balance.news_len, Some(3));
    }

    #[test]
    fn the_champions_are_read_a_few_a_frame() {
        let mut g = save();
        // an answer takes longer than a frame's time for them: one a frame, and the model waits
        g.champion_delay = CHAMPION_BUDGET * 2;
        let mut st = State::new();
        assert!(read_champions(&mut st, &mut g, 4));
        assert_eq!(g.champions_asked, 1);
        assert!(st.champions.is_empty() && !st.dirty);
        let mut frames = 2;
        while read_champions(&mut st, &mut g, 4) {
            frames += 1;
            assert!(frames < 20);
        }
        assert_eq!((frames, g.champions_asked, st.champions.len()), (10, 10, 10), "ten champions, ten frames");
        assert!(st.dirty, "the model is built with them");
        // quick answers: all in one frame
        let mut quick = save();
        let mut st2 = State::new();
        assert!(!read_champions(&mut st2, &mut quick, 4));
        assert_eq!((quick.champions_asked, st2.champions.len()), (10, 10));
        // read once a save
        assert!(!read_champions(&mut st, &mut g, 4));
        assert_eq!(g.champions_asked, 10);
    }

    #[test]
    fn the_champion_list_is_asked_a_name_a_frame_and_kept_for_the_next_session() {
        let file = std::env::temp_dir().join(format!("pma_champion_list_{}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let session = |name: &str| {
            let mut st = State::new();
            st.save = Some((4, name.to_string()));
            st.champions_file = file.clone();
            st
        };
        let game = |extra: &[&str]| {
            let mut g = save();
            g.counts = true;
            g.champions.extend(extra.iter().map(|c| c.to_string()));
            g
        };
        let all = |st: &mut State, g: &mut FakeGame| {
            let mut frames = 1;
            while read_champions(st, g, 4) {
                frames += 1;
                assert!(frames < 100);
            }
            frames
        };
        // a save's first session: the game says how many, then its names one a frame
        let (mut g, mut st) = (game(&[]), session("Mods FC"));
        assert!(read_champions(&mut st, &mut g, 4));
        assert_eq!((g.names_asked, g.champions_asked), (1, 1), "one name and its data a frame");
        assert_eq!(1 + all(&mut st, &mut g), 10, "ten champions, ten frames");
        assert_eq!((g.names_asked, st.champions.clone(), st.dirty), (10, g.champions.clone(), true));
        assert_eq!(st.damage.len(), 10);
        // the next session: the kept list, no name asked, the data in one frame
        let (mut g, mut st) = (game(&[]), session("Mods FC"));
        assert_eq!(all(&mut st, &mut g), 1);
        assert_eq!((g.names_asked, g.champions_asked, st.champions.len()), (2, 10, 10), "its first and last name checked");
        // a mod added a champion (the game counts one more): the kept list for now - the model does not wait - and
        // the game's names behind it, one a frame
        let (mut g, mut st) = (game(&["k"]), session("Mods FC"));
        assert!(!read_champions(&mut st, &mut g, 4), "the kept list is in use after a frame");
        assert_eq!((st.champions.len(), st.dirty, g.names_asked), (10, true, 0));
        st.dirty = false;
        let mut frames = 0;
        while st.champion_read.is_some() {
            assert!(!read_champions(&mut st, &mut g, 4), "nobody waits for the names asked behind it");
            frames += 1;
            assert!(frames < 100);
        }
        assert_eq!((frames, g.names_asked, st.champions.len(), st.dirty), (11, 11, 11, true), "then the game's own list");
        // the last one renamed (the count the same): the game's last name is not the kept one - its names again
        let (mut g, mut st) = (game(&["k2"]), session("Mods FC"));
        all(&mut st, &mut g);
        assert_eq!(st.champions.len(), 10, "the kept list for now, without k: the game has no data for it");
        while st.champion_read.is_some() {
            read_champions(&mut st, &mut g, 4);
        }
        assert_eq!(g.names_asked, 2 + 11);
        assert!(st.champions.contains(&"k2".to_string()) && !st.champions.contains(&"k".to_string()), "{:?}", st.champions);
        // one in the middle renamed: the count and both ends agree, but a kept name has no data - left out for now,
        // and the game's names asked behind
        let (mut g, mut st) = (game(&["k2"]), session("Mods FC"));
        g.champions[5] = "f2".into();
        assert!(!read_champions(&mut st, &mut g, 4));
        assert_eq!(st.champions.len(), 10, "without f, which the game has no data for");
        while st.champion_read.is_some() {
            read_champions(&mut st, &mut g, 4);
        }
        assert_eq!(g.names_asked, 2 + 11);
        assert!(st.champions.contains(&"f2".to_string()) && !st.champions.contains(&"f".to_string()), "{:?}", st.champions);
        // the game's names asked behind a list that was right: nothing changes
        let (mut g, mut st) = (game(&["k2"]), session("Mods FC"));
        g.champions[5] = "f2".into();
        all(&mut st, &mut g);
        assert!(st.champion_read.is_none() && g.names_asked == 2, "the kept list agreed");
        // another save keeps a list of its own
        let (mut g, mut st) = (game(&[]), session("Other FC"));
        all(&mut st, &mut g);
        assert_eq!(g.names_asked, 10);
        assert_eq!(crate::champions::load(&file, &crate::cache::stem(4, "Mods FC")).map(|l| l.len()), Some(11));
        // a game that counts its champions but gives no name one by one: its list all at once in the end, none kept
        let (mut g, mut st) = (game(&[]), session("Third FC"));
        g.nameless = true;
        assert_eq!(all(&mut st, &mut g), 10);
        assert_eq!(st.champions.len(), 10);
        assert_eq!(crate::champions::load(&file, &crate::cache::stem(4, "Third FC")), None);
        // the save's team name is not there yet (the state starts over when it comes): nothing asked
        let (mut g, mut st) = (game(&[]), session(""));
        assert!(read_champions(&mut st, &mut g, 4));
        assert_eq!((g.names_asked, g.champions_asked), (0, 0));
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn the_book_is_opened_once_the_server_said_which_it_is() {
        let _serial = crate::tests::serial();
        crate::reset_for_tests();
        crate::balance::reset_for_tests();
        let mut g = save();
        g.book = ("abc12345".into(), 0);
        let mut st = State::new();
        st.save = Some((4, "Mods FC".into()));
        let cfg = Config::default();
        let t0 = Instant::now();
        // asked first: nothing of the balance before the answer
        sync_balance(&mut st, &mut g, &cfg, 4, t0, &[]);
        assert!(st.balance.book.is_none() && crate::balance::book().is_none());
        sync_balance(&mut st, &mut g, &cfg, 4, t0, &[]);
        assert_eq!(g.events.len(), 1, "asked once while the answer is on its way");
        let events = g.take_events();
        sync_balance(&mut st, &mut g, &cfg, 4, t0, &events);
        assert_eq!(st.balance.book.as_deref(), Some("abc12345"));
        assert!(crate::balance::book().is_some_and(|b| b.ends_with("balance-abc12345.json")));
        // a book, the setting off and no round yet: the game's own champion patches are asked back all the same
        assert_eq!(g.balance_sent, [server::APPLY_BALANCE]);
        assert_eq!(st.balance.waiting.map(|w| (w.0, w.1)), Some((false, 0)));
        // a save with none (the setting off): nothing opened, nothing asked again
        crate::balance::reset_for_tests();
        let mut g = save();
        let mut st = State::new();
        st.save = Some((4, "Mods FC".into()));
        sync_balance(&mut st, &mut g, &cfg, 4, t0, &[]);
        let events = g.take_events();
        sync_balance(&mut st, &mut g, &cfg, 4, t0, &events);
        sync_balance(&mut st, &mut g, &cfg, 4, t0 + BOOK_RETRY * 3, &[]);
        assert_eq!(st.balance.book.as_deref(), Some(""));
        assert!(crate::balance::book().is_none() && g.events.is_empty());
        crate::balance::reset_for_tests();
    }

    #[test]
    fn the_servers_answer_counts_after_the_wait_for_it_was_given_up() {
        let _serial = crate::tests::serial();
        crate::reset_for_tests();
        crate::balance::reset_for_tests();
        let file = std::env::temp_dir().join(format!("pma_late_answer_{}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        crate::balance::open_for_tests(file.clone());
        crate::balance::publish(crate::balance::Round { id: 1, version: "1.1".into(), ..Default::default() });
        let mut g = save();
        let mut st = State::new();
        st.save = Some((4, "Mods FC".into()));
        st.balance.book = Some("book".into());
        // the setting off on a save with a round: the game's own champion patches are to be put back
        let cfg = Config::default();
        let t0 = Instant::now();
        // not before the champions are read (a round's lines are their numbers)
        sync_balance(&mut st, &mut g, &cfg, 4, t0, &[]);
        assert!(g.balance_sent.is_empty());
        st.champions = vec!["a".into()];
        sync_balance(&mut st, &mut g, &cfg, 4, t0, &[]);
        assert_eq!(g.balance_sent, [server::APPLY_BALANCE]);
        assert_eq!(st.balance.waiting.map(|w| (w.0, w.1)), Some((false, 1)));
        // a save being opened stalls the game: no answer in time
        let late = t0 + REPLY_TIMEOUT + Duration::from_secs(1);
        sync_balance(&mut st, &mut g, &cfg, 4, late, &[]);
        assert!(st.balance.waiting.is_none() && st.balance.acked.is_none());
        // it comes a frame later: taken all the same, nothing sent again
        let answer = (server::BALANCE_RESULT.to_string(), b"0/1\tok\tthe game's champion patches put back".to_vec());
        sync_balance(&mut st, &mut g, &cfg, 4, late, &[answer]);
        assert_eq!(st.balance.acked, Some((false, 1)));
        sync_balance(&mut st, &mut g, &cfg, 4, late + Duration::from_secs(900), &[]);
        assert_eq!(g.balance_sent.len(), 1);
        crate::balance::reset_for_tests();
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn the_next_patch_day_comes_from_the_year_schedules() {
        let schedule = r#"[
            {"id": 1, "schedules": [["2027-09-15", "MinorPatch"], ["2027-10-01", {"LeagueMatch": {"index": 0, "round": 0}}],
                                    ["2027-10-03", "MinorPatch"], ["2027-12-30", {"SeasonPatch": {}}]]},
            {"id": 2, "schedules": [["2028-01-01", "StartRecruit"], ["2028-02-01", "MajorPatch"]]}]"#;
        assert_eq!(next_patch_day(schedule, (2027, 10, 1)), Some((2027, 10, 3)));
        assert_eq!(next_patch_day(schedule, (2027, 10, 3)), Some((2027, 10, 3)), "the day itself");
        assert_eq!(next_patch_day(schedule, (2027, 10, 4)), Some((2027, 12, 30)));
        assert_eq!(next_patch_day(schedule, (2028, 1, 1)), Some((2028, 2, 1)));
        assert_eq!(next_patch_day(schedule, (2028, 2, 2)), None);
        assert_eq!(next_patch_day("{", (2027, 1, 1)), None);
    }
}
