//! Reading a draft with the meta model: which lane each pick plays, how likely the line-ups
//! are to win, and what a candidate pick or ban is worth - with the parts it comes from, so the
//! draft screen can say why.
//!
//! Everything is in log-odds (0 = no change). A pick's value is its strength in the best lane
//! still open, plus its synergy with the allies already picked, plus its matchups against the
//! enemies already picked, minus a little when the team's damage would become one-sided, plus -
//! when the team's players are known - the mastery of the player who would play it. A ban's
//! value is what the candidate would be worth to the enemy (their player's mastery included),
//! weighted by how likely they are to take it.
//!
//! Draft order matters too ([`with_exposure`]): an early pick can still be countered by every
//! pick the other team has left, a last pick by none. So a pick loses what its worst likely
//! counters on offer would take from it, in proportion to the picks the other team has left -
//! early picks lean to safe champions, late picks to counters.

use crate::history::Role;
use crate::meta::Meta;

/// A team's players and the lane each one plays (from their newest match).
pub type Roster = [(u32, Option<Role>)];

/// The player of `roster` who plays `role`.
pub fn player_in(roster: &Roster, role: Role) -> Option<u32> {
    roster.iter().find(|(_, r)| *r == Some(role)).map(|(a, _)| *a)
}

/// Physical or magic damage (from the champion's tags).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Damage {
    Physical,
    Magic,
    Mixed,
}

/// A lane a champion plays in under this share of its games is off-role for it.
const OFF_ROLE_SHARE: f32 = 0.08;
/// ... and what playing it there costs (log-odds).
const OFF_ROLE_COST: f32 = 0.6;
/// A team with four or five damage dealers of one kind and none of the other.
const ONE_SIDED_COST: f32 = 0.15;
/// Games below which a champion's lane shares are not trusted (any lane is fine).
const ROLE_EVIDENCE: u32 = 8;

/// Lane assignment of a team's picks: `roles[i]` is the lane of `picks[i]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Lanes {
    pub roles: Vec<Role>,
    /// Probability of this assignment among all assignments (how sure the read is).
    pub confidence: f32,
}

/// How well a champion fits a lane, as a log-likelihood from its share of games there.
fn lane_fit(meta: &Meta, champ: u16, role: Role) -> f32 {
    let Some(c) = meta.by_id(champ) else { return 0.0 };
    let games: u32 = c.roles.iter().map(|r| r.tally.games).sum();
    if games < ROLE_EVIDENCE {
        return (0.2f32).ln();
    }
    // a little smoothing, so an unseen lane is unlikely but possible
    let share = (c.roles[role.index()].tally.games as f32 + 0.25) / (games as f32 + 1.25);
    share.ln()
}

/// The most likely lane for each pick (at most five), and how sure that is: every one-to-one
/// assignment is scored by how often each champion plays its lane.
pub fn lanes(meta: &Meta, picks: &[u16]) -> Lanes {
    let picks = &picks[..picks.len().min(5)];
    let mut best: Option<(f32, Vec<Role>)> = None;
    let mut total = 0.0f32;
    let mut current = Vec::with_capacity(picks.len());
    let mut used = [false; 5];
    fn walk(
        meta: &Meta,
        picks: &[u16],
        current: &mut Vec<Role>,
        used: &mut [bool; 5],
        score: f32,
        best: &mut Option<(f32, Vec<Role>)>,
        total: &mut f32,
    ) {
        if current.len() == picks.len() {
            *total += score.exp();
            if best.as_ref().is_none_or(|(s, _)| score > *s) {
                *best = Some((score, current.clone()));
            }
            return;
        }
        let champ = picks[current.len()];
        for r in Role::ALL {
            if used[r.index()] {
                continue;
            }
            used[r.index()] = true;
            current.push(r);
            walk(meta, picks, current, used, score + lane_fit(meta, champ, r), best, total);
            current.pop();
            used[r.index()] = false;
        }
    }
    walk(meta, picks, &mut current, &mut used, 0.0, &mut best, &mut total);
    match best {
        Some((score, roles)) => Lanes { roles, confidence: if total > 0.0 { score.exp() / total } else { 0.0 } },
        None => Lanes { roles: Vec::new(), confidence: 1.0 },
    }
}

/// For each pick, the probability of each lane (summed over every one-to-one assignment,
/// weighted by how often each champion plays its lane). Rows follow `picks`.
pub fn lane_probabilities(meta: &Meta, picks: &[u16]) -> Vec<[f32; 5]> {
    let picks = &picks[..picks.len().min(5)];
    let mut out = vec![[0.0f32; 5]; picks.len()];
    let mut total = 0.0f32;
    let mut current: Vec<Role> = Vec::with_capacity(picks.len());
    fn walk(meta: &Meta, picks: &[u16], current: &mut Vec<Role>, score: f32, out: &mut [[f32; 5]], total: &mut f32) {
        if current.len() == picks.len() {
            let w = score.exp();
            *total += w;
            for (row, r) in out.iter_mut().zip(current.iter()) {
                row[r.index()] += w;
            }
            return;
        }
        let champ = picks[current.len()];
        for r in Role::ALL {
            if current.contains(&r) {
                continue;
            }
            current.push(r);
            walk(meta, picks, current, score + lane_fit(meta, champ, r), out, total);
            current.pop();
        }
    }
    walk(meta, picks, &mut current, 0.0, &mut out, &mut total);
    if total > 0.0 {
        for row in &mut out {
            row.iter_mut().for_each(|p| *p /= total);
        }
    }
    out
}

/// A champion's strength in a lane (log-odds), with the off-role cost.
pub fn strength_in(meta: &Meta, champ: u16, role: Role) -> f32 {
    let Some(c) = meta.by_id(champ) else { return 0.0 };
    let games: u32 = c.roles.iter().map(|r| r.tally.games).sum();
    let off = games >= ROLE_EVIDENCE && c.role_share()[role.index()] < OFF_ROLE_SHARE;
    c.strength + c.roles[role.index()].value - if off { OFF_ROLE_COST } else { 0.0 }
}

/// A team's own strength: its champions in their lanes and the synergy of every pair.
pub fn team_strength(meta: &Meta, picks: &[u16]) -> f32 {
    let lanes = lanes(meta, picks);
    let mut total: f32 = picks.iter().zip(&lanes.roles).map(|(c, r)| strength_in(meta, *c, *r)).sum();
    for (i, a) in picks.iter().enumerate() {
        for b in &picks[i + 1..] {
            total += meta.synergy(*a, *b).value;
        }
    }
    total
}

/// Probability that `ally` beats `enemy` (picks so far; missing picks count as average).
pub fn win_probability(meta: &Meta, ally: &[u16], enemy: &[u16]) -> f32 {
    let mut eta = team_strength(meta, ally) - team_strength(meta, enemy);
    for a in ally {
        for e in enemy {
            eta += meta.counter(*a, *e).value;
        }
    }
    crate::glm::sigmoid(eta)
}

/// What a candidate pick is worth and why (log-odds).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PickValue {
    pub champ: u16,
    /// The lane it would play.
    pub role: Option<Role>,
    pub strength: f32,
    pub synergy: f32,
    pub counter: f32,
    pub balance: f32,
    /// The mastery of the player who would play it.
    pub mastery: f32,
    /// What the other team's remaining picks could take from it by countering it.
    pub exposure: f32,
    pub total: f32,
}

/// The value of picking `cand` for the team that has `ally` against `enemy`, in the open lane
/// it plays most often.
pub fn pick_value(
    meta: &Meta,
    cand: u16,
    ally: &[u16],
    enemy: &[u16],
    players: &Roster,
    damage: &dyn Fn(u16) -> Option<Damage>,
) -> PickValue {
    pick_value_in(meta, cand, ally, enemy, players, damage, &open_lanes(meta, ally))
}

/// The lanes a team that picked `picks` still has to fill (worked out once per draft state,
/// not once per candidate).
pub fn open_lanes(meta: &Meta, picks: &[u16]) -> Vec<Role> {
    let taken = lanes(meta, picks);
    Role::ALL.into_iter().filter(|r| !taken.roles.contains(r)).collect()
}

/// [`pick_value`] with the team's open lanes already known.
#[allow(clippy::too_many_arguments)]
pub fn pick_value_in(
    meta: &Meta,
    cand: u16,
    ally: &[u16],
    enemy: &[u16],
    players: &Roster,
    damage: &dyn Fn(u16) -> Option<Damage>,
    open: &[Role],
) -> PickValue {
    // the open lane it plays most often (not the one whose estimate happens to be highest:
    // that would favour lucky noise); strength only breaks ties
    let (role, strength) = open
        .iter()
        .map(|r| (Some(*r), lane_fit(meta, cand, *r), strength_in(meta, cand, *r)))
        .max_by(|a, b| (a.1, a.2).partial_cmp(&(b.1, b.2)).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(r, _, s)| (r, s))
        .unwrap_or((None, meta.by_id(cand).map_or(0.0, |c| c.strength) - OFF_ROLE_COST));
    let synergy: f32 = ally.iter().map(|a| meta.synergy(cand, *a).value).sum();
    let counter: f32 = enemy.iter().map(|e| meta.counter(cand, *e).value).sum();
    let balance = balance_cost(cand, ally, damage);
    let mastery = role.and_then(|r| player_in(players, r)).map_or(0.0, |a| meta.mastery(a, cand).value);
    PickValue {
        champ: cand,
        role,
        strength,
        synergy,
        counter,
        balance,
        mastery,
        exposure: 0.0,
        total: strength + synergy + counter - balance + mastery,
    }
}

/// How much of the counter threat the other team's remaining picks pose counts.
const EXPOSURE_WEIGHT: f32 = 0.5;
/// The worst counters averaged into the threat.
const THREATS: usize = 3;

/// What the other team could take from `cand` by countering it: the average of its worst
/// matchups among the champions still on offer (weighted by how often each is played), times
/// the share of picks the other team has left. 0 once they have no picks left.
pub fn exposure(meta: &Meta, cand: u16, their_picks_left: usize, offer: &[u16]) -> f32 {
    if their_picks_left == 0 || offer.is_empty() {
        return 0.0;
    }
    let mut threats: Vec<f32> = offer
        .iter()
        .filter(|e| **e != cand)
        .filter_map(|e| {
            let edge = meta.counter(cand, *e).value;
            if edge >= 0.0 {
                return None;
            }
            let presence = meta.by_id(*e).map_or(0.0, |c| c.presence());
            let likely = crate::model::presence_factor(presence, meta.typical_presence);
            Some(-edge * likely)
        })
        .collect();
    threats.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    threats.truncate(THREATS);
    if threats.is_empty() {
        return 0.0;
    }
    let threat = threats.iter().sum::<f32>() / threats.len() as f32;
    threat * EXPOSURE_WEIGHT * (their_picks_left.min(5) as f32 / 5.0)
}

/// `v` with its counter exposure counted in.
pub fn with_exposure(meta: &Meta, mut v: PickValue, their_picks_left: usize, offer: &[u16]) -> PickValue {
    v.exposure = exposure(meta, v.champ, their_picks_left, offer);
    v.total -= v.exposure;
    v
}

/// The cost of a team's damage becoming one-sided with `cand` added.
fn balance_cost(cand: u16, ally: &[u16], damage: &dyn Fn(u16) -> Option<Damage>) -> f32 {
    let (mut physical, mut magic) = (0, 0);
    for c in ally.iter().chain(std::iter::once(&cand)) {
        match damage(*c) {
            Some(Damage::Physical) => physical += 1,
            Some(Damage::Magic) => magic += 1,
            _ => {}
        }
    }
    let one_sided = (physical >= 4 && magic == 0) || (magic >= 4 && physical == 0);
    if one_sided {
        ONE_SIDED_COST
    } else {
        0.0
    }
}

/// What banning `cand` is worth to the team that has `ally` against `enemy`: the candidate's
/// value to the enemy (synergy with their picks, matchups against ours), weighted by how often
/// it is picked or banned this patch compared with the average champion (half to double).
pub fn ban_value(
    meta: &Meta,
    cand: u16,
    ally: &[u16],
    enemy: &[u16],
    enemy_players: &Roster,
    damage: &dyn Fn(u16) -> Option<Damage>,
) -> PickValue {
    ban_value_in(meta, cand, ally, enemy, enemy_players, damage, &open_lanes(meta, enemy))
}

/// [`ban_value`] with the enemy's open lanes already known.
#[allow(clippy::too_many_arguments)]
pub fn ban_value_in(
    meta: &Meta,
    cand: u16,
    ally: &[u16],
    enemy: &[u16],
    enemy_players: &Roster,
    damage: &dyn Fn(u16) -> Option<Damage>,
    enemy_open: &[Role],
) -> PickValue {
    let mut v = pick_value_in(meta, cand, enemy, ally, enemy_players, damage, enemy_open);
    let presence = meta.by_id(cand).map_or(0.0, |c| c.presence());
    let factor = crate::model::presence_factor(presence, meta.typical_presence);
    // a weak champion is not worth a ban however popular it is
    v.total = if v.total > 0.0 { v.total * factor } else { v.total };
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::tests::{simulate, NAMES};
    use crate::meta::{build, Inputs, Settings};

    fn meta() -> Meta {
        let (names, games) = simulate(3000, "1.1", |n| match n {
            "c" => 0.5,
            "l" => -0.5,
            _ => 0.0,
        }, 11);
        let champions: Vec<String> = NAMES.iter().map(|s| s.to_string()).collect();
        build(
            &Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.1", warm: None },
            &Settings::default(),
        )
    }

    #[test]
    fn values_picks_bans_and_lineups() {
        let m = meta();
        let id = |n: &str| m.names.get(n).unwrap();
        let none = |_: u16| None;
        let c = pick_value(&m, id("c"), &[], &[], &[], &none);
        let l = pick_value(&m, id("l"), &[], &[], &[], &none);
        assert!(c.total > 0.3 && l.total < -0.2, "{c:?} {l:?}");
        // b is worth more next to a (their synergy)
        let b_alone = pick_value(&m, id("b"), &[id("e")], &[], &[], &none);
        let b_with_a = pick_value(&m, id("b"), &[id("a")], &[], &[], &none);
        assert!(b_with_a.total > b_alone.total + 0.15, "{b_with_a:?} vs {b_alone:?}");
        assert!(b_with_a.synergy > 0.15);
        // banning: c is the threat
        let ban_c = ban_value(&m, id("c"), &[], &[], &[], &none);
        let ban_l = ban_value(&m, id("l"), &[], &[], &[], &none);
        assert!(ban_c.total > ban_l.total);
        // a line-up with c beats one with l
        let p = win_probability(&m, &[id("c"), id("e")], &[id("l"), id("f")]);
        assert!(p > 0.65, "{p}");
        assert!((win_probability(&m, &[], &[]) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_players_mastery_counts_for_them() {
        let m = meta();
        let none = |_: u16| None;
        // athlete 1 is strong on every champion; the simulation gives no real mastery, so take
        // whichever (athlete, champion) the model rates highest and check it is used
        let (&(athlete, champ), best) = m
            .mastery
            .iter()
            .max_by(|a, b| a.1.value.partial_cmp(&b.1.value).unwrap())
            .unwrap();
        let plain = pick_value(&m, champ, &[], &[], &[], &none);
        let role = plain.role.unwrap();
        let with = pick_value(&m, champ, &[], &[], &[(athlete, Some(role))], &none);
        assert!((with.mastery - best.value).abs() < 1e-6 && with.total > plain.total);
        let other_lane = Role::ALL.into_iter().find(|r| *r != role).unwrap();
        let elsewhere = pick_value(&m, champ, &[], &[], &[(athlete, Some(other_lane))], &none);
        assert_eq!(elsewhere.mastery, 0.0, "only the player in that lane counts");
        assert_eq!(player_in(&[(7, Some(Role::Mid))], Role::Mid), Some(7));
    }

    #[test]
    fn lanes_follow_role_history() {
        let m = meta();
        // in the simulation every champion plays every lane: no strong read
        let l = lanes(&m, &[0, 1, 2]);
        assert_eq!(l.roles.len(), 3);
        assert!(l.confidence < 0.2, "{}", l.confidence);
        assert_eq!(lanes(&m, &[]).roles, Vec::<Role>::new());
    }

    #[test]
    fn early_picks_pay_for_being_counterable() {
        let m = meta();
        let none = |_: u16| None;
        let offer: Vec<u16> = (0..12).collect();
        // the champion with the worst matchup on offer
        let (exposed, _) = (0..12u16)
            .map(|c| (c, offer.iter().filter(|e| **e != c).map(|e| m.counter(c, *e).value).fold(0.0f32, f32::min)))
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .unwrap();
        let first = with_exposure(&m, pick_value(&m, exposed, &[], &[], &[], &none), 5, &offer);
        let last = with_exposure(&m, pick_value(&m, exposed, &[], &[], &[], &none), 0, &offer);
        assert!(first.exposure > 0.0 && last.exposure == 0.0, "{first:?}");
        assert!(first.total < last.total);
        let half = exposure(&m, exposed, 2, &offer);
        assert!(half < first.exposure && half > 0.0, "fewer picks left, less exposure");
    }

    #[test]
    fn lane_probabilities_sum_to_one() {
        let m = meta();
        let probs = lane_probabilities(&m, &[0, 1]);
        assert_eq!(probs.len(), 2);
        for row in &probs {
            assert!((row.iter().sum::<f32>() - 1.0).abs() < 1e-4);
        }
        assert!(lane_probabilities(&m, &[]).is_empty());
    }

    /// `cargo test --release -- --ignored --nocapture advice_time`: one draft state, 120
    /// candidates valued for pick and ban, the way the overlay does it - per candidate (as
    /// before) and with the per-state work done once.
    #[test]
    #[ignore]
    fn advice_time() {
        use crate::history::{Game, Names, Slot};
        let mut names = Names::default();
        let champions: Vec<String> = (0..130).map(|i| format!("c{i}")).collect();
        for c in &champions {
            names.id(c);
        }
        let mut rng = crate::glm::tests::Lcg(4);
        let games: Vec<Game> = (0..3000)
            .map(|id| {
                let mut sides: [Vec<Slot>; 2] = [Vec::new(), Vec::new()];
                for side in &mut sides {
                    for r in Role::ALL {
                        let champ = (r.index() as f32 * 25.0 + rng.next() * 30.0) as u16 % 130;
                        side.push(Slot { champ, role: Some(r), athlete: Some((rng.next() * 200.0) as u32), lane_gold: None });
                    }
                }
                Game { record: id, solo: false, version: "1.1".into(), blue_win: rng.next() < 0.5, teams: [None, None], sides, bans: [vec![], vec![]], length: None, tactics: Default::default() }
            })
            .collect();
        let m = build(&Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.1", warm: None }, &Settings::default());
        let (ally, enemy) = ([3u16, 40, 77], [10u16, 55]);
        let none = |_: u16| None;
        let started = std::time::Instant::now();
        for c in 0..120u16 {
            pick_value(&m, c, &ally, &enemy, &[], &none);
            // as before: the average presence over every champion, per candidate
            let typical = m.champions.iter().map(|c| c.presence()).sum::<f32>() / m.champions.len() as f32;
            std::hint::black_box(typical);
            ban_value(&m, c, &ally, &enemy, &[], &none);
        }
        let before = started.elapsed();
        let started = std::time::Instant::now();
        let (a, e) = (open_lanes(&m, &ally), open_lanes(&m, &enemy));
        for c in 0..120u16 {
            pick_value_in(&m, c, &ally, &enemy, &[], &none, &a);
            ban_value_in(&m, c, &ally, &enemy, &[], &none, &e);
        }
        eprintln!("one draft state, 120 candidates: per candidate {before:?}, shared {:?}", started.elapsed());
    }

    #[test]
    fn one_sided_damage_costs() {
        let damage = |c: u16| Some(if c < 6 { Damage::Physical } else { Damage::Magic });
        assert_eq!(balance_cost(4, &[0, 1, 2], &damage), ONE_SIDED_COST);
        assert_eq!(balance_cost(7, &[0, 1, 2], &damage), 0.0);
        assert_eq!(balance_cost(4, &[0, 1], &|_| None), 0.0);
    }
}
