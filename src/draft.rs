//! Ban/pick score hook. The game scores every candidate itself; this adds the model's
//! adjustment, computed at the last rebuild (see `client`):
//!
//! - **pick**: `value * pick_strength`;
//! - **ban**: `value * ban_strength`, times the presence factor for strong champions (strong and
//!   played everywhere = up to twice, strong but rarely seen = half). Weak champions get a lower
//!   ban score, so bans are not spent on them.
//!
//! `value` is in [-1, 1]: tanh of the champion's log-odds edge, times how sure the estimate is.

use mod_api_stable::{StableDraftContext, StableDraftDecision, StableDraftHook};

use crate::{config, model, shared};

pub struct MetaDraftHook;

impl StableDraftHook for MetaDraftHook {
    fn id(&self) -> String {
        format!("{}:draft", crate::MOD_ID)
    }

    fn score_ban(&self, ctx: &StableDraftContext<'_>, candidate: usize, _base: f32) -> StableDraftDecision {
        adjust(ctx, candidate, |t| &t.ban)
    }

    fn score_pick(&self, ctx: &StableDraftContext<'_>, candidate: usize, _base: f32) -> StableDraftDecision {
        adjust(ctx, candidate, |t| &t.pick)
    }
}

fn adjust(
    ctx: &StableDraftContext<'_>,
    candidate: usize,
    table: impl Fn(&shared::Tables) -> &std::collections::HashMap<String, f32>,
) -> StableDraftDecision {
    if !config::get().ban_pick {
        return StableDraftDecision::Pass;
    }
    let Some(name) = ctx.champion_name(candidate) else { return StableDraftDecision::Pass };
    let Some(tables) = shared::get() else { return StableDraftDecision::Pass };
    decision(table(&tables).get(name).copied().unwrap_or(0.0))
}

pub fn decision(v: f32) -> StableDraftDecision {
    if v.is_finite() && v.abs() > 1e-4 {
        StableDraftDecision::Add(v)
    } else {
        StableDraftDecision::Pass
    }
}

/// The amounts published for one champion: (pick, ban).
pub fn amounts(cfg: &config::Config, value: f32, presence: f32, typical_presence: f32) -> (f32, f32) {
    let pick = value * cfg.pick_strength;
    let ban = if value > 0.0 {
        value * cfg.ban_strength * model::presence_factor(presence, typical_presence)
    } else {
        value * cfg.ban_strength
    };
    (pick, ban)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn pick_and_ban_amounts() {
        let cfg = Config::default();
        let (pick, ban) = amounts(&cfg, 0.5, 0.3, 0.15);
        assert!((pick - 0.5).abs() < 1e-6);
        assert!((ban - 0.5 * 0.8 * 2.0).abs() < 1e-6, "strong and everywhere: double");
        let (_, rare) = amounts(&cfg, 0.5, 0.01, 0.15);
        assert!((rare - 0.5 * 0.8 * 0.5).abs() < 1e-6, "strong but rarely seen: half");
        let (pick, ban) = amounts(&cfg, -0.4, 0.3, 0.15);
        assert!((pick + 0.4).abs() < 1e-6 && (ban + 0.32).abs() < 1e-6, "weak: lower, no presence");
    }

    #[test]
    fn decisions() {
        assert!(matches!(decision(0.3), StableDraftDecision::Add(v) if (v - 0.3).abs() < 1e-6));
        assert!(matches!(decision(0.0), StableDraftDecision::Pass));
        assert!(matches!(decision(f32::NAN), StableDraftDecision::Pass));
    }
}
