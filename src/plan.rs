//! Planning around the draft, from the meta model:
//!
//! - **strategy** (`tactics_for`): for the team's five champions, the best option of every team
//!   strategy setting - the option's own effect plus how it works with each of the champions -
//!   and how much it is worth over the average option;
//! - **starters** (`best_lineup`): the five players to field, one per position, from each
//!   player's own strength, their position ratings and their champion pool in that position
//!   (champion strength there + their mastery + the game's proficiency);
//! - **seating** (`best_seating`): in the swap phase, which of the team's five champions each
//!   player should take, and what that is worth over the usual lanes;
//! - **training** (`training_for`): the champions a player gains most from practising - strong
//!   in their position this patch, still low in their proficiency.
//!
//! Everything is in log-odds unless said otherwise; nothing here touches the game.

use std::collections::HashMap;

use crate::advisor;
use crate::history::{tactic, Role};
use crate::meta::Meta;

/// What the game records about a player that the plans use.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AthleteInfo {
    pub name: String,
    /// Position ratings (`stat.top` ... `stat.support`; 100 = their main position).
    pub positions: [u8; 5],
    /// The game's champion proficiency (about 230 .. 1000).
    pub proficiency: HashMap<String, u16>,
}

impl AthleteInfo {
    /// Their main position (highest rating).
    pub fn main_position(&self) -> Option<Role> {
        let (i, best) = self.positions.iter().enumerate().max_by_key(|(_, r)| **r)?;
        (*best > 0).then(|| Role::ALL[i])
    }

    /// Reads `stat` and `champion_proficiency` out of an Athlete record.
    pub fn from_record(name: String, stat: &serde_json::Value, proficiency: &serde_json::Value) -> Self {
        let mut positions = [0u8; 5];
        for r in Role::ALL {
            positions[r.index()] =
                stat.get(r.name().to_lowercase()).and_then(serde_json::Value::as_u64).unwrap_or(0).min(255) as u8;
        }
        let proficiency = proficiency
            .as_object()
            .map(|m| {
                m.iter()
                    .filter_map(|(c, v)| {
                        let n = v.get("value").and_then(serde_json::Value::as_u64).or_else(|| v.as_u64())?;
                        Some((c.clone(), n.min(u16::MAX as u64) as u16))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self { name, positions, proficiency }
    }
}

/// The game's proficiency as log-odds: 500 is neutral, 1000 about +0.2 (a small, steady nudge:
/// the save's own games, through the model's mastery, say more once there are any).
fn proficiency_effect(p: Option<u16>) -> f32 {
    p.map_or(0.0, |p| ((p as f32 - 500.0) / 1000.0 * 0.4).clamp(-0.15, 0.25))
}

/// One strategy setting's advice.
#[derive(Clone, Debug, PartialEq)]
pub struct TacticAdvice {
    /// The setting and option as the records name them (`early_jungle`, `CounterJungle`).
    pub setting: String,
    pub option: String,
    /// Worth over the average option of the setting.
    pub gain: f32,
    /// Every option seen often enough, with its value for this team.
    pub options: Vec<(String, f32)>,
}

/// Options seen in fewer games than this are not advised.
const MIN_TACTIC_GAMES: u32 = 8;

/// The best option of every strategy setting for a team of `team` (best gain first).
pub fn tactics_for(meta: &Meta, team: &[u16]) -> Vec<TacticAdvice> {
    let mut by_setting: HashMap<String, Vec<(String, f32)>> = HashMap::new();
    for (t, e) in &meta.tactics {
        if e.tally.games < MIN_TACTIC_GAMES {
            continue;
        }
        let (setting, option) = tactic(*t);
        let with: f32 = team.iter().map(|c| meta.tactic_champ.get(&(*t, *c)).map_or(0.0, |x| x.value)).sum();
        by_setting.entry(setting).or_default().push((option, e.value + with));
    }
    let mut out: Vec<TacticAdvice> = by_setting
        .into_iter()
        .filter(|(_, opts)| opts.len() >= 2)
        .map(|(setting, mut options)| {
            options.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
            let mean = options.iter().map(|o| o.1).sum::<f32>() / options.len() as f32;
            TacticAdvice { option: options[0].0.clone(), gain: options[0].1 - mean, setting, options }
        })
        .collect();
    out.sort_by(|a, b| b.gain.partial_cmp(&a.gain).unwrap_or(std::cmp::Ordering::Equal).then(a.setting.cmp(&b.setting)));
    out
}

/// A player's best champions in a position now: (champion, value), best first. The value is the
/// champion's strength there + the player's mastery (the save's games) + the game's proficiency.
pub fn pool(meta: &Meta, athlete: u32, info: Option<&AthleteInfo>, role: Role, n: usize) -> Vec<(u16, f32)> {
    let mut list: Vec<(u16, f32)> = meta
        .champions
        .iter()
        .filter(|c| c.window.games > 0 || info.is_some_and(|i| i.proficiency.contains_key(&c.name)))
        .map(|c| {
            let prof = info.and_then(|i| i.proficiency.get(&c.name).copied());
            let v = advisor::strength_in(meta, c.id, role) + meta.mastery(athlete, c.id).value + proficiency_effect(prof);
            (c.id, v)
        })
        .collect();
    list.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    list.truncate(n);
    list
}

/// What fielding `athlete` in `role` is worth: their own strength, their top-3 pool there, and
/// their rating for the position (below 50: a position they do not play).
pub fn fielding(meta: &Meta, athlete: u32, info: Option<&AthleteInfo>, role: Role) -> f32 {
    let skill = meta.athletes.get(&athlete).map_or(0.0, |e| e.value);
    let top = pool(meta, athlete, info, role, 3);
    let pool_value = if top.is_empty() { 0.0 } else { top.iter().map(|x| x.1).sum::<f32>() / top.len() as f32 };
    let rating = info.map_or(100, |i| i.positions[role.index()]);
    let fit = if rating < 50 { -1.5 } else { (rating as f32 - 100.0) / 100.0 * 0.3 };
    skill + pool_value + fit
}

/// The best five players for the five positions (`Role::ALL` order) and what each is worth.
pub fn best_lineup(meta: &Meta, squad: &[(u32, Option<&AthleteInfo>)]) -> Option<[(u32, f32); 5]> {
    if squad.len() < 5 {
        return None;
    }
    let value: Vec<[f32; 5]> = squad.iter().map(|(a, i)| Role::ALL.map(|r| fielding(meta, *a, *i, r))).collect();
    let mut best: Option<(f32, [usize; 5])> = None;
    let mut chosen = [usize::MAX; 5];
    let mut used = vec![false; squad.len()];
    fn walk(pos: usize, value: &[[f32; 5]], chosen: &mut [usize; 5], used: &mut [bool], total: f32, best: &mut Option<(f32, [usize; 5])>) {
        if pos == 5 {
            if best.as_ref().is_none_or(|(b, _)| total > *b) {
                *best = Some((total, *chosen));
            }
            return;
        }
        for (i, v) in value.iter().enumerate() {
            if used[i] {
                continue;
            }
            used[i] = true;
            chosen[pos] = i;
            walk(pos + 1, value, chosen, used, total + v[pos], best);
            used[i] = false;
        }
    }
    walk(0, &value, &mut chosen, &mut used, 0.0, &mut best);
    let (_, pick) = best?;
    Some(Role::ALL.map(|r| (squad[pick[r.index()]].0, value[pick[r.index()]][r.index()])))
}

/// The team's seats in the swap phase: a position and the player in it (when known).
pub type Seat = (Role, Option<u32>);

/// The best way to seat `champs` (the team's picks) on `seats`, and what it is worth over the
/// champions in their usual lanes. Returns (seat index -> champion, gain).
pub fn best_seating(meta: &Meta, champs: &[u16], seats: &[Seat], infos: &HashMap<u32, AthleteInfo>) -> Option<(Vec<u16>, f32)> {
    if champs.len() != seats.len() || champs.is_empty() || champs.len() > 5 {
        return None;
    }
    let worth = |c: u16, seat: &Seat| -> f32 {
        let (role, athlete) = *seat;
        let mut v = advisor::strength_in(meta, c, role);
        if let Some(a) = athlete {
            v += meta.mastery(a, c).value;
            let prof = infos.get(&a).and_then(|i| i.proficiency.get(meta.names.name(c)).copied());
            v += proficiency_effect(prof);
        }
        v
    };
    let mut best: Option<(f32, Vec<u16>)> = None;
    let mut order: Vec<u16> = champs.to_vec();
    permute(&mut order, 0, &mut |perm| {
        let total: f32 = perm.iter().zip(seats).map(|(c, s)| worth(*c, s)).sum();
        if best.as_ref().is_none_or(|(b, _)| total > *b) {
            best = Some((total, perm.to_vec()));
        }
    });
    // the usual lanes: each champion where it plays most, as the lane read assigns them
    let lanes = advisor::lanes(meta, champs);
    let usual: f32 = champs
        .iter()
        .zip(&lanes.roles)
        .map(|(c, r)| seats.iter().find(|s| s.0 == *r).map_or(advisor::strength_in(meta, *c, *r), |s| worth(*c, s)))
        .sum();
    let (total, perm) = best?;
    Some((perm, (total - usual).max(0.0)))
}

fn permute(items: &mut [u16], k: usize, f: &mut impl FnMut(&[u16])) {
    if k == items.len() {
        f(items);
        return;
    }
    for i in k..items.len() {
        items.swap(k, i);
        permute(items, k + 1, f);
        items.swap(k, i);
    }
}

/// The champions `athlete` gains most from practising in `role`: strong there this patch and
/// low in their proficiency. (champion, strength in the role, proficiency), best first.
pub fn training_for(meta: &Meta, info: &AthleteInfo, role: Role, n: usize) -> Vec<(u16, f32, u16)> {
    let mut list: Vec<(u16, f32, u16, f32)> = meta
        .champions
        .iter()
        .filter(|c| c.window.games >= 5 && c.role_share()[role.index()] >= 0.15)
        .filter_map(|c| {
            let prof = info.proficiency.get(&c.name).copied().unwrap_or(0);
            let strength = advisor::strength_in(meta, c.id, role);
            // worth practising: above average in the role, and room to grow
            (strength > 0.0 && prof < 700).then(|| (c.id, strength, prof, strength * (1.0 - prof as f32 / 1000.0)))
        })
        .collect();
    list.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));
    list.into_iter().take(n).map(|(c, s, p, _)| (c, s, p)).collect()
}

/// A record option name as the game's UI node name: `CounterJungle` -> `counter_jungle`,
/// `Split131` -> `split131`.
pub fn snake(option: &str) -> String {
    let mut out = String::new();
    for (i, ch) in option.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glm::tests::Lcg;
    use crate::meta::tests::{simulate, NAMES};
    use crate::meta::{build, Inputs, Settings};

    fn meta_with_tactics() -> Meta {
        let (names, mut games) = simulate(2500, "1.1", |n| if n == "c" { 0.4 } else { 0.0 }, 23);
        let (gank, farm) = (crate::history::tactic_id("plan_jungle", "Ganking"), crate::history::tactic_id("plan_jungle", "GrowthAndCover"));
        let mut rng = Lcg(8);
        for g in &mut games {
            let (b, r) = (rng.next() < 0.5, rng.next() < 0.5);
            g.tactics = [vec![if b { gank } else { farm }], vec![if r { gank } else { farm }]];
            if b != r {
                g.blue_win = rng.next() < if b { 0.62 } else { 0.38 };
            }
        }
        let champions: Vec<String> = NAMES.iter().map(|s| s.to_string()).collect();
        build(&Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.1", warm: None }, &Settings::default())
    }

    #[test]
    fn strategy_advice() {
        let meta = meta_with_tactics();
        let team: Vec<u16> = ["a", "b", "c", "d", "e"].iter().map(|n| meta.names.get(n).unwrap()).collect();
        let advice = tactics_for(&meta, &team);
        let jungle = advice.iter().find(|a| a.setting == "plan_jungle").expect("advice for the setting");
        assert_eq!(jungle.option, "Ganking");
        assert!(jungle.gain > 0.05 && jungle.options.len() == 2, "{jungle:?}");
        assert_eq!(snake("CounterJungle"), "counter_jungle");
        assert_eq!(snake("Split131"), "split131");
        assert_eq!(snake("Must"), "must");
    }

    #[test]
    fn starters_seating_and_training() {
        let meta = meta_with_tactics();
        // ten players: the ones rated for a position go there; athlete 1 is the strong one
        let infos: Vec<AthleteInfo> = (0..10)
            .map(|i| {
                let mut positions = [0u8; 5];
                positions[i % 5] = 100;
                AthleteInfo { name: format!("p{i}"), positions, proficiency: HashMap::new() }
            })
            .collect();
        let squad: Vec<(u32, Option<&AthleteInfo>)> = (0..10).map(|i| (i as u32 + 1, Some(&infos[i]))).collect();
        let lineup = best_lineup(&meta, &squad).unwrap();
        for (r, (a, _)) in Role::ALL.iter().zip(lineup) {
            let i = (a - 1) as usize;
            assert_eq!(infos[i].positions[r.index()], 100, "{a} fielded off-position in {r:?}");
        }
        assert!(lineup.iter().any(|(a, _)| *a == 1), "the strongest player starts: {lineup:?}");
        assert!(best_lineup(&meta, &squad[..4]).is_none());

        // seating: five champions on five seats, never worse than the usual lanes
        let champs: Vec<u16> = ["a", "b", "c", "d", "e"].iter().map(|n| meta.names.get(n).unwrap()).collect();
        let seats: Vec<Seat> = Role::ALL.iter().enumerate().map(|(i, r)| (*r, Some(i as u32 + 1))).collect();
        let (perm, gain) = best_seating(&meta, &champs, &seats, &HashMap::new()).unwrap();
        assert_eq!(perm.len(), 5);
        let mut sorted = perm.clone();
        sorted.sort();
        let mut expected = champs.clone();
        expected.sort();
        assert_eq!(sorted, expected, "every champion seated once");
        assert!(gain >= 0.0);
        assert!(best_seating(&meta, &champs[..3], &seats, &HashMap::new()).is_none());

        // training: strong in the role, low proficiency; mastered champions drop out
        let mut info = AthleteInfo { name: "x".into(), positions: [100, 0, 0, 0, 0], proficiency: HashMap::new() };
        let plan = training_for(&meta, &info, Role::Top, 3);
        assert!(!plan.is_empty() && plan.iter().all(|(_, s, _)| *s > 0.0), "{plan:?}");
        let first = plan[0].0;
        info.proficiency.insert(meta.names.name(first).to_string(), 300);
        assert!(training_for(&meta, &info, Role::Top, 3).iter().any(|(c, _, p)| *c == first && *p == 300));
        info.proficiency.insert(meta.names.name(first).to_string(), 950);
        assert!(!training_for(&meta, &info, Role::Top, 3).iter().any(|(c, _, _)| *c == first), "already mastered");
    }

    #[test]
    fn athlete_records() {
        let stat = serde_json::json!({"top": 100, "jungle": 40, "mid": 0, "bottom": 0, "support": 0, "ego": 50});
        let prof = serde_json::json!({"ahri": {"floor": 120, "value": 640}, "zed": 300});
        let info = AthleteInfo::from_record("Kiin".into(), &stat, &prof);
        assert_eq!(info.positions, [100, 40, 0, 0, 0]);
        assert_eq!(info.main_position(), Some(Role::Top));
        assert_eq!(info.proficiency["ahri"], 640);
        assert_eq!(info.proficiency["zed"], 300);
        assert!(proficiency_effect(Some(1000)) > 0.15 && proficiency_effect(None) == 0.0);
    }
}
