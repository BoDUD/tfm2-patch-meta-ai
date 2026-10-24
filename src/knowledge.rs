//! What League of Legends teaches about a champion, built in for the League champions (the
//! `league` mod) - what the save's few matches cannot tell yet:
//!
//! - **positions** (`presets/league_positions.ini`, the same list Smart Position Lock ships): the
//!   lanes a champion is played in, its main one first. The lane reads use them as a prior until
//!   the save has seen enough of the champion;
//! - **profile** (`presets/league_comp.ini`): its damage type and how much it brings of what a
//!   team composition needs - frontline, engage, peel, poke, dive, late-game carry, crowd control
//!   (0-2 each). The model fits what a team lacking each of them loses (`comp`), so a draft is
//!   valued as a composition, not five champions;
//! - **tier**: its competitive priority in League (S-D), a small prior on its strength;
//! - **pairs**: well-known combos and counters, small priors on the model's synergy and
//!   matchup effects.
//!
//! Every one of these is only where the model starts: the save's matches move it.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::history::Role;

const POSITIONS: &str = include_str!("../presets/league_positions.ini");
const COMP: &str = include_str!("../presets/league_comp.ini");

/// Damage type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dmg {
    Ad,
    Ap,
    Mixed,
}

/// A champion's profile (0-2 per trait).
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub dmg: Dmg,
    pub front: f32,
    pub engage: f32,
    pub peel: f32,
    pub poke: f32,
    pub dive: f32,
    pub carry: f32,
    pub cc: f32,
    /// Competitive priority: 2 = S ... -2 = D.
    pub tier: i8,
}

#[derive(Debug, Default)]
pub struct Knowledge {
    pub profiles: HashMap<String, Profile>,
    /// The lanes each champion is played in, main one first.
    pub positions: HashMap<String, Vec<Role>>,
    /// Ally pairs (1 good, 2 signature combo).
    pub synergy: Vec<(String, String, u8)>,
    /// `(a, b, n)`: a is favoured against b (1 favoured, 2 hard counter).
    pub counters: Vec<(String, String, u8)>,
}

pub fn get() -> &'static Knowledge {
    static K: OnceLock<Knowledge> = OnceLock::new();
    K.get_or_init(|| parse(POSITIONS, COMP))
}

fn lines(text: &str) -> impl Iterator<Item = (String, &str)> + '_ {
    let mut section = String::new();
    text.lines().filter_map(move |raw| {
        let line = raw.split(';').next().unwrap_or("").trim();
        if line.is_empty() {
            return None;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_ascii_lowercase();
            return None;
        }
        Some((section.clone(), line))
    })
}

fn profile(spec: &str) -> Option<Profile> {
    let mut p = Profile { dmg: Dmg::Mixed, front: 0.0, engage: 0.0, peel: 0.0, poke: 0.0, dive: 0.0, carry: 0.0, cc: 0.0, tier: 0 };
    for part in spec.split_whitespace() {
        let (k, v) = part.split_once('=')?;
        let n = || v.parse::<f32>().ok().map(|x| x.clamp(0.0, 2.0));
        match k.trim().to_ascii_lowercase().as_str() {
            "dmg" => {
                p.dmg = match v.to_ascii_uppercase().as_str() {
                    "AD" => Dmg::Ad,
                    "AP" => Dmg::Ap,
                    _ => Dmg::Mixed,
                }
            }
            "front" => p.front = n()?,
            "engage" => p.engage = n()?,
            "peel" => p.peel = n()?,
            "poke" => p.poke = n()?,
            "dive" => p.dive = n()?,
            "carry" => p.carry = n()?,
            "cc" => p.cc = n()?,
            "tier" => {
                p.tier = match v.to_ascii_uppercase().as_str() {
                    "S" => 2,
                    "A" => 1,
                    "C" => -1,
                    "D" => -2,
                    _ => 0,
                }
            }
            _ => {}
        }
    }
    Some(p)
}

pub fn parse(positions: &str, comp: &str) -> Knowledge {
    let mut k = Knowledge::default();
    for (section, line) in lines(positions) {
        if section != "positions" {
            continue;
        }
        if let Some((id, list)) = line.split_once('=') {
            let roles: Vec<Role> = list.split(',').filter_map(Role::parse).collect();
            if !roles.is_empty() {
                k.positions.insert(id.trim().to_string(), roles);
            }
        }
    }
    for (section, line) in lines(comp) {
        let Some((left, right)) = line.split_once('=') else { continue };
        let (left, right) = (left.trim(), right.trim());
        match section.as_str() {
            "champions" => {
                if let Some(p) = profile(right) {
                    k.profiles.insert(left.to_string(), p);
                }
            }
            "synergy" => {
                if let (Some((a, b)), Ok(n)) = (left.split_once('+'), right.parse::<u8>()) {
                    k.synergy.push((a.trim().to_string(), b.trim().to_string(), n.min(2)));
                }
            }
            "counters" => {
                if let (Some((a, b)), Ok(n)) = (left.split_once('>'), right.parse::<u8>()) {
                    k.counters.push((a.trim().to_string(), b.trim().to_string(), n.min(2)));
                }
            }
            _ => {}
        }
    }
    k
}

// ------------------------------------------------------------------------------ compositions

/// Composition features: each is 0 for a team that has enough of it and up to 1 for a team that
/// has none. The model fits what each costs (log-odds per full gap).
pub const COMP_FEATURES: usize = 6;
pub const COMP_NAMES: [&str; COMP_FEATURES] = ["frontline", "engage", "damage mix", "crowd control", "damage", "carry protection"];
pub const COMP_NAMES_ZH: [&str; COMP_FEATURES] = ["前排", "开团", "伤害类型", "控制", "输出", "保护C位"];
/// What League says each gap costs (the prior means of the fit).
pub const COMP_PRIOR: [f32; COMP_FEATURES] = [-0.15, -0.10, -0.12, -0.06, -0.10, -0.06];

/// A team's traits summed over its champions (an unknown champion counts as an average one).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sums {
    pub front: f32,
    pub engage: f32,
    pub peel: f32,
    pub poke: f32,
    pub dive: f32,
    pub carry: f32,
    pub cc: f32,
    pub ap: f32,
    pub ad: f32,
    pub n: f32,
}

impl Sums {
    pub fn add(&mut self, p: &Profile, w: f32) {
        self.front += p.front * w;
        self.engage += p.engage * w;
        self.peel += p.peel * w;
        self.poke += p.poke * w;
        self.dive += p.dive * w;
        self.carry += p.carry * w;
        self.cc += p.cc * w;
        match p.dmg {
            Dmg::Ad => self.ad += w,
            Dmg::Ap => self.ap += w,
            Dmg::Mixed => {
                self.ad += 0.5 * w;
                self.ap += 0.5 * w;
            }
        }
        self.n += w;
    }

    /// The gaps (see [`COMP_FEATURES`]) of a five-champion team with these sums.
    pub fn features(&self) -> [f32; COMP_FEATURES] {
        let gap = |need: f32, have: f32| ((need - have) / need).clamp(0.0, 1.0);
        let unprotected = ((self.carry - 2.0) / 2.0).clamp(0.0, 1.0) * gap(2.0, self.peel + 0.5 * self.front);
        [
            gap(2.0, self.front),
            gap(1.5, self.engage),
            gap(1.5, self.ap.min(self.ad)),
            gap(3.0, self.cc),
            gap(3.0, self.carry + self.dive + self.poke),
            unprotected,
        ]
    }
}

/// The average profile over a list (what an open slot is expected to bring).
pub fn average<'a>(profiles: impl Iterator<Item = &'a Profile>) -> Profile {
    let mut s = Sums::default();
    for p in profiles {
        s.add(p, 1.0);
    }
    let n = s.n.max(1.0);
    Profile {
        dmg: Dmg::Mixed,
        front: s.front / n,
        engage: s.engage / n,
        peel: s.peel / n,
        poke: s.poke / n,
        dive: s.dive / n,
        carry: s.carry / n,
        cc: s.cc / n,
        tier: 0,
    }
}

/// The prior mean of a champion's strength from its tier (log-odds, before `lol_prior`).
pub fn tier_prior(tier: i8) -> f32 {
    match tier {
        2 => 0.15,
        1 => 0.08,
        -1 => -0.06,
        t if t <= -2 => -0.12,
        _ => 0.0,
    }
}

/// The prior mean of a known pair's synergy or matchup (log-odds, before `lol_prior`).
pub fn pair_prior(n: u8) -> f32 {
    0.06 * n as f32
}

/// How often a champion plays each lane before the save has seen it (main lane first): a prior
/// share per lane, summing to 1.
pub fn lane_prior(roles: &[Role]) -> [f32; 5] {
    const LISTED: [f32; 3] = [0.7, 0.22, 0.12];
    const OTHER: f32 = 0.02;
    let mut out = [OTHER; 5];
    for (i, r) in roles.iter().take(3).enumerate() {
        out[r.index()] = LISTED[i];
    }
    let total: f32 = out.iter().sum();
    out.iter_mut().for_each(|x| *x /= total);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_tables_parse() {
        let k = get();
        assert_eq!(k.positions.len(), 165, "positions");
        assert_eq!(k.profiles.len(), 165, "profiles");
        for id in k.positions.keys() {
            assert!(k.profiles.contains_key(id), "{id} has no profile");
        }
        for (a, b, _) in k.synergy.iter().chain(&k.counters) {
            assert!(k.profiles.contains_key(a) && k.profiles.contains_key(b), "{a} / {b}");
        }
        assert_eq!(k.positions["league_aatrox"], vec![Role::Top]);
    }

    #[test]
    fn gaps() {
        let tank = profile("dmg=AP front=2 engage=2 peel=1 poke=0 dive=0 carry=0 cc=2 tier=B").unwrap();
        let adc = profile("dmg=AD front=0 engage=0 peel=0 poke=1 dive=0 carry=2 cc=0 tier=A").unwrap();
        let mut five_adcs = Sums::default();
        (0..5).for_each(|_| five_adcs.add(&adc, 1.0));
        let f = five_adcs.features();
        assert_eq!(f[0], 1.0, "no frontline");
        assert_eq!(f[2], 1.0, "all AD");
        assert!(f[5] > 0.9, "carries with nobody to protect them");
        let mut mixed = Sums::default();
        for p in [&tank, &tank, &adc, &adc, &tank] {
            mixed.add(p, 1.0);
        }
        assert_eq!(mixed.features()[0], 0.0);
        assert_eq!(mixed.features()[1], 0.0);
        let shares = lane_prior(&[Role::Mid, Role::Top]);
        assert!((shares.iter().sum::<f32>() - 1.0).abs() < 1e-5 && shares[2] > 0.6);
    }
}
