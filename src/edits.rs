//! The player's own numbers (2.11.0; the user, 2026-11-17: 「英雄的平衡补丁能不能不覆盖玩家修改的数据库」, then:
//! 「玩家在数据库里改过数值的英雄和装备，电竞大脑自动识别出来，可以给它们出平衡补丁，但是不覆盖玩家数据库里改的，完全按
//! 玩家的数值来，后面轮次进行正常削弱加强；另加一个设置可以手动列出不要调的英雄」).
//!
//! The balance never writes a champion's numbers: its power is a buff on top of whatever numbers the champion has in
//! the save (the patch notes' lines, the stat panels and the matches all start from them). What a player changes in
//! the database is a champion's numbers - so this mod's power left on it would still move it away from what the
//! player set. So the balance watches every champion's numbers ([`Numbers`], the game's stat block with its growth,
//! as the save has them) and, when they change, tells who changed them ([`judge`]):
//!
//! - **a mod update**: the champion's definition in the mods' files changed too ([`definitions`]: the
//!   `.data_champion` files of the enabled mods, with the overrides their `mod.override_info` declare) - its new
//!   numbers are the mod's, and the balance carries on as it was;
//! - **the player**: the files are as they were. The balance's power on that champion is taken off at once (a hotfix
//!   round of its own, `balance::make_reset`), so it plays exactly the player's numbers; from there the rounds judge
//!   it again on the games it plays with them, as any champion.
//!
//! The game's own champion patches would change numbers too; they are off while the balance is on (`server::balance`),
//! and numbers that changed while it was off are taken as they are, without a verdict.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::diag;

/// The game's stat block of a champion and its growth per level (`StatV1`, nine numbers each: attack, magic power,
/// health, armour, magic resistance, move speed, health regeneration, stacks, crit chance).
pub type Numbers = [u32; 18];

/// The fields of a stat block, in [`Numbers`]' order (the game's `EntityStat`, as `.data_champion` writes them).
pub const FIELDS: [&str; 9] = ["attack", "magic_power", "hp", "defence", "magic_resistance", "move_speed", "hp_regen", "stack", "crit_chance"];

/// A champion's numbers as the game hands them over.
pub fn numbers(stat: &mod_api_stable::StatV1, growth: &mod_api_stable::StatV1) -> Numbers {
    let part = |s: &mod_api_stable::StatV1| {
        [s.attack, s.magic_power, s.hp, s.defence, s.magic_resistance, s.move_speed, s.hp_regen, s.stack, s.crit_chance]
            .map(|v| u32::try_from(v).unwrap_or(u32::MAX))
    };
    let mut out = [0; 18];
    out[..9].copy_from_slice(&part(stat));
    out[9..].copy_from_slice(&part(growth));
    out
}

/// A champion's numbers in a `.data_champion` document (`stat` and `growth`; a field it leaves out is 0). None: not a
/// champion's document.
pub fn numbers_of(doc: &Value) -> Option<Numbers> {
    let (stat, growth) = (doc.get("stat")?.as_object()?, doc.get("growth")?.as_object()?);
    let mut out = [0; 18];
    for (i, f) in FIELDS.iter().enumerate() {
        let get = |m: &serde_json::Map<String, Value>| m.get(*f).and_then(Value::as_f64).map_or(0, |v| v.round().max(0.0) as u32);
        out[i] = get(stat);
        out[9 + i] = get(growth);
    }
    Some(out)
}

/// What a champion's numbers looked like the last time they were read, and its definition in the mods' files then
/// (None: a champion of the game itself, or the files not read).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    #[serde(rename = "n")]
    pub numbers: Vec<u32>,
    #[serde(rename = "d", default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<Vec<u32>>,
}

impl Seen {
    pub fn new(numbers: &Numbers, definition: Option<&Numbers>) -> Self {
        Seen { numbers: numbers.to_vec(), definition: definition.map(|d| d.to_vec()) }
    }
}

/// Who changed a champion's numbers since they were last read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Not read before (a new champion, or the first read of this save).
    New,
    Same,
    /// Its definition in the mods' files changed with them: a mod's update.
    ModUpdate,
    /// The files are as they were: the player (the database).
    Player,
}

/// See the module notes.
pub fn judge(seen: Option<&Seen>, now: &Numbers, definition: Option<&Numbers>) -> Verdict {
    let Some(seen) = seen else { return Verdict::New };
    if seen.numbers == now.as_slice() {
        return Verdict::Same;
    }
    if seen.definition.as_deref() != definition.map(|d| d.as_slice()) {
        Verdict::ModUpdate
    } else {
        Verdict::Player
    }
}

// ===================== the mods' definitions =====================

#[derive(Deserialize)]
struct Override {
    #[serde(default)]
    remapping: String,
    #[serde(rename = "type", default)]
    kind: String,
}

/// The champion an asset path is (`asset/<mod>/champion/<id>`): the id. None: not a champion's asset.
fn champion_asset(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("asset/")?;
    let (_, rest) = rest.split_once('/')?;
    let id = rest.strip_prefix("champion/")?;
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

/// A remapped asset's file: `asset/<mod>/<path>` -> `<mods>/<mod>/<path>.data_champion`.
fn asset_file(mods: &Path, asset: &str) -> Option<PathBuf> {
    let rest = asset.strip_prefix("asset/")?;
    let (module, path) = rest.split_once('/')?;
    Some(mods.join(module).join(format!("{path}.data_champion")))
}

fn read_doc(file: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(file).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

/// Every champion's definition in the enabled mods' files (`enabled` in the game's order): each mod's own
/// `champion/*.data_champion`, then the overrides the mods declare in `mod.override_info` (`"type": "override"` of a
/// champion's asset), a later mod's over an earlier one's. Champions of the game itself have none unless a mod
/// overrides them.
pub fn definitions(mods: &Path, enabled: &[String]) -> HashMap<String, Numbers> {
    // asset path -> (champion id, numbers)
    let mut by_asset: HashMap<String, (String, Numbers)> = HashMap::new();
    for module in enabled {
        let Ok(dir) = std::fs::read_dir(mods.join(module).join("champion")) else { continue };
        for entry in dir.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("data_champion") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
            let Some(doc) = read_doc(&path) else { continue };
            let Some(n) = numbers_of(&doc) else { continue };
            let id = doc.get("id").and_then(Value::as_str).map_or_else(|| stem.clone(), str::to_string);
            by_asset.insert(format!("asset/{module}/champion/{stem}"), (id, n));
        }
    }
    for module in enabled {
        let Ok(text) = std::fs::read_to_string(mods.join(module).join("mod.override_info")) else { continue };
        let Ok(overrides) = serde_json::from_str::<HashMap<String, Override>>(text.trim_start_matches('\u{feff}')) else { continue };
        let mut keys: Vec<&String> = overrides.keys().collect();
        keys.sort();
        for key in keys {
            let o = &overrides[key];
            let Some(stem) = champion_asset(key) else { continue };
            if o.kind != "override" {
                continue;
            }
            let Some(doc) = asset_file(mods, &o.remapping).and_then(|f| read_doc(&f)) else { continue };
            let Some(n) = numbers_of(&doc) else { continue };
            let id = by_asset
                .get(key)
                .map(|(id, _)| id.clone())
                .or_else(|| doc.get("id").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_else(|| stem.to_string());
            by_asset.insert(key.clone(), (id, n));
        }
    }
    by_asset.into_values().collect()
}

/// The definitions of this game session (the files do not change while the game runs): read once on a thread of
/// their own (some 20 MB of JSON with the League packs); None until then.
/// The session's definitions: whether their read began, and what it read.
type Definitions = (bool, Option<Arc<HashMap<String, Numbers>>>);
static DEFS: LazyLock<Mutex<Definitions>> = LazyLock::new(|| Mutex::new((false, None)));

pub fn session_definitions() -> Option<Arc<HashMap<String, Numbers>>> {
    let mut guard = DEFS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(defs) = &guard.1 {
        return Some(Arc::clone(defs));
    }
    if !guard.0 {
        guard.0 = true;
        let mods = crate::paths::mod_dir().parent().map(Path::to_path_buf);
        let enabled = crate::compat::get().enabled.clone().unwrap_or_default();
        let spawned = std::thread::Builder::new().name("pma-definitions".into()).spawn(move || {
            let started = std::time::Instant::now();
            let defs = mods.map(|m| definitions(&m, &enabled)).unwrap_or_default();
            diag::log(&format!(
                "[edits] the mods' champion definitions read: {} champions in {} ms",
                defs.len(),
                started.elapsed().as_millis()
            ));
            DEFS.lock().unwrap_or_else(PoisonError::into_inner).1 = Some(Arc::new(defs));
        });
        if spawned.is_err() {
            // no thread: read here (once)
            let defs = crate::paths::mod_dir().parent().map(|m| definitions(m, &crate::compat::get().enabled.clone().unwrap_or_default())).unwrap_or_default();
            guard.1 = Some(Arc::new(defs));
            return guard.1.clone();
        }
    }
    None
}

/// Test support: the session's definitions as given.
#[cfg(test)]
pub fn set_definitions_for_tests(defs: HashMap<String, Numbers>) {
    *DEFS.lock().unwrap_or_else(PoisonError::into_inner) = (true, Some(Arc::new(defs)));
}

/// The names of `balance_skip` / `item_balance_skip` as given (ids or the names the game shows), split on commas,
/// Chinese commas, semicolons and blanks.
pub fn list(text: &str) -> Vec<String> {
    text.split([',', '，', ';', '；', '、', ' ', '\t'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(attack: u32) -> Numbers {
        let mut out = [0; 18];
        out[0] = attack;
        out[2] = 900;
        out
    }

    #[test]
    fn who_changed_the_numbers() {
        let def = n(100);
        assert_eq!(judge(None, &n(100), Some(&def)), Verdict::New);
        let seen = Seen::new(&n(70), Some(&def));
        assert_eq!(judge(Some(&seen), &n(70), Some(&def)), Verdict::Same);
        // the files as they were: the player
        assert_eq!(judge(Some(&seen), &n(90), Some(&def)), Verdict::Player);
        // the files changed with them: a mod's update
        assert_eq!(judge(Some(&seen), &n(80), Some(&n(110))), Verdict::ModUpdate);
        // a champion of the game itself (no definition): every change is the player's
        let base = Seen::new(&n(50), None);
        assert_eq!(judge(Some(&base), &n(55), None), Verdict::Player);
        // the definition read for the first time (files not read before): a mod's update, not a verdict on the player
        assert_eq!(judge(Some(&base), &n(55), Some(&def)), Verdict::ModUpdate);
        // kept in the book as JSON
        let json = serde_json::to_string(&seen).unwrap();
        assert_eq!(serde_json::from_str::<Seen>(&json).unwrap(), seen);
        assert!(!serde_json::to_string(&base).unwrap().contains("\"d\""));
    }

    #[test]
    fn definitions_follow_the_mods_overrides() {
        let dir = std::env::temp_dir().join(format!("pma_defs_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let write = |path: &str, text: &str| {
            let p = dir.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        let doc = |id: &str, attack: u32| {
            format!(r#"{{"id":"{id}","stat":{{"attack":{attack},"hp":900}},"growth":{{"attack":20,"hp":90}},"skill":{{}}}}"#)
        };
        write("league/champion/league_jinx.data_champion", &doc("league_jinx", 100));
        write("league/champion/league_kindred.data_champion", &doc("league_kindred", 90));
        write("addons/override/league_kindred.data_champion", &doc("league_kindred", 88));
        write(
            "addons/mod.override_info",
            r#"{"asset/league/champion/league_kindred": {"remapping": "asset/addons/override/league_kindred", "type": "override"},
                "asset/base/text/champion": {"remapping": "asset/addons/text/champion", "type": "merge"}}"#,
        );
        write("off/champion/league_jinx.data_champion", &doc("league_jinx", 1));
        let defs = definitions(&dir, &["league".to_string(), "addons".to_string()]);
        assert_eq!(defs.len(), 2);
        assert_eq!(defs["league_jinx"][0], 100, "a mod not enabled is not read");
        assert_eq!(defs["league_kindred"][0], 88, "the override wins");
        assert_eq!((defs["league_kindred"][2], defs["league_kindred"][9], defs["league_kindred"][11]), (900, 20, 90));
        // without the overriding mod: the league file's own
        let defs = definitions(&dir, &["league".to_string()]);
        assert_eq!(defs["league_kindred"][0], 90);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(list("league_jinx， 金克丝;  tft_maddie、德莱厄斯"), vec!["league_jinx", "金克丝", "tft_maddie", "德莱厄斯"]);
    }
}
