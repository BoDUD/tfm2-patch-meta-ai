//! The meta model: one logistic regression over every kept match (`glm`), with
//!
//! - **champion strength per patch**: one parameter per champion and patch, tied to the patch
//!   before by a random walk - a small step when the champion was not touched, a large one
//!   (centred on the patch notes' direction) when it was buffed, nerfed or reworked. So games
//!   from before a balance change still count, they just count for less, and the current
//!   patch's few games move the estimate exactly as much as they deserve;
//! - **role**: how much better or worse a champion does in each lane than overall;
//! - **players**: each athlete's own strength (so a champion is not rated up for being played by
//!   the best team), and each athlete's mastery of each champion;
//! - **pairs**: synergy between allies and the matchup between opponents;
//! - **side**: the blue-side advantage.
//!
//! Every parameter has a Gaussian prior around 0, so anything seen in few games stays close to
//! "average". A champion's displayed win rate is `sigmoid(strength)`: how often a team with it
//! and four average champions (and average players) beats an average team.

use std::collections::{BTreeMap, HashMap};

use crate::glm::{self, logit, sigmoid, Problem, Row};
use crate::history::{Game, Names, Role};
use crate::patchnotes::PatchNote;
use crate::records::compare_versions;

/// Model settings (from `settings.ini`).
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Patches kept, the current one included.
    pub max_patches: usize,
    /// Prior spread of a champion's strength in its first kept patch (log-odds).
    pub first_sd: f32,
    /// How far an untouched champion may drift from one patch to the next.
    pub drift_sd: f32,
    /// ... and a champion the patch notes buffed or nerfed.
    pub change_sd: f32,
    /// Win-rate step the patch notes' direction suggests (0.02 = 2 points).
    pub patch_shift: f32,
    /// Champions reworked in the current patch: their history barely counts.
    pub reworked: Vec<String>,
    pub role_sd: f32,
    pub athlete_sd: f32,
    pub mastery_sd: f32,
    pub pair_sd: f32,
    pub solo_weight: f32,
    pub side_sd: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_patches: 12,
            first_sd: 0.5,
            drift_sd: 0.08,
            change_sd: 0.3,
            patch_shift: 0.02,
            reworked: Vec::new(),
            role_sd: 0.3,
            athlete_sd: 0.35,
            mastery_sd: 0.2,
            pair_sd: 0.15,
            solo_weight: 0.5,
            side_sd: 0.3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    Side,
    Champ(u16, u16),
    Role(u16, u8),
    Athlete(u32),
    Mastery(u32, u16),
    /// Allies, smaller id first.
    Synergy(u16, u16),
    /// Opponents, smaller id first; positive = the first one wins the matchup.
    Counter(u16, u16),
}

/// Ordered pair key and the sign for "a with/against b".
pub fn pair(a: u16, b: u16) -> ((u16, u16), f32) {
    if a <= b {
        ((a, b), 1.0)
    } else {
        ((b, a), -1.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tally {
    pub games: u32,
    pub wins: u32,
}

impl Tally {
    fn add(&mut self, won: bool) {
        self.games += 1;
        self.wins += won as u32;
    }

    pub fn rate(&self) -> Option<f32> {
        (self.games > 0).then(|| self.wins as f32 / self.games as f32)
    }
}

/// A parameter's estimate and how many games stand behind it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Effect {
    pub value: f32,
    pub sd: f32,
    pub tally: Tally,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Champion {
    pub id: u16,
    pub name: String,
    /// Strength this patch (log-odds against average) and its standard deviation.
    pub strength: f32,
    pub sd: f32,
    /// Strength in the patch before (None when only one patch is kept).
    pub previous: Option<f32>,
    /// Games and wins this patch / over the kept patches.
    pub current: Tally,
    pub window: Tally,
    /// Recent games, each patch back counting half as much (tier eligibility).
    pub evidence: f32,
    /// Strength offset per role and the games in it (kept patches).
    pub roles: [Effect; 5],
    /// The newest patch whose notes changed it: (version, +1 buff / -1 nerf).
    pub last_change: Option<(String, i32)>,
    /// This patch's notes: +1 buffed, -1 nerfed, 0 untouched.
    pub patch_dir: i32,
    /// This patch: share of matches it was picked / banned in.
    pub pick_rate: f32,
    pub ban_rate: f32,
}

impl Champion {
    /// Estimated win rate this patch.
    pub fn win_rate(&self) -> f32 {
        sigmoid(self.strength)
    }

    /// Estimated win rate in one role.
    pub fn role_rate(&self, role: Role) -> f32 {
        sigmoid(self.strength + self.roles[role.index()].value)
    }

    /// Share of its kept games played in each role.
    pub fn role_share(&self) -> [f32; 5] {
        let total: u32 = self.roles.iter().map(|r| r.tally.games).sum();
        let mut out = [0.0; 5];
        if total > 0 {
            for (o, r) in out.iter_mut().zip(&self.roles) {
                *o = r.tally.games as f32 / total as f32;
            }
        }
        out
    }

    /// The cautious score tiers are ranked by.
    pub fn cautious(&self) -> f32 {
        self.strength - self.sd
    }

    pub fn presence(&self) -> f32 {
        self.pick_rate + self.ban_rate
    }
}

#[derive(Clone, Debug, Default)]
pub struct Meta {
    pub current: String,
    /// Kept patches, oldest first (the current one last).
    pub versions: Vec<String>,
    pub names: Names,
    pub champions: Vec<Champion>,
    pub side: f32,
    pub athletes: HashMap<u32, Effect>,
    pub mastery: HashMap<(u32, u16), Effect>,
    pub synergy: HashMap<(u16, u16), Effect>,
    pub counter: HashMap<(u16, u16), Effect>,
    /// Matches this patch (competition), and all kept matches.
    pub current_matches: u32,
    pub matches: u32,
    pub solo_matches: u32,
    /// Fitted parameters, for a warm start next time.
    pub keys: Vec<Key>,
    pub beta: Vec<f32>,
    pub sweeps: u32,
}

impl Meta {
    pub fn champion(&self, name: &str) -> Option<&Champion> {
        let id = self.names.get(name)?;
        self.champions.get(id as usize).filter(|c| c.id == id)
    }

    pub fn by_id(&self, id: u16) -> Option<&Champion> {
        self.champions.get(id as usize).filter(|c| c.id == id)
    }

    pub fn synergy(&self, a: u16, b: u16) -> Effect {
        let (key, _) = pair(a, b);
        self.synergy.get(&key).copied().unwrap_or_default()
    }

    /// The matchup from `a`'s point of view (positive = good for `a`).
    pub fn counter(&self, a: u16, b: u16) -> Effect {
        let (key, sign) = pair(a, b);
        let mut e = self.counter.get(&key).copied().unwrap_or_default();
        e.value *= sign;
        if sign < 0.0 {
            e.tally.wins = e.tally.games - e.tally.wins;
        }
        e
    }

    pub fn mastery(&self, athlete: u32, champ: u16) -> Effect {
        self.mastery.get(&(athlete, champ)).copied().unwrap_or_default()
    }
}

/// What the build needs besides the games.
pub struct Inputs<'a> {
    pub games: &'a [Game],
    pub names: &'a Names,
    /// Every selectable champion (rated even without games).
    pub champions: &'a [String],
    pub notes: &'a [PatchNote],
    /// The current patch (may be newer than any game: announced, not played yet).
    pub current: &'a str,
    /// The previous fit, for a warm start.
    pub warm: Option<&'a Meta>,
}

struct Registry {
    keys: Vec<Key>,
    index: HashMap<Key, u32>,
}

impl Registry {
    fn id(&mut self, key: Key) -> u32 {
        if let Some(i) = self.index.get(&key) {
            return *i;
        }
        let i = self.keys.len() as u32;
        self.keys.push(key);
        self.index.insert(key, i);
        i
    }
}

/// The kept patches, oldest first: the newest `max` with games, plus the current one.
pub fn kept_versions(games: &[Game], current: &str, max: usize) -> Vec<String> {
    let mut versions: Vec<String> = games.iter().map(|g| g.version.clone()).collect();
    versions.push(current.to_string());
    versions.sort_by(|a, b| compare_versions(a, b));
    versions.dedup();
    versions.retain(|v| !compare_versions(v, current).is_gt());
    let skip = versions.len().saturating_sub(max.max(1));
    versions.split_off(skip)
}

/// One game as model terms: (key, coefficient) from blue's point of view.
fn game_terms(game: &Game, version: u16, out: &mut Vec<(Key, f32)>) {
    out.push((Key::Side, 1.0));
    for (side, sign) in [(0usize, 1.0f32), (1, -1.0)] {
        let slots = &game.sides[side];
        for (i, s) in slots.iter().enumerate() {
            out.push((Key::Champ(s.champ, version), sign));
            if let Some(role) = s.role {
                out.push((Key::Role(s.champ, role as u8), sign));
            }
            if let Some(a) = s.athlete {
                out.push((Key::Athlete(a), sign));
                out.push((Key::Mastery(a, s.champ), sign));
            }
            for t in &slots[i + 1..] {
                let ((x, y), _) = pair(s.champ, t.champ);
                out.push((Key::Synergy(x, y), sign));
            }
        }
    }
    for b in &game.sides[0] {
        for r in &game.sides[1] {
            if b.champ == r.champ {
                continue;
            }
            let ((x, y), s) = pair(b.champ, r.champ);
            out.push((Key::Counter(x, y), s));
        }
    }
}

pub fn build(inp: &Inputs<'_>, set: &Settings) -> Meta {
    let versions = kept_versions(inp.games, inp.current, set.max_patches);
    let version_index: HashMap<&str, u16> =
        versions.iter().enumerate().map(|(i, v)| (v.as_str(), i as u16)).collect();
    let cur_index = (versions.len() - 1) as u16;
    let mut names = inp.names.clone();
    for c in inp.champions {
        names.id(c);
    }
    let champ_count = names.len() as u16;

    let mut reg = Registry { keys: Vec::new(), index: HashMap::new() };
    // the previous fit's numbering first, so the warm start lines up
    if let Some(warm) = inp.warm {
        if warm.versions == versions {
            for k in &warm.keys {
                reg.id(*k);
            }
        }
    }
    let side = reg.id(Key::Side);
    for c in 0..champ_count {
        for v in 0..versions.len() as u16 {
            reg.id(Key::Champ(c, v));
        }
    }

    // rows
    let mut rows = Vec::new();
    let mut tallies: HashMap<Key, Tally> = HashMap::new();
    let mut per_version: Vec<HashMap<u16, Tally>> = vec![HashMap::new(); versions.len()];
    let mut picks: HashMap<u16, u32> = HashMap::new();
    let mut bans: HashMap<u16, u32> = HashMap::new();
    let (mut matches, mut solo_matches, mut current_matches) = (0u32, 0u32, 0u32);
    let mut terms = Vec::new();
    for game in inp.games {
        let Some(&vi) = version_index.get(game.version.as_str()) else { continue };
        if game.sides[0].is_empty() || game.sides[1].is_empty() {
            continue;
        }
        terms.clear();
        game_terms(game, vi, &mut terms);
        let row_terms: Vec<(u32, f32)> = terms.iter().map(|(k, a)| (reg.id(*k), *a)).collect();
        rows.push(Row {
            y: if game.blue_win { 1.0 } else { 0.0 },
            weight: if game.solo { set.solo_weight } else { 1.0 },
            terms: row_terms,
        });
        if game.solo {
            solo_matches += 1;
        } else {
            matches += 1;
        }
        // plain counts, for display and eligibility
        for (side_i, slots) in game.sides.iter().enumerate() {
            let won = game.won(side_i);
            for (i, s) in slots.iter().enumerate() {
                if !game.solo {
                    per_version[vi as usize].entry(s.champ).or_default().add(won);
                }
                if let Some(role) = s.role {
                    tallies.entry(Key::Role(s.champ, role as u8)).or_default().add(won);
                }
                if let Some(a) = s.athlete {
                    tallies.entry(Key::Athlete(a)).or_default().add(won);
                    tallies.entry(Key::Mastery(a, s.champ)).or_default().add(won);
                }
                for t in &slots[i + 1..] {
                    let ((x, y), _) = pair(s.champ, t.champ);
                    tallies.entry(Key::Synergy(x, y)).or_default().add(won);
                }
            }
        }
        for b in &game.sides[0] {
            for r in &game.sides[1] {
                if b.champ == r.champ {
                    continue;
                }
                let ((x, y), sign) = pair(b.champ, r.champ);
                // the tally is from x's point of view
                let x_won = if sign > 0.0 { game.blue_win } else { !game.blue_win };
                tallies.entry(Key::Counter(x, y)).or_default().add(x_won);
            }
        }
        if vi == cur_index && !game.solo {
            current_matches += 1;
            for s in game.sides.iter().flatten() {
                *picks.entry(s.champ).or_default() += 1;
            }
            for b in game.bans.iter().flatten() {
                *bans.entry(*b).or_default() += 1;
            }
        }
    }

    // priors
    let mut problem = Problem::new(0);
    problem.prior(side, 0.0, set.side_sd);
    let shift = logit(0.5 + set.patch_shift);
    let mut last_change: HashMap<u16, (String, i32)> = HashMap::new();
    for c in 0..champ_count {
        let name = names.name(c).to_string();
        problem.prior(reg.id(Key::Champ(c, 0)), 0.0, set.first_sd);
        for v in 1..versions.len() as u16 {
            let note = inp.notes.iter().rev().find(|n| n.is_for(&versions[v as usize]));
            let dir = note.map_or(0, |n| n.direction(&name));
            let reworked = v == cur_index && set.reworked.contains(&name);
            let sd = if reworked {
                set.first_sd
            } else if dir != 0 {
                set.change_sd
            } else {
                set.drift_sd
            };
            if dir != 0 {
                last_change.insert(c, (versions[v as usize].clone(), dir));
            }
            let (p, prev) = (reg.id(Key::Champ(c, v)), reg.id(Key::Champ(c, v - 1)));
            problem.chain(p, prev, dir as f32 * shift, sd);
        }
    }
    for (i, key) in reg.keys.iter().enumerate() {
        let sd = match key {
            Key::Side | Key::Champ(..) => continue,
            Key::Role(..) => set.role_sd,
            Key::Athlete(_) => set.athlete_sd,
            Key::Mastery(..) => set.mastery_sd,
            Key::Synergy(..) | Key::Counter(..) => set.pair_sd,
        };
        problem.prior(i as u32, 0.0, sd);
    }
    // keys of a previous fit that no game uses any more still get a prior (they stay at 0)
    problem.params = reg.keys.len();
    problem.rows = rows;

    let warm: Option<Vec<f32>> = inp.warm.filter(|w| w.versions == versions).map(|w| w.beta.clone());
    let fit = glm::fit(&problem, warm.as_deref(), 300, 1e-3);
    let beta = &fit.beta;
    let effect = |key: Key| -> Effect {
        match reg.index.get(&key) {
            Some(&i) => Effect {
                value: beta[i as usize],
                sd: fit.var[i as usize].sqrt(),
                tally: tallies.get(&key).copied().unwrap_or_default(),
            },
            None => Effect::default(),
        }
    };

    let mut champions = Vec::with_capacity(champ_count as usize);
    let cur = &per_version[cur_index as usize];
    for c in 0..champ_count {
        let name = names.name(c).to_string();
        let e = effect(Key::Champ(c, cur_index));
        let mut window = Tally::default();
        let mut evidence = 0.0f32;
        for (vi, tallies) in per_version.iter().enumerate() {
            if let Some(t) = tallies.get(&c) {
                window.games += t.games;
                window.wins += t.wins;
                evidence += t.games as f32 * 0.5f32.powi((cur_index as usize - vi) as i32);
            }
        }
        let mut role_effects = [Effect::default(); 5];
        for r in Role::ALL {
            role_effects[r.index()] = effect(Key::Role(c, r as u8));
        }
        let note = inp.notes.iter().rev().find(|n| n.is_for(inp.current));
        let per_match = |n: u32| if current_matches > 0 { n as f32 / current_matches as f32 } else { 0.0 };
        champions.push(Champion {
            id: c,
            strength: e.value,
            sd: e.sd,
            previous: (cur_index > 0).then(|| effect(Key::Champ(c, cur_index - 1)).value),
            current: cur.get(&c).copied().unwrap_or_default(),
            window,
            evidence,
            roles: role_effects,
            last_change: last_change.get(&c).cloned(),
            patch_dir: note.map_or(0, |n| n.direction(&name)),
            pick_rate: per_match(picks.get(&c).copied().unwrap_or(0)),
            ban_rate: per_match(bans.get(&c).copied().unwrap_or(0)),
            name,
        });
    }

    let mut out = Meta {
        current: inp.current.to_string(),
        versions,
        champions,
        side: beta[side as usize],
        current_matches,
        matches,
        solo_matches,
        sweeps: fit.sweeps,
        ..Default::default()
    };
    for key in &reg.keys {
        match *key {
            Key::Athlete(a) => {
                out.athletes.insert(a, effect(*key));
            }
            Key::Mastery(a, c) => {
                out.mastery.insert((a, c), effect(*key));
            }
            Key::Synergy(a, b) => {
                out.synergy.insert((a, b), effect(*key));
            }
            Key::Counter(a, b) => {
                out.counter.insert((a, b), effect(*key));
            }
            _ => {}
        }
    }
    out.names = names;
    out.keys = reg.keys;
    out.beta = fit.beta;
    out
}

/// How well the model predicts games it was not fitted on: fitted on everything older than the
/// newest `holdout` games, scored on those.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Backtest {
    pub games: u32,
    /// Share of games whose favourite won.
    pub accuracy: f32,
    pub brier: f32,
    pub log_loss: f32,
    /// Brier score of always saying 50%.
    pub coin_brier: f32,
}

pub fn backtest(inp: &Inputs<'_>, set: &Settings, holdout: usize) -> Option<Backtest> {
    let mut comp: Vec<&Game> = inp.games.iter().filter(|g| !g.solo).collect();
    if comp.len() < holdout * 3 || holdout == 0 {
        return None;
    }
    comp.sort_by_key(|g| g.record);
    let split = comp.len() - holdout;
    let train: Vec<Game> = comp[..split].iter().map(|g| (*g).clone()).collect();
    let test = &comp[split..];
    let newest_train = train.iter().map(|g| g.version.clone()).max_by(|a, b| compare_versions(a, b))?;
    let fitted = build(
        &Inputs { games: &train, names: inp.names, champions: inp.champions, notes: inp.notes, current: &newest_train, warm: None },
        set,
    );
    let mut bt = Backtest::default();
    let mut terms = Vec::new();
    let index: HashMap<Key, u32> = fitted.keys.iter().enumerate().map(|(i, k)| (*k, i as u32)).collect();
    let version_index: HashMap<&str, u16> =
        fitted.versions.iter().enumerate().map(|(i, v)| (v.as_str(), i as u16)).collect();
    let newest = (fitted.versions.len() - 1) as u16;
    for game in test {
        terms.clear();
        // a patch the training games never saw is rated as the newest one they did
        let vi = version_index.get(game.version.as_str()).copied().unwrap_or(newest);
        game_terms(game, vi, &mut terms);
        let row: Vec<(u32, f32)> =
            terms.iter().filter_map(|(k, a)| index.get(k).map(|i| (*i, *a))).collect();
        let p = glm::predict(&row, &fitted.beta);
        let y = if game.blue_win { 1.0 } else { 0.0 };
        bt.games += 1;
        bt.accuracy += ((p >= 0.5) == game.blue_win) as u32 as f32;
        bt.brier += (p - y) * (p - y);
        bt.log_loss -= if game.blue_win { p.max(1e-4).ln() } else { (1.0 - p).max(1e-4).ln() };
        bt.coin_brier += 0.25;
    }
    let n = bt.games.max(1) as f32;
    bt.accuracy /= n;
    bt.brier /= n;
    bt.log_loss /= n;
    bt.coin_brier /= n;
    Some(bt)
}

/// Tiers by share of the ranked champions (S top `s`%, then A, B, C, the rest D), ranked by
/// the cautious score, among champions with at least `min_evidence` recent games.
pub fn tiers(meta: &Meta, min_evidence: f32, shares: [f32; 4]) -> BTreeMap<String, crate::model::Tier> {
    let mut ranked: Vec<(String, f32)> = meta
        .champions
        .iter()
        .filter(|c| c.evidence >= min_evidence && c.evidence > 0.0)
        .map(|c| (c.name.clone(), c.cautious()))
        .collect();
    crate::model::split_by_share(&mut ranked, shares).into_iter().collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::glm::tests::Lcg;
    use crate::history::Slot;

    pub const NAMES: [&str; 12] = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"];

    /// Random 5v5 games between 12 champions with hidden strengths, roles by slot order,
    /// athletes drawn from 1-20 (athlete 1 much better than the rest), pairs (a,b) in synergy.
    pub fn simulate(n: usize, version: &str, strength: impl Fn(&str) -> f32, seed: u64) -> (Names, Vec<Game>) {
        let mut names = Names::default();
        for n in NAMES {
            names.id(n);
        }
        let mut rng = Lcg(seed);
        let mut games = Vec::new();
        for id in 0..n {
            let mut ids: Vec<u16> = (0..12).collect();
            for i in (1..ids.len()).rev() {
                let j = ((rng.next() * (i + 1) as f32) as usize).min(i);
                ids.swap(i, j);
            }
            // ten of twenty athletes, in random line-ups
            let mut athletes: Vec<u32> = (1..=20).collect();
            for i in (1..athletes.len()).rev() {
                let j = ((rng.next() * (i + 1) as f32) as usize).min(i);
                athletes.swap(i, j);
            }
            let side = |part: &[u16], who: &[u32]| -> Vec<Slot> {
                part.iter()
                    .zip(who)
                    .enumerate()
                    .map(|(i, (c, a))| Slot { champ: *c, role: Some(Role::ALL[i]), athlete: Some(*a) })
                    .collect()
            };
            let blue = side(&ids[..5], &athletes[..5]);
            let red = side(&ids[5..10], &athletes[5..10]);
            let power = |slots: &[Slot]| -> f32 {
                let mut s: f32 = slots.iter().map(|x| strength(names.name(x.champ))).sum();
                if slots.iter().any(|x| x.athlete == Some(1)) {
                    s += 0.6;
                }
                let has = |n: &str| slots.iter().any(|x| names.name(x.champ) == n);
                if has("a") && has("b") {
                    s += 0.5;
                }
                s
            };
            let p = sigmoid(power(&blue) - power(&red));
            let blue_win = rng.next() < p;
            games.push(Game {
                record: id,
                solo: false,
                version: version.to_string(),
                blue_win,
                teams: [None, None],
                sides: [blue, red],
                bans: [vec![ids[10]], vec![ids[11]]],
                length: None,
            });
        }
        (names, games)
    }

    fn strength(n: &str) -> f32 {
        match n {
            "c" => 0.5,
            "l" => -0.5,
            _ => 0.0,
        }
    }

    #[test]
    fn finds_strong_champions_players_and_pairs() {
        let (names, games) = simulate(3000, "1.1", strength, 11);
        let champions: Vec<String> = NAMES.iter().map(|s| s.to_string()).collect();
        let meta = build(
            &Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.1", warm: None },
            &Settings::default(),
        );
        let wr = |n: &str| meta.champion(n).unwrap().win_rate();
        assert!(wr("c") > 0.58 && wr("l") < 0.42, "c {} l {}", wr("c"), wr("l"));
        assert!((wr("e") - 0.5).abs() < 0.06, "e {}", wr("e"));
        assert!(meta.athletes[&1].value > 0.3, "the strong player is found: {:?}", meta.athletes[&1]);
        let ab = meta.synergy(meta.names.get("a").unwrap(), meta.names.get("b").unwrap());
        assert!(ab.value > 0.15, "a+b synergy: {ab:?}");
        // a and b are not rated up for each other
        assert!(wr("a") < 0.56 && wr("b") < 0.56, "a {} b {}", wr("a"), wr("b"));
        assert_eq!(meta.current_matches, 3000);
        let c = meta.champion("c").unwrap();
        assert!(c.pick_rate > 0.7 && c.ban_rate > 0.1, "{} {}", c.pick_rate, c.ban_rate);
        assert!(c.role_share().iter().all(|s| *s > 0.1));
    }

    #[test]
    fn balance_changes_move_strength_without_forgetting() {
        // patch 1.1: c strong; 1.2 (c nerfed hard): only 150 games, c now weak
        let (names, mut games) = simulate(2000, "1.1", strength, 5);
        let (_, later) = simulate(150, "1.2", |n| match n {
            "c" => -0.4,
            "l" => -0.5,
            _ => 0.0,
        }, 9);
        games.extend(later.into_iter().map(|mut g| {
            g.record += 10_000;
            g
        }));
        let champions: Vec<String> = NAMES.iter().map(|s| s.to_string()).collect();
        let note = PatchNote {
            versions: vec!["1.2".into()],
            changes: [("c".to_string(), -2)].into_iter().collect(),
        };
        let meta = build(
            &Inputs { games: &games, names: &names, champions: &champions, notes: &[note], current: "1.2", warm: None },
            &Settings::default(),
        );
        let c = meta.champion("c").unwrap();
        let l = meta.champion("l").unwrap();
        assert_eq!(c.last_change, Some(("1.2".to_string(), -1)));
        assert_eq!(c.patch_dir, -1);
        // c dropped from ~62%; l (untouched) stays weak on its history
        assert!(c.win_rate() < 0.55 && c.win_rate() > 0.4, "c {} {:?}", c.win_rate(), c.current);
        assert!(c.sd > l.sd, "less sure about the changed champion");
        assert!(l.win_rate() < 0.42, "l {}", l.win_rate());
        assert!(c.current.games + l.current.games > 0);
        // warm start reuses the fit
        let again = build(
            &Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.2", warm: Some(&meta) },
            &Settings::default(),
        );
        assert!(again.sweeps < meta.sweeps, "{} vs {}", again.sweeps, meta.sweeps);
    }

    #[test]
    fn backtest_beats_a_coin() {
        let (names, games) = simulate(2500, "1.1", strength, 21);
        let champions: Vec<String> = NAMES.iter().map(|s| s.to_string()).collect();
        let bt = backtest(
            &Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.1", warm: None },
            &Settings::default(),
            500,
        )
        .unwrap();
        assert_eq!(bt.games, 500);
        assert!(bt.brier < bt.coin_brier, "{bt:?}");
        assert!(bt.accuracy > 0.55, "{bt:?}");
    }

    /// `cargo test --release -- --ignored --nocapture fit_time`: a save-sized fit.
    #[test]
    #[ignore]
    fn fit_time() {
        let mut names = Names::default();
        let champions: Vec<String> = (0..130).map(|i| format!("champ{i}")).collect();
        for c in &champions {
            names.id(c);
        }
        let mut rng = Lcg(1);
        let mut games = Vec::new();
        for id in 0..3000usize {
            let version = format!("1.{}", id / 250);
            let mut sides: [Vec<Slot>; 2] = [Vec::new(), Vec::new()];
            let team = |rng: &mut Lcg| (rng.next() * 40.0) as u32;
            let (t0, t1) = (team(&mut rng), team(&mut rng));
            for (side, t) in [(0usize, t0), (1, t1)] {
                for r in Role::ALL {
                    // each lane draws from its own 30 champions
                    let champ = (r.index() as f32 * 25.0 + rng.next() * 30.0) as u16 % 130;
                    sides[side].push(Slot { champ, role: Some(r), athlete: Some(t * 5 + r.index() as u32) });
                }
            }
            games.push(Game {
                record: id,
                solo: false,
                version,
                blue_win: rng.next() < 0.5,
                teams: [Some(t0), Some(t1)],
                sides,
                bans: [vec![], vec![]],
                length: None,
            });
        }
        let started = std::time::Instant::now();
        let inp = Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.11", warm: None };
        let meta = build(&inp, &Settings::default());
        let cold = started.elapsed();
        let mut more = games.clone();
        more.push(games[0].clone());
        let started = std::time::Instant::now();
        let again = build(&Inputs { games: &more, warm: Some(&meta), ..inp }, &Settings::default());
        eprintln!(
            "params {} cold {:?} ({} it), warm {:?} ({} it)",
            meta.keys.len(),
            cold,
            meta.sweeps,
            started.elapsed(),
            again.sweeps
        );
    }

    #[test]
    fn versions_kept() {
        let g = |v: &str| Game {
            record: 0,
            solo: false,
            version: v.into(),
            blue_win: true,
            teams: [None, None],
            sides: [vec![], vec![]],
            bans: [vec![], vec![]],
            length: None,
        };
        let games = [g("1.9"), g("1.10"), g("1.8"), g("2.0")];
        assert_eq!(kept_versions(&games, "1.10", 2), ["1.9", "1.10"]);
        assert_eq!(kept_versions(&games, "1.11", 3), ["1.9", "1.10", "1.11"]);
    }
}
