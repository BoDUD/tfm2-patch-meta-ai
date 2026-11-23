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
    /// The player's own team's draft when the game's coach drafts for it (coach delegation)
    /// follows the model, even with `ban_pick` off.
    pub coach: bool,
    /// Every AI team bans and picks outright the way the advice would have it (a hard opponent;
    /// over other draft mods' decisions).
    pub ai_bp: bool,
    /// How many win-rate points a turn an AI team may give up for variety: its own read of the
    /// champions varies from draft to draft (0 = always the model's best).
    pub ai_variety: f32,
    /// Teams drafted by the model (AI teams with `ai_bp`, the player's after an auto BP) also
    /// play the strategy the model rates best for their five champions.
    pub ai_tactics: bool,
    /// 换成模型的战术要比队伍原来的好多少（对数几率，2.13.2；用户：「AI 战术更积极」）：0 = 只要更好就换（默认），
    /// 0.1 = 2.7.8 - 2.13.1 的「明显更好才换」
    pub ai_tactics_margin: f32,
    /// This mod makes the balance patches (`balance`, 2.8.0): a round per game patch (written into the game's own
    /// patch notes), the game's own champion patches off while it is on.
    pub balance: bool,
    /// Champions a round changes at most, and its step at most (percent of power).
    pub balance_count: usize,
    pub balance_max: i32,
    /// Percent of power a round changes a champion by for every win-rate point outside the band, and the move the
    /// model expects of such a step (2.10.0, 1.0-5.0; 1.5 fixed before).
    pub balance_per_point: f32,
    /// Champions with too few games for their win rate a round buffs at most (2.10.0, `balance::UNPICKED_STEP`; 0 =
    /// none, up to 60), besides `balance_count`.
    pub balance_unpicked: usize,
    /// The 4+6 item pack's items in the balance patches too (`item_balance`, 2.9.0), at most `item_balance_count` a
    /// round, each a step of at most `item_balance_max` (percent of the item's numbers).
    pub item_balance: bool,
    pub item_balance_count: usize,
    pub item_balance_max: i32,
    /// The builders an item needs, the band around 50 its builders' win rate is left alone in, and the step (percent of
    /// its numbers) for each point beyond it (2.12.0; the user: 「放宽规则」; 4, 2 and 2 fixed before).
    pub item_balance_users: usize,
    pub item_balance_band: f32,
    pub item_balance_per_point: f32,
    /// Champions and items the balance leaves alone (2.11.0; the user: 「另加一个设置可以手动列出不要调的英雄」): ids /
    /// item keys or the names the game shows, as written (`edits::list`); their power comes off.
    pub balance_skip: Vec<String>,
    pub item_balance_skip: Vec<String>,
    /// 防御塔和大小龙的血量也由平衡补丁调（2.13.0，`crate::objectives`；用户：「现在防御塔 大小龙血量补丁不会自动调整
    /// 我们能加吗」）：对局时长的目标（分钟）、小龙和大龙刷新以后活多久的目标（秒）、每轮最多和累计最多（百分比）
    pub objective_balance: bool,
    pub objective_length: (f32, f32),
    pub objective_drake: (f32, f32),
    pub objective_baron: (f32, f32),
    pub objective_step: i32,
    pub objective_max: i32,
    /// 主题版本（2.14.0，`crate::theme`）：每几个游戏版本出一次（0 = 不出），那一类加强多少（百分比）；补丁日前发预告
    pub balance_theme_every: u32,
    pub balance_theme_step: i32,
    pub patch_preview: bool,
    /// 你的比赛前后的前瞻和复盘新闻（2.14.0，`crate::stories`）
    pub match_stories: bool,
    /// AI 战队的风格（2.14.0，`crate::styles`）：照风格选人每一手最多让出多少胜率点（0 = 不用风格）
    pub ai_style: f32,
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
    /// How much of League's built-in knowledge the priors start from (0 = none, 1 = as built in).
    pub lol_prior: f32,
    /// ... or chosen by the save's own matches (`auto`, the default).
    pub lol_prior_auto: bool,
    /// Champions' strength starts from their stats (`stats`)...
    pub stat_prior: bool,
    /// ... or the save's own matches decide (`auto`, the default).
    pub stat_prior_auto: bool,
    /// The players' abilities in the model (2.11.0, `skills`; on by default - the user: 「你帮我直接启用就行」), or the
    /// save's own matches decide (`auto`).
    pub skills: bool,
    pub skills_auto: bool,
    /// Newer matches count more: a match this many competition matches older counts half
    /// (`None`: all alike)...
    pub recency: Option<f32>,
    /// ... or the save's own matches decide (`auto`, the default).
    pub recency_auto: bool,
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
    /// How much the fielded players' mastery of a candidate counts in the AI's picks and bans
    /// (game 0.6.4+ hands the line-ups over); 0 = not at all.
    pub player_mastery: f32,
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
    /// ... how opaque its panels' background is (percent); a third of it once the pick under a
    /// panel (each pick column's fifth) is in, so it shows through.
    pub panel_opacity: f32,
    /// The Meta Analysis page in the left menu.
    pub meta_page: bool,
    // [report]
    /// Write `meta_report.html` next to `meta_table.txt`.
    pub report: bool,
    // [debug]
    pub verbose: bool,
    /// Write the UI tree of every new screen to `ui_dump_*.txt` (Insert, or F9 off the main screen, always does).
    pub explore: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ban_pick: Switch::Off,
            coach: true,
            ai_bp: false,
            ai_variety: 1.0,
            ai_tactics: false,
            ai_tactics_margin: 0.0,
            balance: false,
            balance_count: 30,
            balance_max: 20,
            balance_per_point: crate::balance::DEFAULT_PER_POINT,
            balance_unpicked: 20,
            item_balance: false,
            item_balance_count: crate::balance::DEFAULT_ITEM_COUNT,
            item_balance_max: crate::balance::DEFAULT_ITEM_MAX,
            item_balance_users: crate::balance::DEFAULT_ITEM_USERS,
            item_balance_band: crate::balance::DEFAULT_ITEM_BAND,
            item_balance_per_point: crate::balance::DEFAULT_ITEM_PER_POINT,
            balance_skip: Vec::new(),
            item_balance_skip: Vec::new(),
            objective_balance: true,
            objective_length: crate::objectives::DEFAULT_TARGETS.length,
            objective_drake: crate::objectives::DEFAULT_TARGETS.drake,
            objective_baron: crate::objectives::DEFAULT_TARGETS.baron,
            objective_step: crate::objectives::DEFAULT_TARGETS.step,
            objective_max: crate::objectives::DEFAULT_TARGETS.max,
            balance_theme_every: 3,
            balance_theme_step: 8,
            patch_preview: true,
            match_stories: true,
            ai_style: 1.0,
            tier_list: Switch::Auto,
            patches: 12.0,
            drift: 0.08,
            change: 0.3,
            patch_shift: 0.02,
            solo_weight: 0.5,
            lol_prior: 1.0,
            lol_prior_auto: true,
            stat_prior: true,
            stat_prior_auto: true,
            skills: true,
            skills_auto: false,
            recency: None,
            recency_auto: true,
            reworked: Vec::new(),
            roles: 0.3,
            players: 0.35,
            mastery: 0.2,
            pairs: 0.15,
            pick_strength: 1.0,
            ban_strength: 0.8,
            edge_scale: 0.5,
            player_mastery: 0.5,
            min_games: 10.0,
            s_percent: 10.0,
            a_percent: 20.0,
            b_percent: 40.0,
            c_percent: 20.0,
            clear_unranked: false,
            draft_overlay: true,
            grid_values: true,
            lane_tags: true,
            panel_opacity: 60.0,
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
            balance_per_point: self.balance_per_point,
            reworked: self.reworked.clone(),
            role_sd: self.roles,
            athlete_sd: self.players,
            mastery_sd: self.mastery,
            pair_sd: self.pairs,
            solo_weight: self.solo_weight,
            lol_prior: self.lol_prior,
            lol_prior_auto: self.lol_prior_auto,
            stat_prior: self.stat_prior,
            stat_prior_auto: self.stat_prior_auto,
            skills: self.skills,
            skills_auto: self.skills_auto,
            recency: self.recency,
            recency_auto: self.recency_auto,
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

/// 一个范围：「19-24」「19–24」「19~24」都行，两头反了就换过来
fn range(v: &str, lowest: f32, highest: f32) -> Result<(f32, f32), String> {
    let parts: Vec<&str> = v.split(['-', '–', '—', '~', '～']).map(str::trim).filter(|p| !p.is_empty()).collect();
    let [a, b] = parts.as_slice() else { return Err(format!("expected two numbers like 19-24, got {v:?}")) };
    let (a, b) = (number(a)?.clamp(lowest, highest), number(b)?.clamp(lowest, highest));
    Ok(if a <= b { (a, b) } else { (b, a) })
}

fn apply(cfg: &mut Config, key: &str, value: &str) -> Result<(), String> {
    match key {
        "ban_pick" => cfg.ban_pick = Switch::parse(value)?,
        "coach" => cfg.coach = flag(value)?,
        // `ai_draft`: its name before 2.7.9
        "ai_bp" | "ai_draft" => cfg.ai_bp = flag(value)?,
        "ai_variety" => cfg.ai_variety = number(value)?,
        "ai_tactics" => cfg.ai_tactics = flag(value)?,
        "ai_tactics_margin" => cfg.ai_tactics_margin = number(value)?.clamp(0.0, 1.0),
        "balance" => cfg.balance = flag(value)?,
        // up to 60 and 50 since 2.9.0 (the user: 「英雄上限调多点吧 最多60个 还有max也调高点」); balance_max up to the
        // champions' CAP, 80 since 2.10.0
        "balance_count" => cfg.balance_count = number(value)?.round().clamp(1.0, 60.0) as usize,
        "balance_max" => cfg.balance_max = number(value)?.round().clamp(1.0, crate::balance::CAP as f32) as i32,
        // 2.10.0 (the user: 「你要所有英雄达到整体平衡啊 不能只找超模吧」; `balance::DEFAULT_PER_POINT`)
        "balance_per_point" => cfg.balance_per_point = number(value)?.clamp(1.0, 5.0),
        "balance_unpicked" => cfg.balance_unpicked = number(value)?.round().clamp(0.0, 60.0) as usize,
        "item_balance" => cfg.item_balance = flag(value)?,
        "item_balance_count" => cfg.item_balance_count = number(value)?.round().clamp(1.0, 30.0) as usize,
        "item_balance_max" => cfg.item_balance_max = number(value)?.round().clamp(1.0, crate::balance::ITEM_CAP as f32) as i32,
        // 2.12.0 (the user: 「装备的削弱补丁有点弱啊」 -> 「放宽规则」)
        "item_balance_users" => cfg.item_balance_users = number(value)?.round().clamp(2.0, 10.0) as usize,
        "item_balance_band" => cfg.item_balance_band = number(value)?.clamp(0.0, 5.0),
        "item_balance_per_point" => cfg.item_balance_per_point = number(value)?.clamp(1.0, 50.0),
        "balance_skip" => cfg.balance_skip = crate::edits::list(value),
        // 2.13.0：塔和龙
        "objective_balance" => cfg.objective_balance = flag(value)?,
        "objective_length" => cfg.objective_length = range(value, 5.0, 90.0)?,
        "objective_drake" => cfg.objective_drake = range(value, 5.0, 900.0)?,
        "objective_baron" => cfg.objective_baron = range(value, 5.0, 900.0)?,
        "objective_step" => cfg.objective_step = number(value)?.round().clamp(1.0, 50.0) as i32,
        "objective_max" => cfg.objective_max = number(value)?.round().clamp(0.0, 300.0) as i32,
        // 2.14.0：主题版本、补丁预告、比赛新闻、AI 风格
        "balance_theme_every" => cfg.balance_theme_every = number(value)?.round().clamp(0.0, 20.0) as u32,
        "balance_theme_step" => cfg.balance_theme_step = number(value)?.round().clamp(1.0, 30.0) as i32,
        "patch_preview" => cfg.patch_preview = flag(value)?,
        "match_stories" => cfg.match_stories = flag(value)?,
        "ai_style" => cfg.ai_style = number(value)?.clamp(0.0, 5.0),
        "item_balance_skip" => cfg.item_balance_skip = crate::edits::list(value),
        "recency" if value.trim().eq_ignore_ascii_case("auto") => cfg.recency_auto = true,
        "recency" if value.trim().eq_ignore_ascii_case("off") => {
            cfg.recency = None;
            cfg.recency_auto = false;
        }
        "recency" => {
            cfg.recency = Some(number(value)?);
            cfg.recency_auto = false;
        }
        "tier_list" => cfg.tier_list = Switch::parse(value)?,
        "verbose" => cfg.verbose = flag(value)?,
        "report" => cfg.report = flag(value)?,
        "draft_overlay" => cfg.draft_overlay = flag(value)?,
        "grid_values" => cfg.grid_values = flag(value)?,
        "lane_tags" => cfg.lane_tags = flag(value)?,
        "panel_opacity" => cfg.panel_opacity = number(value)?,
        "meta_page" => cfg.meta_page = flag(value)?,
        "explore" => cfg.explore = flag(value)?,
        "patches" => cfg.patches = number(value)?.round(),
        "drift" => cfg.drift = number(value)?,
        "change" => cfg.change = number(value)?,
        "patch_shift" => cfg.patch_shift = number(value)?,
        "solo_weight" => cfg.solo_weight = number(value)?,
        "lol_prior" if value.trim().eq_ignore_ascii_case("auto") => cfg.lol_prior_auto = true,
        "lol_prior" => {
            cfg.lol_prior = number(value)?;
            cfg.lol_prior_auto = false;
        }
        "stats" if value.trim().eq_ignore_ascii_case("auto") => {
            cfg.stat_prior = true;
            cfg.stat_prior_auto = true;
        }
        "skills" if value.trim().eq_ignore_ascii_case("auto") => {
            cfg.skills = true;
            cfg.skills_auto = true;
        }
        "skills" => {
            cfg.skills = flag(value)?;
            cfg.skills_auto = false;
        }
        "stats" => {
            cfg.stat_prior = flag(value)?;
            cfg.stat_prior_auto = false;
        }
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
        "player_mastery" => cfg.player_mastery = number(value)?,
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
    // the scanners keep the newest scan::KEPT_PATCHES patches: more cannot be used
    check("patches", &mut cfg.patches, (1.0..=crate::scan::KEPT_PATCHES as f32).contains(&v), d.patches);
    let v = cfg.patch_shift;
    check("patch_shift", &mut cfg.patch_shift, (0.0..=0.2).contains(&v), d.patch_shift);
    let v = cfg.solo_weight;
    check("solo_weight", &mut cfg.solo_weight, v >= 0.0, d.solo_weight);
    let v = cfg.lol_prior;
    check("lol_prior", &mut cfg.lol_prior, (0.0..=3.0).contains(&v), d.lol_prior);
    let v = cfg.pick_strength;
    check("pick_strength", &mut cfg.pick_strength, (0.0..=5.0).contains(&v), d.pick_strength);
    let v = cfg.ban_strength;
    check("ban_strength", &mut cfg.ban_strength, (0.0..=5.0).contains(&v), d.ban_strength);
    let v = cfg.edge_scale;
    check("edge_scale", &mut cfg.edge_scale, v > 0.01, d.edge_scale);
    let v = cfg.ai_variety;
    check("ai_variety", &mut cfg.ai_variety, (0.0..=5.0).contains(&v), d.ai_variety);
    let v = cfg.player_mastery;
    check("player_mastery", &mut cfg.player_mastery, (0.0..=2.0).contains(&v), d.player_mastery);
    let v = cfg.min_games;
    check("min_games", &mut cfg.min_games, v >= 0.0, d.min_games);
    let v = cfg.panel_opacity;
    check("panel_opacity", &mut cfg.panel_opacity, (0.0..=100.0).contains(&v), d.panel_opacity);
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
    if let Some(v) = cfg.recency {
        if v.is_nan() || v < 50.0 {
            warnings.push(format!("recency={v} is out of range (50 or more, off or auto), using auto"));
            cfg.recency = None;
            cfg.recency_auto = true;
        }
    }
}

struct Watch {
    next_check: Option<Instant>,
    stamp: Option<(Option<SystemTime>, u64)>,
}

static CURRENT: RwLock<Option<Arc<Config>>> = RwLock::new(None);
static WATCH: Mutex<Watch> = Mutex::new(Watch { next_check: None, stamp: None });

/// Test support: these settings in force (until the next load).
#[doc(hidden)]
pub fn set_for_tests(cfg: Config) {
    *CURRENT.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(cfg));
}

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
        "ban_pick={} coach={} ai_bp={} ai_variety={} ai_tactics={} ai_tactics_margin={} tier_list={} patches={} drift={} change={} patch_shift={} solo_weight={} \
         lol_prior={} stats={} skills={} recency={} reworked=[{}] roles={} players={} mastery={} pairs={} pick_strength={} \
         ban_strength={} edge_scale={} player_mastery={} min_games={} s={}% a={}% b={}% c={}% unranked={} report={} draft_overlay={} grid_values={} lane_tags={} panel_opacity={} meta_page={} explore={} \
         balance={} balance_count={} balance_max={} balance_per_point={} balance_unpicked={} item_balance={} item_balance_count={} item_balance_max={} \
         item_balance_users={} item_balance_band={} item_balance_per_point={} balance_skip=[{}] item_balance_skip=[{}] \
         objective_balance={} objective_length={}-{} objective_drake={}-{} objective_baron={}-{} objective_step={} objective_max={} \
         balance_theme_every={} balance_theme_step={} patch_preview={} match_stories={} ai_style={}",
        c.ban_pick.as_str(),
        if c.coach { "on" } else { "off" },
        if c.ai_bp { "on" } else { "off" },
        c.ai_variety,
        if c.ai_tactics { "on" } else { "off" },
        c.ai_tactics_margin,
        c.tier_list.as_str(),
        c.patches,
        c.drift,
        c.change,
        c.patch_shift,
        c.solo_weight,
        if c.lol_prior_auto { "auto".to_string() } else { c.lol_prior.to_string() },
        match (c.stat_prior_auto, c.stat_prior) {
            (true, _) => "auto",
            (false, true) => "on",
            (false, false) => "off",
        },
        match (c.skills_auto, c.skills) {
            (true, _) => "auto",
            (false, true) => "on",
            (false, false) => "off",
        },
        match (c.recency_auto, c.recency) {
            (true, _) => "auto".to_string(),
            (false, Some(n)) => n.to_string(),
            (false, None) => "off".to_string(),
        },
        c.reworked.join(","),
        c.roles,
        c.players,
        c.mastery,
        c.pairs,
        c.pick_strength,
        c.ban_strength,
        c.edge_scale,
        c.player_mastery,
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
        c.panel_opacity,
        if c.meta_page { "on" } else { "off" },
        if c.explore { "on" } else { "off" },
        if c.balance { "on" } else { "off" },
        c.balance_count,
        c.balance_max,
        c.balance_per_point,
        c.balance_unpicked,
        if c.item_balance { "on" } else { "off" },
        c.item_balance_count,
        c.item_balance_max,
        c.item_balance_users,
        c.item_balance_band,
        c.item_balance_per_point,
        c.balance_skip.join(","),
        c.item_balance_skip.join(","),
        if c.objective_balance { "on" } else { "off" },
        c.objective_length.0,
        c.objective_length.1,
        c.objective_drake.0,
        c.objective_drake.1,
        c.objective_baron.0,
        c.objective_baron.1,
        c.objective_step,
        c.objective_max,
        c.balance_theme_every,
        c.balance_theme_step,
        if c.patch_preview { "on" } else { "off" },
        if c.match_stories { "on" } else { "off" },
        c.ai_style
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
;        Terminator Draft AI drives the AI's draft); on / off = always / never.
;        ban_pick is off unless you turn it on (2.7.10).
; auto = 开启；若已启用其他做同样事情的 Mod（Drafter's Toolbox 写梯队、Terminator Draft AI
;        接管 AI 选人）则自动让给它。on / off = 总是开 / 总是关。ban_pick 默认关（2.7.10 起）。
ban_pick=off
; coach : when you let the game's coach draft for you (coach delegation), your team bans and
;         picks exactly what the panel advises - it decides last, over other draft mods - even
;         with ban_pick=off. Other teams are not touched by this switch.
; coach : 选人时交给游戏的教练托管时，你自己的队伍完全照面板的建议禁用和选人（最后决定，别的 BP Mod
;         接管时也以它为准），即使 ban_pick=off 也生效；不影响其他队伍。
coach=on
; ai_bp : every AI team bans and picks the way this mod would advise it - the same ranking,
;            3+2 lookahead and players' mastery you see on the panel - decided outright, over
;            other draft mods (Hell Draft AI). A hard opponent; also the AI-vs-AI matches.
; ai_bp : 所有 AI 队伍都完全按本 Mod 的建议禁和选（和你在面板上看到的同一套排序、3+2 推演、
;            选手熟练度），直接决定，盖过其他 BP Mod（地狱人机）。对手更难；AI 之间的比赛也一样。
ai_bp=off
; ai_variety : with ai_bp, each AI team's read of the champions varies from draft to draft
;              (more for champions the model is unsure of), so every champion gets its chances
;              and the AI is hard to predict - but a ban or pick may fall at most this many
;              win-rate points below the model's best. 0 = always the best, 1 = default, up to 5.
; ai_variety : AI 战队的 BP 每局对英雄强度的判断会浮动（模型越拿不准的英雄浮动越大），让所有英雄都有机会
;              上场，AI 也不好猜；但每一手最多比模型认为的最佳选择差这么多胜率点。
;              0 = 永远选最佳，1 = 默认，最大 5。
ai_variety=1
; ai_tactics : AI teams the model drafts (ai_bp on) also play the strategy the model rates best
;              for their five champions (the tactics screen's ★) where it is better than their own
;              by ai_tactics_margin, set when the match starts. Your own team keeps your strategy.
;              Game finish stays each team's coach's choice.
; ai_tactics : 由模型 BP 的 AI 队伍（ai_bp 开着时）在比赛开始时也按模型给这五个英雄选战术（战术界面上
;              ★ 的那些；比原来的好 ai_tactics_margin 以上才换）。你自己的战术不动，自动 BP 也一样。
;              「收官」由各队教练自己选。
ai_tactics=off
; ai_tactics_margin : how much better (log-odds) the model's option must be before an AI team
;              switches to it: 0 = whenever it is better (2.13.2 default), 0.1 = only when clearly
;              better (2.7.8 - 2.13.1), up to 1.
; ai_tactics_margin : 模型的战术要比 AI 队伍原来的好多少（对数几率）才换：0 = 只要更好就换（2.13.2 起默认），
;              0.1 = 明显更好才换（2.7.8～2.13.1），最大 1。
ai_tactics_margin=0
; balance : this mod makes the balance patches instead of the game. On every patch the game
;           makes (and once right away, as a hotfix: 2027.4.1b) the champions the model rates
;           furthest outside a 48-52% win rate (with enough games, at most balance_count, up to
;           60) get balance_per_point % of power stronger or weaker for every point outside (at
;           most balance_max a patch, up to 80; 80% in all; half on attack / magic power, half on
;           health), in every match. balance_count is a ceiling: a champion also needs 30 recent
;           games, a win rate outside 48-52% and room under the limit, so a patch may change fewer.
;           A champion changed waits for 30 games with its new numbers before the next change.
;           2.10.0 makes one hotfix right away on a save whose newest patch was made by an older
;           version, so that its rules (balance_per_point, 80%, balance_unpicked) work at once.
;           The changes are the game's own patch notes (inbox, patch history), with the patch's
;           news cards, and the ban/pick and encyclopedia stat panels show the new numbers.
;           While this is on the game's own champion patches are off (its settings are put back
;           when this is turned off).
; balance : 由本 Mod 出版本平衡补丁，代替游戏自带的。游戏每出一个版本（打开时先出一次热修复，
;           如 2027.4.1b），模型认为胜率离 48%–52% 最远的英雄（场次够的，最多 balance_count 个，
;           最多可填 60）每偏 1 个点调 balance_per_point % 的强度（单版最多 balance_max，最多可填 80；
;           累计最多 80%；一半在攻击/法强，一半在生命），所有比赛生效。改过的英雄要在新数值下打满 30 场
;           才会再改。balance_count 只是上限：英雄还要近期场次够 30、胜率在 48%–52% 之外、
;           没到累计上限才会被改，所以一版改的可能比它少。
;           2.10.0 装上后，存档最新一轮是旧版本做的，读档时马上出一次热修复，按新规则
;           （balance_per_point、80%、balance_unpicked）重新校准，只出这一次。
;           改动就写在游戏自己的补丁公告里（收件箱、百科的补丁历史），
;           也发补丁新闻；BP 和百科的英雄属性显示改动后的数值。开着时游戏自带的英雄补丁关掉
;           （关掉本项时恢复游戏原来的设置）。
balance=off
; 2.10.0 defaults: 30 champions and 20% a patch (10 and 10 before)
; 2.10.0 起的默认值：每版最多 30 个英雄、单版最多 20%（以前 10 和 10）
balance_count=30
balance_max=20
; balance_per_point : percent of power for each win-rate point outside 48-52%, 1-5 (2.10.0;
;           1.5 before). The model expects a step to move its champion by as much. Measured on a
;           save's 4+6 games, 10% of power moved a champion about 2.5 points (1.5 assumed 6.7):
;           2.5 is a cautious middle.
; balance_per_point : 胜率每偏 1 个点调多少强度（百分比，1～5；2.10.0 起，以前固定 1.5），模型也按它
;           预期一刀的效果。按一个存档 4+6 环境下的比赛实测，强度改 10% 胜率约变 2.5 个点（1.5 以为会变
;           6.7 个点），所以取偏保守的 2.5。
balance_per_point=2.5
; balance_unpicked : champions with fewer than 30 recent games are never judged by their win rate.
;           Each patch also buffs up to this many of them by 10% of power (0 = off, up to 60; not
;           counted in balance_count): those this save's careers drafted (picked or banned at least
;           once in the kept matches), at most 50% against the field, the fewest games first, never
;           above +30% through this rule. Once one has 30 games its win rate decides again.
; balance_unpicked : 近期场次不到 30 场的英雄没法按胜率判断。每个版本另外给其中最多这么多个先加强 10%
;           （0 = 关，最多 60；不占 balance_count）：只看这个存档里被选过或被禁过的、和场上比胜率不高于 50%
;           的，场次最少的先加，这条规则最多加到 +30%。打满 30 场后照常按胜率调整。
balance_unpicked=20
; item_balance : with alele's "4+6" item pack (alele_runes_and_items + alele_equipment_selection)
;                the balance patches change its finished items too: an item whose builders (the
;                heroes whose published build names it with 30 recent games, at least
;                item_balance_users of them) win too much or too little against the field gets
;                item_balance_per_point % of its numbers weaker or stronger for every point beyond
;                50% +- item_balance_band (at most item_balance_max a patch, 50% in all; at most
;                item_balance_count items a patch), in every match. The items come first (2.12.0): on
;                their builders' win rates before that patch's champion changes, and the champion
;                changes count what the item changes are expected to do. Numbers only: an item's own
;                effects stay, and penetration / tenacity can go up but never down. The changes come
;                as news of their own; the item tooltips still show the old numbers. Needs balance=on.
; item_balance : 装了 alele 的 4+6 装备包时，平衡补丁也改它的成品装备：出这件装备的英雄（发布的出装里
;                写着它的、近期场次够 30 的，至少 item_balance_users 个）和场上比赢得太多或太少，胜率每超出
;                50% ± item_balance_band 一个点，装备数值调 item_balance_per_point %（单版最多
;                item_balance_max，累计最多 50%；每版最多 item_balance_count 件），所有比赛生效。
;                2.12.0 起装备先判：看本版英雄改动之前的胜率，英雄的改动再扣掉装备改动预计带来的变化。
;                只改数值：装备特效不变，穿透、韧性这类只能加不能减。改动单独发一条新闻；装备说明里
;                显示的仍是原来的数值。要先开 balance。
item_balance=off
; 2.12.0 defaults: 8 items and 20% a patch (5 and 10 before); 3 builders, 50% +- 1, 10% a point
;           (4, 48-52% and 2% fixed before). item_balance_users 2-10, item_balance_band 0-5,
;           item_balance_per_point 1-50.
; 2.12.0 起的默认值：每版最多 8 件、单件单版最多 20%（以前 5 和 10）；至少 3 个出装英雄、50% ± 1、
;           每点 10%（以前固定 4 个、48%–52%、2%）。item_balance_users 2～10，item_balance_band 0～5，
;           item_balance_per_point 1～50。
item_balance_count=8
item_balance_max=20
item_balance_users=3
item_balance_band=1
item_balance_per_point=10
; balance_skip : champions the balance never changes, comma separated - their ids or the names
;                the game shows (e.g. balance_skip=league_jinx, 德莱厄斯). Their earlier changes come
;                off at once (a hotfix says so); they play the numbers the save has.
;                A champion whose numbers you change in the database needs no entry: the balance sees
;                it, takes its own changes off at once and judges it again from the games it plays with
;                your numbers.
; balance_skip : 平衡补丁永远不调整的英雄，逗号分隔，填英雄 id 或游戏里显示的名字（例如
;                balance_skip=league_jinx, 德莱厄斯）。之前对它们的调整马上取消（出一个热修复说明），
;                按存档里的数值来。
;                在数据库里改过数值的英雄不用填：电竞大脑会自己认出来，马上取消它之前的调整，之后按你的
;                数值打出来的比赛重新判断、正常加强削弱。
balance_skip=
; item_balance_skip : the same for the 4+6 items (their keys or names, e.g. 无尽之刃)
; item_balance_skip : 4+6 装备的同样设置（填装备的 key 或名字，例如 无尽之刃）
item_balance_skip=
; objective_balance : the balance patches also set the health of the towers, the drakes and Baron
;                (2.13.0): a percentage on top of the save's numbers in every match, the database keeps
;                its numbers. Towers follow the matches' length: shorter on average than objective_length
;                (minutes) and they get sturdier, longer and they get weaker. Drakes and Baron follow how
;                long one lives after it spawns (seconds, measured in the matches): dying sooner than
;                objective_drake / objective_baron and they get more health, later and less. Each patch
;                makes up half the gap, at most objective_step %, in all between -50% and objective_max %.
;                Judged on the matches played since its last change (30 matches, 30 spawned); the
;                changes come as news of their own and under the patch in the patch history. Health you
;                change in the database takes this mod's percentage off at once. Needs balance=on.
; objective_balance : 平衡补丁也调防御塔、小龙、大龙的生命值（2.13.0）：比赛里在存档数值上按百分比加减，
;                所有比赛生效，数据库里的数值不变。塔看对局时长：平均比 objective_length（分钟）短就加血，
;                长就减血。小龙、大龙看刷新以后活多久（秒，比赛里实测）：比 objective_drake / objective_baron
;                死得快就加血，慢就减血。每个版本补差距的一半，单版最多 objective_step %，累计在 -50% 到
;                objective_max % 之间。只看上次改它以后打的比赛（够 30 场、刷新过 30 条才判断）；改动单独
;                发一条新闻，补丁历史里也写在那个版本下面。你在数据库里改了它们的生命值，电竞大脑的百分比
;                马上去掉。要先开 balance。
objective_balance=on
objective_length=19-24
objective_drake=60-120
objective_baron=120-240
objective_step=15
objective_max=100
; balance_theme_every : every this many game patches (hotfixes do not count) a balance patch is a
;                theme patch: one class of champions (fighters, marksmen, mages, supports, assassins,
;                tanks) is buffed together by balance_theme_step % (the two classes of the last two
;                themes are not picked again); later patches judge them by win rate as usual, so the
;                meta turns now and then. 0 = no themes. Needs balance=on.
; balance_theme_every : 每隔这么多个游戏版本（热修复不算），平衡补丁出一次主题版本：挑一类英雄（战士、射手、法师、
;                辅助、刺客、坦克）一起加强 balance_theme_step %（最近两次主题用过的类别不再挑）；之后的版本照常按胜率
;                调整，版本风向隔一段时间变一次。0 = 不出主题。要先开 balance。
balance_theme_every=3
balance_theme_step=8
; patch_preview : a news item a couple of game days before each game patch with what the balance
;                patch plans to change (the matches until then may still change it).
; patch_preview : 补丁日前两天左右发一条「补丁预告」新闻，写平衡补丁计划改什么（之后的比赛还可能让它变）。
patch_preview=on
; match_stories : a preview news item on the day of each of your matches (form, head to head, the
;                opponent's style and likely bans) and a review after it (score, each game's kills, gold,
;                drakes and Baron, the MVP).
; match_stories : 你的每场比赛当天发一条赛前前瞻（近况、交手记录、对方风格、可能针对你的禁用），打完发一条赛后复盘
;                （比分、每局的击杀经济和小龙大龙、MVP）。
match_stories=on
; ai_style : with ai_bp, every AI team keeps a style of its own from its matches (dive, poke,
;                late game, its favourite classes ...) and picks toward it, giving up at most this many
;                win-rate points a pick for it. 0 = no styles, up to 5. Its style shows in the previews
;                and on the ban/pick screen.
; ai_style : 开着 ai_bp 时，每支 AI 队按自己过去的比赛形成固定的风格（开团、消耗、后期、偏爱哪类英雄……），选人往
;                风格上靠，每一手最多为此让出这么多胜率点。0 = 不用风格，最大 5。对方风格写在赛前前瞻和 BP 界面上。
ai_style=1
tier_list=auto

[model]
; One model is fitted to every match of the last `patches` patches (competition, plus solo
; rank counted as solo_weight of a match). It rates at the same time
;   - each champion's strength in each patch: from one patch to the next it may move about
;     `drift` (untouched) or `change` (buffed / nerfed in the patch notes, moved by patch_shift
;     in that direction; a balance patch of this mod's by its size, a point for each
;     balance_per_point % of power) - so games from before a balance change still count, just less;
;   - each champion's strength in each lane (`roles`);
;   - each player's own strength and their mastery of each champion (`players`, `mastery`):
;     a champion is not rated up just because the best team plays it;
;   - synergy between allies and matchups between opponents (`pairs`).
; Larger values let the data move an effect further from "no effect" (log-odds).
; 用最近 patches 个版本的全部比赛（大会 + 单排×solo_weight）拟合一个模型，同时估计：
;   - 每个英雄在每个版本的强度：相邻版本间未改动的英雄最多变化约 drift，被加强/削弱的
;     最多约 change（并按 patch_shift 朝改动方向预移；本 Mod 自己的平衡补丁按改动幅度预移，每
;     balance_per_point % 强度 1 个胜率点）——改动前的比赛仍然计入，只是权重变小；
;   - 英雄在每个位置的强度（roles）；
;   - 选手本人的实力和对每个英雄的熟练度（players、mastery）：不会因为强队爱用就高估某英雄；
;   - 队友配合与对位克制（pairs）。
; 数值越大，数据越容易把该效应推离"无影响"（对数几率）。
patches=12
drift=0.08
change=0.3
patch_shift=0.02
solo_weight=0.5
; League champions start from what League says (built in): their competitive tier, known combos
; and counters, and what a composition without frontline / engage / both damage types / crowd
; control / damage / protection for its carries loses. The save's matches move all of it.
; auto = choose 0 / 0.5 / 1 / 2 by which predicts the save's newest matches best (once there are
; enough matches; 1 until then); 0 = start every champion at "average", 1 = as built in, up to 3.
; 英雄联盟英雄从英雄联盟的经验出发（内置）：职业赛场梯队、知名的配合和克制、阵容缺前排 / 开团 /
; 双伤害类型 / 控制 / 输出 / 保护C位时的损失。存档里的比赛会修正这些。0 = 一律从"平均"出发，
; 1 = 按内置，最大 3。auto = 用存档里最新的比赛检验 0 / 0.5 / 1 / 2 哪个最准，自动选（比赛够多之前按 1）。
lol_prior=auto
; Champions start from their stats (attack, magic power, health, armour, magic resistance, move
; speed and their growth, compared within their category): the model learns from the matches
; what each stat is worth, so a champion seen in few games is rated by what champions with
; similar stats did. auto = on when that predicts the save's newest matches better; on / off.
; 英雄从自己的属性出发（攻击、法强、生命、护甲、魔抗、移速及其成长，和同类英雄比）：模型从比赛里学
; 每项属性值多少胜率，比赛少的英雄按属性相近的英雄的表现估计。auto = 用最新的比赛检验开 / 关哪个更准；on / off。
stats=auto
; skills : the players' abilities (from each match record: their abilities and lane rating at the
;          match) in the model, so a champion the strongest teams play is not rated up for their
;          players: the tier list and the balance read the champion's own strength. on (default) /
;          off / auto (on when that predicts the save's newest matches better).
; skills : 把选手能力值（每场比赛记录里选手当时的各项能力和位置评分）放进模型，强队常用的英雄不会因为
;          选手强而被高估：梯队和平衡补丁看的是英雄自己的强度。on（默认）/ off / auto（用最新比赛
;          检验，更准才开）。
skills=on
; recency : newer matches count more - a match this many competition matches older counts half.
;           Hero mods change champions without a new game patch; this lets the model follow them.
;           auto = check off / 400 / 1200 against the save's newest matches; off; or a number (50+).
; recency : 新比赛权重更高——早这么多场大会比赛的比赛只算一半。英雄 Mod 改英雄时游戏版本号不变，
;           这样模型能跟上改动。auto = 用最新的比赛检验 关 / 400 / 1200 哪个最准；off；或填数字（50 以上）。
recency=auto
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
; player_mastery : game 0.6.4+ tells the AI which players each team fields. A pick's value then
;   also counts the mastery of the AI team's player in the lane the champion would play (the
;   whole line-up's average when that lane is unknown), a ban's that of the enemy's player, times
;   player_mastery: the AI leans to its players' best champions and bans the enemy players' best.
;   Mastery = how much more a player wins on a champion than the champion and the player do on
;   their own (this save's matches). The game's own proficiency is not added again: its score
;   already counts it. 0 = off; players the model does not know count nothing.
; player_mastery：游戏 0.6.4 起会告诉 AI 双方的上场选手。选人的估值再加上本队将打该位置的选手对这个
;   英雄的熟练度（位置不明时取全队平均），禁用加上敌方选手的，乘以 player_mastery：AI 会偏向自己选手
;   的拿手英雄，并禁掉对方选手的拿手英雄。熟练度 = 选手用这个英雄比英雄和选手本身的水平多赢多少（来自
;   本存档的比赛）。游戏自己的熟练度数值不会再加一遍（游戏的评分已经算过）。0 = 关闭；模型不认识的
;   选手不计。
player_mastery=0.5

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
; panel_opacity : how opaque the two panels' background is, 0-100 (percent); once both teams
;                 the panel sits on a pick column's fifth pick: once that pick is in (left panel:
;                 blue's, right panel: red's) it drops to a third, so the pick shows through
;                 (splash-art mods make the picks bigger)
;                 两个面板背景的不透明度，0～100（百分比）；面板下面是两边第五个选人位，
;                 那一边选满五个（左下角看蓝方、右下角看红方）后降到三分之一，让头像（包括原画 Mod 的大图）透出来
; meta_page     : a "Meta Analysis" page in the left menu (champions, duos, players, model)
;                 左侧菜单里的"版本分析"页面（英雄、组合、选手、模型）
draft_overlay=on
grid_values=on
lane_tags=on
panel_opacity=60
meta_page=on

[report]
; meta_report.html in the mod folder: tiers, lane win rates, synergies, matchups, players
; Mod 文件夹里的 meta_report.html：梯队、分位置胜率、配合、克制、选手熟练度（用浏览器打开）
report=on

[debug]
; explore : write the UI tree of every new screen to ui_dump_*.txt (Insert writes one any time; F9
;           too, but on the main screen the game takes F9 for its facilities)
;           把每个新界面的 UI 结构写入 ui_dump_*.txt（任何时候按 Insert 也会写一份；F9 也行，但主界面
;           的 F9 是游戏打开「设施」的键）
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
    fn ai_bp_and_its_old_name() {
        assert!(parse("ai_bp=on
").0.ai_bp);
        assert!(parse("ai_draft=on
").0.ai_bp, "a settings.ini from before 2.7.9");
        assert!(!parse("ai_draft=on
ai_bp=off
").0.ai_bp, "the later line wins");
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
    fn the_balance_limits_and_their_defaults() {
        let d = Config::default();
        // 2.10.0: 30 champions and 20% a patch by default (10 / 10 before); 2.12.0: 8 items and 20% (5 / 10 before)
        assert_eq!((d.balance_count, d.balance_max, d.item_balance_count, d.item_balance_max), (30, 20, 8, 20));
        assert_eq!((d.item_balance_users, d.item_balance_band, d.item_balance_per_point), (3, 1.0, 10.0));
        let cfg = parse("balance_count=70\nbalance_max=100\nitem_balance_count=40\nitem_balance_max=100\n").0;
        // a champion's step up to its CAP (80), an item's up to ITEM_CAP (50)
        assert_eq!((cfg.balance_count, cfg.balance_max, cfg.item_balance_count, cfg.item_balance_max), (60, 80, 30, 50));
        assert_eq!((crate::balance::CAP, crate::balance::ITEM_CAP), (80, 50));
        let cfg = parse("balance_count=0\nbalance_max=-3\nitem_balance_max=0\n").0;
        assert_eq!((cfg.balance_count, cfg.balance_max, cfg.item_balance_max), (1, 1, 1));
        // a settings.ini that names them keeps its own numbers
        let cfg = parse("balance_count=20\nbalance_max=10\n").0;
        assert_eq!((cfg.balance_count, cfg.balance_max), (20, 10));
    }

    #[test]
    fn the_item_rules_are_read_and_clamped() {
        // 2.12.0 (the user: 「装备的削弱补丁有点弱啊」 -> 「放宽规则」): builders 2-10, band 0-5, 1-50% a point
        let (cfg, warnings) = parse("item_balance_users=1
item_balance_band=9
item_balance_per_point=0.5
");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!((cfg.item_balance_users, cfg.item_balance_band, cfg.item_balance_per_point), (2, 5.0, 1.0));
        // 2.9.0's numbers can be had back
        let cfg = parse("item_balance_users=4
item_balance_band=2
item_balance_per_point=2
item_balance_count=5
item_balance_max=10
").0;
        assert_eq!((cfg.item_balance_users, cfg.item_balance_band, cfg.item_balance_per_point), (4, 2.0, 2.0));
        assert_eq!((cfg.item_balance_count, cfg.item_balance_max), (5, 10));
    }

    #[test]
    fn balance_unpicked_is_read_and_clamped() {
        let unpicked = |text: &str| parse(text).0.balance_unpicked;
        assert_eq!(Config::default().balance_unpicked, 20);
        assert_eq!((unpicked("balance_unpicked=0"), unpicked("balance_unpicked=7.6")), (0, 8), "0 = off");
        assert_eq!((unpicked("balance_unpicked=70"), unpicked("balance_unpicked=-5")), (60, 0), "kept within 0-60");
        assert_eq!(parse("balance_unpicked=many").1.len(), 1);
        assert!(summary(&Config::default()).contains("balance_unpicked=20"));
    }

    #[test]
    fn balance_per_point_is_read_clamped_and_given_to_the_model() {
        let per_point = |text: &str| parse(text).0.balance_per_point;
        assert_eq!(Config::default().balance_per_point, 2.5);
        assert_eq!((per_point("balance_per_point=3"), per_point("balance_per_point=1,5")), (3.0, 1.5));
        assert_eq!((per_point("balance_per_point=0.2"), per_point("balance_per_point=9")), (1.0, 5.0), "kept within 1-5");
        let (cfg, warnings) = parse("balance_per_point=lots");
        assert_eq!((cfg.balance_per_point, warnings.len()), (2.5, 1), "a bad value keeps the default");
        // the model expects a step by the same number
        assert_eq!(parse("balance_per_point=4").0.model().balance_per_point, 4.0);
        assert!(summary(&parse("balance_per_point=4").0).contains("balance_per_point=4"));
    }

    #[test]
    fn the_tower_and_monster_targets_are_ranges() {
        let d = Config::default();
        assert!(d.objective_balance);
        assert_eq!((d.objective_length, d.objective_drake, d.objective_baron), ((19.0, 24.0), (60.0, 120.0), (120.0, 240.0)));
        assert_eq!((d.objective_step, d.objective_max), (15, 100));
        let (cfg, warnings) = parse("objective_length=15–20
objective_drake = 90 ~ 45
objective_baron=100-200
objective_step=80
objective_max=150
objective_balance=off
");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!((cfg.objective_length, cfg.objective_drake, cfg.objective_baron), ((15.0, 20.0), (45.0, 90.0), (100.0, 200.0)));
        assert_eq!((cfg.objective_step, cfg.objective_max, cfg.objective_balance), (50, 150, false));
        let (cfg, warnings) = parse("objective_length=20
");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(cfg.objective_length, (19.0, 24.0), "a bad value keeps the default");
        assert!(summary(&d).contains("objective_balance=on objective_length=19-24 objective_drake=60-120"), "{}", summary(&d));
    }

    #[test]
    fn the_ai_tactics_margin_is_read_and_clamped() {
        assert_eq!(Config::default().ai_tactics_margin, 0.0);
        let (cfg, warnings) = parse("ai_tactics_margin=0.1\n");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(cfg.ai_tactics_margin, 0.1);
        assert_eq!(parse("ai_tactics_margin=5\n").0.ai_tactics_margin, 1.0);
        assert_eq!(parse("ai_tactics_margin=-1\n").0.ai_tactics_margin, 0.0);
        assert!(summary(&cfg).contains("ai_tactics=off ai_tactics_margin=0.1 "), "{}", summary(&cfg));
    }

    #[test]
    fn shares_over_100_percent_fall_back() {
        let (cfg, warnings) = parse("s=50\na=30\nb=30\n");
        assert_eq!((cfg.s_percent, cfg.a_percent, cfg.b_percent), (10.0, 20.0, 40.0));
        assert_eq!(warnings.len(), 1);
    }
}
