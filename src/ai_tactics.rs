//! `ai_tactics`: AI teams the model drafted (`ai_bp` on) also play the strategy the model
//! rates best for their five champions (`plan::tactics_for`, the tactics screen's ★), set when
//! the match starts (`StableMatchHook::on_match_start`, `strategy_set_json`). A setting keeps the
//! team's own option unless the advised one is worth [`SWITCH_MARGIN`] more by the model; the
//! player's team always keeps the player's own strategy, auto BP or not.
//!
//! A match is simulated more than once: the server plays it out first (`ServerPresim`, the
//! result), the client plays it again to show it, and replays play it again later. Every run
//! must start alike, or what is shown is not what happened. So the strategy is decided once, by
//! the server's run, and kept in `ai_tactics.json` in the mod folder under the ten champions;
//! every later run of those champions applies the same (and a run of a match from before, with
//! nothing kept, is left alone).
//!
//! The match records may keep the strategy the team had before the match started. What was
//! applied is therefore also handed to the record reader (`scan`), which puts it in place of the
//! record's, so the model learns what was played ([`override_for`]).
//!
//! An option that carries data (`morgard_use` `Split131` / `Split14`: which lanes split) is only
//! set when the team already plays that variant; every other option is a plain name.
//!
//! Some settings stay the team's own coach's ([`LEFT_TO_THE_COACH`]): `game_finish`, whose
//! Aggressive the model rated best for nearly every line-up, which cut the matches short. A match
//! decided before a setting was left to the coach plays what it played then on every later run.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, PoisonError};

use mod_api_stable::{SimOriginKindV1, StableMatchHook, StableSim};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{config, diag, paths, plan, shared};

pub const FILE: &str = "ai_tactics.json";
/// Matches kept in the file at most (the newest).
const KEPT: usize = 4000;
/// Matches written to `diag.log` per game session besides the player's own.
const LOGGED: usize = 4;

/// What the advised option must be worth over the team's own (log-odds) before it is set: on a
/// save's matches the model was not fitted on, the side playing the advice won about 52% where
/// it was ahead by any margin, about 55% where it was ahead by this much.
const SWITCH_MARGIN: f32 = 0.1;

/// Settings `ai_tactics` never sets: the team's own coach chooses them. `game_finish`: with the
/// model's Aggressive on 97% of the AI sides the matches ended about three minutes sooner - on a
/// real save's patch 2027.2.0, 197 matches with `ai_tactics` on averaged 11.5 minutes, 155 with
/// it off 14.3, the coaches choosing Aggressive for 16% of the sides (user 2026-11-07: keep the
/// game's own pace, the other settings as before).
pub const LEFT_TO_THE_COACH: &[&str] = &["game_finish"];

/// The advice `ai_tactics` acts on: every setting but those [`LEFT_TO_THE_COACH`].
fn acted_on(advice: Vec<plan::TacticAdvice>) -> impl Iterator<Item = plan::TacticAdvice> {
    advice.into_iter().filter(|a| !LEFT_TO_THE_COACH.contains(&a.setting.as_str()))
}

/// One side's applied strategy: (setting, option).
pub type Strategy = Vec<(String, String)>;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Entry {
    /// `blue champions|red champions`, each sorted, comma separated.
    key: String,
    sides: [Option<Strategy>; 2],
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    v: u32,
    matches: Vec<Entry>,
}

#[derive(Default)]
struct Store {
    loaded: bool,
    by_key: HashMap<String, [Option<Strategy>; 2]>,
    order: VecDeque<String>,
    logged: usize,
}

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| Mutex::new(Store::default()));

/// The key of a match: both sides' champions, each sorted.
pub fn key(sides: &[Vec<String>; 2]) -> String {
    let side = |s: &Vec<String>| {
        let mut s = s.clone();
        s.sort();
        s.join(",")
    };
    format!("{}|{}", side(&sides[0]), side(&sides[1]))
}

fn load(st: &mut Store) {
    if st.loaded {
        return;
    }
    st.loaded = true;
    let Ok(text) = std::fs::read_to_string(paths::mod_dir().join(FILE)) else { return };
    let Ok(file) = serde_json::from_str::<File>(&text) else {
        diag::log(&format!("[tactics] {FILE} is not readable: starting a new one"));
        return;
    };
    for e in file.matches {
        st.order.push_back(e.key.clone());
        st.by_key.insert(e.key, e.sides);
    }
}

fn save(st: &Store) {
    let matches: Vec<Entry> = st
        .order
        .iter()
        .filter_map(|k| Some(Entry { key: k.clone(), sides: st.by_key.get(k)?.clone() }))
        .collect();
    if let Ok(text) = serde_json::to_string(&File { v: 1, matches }) {
        let _ = std::fs::write(paths::mod_dir().join(FILE), text);
    }
}

fn remember(st: &mut Store, key: String, sides: [Option<Strategy>; 2]) {
    if st.by_key.insert(key.clone(), sides).is_none() {
        st.order.push_back(key);
        while st.order.len() > KEPT {
            if let Some(old) = st.order.pop_front() {
                st.by_key.remove(&old);
            }
        }
    }
    save(st);
}

/// What `ai_tactics` applied to a match with these champions (blue, red; any order inside a
/// side): the record reader puts it in place of the record's strategy.
pub fn override_for(blue: &[String], red: &[String]) -> [Option<Strategy>; 2] {
    let mut st = STORE.lock().unwrap_or_else(PoisonError::into_inner);
    load(&mut st);
    st.by_key.get(&key(&[blue.to_vec(), red.to_vec()])).cloned().unwrap_or_default()
}

/// A record's strategy with what was applied in place of the record's own options.
pub fn merged(record: &[(String, String)], applied: &Strategy) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = record
        .iter()
        .map(|(s, o)| match applied.iter().find(|(a, _)| a == s) {
            Some((_, option)) => (s.clone(), option.clone()),
            None => (s.clone(), o.clone()),
        })
        .collect();
    for (s, o) in applied {
        if !out.iter().any(|(x, _)| x == s) {
            out.push((s.clone(), o.clone()));
        }
    }
    out.sort();
    out
}

/// Options that carry data the model does not choose (which lanes split).
fn carries_data(option: &str) -> bool {
    option.starts_with("Split")
}

/// Whether a setting that holds `current` already plays `option` (a variant with its data
/// counts by its name).
pub fn already(current: Option<&Value>, option: &str) -> bool {
    match current {
        Some(Value::Object(m)) => m.len() == 1 && m.contains_key(option),
        Some(Value::String(s)) => s == option,
        _ => false,
    }
}

/// The JSON to write for `option` over a setting that holds `current` (`None`: leave it -
/// already so, or an option whose data the model does not choose).
pub fn value_for(current: Option<&Value>, option: &str) -> Option<String> {
    if already(current, option) || carries_data(option) {
        None
    } else {
        Some(Value::String(option.to_string()).to_string())
    }
}

/// The ★ strategy for these champions (by the model's ids).
fn advised(snapshot: &shared::Snapshot, champions: &[String]) -> Vec<plan::TacticAdvice> {
    let ids: Vec<u16> = champions.iter().filter_map(|c| snapshot.meta.names.get(c)).collect();
    plan::tactics_for(&snapshot.meta, &ids)
}

/// The option a setting holds (a variant with data by its name).
fn option_of(current: Option<&Value>) -> Option<&str> {
    match current? {
        Value::String(s) => Some(s),
        Value::Object(m) if m.len() == 1 => m.keys().next().map(String::as_str),
        _ => None,
    }
}

/// Whether the advice is worth more than the team's own option by [`SWITCH_MARGIN`] (an own
/// option the model does not rate: yes).
fn worth_switching(advice: &plan::TacticAdvice, current: Option<&str>) -> bool {
    let Some(best) = advice.options.first() else { return false };
    match current.and_then(|c| advice.options.iter().find(|o| o.0 == c)) {
        Some(own) => best.1 - own.1 >= SWITCH_MARGIN,
        None => true,
    }
}

/// The player's side in this match (`None`: not the player's last draft) and whether its picks
/// were an auto BP.
fn players_side(sides: &[Vec<String>; 2], player: Option<&shared::PlayerDraft>) -> Option<(usize, bool)> {
    let p = player?;
    let k = |a: &Vec<String>, b: &Vec<String>| key(&[a.clone(), b.clone()]);
    let here = k(&sides[0], &sides[1]);
    if here == k(&p.sides[0], &p.sides[1]) {
        Some((p.side, p.auto))
    } else if here == k(&p.sides[1], &p.sides[0]) {
        Some((1 - p.side, p.auto))
    } else {
        None
    }
}

/// Which sides of a match play the model's strategy: every AI side when `ai_bp` is on, never
/// the player's (after an auto BP too: the player's own strategy stays).
fn model_sides(sides: &[Vec<String>; 2], player: Option<&shared::PlayerDraft>, ai_bp: bool) -> [bool; 2] {
    let mut out = [ai_bp; 2];
    if let Some((side, _)) = players_side(sides, player) {
        out[side] = false;
    }
    out
}

pub struct TacticsHook;

/// The first call of each kind is described in `diag.log` (what the match shows of its players).
static START_LOGGED: AtomicBool = AtomicBool::new(false);
static TICK_LOGGED: AtomicBool = AtomicBool::new(false);

/// The ticks a match's strategy is tried in, when its champions are not there at the start.
const TICK_WINDOW: usize = 600;

/// The runs seen lately: the run (origin) -> (the last tick seen, done).
static RUNS: LazyLock<Mutex<VecDeque<(RunKey, usize, bool)>>> = LazyLock::new(|| Mutex::new(VecDeque::new()));
const RUNS_KEPT: usize = 64;

type RunKey = (u32, u64, u64, u64);

fn run_key(sim: &StableSim<'_>) -> RunKey {
    sim.sim_origin().map_or((u32::MAX, u64::MAX, u64::MAX, u64::MAX), |o| (o.kind, o.match_id, o.replay_id, o.set_index))
}

fn origin_label(origin: Option<mod_api_stable::SimOriginV1>) -> String {
    match origin.and_then(|o| SimOriginKindV1::from_code(o.kind)) {
        Some(k) => format!("{k:?}"),
        None => "origin unknown".to_string(),
    }
}

/// What the match shows of its players (diag.log, once per kind of call).
fn describe(sim: &StableSim<'_>) -> String {
    let players: Vec<String> = (0..sim.player_count())
        .filter_map(|i| sim.player_at(i))
        .map(|p| format!("{}:{}", p.team(), p.champion().and_then(|e| e.name()).unwrap_or_else(|| "?".into())))
        .collect();
    format!("tick {}, {}, {} players [{}]", sim.tick(), origin_label(sim.sim_origin()), players.len(), players.join(" "))
}

/// Tries one run of a match on this tick: `true` when it is done with (set, kept, or left
/// alone), `false` to try again on a later tick.
fn attempt(sim: &mut StableSim<'_>, at_start: bool) -> bool {
    let run = run_key(sim);
    let tick = sim.tick();
    {
        let mut runs = RUNS.lock().unwrap_or_else(PoisonError::into_inner);
        match runs.iter_mut().find(|(k, _, _)| *k == run) {
            // the same run again: done, unless the tick went back (the match started over)
            Some((_, last, done)) => {
                if tick < *last {
                    *done = false;
                }
                *last = tick;
                if *done {
                    return true;
                }
            }
            None => {
                runs.push_back((run, tick, false));
                while runs.len() > RUNS_KEPT {
                    runs.pop_front();
                }
            }
        }
    }
    let done = apply(sim);
    if done {
        let mut runs = RUNS.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(r) = runs.iter_mut().find(|(k, _, _)| *k == run) {
            r.2 = true;
        }
    } else if !at_start && tick >= TICK_WINDOW {
        diag::log_once("tactics-never", &format!("[tactics] no five champions a side by tick {TICK_WINDOW}: {}", describe(sim)));
    }
    done
}

impl StableMatchHook for TacticsHook {
    fn on_match_start(&self, sim: &mut StableSim<'_>) {
        if !config::get().ai_tactics {
            return;
        }
        if !START_LOGGED.swap(true, Ordering::Relaxed) {
            diag::log(&format!("[tactics] match start: {}", describe(sim)));
        }
        attempt(sim, true);
    }

    fn on_match_tick(&self, sim: &mut StableSim<'_>, _rng_seed: u64) {
        if sim.tick() > TICK_WINDOW || !config::get().ai_tactics {
            return;
        }
        if !TICK_LOGGED.swap(true, Ordering::Relaxed) {
            diag::log(&format!("[tactics] match tick: {}", describe(sim)));
        }
        attempt(sim, false);
    }
}

/// Sets (or keeps, or leaves alone) one run's strategy - see the module notes. `false`: the
/// champions are not all there yet.
fn apply(sim: &mut StableSim<'_>) -> bool {
    let cfg = config::get();
    if !cfg.ai_tactics {
        return true;
    }
    // the five champions of each team (team 0 = blue)
    let mut sides: [Vec<String>; 2] = [Vec::new(), Vec::new()];
    for i in 0..sim.player_count() {
        let Some(p) = sim.player_at(i) else { continue };
        let team = p.team();
        let name = p.champion().and_then(|e| e.name());
        if let (Some(side), Some(name)) = (sides.get_mut(team), name) {
            side.push(name);
        }
    }
    if sides.iter().any(|s| s.len() != 5) {
        // the champions may not be there yet: try again on a later tick
        return false;
    }
    let key = key(&sides);
    let origin = sim.sim_origin();
    let first_run = origin.is_some_and(|o| SimOriginKindV1::from_code(o.kind) == Some(SimOriginKindV1::ServerPresim));
    let mut st = STORE.lock().unwrap_or_else(PoisonError::into_inner);
    load(&mut st);
    let kept = st.by_key.get(&key).cloned();
    let plan: [Option<Strategy>; 2] = match kept {
        Some(kept) => kept,
        // a run that is not the server's first, of a match nothing was decided for: leave it
        None if !first_run => return true,
        None => {
            let Some(snapshot) = shared::get() else {
                remember(&mut st, key, [None, None]);
                return true;
            };
            let player = shared::player_draft();
            let model = model_sides(&sides, player.as_deref(), cfg.ai_bp);
            let mut plan: [Option<Strategy>; 2] = [None, None];
            for side in 0..2 {
                if !model[side] {
                    continue;
                }
                let mut applied = Strategy::new();
                for advice in acted_on(advised(&snapshot, &sides[side])) {
                    let (setting, option) = (advice.setting.as_str(), advice.option.as_str());
                    let current = sim.strategy_get_json(side, setting).and_then(|j| serde_json::from_str::<Value>(&j).ok());
                    if !worth_switching(&advice, option_of(current.as_ref())) {
                        continue;
                    }
                    if let Some(json) = value_for(current.as_ref(), option) {
                        if sim.strategy_set_json(side, setting, &json) {
                            applied.push((setting.to_string(), option.to_string()));
                        } else {
                            diag::log_once(
                                &format!("tactics-reject-{setting}"),
                                &format!("[tactics] the game did not take {setting} = {json} (current {current:?})"),
                            );
                        }
                    } else if already(current.as_ref(), option) {
                        applied.push((setting.to_string(), option.to_string()));
                    }
                }
                plan[side] = Some(applied);
            }
            let mine = players_side(&sides, player.as_deref());
            let ours = mine.is_some();
            if ours || st.logged < LOGGED {
                st.logged += 1;
                let show = |s: &Option<Strategy>| match s {
                    Some(list) => list.iter().map(|(a, b)| format!("{a}={b}")).collect::<Vec<_>>().join(" "),
                    None => "-".to_string(),
                };
                diag::log(&format!(
                    "[tactics]{} blue [{}]: {} | red [{}]: {}{}",
                    if ours { " your match," } else { "" },
                    sides[0].join(","),
                    show(&plan[0]),
                    sides[1].join(","),
                    show(&plan[1]),
                    match mine {
                        Some((side, auto)) => format!(
                            " (you: {}{}, your own tactics)",
                            ["blue", "red"][side],
                            if auto && cfg.coach { ", auto BP" } else { "" }
                        ),
                        None => String::new(),
                    }
                ));
            }
            remember(&mut st, key, plan.clone());
            return true;
        }
    };
    drop(st);
    // a later run of a decided match: the same strategy again
    diag::log_once(
        "tactics-again",
        &format!("[tactics] a later run of a decided match ({}): the same strategy again", origin_label(origin)),
    );
    for (side, strategy) in plan.iter().enumerate() {
        for (setting, option) in strategy.iter().flatten() {
            let current = sim.strategy_get_json(side, setting).and_then(|j| serde_json::from_str::<Value>(&j).ok());
            if let Some(json) = value_for(current.as_ref(), option) {
                sim.strategy_set_json(side, setting, &json);
            }
        }
    }
    true
}

/// Test support: forget the kept matches (the file stays).
#[doc(hidden)]
pub fn reset_for_tests() {
    let mut st = STORE.lock().unwrap_or_else(PoisonError::into_inner);
    *st = Store::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn five(prefix: &str) -> Vec<String> {
        (0..5).map(|i| format!("{prefix}{i}")).collect()
    }

    #[test]
    fn options_and_data() {
        assert_eq!(value_for(Some(&Value::String("Stable".into())), "Flexible").as_deref(), Some("\"Flexible\""));
        assert_eq!(value_for(Some(&Value::String("Flexible".into())), "Flexible"), None, "already");
        let split: Value = serde_json::from_str(r#"{"Split131": {"position1": "Top", "position2": "Bottom"}}"#).unwrap();
        assert_eq!(value_for(Some(&split), "Split131"), None, "already that variant, its data kept");
        assert_eq!(value_for(Some(&split), "Gather").as_deref(), Some("\"Gather\""));
        assert_eq!(value_for(Some(&Value::String("Gather".into())), "Split14"), None, "a variant with data is not made up");
        let applied = vec![("game_finish".to_string(), "Flexible".to_string()), ("new".to_string(), "X".to_string())];
        let record = vec![("early_jungle".to_string(), "Ganking".to_string()), ("game_finish".to_string(), "Stable".to_string())];
        assert_eq!(
            merged(&record, &applied),
            vec![("early_jungle".into(), "Ganking".into()), ("game_finish".into(), "Flexible".into()), ("new".into(), "X".into())]
        );
    }

    #[test]
    fn which_sides_the_model_drafted() {
        let (a, b) = (five("a"), five("b"));
        let sides = [a.clone(), b.clone()];
        // not the player's match: every AI side with ai_bp
        assert_eq!(model_sides(&sides, None, true), [true, true]);
        assert_eq!(model_sides(&sides, None, false), [false, false]);
        // the player's match, the player blue: red only, after an auto BP too
        let mut p = shared::PlayerDraft { sides: [a.clone(), b.clone()], side: 0, auto: false };
        assert_eq!(model_sides(&sides, Some(&p), true), [false, true]);
        p.auto = true;
        assert_eq!(model_sides(&sides, Some(&p), true), [false, true], "the player's own strategy after an auto BP");
        // the same match with the sides the other way round
        let flipped = [b, a];
        assert_eq!(model_sides(&flipped, Some(&p), true), [true, false]);
    }

    #[test]
    fn the_advice_must_be_clearly_better() {
        let advice = |options: &[(&str, f32)]| plan::TacticAdvice {
            setting: "game_finish".into(),
            option: options[0].0.into(),
            gain: 0.0,
            options: options.iter().map(|(o, v)| (o.to_string(), *v)).collect(),
        };
        let close = advice(&[("Aggressive", 0.05), ("Flexible", 0.0), ("Stable", -0.2)]);
        assert!(!worth_switching(&close, Some("Flexible")), "within the margin: the team's own stays");
        assert!(worth_switching(&close, Some("Stable")));
        assert!(worth_switching(&close, None), "no option read");
        assert!(worth_switching(&close, Some("Unknown")), "an option the model does not rate");
        let split: Value = serde_json::from_str(r#"{"Split131": {"position1": "Top"}}"#).unwrap();
        assert_eq!(option_of(Some(&split)), Some("Split131"));
        assert_eq!(option_of(Some(&Value::String("Gather".into()))), Some("Gather"));
    }

    #[test]
    fn game_finish_stays_the_coachs() {
        let advice = |setting: &str| plan::TacticAdvice {
            setting: setting.into(),
            option: "Aggressive".into(),
            gain: 0.5,
            options: vec![("Aggressive".into(), 0.5), ("Stable".into(), -0.5)],
        };
        let set: Vec<String> =
            acted_on(vec![advice("early_jungle"), advice("game_finish"), advice("object_battle")]).map(|a| a.setting).collect();
        assert_eq!(set, ["early_jungle", "object_battle"]);
    }

    #[test]
    fn keys_ignore_the_order_inside_a_side() {
        let (mut a, b) = (five("a"), five("b"));
        let k = key(&[a.clone(), b.clone()]);
        a.reverse();
        assert_eq!(key(&[a, b.clone()]), k);
        assert_ne!(key(&[b, five("a")]), k, "blue and red are not the same");
    }
}
