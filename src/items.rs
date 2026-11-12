//! The item balance (2.9.0, `item_balance`; the user, 2026-11-13: 「那可以加一个46的装备平衡补丁吧」 → 「完整版补丁」):
//! balance patches for the items of alele's "4+6" pack (`alele_runes_and_items` + `alele_equipment_selection`), made
//! with the champions' rounds ([`crate::balance`]).
//!
//! - **What the pack does** (read from its files and a match's buffs, 2026-11-11..13): its items' numbers are built
//!   into its DLL. A champion holding an item carries a permanent buff named `item_owned_<mod id>_<key>_<8 hex>` with
//!   the item's numbers (the key cut so the name fits a buff's 64 bytes: 20 characters for `alele_runes_and_items`).
//!   `item-catalog.json` in its folder lists every item (key, name, tier, kind, numbers under `packageValues.stats`,
//!   named as the buff's fields), and `ai-tail-builds.json` in the equipment mod's folder - what its selector
//!   publishes for the game - every hero's build: six item slots, a `specified` one naming its item.
//! - **Who builds what** ([`Builds`]): a hero's specified tier-5 equipment in the published builds. The heroes of the
//!   user's packs have all six specified; the game's own heroes, none (the pack picks theirs in the match).
//! - **In a match** ([`corrections`]): an item at p percent of power (the round's
//!   [`crate::balance::Round::item_power`]) gives its holder a second permanent buff, `patch_meta_ai_item_<n>`, with
//!   p percent of each number of the item's buff - -10 takes a tenth of each away. Numbers only: an item's own
//!   effects (Rapid Firecannon's range, Guardian Angel's revive ...) stay as they are, and the numbers a buff can only
//!   add (penetration, tenacity ...: unsigned fields) are never taken away.
//! - **The tooltips** stay the pack's own: they show an item's numbers before the patch (the patch notes give both).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::SystemTime;

use mod_api_stable::BuffV1;
use serde::Deserialize;
use serde_json::Value;

use crate::diag;

/// The item pack (its catalog) and the equipment mod (the published builds).
pub const ITEMS_MOD: &str = "alele_runes_and_items";
pub const BUILDS_MOD: &str = "alele_equipment_selection";
/// The pack's buff on an item's holder (`item_owned_<mod id>_<key>_<8 hex>`).
pub const OWNED: &str = "item_owned_";
/// This mod's buff that changes an item's numbers on its holder (`<prefix><n>`, n = the item's place in the catalog).
pub const CORRECTION: &str = "patch_meta_ai_item_";
/// The tier of finished items (the pack's `item_t5_...`; components are 1-4).
pub const FINISHED: u32 = 5;

macro_rules! stat_table {
    ($( ($field:ident, $signed:expr, $zh:expr, $en:expr) ),* $(,)?) => {
        /// The numbers an item's buff can carry: (field, signed - a buff can take it away -, Chinese and English label).
        pub const STATS: &[(&str, bool, &str, &str)] = &[ $( (stringify!($field), $signed, $zh, $en) ),* ];

        /// A buff's number `i` of [`STATS`].
        #[allow(unused_assignments)]
        pub fn get(b: &BuffV1, i: usize) -> i64 {
            let mut k = 0usize;
            $(
                if i == k {
                    return b.$field as i64;
                }
                k += 1;
            )*
            0
        }

        /// Sets a buff's number `i` of [`STATS`] (an unsigned one only to a positive value).
        #[allow(unused_assignments)]
        fn set(b: &mut BuffV1, i: usize, v: i64) {
            let mut k = 0usize;
            $(
                if i == k {
                    b.$field = v as _;
                    return;
                }
                k += 1;
            )*
        }
    };
}

stat_table! {
    (attack, true, "攻击力", "Attack"),
    (attack_mult, true, "攻击力加成", "Attack %"),
    (magic_power, true, "法术强度", "Magic power"),
    (magic_power_mult, true, "法术强度加成", "Magic power %"),
    (defence, true, "护甲", "Armor"),
    (defence_mult, true, "护甲加成", "Armor %"),
    (hp, true, "生命值", "Health"),
    (hp_regen, true, "生命回复", "Health regen"),
    (magic_resistance, true, "魔法抗性", "Magic resist"),
    (magic_resistance_mult, true, "魔法抗性加成", "Magic resist %"),
    (vamp, true, "生命偷取", "Life steal"),
    (hp_mult, true, "生命值加成", "Health %"),
    (move_speed_mult, true, "移动速度", "Move speed"),
    (attack_speed_mult, true, "攻击速度", "Attack speed"),
    (skill_cooldown_mult, true, "技能急速", "Skill haste"),
    (ult_cooldown_mult, true, "大招急速", "Ultimate haste"),
    (crit_chance, true, "暴击几率", "Crit chance"),
    (damage_reflect, false, "伤害反弹", "Damage reflect"),
    (damaged_amplify, false, "承受伤害加深", "Damage taken up"),
    (damaged_reduce, false, "伤害减免", "Damage reduction"),
    (defence_penetration, false, "护甲穿透", "Armor penetration"),
    (magic_resistance_penetration, false, "法术穿透", "Magic penetration"),
    (toughness, false, "韧性", "Tenacity"),
    (heal_reduce, false, "重伤", "Grievous wounds"),
    (range, false, "射程", "Range"),
    (base_attack_enemy_max_hp_damage, false, "普攻最大生命伤害", "On-hit max health damage"),
    (self_max_hp_damage, false, "自身最大生命伤害", "Own max health damage"),
    (skill_enemy_max_hp_damage, false, "技能最大生命伤害", "Skill max health damage"),
    (dot_amplify, false, "持续伤害加深", "Damage over time up"),
    (base_attack_damaged_reduce, false, "普攻伤害减免", "Attack damage reduction"),
    (skill_damaged_reduce, false, "技能伤害减免", "Skill damage reduction"),
}

/// The index in [`STATS`] of a field name.
pub fn stat_index(field: &str) -> Option<usize> {
    STATS.iter().position(|(name, ..)| *name == field)
}

/// The correction for an item at `pct` percent on a holder whose item buff is `owned`: `pct` percent of each of its
/// numbers (rounded; an unsigned number only when it goes up). Named `name`; permanent.
pub fn correction(owned: &BuffV1, pct: i32, name: &str) -> BuffV1 {
    let mut b = BuffV1::named(name);
    for (i, (_, signed, ..)) in STATS.iter().enumerate() {
        let v = get(owned, i);
        if v == 0 {
            continue;
        }
        let d = (v as f64 * f64::from(pct) / 100.0).round() as i64;
        if d == 0 || (!signed && d < 0) {
            continue;
        }
        set(&mut b, i, d);
    }
    b
}

/// Whether two buffs carry the same numbers ([`STATS`]).
pub fn same_numbers(a: &BuffV1, b: &BuffV1) -> bool {
    (0..STATS.len()).all(|i| get(a, i) == get(b, i))
}

/// Whether a buff carries no number at all.
pub fn no_numbers(b: &BuffV1) -> bool {
    (0..STATS.len()).all(|i| get(b, i) == 0)
}

// ===================== the pack's catalog =====================

/// One item of the pack.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    pub key: String,
    pub name: String,
    pub tier: u32,
    /// Equipment (else a rune or a part of the pack's own).
    pub equipment: bool,
    /// Its numbers: (index in [`STATS`], value).
    pub stats: Vec<(usize, f64)>,
}

impl Item {
    /// A finished item a build names (tier 5 equipment).
    pub fn finished(&self) -> bool {
        self.equipment && self.tier == FINISHED
    }
}

/// The pack's items, in the catalog's order (an item's place names its correction buff).
#[derive(Debug, Default)]
pub struct Catalog {
    pub mod_id: String,
    pub items: Vec<Item>,
    by_key: HashMap<String, usize>,
    /// Item buff names already looked up (the same name, the same item).
    names: Mutex<HashMap<String, Option<usize>>>,
}

#[derive(Deserialize)]
struct RawCatalog {
    #[serde(rename = "sourceModId", default)]
    source_mod_id: String,
    #[serde(default)]
    items: Vec<RawItem>,
}

#[derive(Deserialize)]
struct RawItem {
    #[serde(rename = "gameKey", default)]
    game_key: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    tier: Option<Value>,
    #[serde(rename = "itemKind", default)]
    item_kind: String,
    #[serde(rename = "packageValues", default)]
    package_values: Option<RawValues>,
}

#[derive(Deserialize, Default)]
struct RawValues {
    #[serde(default)]
    stats: Option<serde_json::Map<String, Value>>,
}

impl Catalog {
    /// The catalog's text read (None: not the pack's catalog).
    pub fn parse(text: &str) -> Option<Catalog> {
        let raw: RawCatalog = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
        let mut cat = Catalog { mod_id: if raw.source_mod_id.is_empty() { ITEMS_MOD.to_string() } else { raw.source_mod_id }, ..Catalog::default() };
        for it in raw.items {
            let key = if it.game_key.is_empty() { it.id } else { it.game_key };
            if key.is_empty() || cat.by_key.contains_key(&key) {
                continue;
            }
            let tier = it.tier.as_ref().and_then(|t| t.as_u64().or_else(|| t.as_str()?.trim_start_matches('t').parse().ok())).unwrap_or(0) as u32;
            let stats = it
                .package_values
                .and_then(|v| v.stats)
                .map(|m| m.iter().filter_map(|(k, v)| Some((stat_index(k)?, v.as_f64()?))).filter(|(_, v)| *v != 0.0).collect())
                .unwrap_or_default();
            cat.by_key.insert(key.clone(), cat.items.len());
            cat.items.push(Item { key, name: it.name, tier, equipment: it.item_kind == "equipment", stats });
        }
        (!cat.items.is_empty()).then_some(cat)
    }

    /// The item of a key, and its place.
    pub fn item(&self, key: &str) -> Option<(usize, &Item)> {
        let i = *self.by_key.get(key)?;
        Some((i, self.items.get(i)?))
    }

    /// Which item an item buff of the pack is (its place in the catalog): the key cut short in its name, and its
    /// numbers when that names several (four items share `item_t5_213456_0_0_1`). None: not an item of this pack, or
    /// not told apart.
    pub fn owned_item(&self, name: &str, buff: &BuffV1) -> Option<usize> {
        if let Some(found) = self.names.lock().unwrap_or_else(PoisonError::into_inner).get(name) {
            return *found;
        }
        let found = self.find_owned(name, buff);
        self.names.lock().unwrap_or_else(PoisonError::into_inner).insert(name.to_string(), found);
        found
    }

    fn find_owned(&self, name: &str, buff: &BuffV1) -> Option<usize> {
        let rest = name.strip_prefix(OWNED)?.strip_prefix(self.mod_id.as_str())?.strip_prefix('_')?;
        // `<key cut short>_<8 hex>`
        let (stem, hash) = rest.rsplit_once('_')?;
        if hash.len() != 8 || !hash.chars().all(|c| c.is_ascii_hexdigit()) || stem.is_empty() {
            return None;
        }
        let candidates: Vec<usize> = self.items.iter().enumerate().filter(|(_, it)| it.key.starts_with(stem)).map(|(i, _)| i).collect();
        match candidates.as_slice() {
            [] => None,
            [one] => Some(*one),
            many => {
                let same: Vec<usize> = many.iter().copied().filter(|i| self.items[*i].matches(buff)).collect();
                (same.len() == 1).then(|| same[0])
            }
        }
    }
}

impl Item {
    /// Whether a buff carries exactly this item's numbers.
    fn matches(&self, buff: &BuffV1) -> bool {
        let want: HashMap<usize, i64> = self.stats.iter().map(|(i, v)| (*i, v.round() as i64)).collect();
        (0..STATS.len()).all(|i| get(buff, i) == want.get(&i).copied().unwrap_or(0))
    }
}

/// The folder of another mod: next to this one, else in the game's own `mods` folder.
fn mod_folder(id: &str) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(ITEMS_DIR_ENV) {
        return Some(PathBuf::from(dir).join(id));
    }
    let next = crate::paths::mod_dir().parent().map(|p| p.join(id));
    let game = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.join("mods").join(id)));
    [next, game].into_iter().flatten().find(|p| p.is_dir())
}

/// Tests point the mod at a folder holding the two packs' folders.
pub const ITEMS_DIR_ENV: &str = "PATCH_META_AI_ITEMS_DIR";

static CATALOG: LazyLock<Option<Arc<Catalog>>> = LazyLock::new(|| {
    let path = mod_folder(ITEMS_MOD)?.join("item-catalog.json");
    let text = std::fs::read_to_string(&path).ok()?;
    let cat = Catalog::parse(&text);
    match &cat {
        Some(c) => diag::log(&format!(
            "[items] {}: {} items, {} finished equipment with numbers",
            path.display(),
            c.items.len(),
            c.items.iter().filter(|i| i.finished() && !i.stats.is_empty()).count()
        )),
        None => diag::log(&format!("[items] {} is not the 4+6 pack's catalog: no item balance", path.display())),
    }
    cat.map(Arc::new)
});

/// The pack's catalog (read once; None without the pack).
pub fn catalog() -> Option<Arc<Catalog>> {
    CATALOG.clone()
}

// ===================== the published builds =====================

/// Every hero's finished items in the published builds (their order kept, each once).
pub type Builds = HashMap<String, Vec<String>>;

#[derive(Deserialize)]
struct RawBuilds {
    #[serde(default)]
    heroes: HashMap<String, RawHero>,
}

#[derive(Deserialize)]
struct RawHero {
    #[serde(default)]
    slots: Vec<RawSlot>,
}

#[derive(Deserialize)]
struct RawSlot {
    #[serde(default)]
    mode: String,
    #[serde(default)]
    item: Option<RawRef>,
}

#[derive(Deserialize)]
struct RawRef {
    #[serde(default)]
    item_key: String,
}

/// The builds in `ai-tail-builds.json`: each hero's specified items that the catalog has as finished equipment.
pub fn parse_builds(bytes: &[u8], catalog: &Catalog) -> Option<Builds> {
    let raw: RawBuilds = serde_json::from_slice(bytes).ok()?;
    if raw.heroes.is_empty() {
        return None;
    }
    let mut out = Builds::new();
    for (hero, h) in raw.heroes {
        let mut keys: Vec<String> = Vec::new();
        for s in h.slots {
            let Some(key) = s.item.map(|r| r.item_key).filter(|k| !k.is_empty()) else { continue };
            if s.mode == "specified" && catalog.item(&key).is_some_and(|(_, it)| it.finished()) && !keys.contains(&key) {
                keys.push(key);
            }
        }
        if !keys.is_empty() {
            out.insert(hero, keys);
        }
    }
    Some(out)
}

/// The builds read last: the file's time and size, and what it held.
struct Read {
    stamp: (SystemTime, u64),
    builds: Arc<Builds>,
}

static BUILDS: Mutex<Option<Read>> = Mutex::new(None);
/// How often [`refresh`] looks at the file.
const REFRESH_EVERY: std::time::Duration = std::time::Duration::from_secs(20);
static READING: Mutex<bool> = Mutex::new(false);

fn builds_path() -> Option<PathBuf> {
    Some(mod_folder(BUILDS_MOD)?.join("ai-tail-builds.json"))
}

fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// The published builds read again in the background when the file changed (the selector publishes some tens of
/// megabytes after every edit and at every game start: never read on a frame). Cheap to call often.
pub fn refresh() {
    {
        static LOOKED: Mutex<Option<std::time::Instant>> = Mutex::new(None);
        let mut looked = LOOKED.lock().unwrap_or_else(PoisonError::into_inner);
        if looked.is_some_and(|t| t.elapsed() < REFRESH_EVERY) {
            return;
        }
        *looked = Some(std::time::Instant::now());
    }
    let (Some(path), Some(cat)) = (builds_path(), catalog()) else { return };
    let Some(now) = stamp(&path) else { return };
    if BUILDS.lock().unwrap_or_else(PoisonError::into_inner).as_ref().is_some_and(|r| r.stamp == now) {
        return;
    }
    {
        let mut reading = READING.lock().unwrap_or_else(PoisonError::into_inner);
        if *reading {
            return;
        }
        *reading = true;
    }
    let spawned = std::thread::Builder::new().name("pma-item-builds".into()).spawn(move || {
        let started = std::time::Instant::now();
        let read = std::fs::read(&path).ok().and_then(|b| parse_builds(&b, &cat));
        match read {
            Some(builds) => {
                diag::log(&format!(
                    "[items] builds read: {} heroes with finished items ({} ms)",
                    builds.len(),
                    started.elapsed().as_millis()
                ));
                *BUILDS.lock().unwrap_or_else(PoisonError::into_inner) = Some(Read { stamp: now, builds: Arc::new(builds) });
            }
            None => diag::log_once("items-builds-unreadable", &format!("[items] {} not readable as the 4+6 builds", path.display())),
        }
        *READING.lock().unwrap_or_else(PoisonError::into_inner) = false;
    });
    if spawned.is_err() {
        *READING.lock().unwrap_or_else(PoisonError::into_inner) = false;
    }
}

/// The builds as last read (None: not read yet, or no pack).
pub fn builds() -> Option<Arc<Builds>> {
    BUILDS.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(|r| Arc::clone(&r.builds))
}

/// What a round needs to change items: each hero's finished items, and those items' names.
#[derive(Clone, Debug, Default)]
pub struct Inputs {
    pub builds: Arc<Builds>,
    pub names: BTreeMap<String, String>,
}

/// The inputs from the builds and the catalog (None: either missing).
pub fn inputs() -> Option<Inputs> {
    let (builds, cat) = (builds()?, catalog()?);
    let used: HashSet<&String> = builds.values().flatten().collect();
    let names = used.into_iter().filter_map(|k| Some((k.clone(), cat.item(k)?.1.name.clone()))).collect();
    Some(Inputs { builds, names })
}

// ===================== in the matches =====================

/// The corrections a holder should carry: for each of its item buffs (`owned`: name and buff) whose item has a
/// power in `power` (percent), [`correction`] named after the item's place. One per item.
pub fn corrections(catalog: &Catalog, owned: &[(String, BuffV1)], power: &BTreeMap<String, i32>) -> Vec<BuffV1> {
    let mut out: Vec<BuffV1> = Vec::new();
    for (name, buff) in owned {
        let Some(i) = catalog.owned_item(name, buff) else { continue };
        let Some(item) = catalog.items.get(i) else { continue };
        let Some(&p) = power.get(&item.key).filter(|p| **p != 0) else { continue };
        let c = correction(buff, p, &format!("{CORRECTION}{i}"));
        if !no_numbers(&c) && !out.iter().any(|o| o.name() == c.name()) {
            out.push(c);
        }
    }
    out
}

// ===================== the notes =====================

/// One of an item's numbers in the notes: (label, before, after).
pub type Line = (String, String, String);

/// An item's numbers before and after a change (`before` -> `after` percent of power), as the notes give them:
/// (label, from, to) for each number that moves - an unsigned one only while it is above the item's own.
pub fn lines(item: &Item, before: i32, after: i32, chinese: bool) -> Vec<Line> {
    let at = |v: f64, p: i32, signed: bool| {
        let d = (v * f64::from(p) / 100.0).round();
        if !signed && d < 0.0 {
            v
        } else {
            v + d
        }
    };
    let mut out = Vec::new();
    for (i, v) in &item.stats {
        let (_, signed, zh, en) = STATS[*i];
        let (from, to) = (at(*v, before, signed), at(*v, after, signed));
        if (from - to).abs() < 0.5 {
            continue;
        }
        out.push(((if chinese { zh } else { en }).to_string(), crate::balance::number(from), crate::balance::number(to)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const CATALOG_TEXT: &str = r#"{"version": 1, "sourceModId": "alele_runes_and_items", "items": [
        {"gameKey": "item_t1_0_0_0_0_0_0_0", "name": "十字镐", "tier": 1, "itemKind": "equipment",
         "packageValues": {"stats": {"attack": 25}}},
        {"gameKey": "item_t5_134_1_2_101_0_0_0", "name": "黑色切割者", "tier": 5, "itemKind": "equipment",
         "packageValues": {"stats": {"attack": 40, "hp": 400, "skill_cooldown_mult": 20}}},
        {"gameKey": "item_t5_213456_0_0_101_0_0_0", "name": "甲", "tier": 5, "itemKind": "equipment",
         "packageValues": {"stats": {"hp": 300}}},
        {"gameKey": "item_t5_213456_0_0_104_0_0_5", "name": "乙", "tier": 5, "itemKind": "equipment",
         "packageValues": {"stats": {"hp": 500, "defence": 40}}},
        {"gameKey": "item_t5_3_0_0_103_0_0_0", "name": "疾射火炮", "tier": 5, "itemKind": "equipment",
         "packageValues": {"stats": {"attack_speed_mult": 35, "crit_chance": 25, "range": 15}}},
        {"gameKey": "runes0_0_0_2", "name": "致命节奏", "tier": 5, "itemKind": "rune", "packageValues": {"stats": {}}}
    ]}"#;

    fn owned(name: &str, f: impl FnOnce(&mut BuffV1)) -> (String, BuffV1) {
        let mut b = BuffV1::named(name);
        f(&mut b);
        (name.to_string(), b)
    }

    #[test]
    fn the_catalog_reads_names_tiers_kinds_and_numbers() {
        let cat = Catalog::parse(CATALOG_TEXT).expect("a catalog");
        assert_eq!(cat.mod_id, ITEMS_MOD);
        assert_eq!(cat.items.len(), 6);
        let (i, bc) = cat.item("item_t5_134_1_2_101_0_0_0").unwrap();
        assert_eq!((i, bc.name.as_str(), bc.tier, bc.finished()), (1, "黑色切割者", 5, true));
        assert_eq!(bc.stats.len(), 3);
        assert!(!cat.item("item_t1_0_0_0_0_0_0_0").unwrap().1.finished(), "a part");
        assert!(!cat.item("runes0_0_0_2").unwrap().1.finished(), "a rune");
        assert!(Catalog::parse("{}").is_none() && Catalog::parse("not json").is_none());
    }

    #[test]
    fn an_item_buff_is_told_by_its_cut_key_and_else_by_its_numbers() {
        let cat = Catalog::parse(CATALOG_TEXT).unwrap();
        let (n, b) = owned("item_owned_alele_runes_and_items_item_t5_134_1_2_101__8c2bbef1", |b| b.attack = 40);
        assert_eq!(cat.owned_item(&n, &b), Some(1));
        // two items under one cut key: the numbers tell them apart
        let (n, b) = owned("item_owned_alele_runes_and_items_item_t5_213456_0_0_1_0a1b2c3d", |b| {
            b.hp = 500;
            b.defence = 40;
        });
        assert_eq!(cat.owned_item(&n, &b), Some(3));
        let (n2, b2) = owned("item_owned_alele_runes_and_items_item_t5_213456_0_0_1_ffffffff", |b| b.hp = 300);
        assert_eq!(cat.owned_item(&n2, &b2), Some(2));
        let (n3, b3) = owned("item_owned_alele_runes_and_items_item_t5_213456_0_0_1_eeeeeeee", |b| b.hp = 1);
        assert_eq!(cat.owned_item(&n3, &b3), None, "numbers that match neither: not guessed");
        // not this pack's, not an item buff
        let (n4, b4) = owned("item_owned_riot_items_tfm2_item_t5_134_1_2_101__8c2bbef1", |_| {});
        assert_eq!(cat.owned_item(&n4, &b4), None);
        let (n5, b5) = owned("item_owned_alele_runes_and_items_item_t5_134_1_2_101__nothex!", |_| {});
        assert_eq!(cat.owned_item(&n5, &b5), None);
    }

    #[test]
    fn a_correction_is_a_share_of_each_number_and_never_takes_an_unsigned_one() {
        let mut item = BuffV1::named("item_owned_x");
        item.attack = 40;
        item.hp = 400;
        item.crit_chance = 25;
        item.range = 15;
        item.toughness = 10;
        let nerf = correction(&item, -10, "patch_meta_ai_item_1");
        assert_eq!(nerf.name(), "patch_meta_ai_item_1");
        assert_eq!((nerf.attack, nerf.hp, nerf.crit_chance), (-4, -40, -3), "-2.5 rounds away from zero");
        assert_eq!((nerf.range, nerf.toughness), (0, 0), "a buff cannot take range or tenacity away");
        let buff = correction(&item, 20, "patch_meta_ai_item_1");
        assert_eq!((buff.attack, buff.hp, buff.crit_chance, buff.range, buff.toughness), (8, 80, 5, 3, 2));
        assert!(same_numbers(&nerf, &correction(&item, -10, "other name")));
        assert!(!same_numbers(&nerf, &buff));
        assert!(no_numbers(&correction(&BuffV1::named("x"), -10, "y")));
    }

    #[test]
    fn the_corrections_are_the_changed_items_on_the_holder_once_each() {
        let cat = Catalog::parse(CATALOG_TEXT).unwrap();
        let power: BTreeMap<String, i32> =
            [("item_t5_134_1_2_101_0_0_0".to_string(), -10), ("item_t5_3_0_0_103_0_0_0".to_string(), 0)].into_iter().collect();
        let held = vec![
            owned("item_owned_alele_runes_and_items_item_t5_134_1_2_101__8c2bbef1", |b| {
                b.attack = 40;
                b.hp = 400;
            }),
            owned("item_owned_alele_runes_and_items_item_t5_134_1_2_101__8c2bbef1", |b| b.attack = 40),
            owned("item_owned_alele_runes_and_items_item_t5_3_0_0_103_0_0__12345678", |b| b.attack_speed_mult = 35),
            owned("some_other_buff", |b| b.attack = 99),
        ];
        let out = corrections(&cat, &held, &power);
        assert_eq!(out.len(), 1, "the changed item once; the unchanged one and other buffs none");
        assert_eq!((out[0].name(), out[0].attack, out[0].hp), ("patch_meta_ai_item_1", -4, -40));
    }

    #[test]
    fn the_builds_keep_each_heros_specified_finished_equipment() {
        let cat = Catalog::parse(CATALOG_TEXT).unwrap();
        let text = r#"{"schema_version": 3, "smart_candidates": [1, 2, 3], "heroes": {
            "league_jinx": {"rune_front": [], "slots": [
                {"slot": 5, "mode": "specified", "item": {"source_mod_id": "alele_runes_and_items", "item_key": "item_t5_3_0_0_103_0_0_0"}},
                {"slot": 6, "mode": "specified", "item": {"item_key": "item_t5_134_1_2_101_0_0_0"}},
                {"slot": 7, "mode": "specified", "item": {"item_key": "item_t5_3_0_0_103_0_0_0"}},
                {"slot": 8, "mode": "specified", "item": {"item_key": "item_t1_0_0_0_0_0_0_0"}},
                {"slot": 9, "mode": "auto"},
                {"slot": 10, "mode": "specified", "item": {"item_key": "not_in_the_catalog"}}]},
            "fighter": {"slots": [{"slot": 5, "mode": "auto"}]}}}"#;
        let b = parse_builds(text.as_bytes(), &cat).expect("builds");
        assert_eq!(b.get("league_jinx").unwrap(), &vec!["item_t5_3_0_0_103_0_0_0".to_string(), "item_t5_134_1_2_101_0_0_0".to_string()]);
        assert!(!b.contains_key("fighter"), "no specified items");
        assert!(parse_builds(b"[]", &cat).is_none());
    }

    #[test]
    fn the_notes_give_each_number_that_moves() {
        let cat = Catalog::parse(CATALOG_TEXT).unwrap();
        let (_, bc) = cat.item("item_t5_134_1_2_101_0_0_0").unwrap();
        let l = lines(bc, 0, -10, true);
        assert!(l.contains(&("攻击力".to_string(), "40".to_string(), "36".to_string())), "{l:?}");
        assert!(l.contains(&("生命值".to_string(), "400".to_string(), "360".to_string())));
        assert!(l.contains(&("技能急速".to_string(), "20".to_string(), "18".to_string())));
        // range (unsigned) never goes down; attack speed and crit do
        let (_, rf) = cat.item("item_t5_3_0_0_103_0_0_0").unwrap();
        let l = lines(rf, -10, -20, false);
        assert!(l.iter().all(|(label, ..)| label != "Range"), "{l:?}");
        assert!(l.contains(&("Attack speed".to_string(), "31".to_string(), "28".to_string())), "{l:?}");
    }
}
