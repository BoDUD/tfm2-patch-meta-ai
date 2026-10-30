//! Smart Position Lock's lanes (mod id `smart_position_lock`). The lock holds every pick that
//! would leave a team unable to seat its champions one per lane - by its score hook, which a
//! deciding hook (`draft::CoachHook`: the coach override, `ai_draft`) goes past. So those
//! decisions apply the lock's own rule here, with the lock's own table:
//!
//! - a champion's lanes: the lock's `settings.ini` `[positions]` (the player's list), else its
//!   built-in list (compiled into the lock; this mod carries the same file,
//!   `knowledge::POSITIONS` - the `league_positions.ini` in the lock's folder is an older copy
//!   for reading), else the main positions it learned from the ban/pick cards
//!   (`positions.json`), else every lane;
//! - a pick is legal when the team's champions and it can still be seated one per lane (when
//!   none is legal, any is - as the lock does);
//! - the lock's switches: `[lock]` `enabled`, `ai` (AI teams), `player` (the player's team).
//!
//! The same table also gives the model its League lanes (`knowledge::positions_of`), so a
//! champion's value is worked out in the lanes the lock lets it play. Read from the lock's folder
//! next to this mod's, again whenever its `settings.ini` changes (checked every few seconds).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::history::Role;

pub const MOD_ID: &str = "smart_position_lock";

/// Lanes a champion may be seated in (`Role::index` order).
pub type Lanes = [bool; 5];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    /// The player's list and the built-in one merged (the player's wins), main lane first.
    pub positions: HashMap<String, Vec<Role>>,
    /// Main positions learned from the ban/pick cards (champions in neither list).
    pub learned: HashMap<String, Vec<Role>>,
    /// The lock holds AI teams' picks / the player's team's.
    pub ai: bool,
    pub player: bool,
}

impl Table {
    /// The lanes `champ` may play: every lane when nothing is known about it.
    pub fn allowed(&self, champ: &str) -> Lanes {
        let roles = self.positions.get(champ).or_else(|| self.learned.get(champ));
        match roles {
            Some(list) if !list.is_empty() => {
                let mut out = [false; 5];
                for r in list {
                    out[r.index()] = true;
                }
                out
            }
            _ => [true; 5],
        }
    }

    /// Which of `offer` a team that picked `team` may pick: the legal ones, or all of them when
    /// none is.
    pub fn pickable(&self, team: &[&str], offer: &[(u16, &str)]) -> Vec<u16> {
        let lanes: Vec<Lanes> = team.iter().map(|c| self.allowed(c)).collect();
        let legal: Vec<u16> = offer.iter().filter(|(_, n)| legal(&lanes, self.allowed(n))).map(|(c, _)| *c).collect();
        if legal.is_empty() {
            offer.iter().map(|(c, _)| *c).collect()
        } else {
            legal
        }
    }
}

/// Whether the champions can be seated one per lane, each in a lane it can play.
pub fn fits(team: &[Lanes]) -> bool {
    fn place(team: &[Lanes], used: &mut [bool; 5]) -> bool {
        let Some((first, rest)) = team.split_first() else { return true };
        for lane in 0..5 {
            if first[lane] && !used[lane] {
                used[lane] = true;
                let ok = place(rest, used);
                used[lane] = false;
                if ok {
                    return true;
                }
            }
        }
        false
    }
    team.len() <= 5 && place(team, &mut [false; 5])
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

/// `id=Lane,Lane ; comment` lines of an ini section.
fn section(text: &str, name: &str) -> Vec<(String, String)> {
    let mut inside = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.starts_with('[') {
            inside = line.trim_matches(|c| c == '[' || c == ']').eq_ignore_ascii_case(name);
            continue;
        }
        let line = line.split(';').next().unwrap_or("").trim();
        if let (true, Some((k, v))) = (inside, line.split_once('=')) {
            out.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    out
}

fn positions(text: &str) -> HashMap<String, Vec<Role>> {
    section(text, "positions")
        .into_iter()
        .filter_map(|(id, list)| {
            let roles: Vec<Role> = list.split(',').filter_map(|r| Role::parse(r.trim())).collect();
            (!roles.is_empty()).then_some((id, roles))
        })
        .collect()
}

fn switch(text: &str, key: &str) -> bool {
    section(text, "lock")
        .into_iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .is_none_or(|(_, v)| matches!(v.to_ascii_lowercase().as_str(), "on" | "true" | "1" | "yes"))
}

/// The table from the lock's files' contents (`learned`: its `positions.json`).
pub fn parse(settings: &str, preset: &str, learned: &str) -> Table {
    let mut merged = positions(preset);
    merged.extend(positions(settings));
    let learned: HashMap<String, Vec<Role>> = serde_json::from_str::<HashMap<String, Vec<String>>>(learned)
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| (k, v.iter().filter_map(|r| Role::parse(r)).collect::<Vec<Role>>()))
        .filter(|(k, v)| !v.is_empty() && !merged.contains_key(k))
        .collect();
    let enabled = switch(settings, "enabled");
    Table { positions: merged, learned, ai: enabled && switch(settings, "ai"), player: enabled && switch(settings, "player") }
}

/// Its folder: next to this mod's.
pub fn dir() -> Option<PathBuf> {
    Some(crate::paths::mod_dir().parent()?.join(MOD_ID))
}

fn read(dir: &Path) -> Option<Table> {
    let text = |name: &str| std::fs::read(dir.join(name)).ok().map(|b| String::from_utf8_lossy(&b).into_owned());
    let settings = text("settings.ini")?;
    Some(parse(&settings, crate::knowledge::POSITIONS, &text("positions.json").unwrap_or_default()))
}

static TABLE: RwLock<Option<Arc<Table>>> = RwLock::new(None);
/// When its settings were last checked (seconds since the epoch) and their modification time.
static CHECKED: AtomicU64 = AtomicU64::new(0);
static STAMP: AtomicU64 = AtomicU64::new(0);
const CHECK_EVERY: u64 = 5;

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn stamp(dir: &Path) -> u64 {
    std::fs::metadata(dir.join("settings.ini"))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as u64)
}

/// Reads the table when the lock is enabled (`compat`), and says so in `diag.log`.
pub fn load(enabled: bool) {
    let table = enabled.then(dir).flatten().and_then(|d| {
        STAMP.store(stamp(&d), Ordering::Relaxed);
        read(&d)
    });
    CHECKED.store(now(), Ordering::Relaxed);
    if let Some(t) = &table {
        crate::diag::log(&format!(
            "[lock] Smart Position Lock's lanes: {} champions listed, {} learned; it holds AI teams' picks: {}, the player's: {}",
            t.positions.len(),
            t.learned.len(),
            if t.ai { "on" } else { "off" },
            if t.player { "on" } else { "off" }
        ));
    }
    set(table);
}

/// The lock's table (`None`: the lock is not enabled or its settings were not found), read again
/// when its settings changed.
pub fn get() -> Option<Arc<Table>> {
    let current = TABLE.read().unwrap_or_else(PoisonError::into_inner).clone();
    let t = now();
    if current.is_some() && t >= CHECKED.load(Ordering::Relaxed) + CHECK_EVERY {
        CHECKED.store(t, Ordering::Relaxed);
        if let Some(d) = dir() {
            let s = stamp(&d);
            if s != STAMP.swap(s, Ordering::Relaxed) {
                if let Some(fresh) = read(&d) {
                    let fresh = Arc::new(fresh);
                    *TABLE.write().unwrap_or_else(PoisonError::into_inner) = Some(fresh.clone());
                    return Some(fresh);
                }
            }
        }
    }
    current
}

/// Test support, and [`load`].
#[doc(hidden)]
pub fn set(table: Option<Table>) {
    *TABLE.write().unwrap_or_else(PoisonError::into_inner) = table.map(Arc::new);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_locks_table_and_switches() {
        let settings = "\u{feff}[lock]\nenabled=on\nai=on\nplayer=off\n[positions]\nleague_ziggs=Mid ; 吉格斯\nleague_amumu=Jungle\n";
        let preset = "[positions]\nleague_ziggs=Mid,Bottom\nleague_ahri=Mid\nleague_amumu=Jungle,Support\n";
        let learned = r#"{"archer": ["Mid", "Bottom"], "league_ahri": ["Top"]}"#;
        let t = parse(settings, preset, learned);
        assert!(t.ai && !t.player);
        assert_eq!(t.positions["league_ziggs"], vec![Role::Mid], "the player's list wins");
        assert_eq!(t.positions["league_ahri"], vec![Role::Mid]);
        assert_eq!(t.learned.get("archer"), Some(&vec![Role::Mid, Role::Bottom]));
        assert!(!t.learned.contains_key("league_ahri"), "listed: not learned");
        assert_eq!(t.allowed("nobody"), [true; 5]);
        // switched off as a whole
        assert!(!parse("[lock]\nenabled=off\n", "", "").ai);
        // not written: on
        assert!(parse("[lock]\n", "", "").player);
    }

    #[test]
    fn seats_one_per_lane() {
        let t = parse(
            "[positions]\na=Mid\nb=Mid,Bottom\nc=Mid\nd=Top\n",
            "",
            "",
        );
        // a in Mid: b only fits Bottom; c (Mid only) no longer fits
        let offer = [(1, "b"), (2, "c"), (3, "d"), (4, "free")];
        assert_eq!(t.pickable(&["a"], &offer), vec![1, 3, 4]);
        // a and b seated (Mid, Bottom): c still cannot be
        assert_eq!(t.pickable(&["a", "b"], &offer[1..]), vec![3, 4]);
        // nothing legal: anything
        assert_eq!(t.pickable(&["a"], &[(2, "c")]), vec![2]);
        assert!(fits(&[[true, false, false, false, false]; 1]));
        assert!(!fits(&[[true, false, false, false, false]; 2]));
    }
}

/// The table read from a real lock folder:
/// `PMA_LOCK_DIR=<mods>/smart_position_lock cargo test --release --lib lock::real -- --ignored --nocapture`
#[cfg(test)]
mod real {
    use super::*;

    #[test]
    #[ignore]
    fn the_installed_locks_table() {
        let Some(dir) = std::env::var_os("PMA_LOCK_DIR") else { return };
        let t = read(Path::new(&dir)).expect("the lock's settings");
        eprintln!("{} listed, {} learned, ai {}, player {}", t.positions.len(), t.learned.len(), t.ai, t.player);
        let bottom = [false, false, false, true, false];
        assert_eq!(t.allowed("league_zeri"), bottom, "the lock keeps Zeri in the bottom lane");
        assert_eq!(t.allowed("league_tristana"), bottom);
        let offer = [(1, "league_zeri"), (2, "league_ahri")];
        assert_eq!(t.pickable(&["league_tristana"], &offer), vec![2], "Tristana and Zeri cannot both be seated");
        for c in ["tft_loris", "league_nunu", "tft_krug", "league_alistar"] {
            eprintln!("  {c}: {:?}", t.allowed(c));
        }
    }
}
