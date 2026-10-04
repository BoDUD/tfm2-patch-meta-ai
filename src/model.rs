//! The model. Each champion's win rate in the current in-game patch is a Beta estimate:
//!
//! - **start** (prior): `baseline_games` pseudo-games at 50%, plus up to `carry_games` of the
//!   previous patch at the rate they had (only `changed_carry` of that for champions the patch
//!   notes touched, none for `reworked` ones), shifted by `patch_shift` toward the patch notes'
//!   verdict (buff up, nerf down);
//! - **update**: this patch's competition games, plus solo-rank games weighted by `solo_weight`.
//!
//! From the estimate: a draft value `tanh(logit(p) / edge_scale) * certainty` (certainty =
//! evidence / (evidence + reliability_games)), and a cautious score `p - standard error` that
//! ranks champions into tiers by share (S top `s`%, then A, B, C, the rest D).
//! Pure functions, tested below.

use crate::config::Config;

/// Games and wins.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    pub m: f32,
    pub w: f32,
}

impl Sample {
    pub fn new(m: u32, w: u32) -> Self {
        Self { m: m as f32, w: w as f32 }
    }

    pub fn rate(&self) -> Option<f32> {
        (self.m > 0.0).then(|| self.w / self.m)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    S,
    A,
    B,
    C,
    D,
    NoTier,
}

impl Tier {
    /// The game's names for the tiers (`champion_tiers` values).
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::S => "S",
            Tier::A => "A",
            Tier::B => "B",
            Tier::C => "C",
            Tier::D => "D",
            Tier::NoTier => "NoTier",
        }
    }
}

pub struct Input<'a> {
    pub champion: &'a str,
    /// Competition games of the current / previous patch, solo rank of the current patch.
    pub cur: Sample,
    pub prev: Sample,
    pub solo: Sample,
    /// +1 buffed, -1 nerfed in the current patch notes, 0 not mentioned.
    pub patch_dir: i32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Estimate {
    /// Estimated win rate this patch.
    pub p: f32,
    /// Its standard error.
    pub se: f32,
    /// Where the estimate started (previous patch + patch notes).
    pub start: f32,
    /// Real games behind it: carried-over + this patch (+ weighted solo).
    pub evidence: f32,
    pub carried: f32,
    pub certainty: f32,
    /// Draft value in [-1, 1] before the pick/ban strengths.
    pub value: f32,
}

impl Estimate {
    /// The cautious score used for tiers.
    pub fn score(&self) -> f32 {
        self.p - self.se
    }
}

pub fn estimate(cfg: &Config, inp: &Input<'_>) -> Estimate {
    let dir = inp.patch_dir.signum() as f32;
    let carry_share = if cfg.is_reworked(inp.champion) {
        0.0
    } else if dir != 0.0 {
        cfg.changed_carry
    } else {
        1.0
    };
    let carried = inp.prev.m.min(cfg.carry_games) * carry_share;
    let prev_rate = inp.prev.rate().unwrap_or(0.5);
    let base = cfg.baseline_games;
    let start = ((base * 0.5 + carried * prev_rate) / (base + carried) + dir * cfg.patch_shift)
        .clamp(0.02, 0.98);
    let strength = base + carried;

    let m = inp.cur.m + cfg.solo_weight * inp.solo.m;
    let w = inp.cur.w + cfg.solo_weight * inp.solo.w;
    let alpha = start * strength + w;
    let beta = (1.0 - start) * strength + (m - w).max(0.0);
    let n = alpha + beta;
    let p = if n > 0.0 { alpha / n } else { 0.5 };
    let se = (p * (1.0 - p) / (n + 1.0)).sqrt();

    let evidence = carried + m;
    let certainty = evidence / (evidence + cfg.reliability_games);
    let q = p.clamp(0.01, 0.99);
    let edge = (q / (1.0 - q)).ln();
    let value = (edge / cfg.edge_scale).tanh() * certainty;
    Estimate { p, se, start, evidence, carried, certainty, value: if value.is_finite() { value } else { 0.0 } }
}

/// Ban-score factor for how often a champion is played: `presence` is the share of matches it
/// appeared in, `typical` the share an average champion has. Strong *and* popular champions
/// matter most in bans; the factor runs from 0.5 (rarely seen) to 2 (everywhere).
pub fn presence_factor(presence: f32, typical: f32) -> f32 {
    if typical <= 0.0 || !presence.is_finite() {
        return 1.0;
    }
    (presence / typical).clamp(0.5, 2.0)
}

/// Splits champions (name, cautious score) into tiers by share: the first `s`% S, then A, B,
/// C, the rest D. Ties keep the order they came in.
pub fn tiers_by_share(cfg: &Config, ranked: &mut [(String, f32)]) -> Vec<(String, Tier)> {
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let n = ranked.len() as f32;
    let bounds = [
        cfg.s_percent,
        cfg.s_percent + cfg.a_percent,
        cfg.s_percent + cfg.a_percent + cfg.b_percent,
        cfg.s_percent + cfg.a_percent + cfg.b_percent + cfg.c_percent,
    ];
    ranked
        .iter()
        .enumerate()
        .map(|(i, (name, _))| {
            // the champion's position as a percentage of the list (its middle)
            let at = (i as f32 + 0.5) / n * 100.0;
            let tier = if at <= bounds[0] {
                Tier::S
            } else if at <= bounds[1] {
                Tier::A
            } else if at <= bounds[2] {
                Tier::B
            } else if at <= bounds[3] {
                Tier::C
            } else {
                Tier::D
            };
            (name.clone(), tier)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(cur: Sample, prev: Sample, solo: Sample, patch_dir: i32) -> Input<'static> {
        Input { champion: "fighter", cur, prev, solo, patch_dir }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn nothing_known_is_even() {
        let cfg = Config::default();
        let e = estimate(&cfg, &input(Sample::default(), Sample::default(), Sample::default(), 0));
        assert!(close(e.p, 0.5) && close(e.value, 0.0) && e.evidence == 0.0);
        assert!(close(e.se, (0.25f32 / 21.0).sqrt()));
    }

    #[test]
    fn current_games_move_the_estimate() {
        let cfg = Config::default();
        let e = estimate(&cfg, &input(Sample::new(100, 60), Sample::default(), Sample::default(), 0));
        // 10 + 60 wins out of 20 + 100
        assert!(close(e.p, 70.0 / 120.0));
        assert!(close(e.certainty, 100.0 / 130.0));
        let edge = (70.0f32 / 50.0).ln();
        assert!(close(e.value, (edge / 0.5).tanh() * 100.0 / 130.0));
        assert!(e.value > 0.0);
    }

    #[test]
    fn previous_patch_is_the_starting_point() {
        let cfg = Config::default();
        let prev = Sample::new(200, 130); // 65% last patch, only 40 games carry over
        let fresh = estimate(&cfg, &input(Sample::default(), prev, Sample::default(), 0));
        assert!(close(fresh.carried, 40.0));
        assert!(close(fresh.start, (10.0 + 40.0 * 0.65) / 60.0));
        assert!(close(fresh.p, fresh.start), "no games yet: the estimate is the start");
        // nerfed in the patch notes: less carried over, start moved down
        let nerfed = estimate(&cfg, &input(Sample::default(), prev, Sample::default(), -1));
        assert!(close(nerfed.carried, 20.0));
        assert!(close(nerfed.start, (10.0 + 20.0 * 0.65) / 40.0 - 0.02));
        // reworked: the previous patch is ignored, the patch notes still count
        let mut reworked = cfg.clone();
        reworked.reworked = vec!["fighter".into()];
        let r = estimate(&reworked, &input(Sample::default(), prev, Sample::default(), 1));
        assert!(close(r.carried, 0.0) && close(r.start, 0.52));
        // many games this patch outweigh the start
        let later = estimate(&cfg, &input(Sample::new(400, 180), prev, Sample::default(), 0));
        assert!(later.p < 0.5, "{}", later.p);
    }

    #[test]
    fn solo_rank_counts_half() {
        let cfg = Config::default();
        let e = estimate(&cfg, &input(Sample::default(), Sample::default(), Sample::new(100, 70), 0));
        assert!(close(e.evidence, 50.0));
        assert!(close(e.p, (10.0 + 35.0) / 70.0));
    }

    #[test]
    fn presence() {
        assert_eq!(presence_factor(0.30, 0.15), 2.0);
        assert_eq!(presence_factor(0.15, 0.15), 1.0);
        assert_eq!(presence_factor(0.01, 0.15), 0.5);
        assert_eq!(presence_factor(0.2, 0.0), 1.0);
    }

    #[test]
    fn tiers_split_by_share() {
        let cfg = Config::default();
        let mut ranked: Vec<(String, f32)> =
            (0..20).map(|i| (format!("c{i:02}"), i as f32 / 100.0)).collect();
        let tiers = tiers_by_share(&cfg, &mut ranked);
        let count = |t: Tier| tiers.iter().filter(|(_, x)| *x == t).count();
        assert_eq!((count(Tier::S), count(Tier::A), count(Tier::B), count(Tier::C), count(Tier::D)), (2, 4, 8, 4, 2));
        assert_eq!(tiers[0], ("c19".to_string(), Tier::S));
        assert_eq!(tiers[19], ("c00".to_string(), Tier::D));
        // few champions: still sensible
        let mut three: Vec<(String, f32)> = vec![("a".into(), 0.6), ("b".into(), 0.5), ("c".into(), 0.4)];
        let t = tiers_by_share(&cfg, &mut three);
        assert_eq!(t.iter().map(|(_, x)| *x).collect::<Vec<_>>(), [Tier::A, Tier::B, Tier::C]);
        assert!(tiers_by_share(&cfg, &mut []).is_empty());
    }
}
