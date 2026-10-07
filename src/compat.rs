//! Other mods that do the same jobs. The game lists the enabled mods in
//! `<game>/config/game/mods.json` (`enabled_mods`, by mod id); that list is read once at start
//! (enabling or disabling a mod needs a restart anyway), and `auto` settings step aside for:
//!
//! - **tier list**: Bowsori's "Drafter's Toolbox" (`drafters_toolkit`) and yudra's "Win-Rate
//!   Ban/Pick AI + Champion Tiers" (`draft_winrate_penalty`) write the same `champion_tiers`.
//!   Two writers overwrite each other every in-game day and the draft screen and the statistics
//!   page end up showing different letters.
//! - **ban/pick**: Bowsori's "Terminator Draft AI" decides every AI ban and pick outright, so a
//!   score nudge does nothing; yudra's mod nudges the same scores, so both together would count
//!   the meta twice.
//!
//! When the list cannot be read nothing is assumed; the tier writer also notices at run time
//! when another writer keeps putting its own list back (see `client`).

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde_json::Value;

use crate::{diag, paths};

/// Mods that write the player team's tier list.
const TIER_WRITERS: [(&str, &str); 2] = [
    ("drafters_toolkit", "Bows' Drafter's Toolbox"),
    ("draft_winrate_penalty", "Win-Rate Ban/Pick AI + Champion Tiers"),
];

/// Mods that lock champions to positions: Smart Position Lock (its locked cards carry a layer
/// the ban/pick overlay reads, see `ui::draft_screen`) and tfm2mods' Champion Position Lock (and
/// flover's rework, same id). They decide the AI's picks where their rules require, which wins
/// over this mod's score nudges - nothing to step aside from.
const POSITION_LOCKS: [(&str, &str); 2] =
    [("smart_position_lock", "Smart Position Lock"), ("tfm2_champ_pos_lock", "Champion Position Lock")];

/// Mods that drive the AI's bans and picks. Matched as a substring: the Terminator's mod id
/// is not published.
const DRAFT_DRIVERS: [(&str, &str); 2] = [
    ("terminator", "Bows' Terminator Draft AI"),
    ("draft_winrate_penalty", "Win-Rate Ban/Pick AI + Champion Tiers"),
];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Others {
    /// Enabled mod ids, `None` when `mods.json` was not readable.
    pub enabled: Option<Vec<String>>,
}

impl Others {
    pub fn from_mods_json(text: &str) -> Self {
        let enabled = serde_json::from_str::<Value>(text).ok().and_then(|doc| {
            doc.get("enabled_mods")?
                .as_array()
                .map(|ids| ids.iter().filter_map(Value::as_str).map(str::to_string).collect())
        });
        Self { enabled }
    }

    fn find(&self, known: &[(&str, &'static str)], substring: bool) -> Option<&'static str> {
        let enabled = self.enabled.as_ref()?;
        known.iter().find_map(|(id, name)| {
            enabled
                .iter()
                .any(|e| if substring { e.contains(id) } else { e == id })
                .then_some(*name)
        })
    }

    /// Another enabled mod that writes the tier list.
    pub fn tier_writer(&self) -> Option<&'static str> {
        self.find(&TIER_WRITERS, false)
    }

    /// Another enabled mod that drives the AI's bans and picks.
    pub fn draft_driver(&self) -> Option<&'static str> {
        self.find(&DRAFT_DRIVERS, true)
    }

    /// Bowsori's Drafter's Toolbox is enabled (it has its own draft-grid labels).
    pub fn toolbox(&self) -> bool {
        self.enabled.as_ref().is_some_and(|e| e.iter().any(|id| id == "drafters_toolkit"))
    }

    /// An enabled mod that locks champions to positions.
    pub fn position_lock(&self) -> Option<&'static str> {
        self.find(&POSITION_LOCKS, false)
    }

    pub fn describe(&self) -> String {
        match &self.enabled {
            None => "enabled mods: unknown (mods.json not readable)".to_string(),
            Some(ids) => format!("enabled mods: {}", ids.join(", ")),
        }
    }
}

static OTHERS: RwLock<Option<std::sync::Arc<Others>>> = RwLock::new(None);

/// `<game>/config/game/mods.json`, found from the game executable.
fn mods_json() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(paths::MODS_JSON_ENV) {
        return Some(PathBuf::from(path));
    }
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("config").join("game").join("mods.json"))
}

/// Reads the enabled-mod list (once per game session).
pub fn load() {
    let others = mods_json()
        .as_deref()
        .and_then(|p: &Path| std::fs::read(p).ok())
        .map(|bytes| Others::from_mods_json(&String::from_utf8_lossy(&bytes)))
        .unwrap_or_default();
    diag::log(&others.describe());
    if let Some(name) = others.tier_writer() {
        diag::log(&format!("\"{name}\" also writes the tier list: tier_list=auto leaves it to that mod"));
    }
    if let Some(name) = others.draft_driver() {
        diag::log(&format!("\"{name}\" also drives the AI's bans and picks: ban_pick=auto leaves it to that mod"));
    }
    if let Some(name) = others.position_lock() {
        diag::log(&format!("\"{name}\" locks positions: its locks hold the AI's picks, and the advice skips cards it locked"));
    }
    *OTHERS.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(std::sync::Arc::new(others));
}

/// The other mods (shared: the draft hook asks for every candidate).
pub fn get() -> std::sync::Arc<Others> {
    OTHERS.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone().unwrap_or_default()
}

/// Test support.
#[doc(hidden)]
pub fn set(others: Others) {
    *OTHERS.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(std::sync::Arc::new(others));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_games_list() {
        let o = Others::from_mods_json(
            r#"{"enabled_mods":["league","patch_meta_ai","drafters_toolkit"],"known_workshop_mods":["x"]}"#,
        );
        assert_eq!(o.tier_writer(), Some("Bows' Drafter's Toolbox"));
        assert_eq!(o.draft_driver(), None);
        assert!(o.toolbox());

        let o = Others::from_mods_json(r#"{"enabled_mods":["bows_terminator_draft"]}"#);
        assert_eq!(o.draft_driver(), Some("Bows' Terminator Draft AI"));
        assert_eq!(o.tier_writer(), None);

        let o = Others::from_mods_json(r#"{"enabled_mods":["patch_meta_ai","tfm2_champ_pos_lock"]}"#);
        assert_eq!(o.position_lock(), Some("Champion Position Lock"));
        assert!(o.tier_writer().is_none() && o.draft_driver().is_none());
        let o = Others::from_mods_json(r#"{"enabled_mods":["smart_position_lock"]}"#);
        assert_eq!(o.position_lock(), Some("Smart Position Lock"));

        let o = Others::from_mods_json(r#"{"enabled_mods":["draft_winrate_penalty"]}"#);
        assert!(o.tier_writer().is_some() && o.draft_driver().is_some());

        let unknown = Others::from_mods_json("not json");
        assert_eq!(unknown.enabled, None);
        assert_eq!(unknown.tier_writer(), None);
        assert!(!unknown.toolbox());
    }
}
