//! The model's output as the draft hook (and the draft screen) read it. The client publishes a
//! new snapshot after every fit; the draft hook runs wherever the game drafts (AI matches on
//! the management server, the opponent in your own matches) and only reads it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use crate::advisor::Damage;
use crate::meta::Meta;

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub meta: Meta,
    /// Physical / magic damage per champion id (from the champions' tags).
    pub damage: HashMap<u16, Damage>,
    /// Athletes' and teams' names the client has read (for `diag.log`).
    pub athlete_names: HashMap<u32, String>,
    pub team_names: HashMap<u32, String>,
}

impl Snapshot {
    pub fn new(meta: Meta, damage: HashMap<u16, Damage>) -> Self {
        Self { meta, damage, athlete_names: HashMap::new(), team_names: HashMap::new() }
    }

    pub fn damage_of(&self, champ: u16) -> Option<Damage> {
        self.damage.get(&champ).copied()
    }
}

static SNAPSHOT: RwLock<Option<Arc<Snapshot>>> = RwLock::new(None);

pub fn publish(snapshot: Snapshot) {
    *SNAPSHOT.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(snapshot));
}

pub fn clear() {
    *SNAPSHOT.write().unwrap_or_else(PoisonError::into_inner) = None;
    *ONE_LANE.write().unwrap_or_else(PoisonError::into_inner) = None;
}

pub fn get() -> Option<Arc<Snapshot>> {
    SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// 去掉分路的那份（2.15.6）和它是从哪份模型复制的
static ONE_LANE: RwLock<Option<(Arc<Snapshot>, Arc<Snapshot>)>> = RwLock::new(None);

/// 比赛和 BP 用的模型（2.15.6）：打开的是嚎哭深渊生涯（[`crate::compat::aram`]）时给去掉分路的那份（`Meta::no_lanes`），
/// 别的生涯就是 [`get`]。选人钩子、界面上的建议（选人界面、首发推荐、F8 的侦察）、比赛里的实时胜率都从这里拿；
/// 各路的胜率、场次这些统计数字是存档里记的，照旧显示。
pub fn get_for_match() -> Option<Arc<Snapshot>> {
    let base = get()?;
    Some(if crate::compat::aram() { one_lane(&base) } else { base })
}

/// `base` 去掉分路的那份：每份模型只复制一次
pub fn one_lane(base: &Arc<Snapshot>) -> Arc<Snapshot> {
    if base.meta.no_lanes {
        return base.clone();
    }
    if let Some((from, copy)) = ONE_LANE.read().unwrap_or_else(PoisonError::into_inner).as_ref() {
        if Arc::ptr_eq(from, base) {
            return copy.clone();
        }
    }
    let mut copy = Snapshot::clone(base);
    copy.meta.no_lanes = true;
    let copy = Arc::new(copy);
    *ONE_LANE.write().unwrap_or_else(PoisonError::into_inner) = Some((base.clone(), copy.clone()));
    copy
}

/// The player's team id in the open save (`usize::MAX`: none), for the coach delegation.
static PLAYER_TEAM: AtomicUsize = AtomicUsize::new(usize::MAX);

pub fn set_player_team(team: Option<usize>) {
    PLAYER_TEAM.store(team.unwrap_or(usize::MAX), Ordering::Relaxed);
}

pub fn player_team() -> Option<usize> {
    Some(PLAYER_TEAM.load(Ordering::Relaxed)).filter(|t| *t != usize::MAX)
}

/// The draft screen's ranking for the player's team's turn now (the panel's 禁 1 / 选 order, the
/// lookahead included), for the coach override (`draft::CoachHook`). Champions are the model's
/// ids; the picks are sorted, so the hook can tell whether the plan is for the draft it is asked
/// about.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CoachPlan {
    pub ally: Vec<u16>,
    pub enemy: Vec<u16>,
    /// Bans made so far, both sides.
    pub bans: usize,
    pub ban: bool,
    /// Best first.
    pub ranked: Vec<u16>,
    /// Cards a position lock holds (the team may not pick them now).
    pub held: Vec<u16>,
}

static COACH_PLAN: RwLock<Option<Arc<CoachPlan>>> = RwLock::new(None);

pub fn set_coach_plan(plan: Option<CoachPlan>) {
    *COACH_PLAN.write().unwrap_or_else(PoisonError::into_inner) = plan.map(Arc::new);
}

pub fn coach_plan() -> Option<Arc<CoachPlan>> {
    COACH_PLAN.read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// What the coach override chose in the player's own draft (model ids, in order), for the
/// review: a choice that does not show up on the screen was overridden by another mod.
static COACH_CHOICES: RwLock<Vec<(bool, u16)>> = RwLock::new(Vec::new());

pub fn note_coach_choice(ban: bool, champ: u16) {
    let mut list = COACH_CHOICES.write().unwrap_or_else(PoisonError::into_inner);
    if list.last() != Some(&(ban, champ)) {
        list.push((ban, champ));
    }
}

/// The coach override's choices since the last call (and forgets them).
pub fn take_coach_choices() -> Vec<(bool, u16)> {
    std::mem::take(&mut *COACH_CHOICES.write().unwrap_or_else(PoisonError::into_inner))
}

/// What `ai_bp` chose for the other team in the player's own draft (model ids, in order), for
/// the review, like [`note_coach_choice`].
static AI_CHOICES: RwLock<Vec<(bool, u16)>> = RwLock::new(Vec::new());

pub fn note_ai_choice(ban: bool, champ: u16) {
    let mut list = AI_CHOICES.write().unwrap_or_else(PoisonError::into_inner);
    if list.last() != Some(&(ban, champ)) {
        list.push((ban, champ));
    }
}

/// `ai_bp`'s choices since the last call (and forgets them).
pub fn take_ai_choices() -> Vec<(bool, u16)> {
    std::mem::take(&mut *AI_CHOICES.write().unwrap_or_else(PoisonError::into_inner))
}

/// Each team's habits (team id -> share of its recent competition matches it played each
/// champion in, by the model's ids), for `ai_bp`'s scouting. Published by the client.
pub type Habits = HashMap<u32, HashMap<u16, f32>>;

static HABITS: RwLock<Option<Arc<Habits>>> = RwLock::new(None);

pub fn set_habits(habits: Habits) {
    *HABITS.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(habits));
}

pub fn habits() -> Option<Arc<Habits>> {
    HABITS.read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// The player's last draft as the ban/pick screen saw it: both sides' champions (names, sorted),
/// the player's side (0 blue, 1 red) and whether the coach override made its picks (auto BP).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerDraft {
    pub sides: [Vec<String>; 2],
    pub side: usize,
    pub auto: bool,
}

static PLAYER_DRAFT: RwLock<Option<Arc<PlayerDraft>>> = RwLock::new(None);

pub fn set_player_draft(draft: Option<PlayerDraft>) {
    *PLAYER_DRAFT.write().unwrap_or_else(PoisonError::into_inner) = draft.map(Arc::new);
}

pub fn player_draft() -> Option<Arc<PlayerDraft>> {
    PLAYER_DRAFT.read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// 屏幕上这一局每支队选完以后的分路（2.14.5，[`crate::draft::seat_decide`] 记）：按选人顺序的英雄，和每个位置
/// （`Role::index`）坐的是第几手；`None` 是不知道（游戏或别的 Mod 排的）。选人界面照着把标签对上格子里的英雄。
pub type Seated = (Vec<String>, Option<[usize; 5]>);
static SEATED: RwLock<Vec<Seated>> = RwLock::new(Vec::new());
/// 一场两支队，留点余量
const SEATED_KEPT: usize = 8;

/// 记下一支队的分路（同样的选人只留最新的）
pub fn set_seated(picks: Vec<String>, by_lane: Option<[usize; 5]>) {
    let mut list = SEATED.write().unwrap_or_else(PoisonError::into_inner);
    list.retain(|(p, _)| *p != picks);
    list.push((picks, by_lane));
    let extra = list.len().saturating_sub(SEATED_KEPT);
    list.drain(..extra);
}

/// 这些选人（按选人顺序）的分路：`None` 是还没排；`Some(None)` 是排了，但不知道怎么排的
pub fn seated(picks: &[&str]) -> Option<Option<[usize; 5]>> {
    let list = SEATED.read().unwrap_or_else(PoisonError::into_inner);
    list.iter().rev().find(|(p, _)| p.iter().map(String::as_str).eq(picks.iter().copied())).map(|(_, s)| *s)
}

/// 选人界面打开：上一场的不要了
pub fn clear_seated() {
    SEATED.write().unwrap_or_else(PoisonError::into_inner).clear();
}

/// 电竞大脑自己写的一条普通新闻（2.15.2）：收件箱里打开的新闻读不出正文，长新闻滚动按序号从这里取
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OurNews {
    /// 在队伍新闻列表里的序号（收件箱列表那一行的名字）
    pub index: usize,
    pub title: String,
    /// `2027-09-20T13:00:00`
    pub date: String,
    pub content: String,
}

static OUR_NEWS: RwLock<Vec<OurNews>> = RwLock::new(Vec::new());
/// 只留最近这么多条
const OUR_NEWS_KEPT: usize = 60;

/// 整个新闻列表读过一遍：换掉
pub fn set_our_news(mut list: Vec<OurNews>) {
    let extra = list.len().saturating_sub(OUR_NEWS_KEPT);
    list.drain(..extra);
    *OUR_NEWS.write().unwrap_or_else(PoisonError::into_inner) = list;
}

/// 新来了一条（同一个序号的换掉）
pub fn add_our_news(news: OurNews) {
    let mut list = OUR_NEWS.write().unwrap_or_else(PoisonError::into_inner);
    list.retain(|n| n.index != news.index);
    list.push(news);
    let extra = list.len().saturating_sub(OUR_NEWS_KEPT);
    list.drain(..extra);
}

pub fn our_news() -> Vec<OurNews> {
    OUR_NEWS.read().unwrap_or_else(PoisonError::into_inner).clone()
}
