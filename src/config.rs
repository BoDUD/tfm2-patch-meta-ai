//! `settings.ini` in the mod folder: written from [`TEMPLATE`] when missing, re-read a few
//! seconds after every save (no restart needed). `[section]` lines only group the keys.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant, SystemTime};

use crate::{diag, paths};

pub const FILE: &str = "settings.ini";
const CHECK_EVERY: Duration = Duration::from_secs(3);

/// A feature switch. `Auto` is on unless another enabled mod already does the job (`compat`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Switch {
    Auto,
    On,
    Off,
}

impl Switch {
    fn parse(v: &str) -> Result<Switch, String> {
        if v.eq_ignore_ascii_case("auto") {
            return Ok(Switch::Auto);
        }
        flag(v).map(|on| if on { Switch::On } else { Switch::Off }).map_err(|_| format!("expected auto/on/off, got {v:?}"))
    }

    fn as_str(self) -> &'static str {
        match self {
            Switch::Auto => "auto",
            Switch::On => "on",
            Switch::Off => "off",
        }
    }

    /// On, given whether another mod already does the job.
    pub fn resolve(self, taken: bool) -> bool {
        match self {
            Switch::Auto => !taken,
            Switch::On => true,
            Switch::Off => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    // [features]
    pub ban_pick: Switch,
    pub tier_list: Switch,
    // [model]
    /// Patches the model looks back over, the current one included.
    pub patches: f32,
    /// How far a champion's strength may move from one patch to the next (log-odds): untouched,
    /// and buffed / nerfed in the patch notes.
    pub drift: f32,
    pub change: f32,
    /// Win-rate step the patch notes' direction suggests for buffed (+) and nerfed (-) champions.
    pub patch_shift: f32,
    /// One solo-rank game counts as this many competition games.
    pub solo_weight: f32,
    /// Champion ids reworked in the current patch: their history barely counts.
    pub reworked: Vec<String>,
    /// Prior spreads (log-odds) of the lane, player, mastery and pair effects: how far the data
    /// must push them away from "no effect".
    pub roles: f32,
    pub players: f32,
    pub mastery: f32,
    pub pairs: f32,
    // [draft]
    pub pick_strength: f32,
    pub ban_strength: f32,
    /// Log-odds edge that gives tanh(1) = 76% of the strength.
    pub edge_scale: f32,
    // [tiers]
    pub min_games: f32,
    /// Shares of the ranked champions, in percent (D gets the rest).
    pub s_percent: f32,
    pub a_percent: f32,
    pub b_percent: f32,
    pub c_percent: f32,
    /// Champions with fewer than `min_games`: `false` = keep their tier (default), `true` = No Tier.
    pub clear_unranked: bool,
    // [screen]
    /// The ban/pick screen overlay: win chance and advice.
    pub draft_overlay: bool,
    /// ... the value of every champion on the grid.
    pub grid_values: bool,
    /// ... the likely lane of each enemy pick.
    pub lane_tags: bool,
    /// The Meta Analysis page in the left menu.
    pub meta_page: bool,
    // [report]
    /// Write `meta_report.html` next to `meta_table.txt`.
    pub report: bool,
    // [debug]
    pub verbose: bool,
    /// Write the UI tree of every new screen to `ui_dump_*.txt` (F9 always does).
    pub explore: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ban_pick: Switch::Auto,
            tier_list: Switch::Auto,
            patches: 12.0,
            drift: 0.08,
            change: 0.3,
            patch_shift: 0.02,
            solo_weight: 0.5,
            reworked: Vec::new(),
            roles: 0.3,
            players: 0.35,
            mastery: 0.2,
            pairs: 0.15,
            pick_strength: 1.0,
            ban_strength: 0.8,
            edge_scale: 0.5,
            min_games: 10.0,
            s_percent: 10.0,
            a_percent: 20.0,
            b_percent: 40.0,
            c_percent: 20.0,
            clear_unranked: false,
            draft_overlay: true,
            grid_values: true,
            lane_tags: true,
            meta_page: true,
            report: true,
            verbose: false,
            explore: false,
        }
    }
}

impl Config {
    pub fn is_reworked(&self, champion: &str) -> bool {
        self.reworked.iter().any(|c| c == champion)
    }

    /// The model's settings.
    pub fn model(&self) -> crate::meta::Settings {
        crate::meta::Settings {
            max_patches: self.patches as usize,
            drift_sd: self.drift,
            change_sd: self.change,
            patch_shift: self.patch_shift,
            reworked: self.reworked.clone(),
            role_sd: self.roles,
            athlete_sd: self.players,
            mastery_sd: self.mastery,
            pair_sd: self.pairs,
            solo_weight: self.solo_weight,
            ..crate::meta::Settings::default()
        }
    }

    /// The AI's bans and picks are nudged (not left to another draft mod).
    pub fn ban_pick_on(&self) -> bool {
        self.ban_pick.resolve(crate::compat::get().draft_driver().is_some())
    }

    /// The tier list is written (not left to another tier mod).
    pub fn tier_list_on(&self) -> bool {
        self.tier_list.resolve(crate::compat::get().tier_writer().is_some())
    }
}

/// Parses the settings text. Unknown keys and bad values are reported, never fatal: the key
/// keeps its default.
pub fn parse(text: &str) -> (Config, Vec<String>) {
    let mut cfg = Config::default();
    let mut warnings = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw
            .split(['#', ';'])
            .next()
            .unwrap_or("")
            .trim()
            .trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            warnings.push(format!("line {}: no '=' in {:?}", index + 1, diag::clip(line, 40)));
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        if let Err(problem) = apply(&mut cfg, &key, value.trim()) {
            warnings.push(format!("line {}: {key}: {problem}", index + 1));
        }
    }
    sanitize(&mut cfg, &mut warnings);
    (cfg, warnings)
}

fn flag(v: &str) -> Result<bool, String> {
    match v.to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Ok(true),
        "off" | "false" | "no" | "0" => Ok(false),
        _ => Err(format!("expected on/off, got {v:?}")),
    }
}

fn number(v: &str) -> Result<f32, String> {
    let text = if v.contains(',') && !v.contains('.') { v.replace(',', ".") } else { v.to_string() };
    let text = text.trim_end_matches('%');
    match text.parse::<f32>() {
        Ok(n) if n.is_finite() => Ok(n),
        _ => Err(format!("expected a number, got {v:?}")),
    }
}

fn apply(cfg: &mut Config, key: &str, value: &str) -> Result<(), String> {
    match key {
        "ban_pick" => cfg.ban_pick = Switch::parse(value)?,
        "tier_list" => cfg.tier_list = Switch::parse(value)?,
        "verbose" => cfg.verbose = flag(value)?,
        "report" => cfg.report = flag(value)?,
        "draft_overlay" => cfg.draft_overlay = flag(value)?,
        "grid_values" => cfg.grid_values = flag(value)?,
        "lane_tags" => cfg.lane_tags = flag(value)?,
        "meta_page" => cfg.meta_page = flag(value)?,
        "explore" => cfg.explore = flag(value)?,
        "patches" => cfg.patches = number(value)?.round(),
        "drift" => cfg.drift = number(value)?,
        "change" => cfg.change = number(value)?,
        "patch_shift" => cfg.patch_shift = number(value)?,
        "solo_weight" => cfg.solo_weight = number(value)?,
        "roles" => cfg.roles = number(value)?,
        "players" => cfg.players = number(value)?,
        "mastery" => cfg.mastery = number(value)?,
        "pairs" => cfg.pairs = number(value)?,
        // 1.x settings of the old model: still accepted, no longer used
        "baseline_games" | "carry_games" | "changed_carry" | "reliability_games" => {
            number(value)?;
        }
        "reworked" => {
            cfg.reworked = value
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        }
        "pick_strength" => cfg.pick_strength = number(value)?,
        "ban_strength" => cfg.ban_strength = number(value)?,
        "edge_scale" => cfg.edge_scale = number(value)?,
        "min_games" => cfg.min_games = number(value)?,
        "s" => cfg.s_percent = number(value)?,
        "a" => cfg.a_percent = number(value)?,
        "b" => cfg.b_percent = number(value)?,
        "c" => cfg.c_percent = number(value)?,
        "unranked" => {
            cfg.clear_unranked = match value.to_ascii_lowercase().as_str() {
                "keep" => false,
                "clear" => true,
                _ => return Err(format!("expected keep or clear, got {value:?}")),
            }
        }
        _ => return Err("unknown key (ignored)".to_string()),
    }
    Ok(())
}

/// Keeps every value where the formulas stay defined.
fn sanitize(cfg: &mut Config, warnings: &mut Vec<String>) {
    let d = Config::default();
    let mut check = |name: &str, value: &mut f32, ok: bool, fallback: f32| {
        if !ok {
            warnings.push(format!("{name}={value} is out of range, using {fallback}"));
            *value = fallback;
        }
    };
    let v = cfg.patches;
    check("patches", &mut cfg.patches, (1.0..=40.0).contains(&v), d.patches);
    let v = cfg.patch_shift;
    check("patch_shift", &mut cfg.patch_shift, (0.0..=0.2).contains(&v), d.patch_shift);
    let v = cfg.solo_weight;
    check("solo_weight", &mut cfg.solo_weight, v >= 0.0, d.solo_weight);
    let v = cfg.pick_strength;
    check("pick_strength", &mut cfg.pick_strength, (0.0..=5.0).contains(&v), d.pick_strength);
    let v = cfg.ban_strength;
    check("ban_strength", &mut cfg.ban_strength, (0.0..=5.0).contains(&v), d.ban_strength);
    let v = cfg.edge_scale;
    check("edge_scale", &mut cfg.edge_scale, v > 0.01, d.edge_scale);
    let v = cfg.min_games;
    check("min_games", &mut cfg.min_games, v >= 0.0, d.min_games);
    for (name, value, fallback) in [
        ("drift", &mut cfg.drift, d.drift),
        ("change", &mut cfg.change, d.change),
        ("roles", &mut cfg.roles, d.roles),
        ("players", &mut cfg.players, d.players),
        ("mastery", &mut cfg.mastery, d.mastery),
        ("pairs", &mut cfg.pairs, d.pairs),
    ] {
        if !(0.001..=3.0).contains(value) {
            warnings.push(format!("{name}={value} is out of range, using {fallback}"));
            *value = fallback;
        }
    }
    for (name, value, fallback) in [
        ("s", &mut cfg.s_percent, d.s_percent),
        ("a", &mut cfg.a_percent, d.a_percent),
        ("b", &mut cfg.b_percent, d.b_percent),
        ("c", &mut cfg.c_percent, d.c_percent),
    ] {
        if !(0.0..=100.0).contains(value) {
            warnings.push(format!("{name}={value}% is out of range, using {fallback}%"));
            *value = fallback;
        }
    }
    let total = cfg.s_percent + cfg.a_percent + cfg.b_percent + cfg.c_percent;
    if total > 100.0 {
        warnings.push(format!("s+a+b+c = {total}% is more than 100%, using the defaults"));
        (cfg.s_percent, cfg.a_percent, cfg.b_percent, cfg.c_percent) =
            (d.s_percent, d.a_percent, d.b_percent, d.c_percent);
    }
}

struct Watch {
    next_check: Option<Instant>,
    stamp: Option<(Option<SystemTime>, u64)>,
}

static CURRENT: RwLock<Option<Arc<Config>>> = RwLock::new(None);
static WATCH: Mutex<Watch> = Mutex::new(Watch { next_check: None, stamp: None });

/// The settings in force (defaults until the file was read).
pub fn get() -> Arc<Config> {
    let guard = CURRENT.read().unwrap_or_else(PoisonError::into_inner);
    match guard.as_ref() {
        Some(cfg) => Arc::clone(cfg),
        None => Arc::new(Config::default()),
    }
}

fn path() -> PathBuf {
    paths::mod_dir().join(FILE)
}

fn stamp(path: &PathBuf) -> Option<(Option<SystemTime>, u64)> {
    fs::metadata(path).ok().map(|m| (m.modified().ok(), m.len()))
}

/// Reads the settings now (writing the template first if the file does not exist).
pub fn load_now() {
    let path = path();
    if !path.exists() {
        match fs::write(&path, TEMPLATE) {
            Ok(()) => diag::log(&format!("wrote default {}", path.display())),
            Err(err) => diag::log_once(
                "settings-write",
                &format!("cannot write {} ({err}); using built-in defaults", path.display()),
            ),
        }
    }
    let mut watch = WATCH.lock().unwrap_or_else(PoisonError::into_inner);
    watch.stamp = stamp(&path);
    watch.next_check = Some(Instant::now() + CHECK_EVERY);
    drop(watch);
    reload(&path);
}

/// Re-reads the settings when the file changed since the last look (checked every 3 s).
pub fn refresh(now: Instant) {
    let path = path();
    let mut watch = WATCH.lock().unwrap_or_else(PoisonError::into_inner);
    if watch.next_check.is_some_and(|next| now < next) {
        return;
    }
    watch.next_check = Some(now + CHECK_EVERY);
    let current = stamp(&path);
    if current == watch.stamp {
        return;
    }
    watch.stamp = current;
    drop(watch);
    reload(&path);
}

fn reload(path: &PathBuf) {
    let (cfg, warnings) = match fs::read(path) {
        Ok(bytes) => parse(&String::from_utf8_lossy(&bytes)),
        Err(_) => (Config::default(), Vec::new()),
    };
    for warning in &warnings {
        diag::log(&format!("{FILE} {warning}"));
    }
    let before = summary(&get());
    let after = summary(&cfg);
    let mut guard = CURRENT.write().unwrap_or_else(PoisonError::into_inner);
    let first = guard.is_none();
    *guard = Some(Arc::new(cfg));
    drop(guard);
    if first {
        diag::log(&format!("settings: {after}"));
    } else if before != after {
        let old: Vec<&str> = before.split(' ').collect();
        let changed: Vec<&str> = after.split(' ').filter(|p| !old.contains(p)).collect();
        diag::log(&format!("{FILE} reloaded: {}", changed.join(" ")));
    }
}

pub fn summary(c: &Config) -> String {
    format!(
        "ban_pick={} tier_list={} patches={} drift={} change={} patch_shift={} solo_weight={} \
         reworked=[{}] roles={} players={} mastery={} pairs={} pick_strength={} \
         ban_strength={} edge_scale={} min_games={} s={}% a={}% b={}% c={}% unranked={} report={} draft_overlay={} grid_values={} lane_tags={} meta_page={} explore={}",
        c.ban_pick.as_str(),
        c.tier_list.as_str(),
        c.patches,
        c.drift,
        c.change,
        c.patch_shift,
        c.solo_weight,
        c.reworked.join(","),
        c.roles,
        c.players,
        c.mastery,
        c.pairs,
        c.pick_strength,
        c.ban_strength,
        c.edge_scale,
        c.min_games,
        c.s_percent,
        c.a_percent,
        c.b_percent,
        c.c_percent,
        if c.clear_unranked { "clear" } else { "keep" },
        if c.report { "on" } else { "off" },
        if c.draft_overlay { "on" } else { "off" },
        if c.grid_values { "on" } else { "off" },
        if c.lane_tags { "on" } else { "off" },
        if c.meta_page { "on" } else { "off" },
        if c.explore { "on" } else { "off" }
    )
}

/// Written to `settings.ini` when the file does not exist.
pub const TEMPLATE: &str = r#"; ============================================================================
; Patch Meta AI - settings / 设置
; Saved changes apply within a few seconds, no restart needed.
; 保存后几秒内生效，不用重启游戏。删除本文件会重新生成默认设置。
; ============================================================================

[features]
; ban_pick  : the AI's bans and picks follow the current patch's real results
;             AI 的 ban/pick 参考当前版本的真实胜负数据
; tier_list : your team's champion tier list is kept up to date automatically
;             自动维护你队伍的英雄梯队
; auto = on, unless another enabled mod already does it (Drafter's Toolbox writes tiers,
;        Terminator Draft AI drives the AI's draft); on / off = always / never
; auto = 默认开启；若已启用其他做同样事情的 Mod（Drafter's Toolbox 写梯队、Terminator Draft AI
;        接管 AI 选人）则自动让给它。on / off = 总是开 / 总是关
ban_pick=auto
tier_list=auto

[model]
; One model is fitted to every match of the last `patches` patches (competition, plus solo
; rank counted as solo_weight of a match). It rates at the same time
;   - each champion's strength in each patch: from one patch to the next it may move about
;     `drift` (untouched) or `change` (buffed / nerfed in the patch notes, moved by patch_shift
;     in that direction) - so games from before a balance change still count, just less;
;   - each champion's strength in each lane (`roles`);
;   - each player's own strength and their mastery of each champion (`players`, `mastery`):
;     a champion is not rated up just because the best team plays it;
;   - synergy between allies and matchups between opponents (`pairs`).
; Larger values let the data move an effect further from "no effect" (log-odds).
; 用最近 patches 个版本的全部比赛（大会 + 单排×solo_weight）拟合一个模型，同时估计：
;   - 每个英雄在每个版本的强度：相邻版本间未改动的英雄最多变化约 drift，被加强/削弱的
;     最多约 change（并按 patch_shift 朝改动方向预移）——改动前的比赛仍然计入，只是权重变小；
;   - 英雄在每个位置的强度（roles）；
;   - 选手本人的实力和对每个英雄的熟练度（players、mastery）：不会因为强队爱用就高估某英雄；
;   - 队友配合与对位克制（pairs）。
; 数值越大，数据越容易把该效应推离"无影响"（对数几率）。
patches=12
drift=0.08
change=0.3
patch_shift=0.02
solo_weight=0.5
; champion ids reworked this patch (history barely counts), comma separated
; 本版本重做的英雄 id（历史数据几乎不计），逗号分隔，例如 reworked=fighter, demon
reworked=
roles=0.3
players=0.35
mastery=0.2
pairs=0.15

[draft]
; How hard the model pushes the AI's own draft scores. For each candidate the model works out
; what it is worth in this draft: its strength in the best lane still open + synergy with the
; picks already made + matchups against the enemy's picks - a cost for one-sided damage
; (bans: its worth to the enemy, more for champions played or banned a lot).
;   pick : + tanh(value / edge_scale) * pick_strength
;   ban  : + tanh(value / edge_scale) * ban_strength
; 模型对 AI 原有评分的影响力度。每个候选英雄按本局局面估值：剩余位置中的最佳强度 + 与已选
; 队友的配合 + 对已选敌人的克制 - 伤害类型单一的惩罚（ban：对敌方的价值，热门英雄更高）。
pick_strength=1.0
ban_strength=0.8
edge_scale=0.5

[tiers]
; Ranked champions (at least min_games recent games: this patch's, plus each earlier patch's
; at half the weight of the one after it) are sorted by a cautious estimate (strength minus
; one standard error) and split by share: s% S, a% A, b% B, c% C, the rest D.
; 近期场次（本版本场次 + 往前每个版本减半计）达到 min_games 的英雄按保守估计（强度减一个
; 标准误）排序，按比例分为 S/A/B/C/D。
; The list is written whenever it changes and shows from the next in-game day.
; 梯队变化时写入，游戏内第二天显示；手动修改会在数据变化时被覆盖。
min_games=10
s=10
a=20
b=40
c=20
; unranked : champions below min_games - keep = leave their tier alone, clear = No Tier
;            证据不足的英雄：keep = 保持原梯队，clear = 设为无梯队
unranked=keep

[screen]
; On the ban/pick screen / 选人界面:
; draft_overlay : win chance (bottom left) and the best picks and bans for you (bottom right)
;                 左下角显示阵容胜率，右下角显示当前最佳选择与禁用
; grid_values   : on every champion card, what picking it is worth to you now (win-rate points)
;                 每个英雄卡片左上角显示此刻选它的价值（胜率百分点）
; lane_tags     : on each enemy pick, its most likely lane / 敌方每个已选英雄最可能的位置
; meta_page     : a "Meta Analysis" page in the left menu (champions, duos, players, model)
;                 左侧菜单里的"版本分析"页面（英雄、组合、选手、模型）
draft_overlay=on
grid_values=on
lane_tags=on
meta_page=on

[report]
; meta_report.html in the mod folder: tiers, lane win rates, synergies, matchups, players
; Mod 文件夹里的 meta_report.html：梯队、分位置胜率、配合、克制、选手熟练度（用浏览器打开）
report=on

[debug]
; explore : write the UI tree of every new screen to ui_dump_*.txt (F9 writes one any time)
;           把每个新界面的 UI 结构写入 ui_dump_*.txt（任何时候按 F9 也会写一份）
explore=off
; more detail in diag.log / diag.log 写更多细节
verbose=off
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses_to_the_defaults() {
        let (cfg, warnings) = parse(TEMPLATE);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn values_lists_and_bad_lines() {
        let text = "\u{feff}[features]\nban_pick = off ; comment\npatch_shift=0,03\n\
                    reworked=fighter, demon  ninja\nunranked=clear\ns=15%\nedge_scale=-1\n\
                    bogus=1\nchange=9\njunk line\ncarry_games=40\n";
        let (cfg, warnings) = parse(text);
        assert!(cfg.ban_pick == Switch::Off && cfg.tier_list == Switch::Auto);
        assert!((cfg.patch_shift - 0.03).abs() < 1e-6);
        assert_eq!(cfg.reworked, ["fighter", "demon", "ninja"]);
        assert!(cfg.clear_unranked && cfg.is_reworked("demon"));
        assert_eq!(cfg.s_percent, 15.0);
        assert_eq!(cfg.edge_scale, 0.5);
        assert_eq!(cfg.change, 0.3);
        assert_eq!(warnings.len(), 4, "{warnings:?}");
    }

    #[test]
    fn auto_steps_aside_for_another_mod() {
        assert!(Switch::Auto.resolve(false) && !Switch::Auto.resolve(true));
        assert!(Switch::On.resolve(true) && !Switch::Off.resolve(false));
        let (cfg, warnings) = parse("tier_list=AUTO
ban_pick=yes
");
        assert!(warnings.is_empty());
        assert_eq!((cfg.tier_list, cfg.ban_pick), (Switch::Auto, Switch::On));
        assert_eq!(parse("tier_list=maybe").1.len(), 1);
    }

    #[test]
    fn shares_over_100_percent_fall_back() {
        let (cfg, warnings) = parse("s=50\na=30\nb=30\n");
        assert_eq!((cfg.s_percent, cfg.a_percent, cfg.b_percent), (10.0, 20.0, 40.0));
        assert_eq!(warnings.len(), 1);
    }
}
