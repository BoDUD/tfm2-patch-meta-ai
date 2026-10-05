//! Tiers: the game's tier names, and splitting ranked champions into S/A/B/C/D by share; plus
//! the presence factor bans are weighted by. The model itself is in `meta`.

use crate::config::Config;

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
    split_by_share(ranked, [cfg.s_percent, cfg.a_percent, cfg.b_percent, cfg.c_percent])
}

/// The same split with explicit shares (S, A, B, C in percent).
pub fn split_by_share(ranked: &mut [(String, f32)], shares: [f32; 4]) -> Vec<(String, Tier)> {
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let n = ranked.len() as f32;
    let bounds = [
        shares[0],
        shares[0] + shares[1],
        shares[0] + shares[1] + shares[2],
        shares[0] + shares[1] + shares[2] + shares[3],
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
