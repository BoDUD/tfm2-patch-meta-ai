//! Balance patches (2.8.0; the user 2026-11-11: 「现在能不能让meta控制版本平衡的补丁吧 现在系统自带的平衡补丁 英雄
//! 削弱不痛不痒 加强也是 砍不到位 没有lol那种感觉」). On one save at patch 2027.4.1 the game's own patches had nerfed
//! champions at 46% (Gwen, Nami, Brand) and ones with no games while Maddie sat at 73% with a 52% ban rate untouched,
//! and its nerfs barely moved anyone (Zeri 64.4% -> 64.0%). With `balance=on` this mod makes the patches instead:
//!
//! - **A round per game patch**: when the game moves to a new patch (or, the game's own champion patches being off,
//!   after [`ROUND_MATCHES`] more competition matches in one), the champions the model rates furthest outside
//!   [`BAND`] - with [`MIN_EVIDENCE`] recent games or more, at most `balance_count` of them - change by [`PER_POINT`]
//!   percent of power for every win-rate point outside the band, at most `balance_max` a round, at most [`CAP`] in
//!   all (rounds add up, as in League: a champion still too strong is cut again next patch).
//! - **In every match**: a champion's power is a permanent buff ([`BUFF`]), half on damage (attack and magic power),
//!   half on health ([`buff`]), put on it at the start and again on every respawn (a new entity).
//! - **Every run alike**: a match is simulated more than once (the server's run is the result, the client plays it
//!   again to show it, replays again later). The server's first run takes the newest round and keeps it in this
//!   save's book under the ten champions (`ai_tactics`' key); every later run plays the kept round, and a run of a
//!   match nothing was kept for plays none.
//! - **Patch notes**: each round goes to the player's inbox (the server half, `apply_balance`), and to the model as
//!   the patch's notes (the direction of each change), as the game's own notes do.
//! - **The game's own champion patches off**: the server half sets the save's auto-patcher counts to zero while this
//!   is on, keeping the save's own values to put back when it is turned off (`server::balance_autopatch`).
//!
//! The book is one file a save in the mod folder (`balance-team<id>-<hash>.json`): every round (its changes and
//! every champion's power after it) and the kept matches.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use mod_api_stable::{BuffV1, SimOriginKindV1, StableSim};
use serde::{Deserialize, Serialize};

use crate::meta::Meta;
use crate::patchnotes::PatchNote;
use crate::{diag, paths};

/// The buff that carries a champion's balance in a match.
pub const BUFF: &str = "patch_meta_ai_balance";
/// Win rates inside this band (percent) are left alone.
pub const BAND: (f32, f32) = (48.0, 52.0);
/// Power changed for every win-rate point outside the band (percent of power).
pub const PER_POINT: f32 = 1.5;
/// A champion's power stays within this much of its own (percent).
pub const CAP: i32 = 30;
/// Recent games (each patch back counting half, the model's `evidence`) before a champion's win rate is trusted.
pub const MIN_EVIDENCE: f32 = 30.0;
/// Competition matches in one game patch after which another round is made in it.
pub const ROUND_MATCHES: u32 = 600;
/// Matches whose round is kept (the newest).
const KEPT: usize = 4000;
/// A run waits this long for all its champions before it settles with the ones there.
const TICK_WINDOW: usize = 600;
/// Runs remembered (their decided balance).
const RUNS_KEPT: usize = 64;

/// How big a round is (settings `balance_count`, `balance_max`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    pub count: usize,
    pub max_step: i32,
}

/// One champion's change in a round.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub champion: String,
    /// The model's win rate for it when the round was made (percent) and its recent games.
    pub win: f32,
    pub evidence: f32,
    /// This round's step and its power after it (percent).
    pub step: i32,
    pub total: i32,
}

/// One balance patch.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Round {
    pub id: u32,
    /// The game's patch it was made in.
    pub version: String,
    /// Competition matches of that patch when it was made.
    pub matches: u32,
    pub changes: Vec<Change>,
    /// Every champion's power after it (percent; absent = 0).
    pub power: BTreeMap<String, i32>,
}

/// A champion the model rates: name, win rate (percent), recent games.
#[derive(Clone, Copy, Debug)]
pub struct Candidate<'a> {
    pub name: &'a str,
    pub win: f32,
    pub evidence: f32,
}

/// The changes of a round: the champions furthest outside [`BAND`] with enough games, at most `rules.count`, each a
/// step of [`PER_POINT`] a point outside the band (at most `rules.max_step`), their power kept within [`CAP`].
pub fn decide(candidates: &[Candidate<'_>], before: &BTreeMap<String, i32>, rules: Rules) -> Vec<Change> {
    let mut out: Vec<(f32, Change)> = candidates
        .iter()
        .filter(|c| c.evidence >= MIN_EVIDENCE)
        .filter_map(|c| {
            // above the band: a nerf; below it: a buff
            let gap = if c.win > BAND.1 {
                c.win - BAND.1
            } else if c.win < BAND.0 {
                c.win - BAND.0
            } else {
                return None;
            };
            let want = ((-(gap * PER_POINT)).round() as i32).clamp(-rules.max_step, rules.max_step);
            let was = before.get(c.name).copied().unwrap_or(0);
            let total = (was + want).clamp(-CAP, CAP);
            let step = total - was;
            (step != 0).then(|| {
                (gap.abs(), Change { champion: c.name.to_string(), win: c.win, evidence: c.evidence, step, total })
            })
        })
        .collect();
    out.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.champion.cmp(&b.1.champion)));
    out.truncate(rules.count);
    out.into_iter().map(|(_, c)| c).collect()
}

/// The round after `last` for the model's `meta`, or None when none is due: the first one, one for a new game patch,
/// or another in the same patch after [`ROUND_MATCHES`] more matches. A round that changes nobody is still a round
/// (it marks the patch as looked at).
pub fn next_round(meta: &Meta, last: Option<&Round>, rules: Rules) -> Option<Round> {
    if meta.current.is_empty() {
        return None;
    }
    let due = match last {
        None => true,
        Some(r) if r.version != meta.current => true,
        Some(r) => meta.current_matches >= r.matches + ROUND_MATCHES,
    };
    if !due {
        return None;
    }
    let before = last.map(|r| r.power.clone()).unwrap_or_default();
    let candidates: Vec<Candidate<'_>> = meta
        .champions
        .iter()
        .map(|c| Candidate { name: &c.name, win: c.win_rate() * 100.0, evidence: c.evidence })
        .collect();
    let changes = decide(&candidates, &before, rules);
    let mut power = before;
    for c in &changes {
        if c.total == 0 {
            power.remove(&c.champion);
        } else {
            power.insert(c.champion.clone(), c.total);
        }
    }
    Some(Round {
        id: last.map_or(1, |r| r.id + 1),
        version: meta.current.clone(),
        matches: meta.current_matches,
        changes,
        power,
    })
}

/// The buff for a champion at `power` percent: half on damage (attack and magic power), the rest on health.
pub fn buff(power: i32) -> BuffV1 {
    let damage = power / 2;
    BuffV1 { attack_mult: damage, magic_power_mult: damage, hp_mult: power - damage, ..BuffV1::named(BUFF) }
}

/// The patch notes of the rounds for the model: the first round of each game patch gives each champion it changed
/// that patch's direction (+1 buffed, -1 nerfed), as the game's own notes do.
pub fn notes_of(rounds: &[Round]) -> Vec<PatchNote> {
    let mut out: Vec<PatchNote> = Vec::new();
    for r in rounds {
        if out.iter().any(|n| n.is_for(&r.version)) {
            continue;
        }
        let changes = r.changes.iter().map(|c| (c.champion.clone(), c.step.signum())).collect();
        out.push(PatchNote { versions: vec![r.version.clone()], changes });
    }
    out
}

/// The round's news: a title and its lines, champions by `label` (the game's names), in Chinese when those are.
pub fn news(round: &Round, label: &dyn Fn(&str) -> String) -> (String, String) {
    let chinese = round.changes.iter().any(|c| label(&c.champion).chars().any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch)));
    let title = if chinese {
        format!("智能BP 平衡补丁 {}（第 {} 轮）", round.version, round.id)
    } else {
        format!("Patch Meta AI balance patch {} (round {})", round.version, round.id)
    };
    let mut lines = Vec::new();
    if round.changes.is_empty() {
        lines.push(if chinese {
            "这一轮没有英雄胜率超出 48%–52%（场次够的英雄里），不改动。".to_string()
        } else {
            "No champion with enough games is outside 48-52% win rate: no changes.".to_string()
        });
    }
    for (title_cn, title_en, nerf) in [("削弱", "Nerfs", true), ("加强", "Buffs", false)] {
        let list: Vec<&Change> = round.changes.iter().filter(|c| (c.step < 0) == nerf).collect();
        if list.is_empty() {
            continue;
        }
        lines.push(if chinese { format!("【{title_cn}】") } else { format!("[{title_en}]") });
        for c in list {
            let damage = c.step / 2;
            let health = c.step - damage;
            lines.push(if chinese {
                format!(
                    "{}：胜率 {:.1}%（目标 48%–52%）→ 强度 {:+}%（攻击/法强 {:+}%，生命 {:+}%），累计 {:+}%",
                    label(&c.champion),
                    c.win,
                    c.step,
                    damage,
                    health,
                    c.total
                )
            } else {
                format!(
                    "{}: win rate {:.1}% (target 48-52%) -> power {:+}% (attack / magic power {:+}%, health {:+}%), {:+}% in all",
                    label(&c.champion),
                    c.win,
                    c.step,
                    damage,
                    health,
                    c.total
                )
            });
        }
    }
    (title, lines.join("\n"))
}

// ===================== the book (one save) =====================

#[derive(Default, Serialize, Deserialize)]
struct Book {
    v: u32,
    rounds: Vec<Round>,
    /// (match key, round id; 0 = none) - the newest last.
    kept: Vec<(String, u32)>,
}

#[derive(Default)]
struct Store {
    /// This save's file (None: no save open).
    file: Option<PathBuf>,
    rounds: Vec<Round>,
    /// Each round's power, shared with the runs.
    powers: HashMap<u32, Arc<BTreeMap<String, i32>>>,
    kept: HashMap<String, u32>,
    order: VecDeque<String>,
    /// Kept matches not written yet, and when the book was last written (a day plays many matches: the book is
    /// written at most every [`WRITE_EVERY`], and at once for a new round or a closed save).
    unwritten: bool,
    written_at: Option<std::time::Instant>,
}

/// The book is written at most this often while matches are kept.
const WRITE_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| Mutex::new(Store::default()));

fn store() -> std::sync::MutexGuard<'static, Store> {
    STORE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The book's file for a save: the player's team id and a hash of its name (as the match history's).
pub fn path(team: usize, team_name: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    team_name.hash(&mut h);
    paths::mod_dir().join(format!("balance-team{team}-{:08x}.json", h.finish() as u32))
}

/// A save is open: its book (empty when it has none yet).
pub fn open(team: usize, team_name: &str) {
    open_file(path(team, team_name));
}

fn open_file(file: PathBuf) {
    let book = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str::<Book>(&t).ok()).unwrap_or_default();
    let mut st = store();
    *st = Store { file: Some(file), ..Store::default() };
    for r in book.rounds {
        st.powers.insert(r.id, Arc::new(r.power.clone()));
        st.rounds.push(r);
    }
    for (key, id) in book.kept {
        st.order.push_back(key.clone());
        st.kept.insert(key, id);
    }
    if !st.rounds.is_empty() {
        diag::log(&format!("[balance] {} rounds in this save's book, {} matches kept", st.rounds.len(), st.kept.len()));
    }
}

/// The save is closed (its kept matches written first).
pub fn close() {
    let mut st = store();
    if st.unwritten {
        write(&mut st);
    }
    *st = Store::default();
}

fn write(st: &mut Store) {
    let Some(file) = &st.file else { return };
    let kept = st.order.iter().filter_map(|k| Some((k.clone(), *st.kept.get(k)?))).collect();
    let book = Book { v: 1, rounds: st.rounds.clone(), kept };
    if let Ok(text) = serde_json::to_string(&book) {
        let _ = std::fs::write(file, text);
    }
    st.unwritten = false;
    st.written_at = Some(std::time::Instant::now());
}

/// Every round of this save, oldest first.
pub fn rounds() -> Vec<Round> {
    store().rounds.clone()
}

/// This save's newest round (each frame asks: no copy of the whole book).
pub fn latest() -> Option<Round> {
    store().rounds.last().cloned()
}

/// A new round: in the book from now on (new matches play it).
pub fn publish(round: Round) {
    let mut st = store();
    if st.file.is_none() {
        return;
    }
    st.powers.insert(round.id, Arc::new(round.power.clone()));
    st.rounds.push(round);
    write(&mut st);
}

/// The model's patch notes from this save's rounds.
pub fn notes() -> Vec<PatchNote> {
    notes_of(&store().rounds)
}

// ===================== in the matches =====================

type RunKey = (u32, u64, u64, u64);

struct Run {
    key: RunKey,
    last_tick: usize,
    /// None until decided; Some(None) = no balance.
    power: Option<Option<Arc<BTreeMap<String, i32>>>>,
}

static RUNS: LazyLock<Mutex<VecDeque<Run>>> = LazyLock::new(|| Mutex::new(VecDeque::new()));

fn run_key(sim: &StableSim<'_>) -> RunKey {
    sim.sim_origin().map_or((u32::MAX, u64::MAX, u64::MAX, u64::MAX), |o| (o.kind, o.match_id, o.replay_id, o.set_index))
}

/// This run's balance, decided once it has its champions (None: not yet). The server's run takes the newest round
/// (`on`: the setting) and keeps it; any other run plays what was kept (none when nothing was).
fn decide_run(sim: &StableSim<'_>, on: bool) -> Option<Option<Arc<BTreeMap<String, i32>>>> {
    let mut sides: [Vec<String>; 2] = [Vec::new(), Vec::new()];
    let mut missing = 0;
    for i in 0..sim.player_count() {
        let Some(p) = sim.player_at(i) else { continue };
        match (sides.get_mut(p.team()), p.champion().and_then(|e| e.name())) {
            (Some(side), Some(name)) => side.push(name),
            _ => missing += 1,
        }
    }
    if (missing > 0 || sides.iter().any(Vec::is_empty)) && sim.tick() < TICK_WINDOW {
        return None;
    }
    let key = crate::ai_tactics::key(&sides);
    let first_run = sim.sim_origin().is_some_and(|o| SimOriginKindV1::from_code(o.kind) == Some(SimOriginKindV1::ServerPresim));
    Some(round_for(key, first_run, on))
}

/// The power a run of the match `key` plays: the server's run (`first_run`) takes the newest round (none with the
/// setting off) and keeps it under the key; any other run plays what was kept (none when nothing was).
fn round_for(key: String, first_run: bool, on: bool) -> Option<Arc<BTreeMap<String, i32>>> {
    let mut st = store();
    // no save open here (yet): nothing to play, nothing to keep
    st.file.as_ref()?;
    if first_run {
        let id = if on { st.rounds.last().map_or(0, |r| r.id) } else { 0 };
        // nothing ever played on this save: nothing to keep either
        if id == 0 && st.rounds.is_empty() {
            return None;
        }
        match st.kept.insert(key.clone(), id) {
            // the same match run again by the server: nothing new to write
            Some(was) if was == id => {}
            was => {
                if was.is_none() {
                    st.order.push_back(key);
                    while st.order.len() > KEPT {
                        if let Some(old) = st.order.pop_front() {
                            st.kept.remove(&old);
                        }
                    }
                }
                st.unwritten = true;
                if st.written_at.is_none_or(|t| t.elapsed() >= WRITE_EVERY) {
                    write(&mut st);
                }
            }
        }
        return st.powers.get(&id).cloned();
    }
    let id = st.kept.get(&key).copied().unwrap_or(0);
    st.powers.get(&id).cloned()
}

/// The champions to give the buff now: alive, a power of their own, not carrying it yet - (entity id, power).
pub fn missing(champions: &[(usize, String, bool, bool)], power: &BTreeMap<String, i32>) -> Vec<(usize, i32)> {
    champions
        .iter()
        .filter(|(_, _, alive, has)| *alive && !*has)
        .filter_map(|(id, name, _, _)| power.get(name).filter(|p| **p != 0).map(|p| (*id, *p)))
        .collect()
}

/// Every tick of every match (the mod's match hook): the run's balance (decided once) on each of its champions that
/// does not carry it yet - at the start, and again after every respawn.
pub fn on_tick(sim: &mut StableSim<'_>, on: bool) {
    let key = run_key(sim);
    let tick = sim.tick();
    let power = {
        let mut runs = RUNS.lock().unwrap_or_else(PoisonError::into_inner);
        let at = match runs.iter().position(|r| r.key == key) {
            Some(at) => at,
            None => {
                runs.push_back(Run { key, last_tick: tick, power: None });
                while runs.len() > RUNS_KEPT {
                    runs.pop_front();
                }
                runs.len() - 1
            }
        };
        let run = &mut runs[at];
        if tick < run.last_tick {
            // the match started over
            run.power = None;
        }
        run.last_tick = tick;
        if run.power.is_none() {
            run.power = decide_run(sim, on);
        }
        match &run.power {
            Some(Some(p)) => Arc::clone(p),
            _ => return,
        }
    };
    let champions: Vec<(usize, String, bool, bool)> = (0..sim.player_count())
        .filter_map(|i| sim.player_at(i).and_then(|p| p.champion()))
        .map(|c| {
            let has = (0..c.buff_count()).filter_map(|k| c.buff_at(k)).any(|b| b.name() == BUFF);
            (c.id(), c.name().unwrap_or_default(), c.is_alive(), has)
        })
        .collect();
    for (id, pw) in missing(&champions, &power) {
        sim.add_buff(id, &buff(pw));
    }
}

/// Test support: no save, no runs.
pub fn reset_for_tests() {
    close();
    RUNS.lock().unwrap_or_else(PoisonError::into_inner).clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES: Rules = Rules { count: 10, max_step: 10 };

    fn cand(name: &str, win: f32, evidence: f32) -> Candidate<'_> {
        Candidate { name, win, evidence }
    }

    #[test]
    fn the_furthest_outside_the_band_change_most_within_the_caps() {
        let before = BTreeMap::new();
        let out = decide(
            &[cand("maddie", 73.3, 221.0), cand("zeri", 64.0, 422.0), cand("gwen", 46.3, 7.0), cand("ok", 50.5, 300.0), cand("weak", 44.0, 80.0)],
            &before,
            RULES,
        );
        let got: Vec<(&str, i32)> = out.iter().map(|c| (c.champion.as_str(), c.step)).collect();
        // 21.3 points over: -32, held to -10; 12 over: -18 -> -10; 4 under: +6; gwen has too few games; 50.5 is fine
        assert_eq!(got, [("maddie", -10), ("zeri", -10), ("weak", 6)]);
        // a round's count keeps the furthest
        assert_eq!(decide(&[cand("a", 60.0, 50.0), cand("b", 70.0, 50.0)], &before, Rules { count: 1, max_step: 10 })[0].champion, "b");
        // just outside the band: under a step, no change
        assert!(decide(&[cand("a", 52.2, 50.0)], &before, RULES).is_empty());
    }

    #[test]
    fn rounds_add_up_to_the_cap() {
        let mut before = BTreeMap::new();
        before.insert("maddie".to_string(), -25);
        let out = decide(&[cand("maddie", 70.0, 100.0)], &before, RULES);
        assert_eq!((out[0].step, out[0].total), (-5, -CAP), "held to the cap in all");
        before.insert("maddie".to_string(), -CAP);
        assert!(decide(&[cand("maddie", 70.0, 100.0)], &before, RULES).is_empty(), "nothing left to take");
        // nerfed too far: buffed back
        before.insert("maddie".to_string(), -20);
        let out = decide(&[cand("maddie", 44.0, 100.0)], &before, RULES);
        assert_eq!((out[0].step, out[0].total), (6, -14));
    }

    fn logit(p: f32) -> f32 {
        (p / (1.0 - p)).ln()
    }

    fn champ(name: &str, win: f32, evidence: f32) -> crate::meta::Champion {
        crate::meta::Champion { name: name.into(), strength: logit(win / 100.0), evidence, ..crate::meta::Champion::default() }
    }

    #[test]
    fn a_round_is_due_on_a_new_patch_or_many_matches_later() {
        let meta = Meta {
            current: "2027.4.1".into(),
            current_matches: 100,
            champions: vec![champ("maddie", 73.3, 221.0), champ("fine", 50.0, 300.0), champ("weak", 44.0, 80.0)],
            ..Meta::default()
        };
        let first = next_round(&meta, None, RULES).expect("the first round");
        assert_eq!((first.id, first.version.as_str(), first.matches), (1, "2027.4.1", 100));
        assert_eq!(first.power.get("maddie"), Some(&-10));
        assert_eq!(first.power.get("weak"), Some(&6));
        assert!(!first.power.contains_key("fine"));
        // the same patch, a few matches later: none due
        let later = Meta { current_matches: 300, ..meta.clone() };
        assert!(next_round(&later, Some(&first), RULES).is_none());
        // many matches later (the game's own patches off): another, adding up
        let much_later = Meta { current_matches: 100 + ROUND_MATCHES, ..meta.clone() };
        let second = next_round(&much_later, Some(&first), RULES).unwrap();
        assert_eq!((second.id, second.power.get("maddie")), (2, Some(&-20)));
        // a new game patch: due at once
        let next_patch = Meta { current: "2027.5.0".into(), current_matches: 0, ..meta };
        assert_eq!(next_round(&next_patch, Some(&first), RULES).unwrap().version, "2027.5.0");
        // no patch known yet: none
        assert!(next_round(&Meta::default(), None, RULES).is_none());
    }

    #[test]
    fn every_run_of_a_match_plays_the_round_its_first_run_kept() {
        let _serial = crate::tests::serial();
        reset_for_tests();
        let file = std::env::temp_dir().join(format!("pma_balance_test_{}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        // no save open: nothing
        assert_eq!(round_for("k".into(), true, true), None);
        open_file(file.clone());
        // a save with no round: nothing played, nothing kept
        assert_eq!(round_for("k".into(), true, true), None);
        let mut power = BTreeMap::new();
        power.insert("maddie".to_string(), -10);
        publish(Round { id: 1, version: "1.1".into(), power: power.clone(), ..Round::default() });
        // the server's run takes round 1; the client's run of the same match plays it too
        assert_eq!(round_for("k".into(), true, true).as_deref(), Some(&power));
        assert_eq!(round_for("k".into(), false, true).as_deref(), Some(&power));
        // a later round does not change a match already played
        let mut power2 = BTreeMap::new();
        power2.insert("maddie".to_string(), -20);
        publish(Round { id: 2, version: "1.2".into(), power: power2.clone(), ..Round::default() });
        assert_eq!(round_for("k".into(), false, true).as_deref(), Some(&power), "a replay plays its own round");
        // a run of a match nothing was kept for (from before): none
        assert_eq!(round_for("other".into(), false, true), None);
        // with the setting off, a new match plays none - and its replays too
        assert_eq!(round_for("off".into(), true, false), None);
        assert_eq!(round_for("off".into(), false, true), None);
        // the book survives the session
        close();
        open_file(file.clone());
        assert_eq!(rounds().len(), 2);
        assert_eq!(round_for("k".into(), false, true).as_deref(), Some(&power));
        assert_eq!(notes().len(), 2);
        let _ = std::fs::remove_file(&file);
        reset_for_tests();
    }

    #[test]
    fn the_buff_goes_on_living_champions_with_a_power_once() {
        let mut power = BTreeMap::new();
        power.insert("maddie".to_string(), -10);
        power.insert("weak".to_string(), 6);
        let champions = vec![
            (1, "maddie".to_string(), true, false),
            (2, "weak".to_string(), true, true),
            (3, "fine".to_string(), true, false),
            (4, "maddie".to_string(), false, false),
            (5, "weak".to_string(), true, false),
        ];
        // maddie alive without it, the respawned weak (a new entity); not the one carrying it, not the dead
        assert_eq!(missing(&champions, &power), [(1, -10), (5, 6)]);
    }

    #[test]
    fn the_buff_puts_half_on_damage_and_the_rest_on_health() {
        let b = buff(-15);
        assert_eq!((b.attack_mult, b.magic_power_mult, b.hp_mult), (-7, -7, -8));
        assert_eq!(b.name(), BUFF);
        let b = buff(6);
        assert_eq!((b.attack_mult, b.magic_power_mult, b.hp_mult), (3, 3, 3));
    }

    #[test]
    fn a_patch_takes_its_first_round_as_its_notes() {
        let round = |id: u32, version: &str, changes: Vec<(&str, i32)>| Round {
            id,
            version: version.into(),
            changes: changes
                .into_iter()
                .map(|(c, s)| Change { champion: c.into(), win: 50.0, evidence: 50.0, step: s, total: s })
                .collect(),
            ..Round::default()
        };
        let notes = notes_of(&[round(1, "1.2", vec![("a", -5), ("b", 4)]), round(2, "1.2", vec![("c", -3)]), round(3, "1.3", vec![("a", -2)])]);
        assert_eq!(notes.len(), 2);
        assert!(notes[0].is_for("1.2") && notes[0].direction("a") == -1 && notes[0].direction("b") == 1 && notes[0].direction("c") == 0);
        assert!(notes[1].is_for("1.3") && notes[1].direction("a") == -1);
    }

    #[test]
    fn the_news_lists_nerfs_then_buffs_in_the_games_language() {
        let round = Round {
            id: 2,
            version: "2027.4.1".into(),
            changes: vec![
                Change { champion: "tft_maddie".into(), win: 73.3, evidence: 221.0, step: -10, total: -10 },
                Change { champion: "weak".into(), win: 44.0, evidence: 80.0, step: 6, total: 6 },
            ],
            ..Round::default()
        };
        let cn = |n: &str| if n == "tft_maddie" { "麦迪".to_string() } else { "某英雄".to_string() };
        let (title, body) = news(&round, &cn);
        assert_eq!(title, "智能BP 平衡补丁 2027.4.1（第 2 轮）");
        assert!(body.starts_with("【削弱】\n麦迪：胜率 73.3%（目标 48%–52%）→ 强度 -10%（攻击/法强 -5%，生命 -5%），累计 -10%"), "{body}");
        assert!(body.contains("【加强】\n某英雄：胜率 44.0%"), "{body}");
        let (title, body) = news(&round, &|n: &str| n.to_string());
        assert!(title.starts_with("Patch Meta AI balance patch 2027.4.1") && body.starts_with("[Nerfs]"), "{title} / {body}");
    }
}
