//! The champions' stats as a starting point for their strength. A save has a few hundred
//! matches over almost two hundred champions: most champions are seen in a handful of games,
//! many in none, and League's tiers say little about this game (`lol_prior=auto` often settles
//! on 0). What every champion does have is its stat block - attack, magic power, health,
//! armour, magic resistance and move speed, and how much each grows per level.
//!
//! The model (`meta`) learns how much each stat is worth: a champion's strength starts from
//! `sum weight[k] * z[k] + group` - its stats standardised within its category (a mage's attack
//! is compared with other mages'), one weight per stat shared by every champion, and one offset
//! per category - and its own games move it from there. A dozen weights that every match
//! speaks to settle long before the champions do, so a champion seen in few games is rated by
//! what champions with stats like its own did.

use std::collections::HashMap;

use crate::history::Names;

pub const STATS: usize = 6;
/// Base stats, then their growth.
pub const FEATURES: usize = 2 * STATS;
pub const STAT_NAMES_ZH: [&str; STATS] = ["攻击", "法强", "生命", "护甲", "魔抗", "移速"];

/// One champion's category (`ChampionCategoryV1` code) and stat block.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub category: Option<u8>,
    pub base: [f32; STATS],
    pub growth: [f32; STATS],
}

impl Stats {
    pub fn from_game(category: Option<u8>, base: &mod_api_stable::StatV1, growth: &mod_api_stable::StatV1) -> Self {
        let arr = |s: &mod_api_stable::StatV1| {
            [s.attack, s.magic_power, s.hp, s.defence, s.magic_resistance, s.move_speed].map(|v| v as f32)
        };
        Self { category, base: arr(base), growth: arr(growth) }
    }

    fn values(&self) -> [f32; FEATURES] {
        let mut out = [0.0; FEATURES];
        out[..STATS].copy_from_slice(&self.base);
        out[STATS..].copy_from_slice(&self.growth);
        out
    }
}

/// The group of champions whose category has fewer members than this (or none): standardised
/// against the whole pool.
pub const POOL: u8 = u8::MAX;
const MIN_GROUP: usize = 5;
/// A stat further out than this many standard deviations counts as this many.
const Z_CAP: f32 = 3.0;

/// Per champion id (`names` order, the first `count`): its group (category, or [`POOL`]) and its
/// stats standardised within that group. `None`: stats not known.
pub fn features(names: &Names, count: u16, stats: &HashMap<String, Stats>) -> Vec<Option<(u8, [f32; FEATURES])>> {
    let known: Vec<Option<&Stats>> = (0..count).map(|c| stats.get(names.name(c))).collect();
    let mut members: HashMap<u8, usize> = HashMap::new();
    for s in known.iter().flatten() {
        *members.entry(s.category.unwrap_or(POOL)).or_default() += 1;
    }
    let group_of = |s: &Stats| {
        let g = s.category.unwrap_or(POOL);
        if members.get(&g).copied().unwrap_or(0) >= MIN_GROUP { g } else { POOL }
    };
    // mean and spread of every stat per group, the whole pool under POOL
    let mut moments: HashMap<u8, ([f64; FEATURES], [f64; FEATURES], f64)> = HashMap::new();
    for s in known.iter().flatten() {
        let v = s.values();
        for g in [group_of(s), POOL].into_iter().collect::<std::collections::BTreeSet<_>>() {
            let (sum, sq, n) = moments.entry(g).or_insert(([0.0; FEATURES], [0.0; FEATURES], 0.0));
            for k in 0..FEATURES {
                sum[k] += v[k] as f64;
                sq[k] += (v[k] as f64) * (v[k] as f64);
            }
            *n += 1.0;
        }
    }
    known
        .iter()
        .map(|s| {
            let s = (*s)?;
            let g = group_of(s);
            let (sum, sq, n) = moments.get(&g)?;
            let v = s.values();
            let mut z = [0.0f32; FEATURES];
            for k in 0..FEATURES {
                let mean = sum[k] / n;
                let sd = (sq[k] / n - mean * mean).max(0.0).sqrt();
                z[k] = if sd > 1e-6 { (((v[k] as f64 - mean) / sd) as f32).clamp(-Z_CAP, Z_CAP) } else { 0.0 };
            }
            Some((g, z))
        })
        .collect()
}

/// The name of a feature (`攻击`, `成长 攻击`...).
pub fn feature_name(k: usize) -> String {
    if k < STATS {
        STAT_NAMES_ZH[k].to_string()
    } else {
        format!("{}成长", STAT_NAMES_ZH[k - STATS])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standardises_within_the_category() {
        let mut names = Names::default();
        let mut stats = HashMap::new();
        // five mages with attack 10..50, one marksman (too few: compared with everyone)
        for i in 0..5u16 {
            let n = format!("m{i}");
            names.id(&n);
            stats.insert(n, Stats { category: Some(2), base: [10.0 * (i + 1) as f32, 40.0, 900.0, 20.0, 20.0, 900.0], growth: [0.0; STATS] });
        }
        names.id("adc");
        stats.insert("adc".into(), Stats { category: Some(1), base: [100.0, 0.0, 900.0, 20.0, 15.0, 900.0], growth: [0.0; STATS] });
        names.id("unknown");
        let f = features(&names, names.len() as u16, &stats);
        let (g0, z0) = f[0].unwrap();
        let (g4, z4) = f[4].unwrap();
        assert_eq!((g0, g4), (2, 2));
        assert!(z0[0] < -1.0 && z4[0] > 1.0, "{z0:?} {z4:?}");
        assert_eq!(z0[1], 0.0, "the same for every mage: no spread");
        let (ga, za) = f[5].unwrap();
        assert_eq!(ga, POOL);
        assert!(za[0] > 1.5, "the most attack of the pool: {za:?}");
        assert!(f[6].is_none());
        assert_eq!(feature_name(0), "攻击");
        assert_eq!(feature_name(STATS + 2), "生命成长");
    }
}
