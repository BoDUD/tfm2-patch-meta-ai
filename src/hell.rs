//! Hell Draft AI (JIN's "地狱 BP", mod id `hell_draft_ai`): it decides every AI team's bans and
//! picks outright, and with its `ally_draft_ai` on the player's coach's too. It rates champions
//! mostly by their win rate in the save (its own count, `cache/history.json`) on top of their
//! stats, and drafts almost without randomness - so what it will take next is predictable.
//!
//! Read from its folder next to this mod's: `config/settings.ini` (`[side]` `ally_draft_ai`,
//! `enemy_draft_ai`, both `on` when not written) and the win counts of its cache. Nothing of it
//! is written.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const MOD_ID: &str = "hell_draft_ai";

/// What this mod knows of Hell Draft AI's set-up.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hell {
    /// It drafts for the player's coach (`ally_draft_ai`).
    pub ally: bool,
    /// It drafts for the AI teams (`enemy_draft_ai`).
    pub enemy: bool,
    /// Its count per champion: (matches, wins), solo rank weighted as it weighs it.
    pub record: HashMap<String, (f32, f32)>,
}

/// Its folder: next to this mod's.
pub fn dir() -> Option<PathBuf> {
    Some(crate::paths::mod_dir().parent()?.join(MOD_ID))
}

/// `on` / `off` of a key in a section of its settings (`None`: not written).
fn switch(ini: &str, section: &str, key: &str) -> Option<bool> {
    let mut inside = false;
    for line in ini.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.starts_with('[') {
            inside = line.trim_matches(|c| c == '[' || c == ']').eq_ignore_ascii_case(section);
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        if inside && k.trim().eq_ignore_ascii_case(key) {
            let v = v.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
            return Some(matches!(v.as_str(), "on" | "true" | "1" | "yes"));
        }
    }
    None
}

pub fn parse(settings: &str, history: &str) -> Hell {
    let record = serde_json::from_str::<serde_json::Value>(history)
        .ok()
        .and_then(|doc| {
            let map = doc.get("map")?.as_object()?;
            Some(
                map.iter()
                    .filter_map(|(name, v)| {
                        let n = v.get("matches")?.as_f64()? as f32;
                        let w = v.get("wins")?.as_f64()? as f32;
                        (n.is_finite() && w.is_finite() && n > 0.0).then(|| (name.clone(), (n, w.clamp(0.0, n))))
                    })
                    .collect(),
            )
        })
        .unwrap_or_default();
    Hell {
        ally: switch(settings, "side", "ally_draft_ai").unwrap_or(true),
        enemy: switch(settings, "side", "enemy_draft_ai").unwrap_or(true),
        record,
    }
}

/// Its set-up from its folder (`None`: no settings found).
pub fn read(dir: &Path) -> Option<Hell> {
    let settings = std::fs::read(dir.join("config").join("settings.ini")).ok()?;
    let history = std::fs::read(dir.join("cache").join("history.json")).unwrap_or_default();
    Some(parse(&String::from_utf8_lossy(&settings), &String::from_utf8_lossy(&history)))
}

/// How much likelier it is to take a champion that is picked or banned a lot this patch (its
/// rating shows in every AI draft): log-odds per doubling of the usual presence, at most twice.
const PRESENCE_WEIGHT: f32 = 0.25;
/// ... and one that wins a lot by its own count: log-odds at 75% (or less at 25%).
const WIN_WEIGHT: f32 = 0.25;
/// Its count is shrunk towards 50% by this many matches.
const WIN_PRIOR_MATCHES: f32 = 5.0;

/// How much likelier Hell Draft AI is to take each champion than our values say (log-odds), from
/// how often the AI teams' drafts have taken it this patch and its own win counts.
pub fn lean(meta: &crate::meta::Meta, hell: &Hell) -> HashMap<u16, f32> {
    meta.champions
        .iter()
        .map(|c| {
            let presence = crate::model::presence_factor(c.presence(), meta.typical_presence).ln() / 2f32.ln();
            let win = hell.record.get(&c.name).map_or(0.0, |(n, w)| {
                let rate = (w + WIN_PRIOR_MATCHES * 0.5) / (n + WIN_PRIOR_MATCHES);
                ((rate - 0.5) / 0.25).clamp(-1.0, 1.0)
            });
            (c.id, PRESENCE_WEIGHT * presence + WIN_WEIGHT * win)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_its_switches_and_counts() {
        let ini = "\u{feff}; Hell Draft AI\n[master]\nenabled=on\n[side]\nenemy_draft_ai=on\nally_draft_ai = off ; mine\n";
        let history = r#"{"fingerprint":1,"map":{"league_chogath":{"matches":34.0,"wins":9.0},"x":{"matches":0.0,"wins":0.0}}}"#;
        let h = parse(ini, history);
        assert!(h.enemy && !h.ally);
        assert_eq!(h.record.get("league_chogath"), Some(&(34.0, 9.0)));
        assert!(!h.record.contains_key("x"), "no matches: no count");
        // not written: its defaults (both sides on); no cache: no counts
        let d = parse("[side]\n", "");
        assert!(d.ally && d.enemy && d.record.is_empty());
        assert_eq!(switch("[master]\nally_draft_ai=on\n", "side", "ally_draft_ai"), None);
    }

    #[test]
    fn leans_to_what_it_plays_and_wins_with() {
        use crate::meta::tests::{simulate, NAMES};
        use crate::meta::{build, Inputs, Settings};
        let (names, games) = simulate(300, "1.1", |_| 0.0, 4);
        let champions: Vec<String> = NAMES.iter().map(|s| s.to_string()).collect();
        let meta = build(
            &Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.1", warm: None, stats: None },
            &Settings::default(),
        );
        let mut hell = Hell { ally: false, enemy: true, record: HashMap::new() };
        hell.record.insert("a".into(), (40.0, 32.0));
        hell.record.insert("b".into(), (40.0, 8.0));
        let lean = lean(&meta, &hell);
        let id = |n: &str| meta.names.get(n).unwrap();
        assert!(lean[&id("a")] > lean[&id("c")] + 0.15 && lean[&id("b")] < lean[&id("c")] - 0.15, "{lean:?}");
        assert!(lean.values().all(|v| v.abs() <= PRESENCE_WEIGHT + WIN_WEIGHT + 1e-4));
    }
}
