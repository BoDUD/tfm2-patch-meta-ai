//! Balance patches (2.8.0; the user 2026-11-11: 「现在能不能让meta控制版本平衡的补丁吧 现在系统自带的平衡补丁 英雄
//! 削弱不痛不痒 加强也是 砍不到位 没有lol那种感觉」, then 「我希望我们这套系统完全替代官方的 不要有死角」). On one save
//! at patch 2027.4.1 the game's own patches had nerfed champions at 46% (Gwen, Nami, Brand) while Maddie sat at 73%
//! with a 52% ban rate untouched. With `balance=on` this mod makes the patches instead, and they show wherever the
//! game's did:
//!
//! - **When**: a round for every patch the game makes (its patch notes appear in the player's news, [`Due::Patch`]);
//!   a first one when the setting is turned on, as a hotfix of the current patch (`2027.4.1b`, [`Due::First`]); a
//!   hotfix when the game made no patch for [`LATE_DAYS`] game days ([`Due::Late`]); and (2.10.0) one hotfix at once
//!   on a book whose newest round was made under older rules ([`Due::Recalibration`], [`RULES_VERSION`]).
//! - **What**: the champions the model rates furthest outside [`BAND`] - with [`MIN_EVIDENCE`] recent games or more,
//!   at most `balance_count` of them - change by `balance_per_point` percent of power for every win-rate point outside
//!   the band ([`Rules::per_point`], [`DEFAULT_PER_POINT`]), at most `balance_max` a round, at most [`CAP`] in all
//!   (rounds add up, as in League: a champion still too strong is cut again next patch). A champion a round changed
//!   waits until it has played [`MIN_EVIDENCE`] games with its new numbers ([`games_since_change`]; 2.8.1 - the first
//!   rounds on a save, a hotfix and the game's patch two days later, cut the same ten champions twice on the games
//!   from before the first cut).
//! - **Every champion** (2.10.0, `balance_unpicked`): champions with fewer than [`MIN_EVIDENCE`] recent games have no
//!   win rate to judge; a round also buffs some of them by [`UNPICKED_STEP`] (up to [`UNPICKED_CAP`]), besides
//!   `balance_count`, so that they get played and judged ([`unpicked`]) - the game's own patches did that before
//!   this mod replaced them.
//! - **Against the field** (2.9.0, the user 2026-11-13: 「先修再打包」): the win rates a round reads are measured from
//!   the middle of this patch's field ([`field_centre`], [`field_win`]), not from the model's strength 0 - its
//!   prior's centre, a champion nobody may play. On one save (2027.5.0) the 38 champions with 30 games that patch
//!   averaged 59.4% in the model and 51.8% in their games; the round due wanted 51 nerfs and 2 buffs, champions at
//!   38-44% in their games among the nerfs, and Zeri, Jinx and Kindred, at about 50% when the first rounds cut them
//!   20%, had fallen to 33-40%.
//! - **In every match**: a champion's power is a permanent buff ([`BUFF`]), half on damage (attack and magic power),
//!   half on health ([`buff`]), put on it at the start and again on every respawn (a new entity).
//! - **Every run alike**: a match is simulated more than once (the server's run is the result, the client plays it
//!   again to show it, replays again later). The server's first run takes the newest round and keeps it in this
//!   save's book under the ten champions (`ai_tactics`' key); every later run plays the kept round, and a run of a
//!   match nothing was kept for plays none.
//! - **The game's own notes**: a round's notes are written into the notes the game made for its patch (its own
//!   champion changes being off), or are patch notes of their own for a hotfix (`server::write_note`) - so the
//!   inbox, the patch history and the model read them as the game's. A champion's lines are the numbers it plays
//!   with ([`rows`]: base attack, magic power, health and their growth, the game's numbers times the buff), its
//!   reason the model's win rate ([`reason`], texts in `text/news.i18n`). The patch's news cards come as the game
//!   writes them ([`articles`]: the most buffed and most nerfed champion, the player's athletes' signature champions),
//!   and the stat panels of the ban/pick screen and the encyclopedia show the numbers with the power on
//!   (`ui::balance_view`).
//! - **Items** (2.9.0, `item_balance`, [`crate::items`]): with alele's "4+6" item pack, a round also changes the
//!   finished items whose builders - the champions whose published build names them - win too much or too little
//!   against the field ([`decide_items`]). Since 2.12.0 the items come first, on their builders' win rates before the
//!   round's champion changes, and the champions' steps count what the item steps are expected to do
//!   ([`item_effects`]), so a champion is not cut twice for the same points (2.9.0 - 2.11.x judged the items after the
//!   champions' steps, and the champions' steps had taken nearly everything). An item's power is a share of its
//!   numbers, put on its holders by a second buff in every match ([`on_tick`]); the items' notes are news of their own
//!   ([`item_news`]).
//! - **The game's own champion patches off**: the server half sets the save's auto-patcher counts to zero while this
//!   is on, keeping the save's own values to put back when it is turned off (`server::balance`).
//! - **The player's numbers** (2.11.0, [`crate::edits`]; the user: 「不覆盖玩家数据库里改的，完全按玩家的数值来，后面轮次进行
//!   正常削弱加强」): a champion's power is a buff on the numbers the save has, never a number written over them. When
//!   the player changes a champion's numbers in the database, its power comes off at once - a hotfix of its own
//!   ([`make_reset`], [`Reset::Edit`]) - so it plays exactly the player's numbers, and the rounds judge it again from
//!   the games it plays with them. Champions `balance_skip` lists (and items `item_balance_skip` lists) are never
//!   changed, their power taken off the same way ([`Reset::Skip`], [`Skip`]).
//! - **防御塔和大小龙**（2.13.0，[`crate::objectives`]）：每轮也给塔、小龙、大龙各一个生命值百分比，塔看对局时长，
//!   小龙大龙看刷新以后活多久；比赛里挂增益，服务端那一份记下每场的长度和龙的死活（账本里的 `watched`）。
//!
//! The book is one file a save in the mod folder (`balance-<id>.json`, the id kept in the save since 2.8.2 - named by
//! the team before, a new career with the same team took the old one's book): every round (its changes and every
//! champion's power after it) and the kept matches. Opened with the newest round the save's news has: the book's
//! rounds after it are another save's ([`open_book`]).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use mod_api_stable::{BuffV1, SimOriginKindV1, StableSim};
use serde::{Deserialize, Serialize};

use crate::meta::Meta;
use crate::objectives::{Objective, ObjectiveChange, Tracker, Watch};
use crate::patchnotes::PatchNote;
use crate::records::compare_versions;
use crate::stats::Stats;
use crate::{diag, paths};

/// The buff that carries a champion's balance in a match.
pub const BUFF: &str = "patch_meta_ai_balance";
/// Win rates inside this band (percent) are left alone.
pub const BAND: (f32, f32) = (48.0, 52.0);
/// Power changed for every win-rate point outside the band (percent of power), unless `balance_per_point` says
/// otherwise ([`Rules::per_point`]; the model expects a step to move its champion by as much, `meta::step_shift`).
/// 1.5 (a fixed `PER_POINT`) until 2.10.0 (the user, 2026-11-17: 「你要所有英雄达到整体平衡啊 不能只找超模吧」): on the
/// user's save, 2699 games of the 4+6 item era, a fit over champions, players and side with each champion's power per
/// patch as a covariate gave 0.7-1.5 log-odds per 100% of power (best about 1.0; 1.5% a point means 2.67) - the steps
/// moved their champions about 2.5 times less than the rule thought, and the strong champions stayed strong after
/// their cuts (Tryndamere at -32% and Kog'Maw at -35% still won about 60%). Some of our own kit changes came with
/// cuts and bias the fit low, so 2.5 rather than the fit's 3.85.
pub const DEFAULT_PER_POINT: f32 = 2.5;
/// A champion's power stays within this much of its own (percent). 50 since 2.9.0 (the user, 2026-11-13: 「还有max也
/// 调高点」), 80 since 2.10.0: the steps are bigger now ([`DEFAULT_PER_POINT`]), and the strongest champions were held
/// at -32% and -35% while they still won about 60%. `balance_max` goes up to it. The items keep [`ITEM_CAP`].
pub const CAP: i32 = 80;
/// An item's power stays within this much of its numbers (percent): 2.9.0's [`CAP`], kept for the items in 2.10.0.
pub const ITEM_CAP: i32 = 50;
/// Recent games (each patch back counting half, the model's `evidence`) before a champion's win rate is trusted.
pub const MIN_EVIDENCE: f32 = 30.0;
/// Picks this patch (ten a match) before they set the middle of the field ([`field_centre`]); fewer (a patch just
/// begun): the champions' recent games ([`MIN_EVIDENCE`]'s evidence) do.
pub const CENTRE_MIN_PICKS: f32 = 300.0;
/// Items (2.9.0): builders with games enough ([`MIN_EVIDENCE`]) an item needs before it is judged, at least
/// `item_balance_users` of them ([`DEFAULT_ITEM_USERS`]); each builder weighs its games, at most [`ITEM_WEIGHT`];
/// their win rate is drawn toward 50 by n / (n + [`ITEM_SHRINK`]) (a few builders say little about the item); within
/// `item_balance_band` points of 50 ([`DEFAULT_ITEM_BAND`]) it is left alone, beyond it a step of
/// `item_balance_per_point` percent of its numbers a point ([`DEFAULT_ITEM_PER_POINT`]); an item changed waits for
/// [`ITEM_MIN_GAMES`] games of its builders since.
///
/// 2.12.0 (the user, 2026-11-18: 「装备的削弱补丁有点弱啊 需要调整 每次只有那一件？」, then 「放宽规则」): 2.9.0 - 2.11.x
/// judged an item after the round's champion changes, with 4 builders, a band of 48-52 and 2% a point; on their save
/// 14 rounds changed 5 items by 1-3%, all buffs, and none from 2028.2.2 on. An item's numbers are one part of its
/// builders' power (six items, the champion's own numbers), so 2% a point hardly moved them: 3 builders, 50 +- 1 and
/// 10% a point now, the items first ([`make_round_with`]).
pub const DEFAULT_ITEM_USERS: usize = 3;
pub const ITEM_WEIGHT: f32 = 200.0;
pub const ITEM_SHRINK: f32 = 4.0;
pub const DEFAULT_ITEM_BAND: f32 = 1.0;
pub const DEFAULT_ITEM_PER_POINT: f32 = 10.0;
pub const ITEM_MIN_GAMES: f32 = 60.0;
/// Items a round changes at most and an item's step at most (percent of its numbers) unless the settings say otherwise
/// (`item_balance_count`, `item_balance_max`; 5 and 10 before 2.12.0).
pub const DEFAULT_ITEM_COUNT: usize = 8;
pub const DEFAULT_ITEM_MAX: i32 = 20;
/// A match looks at its champions' items every this many ticks (an item is bought now and then).
pub const ITEM_EVERY: usize = 10;
/// "Unpicked" buffs (2.10.0, `balance_unpicked`; the user: 「你要所有英雄达到整体平衡啊 不能只找超模吧」): a champion with
/// fewer than [`MIN_EVIDENCE`] recent games is never judged by its win rate, and the game's own patches - whose
/// "unpicked" buffs brought such champions back - are off while this mod makes them. On the user's save 131 of the
/// 212 champions had fewer than 30 games in the 4+6 era, 40 none, and no round touched them. So a round also buffs
/// some of them ([`unpicked`]) by this much, to bring them into play: once they have [`MIN_EVIDENCE`] games the
/// win-rate rule takes over, and cuts back what was too much.
pub const UNPICKED_STEP: i32 = 10;
/// Through the unpicked buffs a champion's power goes up to this much, never above (percent).
pub const UNPICKED_CAP: i32 = 30;
/// The balance rules a round is made under ([`Round::rules`]): 2 = 2.10.0's ([`DEFAULT_PER_POINT`] and
/// `balance_per_point`, [`CAP`] 80, [`unpicked`] buffs). Rounds of 2.8.0 - 2.9.0 wrote no number (0; 1 stands for
/// 2.9.0's rules). A book whose newest round is older gets a hotfix under the new ones at once
/// ([`Due::Recalibration`]).
pub const RULES_VERSION: u32 = 2;
/// Game days after a round without a new game patch before the next round comes as a hotfix of the same patch (the
/// game makes one every 4-6 weeks; with its champion changes off it might make none).
pub const LATE_DAYS: i64 = 60;
/// Matches whose round is kept (the newest).
const KEPT: usize = 4000;
/// A run waits this long for all its champions before it settles with the ones there.
const TICK_WINDOW: usize = 600;
/// Runs remembered (their decided balance).
const RUNS_KEPT: usize = 64;

/// How big a round is (settings `balance_count`, `balance_max`, `balance_per_point`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rules {
    pub count: usize,
    pub max_step: i32,
    /// Percent of power a win-rate point outside the band (2.10.0, `balance_per_point`, [`DEFAULT_PER_POINT`]): the
    /// step a champion gets, and what the items expect of it ([`Builder::after`]).
    pub per_point: f32,
    /// Unpicked buffs a round makes at most (2.10.0, `balance_unpicked`, [`UNPICKED_STEP`]; 0: none), besides
    /// `count`.
    pub unpicked: usize,
    /// The 4+6 pack's items too (2.9.0, `item_balance`), at most `item_count` a round, each a step of at most
    /// `item_max` (percent of its numbers).
    pub items: bool,
    pub item_count: usize,
    pub item_max: i32,
    /// The builders an item needs, the band around 50 it is left alone in, and its step for each point beyond it
    /// (2.12.0, `item_balance_users`, `item_balance_band`, `item_balance_per_point`; 4, 2 and 2 fixed before).
    pub item_users: usize,
    pub item_band: f32,
    pub item_per_point: f32,
    /// 防御塔和大小龙也调（2.13.0，`objective_balance`）：目标和步子；None = 不调
    pub objectives: Option<crate::objectives::Targets>,
    /// 主题版本（2.14.0，`balance_theme_every` / `balance_theme_step`）：每几个游戏版本一次（0 = 不出），一类加强多少
    pub theme_every: u32,
    pub theme_step: i32,
}

/// One champion's change in a round.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub champion: String,
    /// Its win rate against the field when the round was made (percent, [`field_win`]; the model's own before 2.9.0)
    /// and its recent games.
    pub win: f32,
    pub evidence: f32,
    /// This round's step and its power after it (percent).
    pub step: i32,
    pub total: i32,
    /// An unpicked buff (2.10.0, [`UNPICKED_STEP`]): too few games for its win rate to count (`win` is the model's
    /// guess), buffed to be played. Its reason in the notes says so ([`reason`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unpicked: bool,
    /// Its power taken off at once, and why (2.11.0): the player changed its numbers, or `balance_skip` lists it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset: Option<Reset>,
    /// 主题版本里这一类一起加强的（2.14.0，`crate::theme`）：英雄职业
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<u8>,
}

/// Why a champion's or an item's power was taken off at once (2.11.0, [`make_reset`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Reset {
    /// The player changed its numbers in the database ([`crate::edits`]): it plays theirs.
    Edit,
    /// `balance_skip` / `item_balance_skip` lists it: the balance leaves it alone.
    Skip,
}

/// Champions and items the balance leaves alone (2.11.0, `balance_skip` / `item_balance_skip`, as ids and keys):
/// no round changes them, and their power comes off.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Skip {
    pub champions: std::collections::HashSet<String>,
    pub items: std::collections::HashSet<String>,
}

/// One item's change in a round (2.9.0, `item_balance`): the 4+6 pack's key and the item's name, how many champions
/// with games build it and their win rate (percent, weighted by games), its step and its power after it (percent of
/// the item's numbers).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemChange {
    pub item: String,
    pub name: String,
    pub users: u32,
    pub win: f32,
    pub step: i32,
    pub total: i32,
    /// Its power taken off at once (2.11.0, `item_balance_skip`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset: Option<Reset>,
}

/// One balance patch.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Round {
    pub id: u32,
    /// The game's patch it belongs to.
    pub version: String,
    /// The name its notes carry: the game's patch, or a hotfix of it (`2027.4.1b`).
    #[serde(default)]
    pub label: String,
    /// Its notes go into the game's own notes of `version` (else they are patch notes of their own).
    #[serde(default)]
    pub merged: bool,
    /// The game date it was made.
    #[serde(default)]
    pub date: Option<(i32, u32, u32)>,
    /// Competition matches of the model's newest patch when it was made.
    pub matches: u32,
    pub changes: Vec<Change>,
    /// Every champion's power after it (percent; absent = 0).
    pub power: BTreeMap<String, i32>,
    /// Champions outside the band left for a later round: too few games since a round last changed them (name, games).
    /// Only for the log.
    #[serde(skip)]
    pub held: Vec<(String, f32)>,
    /// The items it changed (2.9.0), and every item's power after it (percent of its numbers; absent = 0).
    #[serde(default)]
    pub items: Vec<ItemChange>,
    #[serde(default)]
    pub item_power: BTreeMap<String, i32>,
    /// Why these champions and no more (the log: how many had games, were outside the band, waited, were at the limit).
    #[serde(skip)]
    pub summary: String,
    /// The balance rules it was made under ([`RULES_VERSION`]; 0: a round of 2.8.0 - 2.9.0, which wrote none).
    #[serde(default)]
    pub rules: u32,
    /// A round that only takes power off (2.11.0, [`make_reset`]): made at once, between the balance's own rounds -
    /// a hotfix in the notes, but not a round the late hotfix's timer or the rules' check counts ([`due`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reset: bool,
    /// 塔、小龙、大龙这一轮的改动，和改完以后各自的百分比（2.13.0）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub objectives: Vec<ObjectiveChange>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub objective_power: BTreeMap<Objective, i32>,
    /// 这一轮塔和龙量到了什么，只写日志
    #[serde(skip)]
    pub objective_summary: String,
    /// 主题版本（2.14.0）：这一轮一起加强的英雄职业
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<u8>,
}

/// The strength in the middle of the field (2.9.0): the median of the champions' strengths, each counted once for
/// every game it was picked in this patch ([`CENTRE_MIN_PICKS`] picks or more; else by its recent games). The
/// model's strength 0 is its prior's centre - a champion nobody may play - and measured from it nearly every
/// champion played is too strong (on one save the 38 champions with 30 games of a patch: 59.4% in the model, 51.8%
/// in their games). The median rather than the mean: a champion in every game does not make all the others look
/// weak. 0 without any game.
pub fn field_centre(meta: &Meta) -> f32 {
    let picks: f32 = meta.champions.iter().map(|c| c.current.games as f32).sum();
    let by_picks = picks >= CENTRE_MIN_PICKS;
    let mut field: Vec<(f32, f32)> = meta
        .champions
        .iter()
        .map(|c| (c.strength, if by_picks { c.current.games as f32 } else { c.evidence }))
        .filter(|(strength, weight)| *weight > 0.0 && strength.is_finite())
        .collect();
    field.sort_by(|a, b| a.0.total_cmp(&b.0));
    let half = field.iter().map(|(_, weight)| weight).sum::<f32>() / 2.0;
    let mut seen = 0.0;
    for (strength, weight) in &field {
        seen += weight;
        if seen >= half {
            return *strength;
        }
    }
    0.0
}

/// A champion's win rate against the field (percent): its strength measured from [`field_centre`].
pub fn field_win(champion: &crate::meta::Champion, centre: f32) -> f32 {
    crate::glm::sigmoid(champion.strength - centre) * 100.0
}

/// A champion the model rates: name, win rate against the field (percent, [`field_win`]), recent games, its games
/// since a round last changed it (None: no round did), and whether it is in this save's pool (2.10.0: picked or
/// banned in a kept game, `meta::Champion::drafted`; only those get [`unpicked`] buffs).
#[derive(Clone, Copy, Debug)]
pub struct Candidate<'a> {
    pub name: &'a str,
    pub win: f32,
    pub evidence: f32,
    pub since_change: Option<f32>,
    pub pooled: bool,
}

impl Candidate<'_> {
    /// How far outside [`BAND`] its win rate is (negative below it); None inside.
    fn gap(&self) -> Option<f32> {
        if self.win > BAND.1 {
            Some(self.win - BAND.1)
        } else if self.win < BAND.0 {
            Some(self.win - BAND.0)
        } else {
            None
        }
    }

    /// Enough games since its last change to judge it again (League waits for a patch's games too).
    fn settled(&self) -> bool {
        self.since_change.is_none_or(|g| g >= MIN_EVIDENCE)
    }
}

/// A champion's games since a round last changed it, from the model's games per patch (`versions` and the champion's
/// `history`, oldest first): a change merged into a game patch counts that patch and the later ones, a hotfix only the
/// later ones (its own patch mixes games from before and after it). None: no round changed it.
pub fn games_since_change(rounds: &[Round], versions: &[String], champion: &crate::meta::Champion) -> Option<f32> {
    let last = rounds.iter().rev().find(|r| r.changes.iter().any(|c| c.champion == champion.name))?;
    Some(games_since_round(last, versions, champion))
}

/// A champion's games since `round`: a round merged into a game patch counts that patch and the later ones, a hotfix
/// only the later ones.
fn games_since_round(round: &Round, versions: &[String], champion: &crate::meta::Champion) -> f32 {
    let games: u32 = versions
        .iter()
        .zip(&champion.history)
        .filter(|(v, _)| match compare_versions(v, &round.version) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Equal => round.merged,
            std::cmp::Ordering::Less => false,
        })
        .map(|(_, (_, tally))| tally.games)
        .sum();
    games as f32
}

/// The changes of a round: the champions furthest outside [`BAND`] with enough games (recent ones, and since their
/// last change), at most `rules.count`, each a step of `rules.per_point` a point outside the band (at most
/// `rules.max_step`), their power kept within [`CAP`].
pub fn decide(candidates: &[Candidate<'_>], before: &BTreeMap<String, i32>, rules: Rules) -> Vec<Change> {
    let mut out = qualified(candidates, before, rules);
    out.truncate(rules.count);
    out
}

/// The step a champion `gap` points outside the band wants (negative above it), before `rules.max_step` and [`CAP`].
fn wanted(gap: f32, rules: Rules) -> i32 {
    ((-(gap * rules.per_point)).round() as i32).clamp(-rules.max_step, rules.max_step)
}

/// Every change [`decide`] could make, furthest outside the band first (before `rules.count` cuts the list).
fn qualified(candidates: &[Candidate<'_>], before: &BTreeMap<String, i32>, rules: Rules) -> Vec<Change> {
    let mut out: Vec<(f32, Change)> = candidates
        .iter()
        .filter(|c| c.evidence >= MIN_EVIDENCE && c.settled())
        .filter_map(|c| {
            // above the band: a nerf; below it: a buff
            let gap = c.gap()?;
            let want = wanted(gap, rules);
            let was = before.get(c.name).copied().unwrap_or(0);
            let total = (was + want).clamp(-CAP, CAP);
            let step = total - was;
            (step != 0).then(|| {
                (gap.abs(), Change { champion: c.name.to_string(), win: c.win, evidence: c.evidence, step, total, unpicked: false, reset: None, theme: None })
            })
        })
        .collect();
    out.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.champion.cmp(&b.1.champion)));
    out.into_iter().map(|(_, c)| c).collect()
}

/// The unpicked buffs of a round (2.10.0, [`UNPICKED_STEP`]), besides `rules.count`: champions of this save's pool
/// ([`Candidate::pooled`]) with fewer than [`MIN_EVIDENCE`] recent games, at most 50% against the field and under
/// [`UNPICKED_CAP`] of power - the fewest games first, then the weakest - at most `rules.unpicked` of them. Each a step
/// of [`UNPICKED_STEP`] (at most `rules.max_step`), its power not above [`UNPICKED_CAP`] through it. No wait for games
/// since their last change: they have too few to wait for. Also how many were eligible (for the log).
pub fn unpicked(candidates: &[Candidate<'_>], before: &BTreeMap<String, i32>, rules: Rules) -> (Vec<Change>, usize) {
    let mut eligible: Vec<(&Candidate<'_>, i32)> = candidates
        .iter()
        .filter(|c| c.pooled && c.evidence < MIN_EVIDENCE && c.win <= 50.0)
        .filter_map(|c| {
            let was = before.get(c.name).copied().unwrap_or(0);
            (was < UNPICKED_CAP).then_some((c, was))
        })
        .collect();
    eligible.sort_by(|(a, _), (b, _)| {
        a.evidence.total_cmp(&b.evidence).then_with(|| a.win.total_cmp(&b.win)).then_with(|| a.name.cmp(b.name))
    });
    let n = eligible.len();
    let step = UNPICKED_STEP.min(rules.max_step);
    let out = eligible
        .into_iter()
        .take(rules.unpicked)
        .map(|(c, was)| {
            let total = (was + step).min(UNPICKED_CAP);
            Change { champion: c.name.to_string(), win: c.win, evidence: c.evidence, step: total - was, total, unpicked: true, reset: None, theme: None }
        })
        .collect();
    (out, n)
}

/// A champion as the items see it (2.9.0): its win rate against the field before this round's changes (2.12.0; the
/// rounds of 2.9.0 - 2.11.x took it once the round's own step to the champion worked), its games, and the finished
/// items its published build names.
#[derive(Clone, Debug, PartialEq)]
pub struct Builder {
    pub champion: String,
    pub win: f32,
    pub evidence: f32,
    pub items: Vec<String>,
}

/// The item changes of a round (2.9.0; first in the round since 2.12.0): for each item named by `rules.item_users`
/// builders or more with games, their win rate against the field as it is, weighted by games (at most
/// [`ITEM_WEIGHT`] each) and drawn toward 50 by n / (n + [`ITEM_SHRINK`]); beyond `rules.item_band` points of 50 a step
/// of `rules.item_per_point` a point (at most `rules.item_max`), the power kept within [`ITEM_CAP`]; an item `settled`
/// says not yet waits. Furthest first, at most `rules.item_count`. `names`: the items that can change (finished ones),
/// with their names.
pub fn decide_items(
    builders: &[Builder],
    names: &BTreeMap<String, String>,
    before: &BTreeMap<String, i32>,
    settled: &dyn Fn(&str) -> bool,
    rules: Rules,
) -> Vec<ItemChange> {
    let mut out: Vec<(f32, ItemChange)> = Vec::new();
    for (item, name) in names {
        let users: Vec<&Builder> = builders.iter().filter(|b| b.evidence >= MIN_EVIDENCE && b.items.contains(item)).collect();
        let n = users.len();
        if n < rules.item_users.max(1) {
            continue;
        }
        let weight: f32 = users.iter().map(|b| b.evidence.min(ITEM_WEIGHT)).sum();
        if weight <= 0.0 {
            continue;
        }
        let win = users.iter().map(|b| b.evidence.min(ITEM_WEIGHT) * b.win).sum::<f32>() / weight;
        let shrunk = (win - 50.0) * n as f32 / (n as f32 + ITEM_SHRINK);
        let gap = if shrunk > rules.item_band {
            shrunk - rules.item_band
        } else if shrunk < -rules.item_band {
            shrunk + rules.item_band
        } else {
            continue;
        };
        let want = ((-(gap * rules.item_per_point)).round() as i32).clamp(-rules.item_max, rules.item_max);
        let was = before.get(item).copied().unwrap_or(0);
        let total = (was + want).clamp(-ITEM_CAP, ITEM_CAP);
        let step = total - was;
        if step == 0 || !settled(item) {
            continue;
        }
        out.push((gap.abs(), ItemChange { item: item.clone(), name: name.clone(), users: n as u32, win, step, total, reset: None }));
    }
    out.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.item.cmp(&b.1.item)));
    out.truncate(rules.item_count);
    out.into_iter().map(|(_, c)| c).collect()
}

/// What a round's item steps are expected to do to the champions that build them (2.12.0), in win-rate points: an
/// item's step is chosen to close step / `rules.item_per_point` points of its builders' win rate, and a champion gets
/// that of every changed item its build names. The champions' own steps are decided on their win rate plus this
/// ([`make_round_with`]), so an item cut and its builders' cuts do not take the same points twice. Power taken off at
/// once ([`Reset`]) is no step the win rates asked for: left out.
pub fn item_effects(changes: &[ItemChange], builds: &crate::items::Builds, rules: Rules) -> BTreeMap<String, f32> {
    let mut out = BTreeMap::new();
    if rules.item_per_point <= 0.0 {
        return out;
    }
    for (champion, items) in builds {
        let effect: f32 = changes
            .iter()
            .filter(|c| c.reset.is_none() && items.contains(&c.item))
            .map(|c| c.step as f32 / rules.item_per_point)
            .sum();
        if effect != 0.0 {
            out.insert(champion.clone(), effect);
        }
    }
    out
}

/// Why a round is due (each with the game's patch it belongs to).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Due {
    /// The game made this patch: the round goes into its notes.
    Patch(String),
    /// No round yet (the setting was just turned on): a hotfix of the game's current patch.
    First(String),
    /// [`LATE_DAYS`] game days without a new game patch: a hotfix of the current one.
    Late(String),
    /// The newest round was made under older rules ([`Round::rules`] below [`RULES_VERSION`]; 2.10.0, the user:
    /// 「你要所有英雄达到整体平衡啊 不能只找超模吧」): a hotfix of the game's current patch at once (as [`Due::First`]),
    /// the new rules put to work without waiting for the game's next patch. Due once: that round is made under them.
    Recalibration(String),
}

impl Due {
    pub fn version(&self) -> &str {
        match self {
            Due::Patch(v) | Due::First(v) | Due::Late(v) | Due::Recalibration(v) => v,
        }
    }
}

/// A game date as a day number (days from 1970-01-01, proleptic Gregorian).
pub fn day_number((year, month, day): (i32, u32, u32)) -> i64 {
    let (m, d) = (i64::from(month), i64::from(day));
    let y = i64::from(year) - i64::from(m <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Whether a round is due: `patch` is the game's newest patch (from its notes in the news), `today` the game date.
pub fn due(rounds: &[Round], patch: Option<&str>, today: Option<(i32, u32, u32)>) -> Option<Due> {
    let patch = patch.map(str::trim).filter(|p| !p.is_empty())?;
    let Some(last) = rounds.last() else { return Some(Due::First(patch.to_string())) };
    // a patch newer than every round's: the game moved on
    if !rounds.iter().any(|r| r.version == patch) && compare_versions(patch, &last.version) == std::cmp::Ordering::Greater {
        return Some(Due::Patch(patch.to_string()));
    }
    // the rounds that only took power off (2.11.0) are not the balance's: the rules and the date are the newest
    // balance round's
    let last = rounds.iter().rev().find(|r| !r.reset).unwrap_or(last);
    // the newest round made under older rules (a book of 2.9.0 or before): a hotfix now, under the new ones. A game
    // patch due at the same time came first above - its round is made under the new rules and puts them to work too.
    if last.rules < RULES_VERSION {
        return Some(Due::Recalibration(patch.to_string()));
    }
    let (made, today) = (last.date?, today?);
    (day_number(today) - day_number(made) >= LATE_DAYS).then(|| Due::Late(patch.to_string()))
}

/// The name of the next hotfix of `version`: `b`, `c`, ... after the hotfixes it already had (as League's 14.1b).
pub fn hotfix_label(rounds: &[Round], version: &str) -> String {
    let n = rounds.iter().filter(|r| r.version == version && !r.merged).count();
    match u8::try_from(n).ok().filter(|n| *n < 25) {
        Some(n) => format!("{version}{}", char::from(b'b' + n)),
        None => format!("{version}-{}", n + 1),
    }
}

/// The round `due` asks for, after `rounds`, from the model's `meta`. A round that changes nobody is still a round
/// (it marks the patch as looked at).
pub fn make_round(meta: &Meta, rounds: &[Round], rules: Rules, due: &Due, today: Option<(i32, u32, u32)>) -> Round {
    make_round_with(meta, rounds, rules, due, today, None, &Skip::default())
}

/// [`make_round`] with the 4+6 pack's builds and item names (2.9.0): with `rules.items` the round changes items too
/// (first since 2.12.0, [`decide_items`]; the champions then count them, [`item_effects`]); without, it carries the
/// item power of the round before unchanged. `skip` (2.11.0): champions and items no round
/// changes; any power they still have comes off in this round ([`Reset::Skip`]).
pub fn make_round_with(
    meta: &Meta,
    rounds: &[Round],
    rules: Rules,
    due: &Due,
    today: Option<(i32, u32, u32)>,
    items: Option<&crate::items::Inputs>,
    skip: &Skip,
) -> Round {
    let last = rounds.last();
    let version = due.version().to_string();
    let merged = matches!(due, Due::Patch(_));
    let label = if merged { version.clone() } else { hotfix_label(rounds, &version) };
    let before = last.map(|r| r.power.clone()).unwrap_or_default();
    let centre = field_centre(meta);
    // the items first (2.12.0): judged on their builders' win rates as they are, before any champion's step; the
    // champions' steps then count what the items' are expected to do ([`item_effects`])
    let mut item_power = last.map(|r| r.item_power.clone()).unwrap_or_default();
    let item_changes = match items.filter(|_| rules.items) {
        Some(inputs) => {
            let builders: Vec<Builder> = meta
                .champions
                .iter()
                .filter_map(|c| {
                    let items: Vec<String> = inputs.builds.get(&c.name)?.iter().filter(|k| !skip.items.contains(*k)).cloned().collect();
                    Some(Builder { champion: c.name.clone(), win: field_win(c, centre), evidence: c.evidence, items })
                })
                .collect();
            // an item a round changed waits for its builders' games since
            let settled = |item: &str| -> bool {
                let Some(r) = rounds.iter().rev().find(|r| r.items.iter().any(|c| c.item == item)) else { return true };
                let games: f32 = meta
                    .champions
                    .iter()
                    .filter(|c| inputs.builds.get(&c.name).is_some_and(|b| b.iter().any(|k| k == item)))
                    .map(|c| games_since_round(r, &meta.versions, c))
                    .sum();
                games >= ITEM_MIN_GAMES
            };
            decide_items(&builders, &inputs.names, &item_power, &settled, rules)
        }
        None => Vec::new(),
    };
    let effects = items.filter(|_| rules.items).map(|inputs| item_effects(&item_changes, &inputs.builds, rules)).unwrap_or_default();
    let candidates: Vec<Candidate<'_>> = meta
        .champions
        .iter()
        .filter(|c| !skip.champions.contains(&c.name))
        .map(|c| Candidate {
            name: &c.name,
            win: field_win(c, centre) + effects.get(&c.name).copied().unwrap_or(0.0),
            evidence: c.evidence,
            since_change: games_since_change(rounds, &meta.versions, c),
            pooled: c.drafted > 0,
        })
        .collect();
    let could = qualified(&candidates, &before, rules);
    let mut changes: Vec<Change> = could.iter().take(rules.count).cloned().collect();
    // out of the band with games enough, but not since their last change: next round, maybe
    let mut held: Vec<(f32, String, f32)> = candidates
        .iter()
        .filter(|c| c.evidence >= MIN_EVIDENCE && !c.settled())
        .filter_map(|c| Some((c.gap()?.abs(), c.name.to_string(), c.since_change.unwrap_or(0.0))))
        .collect();
    held.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    // the unpicked buffs (2.10.0), besides balance_count: champions with too few games for the rule above
    let (fresh, eligible) = unpicked(&candidates, &before, rules);
    let summary = format!(
        "{}; {} unpicked buffs of {eligible} eligible (balance_unpicked {}: in this save's pool, under {MIN_EVIDENCE} recent games, at most 50% against the field, under +{UNPICKED_CAP}%); win rates against this patch's field: the model's {:.1}% counts as 50%",
        summarize(&candidates, &before, rules, could.len(), changes.len()),
        fresh.len(),
        rules.unpicked,
        crate::glm::sigmoid(centre) * 100.0
    );
    changes.extend(fresh);
    // the champions `balance_skip` lists: their power off (2.11.0)
    let mut skipped: Vec<(&String, &i32)> = before.iter().filter(|(c, p)| **p != 0 && skip.champions.contains(*c)).collect();
    skipped.sort();
    for (c, was) in skipped {
        changes.push(Change { champion: c.clone(), win: 0.0, evidence: 0.0, step: -was, total: 0, unpicked: false, reset: Some(Reset::Skip), theme: None });
    }
    let mut power = before;
    for c in &changes {
        if c.total == 0 {
            power.remove(&c.champion);
        } else {
            power.insert(c.champion.clone(), c.total);
        }
    }
    // the items `item_balance_skip` lists: their power off (2.11.0)
    let mut item_changes = item_changes;
    let mut skipped: Vec<(&String, &i32)> = item_power.iter().filter(|(k, p)| **p != 0 && skip.items.contains(*k)).collect();
    skipped.sort();
    for (key, was) in skipped {
        let name = items.and_then(|i| i.names.get(key)).cloned().unwrap_or_else(|| key.clone());
        item_changes.push(ItemChange { item: key.clone(), name, users: 0, win: 0.0, step: -was, total: 0, reset: Some(Reset::Skip) });
    }
    for c in &item_changes {
        if c.total == 0 {
            item_power.remove(&c.item);
        } else {
            item_power.insert(c.item.clone(), c.total);
        }
    }
    Round {
        id: last.map_or(1, |r| r.id + 1),
        version,
        label,
        merged,
        date: today,
        matches: meta.current_matches,
        changes,
        power,
        held: held.into_iter().map(|(_, name, games)| (name, games)).collect(),
        items: item_changes,
        item_power,
        summary,
        rules: RULES_VERSION,
        reset: false,
        // 塔和龙在 `make` 里判断，这里先照抄上一轮的
        objectives: Vec::new(),
        objective_power: last.map(|r| r.objective_power.clone()).unwrap_or_default(),
        objective_summary: String::new(),
        theme: None,
    }
}

/// A round that only takes power off (2.11.0), at once: the champions whose numbers the player changed
/// ([`Reset::Edit`] - listed even without power, so that the notes say the change was seen and the rounds count its
/// games from here) and the champions and items `balance_skip` / `item_balance_skip` list that still have power
/// ([`Reset::Skip`]). A hotfix of the newest round's patch. None: nothing to take off or say.
/// `objectives`（2.13.0）：玩家在数据库里改了生命值的塔和龙，和改后的基础生命；没有百分比的也写上，之后从这里重新数比赛。
pub fn make_reset(
    rounds: &[Round],
    champions: &[(String, Reset)],
    items: &[(String, String, Reset)],
    objectives: &[(Objective, i64)],
    today: Option<(i32, u32, u32)>,
) -> Option<Round> {
    let last = rounds.last()?;
    let mut power = last.power.clone();
    let mut changes = Vec::new();
    for (champion, why) in champions {
        let was = power.get(champion).copied().unwrap_or(0);
        if was == 0 && *why == Reset::Skip {
            continue;
        }
        power.remove(champion);
        changes.push(Change { champion: champion.clone(), win: 0.0, evidence: 0.0, step: -was, total: 0, unpicked: false, reset: Some(*why), theme: None });
    }
    let mut item_power = last.item_power.clone();
    let mut item_changes = Vec::new();
    for (key, name, why) in items {
        let Some(was) = item_power.remove(key).filter(|p| *p != 0) else { continue };
        item_changes.push(ItemChange { item: key.clone(), name: name.clone(), users: 0, win: 0.0, step: -was, total: 0, reset: Some(*why) });
    }
    let mut objective_power = last.objective_power.clone();
    let mut objective_changes = Vec::new();
    for &(objective, base) in objectives {
        let was = objective_power.remove(&objective).unwrap_or(0);
        objective_changes.push(ObjectiveChange {
            objective,
            measured: 0.0,
            evidence: 0,
            step: -was,
            total: 0,
            reset: Some(Reset::Edit),
            band: (0.0, 0.0),
            base,
        });
    }
    if changes.is_empty() && item_changes.is_empty() && objective_changes.is_empty() {
        return None;
    }
    Some(Round {
        id: last.id + 1,
        version: last.version.clone(),
        label: hotfix_label(rounds, &last.version),
        merged: false,
        date: today,
        matches: last.matches,
        changes,
        power,
        held: Vec::new(),
        items: item_changes,
        item_power,
        summary: String::new(),
        rules: last.rules,
        reset: true,
        objectives: objective_changes,
        objective_power,
        objective_summary: String::new(),
        theme: None,
    })
}

/// Why a round changed these champions and no more, for the log (the user, 2026-11-13, a player's 「我设置50个就砍了
/// 20个」: `balance_count` is a ceiling - a champion also needs games, a win rate outside the band, games with its
/// numbers since its last change and room under the limit).
fn summarize(candidates: &[Candidate<'_>], before: &BTreeMap<String, i32>, rules: Rules, could: usize, changed: usize) -> String {
    let rated: Vec<&Candidate<'_>> = candidates.iter().filter(|c| c.evidence >= MIN_EVIDENCE).collect();
    let outside: Vec<&&Candidate<'_>> = rated.iter().filter(|c| c.gap().is_some()).collect();
    let waiting = outside.iter().filter(|c| !c.settled()).count();
    let at_limit = outside
        .iter()
        .filter(|c| c.settled())
        .filter(|c| {
            let want = wanted(c.gap().unwrap_or(0.0), rules);
            let was = before.get(c.name).copied().unwrap_or(0);
            want != 0 && (was + want).clamp(-CAP, CAP) == was
        })
        .count();
    let close = outside.len().saturating_sub(waiting + at_limit + could);
    format!(
        "{} champions rated, {} with {MIN_EVIDENCE} games or more, {} of those outside {}-{}%: {waiting} waiting for {MIN_EVIDENCE} games with their new numbers, {at_limit} at the {CAP}% limit, {close} too close to the band for a step ({}% of power a point); {could} could change, {changed} changed (balance_count {})",
        candidates.len(),
        rated.len(),
        outside.len(),
        BAND.0,
        BAND.1,
        rules.per_point,
        rules.count
    )
}

/// The buff for a champion at `power` percent: half on damage (attack and magic power), the rest on health.
pub fn buff(power: i32) -> BuffV1 {
    let damage = power / 2;
    BuffV1 { attack_mult: damage, magic_power_mult: damage, hp_mult: power - damage, ..BuffV1::named(BUFF) }
}

/// What the buff does at `power` percent: (damage factor, health factor), as [`buff`] splits it.
pub fn factors(power: i32) -> (f64, f64) {
    let damage = power / 2;
    (1.0 + f64::from(damage) / 100.0, 1.0 + f64::from(power - damage) / 100.0)
}

/// The rounds as the model reads them (2.9.0, [`crate::meta::own_steps`]): each step's size, where its games are.
pub fn model_rounds_of(rounds: &[Round]) -> Vec<crate::meta::BalanceRound> {
    rounds
        .iter()
        .map(|r| crate::meta::BalanceRound {
            version: r.version.clone(),
            merged: r.merged,
            matches: r.matches,
            steps: r.changes.iter().map(|c| (c.champion.clone(), c.step)).collect(),
        })
        .collect()
}

/// This save's rounds for the model ([`model_rounds_of`]).
pub fn model_rounds() -> Vec<crate::meta::BalanceRound> {
    model_rounds_of(&rounds())
}

/// The patch notes of the rounds for the model: each round gives each champion it changed its direction (+1
/// buffed, -1 nerfed) in its patch, as the game's own notes do (the client adds up the notes of a patch). Since
/// 2.9.0 the model takes the steps' size from [`model_rounds`] and these lines give no direction of their own.
pub fn notes_of(rounds: &[Round]) -> Vec<PatchNote> {
    rounds
        .iter()
        .map(|r| PatchNote {
            versions: vec![r.version.clone()],
            version: Some(r.version.clone()),
            changes: r.changes.iter().map(|c| (c.champion.clone(), c.step.signum())).collect(),
            ..PatchNote::default()
        })
        .collect()
}

// ===================== the notes as the game writes them =====================

/// The news' author: the patch notes and the patch cards about the league. The mod's Chinese name is 电竞大脑 since
/// 2.8.1 (the user: 「名字也可以改改了吧 现在不是只有BP功能了吧 全方位功能都有了」).
pub const AUTHOR: &str = "电竞大脑 / Patch Meta AI";
/// The author the mod's news had under its old name (2.8.0).
pub const OLD_AUTHORS: [&str; 1] = ["智能BP / Patch Meta AI"];

/// Whether a news item's author is this mod, under its name or an old one.
pub fn is_ours(author: &str) -> bool {
    author == AUTHOR || OLD_AUTHORS.contains(&author)
}

/// One line of a champion's notes as the game writes them (`ChampionPatchData`, in its JSON field order).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub key: String,
    pub target_action: Option<String>,
    pub pre_value: String,
    pub after_value: String,
    pub is_buff: bool,
}

/// The lines a change shows: (the game's text for it, stat index in [`Stats`], health (else damage), growth).
const ROWS: [(&str, usize, bool, bool); 6] = [
    ("#asset/base/text/champion?stat.attack", 0, false, false),
    ("#asset/base/text/champion?stat.magic_power", 1, false, false),
    ("#asset/base/text/champion?stat.hp", 2, true, false),
    ("#asset/base/text/champion?patch_key.growth_attack", 0, false, true),
    ("#asset/base/text/champion?patch_key.growth_magic_power", 1, false, true),
    ("#asset/base/text/champion?patch_key.growth_hp", 2, true, true),
];

/// A number as the game's notes write it: whole when it is, else one decimal.
pub fn number(x: f64) -> String {
    if (x - x.round()).abs() < 0.05 {
        format!("{}", x.round() as i64)
    } else {
        format!("{x:.1}")
    }
}

/// A champion's lines for a change from `before` to `after` percent of power: its base attack, magic power and
/// health and their growth per level as it plays with them (the game's numbers times the buff). Numbers the champion
/// does not have (no magic power) or that do not move are left out.
pub fn rows(stats: &Stats, before: i32, after: i32) -> Vec<Row> {
    let (d0, h0) = factors(before);
    let (d1, h1) = factors(after);
    let mut out = Vec::new();
    for (key, i, health, growth) in ROWS {
        let value = f64::from(if growth { stats.growth[i] } else { stats.base[i] });
        if value <= 0.0 {
            continue;
        }
        let (f0, f1) = if health { (h0, h1) } else { (d0, d1) };
        let (pre, after) = (number(value * f0), number(value * f1));
        if pre == after {
            continue;
        }
        out.push(Row { key: key.to_string(), target_action: None, pre_value: pre, after_value: after, is_buff: f1 > f0 });
    }
    out
}

/// 挑官方口吻理由用的英雄定位（2.14.2）
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    /// 打输出的（大多数英雄）
    #[default]
    Damage,
    /// 前排、坦克
    Tank,
    /// 奶妈、保护型辅助
    Healer,
    /// 后期大核
    Late,
}

/// 一个英雄的定位：有英雄联盟的阵容画像就先看画像（前排、保护、后期），再看游戏的职业（坦克；没有画像的辅助算奶妈）
pub fn kind_of(profile: Option<&crate::knowledge::Profile>, category: Option<u8>) -> Kind {
    if let Some(p) = profile {
        if p.front >= 1.5 && p.carry < 1.0 {
            return Kind::Tank;
        }
        if p.peel >= 1.5 && p.front < 1.5 && p.carry < 1.0 {
            return Kind::Healer;
        }
        if p.carry >= 1.5 {
            return Kind::Late;
        }
    }
    match category {
        Some(5) => Kind::Tank,
        Some(3) if profile.is_none() => Kind::Healer,
        _ => Kind::Damage,
    }
}

/// 官方口吻的理由要看的（2.14.2）：定位；上一次调它是不是就在前两轮、方向也一样（连着调）；上一次是不是也是冷门加强；
/// 挑第几种说法的种子
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flavor {
    pub kind: Kind,
    pub streak: bool,
    pub unpicked_before: bool,
    pub seed: u64,
}

/// 这一轮（`round_id`）里一个改动的 [`Flavor`]：看 `rounds` 里在它之前的轮次
pub fn flavor_of(c: &Change, round_id: u32, rounds: &[Round], kind: Kind) -> Flavor {
    let before = rounds
        .iter()
        .filter(|r| r.id < round_id && !r.reset)
        .rev()
        .find_map(|r| r.changes.iter().find(|x| x.champion == c.champion && x.reset.is_none()).map(|x| (r.id, x)));
    let (streak, unpicked_before) = match before {
        Some((id, x)) if round_id.saturating_sub(id) <= 2 => (x.step != 0 && x.step.signum() == c.step.signum(), x.unpicked && c.unpicked),
        _ => (false, false),
    };
    Flavor { kind, streak, unpicked_before, seed: seed(&c.champion, &round_id.to_string()) }
}

/// 挑说法用的数（FNV-1a）：同一个东西在同一轮总是同一句，换一轮换一句
pub fn seed(name: &str, round: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.bytes().chain([b'|']).chain(round.bytes()) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// 游戏自己的补丁理由（`patch.reason.<code>`，游戏里 17 种语言都有）：按方向、定位、是不是连着调，挑一句。只挑和电竞大脑
/// 知道的事对得上的：连着削的不用「长期占据ban/pick榜首」（nerf_meta_2，禁选率没统计），连着加强的不用「无人问津」（buff_meta_1）
pub fn official_reason(c: &Change, f: &Flavor) -> String {
    let v = 1 + f.seed % 3;
    let base = if c.step > 0 {
        if c.unpicked {
            if f.unpicked_before {
                return "unpicked_long".to_string();
            }
            "unpicked"
        } else if f.streak {
            return format!("buff_meta_{}", [2, 3][(f.seed % 2) as usize]);
        } else {
            match f.kind {
                Kind::Tank => "too_fragile",
                Kind::Healer => "low_healing",
                Kind::Late => "weak_late_game",
                Kind::Damage => "low_damage",
            }
        }
    } else if f.streak {
        return format!("nerf_meta_{}", [1, 3][(f.seed % 2) as usize]);
    } else {
        match f.kind {
            Kind::Tank => "too_tanky",
            Kind::Healer => "too_much_healing",
            Kind::Late => "late_game_scaling",
            Kind::Damage => "too_much_damage",
        }
    };
    format!("{base}_{v}")
}

/// A change's reason as the game's notes give one: codes of `patch.reason.*` texts, comma-separated, a line each on the
/// page. 2.14.2（用户：「现在英雄平衡补丁的文字说明太僵硬 没有之前官方的那种文字的味道」）：先是游戏自己的一句
/// （[`official_reason`]），再是一行短的胜率（`pma_wr_<n>`，往目标区间外取整）或场次（冷门加强 `pma_unpicked`）；整体强度
/// 改成和属性一样的一行（[`power_row`]），所以强度和上限的代码（`pma_power_*`、`pma_sum_*`、`pma_cap`、`pma_total_*`）只留给
/// 以前写下的补丁。撤回调整（2.11.0）写为什么（`pma_custom`、`pma_skip`），主题版本（2.14.0）写这一类。每条都带一个 `pma_`
/// 代码：`patchnotes` 靠它认出电竞大脑写的英雄。
pub fn reason(c: &Change, flavor: &Flavor) -> String {
    if let Some(why) = c.reset {
        return match why {
            Reset::Edit => "pma_custom",
            Reset::Skip => "pma_skip",
        }
        .to_string();
    }
    if let Some(category) = c.theme {
        return format!("pma_theme_{category}");
    }
    let official = official_reason(c, flavor);
    if c.unpicked {
        return format!("{official},pma_unpicked");
    }
    let n = if c.step < 0 { c.win.ceil() } else { c.win.floor() }.clamp(0.0, 100.0) as u32;
    format!("{official},pma_wr_{n}")
}

/// A signed percent in a text code: `m11` for -11, `p9` for +9 (`pma_power_*`, `pma_sum_*`).
pub fn signed_code(v: i32) -> String {
    format!("{}{}", if v < 0 { "m" } else { "p" }, v.unsigned_abs())
}

/// 整体强度那一行的名字（`patch.pma_power_stat`，2.14.2）
pub const POWER_KEY: &str = "#asset/base/text/news?patch.pma_power_stat";

/// 一个改动的整体强度，和属性一样写成补丁里的一行（2.14.2）：这一版之前和之后的累计强度。以前写在理由里（`pma_power_*`）
pub fn power_row(c: &Change) -> Option<Row> {
    (c.step != 0).then(|| Row {
        key: POWER_KEY.to_string(),
        target_action: None,
        pre_value: percent(c.total - c.step),
        after_value: percent(c.total),
        is_buff: c.step > 0,
    })
}

/// 带正负号的百分数（0 不带）
pub fn percent(v: i32) -> String {
    if v == 0 {
        "0%".to_string()
    } else {
        format!("{v:+}%")
    }
}

/// 装备改动的理由（2.14.2，照官方英雄补丁的口吻）：按方向挑一句，带上出它的英雄的胜率；`seed` 挑第几句（[`seed`]）。
/// 2.15.4（用户：「文案太过于僵硬」）：以前三句轮着用，每句都拿「出它的英雄胜率约 X%」开头，同一条新闻里好几件装备说
/// 一模一样的话。现在按装备的定位（[`ItemRole`]，看它给的属性）说它该干什么、这次怎么调，幅度看这一刀多大（小幅 / 适当 /
/// 大幅）；之前削多了又加回来、加多了又削回去的，直说是回调；胜率放到句尾的括号里。只说电竞大脑知道的事：胜率、调了多少、
/// 之前调过什么、装备给什么属性。
pub fn item_reason(c: &ItemChange, seed: u64, chinese: bool) -> String {
    if c.reset.is_some() {
        return if chinese {
            "按你的设置（item_balance_skip）撤回电竞大脑之前的调整，恢复它原本的数值。".to_string()
        } else {
            "item_balance_skip lists it: this mod's earlier changes are taken off and its own numbers return.".to_string()
        };
    }
    let buff = c.step > 0;
    let before = c.total - c.step;
    let size = c.step.unsigned_abs();
    let k = (seed % 2) as usize;
    let (line, adv) = if chinese {
        let adv = match (buff, size) {
            (true, 0..=5) => "小幅",
            (false, 0..=5) => "略微",
            (_, 6..=12) => "适当",
            _ => "大幅",
        };
        let line = if buff && before < 0 {
            ["之前对它的削弱下手重了，这次把一部分数值还给它。", "上次削弱之后它一直没缓过来，这次{adv}回调一部分数值。"][k]
        } else if !buff && before > 0 {
            ["之前给它的加强有些过头，这次收回一部分。", "加强之后它变得过于强势，这次{adv}回调一部分数值。"][k]
        } else {
            ITEM_REASONS_ZH[role_of(&c.item) as usize][usize::from(!buff)][k]
        };
        (line, adv)
    } else {
        let adv = match size {
            0..=5 => "a little",
            6..=12 => "moderately",
            _ => "substantially",
        };
        let line = if buff && before < 0 {
            ["The earlier nerf went too far; part of it is given back.", "It never recovered from the last nerf; some of its numbers come back {adv}."][k]
        } else if !buff && before > 0 {
            ["The earlier buff went too far; part of it is taken back.", "It got too strong after its buff; some of it is taken back {adv}."][k]
        } else {
            ITEM_REASONS_EN[role_of(&c.item) as usize][usize::from(!buff)][k]
        };
        (line, adv)
    };
    let line = line.replace("{adv}", adv);
    if chinese {
        format!("{line}（出装胜率 {:.0}%）", c.win)
    } else {
        format!("{line} (builders win {:.0}%)", c.win)
    }
}

/// 装备的定位（2.15.4，理由按定位挑说法）：看 4+6 装备表里它给的属性
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemRole {
    /// 法强、法穿
    Mage = 0,
    /// 法强加上生命或者双抗（中娅沙漏这种）
    MageGuard = 1,
    /// 攻击加上攻速、暴击、射程或者普攻按生命打的伤害
    Marksman = 2,
    /// 攻击加上穿甲、吸血之类，没有防御也没有攻速暴击
    Assassin = 3,
    /// 攻击加上生命或者双抗
    Fighter = 4,
    /// 只有生命、双抗、减伤
    Tank = 5,
    /// 别的（急速、移速、法强和攻击都给的……）
    Other = 6,
}

/// 按装备给的属性（[`crate::items::STATS`] 的字段名）定位
pub fn item_role(fields: &[&str]) -> ItemRole {
    let any = |names: &[&str]| fields.iter().any(|f| names.contains(f));
    let ap = any(&["magic_power", "magic_power_mult", "magic_resistance_penetration"]);
    let ad = any(&["attack", "attack_mult", "attack_speed_mult", "crit_chance", "defence_penetration", "vamp", "base_attack_enemy_max_hp_damage"]);
    let guard = any(&[
        "hp",
        "hp_mult",
        "defence",
        "defence_mult",
        "magic_resistance",
        "magic_resistance_mult",
        "damaged_reduce",
        "base_attack_damaged_reduce",
        "skill_damaged_reduce",
        "toughness",
        "damage_reflect",
    ]);
    let shooter = any(&["attack_speed_mult", "crit_chance", "range", "base_attack_enemy_max_hp_damage"]);
    match (ap, ad, guard) {
        (true, false, true) => ItemRole::MageGuard,
        (true, false, false) => ItemRole::Mage,
        (false, true, true) => ItemRole::Fighter,
        (false, true, false) if shooter => ItemRole::Marksman,
        (false, true, false) => ItemRole::Assassin,
        (false, false, true) => ItemRole::Tank,
        _ => ItemRole::Other,
    }
}

/// 装备表里这件装备的定位（没有装备表、表里没有它：[`ItemRole::Other`]）
fn role_of(item: &str) -> ItemRole {
    crate::items::catalog()
        .and_then(|cat| cat.item(item).map(|(_, it)| it.stats.iter().map(|(i, _)| crate::items::STATS[*i].0).collect::<Vec<_>>()))
        .map_or(ItemRole::Other, |fields| item_role(&fields))
}

/// 每种定位 [加强, 削弱] 各两句（`{adv}`：小幅 / 略微 / 适当 / 大幅）
const ITEM_REASONS_ZH: [[[&str; 2]; 2]; 7] = [
    // 法师
    [
        ["作为法师装，它眼下的收益撑不起这个定位。我们{adv}提高了它的数值，让法师出它时更有底气。", "法师们带着它很难打出应有的效果，这次{adv}补强它的数值。"],
        ["它给法师的收益有些超出预期，我们{adv}下调了它的数值，给其他法师装留出空间。", "带着它的法师赢得太多了，这次{adv}削减它的数值。"],
    ],
    // 法师的防御装
    [
        ["它本该在团战里护住法师，但现在的数值不太够用，这次{adv}提高它的属性。", "这件攻守兼备的法师装最近表现平平，我们{adv}补强了它，让它在攻守之间更有价值。"],
        ["它在攻守两端给法师的收益都偏高，这次{adv}下调它的数值。", "这件法师装让持有者在团战中占了太多便宜，我们{adv}降低了它的数值。"],
    ],
    // 射手
    [
        ["它给射手带来的输出提升不够明显，我们{adv}提高了它的数值，让它在后期团战里更有分量。", "射手们出了它之后的表现不尽如人意，这次{adv}补强它的数值。"],
        ["它让射手的输出提升过于明显，我们{adv}下调了它的数值。", "射手们出了它之后赢得太轻松了，这次{adv}收一收它的数值。"],
    ],
    // 刺客 / 物理爆发
    [
        ["它眼下的爆发收益偏低，我们{adv}提高了它的数值，让出它的物理输出更有威胁。", "刺客们带着它很难打出足够的伤害，这次{adv}补强它的数值。"],
        ["它带来的爆发有些过头，我们{adv}下调了它的数值。", "带着它的物理输出赢得太多了，这次{adv}削减它的数值。"],
    ],
    // 战士
    [
        ["它本该让战士既能打又能扛，但现在两头都差一点，这次{adv}补强。", "带着它的战士在正面交战中有些乏力，我们{adv}提高了它的数值。"],
        ["它让战士在正面交战中占了太多便宜，这次{adv}削弱它的数值。", "带着它的战士又能打又能扛，有些过头了，我们{adv}下调了它的数值。"],
    ],
    // 坦克
    [
        ["它给前排的承伤能力不够用，我们{adv}提高了它的数值，让坦克能在团战里站得更久。", "出它的前排最近很难顶住压力，这次{adv}加强它的防御属性。"],
        ["它让前排太过耐打，这次{adv}降低它的防御属性。", "它给前排的收益偏高，我们{adv}削减了它的数值，让输出位也有机会打穿前排。"],
    ],
    // 别的
    [
        ["它最近的表现低于预期，我们{adv}提高了它的数值，希望它能在对局中发挥更大的作用。", "和同类装备相比，它眼下的收益偏低，这次{adv}提高它的数值。"],
        ["它目前的收益超出了预期，我们{adv}削减了它的数值，让其他装备也有出场的机会。", "带着它的对局赢得太多了，这次给它降降温。"],
    ],
];

/// [`ITEM_REASONS_ZH`] in English (`{adv}`: a little / moderately / substantially)
const ITEM_REASONS_EN: [[[&str; 2]; 2]; 7] = [
    [
        ["As a mage item it isn't pulling its weight; its numbers go up {adv} so mages can build it with confidence.", "Mages struggle to get much out of it lately; its numbers go up {adv}."],
        ["It gives mages more than intended; its numbers come down {adv} to leave room for other mage items.", "Mages who build it win too often; its numbers come down {adv}."],
    ],
    [
        ["It should keep mages alive in fights, but its numbers fall short; they go up {adv}.", "This mage item with defense has been underwhelming; its numbers go up {adv}."],
        ["It gives mages too much on both offense and defense; its numbers come down {adv}.", "It lets its holder get away with too much in fights; its numbers come down {adv}."],
    ],
    [
        ["It adds too little to a marksman's damage; its numbers go up {adv} so it matters more in late fights.", "Marksmen who build it have underperformed; its numbers go up {adv}."],
        ["It boosts a marksman's damage too much; its numbers come down {adv}.", "Marksmen win too easily with it; its numbers come down {adv}."],
    ],
    [
        ["Its burst is a little lacking; its numbers go up {adv} to make its builders more threatening.", "Assassins struggle to make it work; its numbers go up {adv}."],
        ["Its burst is a bit too much; its numbers come down {adv}.", "Assassins who build it win too often; its numbers come down {adv}."],
    ],
    [
        ["It should let fighters both deal and take damage, but falls short at both; its numbers go up {adv}.", "Fighters who build it run out of steam in head-on fights; its numbers go up {adv}."],
        ["It lets fighters get too much out of head-on fights; its numbers come down {adv}.", "Fighters who build it both deal and take too much; its numbers come down {adv}."],
    ],
    [
        ["It doesn't give front-liners enough to hold the line; its numbers go up {adv}.", "Tanks who build it struggle to absorb pressure; its defensive numbers go up {adv}."],
        ["It makes front-liners too hard to take down; its defensive numbers come down {adv}.", "It gives tanks more than intended; its numbers come down {adv} so damage dealers can break through."],
    ],
    [
        ["It has been underperforming; its numbers go up {adv} so it can do more in a game.", "Compared with similar items it gives too little right now; its numbers go up {adv}."],
        ["It gives more than intended; its numbers come down {adv} so other items get a look.", "Games with it go too well; it gets cooled down a bit."],
    ],
];

/// 新闻和百科里的版式（2.14.2，照官方英雄补丁）：粗体、补丁里的箭头（符号字体）
pub const BOLD: &str = "<f#asset/base/font/set/bold>";
pub const FONT_END: &str = "<f>";
pub const ARROW: &str = "<f#asset/base/font/set/symbol>→<f>";
/// 收件箱的新闻是浅色底：加强、削弱的数字和理由的颜色（游戏浅色主题的正负色，理由用官方理由行的灰）
const NEWS_UP: &str = "#0b7f58ff";
const NEWS_DOWN: &str = "#b8540bff";
const NEWS_REASON: &str = "#667085ff";

/// 一件装备的一行数值：图标（4+6 的属性图标，可以没有）、名字、改前、改后
pub type ItemLine = (String, String, String, String);

/// The round's item changes as news of their own (2.9.0; None: it changed no item). 2.14.2（用户：「装备的平衡补丁现在
/// 文字显示排版很差 而且数值在局内能改了 你还写上去 排版参考官方英雄补丁那种排版」）：照官方英雄补丁排版——先「增强」
/// 再「削弱」，每件装备一段：图标和名字（粗体）、一句理由（灰）、一行一个数值「攻击力: 75 → 69」（改后的数字带颜色）、
/// 整体强度那一行。不再说「装备说明里显示的仍是原来的数值」（2.12.1 起百科和战术界面的装备说明就是改后的数值）。
/// `lines`：一件装备改前改后的数值（[`crate::items::lines`]，带属性图标），`icon`：它的图标文字（[`crate::items::icon_text`]）。
pub fn item_news(
    round: &Round,
    chinese: bool,
    lines: &dyn Fn(&ItemChange) -> Vec<ItemLine>,
    icon: &dyn Fn(&ItemChange) -> String,
) -> Option<(String, String)> {
    if round.items.is_empty() {
        return None;
    }
    let name = if round.label.is_empty() { round.version.as_str() } else { round.label.as_str() };
    let title = if chinese { format!("电竞大脑装备平衡 {name}") } else { format!("Patch Meta AI item balance {name}") };
    let mut out = vec![if chinese {
        "这一版调整了以下装备的数值，装备特效保持不变。".to_string()
    } else {
        "This patch changes the numbers of the items below; their effects stay the same.".to_string()
    }];
    let power_name = if chinese { "整体强度" } else { "Overall power" };
    for buffs in [true, false] {
        let mut items: Vec<&ItemChange> = round.items.iter().filter(|c| (c.step > 0) == buffs).collect();
        if items.is_empty() {
            continue;
        }
        items.sort_by_key(|c| -c.step.abs());
        out.push(String::new());
        out.push(format!(
            "{BOLD}{}{FONT_END}",
            match (buffs, chinese) {
                (true, true) => "增强",
                (false, true) => "削弱",
                (true, false) => "Buffs",
                (false, false) => "Nerfs",
            }
        ));
        let colour = if buffs { NEWS_UP } else { NEWS_DOWN };
        for c in items {
            out.push(String::new());
            out.push(format!("{}{BOLD}{}{FONT_END}", icon(c), c.name));
            out.push(format!("<{NEWS_REASON}>{}<>", item_reason(c, seed(&c.item, name), chinese)));
            for (stat_icon, label, from, to) in lines(c) {
                out.push(format!("{stat_icon}{label}: {from} {ARROW} <{colour}>{to}<>"));
            }
            out.push(format!("{power_name}: {} {ARROW} <{colour}>{}<>", percent(c.total - c.step), percent(c.total)));
        }
    }
    Some((title, out.join("\n")))
}

/// A news card as the game writes them: a `Simple` item with text references and their binds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Article {
    pub title: String,
    pub title_bind: Vec<(String, String)>,
    pub content: String,
    pub content_bind: Vec<(String, String)>,
    pub author: String,
    /// What the news says should the game not take the card's form.
    pub plain_title: String,
    pub plain_content: String,
}

/// An athlete of the player's team and their signature champion (the one they are most proficient with).
#[derive(Clone, Debug, PartialEq)]
pub struct Signature {
    pub athlete: u32,
    pub name: String,
    pub champion: String,
}

/// The patch's news cards, as the game writes them after its own patches (the same texts and binds): the most
/// buffed champion (`PC01`), the most nerfed (`PC02`), a buffed signature champion of the player's athletes (`PC05`)
/// and a nerfed one (`PC06`, an alert). `lines` has each changed champion's lines and reason, `label` names
/// champions for the plain fallback.
pub fn articles(
    round: &Round,
    lines: &BTreeMap<String, (Vec<Row>, String)>,
    team: &str,
    signatures: &[Signature],
    label: &dyn Fn(&str) -> String,
) -> Vec<Article> {
    // a round that only took power off (2.11.0) is no patch to write about
    if round.reset {
        return Vec::new();
    }
    let date = round.date.map(|(y, m, d)| format!("{y:04}-{m:02}-{d:02}")).unwrap_or_default();
    let chinese = round.changes.iter().any(|c| label(&c.champion).chars().any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch)));
    let changes_of = |champion: &str| -> Option<(String, String)> {
        let (rows, reason) = lines.get(champion)?;
        (!rows.is_empty()).then(|| (serde_json::to_string(rows).unwrap_or_default(), reason.clone()))
    };
    let bind = |k: &str, v: &str| (k.to_string(), v.to_string());
    let mut out = Vec::new();
    // the most buffed and the most nerfed champion of the round
    let best = round.changes.iter().filter(|c| c.step > 0).max_by_key(|c| c.step);
    let worst = round.changes.iter().filter(|c| c.step < 0).min_by_key(|c| c.step);
    for (c, code, word) in [(best, "PC01", "buff"), (worst, "PC02", "nerf")] {
        let Some(c) = c else { continue };
        let Some((rows, reason)) = changes_of(&c.champion) else { continue };
        let binds = vec![bind("Champion", &c.champion), bind("PatchChanges", &rows), bind("PatchReason", &reason), bind("Team", team)];
        let (plain_title, plain_content) = plain(chinese, word == "buff", &label(&c.champion), None);
        out.push(card(code, "article", format!("patch:{date}:{word}:{}", c.champion), &date, binds, AUTHOR, plain_title, plain_content));
    }
    // the player's athletes whose signature champion moved most, each way
    let step_of = |s: &Signature| round.changes.iter().find(|c| c.champion == s.champion).map(|c| c.step);
    let boosted = signatures.iter().filter(|s| step_of(s).is_some_and(|v| v > 0)).max_by_key(|s| step_of(s));
    let hit = signatures.iter().filter(|s| step_of(s).is_some_and(|v| v < 0)).min_by_key(|s| step_of(s));
    for (s, code, kind, word) in [(boosted, "PC05", "article", "boost"), (hit, "PC06", "alert", "risk")] {
        let Some(s) = s else { continue };
        let Some((rows, reason)) = changes_of(&s.champion) else { continue };
        let binds = vec![
            bind("Team", team),
            bind("Athlete", &s.name),
            bind("Champion", &s.champion),
            bind("PatchChanges", &rows),
            bind("PatchReason", &reason),
        ];
        let (plain_title, plain_content) = plain(chinese, word == "boost", &label(&s.champion), Some(&s.name));
        let subject = format!("patch:{date}:{word}:{}:{}", s.athlete, s.champion);
        out.push(card(code, kind, subject, &date, binds, team, plain_title, plain_content));
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn card(code: &str, kind: &str, subject: String, date: &str, binds: Vec<(String, String)>, author: &str, plain_title: String, plain_content: String) -> Article {
    let key = code.to_lowercase();
    let mut content_bind = binds.clone();
    for (k, v) in [("ContentId", code), ("SubjectId", subject.as_str()), ("DateKey", date), ("Scope", "patch"), ("Severity", kind)] {
        content_bind.push((k.to_string(), v.to_string()));
    }
    Article {
        title: format!("#asset/base/text/news?{kind}.meta.{key}.title.0"),
        title_bind: binds,
        content: format!("#asset/base/text/news?{kind}.meta.{key}.content.0"),
        content_bind,
        author: author.to_string(),
        plain_title,
        plain_content,
    }
}

/// A card's plain fallback (the game did not take the card's form).
fn plain(chinese: bool, buffed: bool, champion: &str, athlete: Option<&str>) -> (String, String) {
    match (chinese, athlete) {
        (true, None) => {
            let word = if buffed { "加强" } else { "削弱" };
            (format!("{champion}在本次补丁中被{word}"), format!("电竞大脑的平衡补丁{word}了{champion}。"))
        }
        (true, Some(a)) => {
            let word = if buffed { "加强" } else { "削弱" };
            (format!("{a}选手的本命英雄{champion}被{word}"), format!("电竞大脑的平衡补丁{word}了{a}选手的本命英雄{champion}。"))
        }
        (false, None) => {
            let word = if buffed { "buffed" } else { "nerfed" };
            (format!("{champion} {word} this patch"), format!("Patch Meta AI's balance patch {word} {champion}."))
        }
        (false, Some(a)) => {
            let word = if buffed { "buffed" } else { "nerfed" };
            (format!("{a}'s signature {champion} {word}"), format!("Patch Meta AI's balance patch {word} {champion}, {a}'s signature champion."))
        }
    }
}

/// The round's notes as plain news (what the inbox shows should the game not take the patch-notes form): a title
/// and its lines, champions by `label` (the game's names), in Chinese when those are.
pub fn news(round: &Round, label: &dyn Fn(&str) -> String) -> (String, String) {
    let chinese = round.changes.iter().any(|c| label(&c.champion).chars().any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch)));
    let name = if round.label.is_empty() { round.version.as_str() } else { round.label.as_str() };
    let title = if chinese { format!("电竞大脑平衡补丁 {name}") } else { format!("Patch Meta AI balance patch {name}") };
    let mut lines = Vec::new();
    if let Some(category) = round.theme {
        let step = round.changes.iter().find(|c| c.theme.is_some()).map_or(0, |c| c.step);
        lines.push(if chinese {
            format!("本版本是主题版本「{}版本」：所有{}一起加强（最多 {step:+}%），之后照常按胜率调整。", crate::theme::name(category, true), crate::theme::name(category, true))
        } else {
            format!("A theme patch - {}: every one of them is buffed (up to {step:+}%); later patches judge them by win rate as usual.", crate::theme::name(category, false))
        });
    }
    if round.changes.is_empty() {
        lines.push(if chinese {
            "这一轮没有英雄胜率超出 48%–52%（场次够的英雄里），不改动。".to_string()
        } else {
            "No champion with enough games is outside 48-52% win rate: no changes.".to_string()
        });
    }
    for (title_cn, title_en, nerf) in [("削弱", "Nerfs", true), ("加强", "Buffs", false)] {
        let list: Vec<&Change> = round.changes.iter().filter(|c| (c.step < 0) == nerf).collect();
        if list.is_empty() {
            continue;
        }
        lines.push(if chinese { format!("【{title_cn}】") } else { format!("[{title_en}]") });
        for c in list {
            let damage = c.step / 2;
            let health = c.step - damage;
            // why: its win rate, or (an unpicked buff, 2.10.0) its few games
            let why = match (chinese, c.unpicked, c.theme) {
                (true, _, Some(cat)) => format!("主题版本（{}）", crate::theme::name(cat, true)),
                (false, _, Some(cat)) => format!("theme patch ({})", crate::theme::name(cat, false)),
                (true, false, None) => format!("胜率 {:.1}%（目标 48%–52%）", c.win),
                (true, true, None) => format!("近几个版本很少有人选（近期 {:.0} 场）", c.evidence),
                (false, false, None) => format!("win rate {:.1}% (target 48-52%)", c.win),
                (false, true, None) => format!("rarely picked lately ({:.0} recent games)", c.evidence),
            };
            lines.push(if chinese {
                format!(
                    "{}：{why}→ 强度 {:+}%（攻击/法强 {:+}%，生命 {:+}%），累计 {:+}%",
                    label(&c.champion),
                    c.step,
                    damage,
                    health,
                    c.total
                )
            } else {
                format!(
                    "{}: {why} -> power {:+}% (attack / magic power {:+}%, health {:+}%), {:+}% in all",
                    label(&c.champion),
                    c.step,
                    damage,
                    health,
                    c.total
                )
            });
        }
    }
    (title, lines.join("\n"))
}

// ===================== the book (one save) =====================

#[derive(Default, Serialize, Deserialize)]
struct Book {
    v: u32,
    rounds: Vec<Round>,
    /// (match key, round id; 0 = none) - the newest last.
    kept: Vec<(String, u32)>,
    /// Kept matches whose server run had the item balance off (2.9.0): every run of them plays no item change.
    #[serde(default)]
    items_off: Vec<String>,
    /// Every champion's numbers when last read, and its definition then (2.11.0, [`crate::edits`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    seen: BTreeMap<String, crate::edits::Seen>,
    /// 每场比赛服务端那一份看到的塔和龙（2.13.0，比赛编号和记录），最新的在后面
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    watched: Vec<(String, Watch)>,
    /// 服务端那一份关着 objective_balance 的比赛：之后每一份都不改塔和龙的血
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    objectives_off: Vec<String>,
    /// 塔和龙的基础生命上次读到的数，和那时的版本（玩家在数据库里改了没有）
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    seen_objectives: BTreeMap<Objective, (i64, String)>,
}

#[derive(Default)]
struct Store {
    /// This save's file (None: no save open).
    file: Option<PathBuf>,
    rounds: Vec<Round>,
    /// The book's rounds after the open save (made after it was saved: a later save of the career has them in its
    /// news). Not played here; kept in the file until this save makes a round of its own.
    later: Vec<Round>,
    /// The newest round the save's news had when it was opened: a new round's id comes after it (and after the
    /// book's own - they are the same unless the book was lost).
    done: u32,
    /// Each round's power, shared with the runs; and its items' (2.9.0).
    powers: HashMap<u32, Arc<BTreeMap<String, i32>>>,
    item_powers: HashMap<u32, Arc<BTreeMap<String, i32>>>,
    /// Kept matches played without the item balance ([`Book::items_off`]).
    items_off: std::collections::HashSet<String>,
    kept: HashMap<String, u32>,
    order: VecDeque<String>,
    /// Kept matches not written yet, and when the book was last written (a day plays many matches: the book is
    /// written at most every [`WRITE_EVERY`], and at once for a new round or a closed save).
    unwritten: bool,
    written_at: Option<std::time::Instant>,
    /// The newest round as the screens show it.
    view: Option<Arc<View>>,
    /// The champions' numbers when last read ([`Book::seen`]).
    seen: BTreeMap<String, crate::edits::Seen>,
    /// 2.13.0：每轮塔和龙的百分比，每场看到的，关着打的比赛，基础生命上次读到的
    objective_powers: HashMap<u32, Arc<BTreeMap<Objective, i32>>>,
    watched: VecDeque<(String, Watch)>,
    objectives_off: std::collections::HashSet<String>,
    seen_objectives: BTreeMap<Objective, (i64, String)>,
}

/// The newest round as the screens show it: every champion's power, and this round's steps.
#[derive(Debug, Default, PartialEq)]
pub struct View {
    pub id: u32,
    pub power: Arc<BTreeMap<String, i32>>,
    pub steps: BTreeMap<String, i32>,
}

fn view_of(round: &Round, power: &Arc<BTreeMap<String, i32>>) -> Arc<View> {
    Arc::new(View {
        id: round.id,
        power: Arc::clone(power),
        steps: round.changes.iter().map(|c| (c.champion.clone(), c.step)).collect(),
    })
}

/// The book is written at most this often while matches are kept.
const WRITE_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| Mutex::new(Store::default()));

fn store() -> std::sync::MutexGuard<'static, Store> {
    STORE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The book's file of 2.8.0 and 2.8.1: the player's team id and a hash of its name. Two careers with the same team
/// shared it (a new one took the old one's rounds), so a save has an id of its own now ([`open_book`]).
pub fn legacy_path(team: usize, team_name: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    team_name.hash(&mut h);
    paths::mod_dir().join(format!("balance-team{team}-{:08x}.json", h.finish() as u32))
}

/// A save is open: its book, `balance-<id>.json` - the id is kept in the save (`server::book`), `done` is the newest
/// round the save's news has. A save from before the ids with rounds written takes over its book of then
/// ([`legacy_path`]) the first time. The book's rounds after `done` (made after the save was written - a later save of
/// the career has them) are not played, and stay in the file until this save makes a round of its own.
pub fn open_book(id: &str, team: usize, team_name: &str, done: u32) {
    let file = paths::mod_dir().join(format!("balance-{id}.json"));
    open_book_file(file, legacy_path(team, team_name), done);
}

fn open_book_file(file: PathBuf, legacy: PathBuf, done: u32) {
    open_file(file);
    let mut st = store();
    st.done = done;
    if st.rounds.is_empty() && done > 0 {
        // the save has rounds in its news and the book none: a save of 2.8.0 / 2.8.1, its book named by the team
        let old = std::fs::read_to_string(&legacy).ok().and_then(|t| serde_json::from_str::<Book>(&t).ok());
        if let Some(book) = old.filter(|b| !b.rounds.is_empty()) {
            diag::log(&format!("[balance] this save's book taken over from {}", legacy.display()));
            load(&mut st, book);
            write(&mut st);
        }
    }
    let at = st.rounds.iter().position(|r| r.id > done).unwrap_or(st.rounds.len());
    let later = st.rounds.split_off(at);
    if !later.is_empty() {
        for r in &later {
            st.powers.remove(&r.id);
            st.item_powers.remove(&r.id);
            st.objective_powers.remove(&r.id);
        }
        let view = st.rounds.last().and_then(|r| st.powers.get(&r.id).map(|power| view_of(r, power)));
        st.view = view;
        diag::log(&format!("[balance] {} rounds of the book are after this save (not in its news): not played", later.len()));
        st.later = later;
    }
    if !st.rounds.is_empty() {
        diag::log(&format!("[balance] {} rounds in this save's book, {} matches kept", st.rounds.len(), st.kept.len()));
    }
}

fn open_file(file: PathBuf) {
    let book = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str::<Book>(&t).ok()).unwrap_or_default();
    let mut st = store();
    *st = Store { file: Some(file), ..Store::default() };
    load(&mut st, book);
}

fn load(st: &mut Store, book: Book) {
    for r in book.rounds {
        let power = Arc::new(r.power.clone());
        st.view = Some(view_of(&r, &power));
        st.powers.insert(r.id, power);
        st.item_powers.insert(r.id, Arc::new(r.item_power.clone()));
        st.objective_powers.insert(r.id, Arc::new(r.objective_power.clone()));
        st.rounds.push(r);
    }
    for (key, id) in book.kept {
        if st.kept.insert(key.clone(), id).is_none() {
            st.order.push_back(key);
        }
    }
    st.items_off = book.items_off.into_iter().filter(|k| st.kept.contains_key(k)).collect();
    st.objectives_off = book.objectives_off.into_iter().filter(|k| st.kept.contains_key(k)).collect();
    st.seen = book.seen;
    st.watched = book.watched.into_iter().collect();
    st.seen_objectives = book.seen_objectives;
}

/// The save is closed (its kept matches written first).
pub fn close() {
    let mut st = store();
    if st.unwritten {
        write(&mut st);
    }
    *st = Store::default();
}

fn write(st: &mut Store) {
    let Some(file) = &st.file else { return };
    let kept = st.order.iter().filter_map(|k| Some((k.clone(), *st.kept.get(k)?))).collect();
    let items_off = st.order.iter().filter(|k| st.items_off.contains(*k)).cloned().collect();
    let objectives_off = st.order.iter().filter(|k| st.objectives_off.contains(*k)).cloned().collect();
    let book = Book {
        v: 1,
        rounds: st.rounds.iter().chain(&st.later).cloned().collect(),
        kept,
        items_off,
        seen: st.seen.clone(),
        watched: st.watched.iter().cloned().collect(),
        objectives_off,
        seen_objectives: st.seen_objectives.clone(),
    };
    if let Ok(text) = serde_json::to_string(&book) {
        let _ = std::fs::write(file, text);
    }
    st.unwritten = false;
    st.written_at = Some(std::time::Instant::now());
}

/// Every round of this save, oldest first.
pub fn rounds() -> Vec<Round> {
    store().rounds.clone()
}

/// This save's newest round.
pub fn latest() -> Option<Round> {
    store().rounds.last().cloned()
}

/// The newest round as the screens show it (each frame asks: a pointer, no copy).
pub fn view() -> Option<Arc<View>> {
    store().view.clone()
}

/// The round a patch note shows, found by its champions (2.12.2, the patch history's item block `ui::patch_items`: the
/// page's titles are a text reference whose version the mod cannot read, and its entries share one name, so only each
/// column's count and first champion can be read): the rounds whose buffs and nerfs (steps other than 0, in the
/// round's order - the order the game lists them) are as many as shown and hold the shown first ones; of those the one
/// whose first ones are the shown first ones, the newest of equals. Its label and its item changes.
pub fn round_of_patch(shown: &crate::ui::patch_items::Shown) -> crate::ui::patch_items::Found {
    if shown.is_empty() {
        return None;
    }
    let st = store();
    let mut best: Option<(u8, &Round)> = None;
    for r in &st.rounds {
        let side = |buff: bool| -> Vec<&str> { r.changes.iter().filter(|c| c.step != 0 && (c.step > 0) == buff).map(|c| c.champion.as_str()).collect() };
        let (buffs, nerfs) = (side(true), side(false));
        if buffs.len() != shown.buffs || nerfs.len() != shown.nerfs {
            continue;
        }
        let holds = |list: &[&str], first: &Option<String>| first.as_deref().is_none_or(|f| list.contains(&f));
        if !holds(&buffs, &shown.first_buff) || !holds(&nerfs, &shown.first_nerf) {
            continue;
        }
        let score = u8::from(buffs.first().copied() == shown.first_buff.as_deref()) + u8::from(nerfs.first().copied() == shown.first_nerf.as_deref());
        if best.is_none_or(|(b, _)| score >= b) {
            best = Some((score, r));
        }
    }
    let (_, r) = best?;
    Some((if r.label.is_empty() { r.version.clone() } else { r.label.clone() }, r.items.clone(), r.objectives.clone()))
}

/// The newest round's item power as the item pages show it (2.12.1, `ui::item_view`; a pointer, no copy).
pub fn item_view() -> Option<Arc<BTreeMap<String, i32>>> {
    let st = store();
    let newest = st.rounds.last()?;
    st.item_powers.get(&newest.id).cloned()
}

/// Whether a round is due on this save ([`due`]).
pub fn due_now(patch: Option<&str>, today: Option<(i32, u32, u32)>) -> Option<Due> {
    let st = store();
    st.file.as_ref()?;
    due(&st.rounds, patch, today)
}

/// The round `due` asks for on this save ([`make_round`]); not in the book until [`publish`]ed.
pub fn make(meta: &Meta, rules: Rules, due: &Due, today: Option<(i32, u32, u32)>, skip: &Skip, categories: &HashMap<String, u8>) -> Round {
    let items = if rules.items { crate::items::inputs() } else { None };
    if rules.items && items.is_none() {
        diag::log_once("items-no-inputs", "[items] item_balance is on but the 4+6 builds or catalog are not read (yet): this round changes no item");
    }
    let st = store();
    let mut round = make_round_with(meta, &st.rounds, rules, due, today, items.as_ref(), skip);
    round.id = round.id.max(st.done + 1);
    // 塔和龙（2.13.0）：拿账本里每场看到的判断
    if let Some(targets) = rules.objectives {
        let watched: Vec<&Watch> = st.watched.iter().map(|(_, w)| w).collect();
        let (changes, said) = crate::objectives::decide(&st.rounds, &watched, &round.objective_power, &targets);
        for c in &changes {
            if c.total == 0 {
                round.objective_power.remove(&c.objective);
            } else {
                round.objective_power.insert(c.objective, c.total);
            }
        }
        round.objectives = changes;
        round.objective_summary = format!("{said} ({} matches watched)", watched.len());
    }
    apply_theme(&mut round, &st.rounds, rules, categories, meta, skip);
    round
}

/// 主题版本（2.14.0，`crate::theme`）：该出主题的游戏版本里，挑出来的那一类英雄全部加强 `theme_step`%。这一轮按胜率
/// 给它们的改动不算，下一轮再照常判断；`balance_skip` 列的不加，已经到上限的不动。
fn apply_theme(round: &mut Round, rounds: &[Round], rules: Rules, categories: &HashMap<String, u8>, meta: &Meta, skip: &Skip) {
    if !round.merged || round.reset || rules.theme_step == 0 || !crate::theme::due(rounds, rules.theme_every) {
        return;
    }
    let pool: Vec<(String, u8)> = meta
        .champions
        .iter()
        .filter(|c| !skip.champions.contains(&c.name))
        .filter_map(|c| Some((c.name.clone(), *categories.get(&c.name)?)))
        .collect();
    let available: Vec<u8> = crate::theme::CATEGORIES
        .iter()
        .copied()
        .filter(|cat| pool.iter().filter(|(_, c)| c == cat).count() >= crate::theme::MIN_CHAMPIONS)
        .collect();
    let Some(category) = crate::theme::pick(rounds, &round.version, &available) else { return };
    let before = rounds.last().map(|r| r.power.clone()).unwrap_or_default();
    let mut buffed = 0;
    for (name, _) in pool.iter().filter(|(_, c)| *c == category) {
        let was = before.get(name).copied().unwrap_or(0);
        let total = (was + rules.theme_step).clamp(-CAP, CAP);
        let mut win = (0.0, 0.0);
        if let Some(at) = round.changes.iter().position(|c| c.champion == *name) {
            win = (round.changes[at].win, round.changes[at].evidence);
            round.changes.remove(at);
        }
        if total == 0 {
            round.power.remove(name);
        } else {
            round.power.insert(name.clone(), total);
        }
        if total != was {
            buffed += 1;
            round.changes.push(Change {
                champion: name.clone(),
                win: win.0,
                evidence: win.1,
                step: total - was,
                total,
                unpicked: false,
                reset: None,
                theme: Some(category),
            });
        }
    }
    round.theme = Some(category);
    round.summary.push_str(&format!("; theme patch: category {category}, {buffed} champions +{}%", rules.theme_step));
}

/// The round of resets due on this save now ([`make_reset`]); not in the book until [`publish`]ed.
pub fn reset_now(
    champions: &[(String, Reset)],
    items: &[(String, String, Reset)],
    objectives: &[(Objective, i64)],
    today: Option<(i32, u32, u32)>,
) -> Option<Round> {
    let st = store();
    st.file.as_ref()?;
    let mut round = make_reset(&st.rounds, champions, items, objectives, today)?;
    round.id = round.id.max(st.done + 1);
    Some(round)
}

/// The champions and items with power in the newest round of this save, among `champions` / `items` (2.11.0: those a
/// skip list names and that a reset should take power off).
pub fn with_power(champions: &std::collections::HashSet<String>, items: &std::collections::HashSet<String>) -> (Vec<String>, Vec<String>) {
    let st = store();
    let Some(last) = st.rounds.last() else { return (Vec::new(), Vec::new()) };
    let mut c: Vec<String> = last.power.iter().filter(|(k, p)| **p != 0 && champions.contains(*k)).map(|(k, _)| k.clone()).collect();
    let mut i: Vec<String> = last.item_power.iter().filter(|(k, p)| **p != 0 && items.contains(*k)).map(|(k, _)| k.clone()).collect();
    c.sort();
    i.sort();
    (c, i)
}

/// Every champion's numbers as read now (2.11.0, [`crate::edits`]) against what this save's book saw last: the book
/// keeps them from now on, and the champions whose numbers the player changed are returned (only when `judging` -
/// the balance on; numbers changed with it off are taken as they are). Empty without a book.
pub fn observe(
    now: &HashMap<String, crate::edits::Numbers>,
    definitions: &HashMap<String, crate::edits::Numbers>,
    judging: bool,
) -> Vec<(String, crate::edits::Verdict)> {
    let mut st = store();
    if st.file.is_none() {
        return Vec::new();
    }
    let mut names: Vec<&String> = now.keys().collect();
    names.sort();
    let mut out = Vec::new();
    for name in names {
        let numbers = &now[name];
        let def = definitions.get(name);
        let verdict = crate::edits::judge(st.seen.get(name), numbers, def);
        if verdict == crate::edits::Verdict::Same {
            continue;
        }
        st.seen.insert(name.clone(), crate::edits::Seen::new(numbers, def));
        st.unwritten = true;
        if judging && matches!(verdict, crate::edits::Verdict::Player | crate::edits::Verdict::ModUpdate) {
            out.push((name.clone(), verdict));
        }
    }
    if st.unwritten {
        write(&mut st);
    }
    out
}

/// 塔和龙的基础生命（游戏设置里的）和账本上次读到的比（2.13.0）：账本记下新的数，返回玩家在数据库里改了的那些。
/// 同一个版本里、又不在补丁日前后变的才算玩家改的（游戏自己的规则补丁在赛季开始那个补丁日改塔血）；`judging` 关着
/// （平衡没开）的时候只记不算。
pub fn observe_objectives(now: &[(Objective, i64)], patch: &str, near_patch: bool, judging: bool) -> Vec<Objective> {
    let mut st = store();
    if st.file.is_none() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for &(objective, value) in now {
        match st.seen_objectives.get(&objective) {
            Some((was, seen_in)) if *was == value && seen_in == patch => continue,
            Some((was, seen_in)) if *was != value && seen_in == patch && judging && !near_patch => out.push(objective),
            _ => {}
        }
        st.seen_objectives.insert(objective, (value, patch.to_string()));
        st.unwritten = true;
    }
    if st.unwritten {
        write(&mut st);
    }
    out
}

/// 服务端那一份交来的一场比赛的记录（2.13.0）：同一场的换掉，新的放最后，只留 [`crate::objectives::WATCHED`] 场
fn keep_watch(key: &str, watch: Watch) {
    let mut st = store();
    if st.file.is_none() || key.is_empty() {
        return;
    }
    match st.watched.iter().rposition(|(k, _)| k == key) {
        Some(at) => st.watched[at].1 = watch,
        None => {
            st.watched.push_back((key.to_string(), watch));
            while st.watched.len() > crate::objectives::WATCHED {
                st.watched.pop_front();
            }
            // 游戏里看得到已经在记了
            diag::log_once(
                "objectives-watch",
                &format!("[objectives] the server's runs write down each match's length and drake / Baron lives (first: match {key}, {} kept)", st.watched.len()),
            );
        }
    }
    st.unwritten = true;
    if st.written_at.is_none_or(|t| t.elapsed() >= WRITE_EVERY) {
        write(&mut st);
    }
}

/// A new round: in the book from now on (new matches play it).
pub fn publish(round: Round) {
    publish_in(&mut store(), round);
}

fn publish_in(st: &mut Store, round: Round) {
    if st.file.is_none() {
        return;
    }
    // a round of this save's own: the book's rounds after the save are another save's no more
    st.later.clear();
    let power = Arc::new(round.power.clone());
    st.view = Some(view_of(&round, &power));
    st.powers.insert(round.id, power);
    st.item_powers.insert(round.id, Arc::new(round.item_power.clone()));
    st.objective_powers.insert(round.id, Arc::new(round.objective_power.clone()));
    st.rounds.push(round);
    write(st);
}

/// The open save's book (its file), as [`publish_prepared`] checks it.
pub fn book() -> Option<String> {
    store().file.as_ref().map(|f| f.display().to_string())
}

/// A round made ahead for the game's patch `round.version` (`server::watch`) on the save whose book is `book`: in the
/// book unless that save is no longer open or its book moved on since - a round of that patch already (the client's
/// own), or another round in between. Err says why.
pub fn publish_prepared(round: Round, book: &str) -> Result<(), String> {
    let mut st = store();
    match &st.file {
        None => return Err("no save open".to_string()),
        Some(f) if f.display().to_string() != book => return Err("made on another save".to_string()),
        Some(_) => {}
    }
    if st.rounds.iter().any(|r| r.version == round.version) {
        return Err(format!("the book has a round of {} already", round.version));
    }
    let next = st.rounds.last().map_or(1, |r| r.id + 1).max(st.done + 1);
    if round.id != next {
        return Err(format!("made as round {} while the book's next is {next}", round.id));
    }
    publish_in(&mut st, round);
    Ok(())
}

/// The model's patch notes from this save's rounds.
pub fn notes() -> Vec<PatchNote> {
    notes_of(&store().rounds)
}

// ===================== in the matches =====================

type RunKey = (u32, u64, u64, u64);

struct Run {
    key: RunKey,
    last_tick: usize,
    /// None until decided; Some(None) = no balance.
    power: Option<Option<RunPower>>,
    /// 塔和龙：记录（服务端那一份）和改血（2.13.0）
    tracker: Option<Tracker>,
}

/// What a run plays: every champion's power, and the items' (None: no item change).
#[derive(Clone)]
struct RunPower {
    champions: Arc<BTreeMap<String, i32>>,
    items: Option<Arc<BTreeMap<String, i32>>>,
    /// 打的是哪一轮，和塔、小龙、大龙的百分比（None：这一场不改）
    round: u32,
    objectives: Option<Arc<BTreeMap<Objective, i32>>>,
}

static RUNS: LazyLock<Mutex<VecDeque<Run>>> = LazyLock::new(|| Mutex::new(VecDeque::new()));

fn run_key(sim: &StableSim<'_>) -> RunKey {
    sim.sim_origin().map_or((u32::MAX, u64::MAX, u64::MAX, u64::MAX), |o| (o.kind, o.match_id, o.replay_id, o.set_index))
}

/// This run's balance, decided once it has its champions (None: not yet). The server's run takes the newest round
/// (`on`: the setting) and keeps it; any other run plays what was kept (none when nothing was).
fn decide_run(sim: &StableSim<'_>, on: bool, items_on: bool, objectives_on: bool) -> Option<Option<RunPower>> {
    let mut sides: [Vec<String>; 2] = [Vec::new(), Vec::new()];
    let mut missing = 0;
    for i in 0..sim.player_count() {
        let Some(p) = sim.player_at(i) else { continue };
        match (sides.get_mut(p.team()), p.champion().and_then(|e| e.name())) {
            (Some(side), Some(name)) => side.push(name),
            _ => missing += 1,
        }
    }
    if (missing > 0 || sides.iter().any(Vec::is_empty)) && sim.tick() < TICK_WINDOW {
        return None;
    }
    let key = crate::ai_tactics::key(&sides);
    let first_run = sim.sim_origin().is_some_and(|o| SimOriginKindV1::from_code(o.kind) == Some(SimOriginKindV1::ServerPresim));
    Some(round_for(key, first_run, on, items_on, objectives_on))
}

/// The power a run of the match `key` plays: the server's run (`first_run`) takes the newest round (none with the
/// setting off) and keeps it under the key; any other run plays what was kept (none when nothing was). The items'
/// power with it, unless the server's run had `item_balance` off (kept too, 2.9.0); 塔和龙的也一样（2.13.0）。
fn round_for(key: String, first_run: bool, on: bool, items_on: bool, objectives_on: bool) -> Option<RunPower> {
    let mut st = store();
    // no save open here (yet): nothing to play, nothing to keep
    st.file.as_ref()?;
    if first_run {
        let id = if on { st.rounds.last().map_or(0, |r| r.id) } else { 0 };
        // nothing ever played on this save: nothing to keep either
        if id == 0 && st.rounds.is_empty() {
            return None;
        }
        let items_changed = if items_on { st.items_off.remove(&key) } else { st.items_off.insert(key.clone()) };
        let objectives_changed = if objectives_on { st.objectives_off.remove(&key) } else { st.objectives_off.insert(key.clone()) };
        if items_changed || objectives_changed {
            st.unwritten = true;
        }
        match st.kept.insert(key.clone(), id) {
            // the same match run again by the server: nothing new to write
            Some(was) if was == id => {}
            was => {
                if was.is_none() {
                    st.order.push_back(key);
                    while st.order.len() > KEPT {
                        if let Some(old) = st.order.pop_front() {
                            st.kept.remove(&old);
                            st.items_off.remove(&old);
                            st.objectives_off.remove(&old);
                        }
                    }
                }
                st.unwritten = true;
                if st.written_at.is_none_or(|t| t.elapsed() >= WRITE_EVERY) {
                    write(&mut st);
                }
            }
        }
        return run_power(&st, id, items_on, objectives_on);
    }
    let id = st.kept.get(&key).copied().unwrap_or(0);
    let items_on = !st.items_off.contains(&key);
    let objectives_on = !st.objectives_off.contains(&key);
    run_power(&st, id, items_on, objectives_on)
}

fn run_power(st: &Store, id: u32, items_on: bool, objectives_on: bool) -> Option<RunPower> {
    let champions = st.powers.get(&id).cloned()?;
    let items = if items_on { st.item_powers.get(&id).filter(|m| !m.is_empty()).cloned() } else { None };
    let objectives = if objectives_on { st.objective_powers.get(&id).filter(|m| !m.is_empty()).cloned() } else { None };
    Some(RunPower { champions, items, round: id, objectives })
}

/// 这一份要不要看塔和龙：服务端那一份（十个英雄的正式比赛）记录，别的只在有百分比的时候改血
fn start_tracker(sim: &StableSim<'_>, power: &RunPower, objectives_on: bool) -> Option<Tracker> {
    let origin = sim.sim_origin();
    let server = origin.is_some_and(|o| SimOriginKindV1::from_code(o.kind) == Some(SimOriginKindV1::ServerPresim));
    if server && sim.player_count() >= 10 {
        if let Some(o) = origin {
            let mut tracker = Tracker::recording(Watch { r: power.round, off: !objectives_on, ..Watch::default() });
            tracker.key = format!("{}.{}", o.match_id, o.set_index);
            return Some(tracker);
        }
    }
    power.objectives.as_ref().filter(|m| !m.is_empty()).map(|_| Tracker::default())
}

/// The champions to give the buff now: alive, a power of their own, not carrying it yet - (entity id, power).
pub fn missing(champions: &[(usize, String, bool, bool)], power: &BTreeMap<String, i32>) -> Vec<(usize, i32)> {
    champions
        .iter()
        .filter(|(_, _, alive, has)| *alive && !*has)
        .filter_map(|(id, name, _, _)| power.get(name).filter(|p| **p != 0).map(|p| (*id, *p)))
        .collect()
}

/// Every tick of every match (the mod's match hook): the run's balance (decided once) on each of its champions that
/// does not carry it yet - at the start, and again after every respawn; every [`ITEM_EVERY`] ticks the items'
/// corrections on their holders (2.9.0).
pub fn on_tick(sim: &mut StableSim<'_>, on: bool, items_on: bool, objectives_on: bool) {
    let key = run_key(sim);
    let tick = sim.tick();
    let scan = tick.is_multiple_of(crate::objectives::SCAN);
    let (power, tracker) = {
        let mut runs = RUNS.lock().unwrap_or_else(PoisonError::into_inner);
        let at = match runs.iter().position(|r| r.key == key) {
            Some(at) => at,
            None => {
                runs.push_back(Run { key, last_tick: tick, power: None, tracker: None });
                while runs.len() > RUNS_KEPT {
                    runs.pop_front();
                }
                runs.len() - 1
            }
        };
        let run = &mut runs[at];
        if tick < run.last_tick {
            // the match started over
            run.power = None;
            run.tracker = None;
        }
        run.last_tick = tick;
        if run.power.is_none() {
            run.power = decide_run(sim, on, items_on, objectives_on);
            if let Some(Some(p)) = &run.power {
                run.tracker = start_tracker(sim, p, objectives_on);
            }
        }
        let power = match &run.power {
            Some(Some(p)) => p.clone(),
            _ => return,
        };
        // 塔和龙每 SCAN tick 看一次：拿出来用，用完放回去
        let tracker = if scan { run.tracker.take() } else { None };
        (power, tracker)
    };
    let champions: Vec<(usize, String, bool, bool)> = (0..sim.player_count())
        .filter_map(|i| sim.player_at(i).and_then(|p| p.champion()))
        .map(|c| {
            let has = (0..c.buff_count()).filter_map(|k| c.buff_at(k)).any(|b| b.name() == BUFF);
            (c.id(), c.name().unwrap_or_default(), c.is_alive(), has)
        })
        .collect();
    for (id, pw) in missing(&champions, &power.champions) {
        sim.add_buff(id, &buff(pw));
    }
    if let Some(items) = power.items.as_ref().filter(|_| tick.is_multiple_of(ITEM_EVERY)) {
        item_pass(sim, items);
    }
    if let Some(tracker) = tracker {
        objective_pass(sim, key, tick, tracker, &power);
    }
}

/// 塔和龙（2.13.0）：看一次场上，改血、记录，服务端那一份把记录交给账本，最后放回这一份里
fn objective_pass(sim: &mut StableSim<'_>, key: RunKey, tick: usize, mut tracker: Tracker, power: &RunPower) {
    let none = BTreeMap::new();
    let powers = power.objectives.as_deref().unwrap_or(&none);
    for a in tracker.step(&mut crate::objectives::SimField { sim }, powers) {
        diag::log_once(
            &format!("objective-{:?}-{}", a.objective, a.how),
            &format!(
                "[objectives] {:?} {:+}%: hp {}/{} -> {}/{} ({}; later ones are not logged)",
                a.objective, a.power, a.before.0, a.before.1, a.after.0, a.after.1, a.how
            ),
        );
    }
    if tracker.watch.is_some() && (tracker.changed || tracker.over || tick.is_multiple_of(crate::objectives::FLUSH)) {
        if let Some(watch) = tracker.snapshot() {
            keep_watch(&tracker.key, watch);
        }
        tracker.changed = false;
    }
    let mut runs = RUNS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(run) = runs.iter_mut().find(|r| r.key == key) {
        if run.tracker.is_none() && run.power.is_some() {
            run.tracker = Some(tracker);
        }
    }
}

/// The items' corrections on their holders (2.9.0, [`crate::items::corrections`]): added for an item with power,
/// replaced when its numbers moved, taken off once the item is gone.
fn item_pass(sim: &mut StableSim<'_>, items: &BTreeMap<String, i32>) {
    let Some(catalog) = crate::items::catalog() else { return };
    let mut adds: Vec<(usize, BuffV1)> = Vec::new();
    let mut removes: Vec<(usize, String)> = Vec::new();
    for i in 0..sim.player_count() {
        let Some(c) = sim.player_at(i).and_then(|p| p.champion()) else { continue };
        if !c.is_alive() {
            continue;
        }
        let id = c.id();
        let mut owned: Vec<(String, BuffV1)> = Vec::new();
        let mut have: Vec<(String, BuffV1)> = Vec::new();
        for k in 0..c.buff_count() {
            let Some(b) = c.buff_at(k) else { continue };
            let name = b.name().to_string();
            if name.starts_with(crate::items::OWNED) {
                owned.push((name, b));
            } else if name.starts_with(crate::items::CORRECTION) {
                have.push((name, b));
            }
        }
        let want = crate::items::corrections(&catalog, &owned, items);
        for w in &want {
            match have.iter().find(|(n, _)| n == w.name()) {
                Some((_, b)) if crate::items::same_numbers(b, w) => {}
                Some((n, _)) => {
                    removes.push((id, n.clone()));
                    adds.push((id, *w));
                }
                None => adds.push((id, *w)),
            }
        }
        for (n, _) in &have {
            if !want.iter().any(|w| w.name() == n) {
                removes.push((id, n.clone()));
            }
        }
    }
    for (id, name) in removes {
        sim.entity_remove_buff(id, &name);
    }
    for (id, b) in adds {
        sim.add_buff(id, &b);
    }
}

/// Test support: a book in `file` open, as for a save.
#[cfg(test)]
pub fn open_for_tests(file: PathBuf) {
    open_file(file);
}

/// Test support: no save, no runs.
pub fn reset_for_tests() {
    close();
    RUNS.lock().unwrap_or_else(PoisonError::into_inner).clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用真实账本预览新写法（2.14.2）：最近一轮有英雄和装备改动的，每个英雄的理由代码和整体强度、装备新闻。
    /// PMA_BOOK=<账本> PMA_ITEMS=<4+6 的 item-catalog.json> PMA_PREVIEW_OUT=<输出> cargo test preview_notes -- --ignored
    #[test]
    #[ignore]
    fn preview_notes_on_a_book() {
        let book = std::env::var("PMA_BOOK").expect("PMA_BOOK");
        let out = std::env::var("PMA_PREVIEW_OUT").expect("PMA_PREVIEW_OUT");
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&book).unwrap()).unwrap();
        let rounds: Vec<Round> = serde_json::from_value(v["rounds"].clone()).unwrap();
        let round = rounds.iter().rev().find(|r| !r.changes.is_empty() && !r.items.is_empty()).unwrap();
        let catalog = std::env::var("PMA_ITEMS").ok().and_then(|p| crate::items::Catalog::parse(&std::fs::read_to_string(p).ok()?));
        let mut lines = Vec::new();
        for c in &round.changes {
            let kind = kind_of(crate::knowledge::get().profiles.get(&c.champion), None);
            let f = flavor_of(c, round.id, &rounds, kind);
            let power = power_row(c).map(|r| format!("{} -> {}", r.pre_value, r.after_value)).unwrap_or_default();
            lines.push(
                serde_json::json!({"champion": c.champion, "step": c.step, "codes": reason(c, &f), "power": power, "kind": format!("{kind:?}"), "streak": f.streak})
                    .to_string(),
            );
        }
        let item_lines = |c: &ItemChange| -> Vec<ItemLine> {
            catalog
                .as_ref()
                .and_then(|cat| cat.item(&c.item))
                .map(|(_, item)| {
                    crate::items::field_lines(item, c.total - c.step, c.total, true)
                        .into_iter()
                        .map(|(f, l, a, b)| (crate::items::stat_icon_text(f), l, a, b))
                        .collect()
                })
                .unwrap_or_default()
        };
        let icon = |c: &ItemChange| catalog.as_ref().and_then(|cat| cat.item(&c.item)).map(|(_, i)| crate::items::icon_text(&i.icon)).unwrap_or_default();
        let (title, body) = item_news(round, true, &item_lines, &icon).unwrap();
        lines.push(serde_json::json!({"title": title, "body": body, "round": round.label}).to_string());
        std::fs::write(out, lines.join("
")).unwrap();
    }

    /// The champions' power a run plays (the items' left out), as the tests before 2.9.0 read it.
    fn champs(key: String, first_run: bool, on: bool) -> Option<Arc<BTreeMap<String, i32>>> {
        round_for(key, first_run, on, true, true).map(|p| p.champions)
    }

    const RULES: Rules = Rules {
        count: 10,
        max_step: 10,
        per_point: DEFAULT_PER_POINT,
        unpicked: 0,
        items: false,
        item_count: DEFAULT_ITEM_COUNT,
        item_max: DEFAULT_ITEM_MAX,
        item_users: DEFAULT_ITEM_USERS,
        item_band: DEFAULT_ITEM_BAND,
        item_per_point: DEFAULT_ITEM_PER_POINT,
        objectives: None,
        theme_every: 0,
        theme_step: 0,
    };

    fn cand(name: &str, win: f32, evidence: f32) -> Candidate<'_> {
        Candidate { name, win, evidence, since_change: None, pooled: true }
    }

    #[test]
    fn the_furthest_outside_the_band_change_most_within_the_caps() {
        let before = BTreeMap::new();
        let out = decide(
            &[cand("maddie", 73.3, 221.0), cand("zeri", 64.0, 422.0), cand("gwen", 46.3, 7.0), cand("ok", 50.5, 300.0), cand("weak", 44.0, 80.0)],
            &before,
            RULES,
        );
        let got: Vec<(&str, i32)> = out.iter().map(|c| (c.champion.as_str(), c.step)).collect();
        // 2.5% a point: 21.3 points over: -53, held to -10; 12 over: -30 -> -10; 4 under: +10; gwen has too few games;
        // 50.5 is fine
        assert_eq!(got, [("maddie", -10), ("zeri", -10), ("weak", 10)]);
        // a round's count keeps the furthest
        assert_eq!(decide(&[cand("a", 60.0, 50.0), cand("b", 70.0, 50.0)], &before, Rules { count: 1, ..RULES })[0].champion, "b");
        // just outside the band: under half a percent, no change
        assert!(decide(&[cand("a", 52.1, 50.0)], &before, RULES).is_empty());
    }

    #[test]
    fn the_step_is_balance_per_point_a_point() {
        let before = BTreeMap::new();
        let rules = |per_point: f32| Rules { max_step: 40, per_point, ..RULES };
        let step = |win: f32, per_point: f32| decide(&[cand("x", win, 100.0)], &before, rules(per_point)).first().map_or(0, |c| c.step);
        // 4 points over / 6 under: 2.9.0's fixed 1.5, 2.10.0's default 2.5, and more
        assert_eq!((step(56.0, 1.5), step(56.0, 2.5), step(56.0, 4.0)), (-6, -10, -16));
        assert_eq!((step(42.0, 1.5), step(42.0, 2.5), step(42.0, 5.0)), (9, 15, 30));
        // still held to balance_max
        assert_eq!(decide(&[cand("x", 70.0, 100.0)], &before, Rules { per_point: 5.0, ..RULES })[0].step, -10);
        // the log says the size of a point
        let text = summarize(&[cand("x", 56.0, 100.0)], &before, rules(4.0), 1, 1);
        assert!(text.contains("(4% of power a point)"), "{text}");
    }

    #[test]
    fn rounds_add_up_to_the_cap() {
        let mut before = BTreeMap::new();
        before.insert("maddie".to_string(), -(CAP - 5));
        let out = decide(&[cand("maddie", 70.0, 100.0)], &before, RULES);
        assert_eq!((out[0].step, out[0].total), (-5, -CAP), "held to the cap in all");
        before.insert("maddie".to_string(), -CAP);
        assert!(decide(&[cand("maddie", 70.0, 100.0)], &before, RULES).is_empty(), "nothing left to take");
        // nerfed too far: buffed back
        before.insert("maddie".to_string(), -20);
        let out = decide(&[cand("maddie", 44.0, 100.0)], &before, RULES);
        assert_eq!((out[0].step, out[0].total), (10, -10));
    }

    fn logit(p: f32) -> f32 {
        (p / (1.0 - p)).ln()
    }

    fn champ(name: &str, win: f32, evidence: f32) -> crate::meta::Champion {
        crate::meta::Champion { name: name.into(), strength: logit(win / 100.0), evidence, ..crate::meta::Champion::default() }
    }

    fn meta() -> Meta {
        Meta {
            current: "2027.4.1".into(),
            current_matches: 100,
            champions: vec![champ("maddie", 73.3, 221.0), champ("fine", 50.0, 300.0), champ("weak", 44.0, 80.0)],
            ..Meta::default()
        }
    }

    /// `meta()` with games per patch: every champion played `games[i]` games in `versions[i]`.
    fn meta_with_games(versions: &[&str], games: &[u32]) -> Meta {
        let mut m = meta();
        m.versions = versions.iter().map(|v| v.to_string()).collect();
        m.current = versions.last().map_or(String::new(), |v| v.to_string());
        for c in &mut m.champions {
            c.history = games.iter().map(|g| (c.strength, crate::meta::Tally { games: *g, wins: g / 2 })).collect();
        }
        m
    }

    #[test]
    fn a_changed_champion_waits_for_games_with_its_new_numbers() {
        let today = Some((2027, 10, 1));
        // a hotfix of 2027.4.1, then the game's 2027.4.2 two days later (no game of it yet)
        let first = make_round(&meta_with_games(&["2027.4.0", "2027.4.1"], &[300, 200]), &[], RULES, &Due::First("2027.4.1".into()), today);
        assert_eq!(first.power.get("maddie"), Some(&-10));
        let rounds = vec![first];
        let fresh = meta_with_games(&["2027.4.0", "2027.4.1", "2027.4.2"], &[300, 200, 0]);
        let maddie = &fresh.champions[0];
        // the hotfix's own patch mixes games from before it: only later patches count
        assert_eq!(games_since_change(&rounds, &fresh.versions, maddie), Some(0.0));
        assert_eq!(games_since_change(&rounds, &fresh.versions, &fresh.champions[1]), None, "never changed");
        let second = make_round(&fresh, &rounds, RULES, &Due::Patch("2027.4.2".into()), Some((2027, 10, 3)));
        assert_eq!(second.power.get("maddie"), Some(&-10), "not cut again on the old games");
        assert!(second.changes.is_empty(), "{:?}", second.changes);
        assert_eq!(second.held.iter().map(|(n, g)| (n.as_str(), *g)).collect::<Vec<_>>(), [("maddie", 0.0), ("weak", 0.0)]);
        // a patch later, with games enough at the new numbers: judged again (still too strong: cut again)
        let rounds = vec![rounds[0].clone(), second];
        let later = meta_with_games(&["2027.4.1", "2027.4.2", "2027.4.3"], &[200, 40, 0]);
        let third = make_round(&later, &rounds, RULES, &Due::Patch("2027.4.3".into()), Some((2027, 10, 20)));
        assert_eq!((third.power.get("maddie"), third.power.get("weak")), (Some(&-20), Some(&20)));
        assert!(third.held.is_empty());
        // a change merged into a game patch counts that patch's games (all with the new numbers)
        let rounds = vec![rounds[0].clone(), rounds[1].clone(), third];
        let next = meta_with_games(&["2027.4.2", "2027.4.3", "2027.4.4"], &[40, 25, 0]);
        assert_eq!(games_since_change(&rounds, &next.versions, &next.champions[0]), Some(25.0));
        let fourth = make_round(&next, &rounds, RULES, &Due::Patch("2027.4.4".into()), Some((2027, 11, 10)));
        assert!(fourth.changes.is_empty() && fourth.held.len() == 2, "25 games are not enough: {:?}", fourth.held);
    }

    #[test]
    fn the_first_round_is_a_hotfix_of_the_current_patch_then_one_per_game_patch() {
        let today = Some((2027, 9, 20));
        // no patch known yet: none
        assert_eq!(due(&[], None, today), None);
        // the setting just turned on: a hotfix of the game's patch
        let first_due = due(&[], Some("2027.4.1"), today).unwrap();
        assert_eq!(first_due, Due::First("2027.4.1".into()));
        let first = make_round(&meta(), &[], RULES, &first_due, today);
        assert_eq!((first.id, first.version.as_str(), first.label.as_str(), first.merged), (1, "2027.4.1", "2027.4.1b", false));
        assert_eq!((first.power.get("maddie"), first.power.get("weak"), first.power.get("fine")), (Some(&-10), Some(&10), None));
        let rounds = vec![first];
        // the same patch, days later: none
        assert_eq!(due(&rounds, Some("2027.4.1"), Some((2027, 10, 30))), None);
        // the game's next patch: into its notes, adding up (once a champion played games enough with its new numbers)
        let next = due(&rounds, Some("2027.5.0"), Some((2027, 10, 1))).unwrap();
        assert_eq!(next, Due::Patch("2027.5.0".into()));
        let second = make_round(&meta_with_games(&["2027.4.1", "2027.5.0"], &[200, 60]), &rounds, RULES, &next, Some((2027, 10, 1)));
        assert_eq!((second.id, second.label.as_str(), second.merged, second.power.get("maddie")), (2, "2027.5.0", true, Some(&-20)));
        // an older patch's notes seen again are no new patch
        let rounds = vec![rounds[0].clone(), second];
        assert_eq!(due(&rounds, Some("2027.4.1"), Some((2027, 10, 2))), None);
        // no game patch for LATE_DAYS: a hotfix of the current one (its first: b)
        assert_eq!(due(&rounds, Some("2027.5.0"), Some((2027, 11, 29))), None, "59 days");
        let late = due(&rounds, Some("2027.5.0"), Some((2027, 11, 30))).unwrap();
        assert_eq!(late, Due::Late("2027.5.0".into()));
        let third = make_round(&meta(), &rounds, RULES, &late, Some((2027, 11, 30)));
        assert_eq!((third.label.as_str(), third.merged), ("2027.5.0b", false));
        // a round of a book from before the dates: no hotfix by age
        let mut old = rounds[1].clone();
        old.date = None;
        assert_eq!(due(&[old], Some("2027.5.0"), Some((2029, 1, 1))), None);
    }

    #[test]
    fn hotfixes_are_lettered_like_leagues() {
        let r = |version: &str, merged: bool| Round { version: version.into(), merged, ..Round::default() };
        assert_eq!(hotfix_label(&[], "14.1"), "14.1b");
        assert_eq!(hotfix_label(&[r("14.1", true)], "14.1"), "14.1b");
        assert_eq!(hotfix_label(&[r("14.1", true), r("14.1", false), r("14.2", false)], "14.1"), "14.1c");
        let many: Vec<Round> = (0..25).map(|_| r("1.1", false)).collect();
        assert_eq!(hotfix_label(&many, "1.1"), "1.1-26");
        assert_eq!(day_number((1970, 1, 1)), 0);
        assert_eq!(day_number((2027, 3, 1)) - day_number((2027, 2, 28)), 1);
        assert_eq!(day_number((2028, 3, 1)) - day_number((2028, 2, 28)), 2, "a leap year");
    }

    #[test]
    fn the_lines_are_the_numbers_a_champion_plays_with() {
        // attack 94, magic power 0, health 1180; growth 6 / 0 / 90
        let stats = Stats { category: None, base: [94.0, 0.0, 1180.0, 30.0, 30.0, 60.0], growth: [6.0, 0.0, 90.0, 3.0, 1.0, 0.0] };
        let got = rows(&stats, 0, -10);
        let lines: Vec<(&str, &str, &str, bool)> =
            got.iter().map(|r| (r.key.rsplit('?').next().unwrap(), r.pre_value.as_str(), r.after_value.as_str(), r.is_buff)).collect();
        assert_eq!(
            lines,
            [
                ("stat.attack", "94", "89.3", false),
                ("stat.hp", "1180", "1121", false),
                ("patch_key.growth_attack", "6", "5.7", false),
                ("patch_key.growth_hp", "90", "85.5", false),
            ]
        );
        // a later round from -10 to -4: from the old numbers back up
        let back = rows(&stats, -10, -4);
        assert_eq!((back[0].pre_value.as_str(), back[0].after_value.as_str(), back[0].is_buff), ("89.3", "92.1", true));
        assert!(rows(&stats, 5, 5).is_empty());
        // the JSON the game writes for them (the same field order)
        assert_eq!(
            serde_json::to_string(&got[0]).unwrap(),
            r##"{"key":"#asset/base/text/champion?stat.attack","target_action":null,"pre_value":"94","after_value":"89.3","is_buff":false}"##
        );
        assert_eq!((number(1118.16), number(5.0), number(4.96), number(0.33)), ("1118.2".into(), "5".into(), "5".into(), "0.3".into()));
    }

    #[test]
    fn the_reason_is_the_games_own_words_then_the_win_rate() {
        let c = |win: f32, step: i32, total: i32| Change { champion: "x".into(), win, evidence: 50.0, step, total, unpicked: false, reset: None, theme: None };
        let f = |kind: Kind, seed: u64| Flavor { kind, seed, ..Flavor::default() };
        // 削弱：输出、坦度、回复、后期；加强反过来；种子挑第几句
        assert_eq!(reason(&c(73.3, -10, -10), &f(Kind::Damage, 0)), "too_much_damage_1,pma_wr_74");
        assert_eq!(reason(&c(52.3, -1, -1), &f(Kind::Tank, 1)), "too_tanky_2,pma_wr_53", "rounded away from the band");
        assert_eq!(reason(&c(60.0, -5, -5), &f(Kind::Healer, 2)), "too_much_healing_3,pma_wr_60");
        assert_eq!(reason(&c(60.0, -5, -5), &f(Kind::Late, 3)), "late_game_scaling_1,pma_wr_60");
        assert_eq!(reason(&c(44.6, 5, 5), &f(Kind::Damage, 0)), "low_damage_1,pma_wr_44");
        assert_eq!(reason(&c(44.6, 5, 5), &f(Kind::Tank, 0)), "too_fragile_1,pma_wr_44");
        assert_eq!(reason(&c(44.6, 5, 5), &f(Kind::Healer, 0)), "low_healing_1,pma_wr_44");
        assert_eq!(reason(&c(44.6, 5, 5), &f(Kind::Late, 0)), "weak_late_game_1,pma_wr_44");
        // 连着调：版本强势 / 一直萎靡
        let streak = Flavor { streak: true, ..f(Kind::Tank, 4) };
        assert_eq!(reason(&c(70.0, -5, -30), &streak), "nerf_meta_1,pma_wr_70");
        assert_eq!(reason(&c(40.0, 8, 16), &streak), "buff_meta_2,pma_wr_40");
        // 禁选率、选用率没统计：「长期占据ban/pick榜首」「无人问津」这两句不用
        let streak = Flavor { streak: true, ..f(Kind::Tank, 5) };
        assert_eq!(reason(&c(70.0, -5, -30), &streak), "nerf_meta_3,pma_wr_70");
        assert_eq!(reason(&c(40.0, 8, 16), &streak), "buff_meta_3,pma_wr_40");
        // 冷门加强：官方的冷门理由和场次那一行；连着两轮冷门是「阔别太久」
        let rare = Change { unpicked: true, ..c(50.0, 10, 10) };
        assert_eq!(reason(&rare, &f(Kind::Damage, 1)), "unpicked_2,pma_unpicked");
        assert_eq!(reason(&rare, &Flavor { unpicked_before: true, ..f(Kind::Damage, 1) }), "unpicked_long,pma_unpicked");
        // 撤回、主题：电竞大脑自己的话；每条都有 pma_ 代码（patchnotes 靠它认出来）
        assert_eq!(reason(&Change { reset: Some(Reset::Skip), ..c(50.0, 10, 0) }, &f(Kind::Damage, 0)), "pma_skip");
        assert_eq!(reason(&Change { theme: Some(1), ..c(50.0, 8, 8) }, &f(Kind::Damage, 0)), "pma_theme_1");
        assert_eq!((signed_code(-11), signed_code(9)), ("m11".to_string(), "p9".to_string()));
        // 整体强度是一行：这一版之前和之后
        let row = power_row(&c(70.0, -5, -30)).unwrap();
        assert_eq!((row.key.as_str(), row.pre_value.as_str(), row.after_value.as_str(), row.is_buff), (POWER_KEY, "-25%", "-30%", false));
        let row = power_row(&c(40.0, 8, 8)).unwrap();
        assert_eq!((row.pre_value.as_str(), row.after_value.as_str(), row.is_buff), ("0%", "+8%", true));
        assert!(power_row(&c(40.0, 0, 8)).is_none());
    }

    #[test]
    fn the_flavor_comes_from_the_role_and_the_last_rounds() {
        let profile = |front: f32, peel: f32, carry: f32| crate::knowledge::Profile {
            dmg: crate::knowledge::Dmg::Ad,
            front,
            engage: 0.0,
            peel,
            poke: 0.0,
            dive: 0.0,
            carry,
            cc: 0.0,
            tier: 0,
        };
        assert_eq!(kind_of(Some(&profile(2.0, 0.0, 0.0)), Some(0)), Kind::Tank);
        assert_eq!(kind_of(Some(&profile(0.0, 2.0, 0.0)), Some(3)), Kind::Healer);
        assert_eq!(kind_of(Some(&profile(0.0, 0.0, 2.0)), Some(1)), Kind::Late);
        assert_eq!(kind_of(Some(&profile(0.5, 0.5, 0.5)), Some(2)), Kind::Damage);
        assert_eq!(kind_of(None, Some(5)), Kind::Tank);
        assert_eq!(kind_of(None, Some(3)), Kind::Healer);
        assert_eq!(kind_of(None, None), Kind::Damage);
        let change = |name: &str, step: i32, unpicked: bool| Change {
            champion: name.into(),
            win: 50.0,
            evidence: 50.0,
            step,
            total: step,
            unpicked,
            reset: None,
            theme: None,
        };
        let round = |id: u32, changes: Vec<Change>| Round { id, changes, ..Round::default() };
        let rounds = vec![round(3, vec![change("a", -5, false), change("b", 5, true)]), round(4, vec![change("c", 5, false)])];
        // 上一轮（或上上轮）也往同一个方向调：连着调
        assert!(flavor_of(&change("a", -5, false), 5, &rounds, Kind::Damage).streak);
        assert!(!flavor_of(&change("a", 5, false), 5, &rounds, Kind::Damage).streak, "the other way");
        assert!(!flavor_of(&change("a", -5, false), 6, &rounds, Kind::Damage).streak, "three rounds ago");
        let rare = flavor_of(&change("b", 5, true), 5, &rounds, Kind::Damage);
        assert!(rare.streak && rare.unpicked_before);
        // 同一个英雄同一轮总是同一句
        assert_eq!(seed("a", "5"), flavor_of(&change("a", -5, false), 5, &rounds, Kind::Damage).seed);
        assert_ne!(seed("a", "5"), seed("a", "6"));
    }

    #[test]
    fn the_cards_are_the_games_after_a_patch() {
        let round = Round {
            id: 3,
            version: "2027.5.0".into(),
            label: "2027.5.0".into(),
            merged: true,
            date: Some((2027, 10, 1)),
            changes: vec![
                Change { champion: "tft_maddie".into(), win: 73.3, evidence: 221.0, step: -10, total: -10, unpicked: false, reset: None, theme: None },
                Change { champion: "league_warwick".into(), win: 60.0, evidence: 90.0, step: -10, total: -20, unpicked: false, reset: None, theme: None },
                Change { champion: "league_kaisa".into(), win: 35.9, evidence: 80.0, step: 10, total: 10, unpicked: false, reset: None, theme: None },
            ],
            ..Round::default()
        };
        let row = Row { key: "#asset/base/text/champion?stat.attack".into(), target_action: None, pre_value: "94".into(), after_value: "89.3".into(), is_buff: false };
        let mut lines = BTreeMap::new();
        for c in &round.changes {
            lines.insert(c.champion.clone(), (vec![Row { is_buff: c.step > 0, ..row.clone() }], reason(c, &Flavor::default())));
        }
        let signatures = vec![
            Signature { athlete: 2240, name: "OneJey".into(), champion: "league_warwick".into() },
            Signature { athlete: 565, name: "Faker".into(), champion: "league_ahri".into() },
        ];
        let label = |c: &str| if c == "tft_maddie" { "麦迪".to_string() } else { c.to_string() };
        let cards = articles(&round, &lines, "Bark Knights", &signatures, &label);
        let codes: Vec<&str> = cards.iter().map(|a| a.content_bind.iter().find(|(k, _)| k == "ContentId").unwrap().1.as_str()).collect();
        // the most buffed, the most nerfed (the first of the equal ones, as the round lists them), the hit signature
        assert_eq!(codes, ["PC01", "PC02", "PC06"]);
        let pc02 = &cards[1];
        assert_eq!(pc02.title, "#asset/base/text/news?article.meta.pc02.title.0");
        assert_eq!(pc02.content, "#asset/base/text/news?article.meta.pc02.content.0");
        assert_eq!(pc02.author, AUTHOR);
        let keys: Vec<&str> = pc02.title_bind.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["Champion", "PatchChanges", "PatchReason", "Team"]);
        assert_eq!(pc02.title_bind[0].1, "tft_maddie");
        assert!(pc02.title_bind[1].1.starts_with(r##"[{"key":"#asset/base/text/champion?stat.attack""##), "{}", pc02.title_bind[1].1);
        assert_eq!(pc02.title_bind[2].1, "too_much_damage_1,pma_wr_74");
        let tail: Vec<(&str, &str)> = pc02.content_bind[4..].iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(
            tail,
            [("ContentId", "PC02"), ("SubjectId", "patch:2027-10-01:nerf:tft_maddie"), ("DateKey", "2027-10-01"), ("Scope", "patch"), ("Severity", "article")]
        );
        assert_eq!(pc02.plain_title, "麦迪在本次补丁中被削弱");
        let pc06 = &cards[2];
        assert_eq!(pc06.title, "#asset/base/text/news?alert.meta.pc06.title.0");
        assert_eq!(pc06.author, "Bark Knights");
        let keys: Vec<&str> = pc06.title_bind.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["Team", "Athlete", "Champion", "PatchChanges", "PatchReason"]);
        assert!(pc06.content_bind.contains(&("SubjectId".into(), "patch:2027-10-01:risk:2240:league_warwick".into())));
        assert!(pc06.content_bind.contains(&("Severity".into(), "alert".into())));
        // a champion with no lines (its numbers do not move) gets no card
        lines.insert("league_kaisa".into(), (Vec::new(), String::new()));
        assert!(!articles(&round, &lines, "T", &[], &label).iter().any(|a| a.title.contains("pc01")));
    }

    #[test]
    fn every_run_of_a_match_plays_the_round_its_first_run_kept() {
        let _serial = crate::tests::serial();
        reset_for_tests();
        let file = std::env::temp_dir().join(format!("pma_balance_test_{}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        // no save open: nothing
        assert_eq!(champs("k".into(), true, true), None);
        assert_eq!(due_now(Some("1.1"), None), None);
        open_file(file.clone());
        // a save with no round: nothing played, nothing kept
        assert_eq!(champs("k".into(), true, true), None);
        assert_eq!(due_now(Some("1.1"), None), Some(Due::First("1.1".into())));
        let mut power = BTreeMap::new();
        power.insert("maddie".to_string(), -10);
        publish(Round { id: 1, version: "1.1".into(), power: power.clone(), ..Round::default() });
        assert_eq!(view().map(|v| (v.id, v.power.get("maddie").copied())), Some((1, Some(-10))));
        // the server's run takes round 1; the client's run of the same match plays it too
        assert_eq!(champs("k".into(), true, true).as_deref(), Some(&power));
        assert_eq!(champs("k".into(), false, true).as_deref(), Some(&power));
        // a later round does not change a match already played
        let mut power2 = BTreeMap::new();
        power2.insert("maddie".to_string(), -20);
        publish(Round {
            id: 2,
            version: "1.2".into(),
            power: power2.clone(),
            changes: vec![Change { champion: "maddie".into(), win: 70.0, evidence: 99.0, step: -10, total: -20, unpicked: false, reset: None, theme: None }],
            ..Round::default()
        });
        assert_eq!(view().map(|v| v.steps.get("maddie").copied()), Some(Some(-10)));
        assert_eq!(champs("k".into(), false, true).as_deref(), Some(&power), "a replay plays its own round");
        // a run of a match nothing was kept for (from before): none
        assert_eq!(champs("other".into(), false, true), None);
        // with the setting off, a new match plays none - and its replays too
        assert_eq!(champs("off".into(), true, false), None);
        assert_eq!(champs("off".into(), false, true), None);
        // the book survives the session
        close();
        assert_eq!(view(), None);
        open_file(file.clone());
        assert_eq!(rounds().len(), 2);
        assert_eq!(view().map(|v| v.id), Some(2));
        assert_eq!(champs("k".into(), false, true).as_deref(), Some(&power));
        assert_eq!(notes().len(), 2);
        let _ = std::fs::remove_file(&file);
        reset_for_tests();
    }

    #[test]
    fn a_save_has_a_book_of_its_own_with_the_rounds_it_knows() {
        let _serial = crate::tests::serial();
        reset_for_tests();
        let file = |name: &str| std::env::temp_dir().join(format!("pma_book_{name}_{}.json", std::process::id()));
        let (legacy, own, career) = (file("legacy"), file("own"), file("career"));
        for f in [&legacy, &own, &career] {
            let _ = std::fs::remove_file(f);
        }
        // a book of 2.8.1 (named by the team): three rounds, a match played with each
        open_file(legacy.clone());
        for id in 1..=3u32 {
            let power = [("maddie".to_string(), -10 * id as i32)].into_iter().collect();
            publish(Round { id, version: format!("1.{id}"), power, ..Round::default() });
            champs(format!("match{id}"), true, true);
        }
        close();
        // its save opened by 2.8.2: the save's news has two rounds (the third was made after it was saved)
        open_book_file(own.clone(), legacy.clone(), 2);
        assert_eq!(rounds().iter().map(|r| r.id).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(view().map(|v| (v.id, v.power.get("maddie").copied())), Some((2, Some(-20))));
        assert_eq!(champs("match2".into(), false, true).map(|p| p["maddie"]), Some(-20));
        assert_eq!(champs("match3".into(), false, true), None, "a round after this save is not played");
        assert!(legacy.exists(), "the old book stays for that career's other saves");
        close();
        // the career's later save opened again (its own book now, whatever became of the old one): the third round
        // is still there
        let _ = std::fs::remove_file(&legacy);
        open_book_file(own.clone(), legacy.clone(), 3);
        assert_eq!(rounds().len(), 3);
        assert_eq!(champs("match3".into(), false, true).map(|p| p["maddie"]), Some(-30));
        close();
        // back on the earlier save, and it goes its own way: its round takes the later one's place
        open_book_file(own.clone(), legacy.clone(), 2);
        assert_eq!(due_now(Some("1.3"), None), Some(Due::Patch("1.3".into())), "the later round is not this save's");
        publish(Round { id: 3, version: "1.3".into(), label: "its own".into(), ..Round::default() });
        close();
        open_book_file(own.clone(), legacy.clone(), 3);
        assert_eq!(rounds().iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["", "", "its own"]);
        close();
        // a new career with the same team (its news has no round): an empty book of its own
        open_file(legacy.clone());
        publish(Round { id: 1, version: "1.1".into(), ..Round::default() });
        close();
        open_book_file(career.clone(), legacy.clone(), 0);
        assert!(rounds().is_empty() && view().is_none() && !career.exists());
        assert_eq!(due_now(Some("1.1"), None), Some(Due::First("1.1".into())));
        assert_eq!(make(&meta(), RULES, &Due::First("1.1".into()), None, &Skip::default(), &HashMap::new()).id, 1);
        close();
        // a save whose book was lost (its news has two rounds, no file anywhere): the ids go on after them
        let _ = std::fs::remove_file(&legacy);
        open_book_file(career.clone(), legacy.clone(), 2);
        assert!(rounds().is_empty());
        let next = make(&meta(), RULES, &Due::First("1.1".into()), None, &Skip::default(), &HashMap::new());
        assert_eq!(next.id, 3);
        assert_eq!(publish_prepared(next, &book().unwrap()), Ok(()));
        assert_eq!(rounds().iter().map(|r| r.id).collect::<Vec<_>>(), [3]);
        reset_for_tests();
        for f in [&legacy, &own, &career] {
            let _ = std::fs::remove_file(f);
        }
    }

    #[test]
    fn the_buff_goes_on_living_champions_with_a_power_once() {
        let mut power = BTreeMap::new();
        power.insert("maddie".to_string(), -10);
        power.insert("weak".to_string(), 6);
        let champions = vec![
            (1, "maddie".to_string(), true, false),
            (2, "weak".to_string(), true, true),
            (3, "fine".to_string(), true, false),
            (4, "maddie".to_string(), false, false),
            (5, "weak".to_string(), true, false),
        ];
        // maddie alive without it, the respawned weak (a new entity); not the one carrying it, not the dead
        assert_eq!(missing(&champions, &power), [(1, -10), (5, 6)]);
    }

    #[test]
    fn the_buff_puts_half_on_damage_and_the_rest_on_health() {
        let b = buff(-15);
        assert_eq!((b.attack_mult, b.magic_power_mult, b.hp_mult), (-7, -7, -8));
        assert_eq!(b.name(), BUFF);
        let (d, h) = factors(-15);
        assert!((d - 0.93).abs() < 1e-9 && (h - 0.92).abs() < 1e-9, "{d} {h}");
        let b = buff(6);
        assert_eq!((b.attack_mult, b.magic_power_mult, b.hp_mult), (3, 3, 3));
    }

    #[test]
    fn each_round_is_a_note_of_its_patch() {
        let round = |id: u32, version: &str, changes: Vec<(&str, i32)>| Round {
            id,
            version: version.into(),
            changes: changes
                .into_iter()
                .map(|(c, s)| Change { champion: c.into(), win: 50.0, evidence: 50.0, step: s, total: s, unpicked: false, reset: None, theme: None })
                .collect(),
            ..Round::default()
        };
        let notes = notes_of(&[round(1, "1.2", vec![("a", -5), ("b", 4)]), round(2, "1.2", vec![("c", -3)]), round(3, "1.3", vec![("a", -2)])]);
        assert_eq!(notes.len(), 3);
        assert!(notes[0].is_for("1.2") && notes[0].direction("a") == -1 && notes[0].direction("b") == 1);
        assert!(notes[1].is_for("1.2") && notes[1].direction("c") == -1);
        assert_eq!(notes[2].version.as_deref(), Some("1.3"));
    }

    #[test]
    fn the_plain_news_lists_nerfs_then_buffs_in_the_games_language() {
        let round = Round {
            id: 2,
            version: "2027.4.1".into(),
            label: "2027.4.1b".into(),
            changes: vec![
                Change { champion: "tft_maddie".into(), win: 73.3, evidence: 221.0, step: -10, total: -10, unpicked: false, reset: None, theme: None },
                Change { champion: "weak".into(), win: 44.0, evidence: 80.0, step: 6, total: 6, unpicked: false, reset: None, theme: None },
            ],
            ..Round::default()
        };
        let cn = |n: &str| if n == "tft_maddie" { "麦迪".to_string() } else { "某英雄".to_string() };
        let (title, body) = news(&round, &cn);
        assert_eq!(title, "电竞大脑平衡补丁 2027.4.1b");
        assert!(body.starts_with("【削弱】\n麦迪：胜率 73.3%（目标 48%–52%）→ 强度 -10%（攻击/法强 -5%，生命 -5%），累计 -10%"), "{body}");
        assert!(body.contains("【加强】\n某英雄：胜率 44.0%"), "{body}");
        let (title, body) = news(&round, &|n: &str| n.to_string());
        assert!(title == "Patch Meta AI balance patch 2027.4.1b" && body.starts_with("[Nerfs]"), "{title} / {body}");
    }

    // ===================== 2.9.0: items, the log, the wider limits =====================

    const ITEM_RULES: Rules = Rules { items: true, ..RULES };

    fn builder(name: &str, win: f32, evidence: f32, items: &[&str]) -> Builder {
        Builder { champion: name.into(), win, evidence, items: items.iter().map(|s| s.to_string()).collect() }
    }

    fn names(keys: &[&str]) -> BTreeMap<String, String> {
        keys.iter().map(|k| (k.to_string(), format!("名字{k}"))).collect()
    }

    #[test]
    fn an_item_whose_builders_win_too_much_is_cut() {
        let always = |_: &str| true;
        let before = BTreeMap::new();
        // 2.12.0: five builders at 54% against the field: 4 over 50, drawn to 4 x 5/9 = 2.2, 1.2 beyond the band of 1,
        // 10% a point: a step of -12
        let five: Vec<Builder> = (0..5).map(|i| builder(&format!("c{i}"), 54.0, 100.0, &["rfc"])).collect();
        let out = decide_items(&five, &names(&["rfc"]), &before, &always, ITEM_RULES);
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].item.as_str(), out[0].users, out[0].step, out[0].total), ("rfc", 5, -12, -12));
        assert!((out[0].win - 54.0).abs() < 1e-4, "the notes give the builders' win rate as it is");
        // 2.9.0's numbers (4 builders, 48-52, 2% a point) left it alone: 0.2 beyond their band, -0.4 rounds to 0
        let old = Rules { item_users: 4, item_band: 2.0, item_per_point: 2.0, item_count: 5, item_max: 10, ..ITEM_RULES };
        assert!(decide_items(&five, &names(&["rfc"]), &before, &always, old).is_empty());
        // three builders are enough now (4 before): 4 x 3/7 = 1.7, a step of -7; two are too few, and builders with few
        // games do not count
        let three: Vec<Builder> = (0..3).map(|i| builder(&format!("c{i}"), 54.0, 100.0, &["rfc"])).collect();
        assert_eq!(decide_items(&three, &names(&["rfc"]), &before, &always, ITEM_RULES)[0].step, -7);
        let two: Vec<Builder> = (0..2).map(|i| builder(&format!("c{i}"), 70.0, 100.0, &["rfc"])).collect();
        assert!(decide_items(&two, &names(&["rfc"]), &before, &always, ITEM_RULES).is_empty());
        let mut thin = two.clone();
        thin.push(builder("new", 70.0, MIN_EVIDENCE - 1.0, &["rfc"]));
        assert!(decide_items(&thin, &names(&["rfc"]), &before, &always, ITEM_RULES).is_empty());
        // within the band of 1 once drawn toward 50: left alone
        let near: Vec<Builder> = (0..5).map(|i| builder(&format!("c{i}"), 51.5, 100.0, &["rfc"])).collect();
        assert!(decide_items(&near, &names(&["rfc"]), &before, &always, ITEM_RULES).is_empty());
        // far off: held to item_max a round (20 by default since 2.12.0), ITEM_CAP in all (still 50 where the
        // champions' CAP is 80); weak builders get the item buffed
        let far: Vec<Builder> = (0..20).map(|i| builder(&format!("c{i}"), 80.0, 500.0, &["rfc", "weak"])).collect();
        let out = decide_items(&far, &names(&["rfc"]), &before, &always, ITEM_RULES);
        assert_eq!((out[0].step, out[0].total), (-DEFAULT_ITEM_MAX, -DEFAULT_ITEM_MAX));
        let was: BTreeMap<String, i32> = [("rfc".to_string(), -(ITEM_CAP - 4))].into_iter().collect();
        let out = decide_items(&far, &names(&["rfc"]), &was, &always, ITEM_RULES);
        assert_eq!((out[0].step, out[0].total), (-4, -50));
        let weak: Vec<Builder> = (0..8).map(|i| builder(&format!("c{i}"), 40.0, 100.0, &["staff"])).collect();
        assert!(decide_items(&weak, &names(&["staff"]), &before, &always, ITEM_RULES)[0].step > 0);
        // an item that has to wait, and the round's count keeps the furthest
        let wait = |item: &str| item != "rfc";
        assert!(decide_items(&five, &names(&["rfc"]), &before, &wait, ITEM_RULES).is_empty());
        let mut mixed = five.clone();
        mixed.extend((0..5).map(|i| builder(&format!("d{i}"), 70.0, 100.0, &["bc"])));
        let out = decide_items(&mixed, &names(&["rfc", "bc"]), &before, &always, Rules { item_count: 1, ..ITEM_RULES });
        assert_eq!(out.iter().map(|c| c.item.as_str()).collect::<Vec<_>>(), ["bc"]);
    }

    #[test]
    fn the_champions_count_what_the_item_steps_do() {
        // 2.12.0: an item cut by 20 at 10% a point is expected to close 2 points of each builder's win rate, a buff of
        // 5 to open half a point; power taken off at once is no such step
        let builds: crate::items::Builds =
            [("a".to_string(), vec!["rfc".to_string(), "bc".to_string()]), ("b".to_string(), vec!["staff".to_string()])].into_iter().collect();
        let change = |item: &str, step: i32, reset: Option<Reset>| ItemChange { item: item.into(), name: item.into(), users: 3, win: 60.0, step, total: step, reset };
        let effects = item_effects(&[change("rfc", -20, None), change("bc", 5, None), change("staff", -10, Some(Reset::Skip))], &builds, ITEM_RULES);
        assert_eq!(effects.get("a").copied(), Some(-1.5));
        assert_eq!(effects.get("b"), None);
    }

    fn inputs(builds: &[(&str, &[&str])]) -> crate::items::Inputs {
        let b: crate::items::Builds =
            builds.iter().map(|(h, items)| (h.to_string(), items.iter().map(|s| s.to_string()).collect())).collect();
        let names = builds.iter().flat_map(|(_, items)| items.iter()).map(|k| (k.to_string(), format!("名字{k}"))).collect();
        crate::items::Inputs { builds: Arc::new(b), names }
    }

    #[test]
    fn a_round_changes_items_with_the_champions_and_keeps_their_power() {
        let mut m = meta();
        // four more strong builders of one item; maddie (73%) builds it too
        for i in 0..4 {
            m.champions.push(champ(&format!("adc{i}"), 64.0, 120.0));
        }
        // the middle of the field stays at 50%: champions at 50% play the most
        for i in 0..3 {
            m.champions.push(champ(&format!("mid{i}"), 50.0, 300.0));
        }
        let builds: Vec<(&str, &[&str])> = vec![
            ("maddie", &["rfc"]),
            ("adc0", &["rfc"]),
            ("adc1", &["rfc"]),
            ("adc2", &["rfc"]),
            ("adc3", &["rfc"]),
            ("fine", &["staff"]),
        ];
        let inp = inputs(&builds);
        let today = Some((2027, 10, 1));
        let round = make_round_with(&m, &[], ITEM_RULES, &Due::First("2027.4.1".into()), today, Some(&inp), &Skip::default());
        assert!(round.changes.iter().any(|c| c.champion == "maddie"), "the champions as before");
        assert_eq!(round.items.len(), 1, "{:?}", round.items);
        assert_eq!(round.items[0].item, "rfc");
        assert_eq!(round.item_power.get("rfc"), Some(&round.items[0].total));
        // the item balance off: no item change, the power so far carried on
        let next = make_round_with(&m, std::slice::from_ref(&round), RULES, &Due::Late("2027.4.1".into()), today, Some(&inp), &Skip::default());
        assert!(next.items.is_empty());
        assert_eq!(next.item_power, round.item_power);
        // the item just changed waits for its builders' games
        let again = make_round_with(&m, std::slice::from_ref(&round), ITEM_RULES, &Due::Late("2027.4.1".into()), today, Some(&inp), &Skip::default());
        assert!(again.items.is_empty(), "no games since: {:?}", again.items);
        // the log says why these champions and no more
        assert!(round.summary.starts_with("10 champions rated"), "{}", round.summary);
        assert!(round.summary.contains("changed (balance_count 10)"), "{}", round.summary);
        // the items come first (2.12.0), on their builders' win rates as they are: the champions' balance_per_point
        // (2.10.0 - 2.11.x: the step a builder's own cut was expected to close) leaves the item's step alone
        let item = |per_point: f32| {
            let rules = Rules { per_point, ..ITEM_RULES };
            make_round_with(&m, &[], rules, &Due::First("2027.4.1".into()), today, Some(&inp), &Skip::default()).items[0].step
        };
        assert_eq!(item(1.5), item(2.5));
        // and the champions count the item's step: a builder's cut is smaller by the points the item is expected to
        // close (its step / item_balance_per_point), at balance_per_point a point
        let open = Rules { max_step: 50, ..ITEM_RULES };
        let step_of = |rules: Rules, who: &str| {
            let r = make_round_with(&m, &[], rules, &Due::First("2027.4.1".into()), today, Some(&inp), &Skip::default());
            (r.changes.iter().find(|c| c.champion == who).map_or(0, |c| c.step), r.items.first().map_or(0, |c| c.step))
        };
        let (with_item, item_step) = step_of(open, "adc0");
        let (without, _) = step_of(Rules { items: false, ..open }, "adc0");
        let expected = (item_step as f32 / DEFAULT_ITEM_PER_POINT * DEFAULT_PER_POINT).round() as i32;
        assert!(item_step < 0 && without < 0, "{item_step} {without}");
        assert_eq!(with_item - without, -expected, "the item {item_step}: adc0 {with_item} against {without} without it");
    }

    #[test]
    fn the_log_says_why_a_round_changed_fewer_than_balance_count() {
        let c = |name: &'static str, win: f32, evidence: f32, since: Option<f32>| Candidate { name, win, evidence, since_change: since, pooled: true };
        let cands = [
            c("a", 70.0, 100.0, None),
            c("b", 65.0, 100.0, Some(5.0)),
            c("cap", 60.0, 100.0, None),
            c("near", 52.1, 100.0, None),
            c("few", 75.0, 10.0, None),
            c("in", 50.0, 100.0, None),
        ];
        let before: BTreeMap<String, i32> = [("cap".to_string(), -CAP)].into_iter().collect();
        let rules = Rules { count: 60, max_step: 50, ..RULES };
        let could = qualified(&cands, &before, rules);
        let text = summarize(&cands, &before, rules, could.len(), could.len().min(rules.count));
        assert!(text.starts_with("6 champions rated, 5 with 30 games or more, 4 of those outside 48-52%"), "{text}");
        assert!(text.contains("1 waiting") && text.contains("1 at the 80% limit") && text.contains("1 too close"), "{text}");
        assert!(text.contains("1 could change, 1 changed (balance_count 60)"), "{text}");
    }

    #[test]
    fn the_item_news_is_laid_out_like_the_games_patch_notes() {
        let mut round = Round { id: 4, version: "2027.5.0".into(), label: "2027.5.0".into(), ..Round::default() };
        let no_icon = |_: &ItemChange| String::new();
        assert_eq!(item_news(&round, true, &|_| Vec::new(), &no_icon), None, "no item changed: no news");
        round.items = vec![
            ItemChange { item: "staff".into(), name: "大天使之杖".into(), users: 6, win: 43.5, step: 6, total: 6, reset: None },
            ItemChange { item: "rfc".into(), name: "疾射火炮".into(), users: 4, win: 64.6, step: -10, total: -20, reset: None },
        ];
        let lines = |c: &ItemChange| {
            if c.item == "rfc" {
                vec![("<i#asset/base/ui/banpick/champion_stat_icon:attack_speed_0>".to_string(), "攻击速度".to_string(), "31".to_string(), "28".to_string())]
            } else {
                Vec::new()
            }
        };
        let (title, content) = item_news(&round, true, &lines, &no_icon).unwrap();
        assert_eq!(title, "电竞大脑装备平衡 2027.5.0");
        let rows: Vec<&str> = content.lines().collect();
        assert!(rows[0].starts_with("这一版调整了以下装备的数值") && !content.contains("仍是原来的数值"), "{content}");
        // 先增强后削弱，每件一段：粗体名字、一句理由、一行一个数值、整体强度
        assert_eq!(rows[2], format!("{BOLD}增强{FONT_END}"));
        assert_eq!(rows[4], format!("{BOLD}大天使之杖{FONT_END}"));
        assert!(rows[5].starts_with(&format!("<{NEWS_REASON}>")) && rows[5].contains("44%"), "{}", rows[5]);
        assert_eq!(rows[6], format!("整体强度: 0% {ARROW} <{NEWS_UP}>+6%<>"));
        assert_eq!(rows[8], format!("{BOLD}削弱{FONT_END}"));
        assert_eq!(rows[10], format!("{BOLD}疾射火炮{FONT_END}"));
        assert!(rows[11].contains("65%"), "{}", rows[11]);
        assert_eq!(rows[12], format!("<i#asset/base/ui/banpick/champion_stat_icon:attack_speed_0>攻击速度: 31 {ARROW} <{NEWS_DOWN}>28<>"));
        assert_eq!(rows[13], format!("整体强度: -10% {ARROW} <{NEWS_DOWN}>-20%<>"));
        assert_eq!(rows.len(), 14);
        let (title, content) = item_news(&round, false, &lines, &no_icon).unwrap();
        assert_eq!(title, "Patch Meta AI item balance 2027.5.0");
        assert!(content.contains(&format!("{BOLD}Nerfs{FONT_END}")) && content.contains("Overall power: -10%"), "{content}");
        // 装备图标在名字前面（2.11.0）
        let icon = |c: &ItemChange| crate::items::icon_text(if c.item == "rfc" { "i_t5_rfc" } else { "" });
        let (_, content) = item_news(&round, true, &lines, &icon).unwrap();
        assert!(content.contains(&format!("<i#asset/base/aseprite_resources/ingame/item_icons_18x18:i_t5_rfc>{BOLD}疾射火炮")), "{content}");
        // 同一件装备同一轮总是同一句理由
        let staff = &round.items[0];
        assert_eq!(item_reason(staff, seed("staff", "2027.5.0"), true), item_reason(staff, seed("staff", "2027.5.0"), true));
    }

    #[test]
    fn item_reasons_follow_the_items_role_and_history() {
        use ItemRole::*;
        assert_eq!(item_role(&["magic_power", "magic_resistance_penetration"]), Mage);
        assert_eq!(item_role(&["magic_power", "defence"]), MageGuard);
        assert_eq!(item_role(&["attack", "crit_chance"]), Marksman);
        assert_eq!(item_role(&["attack", "defence_penetration"]), Assassin);
        assert_eq!(item_role(&["attack", "hp", "skill_cooldown_mult"]), Fighter);
        assert_eq!(item_role(&["hp", "magic_resistance"]), Tank);
        assert_eq!(item_role(&["skill_cooldown_mult", "move_speed_mult"]), Other);
        assert_eq!(item_role(&["magic_power", "attack_speed_mult"]), Other, "hybrid");
        let change = |step: i32, total: i32| ItemChange { item: "x".into(), name: "某装备".into(), users: 5, win: 46.6, step, total, reset: None };
        for k in 0..2u64 {
            for (step, total) in [(8, 8), (-15, -15), (8, -11), (-6, 4), (3, 3), (-20, -20)] {
                for chinese in [true, false] {
                    let r = item_reason(&change(step, total), k, chinese);
                    assert!(!r.contains("{adv}") && !r.starts_with("出它的英雄"), "{r}");
                    assert!(r.ends_with(if chinese { "（出装胜率 47%）" } else { "(builders win 47%)" }), "{r}");
                }
            }
            // 之前削多了又加回来、加多了又削回去：直说是回调
            let back = item_reason(&change(8, -11), k, true);
            assert!(back.contains("还给它") || back.contains("回调"), "{back}");
            let back = item_reason(&change(-6, 4), k, true);
            assert!(back.contains("收回") || back.contains("回调"), "{back}");
        }
        // 幅度跟着这一刀走（没有装备表：「别的」那两句）
        let big = item_reason(&change(20, 20), 0, true);
        assert!(big.contains("大幅"), "{big}");
        let small = item_reason(&change(-3, -3), 0, true);
        assert!(small.contains("略微"), "{small}");
    }

    #[test]
    fn a_matchs_runs_play_its_items_unless_its_first_run_had_them_off() {
        let _serial = crate::tests::serial();
        reset_for_tests();
        let file = std::env::temp_dir().join(format!("pma_items_test_{}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        open_file(file.clone());
        let power = [("maddie".to_string(), -10)].into_iter().collect();
        let item_power = [("rfc".to_string(), -10)].into_iter().collect();
        publish(Round { id: 1, version: "1.1".into(), power, item_power, ..Round::default() });
        let items_of = |p: Option<RunPower>| p.and_then(|p| p.items).map(|m| m.get("rfc").copied());
        // the server's run with the item balance on: every run of the match plays the items
        assert_eq!(items_of(round_for("on".into(), true, true, true, true)), Some(Some(-10)));
        assert_eq!(items_of(round_for("on".into(), false, true, false, true)), Some(Some(-10)), "a later run follows the first");
        // with it off: the champions' power, no items - for its replays too, after the book is read again
        let off = round_for("off".into(), true, true, false, true).expect("a run power");
        assert_eq!(off.champions.get("maddie"), Some(&-10));
        assert!(off.items.is_none());
        assert_eq!(items_of(round_for("off".into(), false, true, true, true)), None);
        close();
        open_file(file.clone());
        assert_eq!(items_of(round_for("off".into(), false, true, true, true)), None);
        assert_eq!(items_of(round_for("on".into(), false, true, true, true)), Some(Some(-10)));
        let _ = std::fs::remove_file(&file);
        reset_for_tests();
    }

    #[test]
    fn the_model_reads_each_rounds_steps() {
        let round = Round {
            id: 1,
            version: "2027.4.1".into(),
            merged: false,
            matches: 586,
            changes: vec![Change { champion: "maddie".into(), win: 70.0, evidence: 150.0, step: -10, total: -10, unpicked: false, reset: None, theme: None }],
            ..Round::default()
        };
        let got = model_rounds_of(&[round]);
        assert_eq!(
            got,
            [crate::meta::BalanceRound { version: "2027.4.1".into(), merged: false, matches: 586, steps: vec![("maddie".into(), -10)] }]
        );
        // the round log line reads as one line
        let c = |name: &'static str, win: f32| Candidate { name, win, evidence: 100.0, since_change: Some(5.0), pooled: true };
        let text = summarize(&[c("a", 70.0), c("b", 30.0)], &BTreeMap::new(), RULES, 0, 0);
        assert!(!text.contains("  "), "{text}");
        assert!(text.contains("2 waiting for 30 games with their new numbers"), "{text}");
    }

    #[test]
    fn win_rates_count_from_the_middle_of_the_field() {
        let mut m = meta();
        for i in 0..3 {
            m.champions.push(champ(&format!("mid{i}"), 50.0, 300.0));
        }
        let due = Due::First("2027.4.1".into());
        let plain = make_round(&m, &[], RULES, &due, None);
        let steps = |r: &Round| r.changes.iter().map(|c| (c.champion.clone(), c.step)).collect::<Vec<_>>();
        assert_eq!(steps(&plain), [("maddie".to_string(), -10), ("weak".to_string(), 10)]);
        assert!(plain.summary.ends_with("the model's 50.0% counts as 50%"), "{}", plain.summary);
        // the model's strength 0 is not the field's: every champion a step stronger changes as before
        let mut up = m.clone();
        for c in &mut up.champions {
            c.strength += 0.4;
        }
        assert!((field_centre(&up) - 0.4).abs() < 1e-6);
        let shifted = make_round(&up, &[], RULES, &due, None);
        assert_eq!(steps(&shifted), steps(&plain));
        assert!((shifted.changes[0].win - plain.changes[0].win).abs() < 1e-3, "the notes give the win against the field");
        assert!(shifted.summary.ends_with("the model's 59.9% counts as 50%"), "{}", shifted.summary);
        // the field strong as a whole: champions at the model's 50% are its weak end, and get buffed
        let mut strong = m.clone();
        for c in strong.champions.iter_mut().filter(|c| c.name.starts_with("mid")) {
            c.strength = logit(0.58);
        }
        let out = make_round(&strong, &[], RULES, &due, None);
        assert!(out.changes.iter().any(|c| c.champion == "fine" && c.step > 0), "{:?}", steps(&out));
        // this patch's picks set the middle once there are enough of them - the median: maddie in every game does
        // not move it
        let mut picked = m.clone();
        for c in &mut picked.champions {
            c.current.games = if c.name == "maddie" { 100 } else { 60 };
        }
        assert_eq!(field_centre(&picked), 0.0);
        for c in &mut picked.champions {
            c.current.games = if c.name == "maddie" { 400 } else { 10 };
        }
        assert!(field_centre(&picked) > 0.9, "picks alone: maddie is the field");
        for c in &mut picked.champions {
            c.current.games = 5;
        }
        assert_eq!(field_centre(&picked), 0.0, "a patch just begun: the recent games decide");
        assert_eq!(field_centre(&Meta::default()), 0.0);
    }

    // ===================== 2.10.0: unpicked buffs =====================

    fn pooled(name: &str, win: f32, evidence: f32) -> Candidate<'_> {
        Candidate { name, win, evidence, since_change: None, pooled: true }
    }

    #[test]
    fn champions_with_too_few_games_get_unpicked_buffs() {
        let before: BTreeMap<String, i32> =
            [("buffed".to_string(), 25), ("full".to_string(), 30), ("cut".to_string(), -40)].into_iter().collect();
        let cands = [
            pooled("none", 45.0, 0.0),
            pooled("none_weak", 40.0, 0.0),
            pooled("few", 49.0, 12.0),
            // above 50% against the field: no
            pooled("strong", 55.0, 3.0),
            // 30 games: the win-rate rule's
            pooled("enough", 45.0, 30.0),
            // never picked or banned on this save (a base hero under a LoL-only filter)
            Candidate { pooled: false, ..pooled("base", 30.0, 0.0) },
            // +25 so far: +5 to the rule's +30
            pooled("buffed", 48.0, 20.0),
            // +30: no more through this rule
            pooled("full", 47.0, 1.0),
            // cut once, nobody plays it now: buffed too
            pooled("cut", 44.0, 25.0),
        ];
        let (out, eligible) = unpicked(&cands, &before, Rules { unpicked: 3, ..RULES });
        assert_eq!(eligible, 5, "none, none_weak, few, buffed, cut");
        // the fewest games first, then the weakest
        let got: Vec<(&str, i32, i32)> = out.iter().map(|c| (c.champion.as_str(), c.step, c.total)).collect();
        assert_eq!(got, [("none_weak", 10, 10), ("none", 10, 10), ("few", 10, 10)]);
        assert!(out.iter().all(|c| c.unpicked));
        // room for all: each to its limit
        let (out, _) = unpicked(&cands, &before, Rules { unpicked: 60, ..RULES });
        let got: Vec<(&str, i32, i32)> = out.iter().map(|c| (c.champion.as_str(), c.step, c.total)).collect();
        assert_eq!(got, [("none_weak", 10, 10), ("none", 10, 10), ("few", 10, 10), ("buffed", 5, 30), ("cut", 10, -30)]);
        // off; and a step held to balance_max
        assert!(unpicked(&cands, &before, RULES).0.is_empty(), "balance_unpicked 0");
        assert_eq!(unpicked(&cands, &before, Rules { unpicked: 1, max_step: 4, ..RULES }).0[0].step, 4);
        // once one has its 30 games the win-rate rule takes it over - and cuts back what was too much
        let played = Candidate { since_change: Some(40.0), ..pooled("buffed", 60.0, 100.0) };
        assert!(unpicked(&[played], &before, Rules { unpicked: 60, ..RULES }).0.is_empty());
        let out = decide(&[played], &before, RULES);
        assert_eq!((out[0].step, out[0].total, out[0].unpicked), (-10, 15, false));
    }

    #[test]
    fn a_round_makes_unpicked_buffs_besides_balance_count() {
        let mut m = meta();
        for c in &mut m.champions {
            c.drafted = 50;
        }
        // three champions with too few games: two drafted on this save, one never
        for (name, win, evidence, drafted) in [("rare", 45.0, 4.0, 3), ("new", 47.0, 0.0, 1), ("base", 40.0, 0.0, 0)] {
            m.champions.push(crate::meta::Champion { drafted, ..champ(name, win, evidence) });
        }
        let rules = Rules { count: 1, unpicked: 5, ..RULES };
        let round = make_round(&m, &[], rules, &Due::First("2027.4.1".into()), None);
        let got: Vec<(&str, i32, bool)> = round.changes.iter().map(|c| (c.champion.as_str(), c.step, c.unpicked)).collect();
        // balance_count 1: maddie by her win rate; the unpicked ones on top of it, the never-drafted one not
        assert_eq!(got, [("maddie", -10, false), ("new", 10, true), ("rare", 10, true)]);
        assert_eq!((round.power.get("new"), round.power.get("rare"), round.power.get("base")), (Some(&10), Some(&10), None));
        assert!(round.summary.contains("1 changed (balance_count 1); 2 unpicked buffs of 2 eligible (balance_unpicked 5"), "{}", round.summary);
        assert!(round.summary.ends_with("counts as 50%"), "{}", round.summary);
        // the model reads their steps like any other's
        let steps = &model_rounds_of(std::slice::from_ref(&round))[0].steps;
        assert!(steps.contains(&("new".to_string(), 10)) && steps.contains(&("maddie".to_string(), -10)), "{steps:?}");
        // the notes say why, not a win rate (2.14.2: the game's own words for it, then the games line)
        let reason_of = |name: &str| reason(round.changes.iter().find(|c| c.champion == name).unwrap(), &Flavor::default());
        assert_eq!(reason_of("new"), "unpicked_1,pma_unpicked");
        // and so does the plain news
        let (_, body) = news(&round, &|n: &str| if n == "new" { "新英雄".to_string() } else { "某英雄".to_string() });
        assert!(body.contains("新英雄：近几个版本很少有人选（近期 0 场）→ 强度 +10%（攻击/法强 +5%，生命 +5%），累计 +10%"), "{body}");
        // the next round: still unpicked, buffed again up to +30 in all
        let next = make_round(&m, std::slice::from_ref(&round), rules, &Due::Late("2027.4.1".into()), None);
        assert_eq!(next.power.get("new"), Some(&20));
        // an unpicked change is kept in the book as such; the others as before
        let json = serde_json::to_string(&round.changes).unwrap();
        assert!(json.contains(r#""unpicked":true"#) && json.matches("unpicked").count() == 2, "{json}");
        let back: Vec<Change> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, round.changes);
        let old: Change = serde_json::from_str(r#"{"champion":"x","win":50.0,"evidence":40.0,"step":-5,"total":-5}"#).unwrap();
        assert!(!old.unpicked, "a book of 2.9.0");
    }

    // ===================== 2.10.0: the recalibration hotfix =====================

    #[test]
    fn a_book_from_before_2_10_gets_one_recalibration_hotfix() {
        let today = Some((2028, 3, 20));
        // a book of 2.9.0: its rounds wrote no rules; the newest a game patch's, maddie cut in its notes
        let old: Round = serde_json::from_str(
            r#"{"id":9,"version":"2028.2.0","label":"2028.2.0","merged":true,"date":[2028,3,15],"matches":250,
                "changes":[{"champion":"maddie","win":70.0,"evidence":200.0,"step":-5,"total":-5}],"power":{"maddie":-5}}"#,
        )
        .unwrap();
        assert_eq!(old.rules, 0);
        let rounds = vec![old];
        // due at once (five days after the round, long before LATE_DAYS): a hotfix of the game's current patch
        let now = due(&rounds, Some("2028.2.0"), today).unwrap();
        assert_eq!(now, Due::Recalibration("2028.2.0".into()));
        let m = meta_with_games(&["2028.1.2", "2028.2.0"], &[300, 10]);
        let fix = make_round(&m, &rounds, RULES, &now, today);
        assert_eq!((fix.id, fix.label.as_str(), fix.merged, fix.rules), (10, "2028.2.0b", false, RULES_VERSION));
        // the settled rule still holds: maddie has 10 games since her cut; weak, never changed, is buffed
        assert!(fix.changes.iter().all(|c| c.champion != "maddie"), "{:?}", fix.changes);
        assert_eq!((fix.power.get("maddie"), fix.power.get("weak")), (Some(&-5), Some(&10)));
        assert_eq!(fix.held.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["maddie"]);
        // once: after it nothing more until the game's next patch (nor by age before LATE_DAYS)
        let rounds = vec![rounds[0].clone(), fix];
        assert_eq!(due(&rounds, Some("2028.2.0"), today), None);
        assert_eq!(due(&rounds, Some("2028.2.0"), Some((2028, 4, 30))), None);
        assert_eq!(due(&rounds, Some("2028.2.1"), today), Some(Due::Patch("2028.2.1".into())));
        // the number is kept in the book: read again, still nothing due
        let json = serde_json::to_string(&rounds[1]).unwrap();
        assert!(json.contains(r#""rules":2"#), "{json}");
        let read: Round = serde_json::from_str(&json).unwrap();
        assert_eq!(due(&[rounds[0].clone(), read], Some("2028.2.0"), today), None);
        // the game's patch due at the same time comes first: its round is made under the new rules as well
        let old_only = &rounds[..1];
        assert_eq!(due(old_only, Some("2028.2.1"), today), Some(Due::Patch("2028.2.1".into())));
        let patched = make_round(&m, old_only, RULES, &Due::Patch("2028.2.1".into()), today);
        assert_eq!((patched.merged, patched.rules), (true, RULES_VERSION));
        assert_eq!(due(&[old_only[0].clone(), patched], Some("2028.2.1"), today), None);
        // no patch known: nothing yet; an empty book: the first round, as before
        assert_eq!(due(old_only, None, today), None);
        assert_eq!(due(&[], Some("2028.2.0"), today), Some(Due::First("2028.2.0".into())));
    }

    // ===================== 2.11.0: the player's numbers and the skip lists =====================

    fn powered(power: &[(&str, i32)], items: &[(&str, i32)]) -> Round {
        Round {
            id: 4,
            version: "2028.2.0".into(),
            label: "2028.2.0".into(),
            merged: true,
            date: Some((2028, 3, 1)),
            rules: RULES_VERSION,
            power: power.iter().map(|(c, p)| (c.to_string(), *p)).collect(),
            item_power: items.iter().map(|(c, p)| (c.to_string(), *p)).collect(),
            ..Round::default()
        }
    }

    #[test]
    fn a_reset_takes_the_power_off_at_once_as_a_hotfix() {
        let today = Some((2028, 3, 20));
        let rounds = vec![powered(&[("jinx", -20), ("zeri", 10)], &[("ie", -10)])];
        // jinx: the player changed her numbers; ezreal too (no power: listed, step 0); zeri: balance_skip; one skipped
        // without power (nothing to say); an item skipped
        let champions = [
            ("jinx".to_string(), Reset::Edit),
            ("ezreal".to_string(), Reset::Edit),
            ("zeri".to_string(), Reset::Skip),
            ("nopower".to_string(), Reset::Skip),
        ];
        let reset = make_reset(&rounds, &champions, &[("ie".into(), "无尽之刃".into(), Reset::Skip)], &[], today).unwrap();
        assert!(reset.reset && !reset.merged);
        assert_eq!((reset.id, reset.label.as_str(), reset.version.as_str(), reset.date), (5, "2028.2.0b", "2028.2.0", today));
        assert!(reset.power.is_empty() && reset.item_power.is_empty(), "{:?} {:?}", reset.power, reset.item_power);
        let got: Vec<(&str, i32, i32, Option<Reset>)> = reset.changes.iter().map(|c| (c.champion.as_str(), c.step, c.total, c.reset)).collect();
        assert_eq!(got, [("jinx", 20, 0, Some(Reset::Edit)), ("ezreal", 0, 0, Some(Reset::Edit)), ("zeri", -10, 0, Some(Reset::Skip))]);
        assert_eq!((reset.items[0].step, reset.items[0].total, reset.items[0].reset), (10, 0, Some(Reset::Skip)));
        // the notes' reasons: why, no win rate; how much is the power line (2.14.2)
        assert_eq!(reason(&reset.changes[0], &Flavor::default()), "pma_custom");
        assert_eq!(reason(&reset.changes[2], &Flavor::default()), "pma_skip");
        assert_eq!(power_row(&reset.changes[0]).map(|r| (r.pre_value, r.after_value)), Some(("-20%".to_string(), "0%".to_string())));
        // the notes' lines: the numbers with this mod's power, then the save's own (the player's)
        let stats = Stats { category: None, base: [70.0, 0.0, 720.0, 16.0, 12.0, 810.0], growth: [20.0, 0.0, 90.0, 0.0, 0.0, 0.0] };
        let lines = rows(&stats, reset.changes[0].total - reset.changes[0].step, reset.changes[0].total);
        let attack = lines.iter().find(|r| r.key.ends_with("stat.attack")).unwrap();
        assert_eq!((attack.pre_value.as_str(), attack.after_value.as_str(), attack.is_buff), ("63", "70", true));
        // no news cards; the item news says why
        assert!(articles(&reset, &BTreeMap::new(), "T1", &[], &|n: &str| n.to_string()).is_empty());
        let (_, body) = item_news(&reset, true, &|_| Vec::new(), &|_| String::new()).unwrap();
        assert!(body.contains("无尽之刃") && body.contains("item_balance_skip") && body.contains("整体强度: -10%"), "{body}");
        // nothing to take off or say: no round; no round yet: no power at all
        assert!(make_reset(&rounds, &[("nopower".into(), Reset::Skip)], &[], &[], today).is_none());
        assert!(make_reset(&[], &[("jinx".into(), Reset::Edit)], &[], &[], today).is_none());
        // a round of resets is not the balance's: the late hotfix's timer and the rules' check look past it
        let mut book = rounds.clone();
        book.push(reset.clone());
        assert_eq!(due(&book, Some("2028.2.0"), Some((2028, 4, 29))), None, "59 days after the balance round");
        assert_eq!(due(&book, Some("2028.2.0"), Some((2028, 4, 30))), Some(Due::Late("2028.2.0".into())));
        assert_eq!(due(&book, Some("2028.3.0"), today), Some(Due::Patch("2028.3.0".into())));
        // the next balance round starts from the reset's power, and the champion waits for games with its new numbers
        let mut m = meta();
        m.champions.push(champ("jinx", 65.0, 300.0));
        let next = make_round(&m, &book, RULES, &Due::Patch("2028.3.0".into()), today);
        assert!(!next.reset && next.id == 6);
        assert!(!next.changes.iter().any(|c| c.champion == "jinx"), "jinx waits for games after the reset: {:?}", next.changes);
        // kept in the book as such; a round without resets writes nothing new
        let json = serde_json::to_string(&book).unwrap();
        let back: Vec<Round> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, book);
        let plain = serde_json::to_string(&rounds).unwrap();
        assert!(!plain.contains("reset") && json.contains(r#""reset":"edit""#) && json.contains(r#""reset":true"#), "{json}");
    }

    #[test]
    fn balance_skip_leaves_a_champion_alone_and_takes_its_power_off() {
        let m = meta();
        let rounds = vec![Round { version: "2027.4.1".into(), ..powered(&[("maddie", -10)], &[]) }];
        let skip = Skip { champions: ["maddie".to_string()].into(), items: Default::default() };
        let round = make_round_with(&m, &rounds, RULES, &Due::Patch("2027.4.2".into()), None, None, &skip);
        // maddie (73%) would be cut again; she is listed: her power comes off instead, nothing else changes her
        let maddie: Vec<&Change> = round.changes.iter().filter(|c| c.champion == "maddie").collect();
        assert_eq!(maddie.len(), 1, "{:?}", round.changes);
        assert_eq!((maddie[0].step, maddie[0].total, maddie[0].reset), (10, 0, Some(Reset::Skip)));
        assert!(!round.power.contains_key("maddie") && round.changes.iter().any(|c| c.champion == "weak"));
        // without the list she is cut as before
        let plain = make_round_with(&m, &rounds, RULES, &Due::Patch("2027.4.2".into()), None, None, &Skip::default());
        assert!(plain.changes.iter().any(|c| c.champion == "maddie" && c.step < 0 && c.reset.is_none()));
        // listed without power: just left alone
        let fresh = make_round_with(&m, &[], RULES, &Due::First("2027.4.1".into()), None, None, &skip);
        assert!(!fresh.changes.iter().any(|c| c.champion == "maddie"));
    }

    #[test]
    fn the_book_keeps_every_champions_numbers_and_tells_the_players_changes() {
        use crate::edits::Verdict;
        let _serial = crate::tests::serial();
        reset_for_tests();
        let file = std::env::temp_dir().join(format!("pma_seen_{}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let n = |a: u32| {
            let mut x = [0u32; 18];
            x[0] = a;
            x
        };
        let mut now: HashMap<String, crate::edits::Numbers> = [("jinx".to_string(), n(70)), ("fighter".to_string(), n(50))].into();
        let defs: HashMap<String, crate::edits::Numbers> = [("jinx".to_string(), n(100))].into();
        assert!(observe(&now, &defs, true).is_empty(), "no book: nothing kept");
        open_for_tests(file.clone());
        assert!(observe(&now, &defs, true).is_empty(), "the first read: nothing to tell");
        now.insert("jinx".into(), n(90));
        now.insert("fighter".into(), n(55));
        assert_eq!(observe(&now, &defs, true), vec![("fighter".to_string(), Verdict::Player), ("jinx".to_string(), Verdict::Player)]);
        assert!(observe(&now, &defs, true).is_empty(), "seen now");
        // a mod's update: its file changed with the numbers
        now.insert("jinx".into(), n(110));
        let defs2: HashMap<String, crate::edits::Numbers> = [("jinx".to_string(), n(140))].into();
        assert_eq!(observe(&now, &defs2, true), vec![("jinx".to_string(), Verdict::ModUpdate)]);
        // with the balance off: taken as they are
        now.insert("fighter".into(), n(60));
        assert!(observe(&now, &defs2, false).is_empty());
        // kept in the book's file: the next session compares with them
        close();
        open_for_tests(file.clone());
        now.insert("fighter".into(), n(61));
        assert_eq!(observe(&now, &defs2, true), vec![("fighter".to_string(), Verdict::Player)]);
        close();
        let _ = std::fs::remove_file(&file);
    }

    // ===================== 2.13.0：防御塔和大小龙 =====================

    fn watch(r: u32) -> Watch {
        // 12.5 分钟；小龙活 10 秒，大龙活 1000 tick
        Watch { r, e: 45000, d: vec![(7200, 7800)], b: vec![(36000, 37000)], off: false }
    }

    #[test]
    fn a_round_judges_the_towers_and_monsters_on_the_matches_watched() {
        reset_for_tests();
        let file = std::env::temp_dir().join(format!("pma_objectives_test_{}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        open_file(file.clone());
        publish(Round { id: 1, version: "1.1".into(), rules: RULES_VERSION, ..Round::default() });
        for k in 0..40 {
            keep_watch(&format!("{k}.0"), watch(1));
        }
        // 同一场再交一次是换掉
        keep_watch("39.0", watch(1));
        let targets = crate::objectives::Targets { base: [3713, 5000, 10000], ..crate::objectives::DEFAULT_TARGETS };
        let rules = Rules { objectives: Some(targets), ..RULES };
        let round = make(&meta(), rules, &Due::Patch("1.2".into()), None, &Skip::default(), &HashMap::new());
        let got: Vec<(Objective, i32, i64)> = round.objectives.iter().map(|c| (c.objective, c.step, c.base)).collect();
        assert_eq!(got, [(Objective::Tower, 15, 3713), (Objective::Drake, 15, 5000), (Objective::Baron, 15, 10000)]);
        assert_eq!(round.objective_power.get(&Objective::Tower), Some(&15));
        assert!(round.objective_summary.contains("tower: 12.5 min on 40") && round.objective_summary.contains("40 matches watched"), "{}", round.objective_summary);
        // 关着 objective_balance：上一轮的照抄，不判断
        let off = make(&meta(), RULES, &Due::Patch("1.2".into()), None, &Skip::default(), &HashMap::new());
        assert!(off.objectives.is_empty() && off.objective_power.is_empty());
        publish(round);
        // 账本写下了记录和百分比，再读回来一样
        close();
        open_file(file.clone());
        assert_eq!(latest().unwrap().objective_power.get(&Objective::Drake), Some(&15));
        assert_eq!(store().watched.len(), 40);
        // 比赛按最新一轮改血；服务端那一份关着 objective_balance，它的重播也不改
        let p = round_for("m".into(), true, true, true, true).unwrap();
        assert_eq!((p.round, p.objectives.unwrap().get(&Objective::Baron).copied()), (2, Some(15)));
        assert!(round_for("n".into(), true, true, true, false).unwrap().objectives.is_none());
        assert!(round_for("n".into(), false, true, true, true).unwrap().objectives.is_none(), "its replays follow the server run");
        // 下一轮：改过以后的比赛还不够 30 场，不动
        let next = make(&meta(), rules, &Due::Patch("1.3".into()), None, &Skip::default(), &HashMap::new());
        assert!(next.objectives.is_empty(), "{:?}", next.objectives);
        assert_eq!(next.objective_power.get(&Objective::Tower), Some(&15), "carried");
        close();
        let _ = std::fs::remove_file(&file);
        reset_for_tests();
    }

    #[test]
    fn health_the_player_changes_takes_the_percentage_off_at_once() {
        let today = Some((2028, 3, 20));
        let mut rounds = vec![powered(&[("jinx", -20)], &[])];
        rounds[0].objective_power = [(Objective::Tower, 30), (Objective::Drake, 15)].into_iter().collect();
        let reset = make_reset(&rounds, &[], &[], &[(Objective::Tower, 5000), (Objective::Baron, 12000)], today).unwrap();
        assert!(reset.reset);
        let got: Vec<(Objective, i32, i32, i64)> = reset.objectives.iter().map(|c| (c.objective, c.step, c.total, c.base)).collect();
        // 大龙没有百分比：也写上，之后从这一轮重新数比赛
        assert_eq!(got, [(Objective::Tower, -30, 0, 5000), (Objective::Baron, 0, 0, 12000)]);
        assert_eq!(reset.objective_power, [(Objective::Drake, 15)].into_iter().collect());
        assert_eq!(reset.power.get("jinx"), Some(&-20), "the champions keep theirs");
    }

    #[test]
    fn a_health_change_in_the_same_patch_away_from_patch_days_is_the_players() {
        reset_for_tests();
        let file = std::env::temp_dir().join(format!("pma_objectives_seen_{}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        open_file(file.clone());
        let tower = |hp: i64| [(Objective::Tower, hp)];
        // 第一次读：只记下
        assert!(observe_objectives(&tower(3713), "2028.5.1", false, true).is_empty());
        assert!(observe_objectives(&tower(3713), "2028.5.1", false, true).is_empty());
        // 同一个版本里改了：玩家改的
        assert_eq!(observe_objectives(&tower(5000), "2028.5.1", false, true), [Objective::Tower]);
        // 补丁日前后变的、换了版本才变的：游戏自己的规则补丁
        assert!(observe_objectives(&tower(4500), "2028.5.1", true, true).is_empty());
        assert!(observe_objectives(&tower(4000), "2029.0.0", false, true).is_empty());
        // 平衡关着：只记不算
        assert!(observe_objectives(&tower(3000), "2029.0.0", false, false).is_empty());
        close();
        open_file(file.clone());
        assert_eq!(store().seen_objectives.get(&Objective::Tower), Some(&(3000, "2029.0.0".to_string())));
        close();
        let _ = std::fs::remove_file(&file);
        reset_for_tests();
    }

    // ===================== 2.14.0：主题版本、补丁预告 =====================

    #[test]
    fn a_theme_patch_buffs_one_class_together_and_says_so() {
        reset_for_tests();
        let mut m = meta();
        m.champions.push(champ("other", 50.0, 300.0));
        let categories: HashMap<String, u8> = ["maddie", "fine", "weak", "other"].iter().map(|c| (c.to_string(), 4u8)).collect();
        let rules = Rules { theme_every: 1, theme_step: 8, ..RULES };
        let round = make(&m, rules, &Due::Patch("1.2".into()), None, &Skip::default(), &categories);
        assert_eq!(round.theme, Some(4));
        // 四个刺客都 +8，原来按胜率给 maddie 的削弱这一轮不算
        let steps: Vec<(&str, i32, Option<u8>)> = round.changes.iter().map(|c| (c.champion.as_str(), c.step, c.theme)).collect();
        assert_eq!(steps.len(), 4, "{steps:?}");
        assert!(steps.iter().all(|(_, step, theme)| *step == 8 && *theme == Some(4)), "{steps:?}");
        assert_eq!(round.power.get("maddie"), Some(&8));
        let maddie = round.changes.iter().find(|c| c.champion == "maddie").unwrap();
        assert_eq!(reason(maddie, &Flavor::default()), "pma_theme_4");
        let (_, body) = news(&round, &|c: &str| if c == "maddie" { "麦迪".to_string() } else { c.to_string() });
        assert!(body.starts_with("本版本是主题版本「刺客版本」"), "{body}");
        assert!(body.contains("麦迪：主题版本（刺客）"), "{body}");
        // 热修复不出主题；关掉也不出
        assert_eq!(make(&m, rules, &Due::First("1.2".into()), None, &Skip::default(), &categories).theme, None);
        assert_eq!(make(&m, Rules { theme_every: 0, ..rules }, &Due::Patch("1.2".into()), None, &Skip::default(), &categories).theme, None);
        // 一类不够 3 个英雄不做主题
        let few: HashMap<String, u8> = [("maddie".to_string(), 1u8)].into_iter().collect();
        assert_eq!(make(&m, rules, &Due::Patch("1.2".into()), None, &Skip::default(), &few).theme, None);
        reset_for_tests();
    }

}
