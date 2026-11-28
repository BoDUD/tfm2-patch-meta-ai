//! 比赛中的实时胜率（2.15.0）。用户从「电竞大脑还能做什么牛逼的功能」里挑了「实时胜率曲线」。
//!
//! - **记录**：服务端预演每一场正式比赛（十名选手）时，每 [`ROW_SECONDS`] 秒记一行局面（蓝方减红方：经济、人头、
//!   还剩几座塔、平均等级、小龙、大龙增益、活着几个人），加上 BP 完的阵容胜率（电竞大脑的模型），比赛结束看哪边
//!   水晶还在就知道谁赢，整场一行写进 Mod 文件夹的 [`FILE`]（只留最近 [`KEPT`] 场）。
//! - **模型**：逻辑回归，系数随比赛时间变：在 [`KNOTS`] 这几个分钟点上各一组，中间线性插值；往一组先验
//!   （[`PRIOR`]）靠，记录的比赛少时就是先验，够 [`MIN_MATCHES`] 场就按记录拟合（后台线程，[`maybe_fit`]）。
//! - **显示**：客户端看比赛（观看、观战、回放）的那一份模拟每 [`LIVE_SECONDS`] 秒同样看一次局面，算出蓝方胜率交给
//!   界面（`ui::live`），画在比赛时间下面；十秒里起落十个点以上，就写一句原因（[`Live::note`]）。
//!
//! 小龙、大龙是哪队拿的游戏不告诉 Mod：看它死的那一刻离它最近的三个英雄哪队多（[`credit`]）。大龙增益按拿到以后
//! [`BARON_SECONDS`] 秒算。只读场上，不改比赛。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError, RwLock};
use std::time::Instant;

use mod_api_stable::{EntityKindV1, SimOriginKindV1, StableSim};

use crate::diag;
use crate::glm::sigmoid;

/// 记录的文件（Mod 文件夹里，所有存档共用：局面和胜负的关系是游戏本身的事）
pub const FILE: &str = "winprob.jsonl";
/// 文件里留最近这么多场
pub const KEPT: usize = 4000;
/// 够这么多场才按记录拟合，不然用先验
pub const MIN_MATCHES: usize = 40;
/// 每隔这么多 tick 看一次场上（小龙大龙什么时候死的、谁拿的）
const SCAN: usize = 30;
/// 每隔这么多次才全场找一遍小龙、大龙和水晶（平时只看认得的那几个，后台预演很多，省着点）
const FULL_EVERY: u32 = 20;
/// 记录：每隔这么多秒一行
pub const ROW_SECONDS: f32 = 30.0;
/// 显示：每隔这么多秒算一次胜率
pub const LIVE_SECONDS: f32 = 1.0;
/// 大龙增益算多少秒
pub const BARON_SECONDS: f32 = 150.0;
/// 拟合完以后文件又长了这么多字节，就再拟合一次（一场一千字节上下）
const REFIT_BYTES: u64 = 60_000;
/// 同时跟着的模拟（一个比赛日的预演可能一起跑）
const RUNS_KEPT: usize = 64;

/// 特征：BP 完的阵容胜率（对数几率）、蓝方本身、经济差（千）、人头差、塔差、平均等级差、小龙差、大龙增益、活着的人数差
pub const FEATURES: usize = 9;
pub const NAMES: [&str; FEATURES] = ["draft", "blue side", "gold (k)", "kills", "towers", "level", "drakes", "Baron", "alive"];
/// 系数随时间变的几个点（分钟）
pub const KNOTS: [f32; 6] = [0.0, 5.0, 10.0, 15.0, 20.0, 30.0];
const K: usize = KNOTS.len();

/// 先验：每一项在每个时间点的系数（对数几率）和它能偏多远（标准差）。比赛少的时候就用它
pub const PRIOR: [[f32; K]; FEATURES] = [
    [1.0, 1.0, 0.9, 0.8, 0.7, 0.6],
    [0.0; K],
    [0.0, 0.25, 0.25, 0.22, 0.2, 0.15],
    [0.03; K],
    [0.15; K],
    [0.0, 0.2, 0.2, 0.2, 0.2, 0.2],
    [0.1; K],
    [0.6; K],
    [0.1, 0.15, 0.2, 0.25, 0.3, 0.35],
];
const PRIOR_SD: [f32; FEATURES] = [0.5, 0.3, 0.2, 0.05, 0.15, 0.2, 0.15, 0.4, 0.2];

// ===================================================================== 局面

/// 一名选手此刻的样子
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Player {
    /// 0 蓝方，1 红方
    pub team: usize,
    pub champion: String,
    pub alive: bool,
    pub pos: (u64, u64),
    pub kills: u32,
    pub level: u32,
    /// 这一场到现在的经济（读不到统计就是身上的钱）
    pub gold: f32,
}

/// 场上的大目标是什么
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum What {
    Drake,
    Baron,
    /// 哪一队的水晶
    Nexus(usize),
}

/// 一条小龙、大龙，或者一边的水晶
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Objective {
    pub id: usize,
    pub what: What,
    pub alive: bool,
    pub pos: (u64, u64),
}

/// 一份模拟此刻的场上（游戏里是 [`SimPitch`]，测试里用假的）
pub trait Pitch {
    fn tick(&self) -> usize;
    fn players(&self) -> Vec<Player>;
    /// 每座塔：队、还在不在
    fn towers(&self) -> Vec<(usize, bool)>;
    /// 全场找一遍小龙、大龙和水晶
    fn objectives(&self) -> Vec<Objective>;
    /// 再看认得的一个（不在场上了就是 None）
    fn objective(&self, id: usize, what: What) -> Option<Objective>;
}

/// 一个时刻的局面，都是蓝方减红方
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Situation {
    pub minutes: f32,
    pub gold_k: f32,
    pub kills: f32,
    pub towers: f32,
    pub level: f32,
    pub drakes: f32,
    pub baron: f32,
    pub alive: f32,
}

impl Situation {
    /// 模型的输入：阵容胜率（对数几率）在前，蓝方本身是常数 1
    pub fn features(&self, draft: f32) -> [f32; FEATURES] {
        [draft, 1.0, self.gold_k, self.kills, self.towers, self.level, self.drakes, self.baron, self.alive]
    }

    fn row(&self) -> [f32; 8] {
        [self.minutes, self.gold_k, self.kills, self.towers, self.level, self.drakes, self.baron, self.alive]
    }

    fn from_row(r: &[f32]) -> Option<Situation> {
        let [minutes, gold_k, kills, towers, level, drakes, baron, alive] = <[f32; 8]>::try_from(r).ok()?;
        Some(Situation { minutes, gold_k, kills, towers, level, drakes, baron, alive })
    }
}

/// 小龙大龙死的时候谁拿的：离它最近的（最多）三个活着的英雄里哪队多，只剩两个、一边一个就看最近的那个
pub fn credit(pos: (u64, u64), players: &[Player]) -> Option<usize> {
    let d2 = |p: &Player| {
        let dx = p.pos.0.abs_diff(pos.0) as f64;
        let dy = p.pos.1.abs_diff(pos.1) as f64;
        dx * dx + dy * dy
    };
    let mut near: Vec<&Player> = players.iter().filter(|p| p.alive && p.team < 2).collect();
    near.sort_by(|a, b| d2(a).total_cmp(&d2(b)));
    near.truncate(3);
    let first = near.first()?.team;
    let blue = near.iter().filter(|p| p.team == 0).count();
    let red = near.len() - blue;
    Some(match blue.cmp(&red) {
        std::cmp::Ordering::Greater => 0,
        std::cmp::Ordering::Less => 1,
        std::cmp::Ordering::Equal => first,
    })
}

/// 一份模拟一路看下来记着的东西
#[derive(Clone, Debug, Default)]
pub struct Tracker {
    /// BP 完的阵容胜率（对数几率），十个英雄都在场上时算一次
    pub draft: Option<f32>,
    pub drakes: [u32; 2],
    /// 大龙增益到哪个 tick
    pub baron_until: [usize; 2],
    /// 认得的小龙、大龙、水晶
    known: HashMap<usize, Objective>,
    scans: u32,
    /// 记录的行（服务端预演）
    pub rows: Vec<Situation>,
    next_row: usize,
    next_live: usize,
    /// 已经分出胜负（记过了）
    pub done: bool,
}

/// 看一次场上的结果
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Step {
    pub situation: Situation,
    /// 到了该算胜率的时候：蓝方胜率
    pub chance: Option<f32>,
    /// 一边的水晶没了：赢的那边
    pub winner: Option<usize>,
}

impl Tracker {
    /// 看一次场上：小龙大龙谁拿了、局面，该记的记、该算的算（`draft_of`：十个英雄的阵容胜率怎么算）
    pub fn step(
        &mut self,
        pitch: &impl Pitch,
        tps: f32,
        record: bool,
        show: bool,
        model: &Model,
        draft_of: &dyn Fn(&[Player]) -> f32,
    ) -> Step {
        let tick = pitch.tick();
        // 小龙、大龙、水晶：平时只看认得的，隔一阵全场找一遍（新刷出来的）
        let full = self.known.is_empty() || self.scans.is_multiple_of(FULL_EVERY);
        self.scans += 1;
        let mut now: Vec<Objective> = if full {
            pitch.objectives()
        } else {
            self.known.values().filter_map(|o| pitch.objective(o.id, o.what)).collect()
        };
        let ids: HashSet<usize> = now.iter().map(|o| o.id).collect();
        // 水晶被打掉以后可能整个从场上拿掉：认得的水晶不见了，就当它倒了
        now.extend(self.known.values().filter(|b| matches!(b.what, What::Nexus(_)) && !ids.contains(&b.id)).map(|b| Objective { alive: false, ..*b }));
        let mut died: Vec<Objective> = Vec::new();
        for o in &now {
            if let Some(b) = self.known.get(&o.id).filter(|b| b.alive && !o.alive) {
                died.push(*b);
            }
        }
        // 死了以后整个从场上拿掉的也算
        died.extend(self.known.values().filter(|b| b.alive && !ids.contains(&b.id)).copied());
        died.retain(|o| !matches!(o.what, What::Nexus(_)));
        self.known = now.into_iter().map(|o| (o.id, o)).collect();
        // 选手：有龙刚死（看谁拿的）、到了记一行或算一次的时候、还没算阵容胜率，才读
        let row_due = record && tick >= self.next_row;
        let live_due = show && tick >= self.next_live;
        let mut step = Step::default();
        if !died.is_empty() || row_due || live_due || self.draft.is_none() {
            let players = pitch.players();
            for m in died {
                let Some(team) = credit(m.pos, &players) else { continue };
                if m.what == What::Baron {
                    self.baron_until[team] = tick + (BARON_SECONDS * tps) as usize;
                } else {
                    self.drakes[team] += 1;
                }
            }
            if self.draft.is_none() && players.iter().filter(|p| !p.champion.is_empty()).count() >= 10 {
                self.draft = Some(draft_of(&players));
            }
            if row_due || live_due {
                step.situation = self.situation(tick, tps, &players, &pitch.towers());
            }
        }
        if row_due {
            self.rows.push(step.situation);
            self.next_row = tick + ((ROW_SECONDS * tps) as usize).max(1);
        }
        if live_due {
            self.next_live = tick + ((LIVE_SECONDS * tps) as usize).max(1);
            step.chance = Some(model.chance(step.situation.minutes, &step.situation.features(self.draft.unwrap_or(0.0))));
        }
        // 一边的水晶没了：另一边赢
        let fallen: Vec<usize> = self
            .known
            .values()
            .filter_map(|o| match o.what {
                What::Nexus(t) if !o.alive && t < 2 => Some(t),
                _ => None,
            })
            .collect();
        if let [t] = fallen.as_slice() {
            step.winner = Some(1 - *t);
        }
        step
    }

    fn situation(&self, tick: usize, tps: f32, players: &[Player], towers: &[(usize, bool)]) -> Situation {
        let (mut gold, mut kills, mut level, mut count, mut alive) = ([0f32; 2], [0f32; 2], [0f32; 2], [0f32; 2], [0f32; 2]);
        for p in players.iter().filter(|p| p.team < 2) {
            gold[p.team] += p.gold;
            kills[p.team] += p.kills as f32;
            level[p.team] += p.level as f32;
            count[p.team] += 1.0;
            alive[p.team] += f32::from(u8::from(p.alive));
        }
        let mut standing = [0f32; 2];
        for (team, up) in towers {
            if *team < 2 && *up {
                standing[*team] += 1.0;
            }
        }
        let avg = |t: usize| level[t] / count[t].max(1.0);
        let baron = |t: usize| f32::from(u8::from(tick < self.baron_until[t]));
        Situation {
            minutes: tick as f32 / tps.max(1.0) / 60.0,
            gold_k: (gold[0] - gold[1]) / 1000.0,
            kills: kills[0] - kills[1],
            towers: standing[0] - standing[1],
            level: avg(0) - avg(1),
            drakes: self.drakes[0] as f32 - self.drakes[1] as f32,
            baron: baron(0) - baron(1),
            alive: alive[0] - alive[1],
        }
    }
}

// ===================================================================== 模型

/// 每个时间点的权重（帽子函数，加起来是 1；最后一个点以后都算它的）
fn weights(minutes: f32) -> [f32; K] {
    let mut w = [0f32; K];
    let t = minutes.max(0.0);
    if t >= KNOTS[K - 1] {
        w[K - 1] = 1.0;
        return w;
    }
    for k in 0..K - 1 {
        if t <= KNOTS[k + 1] {
            let f = (t - KNOTS[k]) / (KNOTS[k + 1] - KNOTS[k]);
            w[k] = 1.0 - f;
            w[k + 1] = f;
            break;
        }
    }
    w
}

/// 胜率模型：每一项在每个时间点的系数
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub coef: [[f32; K]; FEATURES],
    /// 拟合用了多少场、多少行（0 = 先验）
    pub matches: usize,
    pub rows: usize,
}

impl Default for Model {
    fn default() -> Self {
        Model { coef: PRIOR, matches: 0, rows: 0 }
    }
}

impl Model {
    /// 某个时刻这一项的系数
    pub fn at(&self, feature: usize, minutes: f32) -> f32 {
        weights(minutes).iter().zip(&self.coef[feature]).map(|(w, c)| w * c).sum()
    }

    pub fn logit(&self, minutes: f32, x: &[f32; FEATURES]) -> f32 {
        (0..FEATURES).map(|f| x[f] * self.at(f, minutes)).sum()
    }

    /// 蓝方胜率
    pub fn chance(&self, minutes: f32, x: &[f32; FEATURES]) -> f32 {
        sigmoid(self.logit(minutes, x))
    }
}

/// 一场记录：BP 完的阵容胜率（对数几率）、蓝方赢没赢、每隔 [`ROW_SECONDS`] 秒的局面
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Match {
    pub draft: f32,
    pub blue_won: bool,
    pub rows: Vec<Situation>,
}

impl Match {
    /// 文件里的一行：`{"v":1,"d":阵容,"w":1,"r":[[分钟,经济,人头,塔,等级,小龙,大龙,活着],...]}`
    pub fn encode(&self) -> String {
        let round = |x: f32| (x * 100.0).round() / 100.0;
        let rows: Vec<Vec<f32>> = self.rows.iter().map(|s| s.row().iter().map(|x| round(*x)).collect()).collect();
        serde_json::json!({ "v": 1, "d": round(self.draft), "w": u8::from(self.blue_won), "r": rows }).to_string()
    }

    pub fn parse(line: &str) -> Option<Match> {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        if v.get("v")?.as_u64()? != 1 {
            return None;
        }
        let rows = v
            .get("r")?
            .as_array()?
            .iter()
            .map(|r| {
                let nums: Vec<f32> = r.as_array()?.iter().map(|x| x.as_f64().map(|x| x as f32)).collect::<Option<_>>()?;
                Situation::from_row(&nums)
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Match { draft: v.get("d")?.as_f64()? as f32, blue_won: v.get("w")?.as_u64()? == 1, rows })
    }
}

/// 某一行在全部参数（每一项 × 每个时间点）上的取值，只有两个时间点不是 0
fn expand(s: &Situation, draft: f32) -> Vec<(usize, f64)> {
    let x = s.features(draft);
    let w = weights(s.minutes);
    let mut out = Vec::with_capacity(FEATURES * 2);
    for (f, xf) in x.iter().enumerate() {
        if *xf == 0.0 {
            continue;
        }
        for (k, wk) in w.iter().enumerate() {
            if *wk > 0.0 {
                out.push((f * K + k, f64::from(*xf) * f64::from(*wk)));
            }
        }
    }
    out
}

/// 按记录拟合（牛顿法，往先验靠）；不够 [`MIN_MATCHES`] 场就是 None
pub fn fit(matches: &[Match]) -> Option<Model> {
    if matches.len() < MIN_MATCHES {
        return None;
    }
    let n = FEATURES * K;
    let mut beta: Vec<f64> = (0..n).map(|i| f64::from(PRIOR[i / K][i % K])).collect();
    let rows: Vec<(Vec<(usize, f64)>, f64)> = matches
        .iter()
        .flat_map(|m| m.rows.iter().map(move |s| (expand(s, m.draft), if m.blue_won { 1.0 } else { 0.0 })))
        .collect();
    for _ in 0..25 {
        let mut grad = vec![0f64; n];
        let mut hess = vec![0f64; n * n];
        for i in 0..n {
            let (mean, sd) = (f64::from(PRIOR[i / K][i % K]), f64::from(PRIOR_SD[i / K]));
            grad[i] -= (beta[i] - mean) / (sd * sd);
            hess[i * n + i] += 1.0 / (sd * sd);
        }
        for (z, y) in &rows {
            let eta: f64 = z.iter().map(|(i, v)| beta[*i] * v).sum();
            let p = 1.0 / (1.0 + (-eta).exp());
            let wgt = p * (1.0 - p);
            for (i, vi) in z {
                grad[*i] += (y - p) * vi;
                for (j, vj) in z {
                    hess[i * n + j] += wgt * vi * vj;
                }
            }
        }
        let step = solve(&mut hess, grad, n)?;
        let biggest = step.iter().fold(0f64, |m, s| m.max(s.abs()));
        for (b, s) in beta.iter_mut().zip(&step) {
            *b += s;
        }
        if biggest < 1e-6 {
            break;
        }
    }
    let mut coef = [[0f32; K]; FEATURES];
    for (i, b) in beta.iter().enumerate() {
        coef[i / K][i % K] = *b as f32;
    }
    Some(Model { coef, matches: matches.len(), rows: rows.len() })
}

/// 解 `a x = b`（a 对称正定：Cholesky）；数值出问题就是 None
fn solve(a: &mut [f64], mut b: Vec<f64>, n: usize) -> Option<Vec<f64>> {
    for j in 0..n {
        let mut d = a[j * n + j];
        for k in 0..j {
            d -= a[j * n + k] * a[j * n + k];
        }
        if d <= 1e-12 || !d.is_finite() {
            return None;
        }
        let d = d.sqrt();
        a[j * n + j] = d;
        for i in j + 1..n {
            let mut s = a[i * n + j];
            for k in 0..j {
                s -= a[i * n + k] * a[j * n + k];
            }
            a[i * n + j] = s / d;
        }
    }
    for i in 0..n {
        let mut s = b[i];
        for k in 0..i {
            s -= a[i * n + k] * b[k];
        }
        b[i] = s / a[i * n + i];
    }
    for i in (0..n).rev() {
        let mut s = b[i];
        for k in i + 1..n {
            s -= a[k * n + i] * b[k];
        }
        b[i] = s / a[i * n + i];
    }
    b.iter().all(|x| x.is_finite()).then_some(b)
}

/// 一个模型在这些比赛上的平均对数损失和 Brier 分数（越小越好）
pub fn score(model: &Model, matches: &[Match]) -> (f32, f32) {
    let (mut loss, mut brier, mut n) = (0f64, 0f64, 0f64);
    for m in matches {
        let y = if m.blue_won { 1.0 } else { 0.0 };
        for s in &m.rows {
            let p = f64::from(model.chance(s.minutes, &s.features(m.draft))).clamp(1e-6, 1.0 - 1e-6);
            loss -= y * p.ln() + (1.0 - y) * (1.0 - p).ln();
            brier += (p - y) * (p - y);
            n += 1.0;
        }
    }
    if n == 0.0 {
        return (0.0, 0.0);
    }
    ((loss / n) as f32, (brier / n) as f32)
}

// ===================================================================== 文件、模型、给界面的

static MODEL: RwLock<Option<Arc<Model>>> = RwLock::new(None);
static FILE_LOCK: Mutex<()> = Mutex::new(());
static FITTING: AtomicBool = AtomicBool::new(false);
/// 上次拟合时文件多长（字节）
static FITTED_LEN: AtomicU64 = AtomicU64::new(0);
/// 一秒几个 tick（客户端读游戏设置后告诉这里）
static TPS: AtomicU32 = AtomicU32::new(0);

pub fn set_tps(tps: f32) {
    if tps > 0.0 {
        TPS.store(tps.to_bits(), Ordering::Relaxed);
    }
}

fn tps() -> f32 {
    let bits = TPS.load(Ordering::Relaxed);
    if bits == 0 {
        crate::objectives::DEFAULT_TARGETS.tps
    } else {
        f32::from_bits(bits)
    }
}

/// 现在用的模型（还没拟合就是先验）
pub fn model() -> Arc<Model> {
    MODEL.read().unwrap_or_else(PoisonError::into_inner).clone().unwrap_or_default()
}

pub fn set_model(model: Option<Model>) {
    *MODEL.write().unwrap_or_else(PoisonError::into_inner) = model.map(Arc::new);
}

fn path() -> std::path::PathBuf {
    crate::paths::mod_dir().join(FILE)
}

/// 一场记完：追加一行
fn append(m: &Match) {
    let _guard = FILE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    // 2.15.7：交给后台按顺序追加，比赛线程不等硬盘（写不进去后台会在 diag.log 记一次）
    crate::writer::append(path(), m.encode());
}

/// 读记录（最近 [`KEPT`] 场）；文件太长就顺手只留最近的
pub fn read() -> Vec<Match> {
    let _guard = FILE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    // 后台还没追加完的行先写完再读（拿着 FILE_LOCK，读和修剪的时候不会有新行插进来）
    crate::writer::flush();
    let Ok(text) = std::fs::read_to_string(path()) else { return Vec::new() };
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let from = lines.len().saturating_sub(KEPT);
    if from > KEPT / 2 {
        let kept: String = lines[from..].iter().map(|l| format!("{l}\n")).collect();
        let _ = std::fs::write(path(), kept);
    }
    lines[from..].iter().filter_map(|l| Match::parse(l)).collect()
}

/// 客户端隔一会儿叫一次：还没有模型、或者记录又多了不少，就在后台重新拟合
pub fn maybe_fit() {
    let len = std::fs::metadata(path()).map_or(0, |m| m.len());
    let fitted = FITTED_LEN.load(Ordering::Relaxed);
    // 第一次有记录就试一次，之后记录多了一截再来
    if len == 0 || (fitted != 0 && len < fitted + REFIT_BYTES) {
        return;
    }
    if FITTING.swap(true, Ordering::Relaxed) {
        return;
    }
    FITTED_LEN.store(len.max(1), Ordering::Relaxed);
    std::thread::spawn(|| {
        refit();
        FITTING.store(false, Ordering::Relaxed);
    });
}

/// 读记录、拟合、用最近两成比赛检验，日志写一行
pub fn refit() {
    let matches = read();
    if matches.len() < MIN_MATCHES {
        diag::log_once(
            "winprob-few",
            &format!("[winprob] {} matches recorded so far: the live win chance uses its starting values until {MIN_MATCHES}", matches.len()),
        );
        return;
    }
    let cut = matches.len() * 4 / 5;
    let check = fit(&matches[..cut]).map(|m| (score(&m, &matches[cut..]), score(&Model::default(), &matches[cut..])));
    let Some(model) = fit(&matches) else {
        diag::log("[winprob] the fit failed: keeping the model it had");
        return;
    };
    let at = |f: usize, t: f32| model.at(f, t);
    let checked = check.map_or(String::new(), |((loss, brier), (loss0, brier0))| {
        format!("; on the newest fifth after fitting the rest: log loss {loss:.3} (starting values {loss0:.3}), Brier {brier:.3} ({brier0:.3})")
    });
    diag::log(&format!(
        "[winprob] fitted on {} matches ({} rows){checked}; at 10 min: draft x{:.2}, gold {:+.2} a thousand, kills {:+.3}, towers {:+.2}, \
         level {:+.2}, drakes {:+.2}, Baron {:+.2}, alive {:+.2}",
        model.matches,
        model.rows,
        at(0, 10.0),
        at(2, 10.0),
        at(3, 10.0),
        at(4, 10.0),
        at(5, 10.0),
        at(6, 10.0),
        at(7, 10.0),
        at(8, 10.0)
    ));
    set_model(Some(model));
}

/// 界面要画的：这一份比赛的胜率一路下来，和最近一次大起落的说明
#[derive(Clone, Debug)]
pub struct Live {
    /// 每次有新的一点就加一
    pub version: u64,
    /// （分钟，蓝方胜率）
    pub samples: Vec<(f32, f32)>,
    /// 最近一次大起落：从第几分钟起、说明
    pub note: Option<(f32, String)>,
    /// 最近一次更新（比赛不看了就不画）
    pub at: Instant,
    key: RunKey,
    /// 最近的局面，找大起落的原因用：（分钟，蓝方胜率，局面）
    recent: VecDeque<(f32, f32, Situation)>,
}

static LIVE: Mutex<Option<Live>> = Mutex::new(None);
static VERSION: AtomicU64 = AtomicU64::new(0);
/// 起落看多长一段（分钟）、多大算大、两句说明至少隔多久
const SWING_WINDOW: f32 = 10.0 / 60.0;
const SWING: f32 = 0.10;
const NOTE_GAP: f32 = 15.0 / 60.0;

/// 现在的显示（界面每隔几帧取一次）
pub fn live() -> Option<Live> {
    LIVE.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// 起落的原因：这段时间里变得最明显的一项
pub fn reason(then: &Situation, now: &Situation, delta: f32) -> String {
    let side = if delta > 0.0 { "蓝方" } else { "红方" };
    let toward = |d: f32| d * delta.signum() > 0.0;
    let what = if toward(now.baron - then.baron) {
        "拿下大龙".to_string()
    } else if toward(now.drakes - then.drakes) {
        "拿下小龙".to_string()
    } else if toward(now.towers - then.towers) {
        let n = (now.towers - then.towers).abs().round() as i32;
        if n > 1 { format!("推掉 {n} 座塔") } else { "推掉一座塔".to_string() }
    } else if toward(now.kills - then.kills) || toward(now.alive - then.alive) {
        "打赢一波团战".to_string()
    } else {
        "扩大优势".to_string()
    };
    format!("{side}{what} {:+.0}%", delta.abs() * 100.0)
}

fn publish(key: RunKey, minutes: f32, chance: f32, situation: Situation) {
    let mut live = LIVE.lock().unwrap_or_else(PoisonError::into_inner);
    // 正在显示的另一份两秒内还在更新：不换（客户端可能同时跑着别的模拟）
    if live.as_ref().is_some_and(|l| l.key != key && l.at.elapsed() < std::time::Duration::from_secs(2)) {
        return;
    }
    let fresh = match live.as_ref() {
        Some(l) => l.key != key || l.samples.last().is_some_and(|(t, _)| minutes + 1e-3 < *t),
        None => true,
    };
    if fresh {
        *live = Some(Live { version: 0, samples: Vec::new(), note: None, at: Instant::now(), key, recent: VecDeque::new() });
    }
    let Some(l) = live.as_mut() else { return };
    l.samples.push((minutes, chance));
    l.recent.push_back((minutes, chance, situation));
    while l.recent.front().is_some_and(|(t, _, _)| *t < minutes - SWING_WINDOW) {
        l.recent.pop_front();
    }
    if let Some((_, then_chance, then)) = l.recent.front().copied() {
        let delta = chance - then_chance;
        let quiet = l.note.as_ref().is_none_or(|(t, _)| minutes - t >= NOTE_GAP);
        if delta.abs() >= SWING && quiet {
            l.note = Some((minutes, reason(&then, &situation, delta)));
        }
    }
    l.at = Instant::now();
    l.version = VERSION.fetch_add(1, Ordering::Relaxed) + 1;
}

// ===================================================================== 接到比赛里

type RunKey = (u32, u64, u64, u64);

struct Run {
    key: RunKey,
    last_tick: usize,
    tracker: Option<Tracker>,
}

static RUNS: LazyLock<Mutex<VecDeque<Run>>> = LazyLock::new(|| Mutex::new(VecDeque::new()));

/// BP 完的阵容胜率（对数几率）：电竞大脑的模型，蓝方英雄对红方英雄；模型不认识的英雄不算，没有模型就是五五开。
/// 嚎哭深渊生涯里按不分路的那份算（2.15.6，和选人界面的胜率一样）
fn draft_logit(players: &[Player]) -> f32 {
    let Some(snapshot) = crate::shared::get_for_match() else { return 0.0 };
    let meta = &snapshot.meta;
    let side = |t: usize| -> Vec<u16> { players.iter().filter(|p| p.team == t).filter_map(|p| meta.names.get(&p.champion)).collect() };
    let (blue, red) = (side(0), side(1));
    if blue.is_empty() || red.is_empty() {
        return 0.0;
    }
    crate::glm::logit(crate::advisor::win_probability(meta, &blue, &red))
}

/// 每一份比赛的每个 tick（Mod 的比赛钩子）：服务端预演记录，客户端看比赛算胜率。`on`：设置 `live_win_chance`
pub fn on_tick(sim: &StableSim<'_>, on: bool) {
    if !on {
        return;
    }
    let tick = sim.tick();
    let ended = sim.is_end();
    if !tick.is_multiple_of(SCAN) && !ended {
        return;
    }
    let Some(origin) = sim.sim_origin() else { return };
    let (record, show) = match SimOriginKindV1::from_code(origin.kind) {
        Some(SimOriginKindV1::ServerPresim) => (sim.player_count() >= 10, false),
        Some(SimOriginKindV1::ClientMatchView | SimOriginKindV1::ClientSpectate | SimOriginKindV1::ClientReplay) => (false, true),
        _ => return,
    };
    if !record && !show {
        return;
    }
    let key: RunKey = (origin.kind, origin.match_id, origin.replay_id, origin.set_index);
    // 拿出来算，算完放回去：别的比赛同时在跑
    let mut tracker = {
        let mut runs = RUNS.lock().unwrap_or_else(PoisonError::into_inner);
        let at = match runs.iter().position(|r| r.key == key) {
            Some(at) => at,
            None => {
                runs.push_back(Run { key, last_tick: tick, tracker: Some(Tracker::default()) });
                while runs.len() > RUNS_KEPT {
                    runs.pop_front();
                }
                runs.len() - 1
            }
        };
        let run = &mut runs[at];
        if tick < run.last_tick {
            // 这一份从头来了
            run.tracker = Some(Tracker::default());
        }
        run.last_tick = tick;
        match run.tracker.take() {
            Some(t) if !t.done => t,
            other => {
                run.tracker = other;
                return;
            }
        }
    };
    let model = model();
    let step = tracker.step(&SimPitch { sim }, tps(), record, show, &model, &draft_logit);
    if let Some(chance) = step.chance {
        publish(key, step.situation.minutes, chance, step.situation);
    }
    let winner = step.winner.or_else(|| ended.then_some(usize::MAX));
    if record && winner == Some(usize::MAX) {
        let nexus: Vec<String> = tracker
            .known
            .values()
            .filter_map(|o| match o.what {
                What::Nexus(t) => Some(format!("team {t} {}", if o.alive { "standing" } else { "down" })),
                _ => None,
            })
            .collect();
        diag::log_once("winprob-no-winner", &format!("[winprob] a match ended with no winner seen (nexus: {nexus:?}): not recorded"));
    }
    if let Some(w) = winner {
        if record && w < 2 && !tracker.rows.is_empty() {
            append(&Match { draft: tracker.draft.unwrap_or(0.0), blue_won: w == 0, rows: std::mem::take(&mut tracker.rows) });
            diag::log_once("winprob-first", "[winprob] the first match is in winprob.jsonl (the server's runs write one line a match)");
        }
        tracker.done = true;
    }
    let mut runs = RUNS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(run) = runs.iter_mut().find(|r| r.key == key) {
        if run.tracker.is_none() {
            run.tracker = Some(tracker);
        }
    }
}

/// 游戏的一份模拟
pub struct SimPitch<'s, 'a> {
    pub sim: &'s StableSim<'a>,
}

impl Pitch for SimPitch<'_, '_> {
    fn tick(&self) -> usize {
        self.sim.tick()
    }

    fn players(&self) -> Vec<Player> {
        let mut out = Vec::new();
        for i in 0..self.sim.player_count() {
            let Some(p) = self.sim.player_at(i) else { continue };
            let champion = p.champion();
            // 这一场到现在的经济：比赛统计里的 gold，读不到就用身上的钱
            let earned = p.statistics_json("gold").and_then(|j| j.trim().parse::<f64>().ok());
            let wallet = p.gold() as f64;
            match earned {
                Some(e) if (e - wallet).abs() > 1000.0 => diag::log_once(
                    "winprob-gold-earned",
                    &format!("[winprob] the match statistics' gold runs ahead of the gold on hand ({e:.0} vs {wallet:.0}): it is the gold earned"),
                ),
                Some(e) => diag::log_once("winprob-gold", &format!("[winprob] gold from the match statistics (e.g. {e:.0}, on hand {wallet:.0})")),
                None => diag::log_once("winprob-gold", &format!("[winprob] no gold in the match statistics: gold on hand is used (e.g. {wallet:.0})")),
            }
            out.push(Player {
                team: p.team(),
                champion: champion.as_ref().and_then(|c| c.name()).unwrap_or_default(),
                alive: p.is_alive(),
                pos: champion.as_ref().map_or((0, 0), |c| c.pos()),
                kills: p.kills() as u32,
                level: p.level() as u32,
                gold: earned.unwrap_or(wallet) as f32,
            });
        }
        out
    }

    fn towers(&self) -> Vec<(usize, bool)> {
        (0..self.sim.tower_count())
            .filter_map(|i| self.sim.get_entity(self.sim.tower_id_at(i)))
            .map(|e| (e.team(), e.is_alive()))
            .collect()
    }

    fn objectives(&self) -> Vec<Objective> {
        let mut out = Vec::new();
        for i in 0..self.sim.entity_count() {
            let Some(e) = self.sim.entity_at(i) else { continue };
            let what = match e.kind() {
                Some(EntityKindV1::Serpen) => What::Drake,
                Some(EntityKindV1::Epic) => What::Baron,
                Some(EntityKindV1::Nexus) => What::Nexus(e.team()),
                _ => continue,
            };
            out.push(Objective { id: e.id(), what, alive: e.is_alive(), pos: e.pos() });
        }
        out
    }

    fn objective(&self, id: usize, what: What) -> Option<Objective> {
        let e = self.sim.get_entity(id)?;
        Some(Objective { id, what, alive: e.is_alive(), pos: e.pos() })
    }
}

/// 测试：给界面塞一个点
#[cfg(test)]
pub fn publish_for_tests(minutes: f32, chance: f32) {
    publish((2, 0, 0, 0), minutes, chance, Situation::default());
}

pub fn reset_for_tests() {
    set_model(None);
    *LIVE.lock().unwrap_or_else(PoisonError::into_inner) = None;
    RUNS.lock().unwrap_or_else(PoisonError::into_inner).clear();
    FITTED_LEN.store(0, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(team: usize, x: u64) -> Player {
        Player { team, champion: format!("c{team}{x}"), alive: true, pos: (x, 0), kills: 0, level: 1, gold: 500.0 }
    }

    #[test]
    fn the_time_weights_are_hats() {
        assert_eq!(weights(0.0), [1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(weights(7.5), [0.0, 0.5, 0.5, 0.0, 0.0, 0.0]);
        assert_eq!(weights(45.0), [0.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
        let m = Model::default();
        assert!((m.at(2, 7.5) - 0.25).abs() < 1e-6);
        // 局面全平、阵容五五开：五五开
        assert!((m.chance(12.0, &Situation::default().features(0.0)) - 0.5).abs() < 1e-6);
        // 大龙在手：蓝方占优
        let s = Situation { minutes: 20.0, baron: 1.0, ..Situation::default() };
        assert!(m.chance(20.0, &s.features(0.0)) > 0.6);
    }

    #[test]
    fn a_monster_goes_to_the_team_around_it() {
        let players = [player(0, 10), player(0, 12), player(1, 11), player(1, 500), player(0, 900)];
        assert_eq!(credit((11, 0), &players), Some(0), "two blue and one red near it");
        let tied = [player(1, 10), player(0, 20)];
        assert_eq!(credit((9, 0), &tied), Some(1), "a tie: the nearest");
        assert_eq!(credit((0, 0), &[]), None);
    }

    /// 假的一份比赛
    #[derive(Default)]
    struct Fake {
        tick: usize,
        players: Vec<Player>,
        towers: Vec<(usize, bool)>,
        objectives: Vec<Objective>,
    }

    impl Pitch for Fake {
        fn tick(&self) -> usize {
            self.tick
        }
        fn players(&self) -> Vec<Player> {
            self.players.clone()
        }
        fn towers(&self) -> Vec<(usize, bool)> {
            self.towers.clone()
        }
        fn objectives(&self) -> Vec<Objective> {
            self.objectives.clone()
        }
        fn objective(&self, id: usize, _what: What) -> Option<Objective> {
            self.objectives.iter().find(|o| o.id == id).copied()
        }
    }

    #[test]
    fn a_match_is_followed_and_recorded() {
        let mut f = Fake {
            players: (0..5).map(|i| player(0, 100 + i)).chain((0..5).map(|i| player(1, 900 + i))).collect(),
            towers: vec![(0, true), (0, true), (1, true), (1, true)],
            objectives: vec![
                Objective { id: 50, what: What::Baron, alive: true, pos: (101, 0) },
                Objective { id: 51, what: What::Drake, alive: true, pos: (902, 0) },
                Objective { id: 1, what: What::Nexus(0), alive: true, pos: (0, 0) },
                Objective { id: 2, what: What::Nexus(1), alive: true, pos: (999, 0) },
            ],
            ..Fake::default()
        };
        let mut t = Tracker::default();
        let model = Model::default();
        let draft = |_: &[Player]| 0.4f32;
        // 开局第 0 tick 英雄还没有名字：先不算阵容胜率
        let nameless: Vec<Player> = f.players.iter().map(|p| Player { champion: String::new(), ..p.clone() }).collect();
        let named = std::mem::replace(&mut f.players, nameless);
        t.step(&f, 60.0, false, false, &model, &draft);
        assert_eq!(t.draft, None);
        f.players = named;
        let step = t.step(&f, 60.0, true, true, &model, &draft);
        assert_eq!(t.draft, Some(0.4));
        assert_eq!(t.rows.len(), 1, "the first row at once");
        assert!((step.chance.unwrap() - sigmoid(0.4)).abs() < 1e-6, "at the start: the draft alone");
        // 一分钟后：蓝方拿了大龙（大龙在蓝方中间死的）、红方拿了小龙、红方掉一座塔、蓝方多 2000 经济
        f.tick = 3600;
        f.objectives[0].alive = false;
        f.objectives.remove(1);
        f.towers[3].1 = false;
        for p in f.players.iter_mut().filter(|p| p.team == 0) {
            p.gold = 900.0;
        }
        let step = t.step(&f, 60.0, true, true, &model, &draft);
        assert_eq!(t.drakes, [0, 1], "the drake that vanished went to red, who stood by it");
        assert!(t.baron_until[0] > 3600 && t.baron_until[1] == 0);
        let s = step.situation;
        assert_eq!((s.baron, s.drakes, s.towers, s.gold_k), (1.0, -1.0, 1.0, 2.0));
        assert!((s.minutes - 1.0).abs() < 1e-6);
        assert_eq!(t.rows.len(), 2, "a row every 30 s");
        assert!(step.chance.unwrap() > sigmoid(0.4));
        // 红方水晶没了：蓝方赢
        f.tick = 3630;
        f.objectives[2].alive = false;
        assert_eq!(t.step(&f, 60.0, true, true, &model, &draft).winner, Some(0));
        assert_eq!(t.rows.len(), 2, "not 30 s yet");
    }

    #[test]
    fn a_nexus_that_vanishes_has_fallen() {
        let mut f = Fake {
            players: (0..5).map(|i| player(0, 100 + i)).chain((0..5).map(|i| player(1, 900 + i))).collect(),
            objectives: vec![
                Objective { id: 1, what: What::Nexus(0), alive: true, pos: (0, 0) },
                Objective { id: 2, what: What::Nexus(1), alive: true, pos: (999, 0) },
            ],
            ..Fake::default()
        };
        let mut t = Tracker::default();
        let model = Model::default();
        let draft = |_: &[Player]| 0.0f32;
        assert_eq!(t.step(&f, 60.0, true, false, &model, &draft).winner, None);
        // 蓝方的水晶整个从场上拿掉了：红方赢
        f.tick = 30;
        f.objectives.remove(0);
        assert_eq!(t.step(&f, 60.0, true, false, &model, &draft).winner, Some(1));
    }

    #[test]
    fn a_match_line_round_trips() {
        let m = Match {
            draft: 0.31,
            blue_won: true,
            rows: vec![Situation { minutes: 0.5, gold_k: 1.25, kills: 2.0, towers: -1.0, level: 0.4, drakes: 1.0, baron: 0.0, alive: 1.0 }],
        };
        let back = Match::parse(&m.encode()).unwrap();
        assert_eq!(back, m);
        assert!(Match::parse("{\"v\":2}").is_none() && Match::parse("junk").is_none());
    }

    /// 造的比赛按已知的系数出胜负，拟合应该找回这些系数
    #[test]
    fn the_fit_finds_the_numbers_it_was_made_with() {
        let mut truth = Model::default();
        truth.coef[2] = [0.0, 0.5, 0.5, 0.5, 0.5, 0.5];
        truth.coef[7] = [1.2; K];
        let mut seed: u64 = 7;
        let mut rnd = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as f32) / (u32::MAX >> 1) as f32
        };
        let mut matches = Vec::new();
        for _ in 0..600 {
            let draft = (rnd() - 0.5) * 1.0;
            let mut rows = Vec::new();
            let mut gold = 0.0;
            for r in 0..24 {
                gold += (rnd() - 0.5) * 2.0;
                let minutes = r as f32 * 0.5;
                let baron = if minutes > 8.0 && rnd() < 0.2 { if rnd() < 0.5 { 1.0 } else { -1.0 } } else { 0.0 };
                rows.push(Situation { minutes, gold_k: gold, baron, ..Situation::default() });
            }
            // 胜负按最后一行的局面掷
            let last = rows.last().unwrap();
            let p = truth.chance(last.minutes, &last.features(draft));
            matches.push(Match { draft, blue_won: rnd() < p, rows });
        }
        let fitted = fit(&matches).expect("enough matches");
        assert!((fitted.at(2, 11.0) - 0.5).abs() < 0.15, "gold {}", fitted.at(2, 11.0));
        assert!(score(&fitted, &matches).0 < score(&Model::default(), &matches).0, "better than the starting values");
        assert!(fit(&matches[..10]).is_none(), "too few");
    }

    #[test]
    fn a_swing_says_why() {
        let then = Situation { minutes: 20.0, ..Situation::default() };
        let now = Situation { minutes: 20.1, baron: 1.0, ..Situation::default() };
        assert_eq!(reason(&then, &now, 0.18), "蓝方拿下大龙 +18%");
        let towers = Situation { minutes: 20.1, towers: -2.0, ..Situation::default() };
        assert_eq!(reason(&then, &towers, -0.12), "红方推掉 2 座塔 +12%");
        let fight = Situation { minutes: 20.1, kills: 3.0, alive: 3.0, ..Situation::default() };
        assert_eq!(reason(&then, &fight, 0.11), "蓝方打赢一波团战 +11%");
    }

    #[test]
    fn the_live_view_starts_over_for_another_match_and_notes_a_swing() {
        let _serial = crate::tests::serial();
        reset_for_tests();
        let calm = Situation::default();
        publish((2, 1, 0, 0), 0.0, 0.5, calm);
        publish((2, 1, 0, 0), 0.05, 0.52, calm);
        let baron = Situation { baron: 1.0, ..calm };
        publish((2, 1, 0, 0), 0.1, 0.70, baron);
        let l = live().unwrap();
        assert_eq!(l.samples.len(), 3);
        assert_eq!(l.note.as_ref().map(|(_, n)| n.as_str()), Some("蓝方拿下大龙 +20%"));
        // 另一份模拟两秒内插进来：不换
        publish((2, 9, 0, 0), 0.0, 0.1, calm);
        assert_eq!(live().unwrap().samples.len(), 3);
        // 这一份停了一阵，另一场开始：从头来
        if let Some(l) = LIVE.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
            l.at = l.at.checked_sub(std::time::Duration::from_secs(3)).unwrap_or(l.at);
        }
        publish((2, 2, 0, 0), 0.0, 0.45, calm);
        let l = live().unwrap();
        assert_eq!(l.samples, vec![(0.0, 0.45)]);
        assert!(l.note.is_none());
        reset_for_tests();
    }
}
