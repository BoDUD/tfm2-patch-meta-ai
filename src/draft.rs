//! Ban/pick score hook. The game scores every candidate itself (lanes, its players' champion
//! pools); this adds what the meta model says about the candidate *in this draft* (`advisor`):
//!
//! - **pick**: its strength in the best lane still open, its synergy with the picks already on
//!   the team, its matchups against the enemy's picks, and a cost for one-sided damage;
//! - **ban**: what it would be worth to the enemy, weighted by how often it is played or banned.
//!
//! The value (log-odds) goes through `tanh(value / edge_scale)` so one extreme estimate cannot
//! swamp the game's own reasoning, then `pick_strength` / `ban_strength` scale it.
//!
//! With the position lock on (`poslock`), a pick that no open position of the team can take is
//! scored out of reach - unless nothing on offer can, then nothing is.

use mod_api_stable::{StableDraftContext, StableDraftDecision, StableDraftHook};

use crate::shared::Snapshot;
use crate::{advisor, config, shared};

pub struct MetaDraftHook;

impl StableDraftHook for MetaDraftHook {
    fn id(&self) -> String {
        format!("{}:draft", crate::MOD_ID)
    }

    fn score_ban(&self, ctx: &StableDraftContext<'_>, candidate: usize, _base: f32) -> StableDraftDecision {
        score(ctx, candidate, true)
    }

    fn score_pick(&self, ctx: &StableDraftContext<'_>, candidate: usize, _base: f32) -> StableDraftDecision {
        score(ctx, candidate, false)
    }
}

/// The score of a pick the position lock rules out: below anything the game gives.
pub const LOCKED_SCORE: f32 = -1000.0;

fn score(ctx: &StableDraftContext<'_>, candidate: usize, ban: bool) -> StableDraftDecision {
    let cfg = config::get();
    let lock = !ban && cfg.position_lock_on();
    if !cfg.ban_pick_on() && !lock {
        return StableDraftDecision::Pass;
    }
    let Some(snapshot) = shared::get() else { return StableDraftDecision::Pass };
    let ids = |list: &[usize]| -> Vec<u16> {
        list.iter().filter_map(|id| ctx.champion_name(*id)).filter_map(|n| snapshot.meta.names.get(n)).collect()
    };
    let Some(cand) = ctx.champion_name(candidate).and_then(|n| snapshot.meta.names.get(n)) else {
        return StableDraftDecision::Pass;
    };
    let ally = ids(ctx.ally_picks());
    let offer = ids(ctx.available_champions());
    if lock && !lock_allows(&snapshot, &cfg.lock_rules(), cand, &ally, &offer) {
        return StableDraftDecision::Replace(LOCKED_SCORE);
    }
    if !cfg.ban_pick_on() {
        return StableDraftDecision::Pass;
    }
    let value = value(&snapshot, cand, &ally, &ids(ctx.enemy_picks()), ban, &offer);
    let strength = if ban { cfg.ban_strength } else { cfg.pick_strength };
    decision(amount(value, cfg.edge_scale, strength))
}

/// A team's picks, the offer, and which of the offer the team may pick.
type Pickable = (Vec<u16>, Vec<u16>, Vec<u16>);

thread_local! {
    /// The champions the last team asked about may pick (`poslock::pickable`): the game asks
    /// about every candidate of one decision in a row, with the same picks and offer.
    static PICKABLE: std::cell::RefCell<Option<Pickable>> = const { std::cell::RefCell::new(None) };
}

/// Whether the position lock lets a team that picked `ally` pick `cand` from `available`.
pub fn lock_allows(snapshot: &Snapshot, rules: &crate::poslock::Rules, cand: u16, ally: &[u16], available: &[u16]) -> bool {
    PICKABLE.with(|cell| {
        let mut cache = cell.borrow_mut();
        let fresh = cache.as_ref().is_some_and(|(a, o, _)| a == ally && o == available);
        if !fresh {
            let ok = crate::poslock::pickable(&snapshot.meta, rules, ally, available);
            *cache = Some((ally.to_vec(), available.to_vec(), ok));
        }
        cache.as_ref().is_some_and(|(_, _, ok)| ok.contains(&cand))
    })
}

thread_local! {
    /// The open lanes of the last team asked about: the game scores every candidate of one
    /// decision in a row, with the same picks (hooks may run on several threads).
    static OPEN: std::cell::RefCell<Option<(Vec<u16>, Vec<crate::history::Role>)>> = const { std::cell::RefCell::new(None) };
}

fn open_lanes(snapshot: &Snapshot, picks: &[u16]) -> Vec<crate::history::Role> {
    OPEN.with(|cell| {
        let mut cache = cell.borrow_mut();
        if let Some((key, open)) = cache.as_ref() {
            if key == picks {
                return open.clone();
            }
        }
        let open = advisor::open_lanes(&snapshot.meta, picks);
        *cache = Some((picks.to_vec(), open.clone()));
        open
    })
}

/// The model's value (log-odds) of picking or banning `cand` in this draft. `offer` is what is
/// still on offer (for how counterable a pick is while the other side has picks left).
pub fn value(snapshot: &Snapshot, cand: u16, ally: &[u16], enemy: &[u16], ban: bool, offer: &[u16]) -> f32 {
    let damage = |c: u16| snapshot.damage_of(c);
    let meta = &snapshot.meta;
    if ban {
        let open = open_lanes(snapshot, enemy);
        let v = advisor::ban_value_in(meta, cand, ally, enemy, &[], &damage, &open);
        advisor::with_exposure(meta, v, 5usize.saturating_sub(ally.len()), offer).total
    } else {
        let open = open_lanes(snapshot, ally);
        let v = advisor::pick_value_in(meta, cand, ally, enemy, &[], &damage, &open);
        advisor::with_exposure(meta, v, 5usize.saturating_sub(enemy.len()), offer).total
    }
}

/// What is added to the game's score.
pub fn amount(value: f32, edge_scale: f32, strength: f32) -> f32 {
    (value / edge_scale).tanh() * strength
}

pub fn decision(v: f32) -> StableDraftDecision {
    if v.is_finite() && v.abs() > 1e-4 {
        StableDraftDecision::Add(v)
    } else {
        StableDraftDecision::Pass
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_are_bounded() {
        assert!((amount(0.25, 0.5, 1.0) - 0.5f32.tanh()).abs() < 1e-6);
        assert!(amount(10.0, 0.5, 0.8) <= 0.8 + 1e-6);
        assert!(amount(-0.3, 0.5, 1.0) < 0.0);
    }

    #[test]
    fn the_lock_rules_out_picks_no_open_position_can_take() {
        let _serial = crate::tests::serial();
        crate::poslock::clear();
        let (names, games) = crate::meta::tests::simulate(300, "1.1", |_| 0.0, 2);
        let champions: Vec<String> = crate::meta::tests::NAMES.iter().map(|s| s.to_string()).collect();
        let meta = crate::meta::build(
            &crate::meta::Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.1", warm: None },
            &crate::meta::Settings::default(),
        );
        let snapshot = Snapshot::new(meta, Default::default());
        let id = |n: &str| snapshot.meta.names.get(n).unwrap();
        let top = [true, false, false, false, false];
        crate::poslock::learn("a", top);
        crate::poslock::learn("b", top);
        crate::poslock::learn("c", [false, true, false, false, false]);
        let strict = crate::poslock::Rules { min_games: 100_000, share: 0.5 };
        // a team with "a" (top only) may not take "b" (top only) while "c" fits
        assert!(!lock_allows(&snapshot, &strict, id("b"), &[id("a")], &[id("b"), id("c")]));
        assert!(lock_allows(&snapshot, &strict, id("c"), &[id("a")], &[id("b"), id("c")]));
        // nothing else on offer: allowed (a draft never gets stuck)
        assert!(lock_allows(&snapshot, &strict, id("b"), &[id("a")], &[id("b")]));
        crate::poslock::clear();
    }

    #[test]
    fn decisions() {
        assert!(matches!(decision(0.3), StableDraftDecision::Add(v) if (v - 0.3).abs() < 1e-6));
        assert!(matches!(decision(0.0), StableDraftDecision::Pass));
        assert!(matches!(decision(f32::NAN), StableDraftDecision::Pass));
    }
}
