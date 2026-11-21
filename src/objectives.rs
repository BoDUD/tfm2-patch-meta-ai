//! 防御塔和大小龙的血量（2.13.0）。
//!
//! 用户 2026-11-21：「现在防御塔 大小龙血量补丁不会自动调整 我们能加吗」，选了塔看对局时长、大小龙看刷新以后活多久，
//! 对局时长目标 19–24 分钟。游戏自己的规则补丁只在赛季开始出一次，嫌对局太长就砍塔血（那个存档 4800 → 4080 →
//! 3713），小龙大龙只改吃到的增益，龙本身的血从来不动；2028 赛季 5118 场平均才 12.8 分钟。
//!
//! 做法跟英雄平衡一样：每轮补丁给塔、小龙、大龙各一个百分比，比赛里挂一个加生命上限的增益（[`BUFF`]），
//! 存档和数据库里的数值不改。服务端跑出结果的那一份模拟记下这一场打了多久、每条小龙和大龙什么时候刷新、
//! 什么时候被打死（[`Watch`]，存进账本），下一轮补丁拿上次改它以后打的比赛来判断（[`decide`]）：
//! 塔看平均对局时长，小龙大龙看平均活多久（活着的总时间除以被打死的条数，打完还活着的也算时间）。
//! 在目标里面不动，外面就补差距的一半，每轮最多 `objective_step`，累计在 -50% 到 `objective_max` 之间。
//! 玩家在数据库里改了它们的生命值，电竞大脑的百分比马上去掉（和英雄一样，按玩家的数值来，之后照常判断）。

use std::collections::BTreeMap;

use mod_api_stable::{BuffV1, EntityKindV1, StableSim};
use serde::{Deserialize, Serialize};

use crate::balance::Reset;

/// 比赛里挂在塔和龙身上的增益，挂上了就算改过
pub const BUFF: &str = "patch_meta_ai_objective";
/// 每隔这么多 tick 看一次场上的塔和龙
pub const SCAN: usize = 30;
/// 服务端那一份每隔这么多 tick 把记录交给账本一次（死了一条龙、打完了也马上交）
pub const FLUSH: usize = 600;
/// 账本里留最近这么多场
pub const WATCHED: usize = 3000;
/// 血量最多降到 -50%
pub const FLOOR: i32 = -50;
/// 上次改了以后，塔要这么多场、小龙大龙要刷新过这么多条才判断
pub const MIN_MATCHES: u32 = 30;
pub const MIN_SPAWNS: u32 = 30;
/// 每轮只补差距的一半，免得来回晃
pub const DAMPING: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Objective {
    Tower,
    Drake,
    Baron,
}

impl Objective {
    pub const ALL: [Objective; 3] = [Objective::Tower, Objective::Drake, Objective::Baron];

    /// 模拟里的单位是哪一样：塔（双子塔也算），小龙（远古龙也是它），大龙；水晶枢纽不算
    pub fn of_kind(kind: EntityKindV1) -> Option<Objective> {
        match kind {
            EntityKindV1::Tower => Some(Objective::Tower),
            EntityKindV1::Serpen => Some(Objective::Drake),
            EntityKindV1::Epic => Some(Objective::Baron),
            _ => None,
        }
    }

    pub fn index(self) -> usize {
        match self {
            Objective::Tower => 0,
            Objective::Drake => 1,
            Objective::Baron => 2,
        }
    }

    pub fn name(self, chinese: bool) -> &'static str {
        match (self, chinese) {
            (Objective::Tower, true) => "防御塔",
            (Objective::Drake, true) => "小龙",
            (Objective::Baron, true) => "大龙",
            (Objective::Tower, false) => "Towers",
            (Objective::Drake, false) => "Drakes",
            (Objective::Baron, false) => "Baron",
        }
    }

    /// 游戏设置里它的生命值，数据库里改的就是这个
    pub fn setting_path(self) -> &'static str {
        match self {
            Objective::Tower => "tower.stat.hp",
            Objective::Drake => "serpen_jungle.stat.hp",
            Objective::Baron => "epic_jungle.stat.hp",
        }
    }
}

/// 一场比赛里看到的：打的是哪一轮补丁、打了多少 tick、每条小龙和大龙刷新和被打死的 tick（死的 0 = 打完还活着）
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Watch {
    pub r: u32,
    pub e: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub d: Vec<(u32, u32)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub b: Vec<(u32, u32)>,
    /// 这一场没按电竞大脑的血量打（objective_balance 关着）
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub off: bool,
}

impl Watch {
    fn lives(&self, objective: Objective) -> &[(u32, u32)] {
        match objective {
            Objective::Drake => &self.d,
            Objective::Baron => &self.b,
            Objective::Tower => &[],
        }
    }
}

/// 判断用的目标（设置 objective_length、objective_drake、objective_baron、objective_step、objective_max）
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Targets {
    /// 对局时长，分钟
    pub length: (f32, f32),
    /// 小龙、大龙刷新以后活多久，秒
    pub drake: (f32, f32),
    pub baron: (f32, f32),
    pub step: i32,
    pub max: i32,
    /// 游戏一秒几个 tick（游戏设置 tick_per_second）
    pub tps: f32,
    /// 这时游戏设置里塔、小龙、大龙的基础生命（0 = 没读到），记进改动里给新闻用
    pub base: [i64; 3],
}

pub const DEFAULT_TARGETS: Targets =
    Targets { length: (19.0, 24.0), drake: (60.0, 120.0), baron: (120.0, 240.0), step: 15, max: 100, tps: 60.0, base: [0; 3] };

impl Default for Targets {
    fn default() -> Self {
        DEFAULT_TARGETS
    }
}

impl Targets {
    pub fn band(&self, objective: Objective) -> (f32, f32) {
        match objective {
            Objective::Tower => self.length,
            Objective::Drake => self.drake,
            Objective::Baron => self.baron,
        }
    }
}

/// 一轮补丁里一样东西的改动
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ObjectiveChange {
    pub objective: Objective,
    /// 量到的：塔是平均对局时长（分钟），小龙大龙是平均活多久（秒）；量了多少场、多少条
    pub measured: f32,
    pub evidence: u32,
    pub step: i32,
    pub total: i32,
    /// 玩家在数据库里改了它的生命值，电竞大脑的百分比去掉
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset: Option<Reset>,
    /// 当时的目标和游戏设置里的基础生命（0 = 没读到），新闻和补丁历史照这个写
    #[serde(default)]
    pub band: (f32, f32),
    #[serde(default, skip_serializing_if = "is_zero")]
    pub base: i64,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// 量一样东西：(量到的数, 多少场或多少条)；不够判断就是 None
pub fn measure(objective: Objective, watched: &[&Watch], tps: f32) -> Option<(f32, u32)> {
    let tps = if tps > 0.0 { tps } else { DEFAULT_TARGETS.tps };
    if objective == Objective::Tower {
        let n = u32::try_from(watched.len()).unwrap_or(u32::MAX);
        if n < MIN_MATCHES {
            return None;
        }
        let ticks: f64 = watched.iter().map(|w| f64::from(w.e)).sum();
        return Some(((ticks / f64::from(n)) as f32 / tps / 60.0, n));
    }
    let (mut spawns, mut kills, mut ticks) = (0u32, 0u32, 0f64);
    for w in watched {
        for &(spawn, death) in w.lives(objective) {
            spawns += 1;
            let end = if death > 0 {
                kills += 1;
                death
            } else {
                w.e
            };
            ticks += f64::from(end.saturating_sub(spawn));
        }
    }
    if spawns < MIN_SPAWNS {
        return None;
    }
    // 活着的总时间除以被打死的条数：一直没人打的也算进时间，没人打就显得活得久
    Some(((ticks / f64::from(kills.max(1))) as f32 / tps, spawns))
}

/// 量到的数要改多少（百分比）：在目标里面不动；外面就补到最近那一头差距的一半，按生命值和要花的时间成正比算
pub fn wanted(measured: f32, band: (f32, f32), targets: &Targets) -> i32 {
    let edge = if measured < band.0 {
        band.0
    } else if measured > band.1 {
        band.1
    } else {
        return 0;
    };
    if measured <= 0.0 {
        return targets.step;
    }
    let step = ((edge / measured - 1.0) * 100.0 * DAMPING).round() as i32;
    step.clamp(-targets.step, targets.step)
}

/// 这一轮三样各怎么改。每样只看上次改它以后打的比赛（比赛记着打的是哪一轮），objective_balance 关着打的
/// 那些只在它现在是 0 的时候算。第二个是写日志用的一句话：量到了什么、为什么没改。
pub fn decide(rounds: &[crate::balance::Round], watched: &[&Watch], power: &BTreeMap<Objective, i32>, targets: &Targets) -> (Vec<ObjectiveChange>, String) {
    let mut out = Vec::new();
    let mut said = Vec::new();
    for objective in Objective::ALL {
        let since = rounds.iter().rev().find(|r| r.objectives.iter().any(|c| c.objective == objective)).map(|r| r.id);
        let was = power.get(&objective).copied().unwrap_or(0);
        let games: Vec<&Watch> =
            watched.iter().copied().filter(|w| !w.off || was == 0).filter(|w| since.is_none_or(|id| w.r >= id)).collect();
        let key = format!("{objective:?}").to_lowercase();
        let Some((measured, evidence)) = measure(objective, &games, targets.tps) else {
            said.push(format!("{key}: too few since round {} ({} matches)", since.unwrap_or(0), games.len()));
            continue;
        };
        let band = targets.band(objective);
        let want = wanted(measured, band, targets);
        let total = (was + want).clamp(FLOOR, targets.max.max(0));
        let unit = if objective == Objective::Tower { "min" } else { "s" };
        said.push(format!("{key}: {measured:.1} {unit} on {evidence} (target {}-{}), {was:+}% -> {total:+}%", band.0, band.1));
        if total == was {
            continue;
        }
        out.push(ObjectiveChange { objective, measured, evidence, step: total - was, total, reset: None, band, base: targets.base[objective.index()] });
    }
    (out, said.join("; "))
}

// ===================== 比赛里 =====================

/// 场上的一个塔或龙
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Unit {
    pub id: usize,
    pub objective: Objective,
    pub alive: bool,
    /// (当前, 上限)
    pub hp: (usize, usize),
    /// 身上已经有 [`BUFF`]
    pub marked: bool,
}

/// 比赛里要用到的几件事：游戏的模拟，测试里是假的
pub trait Field {
    fn tick(&self) -> usize;
    /// 场上的塔和龙，还有活着的水晶枢纽有几个
    fn units(&self) -> (Vec<Unit>, usize);
    fn add_buff(&mut self, id: usize, power: i32);
    fn hp(&self, id: usize) -> Option<(usize, usize)>;
    fn set_hp(&mut self, id: usize, hp: usize) -> bool;
    /// 增益加不上上限时的退路：基础生命直接乘上去
    fn scale_base_hp(&mut self, id: usize, power: i32) -> bool;
}

/// 改了一个的血，写日志用（每样只写第一次）
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Applied {
    pub objective: Objective,
    pub power: i32,
    pub before: (usize, usize),
    pub after: (usize, usize),
    pub how: &'static str,
}

/// 挂了增益、上限还没变的（引擎可能下一 tick 才算）
#[derive(Clone, Copy, Debug)]
struct Pending {
    id: usize,
    objective: Objective,
    power: i32,
    before: (usize, usize),
    waited: u8,
    base: bool,
}

/// 一份模拟里的记录和改血
#[derive(Default)]
pub struct Tracker {
    /// 活着的小龙大龙：编号、哪一样、刷新的 tick
    alive: Vec<(usize, Objective, u32)>,
    pending: Vec<Pending>,
    /// 开局看到的水晶枢纽
    nexus: usize,
    /// 只有服务端那一份记，记在账本里这一场的编号下
    pub watch: Option<Watch>,
    pub key: String,
    /// 上次交给账本以后有新东西
    pub changed: bool,
    /// 水晶枢纽倒了，打完了
    pub over: bool,
}

impl Tracker {
    pub fn recording(watch: Watch) -> Tracker {
        Tracker { watch: Some(watch), ..Tracker::default() }
    }

    /// 交给账本的样子：还活着的龙记成打完还活着
    pub fn snapshot(&self) -> Option<Watch> {
        let mut w = self.watch.clone()?;
        for &(_, objective, spawn) in &self.alive {
            match objective {
                Objective::Baron => w.b.push((spawn, 0)),
                _ => w.d.push((spawn, 0)),
            }
        }
        Some(w)
    }

    /// 看一次场上：记下龙的刷新和死亡，没改过的塔和龙按 `power` 改血。返回这次改好的（写日志用）。
    pub fn step(&mut self, field: &mut impl Field, power: &BTreeMap<Objective, i32>) -> Vec<Applied> {
        let tick = u32::try_from(field.tick()).unwrap_or(u32::MAX);
        let (units, nexus) = field.units();
        if let Some(w) = self.watch.as_mut() {
            for u in units.iter().filter(|u| u.alive && u.objective != Objective::Tower) {
                if !self.alive.iter().any(|a| a.0 == u.id) {
                    self.alive.push((u.id, u.objective, tick));
                    self.changed = true;
                }
            }
            let mut k = 0;
            while k < self.alive.len() {
                let (id, objective, spawn) = self.alive[k];
                if units.iter().any(|u| u.id == id && u.alive) {
                    k += 1;
                    continue;
                }
                // 死的 tick 记成至少 1，0 留给打完还活着的
                let list = if objective == Objective::Baron { &mut w.b } else { &mut w.d };
                list.push((spawn, tick.max(1)));
                self.alive.remove(k);
                self.changed = true;
            }
            w.e = tick;
        }
        if nexus > self.nexus {
            self.nexus = nexus;
        } else if nexus < self.nexus && !self.over {
            self.over = true;
            self.changed = true;
        }
        let mut out = Vec::new();
        self.settle(field, &mut out);
        for u in &units {
            if !u.alive || u.marked || self.pending.iter().any(|p| p.id == u.id) {
                continue;
            }
            let Some(&p) = power.get(&u.objective).filter(|p| **p != 0) else { continue };
            field.add_buff(u.id, p);
            let pending = Pending { id: u.id, objective: u.objective, power: p, before: u.hp, waited: 0, base: false };
            match field.hp(u.id) {
                Some(now) if now.1 != u.hp.1 => out.push(fill(field, &pending, now, now.0 == u.hp.0)),
                _ => self.pending.push(pending),
            }
        }
        out
    }

    /// 上次挂了增益、上限还没变的：变了就把当前血按比例补上；两次还没变就改基础生命；再不变就算了
    fn settle(&mut self, field: &mut impl Field, out: &mut Vec<Applied>) {
        let mut k = 0;
        while k < self.pending.len() {
            let p = self.pending[k];
            let Some(now) = field.hp(p.id) else {
                self.pending.remove(k);
                continue;
            };
            if now.1 != p.before.1 {
                out.push(fill(field, &p, now, now.0 <= p.before.1));
                self.pending.remove(k);
                continue;
            }
            let waited = p.waited + 1;
            if waited < 2 {
                self.pending[k].waited = waited;
                k += 1;
                continue;
            }
            if p.base {
                out.push(Applied { objective: p.objective, power: p.power, before: p.before, after: now, how: "no effect" });
                self.pending.remove(k);
                continue;
            }
            field.scale_base_hp(p.id, p.power);
            let p = Pending { base: true, waited: 0, ..p };
            match field.hp(p.id) {
                Some(after) if after.1 != p.before.1 => {
                    out.push(fill(field, &p, after, after.0 <= p.before.1));
                    self.pending.remove(k);
                }
                _ => {
                    self.pending[k] = p;
                    k += 1;
                }
            }
        }
    }
}

/// 上限变了以后把当前血按原来的比例补上（`ours`：引擎自己没动当前血）
fn fill(field: &mut impl Field, p: &Pending, now: (usize, usize), ours: bool) -> Applied {
    if ours {
        field.set_hp(p.id, scale(now.0, p.before.1, now.1));
    }
    let after = field.hp(p.id).unwrap_or(now);
    Applied { objective: p.objective, power: p.power, before: p.before, after, how: if p.base { "base stat" } else { "buff" } }
}

fn scale(current: usize, max: usize, new_max: usize) -> usize {
    if max == 0 {
        return new_max;
    }
    let current = current.min(max);
    usize::try_from(current as u128 * new_max as u128 / max as u128).unwrap_or(new_max)
}

/// 增益：只加生命上限
pub fn buff(power: i32) -> BuffV1 {
    BuffV1 { hp_mult: power, ..BuffV1::named(BUFF) }
}

/// 游戏的模拟
pub struct SimField<'s, 'a> {
    pub sim: &'s mut StableSim<'a>,
}

impl Field for SimField<'_, '_> {
    fn tick(&self) -> usize {
        self.sim.tick()
    }

    fn units(&self) -> (Vec<Unit>, usize) {
        let mut units = Vec::new();
        let mut nexus = 0;
        for i in 0..self.sim.entity_count() {
            let Some(e) = self.sim.entity_at(i) else { continue };
            let Some(kind) = e.kind() else { continue };
            if kind == EntityKindV1::Nexus {
                nexus += usize::from(e.is_alive());
                continue;
            }
            let Some(objective) = Objective::of_kind(kind) else { continue };
            let marked = (0..e.buff_count()).filter_map(|k| e.buff_at(k)).any(|b| b.name() == BUFF);
            units.push(Unit { id: e.id(), objective, alive: e.is_alive(), hp: e.hp(), marked });
        }
        (units, nexus)
    }

    fn add_buff(&mut self, id: usize, power: i32) {
        self.sim.add_buff(id, &buff(power));
    }

    fn hp(&self, id: usize) -> Option<(usize, usize)> {
        self.sim.get_entity(id).map(|e| e.hp())
    }

    fn set_hp(&mut self, id: usize, hp: usize) -> bool {
        self.sim.entity_set_hp(id, hp)
    }

    fn scale_base_hp(&mut self, id: usize, power: i32) -> bool {
        let Some(mut stat) = self.sim.get_entity(id).map(|e| e.stat()) else { return false };
        let factor = i64::from(100 + power.max(-99));
        stat.hp = usize::try_from(stat.hp as i64 * factor / 100).unwrap_or(stat.hp);
        self.sim.entity_set_base_stat(id, &stat)
    }
}

// ===================== 新闻和补丁历史 =====================

/// 一样东西改了以后的那句话：改了多少、量到了什么
fn because(c: &ObjectiveChange, chinese: bool) -> String {
    let band = c.band;
    match (c.objective, chinese) {
        (Objective::Tower, true) => format!("最近 {} 场平均 {:.1} 分钟，目标 {}–{} 分钟", c.evidence, c.measured, band.0, band.1),
        (_, true) => format!("最近刷新的 {} 条平均活 {:.0} 秒才被打死，目标 {}–{} 秒", c.evidence, c.measured, band.0, band.1),
        (Objective::Tower, false) => format!("the last {} matches averaged {:.1} min, target {}-{} min", c.evidence, c.measured, band.0, band.1),
        (_, false) => format!("the last {} spawned lived {:.0} s on average before they fell, target {}-{} s", c.evidence, c.measured, band.0, band.1),
    }
}

/// 生命值改前改后（游戏设置里的基础生命，没读到就不写数）
pub fn numbers(c: &ObjectiveChange) -> Option<(i64, i64)> {
    let base = Some(c.base).filter(|b| *b > 0)?;
    let at = |p: i32| ((base as f64) * f64::from(100 + p) / 100.0).round() as i64;
    Some((at(c.total - c.step), at(c.total)))
}

/// 塔、小龙、大龙的改动单独发一条新闻：标题和内容；这一轮没改它们就是 None
pub fn news(label: &str, changes: &[ObjectiveChange], chinese: bool) -> Option<(String, String)> {
    if changes.is_empty() {
        return None;
    }
    let title = if chinese { format!("电竞大脑防御塔和大小龙 {label}") } else { format!("Patch Meta AI towers and epic monsters {label}") };
    let mut out = vec![if chinese {
        "防御塔、小龙、大龙的生命值：比赛里在存档数值上按百分比加减，所有比赛生效，数据库里的数值不变。".to_string()
    } else {
        "Health of the towers, drakes and Baron: a percentage on top of the save's numbers in every match; the database keeps its numbers.".to_string()
    }];
    let mut changes: Vec<&ObjectiveChange> = changes.iter().collect();
    changes.sort_by_key(|c| c.objective);
    for c in changes {
        let name = c.objective.name(chinese);
        let hp = numbers(c).map(|(a, b)| if chinese { format!(" 生命值 {a} → {b}") } else { format!(" health {a} -> {b}") }).unwrap_or_default();
        out.push(match (c.reset.is_some(), chinese) {
            // 没有百分比可取消：只说记下了，之后按玩家的数值重新判断
            (true, true) if c.step == 0 => format!("【记下】{name}{hp}：你在数据库里改了它的生命值，之后按你的数值打出来的比赛重新判断"),
            (true, false) if c.step == 0 => format!("[Noted] {name}{hp}: you changed its health in the database; judged again on the matches with your numbers"),
            (true, true) => format!("【恢复】{name}{hp}：你在数据库里改了它的生命值，取消电竞大脑之前的 {:+}%，按你的数值来", -c.step),
            (true, false) => format!("[Reset] {name}{hp}: you changed its health in the database; this mod's earlier {:+}% taken off", -c.step),
            (false, true) => format!(
                "【{}】{name}{hp}（{:+}%，累计 {:+}%）：{}",
                if c.step > 0 { "加强" } else { "削弱" },
                c.step,
                c.total,
                because(c, true)
            ),
            (false, false) => format!(
                "[{}] {name}{hp} ({:+}%, {:+}% in all): {}",
                if c.step > 0 { "Buff" } else { "Nerf" },
                c.step,
                c.total,
                because(c, false)
            ),
        });
    }
    Some((title, out.join("\n")))
}

/// 补丁历史页里的一行（不带颜色的部分）：名字、生命值改前改后，和量到的东西
pub fn history_parts(c: &ObjectiveChange, chinese: bool) -> (String, String, String) {
    let name = c.objective.name(chinese).to_string();
    let hp = numbers(c).map(|(a, b)| if chinese { format!("生命值 {a} → {b}") } else { format!("health {a} -> {b}") }).unwrap_or_default();
    let why = match (c.reset.is_some(), chinese) {
        (true, true) if c.step == 0 => "你在数据库里改了生命值，之后按你的数值重新判断".to_string(),
        (true, false) if c.step == 0 => "you changed its health in the database: judged again from here".to_string(),
        (true, true) => "你在数据库里改了生命值，取消之前的改动".to_string(),
        (true, false) => "you changed its health in the database: earlier changes taken off".to_string(),
        (false, _) => because(c, chinese),
    };
    (name, hp, why)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(r: u32, minutes: f32, d: &[(u32, u32)], b: &[(u32, u32)]) -> Watch {
        Watch { r, e: (minutes * 3600.0) as u32, d: d.to_vec(), b: b.to_vec(), off: false }
    }

    #[test]
    fn towers_are_judged_on_the_matches_length() {
        let games: Vec<Watch> = (0..40).map(|_| w(5, 12.5, &[], &[])).collect();
        let refs: Vec<&Watch> = games.iter().collect();
        let (m, n) = measure(Objective::Tower, &refs, 60.0).unwrap();
        assert!((m - 12.5).abs() < 0.01 && n == 40, "{m} {n}");
        // 12.5 分钟、目标 19：差 52%，补一半 26%，一轮最多 15
        assert_eq!(wanted(m, (19.0, 24.0), &DEFAULT_TARGETS), 15);
        assert_eq!(wanted(17.0, (19.0, 24.0), &DEFAULT_TARGETS), 6);
        assert_eq!(wanted(21.0, (19.0, 24.0), &DEFAULT_TARGETS), 0, "in the band");
        assert_eq!(wanted(30.0, (19.0, 24.0), &DEFAULT_TARGETS), -10);
        // 不到 30 场不判断
        assert!(measure(Objective::Tower, &refs[..29], 60.0).is_none());
    }

    #[test]
    fn a_monster_lives_its_time_alive_over_its_deaths() {
        // 每场 2 条小龙，一条刷新 20 秒就被打死，一条打完还活着 100 秒
        let games: Vec<Watch> = (0..20).map(|_| w(1, 10.0, &[(7200, 8400), (30000, 0)], &[])).collect();
        let refs: Vec<&Watch> = games.iter().collect();
        let (life, spawns) = measure(Objective::Drake, &refs, 60.0).unwrap();
        // 每场活着的时间 20 + 100 秒，死 1 条
        assert_eq!(spawns, 40);
        assert!((life - 120.0).abs() < 0.1, "{life}");
        // 大龙一条都没刷：不判断
        assert!(measure(Objective::Baron, &refs, 60.0).is_none());
        // 一条都没被打死：活着的时间全算
        let never: Vec<Watch> = (0..30).map(|_| w(1, 12.0, &[], &[(36000, 0)])).collect();
        let refs: Vec<&Watch> = never.iter().collect();
        let (life, _) = measure(Objective::Baron, &refs, 60.0).unwrap();
        assert!((life - 30.0 * 120.0).abs() < 1.0, "{life}");
    }

    fn round(id: u32, objectives: &[(Objective, i32, i32)]) -> crate::balance::Round {
        crate::balance::Round {
            id,
            version: format!("1.{id}"),
            objectives: objectives
                .iter()
                .map(|(o, step, total)| ObjectiveChange { objective: *o, measured: 0.0, evidence: 0, step: *step, total: *total, reset: None, band: (0.0, 0.0), base: 0 })
                .collect(),
            objective_power: objectives.iter().map(|(o, _, total)| (*o, *total)).collect(),
            ..crate::balance::Round::default()
        }
    }

    #[test]
    fn each_is_judged_on_the_matches_since_its_last_change() {
        // 第 2 轮把塔加了 15%：之后（第 2、3 轮）的比赛 23 分钟，之前的 12 分钟不算
        let rounds = vec![round(1, &[]), round(2, &[(Objective::Tower, 15, 15)]), round(3, &[])];
        let mut games: Vec<Watch> = (0..50).map(|_| w(1, 12.0, &[(7200, 7800)], &[(36000, 37200)])).collect();
        games.extend((0..40).map(|_| w(3, 23.0, &[(7200, 7800)], &[(36000, 37200)])));
        let refs: Vec<&Watch> = games.iter().collect();
        let power: BTreeMap<Objective, i32> = [(Objective::Tower, 15)].into_iter().collect();
        let (changes, said) = decide(&rounds, &refs, &power, &DEFAULT_TARGETS);
        // 塔：23 分钟在目标里，不动；小龙活 10 秒（目标 60）+15；大龙活 20 秒（目标 120）+15
        assert!(!changes.iter().any(|c| c.objective == Objective::Tower), "{changes:?}");
        let drake = changes.iter().find(|c| c.objective == Objective::Drake).unwrap();
        assert_eq!((drake.step, drake.total, drake.evidence, drake.band), (15, 15, 90, (60.0, 120.0)));
        assert!((drake.measured - 10.0).abs() < 0.01);
        assert!(changes.iter().any(|c| c.objective == Objective::Baron && c.step == 15));
        assert!(said.contains("tower: 23.0 min on 40"), "{said}");
        // 关着 objective_balance 打的比赛，塔现在有 15% 就不算
        let off: Vec<Watch> = (0..40).map(|_| Watch { off: true, ..w(3, 12.0, &[], &[]) }).collect();
        let refs: Vec<&Watch> = off.iter().collect();
        let (changes, _) = decide(&rounds, &refs, &power, &DEFAULT_TARGETS);
        assert!(!changes.iter().any(|c| c.objective == Objective::Tower), "{changes:?}");
    }

    #[test]
    fn the_totals_stay_between_the_floor_and_the_max() {
        let rounds = vec![round(1, &[(Objective::Tower, 15, 95)])];
        let games: Vec<Watch> = (0..40).map(|_| w(1, 10.0, &[], &[])).collect();
        let refs: Vec<&Watch> = games.iter().collect();
        let power: BTreeMap<Objective, i32> = [(Objective::Tower, 95)].into_iter().collect();
        let (changes, _) = decide(&rounds, &refs, &power, &DEFAULT_TARGETS);
        assert_eq!((changes[0].step, changes[0].total), (5, 100), "held at objective_max");
        let (changes, _) = decide(&rounds, &refs, &[(Objective::Tower, 100)].into_iter().collect(), &DEFAULT_TARGETS);
        assert!(changes.is_empty(), "at the max: nothing");
        let long: Vec<Watch> = (0..40).map(|_| w(1, 60.0, &[], &[])).collect();
        let refs: Vec<&Watch> = long.iter().collect();
        let (changes, _) = decide(&rounds, &refs, &[(Objective::Tower, -45)].into_iter().collect(), &DEFAULT_TARGETS);
        assert_eq!((changes[0].step, changes[0].total), (-5, FLOOR));
    }

    /// 假的比赛：单位和它们的血；`lazy` = 上限下一次看才变，`dead_buff` = 增益加不上上限
    #[derive(Default)]
    struct FakeField {
        tick: usize,
        units: Vec<Unit>,
        nexus: usize,
        base: BTreeMap<usize, usize>,
        buffs: BTreeMap<usize, i32>,
        lazy: bool,
        dead_buff: bool,
        due: Vec<usize>,
        set: Vec<(usize, usize)>,
    }

    impl FakeField {
        fn unit(&mut self, id: usize, objective: Objective, hp: usize) {
            self.base.insert(id, hp);
            self.units.push(Unit { id, objective, alive: true, hp: (hp, hp), marked: false });
        }
        fn max_of(&self, id: usize) -> usize {
            let base = self.base[&id];
            match self.buffs.get(&id) {
                Some(p) if !self.dead_buff && !self.due.contains(&id) => base * (100 + *p as usize) / 100,
                _ => base,
            }
        }
        fn next(&mut self, ticks: usize) {
            self.tick += ticks;
            self.due.clear();
            for k in 0..self.units.len() {
                let id = self.units[k].id;
                let max = self.max_of(id);
                self.units[k].hp = (self.units[k].hp.0.min(max), max);
            }
        }
    }

    impl Field for FakeField {
        fn tick(&self) -> usize {
            self.tick
        }
        fn units(&self) -> (Vec<Unit>, usize) {
            (self.units.clone(), self.nexus)
        }
        fn add_buff(&mut self, id: usize, power: i32) {
            self.buffs.insert(id, power);
            if self.lazy {
                self.due.push(id);
            }
            let max = self.max_of(id);
            if let Some(u) = self.units.iter_mut().find(|u| u.id == id) {
                u.marked = true;
                u.hp.1 = max;
            }
        }
        fn hp(&self, id: usize) -> Option<(usize, usize)> {
            self.units.iter().find(|u| u.id == id).map(|u| u.hp)
        }
        fn set_hp(&mut self, id: usize, hp: usize) -> bool {
            self.set.push((id, hp));
            if let Some(u) = self.units.iter_mut().find(|u| u.id == id) {
                u.hp.0 = hp.min(u.hp.1);
            }
            true
        }
        fn scale_base_hp(&mut self, id: usize, power: i32) -> bool {
            let b = self.base[&id] * (100 + power as usize) / 100;
            self.base.insert(id, b);
            if let Some(u) = self.units.iter_mut().find(|u| u.id == id) {
                u.hp.1 = b;
            }
            true
        }
    }

    #[test]
    fn the_towers_and_monsters_get_their_health_once_at_full() {
        let power: BTreeMap<Objective, i32> = [(Objective::Tower, 15), (Objective::Drake, 10)].into_iter().collect();
        let mut field = FakeField { nexus: 2, ..FakeField::default() };
        field.unit(1, Objective::Tower, 3713);
        field.unit(2, Objective::Baron, 10000);
        let mut t = Tracker::default();
        let applied = t.step(&mut field, &power);
        assert_eq!(applied.len(), 1);
        assert_eq!((applied[0].before, applied[0].after, applied[0].how), ((3713, 3713), (4269, 4269), "buff"));
        assert_eq!(field.hp(2), Some((10000, 10000)), "no power for the Baron");
        // 一条小龙刷出来，已经被打了一点：按比例
        field.unit(3, Objective::Drake, 5000);
        field.units[2].hp.0 = 4000;
        field.next(SCAN);
        let applied = t.step(&mut field, &power);
        assert_eq!(applied.len(), 1);
        assert_eq!(field.hp(3), Some((4400, 5500)));
        // 已经改过的不再动
        field.next(SCAN);
        assert!(t.step(&mut field, &power).is_empty());
        assert_eq!(field.hp(1), Some((4269, 4269)));
    }

    #[test]
    fn a_maximum_that_moves_a_scan_later_or_never_is_handled() {
        let power: BTreeMap<Objective, i32> = [(Objective::Tower, 20)].into_iter().collect();
        // 下一次看才变
        let mut field = FakeField { nexus: 2, lazy: true, ..FakeField::default() };
        field.unit(1, Objective::Tower, 4000);
        let mut t = Tracker::default();
        assert!(t.step(&mut field, &power).is_empty());
        field.next(SCAN);
        let applied = t.step(&mut field, &power);
        assert_eq!((applied.len(), field.hp(1)), (1, Some((4800, 4800))));
        // 增益一直加不上：改基础生命
        let mut field = FakeField { nexus: 2, dead_buff: true, ..FakeField::default() };
        field.unit(1, Objective::Tower, 4000);
        let mut t = Tracker::default();
        for _ in 0..2 {
            assert!(t.step(&mut field, &power).is_empty());
            field.next(SCAN);
        }
        let applied = t.step(&mut field, &power);
        assert_eq!(applied.len(), 1, "{applied:?}");
        assert_eq!((applied[0].how, field.hp(1)), ("base stat", Some((4800, 4800))));
    }

    #[test]
    fn the_server_run_writes_down_each_monsters_life_and_the_end() {
        let mut field = FakeField { nexus: 2, ..FakeField::default() };
        field.unit(1, Objective::Tower, 3713);
        let mut t = Tracker::recording(Watch { r: 7, ..Watch::default() });
        t.step(&mut field, &BTreeMap::new());
        field.tick = 7200;
        field.unit(5, Objective::Drake, 5000);
        t.step(&mut field, &BTreeMap::new());
        field.tick = 8400;
        field.units.retain(|u| u.id != 5);
        t.step(&mut field, &BTreeMap::new());
        field.tick = 36000;
        field.unit(9, Objective::Baron, 10000);
        t.step(&mut field, &BTreeMap::new());
        assert!(t.changed && !t.over);
        field.tick = 40000;
        field.nexus = 1;
        t.step(&mut field, &BTreeMap::new());
        assert!(t.over);
        let w = t.snapshot().unwrap();
        assert_eq!(w, Watch { r: 7, e: 40000, d: vec![(7200, 8400)], b: vec![(36000, 0)], off: false });
        // 别的那几份不记
        let mut other = Tracker::default();
        other.step(&mut field, &BTreeMap::new());
        assert!(other.snapshot().is_none());
    }

    #[test]
    fn the_news_says_each_change_with_its_numbers() {
        let changes = vec![
            ObjectiveChange { objective: Objective::Drake, measured: 24.4, evidence: 1630, step: 15, total: 15, reset: None, band: (60.0, 120.0), base: 5000 },
            ObjectiveChange { objective: Objective::Tower, measured: 12.6, evidence: 412, step: 15, total: 30, reset: None, band: (19.0, 24.0), base: 3713 },
            ObjectiveChange { objective: Objective::Baron, measured: 0.0, evidence: 0, step: -10, total: 0, reset: Some(Reset::Edit), band: (0.0, 0.0), base: 0 },
        ];
        let (title, body) = news("2028.6.0", &changes, true).unwrap();
        assert_eq!(title, "电竞大脑防御塔和大小龙 2028.6.0");
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines[1], "【加强】防御塔 生命值 4270 → 4827（+15%，累计 +30%）：最近 412 场平均 12.6 分钟，目标 19–24 分钟");
        assert_eq!(lines[2], "【加强】小龙 生命值 5000 → 5750（+15%，累计 +15%）：最近刷新的 1630 条平均活 24 秒才被打死，目标 60–120 秒");
        assert_eq!(lines[3], "【恢复】大龙：你在数据库里改了它的生命值，取消电竞大脑之前的 +10%，按你的数值来");
        let (title, body) = news("2028.6.0", &changes, false).unwrap();
        assert!(title.starts_with("Patch Meta AI towers") && body.contains("[Buff] Towers health 4270 -> 4827 (+15%, +30% in all)"), "{body}");
        assert!(news("x", &[], true).is_none());
        // 没有百分比可取消的：只说记下了
        let noted = [ObjectiveChange { step: 0, ..changes[2].clone() }];
        assert!(news("x", &noted, true).unwrap().1.contains("【记下】大龙"));
    }

    #[test]
    fn a_watch_reads_back_and_old_books_have_none() {
        let w = Watch { r: 3, e: 43000, d: vec![(7200, 7500)], b: vec![], off: false };
        let text = serde_json::to_string(&w).unwrap();
        assert_eq!(text, r#"{"r":3,"e":43000,"d":[[7200,7500]]}"#);
        assert_eq!(serde_json::from_str::<Watch>(&text).unwrap(), w);
        let c = ObjectiveChange { objective: Objective::Baron, measured: 1.0, evidence: 2, step: 3, total: 3, reset: None, band: (120.0, 240.0), base: 0 };
        let text = serde_json::to_string(&c).unwrap();
        assert!(text.contains(r#""objective":"baron""#) && !text.contains("base"), "{text}");
        assert_eq!(serde_json::from_str::<ObjectiveChange>(&text).unwrap(), c);
    }
}
