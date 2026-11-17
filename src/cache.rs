//! Keeping a save's matches beyond the game's own records. The game prunes old match records;
//! the model and the statistics would lose them. So every save's matches are also written to
//! `history/<save>.json` in the mod folder (never into the save itself), and read back next time.
//!
//! The file is only used for the save it came from. Since 2.8.3 a save has an id of its own, kept in
//! the save, and its file is named by it ([`path_of`]): its matches are taken as soon as the save is
//! open. Before, the file was named after the player's team ([`path`]), and two saves can share a
//! team - so such a file's matches are merged only once the save's own records confirm it: at least
//! [`CONFIRM_SHARE`] of the first [`CONFIRM_MATCHES`] matches read from the save are in the file.
//! Otherwise it is ignored (and this save's are written under its id).
//!
//! Matches are stored with champion and strategy names, never the session's small ids.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::history::{tactic, tactic_id, Game, Names, Role, Slot};
use crate::paths;

pub const DIR: &str = "history";
/// Matches read from the save before the file is checked against them.
pub const CONFIRM_MATCHES: usize = 20;
pub const CONFIRM_SHARE: f32 = 0.8;
/// Matches kept in the file at most (the newest).
pub const MAX_MATCHES: usize = 8000;

#[derive(Serialize, Deserialize)]
struct File {
    v: u32,
    games: Vec<Stored>,
}

/// A player: (champion, lane, athlete, lane gold).
type StoredSlot = (String, Option<String>, Option<u32>, Option<i32>);

#[derive(Serialize, Deserialize)]
struct Stored {
    k: u64,
    s: bool,
    v: String,
    w: bool,
    t: [Option<u32>; 2],
    p: [Vec<StoredSlot>; 2],
    b: [Vec<String>; 2],
    x: [Vec<String>; 2],
    l: Option<f32>,
}

/// A save's name in the mod's files: the player's team id and a hash of its name.
pub fn stem(team: usize, team_name: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    team_name.hash(&mut h);
    format!("team{team}-{:08x}", h.finish() as u32)
}

/// The file for a save ([`stem`]): where its matches were kept before a save had an id of its own.
pub fn path(team: usize, team_name: &str) -> PathBuf {
    paths::mod_dir().join(DIR).join(format!("{}.json", stem(team, team_name)))
}

/// The file for a save by its own id (2.8.3, `server::save_id`): that save's for sure, so its matches are taken
/// without waiting for the save's own records to confirm them.
pub fn path_of(id: &str) -> PathBuf {
    paths::mod_dir().join(DIR).join(format!("save-{id}.json"))
}

pub fn encode<'a>(games: impl Iterator<Item = (&'a u64, &'a Game)>, names: &Names) -> String {
    let mut list: Vec<Stored> = games
        .map(|(k, g)| Stored {
            k: *k,
            s: g.solo,
            v: g.version.clone(),
            w: g.blue_win,
            t: g.teams,
            p: [0, 1].map(|side| {
                g.sides[side]
                    .iter()
                    .map(|s| (names.name(s.champ).to_string(), s.role.map(|r| r.name().to_string()), s.athlete, s.lane_gold))
                    .collect()
            }),
            b: [0, 1].map(|side| g.bans[side].iter().map(|b| names.name(*b).to_string()).collect()),
            x: [0, 1].map(|side| {
                g.tactics[side]
                    .iter()
                    .map(|t| {
                        let (s, o) = tactic(*t);
                        format!("{s}={o}")
                    })
                    .collect()
            }),
            l: g.length,
        })
        .collect();
    // newest last: keep the tail
    if list.len() > MAX_MATCHES {
        list.drain(..list.len() - MAX_MATCHES);
    }
    serde_json::to_string(&File { v: 1, games: list }).unwrap_or_default()
}

/// The matches in a file (record ids 0: they are older than anything the save lists now).
pub fn decode(text: &str, names: &mut Names) -> Vec<(u64, Game)> {
    parse(text).map(|p| build(p, names)).unwrap_or_default()
}

/// A history file parsed (its champions still names: [`build`] gives them the save's ids).
pub struct Parsed(File);

/// A history file's text parsed (None: not a history file of this format).
pub fn parse(text: &str) -> Option<Parsed> {
    let file = serde_json::from_str::<File>(text).ok()?;
    (file.v == 1).then_some(Parsed(file))
}

/// A history file read and parsed on a thread of its own (2.11.0; the user, 2026-11-17: 「主界面读取数据的时候卡顿
/// 优化了吧？」): the files are on the game's disk - on the user's a hard disk, where the 8.6 MB file of a long
/// career held the frame it was read in for about half a second. Tests read it at once.
pub struct Loading {
    slot: Arc<Mutex<Option<Option<Parsed>>>>,
}

impl Loading {
    pub fn start(path: PathBuf) -> Self {
        let slot = Arc::new(Mutex::new(None));
        let out = Arc::clone(&slot);
        let work = move || {
            let parsed = std::fs::read_to_string(&path).ok().and_then(|text| parse(&text));
            *out.lock().unwrap_or_else(PoisonError::into_inner) = Some(parsed);
        };
        if cfg!(test) {
            work();
        } else if let Err(err) = std::thread::Builder::new().name(format!("{}-history-read", crate::MOD_ID)).spawn(work) {
            crate::diag::log(&format!("[cache] could not start the thread that reads the history file: {err}"));
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(None);
        }
        Loading { slot }
    }

    /// One whose read never ends (tests).
    pub fn pending() -> Self {
        Loading { slot: Arc::new(Mutex::new(None)) }
    }

    /// The file once read (`Some(None)`: missing, or not a history file); None while it is being read.
    pub fn take(&self) -> Option<Option<Parsed>> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// A parsed file's matches, their champions given the save's ids.
pub fn build(parsed: Parsed, names: &mut Names) -> Vec<(u64, Game)> {
    parsed
        .0
        .games
        .into_iter()
        .map(|s| {
            let sides = s.p.map(|side| {
                side.into_iter()
                    .map(|(c, lane, athlete, gold)| Slot {
                        champ: names.id(&c),
                        role: lane.as_deref().and_then(Role::parse),
                        athlete,
                        lane_gold: gold,
                    })
                    .collect()
            });
            let game = Game {
                record: 0,
                solo: s.s,
                version: s.v,
                blue_win: s.w,
                teams: s.t,
                sides,
                bans: s.b.map(|side| side.iter().map(|b| names.id(b)).collect()),
                length: s.l,
                tactics: s.x.map(|side| {
                    side.iter().filter_map(|x| x.split_once('=')).map(|(s, o)| tactic_id(s, o)).collect()
                }),
            };
            (s.k, game)
        })
        .collect()
}

/// Whether a file belongs to this save: most of the save's first matches are in it.
pub fn confirms(file_keys: &HashSet<u64>, save_keys: &[u64]) -> bool {
    if save_keys.len() < CONFIRM_MATCHES {
        return false;
    }
    let found = save_keys.iter().filter(|k| file_keys.contains(k)).count();
    found as f32 >= save_keys.len() as f32 * CONFIRM_SHARE
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::tests::simulate;

    #[test]
    fn round_trip_and_save_check() {
        let (mut names, mut games) = simulate(50, "1.3", |_| 0.0, 4);
        games[0].tactics = [vec![tactic_id("early_jungle", "Ganking")], vec![]];
        games[0].sides[0][0].lane_gold = Some(3100);
        let keyed: Vec<(u64, Game)> = games.iter().cloned().enumerate().map(|(i, g)| (i as u64 + 1000, g)).collect();
        let text = encode(keyed.iter().map(|(k, g)| (k, g)), &names);
        // another session: other small ids for the same names
        let mut other = Names::default();
        other.id("zzz");
        let back = decode(&text, &mut other);
        assert_eq!(back.len(), 50);
        let (k, g) = &back[0];
        assert_eq!(*k, 1000);
        assert_eq!(g.record, 0, "older than anything listed now");
        assert_eq!(other.name(g.sides[0][0].champ), names.name(games[0].sides[0][0].champ));
        assert_eq!(g.sides[0][0].lane_gold, Some(3100));
        assert_eq!(tactic(g.tactics[0][0]), ("early_jungle".to_string(), "Ganking".to_string()));
        assert_eq!(g.blue_win, games[0].blue_win);
        assert!(decode("{not json", &mut names).is_empty());

        // the save check
        let file: HashSet<u64> = keyed.iter().map(|(k, _)| *k).collect();
        let same: Vec<u64> = (1000..1030).collect();
        assert!(confirms(&file, &same));
        let other_save: Vec<u64> = (5000..5030).collect();
        assert!(!confirms(&file, &other_save), "another save with the same team");
        assert!(!confirms(&file, &same[..10]), "too few matches to tell yet");
    }
}
