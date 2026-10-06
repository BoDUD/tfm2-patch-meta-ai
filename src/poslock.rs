//! Position lock, without any setting up: a team only picks champions that can still take one of
//! its open positions.
//!
//! The positions a champion can play are
//! - its two **main positions** as the game gives them (the position icons on its ban/pick card,
//!   e.g. what a champion mod set for it), learned on the ban/pick screen and kept in
//!   `positions.json` in the mod folder so AI drafts know them too; plus
//! - every position it has really been played in this save, once there are enough games
//!   (`lock_min_games`) and the position is a real share of them (`lock_share`).
//!
//! A champion with neither is not restricted. A pick is legal when the team's picks and the
//! candidate can still be seated one per position, each in a position it can play - so two
//! top-only champions are not picked together. When nothing on offer is legal (bans and earlier
//! picks used every fitting champion), everything is: a draft never gets stuck.

use std::collections::HashMap;
use std::sync::{PoisonError, RwLock};

use crate::history::Role;
use crate::meta::Meta;
use crate::{diag, paths};

/// Positions a champion can play, in `Role::ALL` order.
pub type Lanes = [bool; 5];

pub const FILE: &str = "positions.json";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rules {
    /// Games a champion needs before its own history adds positions.
    pub min_games: u32,
    /// ... and the share of them a position needs.
    pub share: f32,
}

impl Default for Rules {
    fn default() -> Self {
        Self { min_games: 8, share: 0.15 }
    }
}

/// The game's main positions per champion, learned from ban/pick cards.
static MAIN: RwLock<Option<HashMap<String, Lanes>>> = RwLock::new(None);

/// Reads `positions.json` (main positions learned in earlier sessions).
pub fn load() {
    let path = paths::mod_dir().join(FILE);
    let map: HashMap<String, Lanes> = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<HashMap<String, Vec<String>>>(&b).ok())
        .map(|m| {
            m.into_iter()
                .map(|(c, lanes)| {
                    let mut out = [false; 5];
                    for l in lanes.iter().filter_map(|l| Role::parse(l)) {
                        out[l.index()] = true;
                    }
                    (c, out)
                })
                .collect()
        })
        .unwrap_or_default();
    if !map.is_empty() {
        diag::log(&format!("position lock: main positions of {} champions from {FILE}", map.len()));
    }
    *MAIN.write().unwrap_or_else(PoisonError::into_inner) = Some(map);
}

/// Writes the learned main positions to `positions.json`.
pub fn save() {
    let guard = MAIN.read().unwrap_or_else(PoisonError::into_inner);
    let Some(map) = guard.as_ref() else { return };
    let out: std::collections::BTreeMap<&String, Vec<&str>> = map
        .iter()
        .map(|(c, lanes)| (c, Role::ALL.iter().filter(|r| lanes[r.index()]).map(|r| r.name()).collect()))
        .collect();
    if let Ok(text) = serde_json::to_string_pretty(&out) {
        diag::write_file(FILE, &text);
    }
}

/// Learns a champion's main positions. True when they were new.
pub fn learn(champion: &str, lanes: Lanes) -> bool {
    if lanes == [false; 5] {
        return false;
    }
    let mut guard = MAIN.write().unwrap_or_else(PoisonError::into_inner);
    let map = guard.get_or_insert_with(HashMap::new);
    map.insert(champion.to_string(), lanes) != Some(lanes)
}

pub fn knows(champion: &str) -> bool {
    MAIN.read().unwrap_or_else(PoisonError::into_inner).as_ref().is_some_and(|m| m.contains_key(champion))
}

fn main_of(champion: &str) -> Option<Lanes> {
    MAIN.read().unwrap_or_else(PoisonError::into_inner).as_ref()?.get(champion).copied()
}

/// Test support.
#[doc(hidden)]
pub fn clear() {
    *MAIN.write().unwrap_or_else(PoisonError::into_inner) = None;
}

/// The positions a champion can play (all of them when nothing is known about it).
pub fn allowed(meta: &Meta, champ: u16, rules: &Rules) -> Lanes {
    let name = meta.names.name(champ);
    let mut out = main_of(name).unwrap_or([false; 5]);
    if let Some(c) = meta.by_id(champ) {
        let total: u32 = c.roles.iter().map(|r| r.tally.games).sum();
        if total >= rules.min_games {
            for r in Role::ALL {
                let games = c.roles[r.index()].tally.games;
                if games >= 2 && games as f32 / total as f32 >= rules.share {
                    out[r.index()] = true;
                }
            }
        }
    }
    if out == [false; 5] {
        [true; 5]
    } else {
        out
    }
}

/// Whether the champions can be seated one per position, each in a position it can play.
pub fn fits(team: &[Lanes]) -> bool {
    if team.len() > 5 {
        return false;
    }
    fn seat(team: &[Lanes], used: &mut [bool; 5]) -> bool {
        let Some((first, rest)) = team.split_first() else { return true };
        for p in 0..5 {
            if first[p] && !used[p] {
                used[p] = true;
                let ok = seat(rest, used);
                used[p] = false;
                if ok {
                    return true;
                }
            }
        }
        false
    }
    seat(team, &mut [false; 5])
}

/// Whether a team that picked `team` may pick a champion that plays `cand`.
pub fn legal(team: &[Lanes], cand: Lanes) -> bool {
    if team.len() >= 5 {
        return true;
    }
    let mut all = team.to_vec();
    all.push(cand);
    fits(&all)
}

/// Which of `open` champions the team may pick: the legal ones, or all of them when none is.
pub fn pickable(meta: &Meta, rules: &Rules, team: &[u16], open: &[u16]) -> Vec<u16> {
    let lanes: Vec<Lanes> = team.iter().map(|c| allowed(meta, *c, rules)).collect();
    let legal: Vec<u16> = open.iter().copied().filter(|c| self::legal(&lanes, allowed(meta, *c, rules))).collect();
    if legal.is_empty() {
        open.to_vec()
    } else {
        legal
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const TOP: Lanes = [true, false, false, false, false];
    const JG: Lanes = [false, true, false, false, false];
    const TOP_JG: Lanes = [true, true, false, false, false];
    const MID_SUP: Lanes = [false, false, true, false, true];

    #[test]
    fn seating() {
        assert!(fits(&[TOP, JG]));
        assert!(!fits(&[TOP, TOP]), "two top-only champions");
        assert!(fits(&[TOP, TOP_JG]), "the flexible one takes jungle");
        assert!(!fits(&[TOP, JG, TOP_JG]));
        assert!(legal(&[TOP], MID_SUP) && !legal(&[TOP], TOP));
        assert!(legal(&[TOP, JG, MID_SUP, MID_SUP, [false, false, false, true, false]], TOP), "a full team");
    }

    #[test]
    fn positions_from_the_game_and_from_history() {
        let _serial = crate::tests::serial();
        clear();
        let (names, games) = crate::meta::tests::simulate(400, "1.1", |_| 0.0, 3);
        let champions: Vec<String> = crate::meta::tests::NAMES.iter().map(|s| s.to_string()).collect();
        let meta = crate::meta::build(
            &crate::meta::Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.1", warm: None },
            &crate::meta::Settings::default(),
        );
        let a = meta.names.get("a").unwrap();
        // the simulation plays everyone everywhere: history allows every lane
        assert_eq!(allowed(&meta, a, &Rules::default()), [true; 5]);
        // a demanding rule: only the game's main positions
        let strict = Rules { min_games: 100_000, share: 0.5 };
        assert_eq!(allowed(&meta, a, &strict), [true; 5], "nothing known: unrestricted");
        assert!(learn("a", MID_SUP));
        assert!(!learn("a", MID_SUP), "nothing new");
        assert!(knows("a"));
        assert_eq!(allowed(&meta, a, &strict), MID_SUP);
        // pickable: legal ones, or everything when nothing is
        learn("b", MID_SUP);
        learn("c", TOP);
        let (b, c) = (meta.names.get("b").unwrap(), meta.names.get("c").unwrap());
        assert_eq!(pickable(&meta, &strict, &[a], &[b, c]), [b, c], "b takes the other of mid/support");
        learn("d", MID_SUP);
        let d = meta.names.get("d").unwrap();
        assert_eq!(pickable(&meta, &strict, &[a, b], &[c, d]), [c], "a and b hold mid and support");
        assert_eq!(pickable(&meta, &strict, &[a, b], &[d]), [d], "nothing legal: open up");
        clear();
    }
}
