//! Ban/pick score hook. The game scores every candidate itself (lanes, its players' champion
//! pools); this adds what the meta model says about the candidate *in this draft* (`advisor`):
//!
//! - **pick**: its strength in the best lane still open, its synergy with the picks already on
//!   the team, its matchups against the enemy's picks, and a cost for one-sided damage;
//! - **ban**: what it would be worth to the enemy, weighted by how often it is played or banned.
//!
//! The value (log-odds) goes through `tanh(value / edge_scale)` so one extreme estimate cannot
//! swamp the game's own reasoning, then `pick_strength` / `ban_strength` scale it.

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

fn score(ctx: &StableDraftContext<'_>, candidate: usize, ban: bool) -> StableDraftDecision {
    let cfg = config::get();
    if !cfg.ban_pick_on() {
        return StableDraftDecision::Pass;
    }
    let Some(snapshot) = shared::get() else { return StableDraftDecision::Pass };
    let ids = |list: &[usize]| -> Vec<u16> {
        list.iter().filter_map(|id| ctx.champion_name(*id)).filter_map(|n| snapshot.meta.names.get(n)).collect()
    };
    let Some(cand) = ctx.champion_name(candidate).and_then(|n| snapshot.meta.names.get(n)) else {
        return StableDraftDecision::Pass;
    };
    let value = value(&snapshot, cand, &ids(ctx.ally_picks()), &ids(ctx.enemy_picks()), ban);
    let strength = if ban { cfg.ban_strength } else { cfg.pick_strength };
    decision(amount(value, cfg.edge_scale, strength))
}

/// The model's value (log-odds) of picking or banning `cand` in this draft.
pub fn value(snapshot: &Snapshot, cand: u16, ally: &[u16], enemy: &[u16], ban: bool) -> f32 {
    let damage = |c: u16| snapshot.damage_of(c);
    if ban {
        advisor::ban_value(&snapshot.meta, cand, ally, enemy, &[], &damage).total
    } else {
        advisor::pick_value(&snapshot.meta, cand, ally, enemy, &[], &damage).total
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
    fn decisions() {
        assert!(matches!(decision(0.3), StableDraftDecision::Add(v) if (v - 0.3).abs() < 1e-6));
        assert!(matches!(decision(0.0), StableDraftDecision::Pass));
        assert!(matches!(decision(f32::NAN), StableDraftDecision::Pass));
    }
}
