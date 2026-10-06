//! Reading the save's match records a few at a time, newest first: every usable match is kept
//! (`history::Game`), and games and wins are counted per patch version and champion.
//!
//! The scanner remembers the record **ids** it has read, never positions in an id list: the
//! list may shrink or change completely between two calls (another save, a new game, pruned
//! replays) and that must only mean fewer records, not an out-of-bounds index. The game also
//! hands ids out again after pruning, so an id that leaves the list is forgotten (read again if
//! it comes back), while the games read so far are kept - keyed by the match's `seed`, so the
//! same match under a new id replaces itself instead of counting twice.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};

use mod_api_stable::RecordKindV1;

use crate::history::{Game, Names, Role, Slot};
use crate::records::{compare_versions, parse_record, MatchSummary, Parsed, Player};

/// What the scanner needs from the game (implemented by the client context, and by tests).
pub trait Source {
    fn record_ids(&mut self, kind: RecordKindV1) -> Vec<usize>;
    fn record_json(&mut self, kind: RecordKindV1, id: usize, path: &str) -> Option<String>;
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChampStats {
    pub m: u32,
    pub w: u32,
    /// Games per lane label (`"Top"`, ...).
    pub lanes: BTreeMap<String, u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VersionStats {
    pub matches: u32,
    pub champs: HashMap<String, ChampStats>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counts {
    pub read: u32,
    pub matches: u32,
    pub not_played: u32,
    pub invalid: u32,
    pub fetch_failed: u32,
    pub searched: u32,
    pub skipped_old: u32,
}

/// After this many records in a row older than every kept patch, the rest of the backlog is
/// assumed to be older too and is skipped.
const OLD_STREAK_STOP: u32 = 100;

/// Patches whose games are read (the model keeps as many; see `meta::Settings::max_patches`).
pub const KEPT_PATCHES: usize = 12;

pub struct Scanner {
    pub kind: RecordKindV1,
    pub solo: bool,
    done: HashSet<usize>,
    queued: HashSet<usize>,
    /// Ascending; the newest id is popped first.
    queue: Vec<usize>,
    not_played: HashSet<usize>,
    old_streak: u32,
    capped: bool,
    pub versions: BTreeMap<String, VersionStats>,
    /// Every usable match, by [`match_key`].
    pub games: BTreeMap<u64, Game>,
    pub counts: Counts,
    /// Every record id the game listed at the last refresh.
    pub listed: usize,
    /// The first parsed record and the first failure are described once in `diag.log`.
    pub first_match: Option<String>,
    pub first_problem: Option<String>,
    /// The first usable record as the game gave it (written to `probe_*.json` once).
    pub first_raw: Option<String>,
    /// The keys of the first matches read (newest first), to recognise this save's history file.
    pub first_keys: Vec<u64>,
}

pub enum Step {
    /// Nothing left to read.
    Idle,
    /// Read one record (whether or not it was usable).
    Read,
}

impl Scanner {
    pub fn new(kind: RecordKindV1, solo: bool) -> Self {
        Self {
            kind,
            solo,
            done: HashSet::new(),
            queued: HashSet::new(),
            queue: Vec::new(),
            not_played: HashSet::new(),
            old_streak: 0,
            capped: false,
            versions: BTreeMap::new(),
            games: BTreeMap::new(),
            counts: Counts::default(),
            listed: 0,
            first_match: None,
            first_problem: None,
            first_raw: None,
            first_keys: Vec::new(),
        }
    }

    pub fn pending(&self) -> usize {
        self.queue.len()
    }

    /// Lists the game's record ids and queues the ones not read yet. Solo-rank matches that
    /// were not played yet stay out of the queue; `recheck_unplayed` puts the oldest of them
    /// (at most that many - a save can hold thousands of scheduled ones) back in.
    pub fn refresh(&mut self, src: &mut impl Source, recheck_unplayed: usize) {
        if recheck_unplayed > 0 && !self.not_played.is_empty() {
            let mut oldest: Vec<usize> = self.not_played.iter().copied().collect();
            oldest.sort_unstable();
            oldest.truncate(recheck_unplayed);
            for id in oldest {
                self.not_played.remove(&id);
            }
        }
        if self.capped {
            return;
        }
        let ids = src.record_ids(self.kind);
        self.listed = ids.len();
        self.forget_missing(&ids);
        let mut added = false;
        for id in ids {
            if self.done.contains(&id) || self.queued.contains(&id) || self.not_played.contains(&id) {
                continue;
            }
            self.queued.insert(id);
            self.queue.push(id);
            added = true;
        }
        if added {
            self.queue.sort_unstable();
        }
    }

    /// Ids no longer listed are forgotten: if the game hands one out again it is a new record.
    fn forget_missing(&mut self, ids: &[usize]) {
        let listed: HashSet<usize> = ids.iter().copied().collect();
        self.done.retain(|id| listed.contains(id));
        self.not_played.retain(|id| listed.contains(id));
    }

    /// Stops the backlog after `limit` records read in this save (newest first, so what is
    /// dropped is the oldest). New records keep coming in after that.
    pub fn cap_backlog(&mut self, limit: u32) {
        if self.capped || self.counts.read < limit {
            return;
        }
        self.capped = true;
        self.counts.skipped_old += self.queue.len() as u32;
        for id in self.queue.drain(..) {
            self.done.insert(id);
        }
        self.queued.clear();
    }

    /// After the backlog cap: queue only ids newer than everything read so far.
    pub fn refresh_new(&mut self, src: &mut impl Source) {
        if !self.capped {
            return;
        }
        let newest = self.done.iter().copied().max().unwrap_or(0);
        let ids = src.record_ids(self.kind);
        self.listed = ids.len();
        self.forget_missing(&ids);
        for id in ids {
            if id > newest && !self.done.contains(&id) && !self.queued.contains(&id) && !self.not_played.contains(&id) {
                self.queued.insert(id);
                self.queue.push(id);
            }
        }
        self.queue.sort_unstable();
    }

    /// Reads and counts the newest queued record.
    pub fn step(&mut self, src: &mut impl Source, names: &mut Names) -> Step {
        let Some(id) = self.queue.pop() else { return Step::Idle };
        self.queued.remove(&id);
        self.counts.read += 1;
        let Some(json) = src.record_json(self.kind, id, "") else {
            self.counts.fetch_failed += 1;
            self.done.insert(id);
            self.note_problem(id, "the game returned nothing for this record".to_string());
            return Step::Read;
        };
        match parse_record(&json, self.solo) {
            Parsed::Match(summary) => {
                self.done.insert(id);
                self.counts.matches += 1;
                if summary.searched {
                    self.counts.searched += 1;
                }
                if self.first_match.is_none() {
                    self.first_match = Some(describe(id, &summary, json.len()));
                    self.first_raw = Some(json.clone());
                }
                self.track_age(&summary.version);
                self.add(id, &summary, names);
            }
            Parsed::NotPlayed => {
                self.counts.not_played += 1;
                self.not_played.insert(id);
            }
            Parsed::Invalid(why) => {
                self.done.insert(id);
                self.counts.invalid += 1;
                self.note_problem(id, why);
            }
        }
        Step::Read
    }

    fn note_problem(&mut self, id: usize, why: String) {
        if self.first_problem.is_none() {
            self.first_problem = Some(format!("record #{id}: {why}"));
        }
    }

    /// Stops the backfill once it is clearly past the oldest kept patch.
    fn track_age(&mut self, version: &str) {
        let oldest_kept = self.versions_newest_first().into_iter().nth(KEPT_PATCHES - 1);
        let too_old = oldest_kept.is_some_and(|v| compare_versions(version, &v) == Ordering::Less);
        self.old_streak = if too_old { self.old_streak + 1 } else { 0 };
        if self.old_streak >= OLD_STREAK_STOP {
            self.counts.skipped_old += self.queue.len() as u32;
            for id in self.queue.drain(..) {
                self.done.insert(id);
            }
            self.queued.clear();
            self.old_streak = 0;
        }
    }

    fn add(&mut self, id: usize, m: &MatchSummary, names: &mut Names) {
        let key = match_key(id, m);
        if self.first_keys.len() < crate::cache::CONFIRM_MATCHES {
            self.first_keys.push(key);
        }
        if self.games.contains_key(&key) {
            // the same match under a new record id: it is already counted
            return;
        }
        let side = |players: &[Player], names: &mut Names| -> Vec<Slot> {
            players
                .iter()
                .map(|p| Slot {
                    champ: names.id(&p.champion),
                    role: p.position.as_deref().and_then(Role::parse),
                    athlete: p.athlete,
                    lane_gold: p.lane_gold,
                })
                .collect()
        };
        let game = Game {
            record: id,
            solo: self.solo,
            version: m.version.clone(),
            blue_win: m.blue_win,
            teams: m.teams,
            sides: [side(&m.blue, names), side(&m.red, names)],
            bans: [
                m.bans[0].iter().map(|b| names.id(b)).collect(),
                m.bans[1].iter().map(|b| names.id(b)).collect(),
            ],
            length: m.length.map(|t| t as f32),
            tactics: [
                m.strategies[0].iter().map(|(s, o)| crate::history::tactic_id(s, o)).collect(),
                m.strategies[1].iter().map(|(s, o)| crate::history::tactic_id(s, o)).collect(),
            ],
        };
        self.games.insert(key, game);
        let stats = self.versions.entry(m.version.clone()).or_default();
        stats.matches += 1;
        let mut count = |players: &[Player], won: bool| {
            for p in players {
                let c = stats.champs.entry(p.champion.clone()).or_default();
                c.m += 1;
                c.w += won as u32;
                if let Some(lane) = &p.position {
                    *c.lanes.entry(lane.clone()).or_insert(0) += 1;
                }
            }
        };
        count(&m.blue, m.blue_win);
        count(&m.red, !m.blue_win);
    }

    /// Takes a match kept from an earlier session (`cache`), unless it is known already.
    pub fn adopt(&mut self, key: u64, game: Game, names: &Names) -> bool {
        if self.games.contains_key(&key) {
            return false;
        }
        let stats = self.versions.entry(game.version.clone()).or_default();
        stats.matches += 1;
        for (side, slots) in game.sides.iter().enumerate() {
            let won = game.won(side);
            for s in slots {
                let c = stats.champs.entry(names.name(s.champ).to_string()).or_default();
                c.m += 1;
                c.w += won as u32;
                if let Some(role) = s.role {
                    *c.lanes.entry(role.name().to_string()).or_insert(0) += 1;
                }
            }
        }
        self.games.insert(key, game);
        true
    }

    /// Versions with games, newest first.
    pub fn versions_newest_first(&self) -> Vec<String> {
        let mut v: Vec<String> = self.versions.keys().cloned().collect();
        v.sort_by(|a, b| compare_versions(b, a));
        v
    }

    pub fn current_version(&self) -> Option<String> {
        self.versions_newest_first().into_iter().next()
    }

    pub fn previous_version(&self) -> Option<String> {
        self.versions_newest_first().into_iter().nth(1)
    }
}

/// Identifies a match across record ids: its seed, patch, champions and result (several
/// matches could share a seed). Without a seed, the record id.
fn match_key(id: usize, m: &MatchSummary) -> u64 {
    use std::hash::{Hash, Hasher};
    let Some(seed) = m.seed else { return id as u64 | 1 << 63 };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (seed, &m.version, m.blue_win).hash(&mut h);
    for p in m.blue.iter().chain(&m.red) {
        p.champion.hash(&mut h);
    }
    h.finish() & !(1 << 63)
}

fn describe(id: usize, m: &MatchSummary, bytes: usize) -> String {
    let lanes = m.blue.iter().chain(&m.red).filter(|p| p.position.is_some()).count();
    let sample: Vec<String> = m
        .blue
        .iter()
        .take(2)
        .map(|p| format!("{}@{}", p.champion, p.position.as_deref().unwrap_or("?")))
        .collect();
    format!(
        "record #{id} ({bytes} bytes{}): version {:?}, {} vs {} champions ({} with a lane), {} won, e.g. {}",
        if m.searched { ", fields found by searching - layout differs from 0.6.0" } else { "" },
        m.version,
        m.blue.len(),
        m.red.len(),
        lanes,
        if m.blue_win { "blue" } else { "red" },
        sample.join(", ")
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::BTreeMap as Map;

    /// A save with records that can be replaced (another save) or pruned.
    #[derive(Default)]
    pub struct FakeSave {
        pub records: Map<usize, String>,
        pub reads: u32,
    }

    impl Source for FakeSave {
        fn record_ids(&mut self, _kind: RecordKindV1) -> Vec<usize> {
            self.records.keys().copied().collect()
        }
        fn record_json(&mut self, _kind: RecordKindV1, id: usize, _path: &str) -> Option<String> {
            self.reads += 1;
            self.records.get(&id).cloned()
        }
    }

    pub fn game(version: &str, blue: &[&str], red: &[&str], blue_win: bool) -> String {
        let side = |names: &[&str]| -> String {
            let lanes = ["Top", "Jungle", "Mid", "Bottom", "Support"];
            names
                .iter()
                .zip(lanes)
                .map(|(n, l)| format!(r#"{{"champion":"{n}","position":"{l}"}}"#))
                .collect::<Vec<_>>()
                .join(",")
        };
        format!(
            r#"{{"version":"{version}","blue_team_win":{blue_win},"blue_team":[{}],"red_team":[{}]}}"#,
            side(blue),
            side(red)
        )
    }

    fn drain(scan: &mut Scanner, save: &mut FakeSave) {
        let mut names = Names::default();
        while let Step::Read = scan.step(save, &mut names) {}
    }

    #[test]
    fn counts_games_wins_and_lanes_per_version() {
        let mut save = FakeSave::default();
        save.records.insert(1, game("1.2", &["a", "b"], &["c", "d"], true));
        save.records.insert(2, game("1.3", &["a", "c"], &["b", "d"], false));
        save.records.insert(3, game("1.3", &["c", "a"], &["b", "d"], true));
        let mut scan = Scanner::new(RecordKindV1::MatchReplay, false);
        scan.refresh(&mut save, 0);
        drain(&mut scan, &mut save);
        assert_eq!(scan.current_version().as_deref(), Some("1.3"));
        assert_eq!(scan.previous_version().as_deref(), Some("1.2"));
        let cur = &scan.versions["1.3"];
        assert_eq!(cur.matches, 2);
        assert_eq!((cur.champs["a"].m, cur.champs["a"].w), (2, 1));
        assert_eq!((cur.champs["c"].m, cur.champs["c"].w), (2, 1));
        assert_eq!((cur.champs["b"].m, cur.champs["b"].w), (2, 1));
        assert_eq!(cur.champs["a"].lanes["Top"], 1);
        assert_eq!(cur.champs["a"].lanes["Jungle"], 1);
        assert_eq!(scan.counts.matches, 3);
        assert!(scan.first_match.as_deref().unwrap().starts_with("record #3"), "newest first");
    }

    #[test]
    fn a_shorter_or_different_id_list_is_fine() {
        let mut names = Names::default();
        let mut save = FakeSave::default();
        for id in 1..=50 {
            save.records.insert(id, game("1.3", &["a"], &["b"], id % 2 == 0));
        }
        let mut scan = Scanner::new(RecordKindV1::MatchReplay, false);
        scan.refresh(&mut save, 0);
        for _ in 0..10 {
            scan.step(&mut save, &mut names);
        }
        // records pruned / another save: the list shrinks to 3 ids, two of them unknown
        save.records.retain(|id, _| *id <= 3);
        save.records.insert(1000, game("1.3", &["c"], &["d"], true));
        scan.refresh(&mut save, 0);
        drain(&mut scan, &mut save);
        assert_eq!(scan.counts.matches, 10 + 3 + 1);
        assert_eq!(scan.counts.fetch_failed, 37, "queued ids that vanished are skipped");
        // nothing is read twice
        let before = save.reads;
        scan.refresh(&mut save, 0);
        drain(&mut scan, &mut save);
        assert_eq!(save.reads, before);
    }

    #[test]
    fn unplayed_solo_matches_are_read_again_later() {
        let mut save = FakeSave::default();
        let unplayed = r#"{"played":false,"version":"1.3","blue_team_win":false,"blue_team":[{"champion":"a"}],"red_team":[{"champion":"b"}]}"#;
        save.records.insert(7, unplayed.to_string());
        let mut scan = Scanner::new(RecordKindV1::SoloRankMatch, true);
        scan.refresh(&mut save, 0);
        drain(&mut scan, &mut save);
        assert_eq!(scan.counts.not_played, 1);
        scan.refresh(&mut save, 0);
        assert_eq!(scan.pending(), 0, "not re-read on every refresh");
        save.records.insert(7, unplayed.replace("\"played\":false", "\"played\":true"));
        scan.refresh(&mut save, 100);
        drain(&mut scan, &mut save);
        assert_eq!(scan.counts.matches, 1);
        assert_eq!(scan.versions["1.3"].champs["b"].w, 1);
    }

    #[test]
    fn stops_after_a_long_run_of_old_patches() {
        // 20 patches of 50 records each, newest last; only the newest KEPT_PATCHES are wanted
        let mut save = FakeSave::default();
        for id in 1..=1000 {
            let v = format!("1.{}", (id - 1) / 50);
            save.records.insert(id, game(&v, &["a"], &["b"], true));
        }
        let mut scan = Scanner::new(RecordKindV1::MatchReplay, false);
        scan.refresh(&mut save, 0);
        drain(&mut scan, &mut save);
        let kept = 50 * KEPT_PATCHES as u32;
        assert_eq!(scan.counts.matches, kept + OLD_STREAK_STOP);
        assert_eq!(scan.counts.skipped_old, 1000 - kept - OLD_STREAK_STOP);
        assert_eq!(scan.games.len() as u32, kept + OLD_STREAK_STOP);
    }

    #[test]
    fn a_reused_id_is_read_again_and_a_moved_match_counts_once() {
        let mut save = FakeSave::default();
        let with_seed = |seed: u32, champ: &str| {
            game("1.3", &[champ], &["b"], true).replacen('{', &format!("{{\"seed\":{seed},"), 1)
        };
        save.records.insert(1, with_seed(10, "a"));
        save.records.insert(2, with_seed(20, "c"));
        let mut scan = Scanner::new(RecordKindV1::MatchReplay, false);
        scan.refresh(&mut save, 0);
        drain(&mut scan, &mut save);
        assert_eq!(scan.games.len(), 2);
        // the game prunes record 1 and later hands id 1 to a new match; match 20 moves to id 5
        save.records.remove(&1);
        scan.refresh(&mut save, 0);
        save.records.insert(1, with_seed(30, "d"));
        save.records.remove(&2);
        save.records.insert(5, with_seed(20, "c"));
        scan.refresh(&mut save, 0);
        drain(&mut scan, &mut save);
        assert_eq!(scan.games.len(), 3, "pruned games are kept, the moved one is not doubled");
        assert_eq!(scan.versions["1.3"].matches, 3);
    }

    #[test]
    fn unplayed_rechecks_and_the_backlog_are_bounded() {
        let mut names = Names::default();
        let mut save = FakeSave::default();
        let unplayed = r#"{"played":false,"version":"1.3","blue_team_win":false,"blue_team":[{"champion":"a"}],"red_team":[{"champion":"b"}]}"#;
        for id in 1..=1000 {
            save.records.insert(id, unplayed.to_string());
        }
        let mut scan = Scanner::new(RecordKindV1::SoloRankMatch, true);
        scan.refresh(&mut save, 0);
        drain(&mut scan, &mut save);
        assert_eq!(save.reads, 1000);
        scan.refresh(&mut save, 0);
        assert_eq!(scan.pending(), 0);
        scan.refresh(&mut save, 100);
        assert_eq!(scan.pending(), 100, "only the oldest 100 are looked at again");
        drain(&mut scan, &mut save);
        assert_eq!(save.reads, 1100);

        // the backlog stops after the cap; newer records still come in
        let mut save = FakeSave::default();
        for id in 1..=500 {
            save.records.insert(id, game("1.3", &["a"], &["b"], true));
        }
        let mut scan = Scanner::new(RecordKindV1::MatchReplay, false);
        scan.refresh(&mut save, 0);
        for _ in 0..50 {
            scan.step(&mut save, &mut names);
        }
        scan.cap_backlog(50);
        assert_eq!(scan.pending(), 0);
        assert_eq!(scan.counts.skipped_old, 450);
        save.records.insert(501, game("1.3", &["a"], &["b"], false));
        scan.refresh(&mut save, 0);
        scan.refresh_new(&mut save);
        assert_eq!(scan.pending(), 1);
        drain(&mut scan, &mut save);
        assert_eq!(scan.counts.matches, 51);
    }

    #[test]
    fn unusable_records_are_described_once() {
        let mut save = FakeSave::default();
        save.records.insert(1, r#"{"date":3}"#.to_string());
        save.records.insert(2, "garbage".to_string());
        let mut scan = Scanner::new(RecordKindV1::MatchReplay, false);
        scan.refresh(&mut save, 0);
        drain(&mut scan, &mut save);
        assert_eq!(scan.counts.invalid, 2);
        assert!(scan.first_problem.as_deref().unwrap().starts_with("record #2: not JSON"));
    }
}
