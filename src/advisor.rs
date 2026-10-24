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
//! champion first.

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

/// One side of a draft: its picks so far and its fielded players.
#[derive(Clone, Copy)]
pub struct Team<'a> {
    pub picks: &'a [u16],
    pub players: &'a Roster,
}

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
    let likely = |c: u16| {
        let presence = meta.by_id(c).map_or(0.0, |c| c.presence());
        PRESENCE_WEIGHT * crate::model::presence_factor(presence, meta.typical_presence).ln() / 2f32.ln()
    };
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
            BanValue { champ: *c, deny, own, threat, lanes, irreplaceable, total: deny - own_weight * own }
        })
        .collect();
    board.sort_by(|a, b| b.total.partial_cmp(&a.total).unwrap_or(std::cmp::Ordering::Equal));
    board
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
    fn bans_take_what_has_no_substitute() {
        let m = meta();
        let none = |_: u16| None;
        let offer: Vec<u16> = (0..NAMES.len() as u16).collect();
        let us = Team { picks: &[], players: &[] };
        let them = Team { picks: &[], players: &[] };
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
        let late = ban_board(&m, us, Team { picks: &five, players: &[] }, Mastery::FULL, &none, &offer_rest, None);
        assert!(late.iter().all(|b| b.deny == 0.0));
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
}
