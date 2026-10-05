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
    /// Pseudo-games at a 50% win rate every champion starts from.
    pub baseline_games: f32,
    /// At most this many games of the previous patch carry over into the current one.
    pub carry_games: f32,
    /// Share of that carry-over kept for champions named in the current patch notes.
    pub changed_carry: f32,
    /// Win-rate shift of the starting point for buffed (+) and nerfed (-) champions.
    pub patch_shift: f32,
    /// One solo-rank game counts as this many competition games.
    pub solo_weight: f32,
    /// Games of evidence that give 50% certainty.
    pub reliability_games: f32,
    /// Champion ids reworked in the current patch: their previous patch is ignored.
    pub reworked: Vec<String>,
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
    // [debug]
    pub verbose: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ban_pick: Switch::Auto,
            tier_list: Switch::Auto,
            baseline_games: 20.0,
            carry_games: 40.0,
            changed_carry: 0.5,
            patch_shift: 0.02,
            solo_weight: 0.5,
            reliability_games: 30.0,
            reworked: Vec::new(),
            pick_strength: 1.0,
            ban_strength: 0.8,
            edge_scale: 0.5,
            min_games: 10.0,
            s_percent: 10.0,
            a_percent: 20.0,
            b_percent: 40.0,
            c_percent: 20.0,
            clear_unranked: false,
            verbose: false,
        }
    }
}

impl Config {
    pub fn is_reworked(&self, champion: &str) -> bool {
        self.reworked.iter().any(|c| c == champion)
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
        "baseline_games" => cfg.baseline_games = number(value)?,
        "carry_games" => cfg.carry_games = number(value)?,
        "changed_carry" => cfg.changed_carry = number(value)?,
        "patch_shift" => cfg.patch_shift = number(value)?,
        "solo_weight" => cfg.solo_weight = number(value)?,
        "reliability_games" => cfg.reliability_games = number(value)?,
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
    let v = cfg.baseline_games;
    check("baseline_games", &mut cfg.baseline_games, v >= 1.0, d.baseline_games);
    let v = cfg.carry_games;
    check("carry_games", &mut cfg.carry_games, v >= 0.0, d.carry_games);
    let v = cfg.changed_carry;
    check("changed_carry", &mut cfg.changed_carry, (0.0..=1.0).contains(&v), d.changed_carry);
    let v = cfg.patch_shift;
    check("patch_shift", &mut cfg.patch_shift, (0.0..=0.2).contains(&v), d.patch_shift);
    let v = cfg.solo_weight;
    check("solo_weight", &mut cfg.solo_weight, v >= 0.0, d.solo_weight);
    let v = cfg.reliability_games;
    check("reliability_games", &mut cfg.reliability_games, v > 0.0, d.reliability_games);
    let v = cfg.pick_strength;
    check("pick_strength", &mut cfg.pick_strength, (0.0..=5.0).contains(&v), d.pick_strength);
    let v = cfg.ban_strength;
    check("ban_strength", &mut cfg.ban_strength, (0.0..=5.0).contains(&v), d.ban_strength);
    let v = cfg.edge_scale;
    check("edge_scale", &mut cfg.edge_scale, v > 0.01, d.edge_scale);
    let v = cfg.min_games;
    check("min_games", &mut cfg.min_games, v >= 0.0, d.min_games);
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
        "ban_pick={} tier_list={} baseline_games={} carry_games={} changed_carry={} \
         patch_shift={} solo_weight={} reliability_games={} reworked=[{}] pick_strength={} \
         ban_strength={} edge_scale={} min_games={} s={}% a={}% b={}% c={}% unranked={}",
        c.ban_pick.as_str(),
        c.tier_list.as_str(),
        c.baseline_games,
        c.carry_games,
        c.changed_carry,
        c.patch_shift,
        c.solo_weight,
        c.reliability_games,
        c.reworked.join(","),
        c.pick_strength,
        c.ban_strength,
        c.edge_scale,
        c.min_games,
        c.s_percent,
        c.a_percent,
        c.b_percent,
        c.c_percent,
        if c.clear_unranked { "clear" } else { "keep" }
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
; Every champion's win rate is estimated per in-game patch:
;   start  = baseline_games at 50%, plus up to carry_games of the previous patch at the rate
;            they had, moved by patch_shift when the patch notes buffed or nerfed the champion
;   update = this patch's competition games + solo-rank games * solo_weight
; 每个英雄的胜率按游戏内版本单独估计：
;   起点 = baseline_games 场 50% 的虚拟对局 + 上个版本最多 carry_games 场的实际战绩，
;          补丁公告加强/削弱的英雄再上调/下调 patch_shift
;   更新 = 本版本大会对局 + 单排对局 × solo_weight
baseline_games=20
carry_games=40
; share of the carry-over kept for champions named in the patch notes / 被调整英雄保留的比例
changed_carry=0.5
patch_shift=0.02
solo_weight=0.5
; games of evidence for 50% certainty / 置信度达到一半所需的场次
reliability_games=30
; champion ids reworked this patch (previous patch ignored), comma separated
; 本版本重做的英雄 id，忽略其上个版本数据，逗号分隔，例如 reworked=fighter, demon
reworked=

[draft]
; How hard the estimate pushes the AI's own draft scores.
;   edge  = log-odds of the estimated win rate (0 at 50%)
;   value = tanh(edge / edge_scale) * certainty
;   pick  : + value * pick_strength
;   ban   : + value * ban_strength, and strong champions that are played a lot get up to
;           twice as much (rarely seen ones half) - bans go where they hurt the opponent
; 估计值推动 AI 原有评分的力度。ban 时，强势且出场多的英雄权重最高翻倍，冷门英雄减半。
pick_strength=1.0
ban_strength=0.8
edge_scale=0.5

[tiers]
; Ranked champions (at least min_games of evidence) are sorted by a cautious estimate
; (win rate minus one standard error) and split by share: s% S, a% A, b% B, c% C, the rest D.
; 证据场次达到 min_games 的英雄按保守估计（胜率减一个标准误）排序，按比例分为 S/A/B/C/D。
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

[debug]
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
                    bogus=1\nchanged_carry=2\njunk line\n";
        let (cfg, warnings) = parse(text);
        assert!(cfg.ban_pick == Switch::Off && cfg.tier_list == Switch::Auto);
        assert!((cfg.patch_shift - 0.03).abs() < 1e-6);
        assert_eq!(cfg.reworked, ["fighter", "demon", "ninja"]);
        assert!(cfg.clear_unranked && cfg.is_reworked("demon"));
        assert_eq!(cfg.s_percent, 15.0);
        assert_eq!(cfg.edge_scale, 0.5);
        assert_eq!(cfg.changed_carry, 0.5);
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
