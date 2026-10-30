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
//!
//! A pick also counts what it does to its team's **composition** (`Meta::comp_score`: the gaps a
//! line-up has - no frontline, no engage, one damage type... - with its open slots counted as
//! average champions, so a gap weighs more the fewer picks are left to fill it).
//!
//! **Bans** ([`ban_board`]) are valued the way a coach does it: not "how good is it" but "how
//! much worse does the enemy's draft get without it". For every lane the enemy still has to fill,
//! the enemy's options are valued as picks of theirs (strength there, synergy with their picks,
//! matchups against ours, composition, their player's mastery); a ban is worth what the enemy's
//! best option in those lanes loses - nothing when an equally good substitute is left, a lot for
//! the one champion nothing else replaces, and more for a flex pick that threatens several lanes.
//! What the same ban takes from our own options is subtracted, more when we would get the
//! champion first. A team's habits (the champions it played lately) make it likelier to take
//! them, so they weigh more in its options.
//!
//! **Lookahead** ([`simulate`]): the rest of a standard 3+2 draft played out from an action -
//! each side taking its best ban and pick in turn from the strongest champions on offer - and
//! the line-ups it ends in rated, so an action is judged by the draft it leads to.

use std::collections::HashMap;

use crate::history::Role;
use crate::meta::Meta;

/// A team's players and the lane each one plays (from their newest match).
pub type Roster = [(u32, Option<Role>)];

/// The player of `roster` who plays `role`.
pub fn player_in(roster: &Roster, role: Role) -> Option<u32> {
    roster.iter().find(|(_, r)| *r == Some(role)).map(|(a, _)| *a)
}

/// The mastery (the model's player-on-champion effect) that counts for `cand` played in `role`:
/// the player of `roster` in that lane, and which player that is. When the lane, or every
/// player's lane, is unknown: the line-up as a whole - the average over its players (no single
/// player). Nothing for an empty roster, or when nobody plays that lane.
pub fn lineup_mastery(meta: &Meta, roster: &Roster, role: Option<Role>, cand: u16) -> (f32, Option<u32>) {
    if roster.is_empty() {
        return (0.0, None);
    }
    let lanes_known = roster.iter().any(|(_, r)| r.is_some());
    match role {
        Some(r) if lanes_known => match player_in(roster, r) {
            Some(a) => (meta.mastery(a, cand).value, Some(a)),
            None => (0.0, None),
        },
        _ => (roster.iter().map(|(a, _)| meta.mastery(*a, cand).value).sum::<f32>() / roster.len() as f32, None),
    }
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

/// A champion's share of play in each lane: its games there, with its League lanes (when it is a
/// League champion) counting as [`ROLE_EVIDENCE`] games. `None`: too few games and no League
/// lanes - any lane is as likely.
pub fn lane_shares(meta: &Meta, champ: u16) -> Option<[f32; 5]> {
    let c = meta.by_id(champ)?;
    let games: u32 = c.roles.iter().map(|r| r.tally.games).sum();
    let prior = meta.lane_priors.get(champ as usize).copied().flatten();
    if games < ROLE_EVIDENCE && prior.is_none() {
        return None;
    }
    let weight = if prior.is_some() { ROLE_EVIDENCE as f32 } else { 0.0 };
    let prior = prior.unwrap_or([0.2; 5]);
    let mut out = [0.0; 5];
    for (i, o) in out.iter_mut().enumerate() {
        // a little smoothing, so an unseen lane is unlikely but possible
        *o = (c.roles[i].tally.games as f32 + weight * prior[i] + 0.05) / (games as f32 + weight + 0.25);
    }
    Some(out)
}

/// How well a champion fits a lane, as a log-likelihood from its share of games there.
fn lane_fit(meta: &Meta, champ: u16, role: Role) -> f32 {
    match lane_shares(meta, champ) {
        Some(shares) => shares[role.index()].ln(),
        None => (0.2f32).ln(),
    }
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

/// Whether `champ` is a real option in one of `open` (not off-role there; a champion whose lanes
/// are not known fits anywhere).
pub fn fits_lanes(meta: &Meta, champ: u16, open: &[Role]) -> bool {
    match lane_shares(meta, champ) {
        Some(shares) => open.iter().any(|r| shares[r.index()] >= OFF_ROLE_SHARE),
        None => true,
    }
}

/// A champion's strength in a lane (log-odds), with the off-role cost.
pub fn strength_in(meta: &Meta, champ: u16, role: Role) -> f32 {
    let Some(c) = meta.by_id(champ) else { return 0.0 };
    let off = lane_shares(meta, champ).is_some_and(|s| s[role.index()] < OFF_ROLE_SHARE);
    c.strength + c.roles[role.index()].value - if off { OFF_ROLE_COST } else { 0.0 }
}

/// A team's own strength: its champions in their lanes, the synergy of every pair and its
/// composition.
pub fn team_strength(meta: &Meta, picks: &[u16]) -> f32 {
    let lanes = lanes(meta, picks);
    let mut total: f32 = picks.iter().zip(&lanes.roles).map(|(c, r)| strength_in(meta, *c, *r)).sum();
    for (i, a) in picks.iter().enumerate() {
        for b in &picks[i + 1..] {
            total += meta.synergy(*a, *b).value;
        }
    }
    total + meta.comp_score(picks).0
}

/// Probability that `ally` beats `enemy` (picks so far; missing picks count as average), as
/// sure of itself as the held-out matches allow (`Meta::calibration`).
pub fn win_probability(meta: &Meta, ally: &[u16], enemy: &[u16]) -> f32 {
    let mut eta = team_strength(meta, ally) - team_strength(meta, enemy);
    for a in ally {
        for e in enemy {
            eta += meta.counter(*a, *e).value;
        }
    }
    crate::glm::sigmoid(eta * meta.calibration.unwrap_or(1.0))
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
    /// What it does to its team's composition (filling a gap > 0).
    pub comp: f32,
    /// The composition gap it fills most, when that is worth mentioning (`knowledge::COMP_NAMES`).
    pub fills: Option<usize>,
    /// The mastery of the player who would play it (as counted in `total`).
    pub mastery: f32,
    /// That player (`None`: nobody known, or the line-up as a whole).
    pub player: Option<u32>,
    /// What the other team's remaining picks could take from it by countering it.
    pub exposure: f32,
    /// What taking it keeps from the other team (part of what banning it would).
    pub contest: f32,
    /// What spending it now costs the rest of a Fearless series.
    pub conserve: f32,
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
    pick_value_weighted(meta, cand, ally, enemy, players, Mastery::FULL, damage, open)
}

/// How a player's mastery counts in a value: limited to `±cap`, then times `weight`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mastery {
    pub weight: f32,
    pub cap: f32,
}

impl Mastery {
    /// As the model has it (the player's own advice).
    pub const FULL: Mastery = Mastery { weight: 1.0, cap: f32::INFINITY };
}

/// [`pick_value_in`] with the players' mastery counted as `mastery` says (weight 0 = not at
/// all). The AI's draft hook adds to the game's own score, which already rewards the champions
/// its players are proficient in, so it counts the mastery for less than the player's advice.
#[allow(clippy::too_many_arguments)]
pub fn pick_value_weighted(
    meta: &Meta,
    cand: u16,
    ally: &[u16],
    enemy: &[u16],
    players: &Roster,
    mastery: Mastery,
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
    // the composition: the League profiles' gaps when they are known, else the plain damage mix
    let (comp, fills) = comp_gain(meta, cand, ally);
    let balance = if meta.open_slot.is_some() { 0.0 } else { balance_cost(cand, ally, damage) };
    let (mastery, player) = if mastery.weight > 0.0 {
        let (m, player) = lineup_mastery(meta, players, role, cand);
        (m.clamp(-mastery.cap, mastery.cap) * mastery.weight, player)
    } else {
        (0.0, None)
    };
    PickValue {
        champ: cand,
        role,
        strength,
        synergy,
        counter,
        balance,
        comp,
        fills,
        mastery,
        player,
        exposure: 0.0,
        contest: 0.0,
        conserve: 0.0,
        total: strength + synergy + counter - balance + comp + mastery,
    }
}

/// What adding `cand` does to the composition of a team that has `ally` (log-odds), and the gap
/// it fills most when that is at least 1 win-rate point.
pub fn comp_gain(meta: &Meta, cand: u16, ally: &[u16]) -> (f32, Option<usize>) {
    if meta.open_slot.is_none() || ally.len() >= 5 {
        return (0.0, None);
    }
    let (before, parts_before) = meta.comp_score(ally);
    let mut with = ally.to_vec();
    with.push(cand);
    let (after, parts_after) = meta.comp_score(&with);
    let best = (0..parts_before.len())
        .map(|i| (i, parts_after[i] - parts_before[i]))
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .filter(|(_, g)| *g >= 0.04)
        .map(|(i, _)| i);
    (after - before, best)
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
    ban_value_weighted(meta, cand, ally, enemy, enemy_players, Mastery::FULL, damage, enemy_open)
}

/// [`ban_value_in`] with the enemy players' mastery counted as `mastery` says.
#[allow(clippy::too_many_arguments)]
pub fn ban_value_weighted(
    meta: &Meta,
    cand: u16,
    ally: &[u16],
    enemy: &[u16],
    enemy_players: &Roster,
    mastery: Mastery,
    damage: &dyn Fn(u16) -> Option<Damage>,
    enemy_open: &[Role],
) -> PickValue {
    let mut v = pick_value_weighted(meta, cand, enemy, ally, enemy_players, mastery, damage, enemy_open);
    let presence = meta.by_id(cand).map_or(0.0, |c| c.presence());
    let factor = crate::model::presence_factor(presence, meta.typical_presence);
    // a weak champion is not worth a ban however popular it is
    v.total = if v.total > 0.0 { v.total * factor } else { v.total };
    v
}

/// One side of a draft: its picks so far, its fielded players and its habits.
#[derive(Clone, Copy)]
pub struct Team<'a> {
    pub picks: &'a [u16],
    pub players: &'a Roster,
    /// Share of its recent matches it played each champion in (scouting); `None`: not known.
    pub habits: Option<&'a HashMap<u16, f32>>,
    /// Hell Draft AI drafts for this team: how much likelier it takes each champion than our
    /// values say (log-odds, `hell::lean`). Its bans follow its ratings, not a ban board.
    pub lean: Option<&'a HashMap<u16, f32>>,
}

impl<'a> Team<'a> {
    pub fn new(picks: &'a [u16], players: &'a Roster) -> Self {
        Team { picks, players, habits: None, lean: None }
    }

    /// How much likelier the team is to take `champ` because it plays it a lot (log-odds).
    pub fn habit(&self, champ: u16) -> f32 {
        let share = self.habits.and_then(|h| h.get(&champ)).copied().unwrap_or(0.0);
        let lean = self.lean.and_then(|l| l.get(&champ)).copied().unwrap_or(0.0);
        HABIT_WEIGHT * (1.0 + HABIT_SCALE * share.clamp(0.0, 1.0)).ln() + lean
    }

    /// How Hell Draft AI rates a champion for its bans when it drafts for this team: its
    /// strength in its best lane and the lean (`None` when it does not draft for the team).
    pub fn hell_rating(&self, meta: &Meta, champ: u16) -> Option<f32> {
        let lean = self.lean?;
        let best = Role::ALL.iter().map(|r| strength_in(meta, champ, *r)).fold(f32::NEG_INFINITY, f32::max);
        Some(best + lean.get(&champ).copied().unwrap_or(0.0))
    }

    /// What Hell Draft AI bans for this team next: the best on offer by [`Team::hell_rating`].
    pub fn hell_ban(&self, meta: &Meta, offer: &[u16]) -> Option<u16> {
        offer
            .iter()
            .filter_map(|c| Some((*c, self.hell_rating(meta, *c)?)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(c, _)| c)
    }
}

/// A champion a team plays in every match is likelier by `HABIT_WEIGHT * ln(1 + HABIT_SCALE)`
/// (log-odds; about 0.19 = 5 win-rate points of "it would take it").
const HABIT_WEIGHT: f32 = 0.12;
const HABIT_SCALE: f32 = 4.0;

/// A ban candidate valued on the [`ban_board`] (log-odds).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BanValue {
    pub champ: u16,
    /// What the ban takes from the enemy's draft: how much worse their best option gets in the
    /// lanes they still have to fill.
    pub deny: f32,
    /// What it takes from ours (before the pick-order weight).
    pub own: f32,
    /// Its value to the enemy in the lane where the ban hurts them most (the reasons).
    pub threat: PickValue,
    /// How many of the enemy's open lanes it is a real option in (flex pick).
    pub lanes: u8,
    /// Nothing else on offer comes close to it there (the ban takes most of what it is worth).
    pub irreplaceable: bool,
    /// How much likelier the enemy is to take it because they play it a lot (log-odds).
    pub habit: f32,
    pub total: f32,
}

/// The options per lane "the best option" is taken over.
const BAN_OPTIONS: usize = 8;
/// How soft "the best option" is (log-odds): a team does not always take the top one.
const BAN_SOFTNESS: f32 = 0.08;
/// A flex pick's lanes after the one where the ban hurts most count this much each.
const FLEX_WEIGHT: f32 = 0.35;
/// A lane counts for a flex pick when the ban takes at least this much there.
const FLEX_MIN: f32 = 0.02;
/// How likely the enemy is to take a champion, from how often it is played or banned: at most
/// this much (log-odds) either way.
const PRESENCE_WEIGHT: f32 = 0.1;

/// Whether the team with `ours` picks takes the next champion before the team with `theirs`, in
/// the standard order (B R R B B R, then R B B R): the side with fewer picks goes first; at equal
/// counts blue goes first after an even number each (0-0, 2-2, 4-4) and red after an odd one
/// (1-1, 3-3: red opens the second pick phase). `None` when that needs the side and it is unknown.
pub fn picks_first(ours: usize, theirs: usize, blue: Option<bool>) -> Option<bool> {
    if ours != theirs {
        return Some(ours < theirs);
    }
    blue.map(|b| b == ours.is_multiple_of(2))
}

/// `tau * ln(sum(exp(v / tau)))` over `values`: the best of them, softened.
fn smooth_best(values: impl Iterator<Item = f32>) -> f32 {
    let values: Vec<f32> = values.collect();
    let Some(top) = values.iter().copied().reduce(f32::max) else { return 0.0 };
    top + BAN_SOFTNESS * values.iter().map(|v| ((v - top) / BAN_SOFTNESS).exp()).sum::<f32>().ln()
}

/// For every lane `team` still has to fill: the best `BAN_OPTIONS + 1` options on offer as
/// its picks there, best first.
fn lane_options(
    meta: &Meta,
    team: Team<'_>,
    other: &[u16],
    mastery: Mastery,
    damage: &dyn Fn(u16) -> Option<Damage>,
    offer: &[u16],
    likely: &dyn Fn(u16) -> f32,
) -> Vec<Vec<PickValue>> {
    if team.picks.len() >= 5 {
        return Vec::new();
    }
    open_lanes(meta, team.picks)
        .into_iter()
        .map(|role| {
            let mut options: Vec<PickValue> = offer
                .iter()
                .map(|c| {
                    let mut v = pick_value_weighted(meta, *c, team.picks, other, team.players, mastery, damage, &[role]);
                    v.total += likely(*c);
                    v
                })
                .collect();
            options.sort_by(|a, b| b.total.partial_cmp(&a.total).unwrap_or(std::cmp::Ordering::Equal));
            options.truncate(BAN_OPTIONS + 1);
            options
        })
        .collect()
}

/// What banning `champ` takes from a team with these lane options: per lane, how much its
/// softened best option drops; the worst lane fully, the others at [`FLEX_WEIGHT`]. Also the
/// lane where it hurts most (its option there), how many lanes it matters in, and whether the
/// next option in that lane is far behind.
fn drop_of(lanes: &[Vec<PickValue>], champ: u16) -> (f32, Option<PickValue>, u8, bool) {
    let mut drops: Vec<(f32, &PickValue, bool)> = Vec::new();
    for options in lanes {
        let Some(at) = options.iter().position(|v| v.champ == champ) else { continue };
        if at >= BAN_OPTIONS {
            continue;
        }
        let with = smooth_best(options[..BAN_OPTIONS.min(options.len())].iter().map(|v| v.total));
        let without = smooth_best(
            options.iter().enumerate().filter(|(i, _)| *i != at).take(BAN_OPTIONS).map(|(_, v)| v.total),
        );
        let next = options.iter().enumerate().find(|(i, _)| *i != at).map_or(f32::NEG_INFINITY, |(_, v)| v.total);
        let lone = options[at].total - next >= 0.1;
        drops.push(((with - without).max(0.0), &options[at], lone));
    }
    drops.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let Some(&(worst, threat, lone)) = drops.first() else { return (0.0, None, 0, false) };
    let rest: f32 = drops[1..].iter().map(|d| d.0).sum();
    let lanes = drops.iter().filter(|d| d.0 >= FLEX_MIN).count() as u8;
    (worst + FLEX_WEIGHT * rest, Some(threat.clone()), lanes, lone)
}

/// Every champion on offer valued as our ban, best first (see the module notes). `blue`: whether
/// we are the blue side (`None`: not known), for who would get a champion first.
#[allow(clippy::too_many_arguments)]
pub fn ban_board(
    meta: &Meta,
    us: Team<'_>,
    them: Team<'_>,
    mastery: Mastery,
    damage: &dyn Fn(u16) -> Option<Damage>,
    offer: &[u16],
    blue: Option<bool>,
) -> Vec<BanValue> {
    let likely = |c: u16| presence_bonus(meta, c) + them.habit(c);
    let theirs = lane_options(meta, them, us.picks, mastery, damage, offer, &likely);
    let ours = lane_options(meta, us, them.picks, mastery, damage, offer, &|_| 0.0);
    // a champion we would get first costs us more to ban
    let own_weight = match picks_first(us.picks.len(), them.picks.len(), blue) {
        Some(true) => 0.7,
        Some(false) => 0.25,
        None => 0.45,
    };
    let mut board: Vec<BanValue> = offer
        .iter()
        .map(|c| {
            let (deny, threat, lanes, irreplaceable) = drop_of(&theirs, *c);
            let (own, _, _, _) = drop_of(&ours, *c);
            let threat = threat.unwrap_or_else(|| {
                pick_value_weighted(meta, *c, them.picks, us.picks, them.players, mastery, damage, &open_lanes(meta, them.picks))
            });
            BanValue { champ: *c, deny, own, threat, lanes, irreplaceable, habit: them.habit(*c), total: deny - own_weight * own }
        })
        .collect();
    board.sort_by(|a, b| b.total.partial_cmp(&a.total).unwrap_or(std::cmp::Ordering::Equal));
    board
}

/// How much likelier any team is to take a champion that is picked or banned a lot this patch
/// (log-odds, at most `PRESENCE_WEIGHT` either way).
fn presence_bonus(meta: &Meta, c: u16) -> f32 {
    let presence = meta.by_id(c).map_or(0.0, |c| c.presence());
    PRESENCE_WEIGHT * crate::model::presence_factor(presence, meta.typical_presence).ln() / 2f32.ln()
}

/// What `them` most likely picks next: their best option in each open lane (their habits
/// counted), best first, each champion once.
pub fn likely_picks(
    meta: &Meta,
    them: Team<'_>,
    us: &[u16],
    mastery: Mastery,
    damage: &dyn Fn(u16) -> Option<Damage>,
    offer: &[u16],
) -> Vec<PickValue> {
    let likely = |c: u16| presence_bonus(meta, c) + them.habit(c);
    let mut all: Vec<PickValue> = lane_options(meta, them, us, mastery, damage, offer, &likely).into_iter().flatten().collect();
    all.sort_by(|a, b| b.total.partial_cmp(&a.total).unwrap_or(std::cmp::Ordering::Equal));
    let mut seen = Vec::new();
    all.retain(|v| {
        let fresh = !seen.contains(&v.champ);
        seen.push(v.champ);
        fresh
    });
    all
}

// ------------------------------------------------------------------------------- lookahead

/// One step of a draft: the blue side acts (else red), a ban (else a pick).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub blue: bool,
    pub ban: bool,
}

const fn step(blue: bool, ban: bool) -> Step {
    Step { blue, ban }
}

/// The standard 3+2 order: three bans each, picks B R R B B R, two bans each (red first), picks
/// R B B R.
pub const ORDER_3_2: [Step; 20] = [
    step(true, true), step(false, true), step(true, true), step(false, true), step(true, true), step(false, true),
    step(true, false), step(false, false), step(false, false), step(true, false), step(true, false), step(false, false),
    step(false, true), step(true, true), step(false, true), step(true, true),
    step(false, false), step(true, false), step(true, false), step(false, false),
];

/// Where a draft with `bans` bans in all and these picks stands in [`ORDER_3_2`]: the next
/// step's index. `None` when the counts do not fit that order (another format) or it is over.
pub fn order_position(bans: usize, blue_picks: usize, red_picks: usize) -> Option<usize> {
    let (mut b, mut bp, mut rp) = (0, 0, 0);
    for (i, s) in ORDER_3_2.iter().enumerate() {
        if (b, bp, rp) == (bans, blue_picks, red_picks) {
            return Some(i);
        }
        match (s.ban, s.blue) {
            (true, _) => b += 1,
            (false, true) => bp += 1,
            (false, false) => rp += 1,
        }
    }
    None
}

/// The champions the lookahead plays with: the `n` strongest on offer (strength in their best
/// lane), which is where every sensible ban and pick comes from.
pub fn lookahead_pool(meta: &Meta, offer: &[u16], n: usize) -> Vec<u16> {
    let best = |c: u16| Role::ALL.iter().map(|r| strength_in(meta, c, *r)).fold(f32::NEG_INFINITY, f32::max);
    let mut pool: Vec<(u16, f32)> = offer.iter().map(|c| (*c, best(*c))).collect();
    pool.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    pool.into_iter().take(n).map(|(c, _)| c).collect()
}

/// Plays out the rest of a 3+2 draft from step `at` of [`ORDER_3_2`], where `first` is the
/// acting side's choice; afterwards each side bans the top of its ban board and picks its best
/// pick from `pool` - a side Hell Draft AI drafts for its way ([`Team::lean`]). Returns the blue side's win chance with the line-ups it ends in.
pub fn simulate(
    meta: &Meta,
    blue: Team<'_>,
    red: Team<'_>,
    pool: &[u16],
    at: usize,
    first: u16,
    damage: &dyn Fn(u16) -> Option<Damage>,
) -> f32 {
    let (mut bp, mut rp) = (blue.picks.to_vec(), red.picks.to_vec());
    let mut offer: Vec<u16> = pool.iter().copied().filter(|c| *c != first).collect();
    let none = Mastery { weight: 0.0, cap: 0.0 };
    for (i, s) in ORDER_3_2.iter().enumerate().skip(at) {
        let (own, other) = if s.blue { (&mut bp, &rp) } else { (&mut rp, &bp) };
        let (own_team, other_team) = if s.blue { (blue, red) } else { (red, blue) };
        let choice = if i == at {
            Some(first)
        } else if s.ban && own_team.lean.is_some() {
            own_team.hell_ban(meta, &offer)
        } else if s.ban {
            let us = Team { picks: own, ..own_team };
            let them = Team { picks: other, ..other_team };
            ban_board(meta, us, them, none, damage, &offer, Some(s.blue)).first().map(|b| b.champ)
        } else if own.len() < 5 {
            let open = open_lanes(meta, own);
            offer
                .iter()
                .map(|c| (*c, pick_value_weighted(meta, *c, own, other, own_team.players, none, damage, &open).total + own_team.habit(*c)))
                .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(c, _)| c)
        } else {
            None
        };
        let Some(c) = choice else { continue };
        offer.retain(|x| *x != c);
        if !s.ban && own.len() < 5 {
            own.push(c);
        }
    }
    win_probability(meta, &bp, &rp)
}

// ------------------------------------------------------------------------------ one turn

/// A pick also keeps the champion from the enemy: this share of what banning it would take from
/// them counts.
pub const CONTEST: f32 = 0.5;
/// Candidates the lookahead plays out, and the champions it plays with.
pub const LOOKAHEAD: usize = 5;
pub const LOOKAHEAD_POOL: usize = 28;

/// One team's turn: what the draft screen advises the player, and what the AI does with
/// `ai_draft` on.
#[derive(Clone, Copy)]
pub struct TurnInput<'a> {
    pub us: Team<'a>,
    pub them: Team<'a>,
    /// Still on offer, and what of it `us` may pick (a position lock may hold some).
    pub offer: &'a [u16],
    pub pickable: &'a [u16],
    pub banning: bool,
    /// `us` is the blue side (`None`: not known - then no lookahead).
    pub blue: Option<bool>,
    /// Bans made so far, both sides (where a 3+2 draft stands).
    pub bans_made: usize,
    /// Play the best few out to the end of a 3+2 draft when it is `us`'s turn in it.
    pub lookahead: bool,
    /// Fearless series: the share of what a pick takes from our later games that counts.
    pub conserve: f32,
}

/// [`plan_turn`]'s ranking.
#[derive(Clone, Debug, Default)]
pub struct TurnPlan {
    /// Our bans, best first (the lookahead's order when it ran on a ban).
    pub board: Vec<BanValue>,
    /// Our picks, best first (likewise).
    pub picks: Vec<PickValue>,
    /// The win chance each played-out candidate leads to (lookahead).
    pub outlook: HashMap<u16, f32>,
}

impl TurnPlan {
    /// The champions of this turn, best first: bans or picks.
    pub fn ranked(&self, banning: bool) -> Vec<u16> {
        if banning {
            self.board.iter().map(|b| b.champ).collect()
        } else {
            self.picks.iter().map(|v| v.champ).collect()
        }
    }
}

/// Ranks a team's bans (by what they take from the enemy's draft) and picks (value in an open
/// lane, how counterable while the enemy has picks left, and half of what it would take from them
/// as a ban); on its turn of a 3+2 draft the best few are played out to the end and lead in the
/// order of the win chance they end in.
pub fn plan_turn(meta: &Meta, t: &TurnInput<'_>, damage: &dyn Fn(u16) -> Option<Damage>) -> TurnPlan {
    let (ally, enemy) = (t.us.picks, t.them.picks);
    let mut board = ban_board(meta, t.us, t.them, Mastery::FULL, damage, t.offer, t.blue);
    let by_ban: HashMap<u16, &BanValue> = board.iter().map(|b| (b.champ, b)).collect();
    let open = open_lanes(meta, ally);
    let their_left = 5usize.saturating_sub(enemy.len());
    let mut picks: Vec<PickValue> = t
        .pickable
        .iter()
        .map(|c| {
            let mut v = with_exposure(meta, pick_value_in(meta, *c, ally, enemy, t.us.players, damage, &open), their_left, t.offer);
            let b = by_ban.get(c);
            v.contest = CONTEST * b.map_or(0.0, |b| b.deny);
            v.conserve = t.conserve * b.map_or(0.0, |b| b.own);
            v.total += v.contest - v.conserve;
            v
        })
        .collect();
    drop(by_ban);
    picks.sort_by(|a, b| b.total.partial_cmp(&a.total).unwrap_or(std::cmp::Ordering::Equal));

    let mut outlook = HashMap::new();
    let at = t.blue.filter(|_| t.lookahead).and_then(|blue| {
        let (bp, rp) = if blue { (ally.len(), enemy.len()) } else { (enemy.len(), ally.len()) };
        order_position(t.bans_made, bp, rp).filter(|i| ORDER_3_2[*i].blue == blue && ORDER_3_2[*i].ban == t.banning).map(|i| (i, blue))
    });
    if let Some((at, blue)) = at {
        let candidates: Vec<u16> = if t.banning {
            board.iter().take(LOOKAHEAD).map(|b| b.champ).collect()
        } else {
            picks.iter().take(LOOKAHEAD).map(|v| v.champ).collect()
        };
        let mut pool = lookahead_pool(meta, t.offer, LOOKAHEAD_POOL);
        for c in &candidates {
            if !pool.contains(c) {
                pool.push(*c);
            }
        }
        let (blue_team, red_team) = if blue { (t.us, t.them) } else { (t.them, t.us) };
        for c in candidates {
            let blue_wins = simulate(meta, blue_team, red_team, &pool, at, c, damage);
            outlook.insert(c, if blue { blue_wins } else { 1.0 - blue_wins });
        }
        let rank = |c: u16| outlook.get(&c).copied().unwrap_or(-1.0);
        if t.banning {
            board.sort_by(|a, b| rank(b.champ).partial_cmp(&rank(a.champ)).unwrap_or(std::cmp::Ordering::Equal));
        } else {
            picks.sort_by(|a, b| rank(b.champ).partial_cmp(&rank(a.champ)).unwrap_or(std::cmp::Ordering::Equal));
        }
    }
    TurnPlan { board, picks, outlook }
}

/// Which side a team is when the game does not say, from where the draft stands: the side whose
/// next step of [`ORDER_3_2`] is this team's ban or pick (`None`: another draft format, or either
/// side fits).
pub fn side_from_order(bans_made: usize, ours: usize, theirs: usize, banning: bool) -> Option<bool> {
    let fits = |blue: bool| {
        let (bp, rp) = if blue { (ours, theirs) } else { (theirs, ours) };
        order_position(bans_made, bp, rp).is_some_and(|i| ORDER_3_2[i].blue == blue && ORDER_3_2[i].ban == banning)
    };
    match (fits(true), fits(false)) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
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
            &Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.1", warm: None, stats: None },
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
        let m = build(&Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.1", warm: None, stats: None }, &Settings::default());
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
    fn bans_take_what_has_no_substitute() {
        let m = meta();
        let none = |_: u16| None;
        let offer: Vec<u16> = (0..NAMES.len() as u16).collect();
        let us = Team::new(&[], &[]);
        let them = Team::new(&[], &[]);
        let board = ban_board(&m, us, them, Mastery::FULL, &none, &offer, Some(false));
        assert_eq!(board.len(), offer.len());
        // c is the strong one in the simulation; l the weak one
        let rank = |n: &str| board.iter().position(|b| b.champ == m.names.get(n).unwrap()).unwrap();
        assert!(rank("c") < 3, "{board:?}");
        assert!(rank("l") > rank("c"));
        assert!(board.windows(2).all(|w| w[0].total >= w[1].total));
        // a ban never helps the enemy
        assert!(board.iter().all(|b| b.deny >= 0.0 && b.own >= 0.0));
        // nothing left to deny once they have picked five
        let five: Vec<u16> = (0..5).collect();
        let offer_rest: Vec<u16> = (5..NAMES.len() as u16).collect();
        let late = ban_board(&m, us, Team::new(&five, &[]), Mastery::FULL, &none, &offer_rest, None);
        assert!(late.iter().all(|b| b.deny == 0.0));
    }

    #[test]
    fn the_3_2_order_and_its_lookahead() {
        assert_eq!(order_position(0, 0, 0), Some(0));
        assert_eq!(order_position(6, 0, 0), Some(6), "first pick");
        assert_eq!(order_position(6, 3, 3), Some(12), "the second ban phase");
        assert_eq!(order_position(10, 3, 3), Some(16));
        assert_eq!(order_position(10, 5, 5), None, "over");
        assert_eq!(order_position(4, 2, 0), None, "not this format");
        assert_eq!(ORDER_3_2.iter().filter(|s| s.ban).count(), 10);
        let m = meta();
        let none = |_: u16| None;
        let offer: Vec<u16> = (0..NAMES.len() as u16).collect();
        let pool = lookahead_pool(&m, &offer, 10);
        assert_eq!(pool.len(), 10);
        let (c, l) = (m.names.get("c").unwrap(), m.names.get("l").unwrap());
        assert!(pool.contains(&c) && !pool.contains(&l), "the strongest are in the pool");
        // blue's first pick: the strong champion leads to a better draft than the weak one
        let blue = Team::new(&[], &[]);
        let red = Team::new(&[], &[]);
        let with_c = super::simulate(&m, blue, red, &pool, 6, c, &none);
        let with_l = super::simulate(&m, blue, red, &[pool.clone(), vec![l]].concat(), 6, l, &none);
        assert!(with_c > with_l, "{with_c} vs {with_l}");
        assert!((0.0..=1.0).contains(&with_c));
    }

    #[test]
    fn habits_make_a_champion_likelier() {
        let m = meta();
        let none = |_: u16| None;
        let offer: Vec<u16> = (0..NAMES.len() as u16).collect();
        let e = m.names.get("e").unwrap();
        let mut habits = HashMap::new();
        habits.insert(e, 1.0);
        let plain = Team::new(&[], &[]);
        let fond = Team { habits: Some(&habits), ..plain };
        assert!(fond.habit(e) > 0.15 && plain.habit(e) == 0.0);
        let rank = |them: Team<'_>| likely_picks(&m, them, &[], Mastery::FULL, &none, &offer).iter().position(|v| v.champ == e).unwrap();
        assert!(rank(fond) < rank(plain), "a team that always plays e is likelier to take it");
        let board = ban_board(&m, plain, fond, Mastery::FULL, &none, &offer, None);
        assert!(board.iter().find(|b| b.champ == e).unwrap().habit > 0.15);
    }

    #[test]
    fn standard_pick_order() {
        // B R R B B R | R B B R
        let order = [(0, 0, true), (1, 0, false), (1, 1, false), (1, 2, true), (2, 2, true), (3, 2, false), (3, 3, false), (3, 4, true), (4, 4, true), (5, 4, false)];
        for (b, r, blue_next) in order {
            assert_eq!(picks_first(b, r, Some(true)), Some(blue_next), "{b}-{r}");
            assert_eq!(picks_first(r, b, Some(false)), Some(!blue_next), "{b}-{r} from red");
        }
        assert_eq!(picks_first(2, 2, None), None);
        assert_eq!(picks_first(1, 2, None), Some(true));
    }

    #[test]
    fn one_sided_damage_costs() {
        let damage = |c: u16| Some(if c < 6 { Damage::Physical } else { Damage::Magic });
        assert_eq!(balance_cost(4, &[0, 1, 2], &damage), ONE_SIDED_COST);
        assert_eq!(balance_cost(7, &[0, 1, 2], &damage), 0.0);
        assert_eq!(balance_cost(4, &[0, 1], &|_| None), 0.0);
    }

    #[test]
    fn hell_bans_what_it_rates() {
        let m = meta();
        let id = |n: &str| m.names.get(n).unwrap();
        let offer: Vec<u16> = (0..12).collect();
        let plain = Team::new(&[], &[]);
        assert_eq!(plain.hell_ban(&m, &offer), None, "not drafted by Hell Draft AI");
        // c is the strongest; a lean on l outweighs it
        let mut lean = HashMap::new();
        let hell = Team { lean: Some(&lean), ..plain };
        assert_eq!(hell.hell_ban(&m, &offer), Some(id("c")));
        lean.insert(id("l"), 2.0);
        let hell = Team { lean: Some(&lean), ..plain };
        assert_eq!(hell.hell_ban(&m, &offer), Some(id("l")));
        assert!(hell.habit(id("l")) >= 2.0, "its picks lean the same way");
    }

    #[test]
    fn the_side_from_where_the_draft_stands() {
        // nothing done yet: blue bans first
        assert_eq!(side_from_order(0, 0, 0, true), Some(true));
        // one ban made: red's
        assert_eq!(side_from_order(1, 0, 0, true), Some(false));
        // six bans, no picks: blue picks first
        assert_eq!(side_from_order(6, 0, 0, false), Some(true));
        // blue has one pick, red none: red picks twice
        assert_eq!(side_from_order(6, 0, 1, false), Some(false));
        // phase two of the bans: red first
        assert_eq!(side_from_order(6, 3, 3, true), Some(false));
        // a pick asked where the order has a ban: another format
        assert_eq!(side_from_order(0, 0, 0, false), None);
    }

    #[test]
    fn a_turn_on_a_full_pool_is_quick_and_plays_out() {
        // the game's pool: two hundred champions besides the twelve with games
        let (names, games) = simulate(3000, "1.1", |n| match n {
            "c" => 0.5,
            "l" => -0.5,
            _ => 0.0,
        }, 11);
        let mut champions: Vec<String> = NAMES.iter().map(|s| s.to_string()).collect();
        champions.extend((0..200).map(|i| format!("x{i}")));
        let m = build(
            &Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.1", warm: None, stats: None },
            &Settings::default(),
        );
        let offer: Vec<u16> = (0..m.names.len() as u16).collect();
        let none = |_: u16| None;
        let started = std::time::Instant::now();
        let turn = TurnInput {
            us: Team::new(&[], &[]),
            them: Team::new(&[], &[]),
            offer: &offer,
            pickable: &offer,
            banning: true,
            blue: Some(true),
            bans_made: 0,
            lookahead: true,
            conserve: 0.0,
        };
        let plan = plan_turn(&m, &turn, &none);
        let took = started.elapsed();
        assert_eq!(plan.outlook.len(), LOOKAHEAD, "the first ban of a 3+2 draft is played out");
        assert!(plan.outlook.contains_key(&plan.board[0].champ), "the lookahead's best leads");
        assert!(took.as_secs_f32() < 2.0, "{took:?}");
        eprintln!("one turn on {} champions: {took:?}", offer.len());
        // the side not known: no lookahead
        let blind = plan_turn(&m, &TurnInput { blue: None, ..turn }, &none);
        assert!(blind.outlook.is_empty());
        // a pick: the strongest
        let pick = plan_turn(&m, &TurnInput { banning: false, bans_made: 6, ..turn }, &none);
        assert_eq!(pick.ranked(false)[0], m.names.get("c").unwrap());
    }
}
