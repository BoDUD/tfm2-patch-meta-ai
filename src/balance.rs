//! Balance patches (2.8.0; the user 2026-11-11: 「现在能不能让meta控制版本平衡的补丁吧 现在系统自带的平衡补丁 英雄
//! 削弱不痛不痒 加强也是 砍不到位 没有lol那种感觉」, then 「我希望我们这套系统完全替代官方的 不要有死角」). On one save
//! at patch 2027.4.1 the game's own patches had nerfed champions at 46% (Gwen, Nami, Brand) while Maddie sat at 73%
//! with a 52% ban rate untouched. With `balance=on` this mod makes the patches instead, and they show wherever the
//! game's did:
//!
//! - **When**: a round for every patch the game makes (its patch notes appear in the player's news, [`Due::Patch`]);
//!   a first one when the setting is turned on, as a hotfix of the current patch (`2027.4.1b`, [`Due::First`]); and a
//!   hotfix when the game made no patch for [`LATE_DAYS`] game days ([`Due::Late`]).
//! - **What**: the champions the model rates furthest outside [`BAND`] - with [`MIN_EVIDENCE`] recent games or more,
//!   at most `balance_count` of them - change by [`PER_POINT`] percent of power for every win-rate point outside the
//!   band, at most `balance_max` a round, at most [`CAP`] in all (rounds add up, as in League: a champion still too
//!   strong is cut again next patch). A champion a round changed waits until it has played [`MIN_EVIDENCE`] games
//!   with its new numbers ([`games_since_change`]; 2.8.1 - the first rounds on a save, a hotfix and the game's patch
//!   two days later, cut the same ten champions twice on the games from before the first cut).
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
//!   finished items whose builders - the champions whose published build names them - stay outside the band once
//!   this round's own champion changes are counted ([`decide_items`]): so the items catch what the champions' own
//!   changes leave (a champion at the limit, a whole family of builders) rather than cutting a champion twice. An
//!   item's power is a share of its numbers, put on its holders by a second buff in every match ([`on_tick`]); the
//!   items' notes are news of their own ([`item_news`]).
//! - **The game's own champion patches off**: the server half sets the save's auto-patcher counts to zero while this
//!   is on, keeping the save's own values to put back when it is turned off (`server::balance`).
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
use crate::patchnotes::PatchNote;
use crate::records::compare_versions;
use crate::stats::Stats;
use crate::{diag, paths};

/// The buff that carries a champion's balance in a match.
pub const BUFF: &str = "patch_meta_ai_balance";
/// Win rates inside this band (percent) are left alone.
pub const BAND: (f32, f32) = (48.0, 52.0);
/// Power changed for every win-rate point outside the band (percent of power).
pub const PER_POINT: f32 = 1.5;
/// A champion's (and an item's) power stays within this much of its own (percent). 50 since 2.9.0 (the user,
/// 2026-11-13: 「还有max也调高点」 - 30 held the champions that did not move at -30%, and `balance_max` up to 50 needs
/// room in all as well).
pub const CAP: i32 = 50;
/// Recent games (each patch back counting half, the model's `evidence`) before a champion's win rate is trusted.
pub const MIN_EVIDENCE: f32 = 30.0;
/// Items (2.9.0): builders with games enough ([`MIN_EVIDENCE`]) an item needs before it is judged; each builder
/// weighs its games, at most [`ITEM_WEIGHT`]; their win rate is drawn toward 50 by n / (n + [`ITEM_SHRINK`]) (a few
/// builders say little about the item); within [`ITEM_BAND`] points of 50 it is left alone, beyond it a step of
/// [`ITEM_PER_POINT`] percent of its numbers a point; an item changed waits for [`ITEM_MIN_GAMES`] games of its
/// builders since.
pub const ITEM_MIN_USERS: usize = 4;
pub const ITEM_WEIGHT: f32 = 200.0;
pub const ITEM_SHRINK: f32 = 4.0;
pub const ITEM_BAND: f32 = 2.0;
pub const ITEM_PER_POINT: f32 = 2.0;
pub const ITEM_MIN_GAMES: f32 = 60.0;
/// A match looks at its champions' items every this many ticks (an item is bought now and then).
pub const ITEM_EVERY: usize = 10;
/// Game days after a round without a new game patch before the next round comes as a hotfix of the same patch (the
/// game makes one every 4-6 weeks; with its champion changes off it might make none).
pub const LATE_DAYS: i64 = 60;
/// Matches whose round is kept (the newest).
const KEPT: usize = 4000;
/// A run waits this long for all its champions before it settles with the ones there.
const TICK_WINDOW: usize = 600;
/// Runs remembered (their decided balance).
const RUNS_KEPT: usize = 64;

/// How big a round is (settings `balance_count`, `balance_max`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    pub count: usize,
    pub max_step: i32,
    /// The 4+6 pack's items too (2.9.0, `item_balance`), at most `item_count` a round, each a step of at most
    /// `item_max` (percent of its numbers).
    pub items: bool,
    pub item_count: usize,
    pub item_max: i32,
}

/// One champion's change in a round.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub champion: String,
    /// The model's win rate for it when the round was made (percent) and its recent games.
    pub win: f32,
    pub evidence: f32,
    /// This round's step and its power after it (percent).
    pub step: i32,
    pub total: i32,
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
}

/// A champion the model rates: name, win rate (percent), recent games, and its games since a round last changed it
/// (None: no round did).
#[derive(Clone, Copy, Debug)]
pub struct Candidate<'a> {
    pub name: &'a str,
    pub win: f32,
    pub evidence: f32,
    pub since_change: Option<f32>,
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
/// last change), at most `rules.count`, each a step of [`PER_POINT`] a point outside the band (at most
/// `rules.max_step`), their power kept within [`CAP`].
pub fn decide(candidates: &[Candidate<'_>], before: &BTreeMap<String, i32>, rules: Rules) -> Vec<Change> {
    let mut out = qualified(candidates, before, rules);
    out.truncate(rules.count);
    out
}

/// Every change [`decide`] could make, furthest outside the band first (before `rules.count` cuts the list).
fn qualified(candidates: &[Candidate<'_>], before: &BTreeMap<String, i32>, rules: Rules) -> Vec<Change> {
    let mut out: Vec<(f32, Change)> = candidates
        .iter()
        .filter(|c| c.evidence >= MIN_EVIDENCE && c.settled())
        .filter_map(|c| {
            // above the band: a nerf; below it: a buff
            let gap = c.gap()?;
            let want = ((-(gap * PER_POINT)).round() as i32).clamp(-rules.max_step, rules.max_step);
            let was = before.get(c.name).copied().unwrap_or(0);
            let total = (was + want).clamp(-CAP, CAP);
            let step = total - was;
            (step != 0).then(|| {
                (gap.abs(), Change { champion: c.name.to_string(), win: c.win, evidence: c.evidence, step, total })
            })
        })
        .collect();
    out.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.champion.cmp(&b.1.champion)));
    out.into_iter().map(|(_, c)| c).collect()
}

/// A champion as the items see it (2.9.0): its win rate, the win rate once this round's own change to it works (a
/// step closes step / [`PER_POINT`] points), its games, and the finished items its published build names.
#[derive(Clone, Debug, PartialEq)]
pub struct Builder {
    pub champion: String,
    pub win: f32,
    pub after: f32,
    pub evidence: f32,
    pub items: Vec<String>,
}

/// The item changes of a round (2.9.0): for each item named by [`ITEM_MIN_USERS`] builders or more with games, their
/// win rate after this round's champion changes, weighted by games (at most [`ITEM_WEIGHT`] each) and drawn toward 50
/// by n / (n + [`ITEM_SHRINK`]); beyond [`ITEM_BAND`] points of 50 a step of [`ITEM_PER_POINT`] a point (at most
/// `rules.item_max`), the power kept within [`CAP`]; an item `settled` says not yet waits. Furthest first, at most
/// `rules.item_count`. `names`: the items that can change (finished ones), with their names.
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
        if n < ITEM_MIN_USERS {
            continue;
        }
        let weight: f32 = users.iter().map(|b| b.evidence.min(ITEM_WEIGHT)).sum();
        if weight <= 0.0 {
            continue;
        }
        let mean = |f: &dyn Fn(&Builder) -> f32| users.iter().map(|b| b.evidence.min(ITEM_WEIGHT) * f(b)).sum::<f32>() / weight;
        let (win, after) = (mean(&|b| b.win), mean(&|b| b.after));
        let shrunk = (after - 50.0) * n as f32 / (n as f32 + ITEM_SHRINK);
        let gap = if shrunk > ITEM_BAND {
            shrunk - ITEM_BAND
        } else if shrunk < -ITEM_BAND {
            shrunk + ITEM_BAND
        } else {
            continue;
        };
        let want = ((-(gap * ITEM_PER_POINT)).round() as i32).clamp(-rules.item_max, rules.item_max);
        let was = before.get(item).copied().unwrap_or(0);
        let total = (was + want).clamp(-CAP, CAP);
        let step = total - was;
        if step == 0 || !settled(item) {
            continue;
        }
        out.push((gap.abs(), ItemChange { item: item.clone(), name: name.clone(), users: n as u32, win, step, total }));
    }
    out.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.item.cmp(&b.1.item)));
    out.truncate(rules.item_count);
    out.into_iter().map(|(_, c)| c).collect()
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
}

impl Due {
    pub fn version(&self) -> &str {
        match self {
            Due::Patch(v) | Due::First(v) | Due::Late(v) => v,
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
    make_round_with(meta, rounds, rules, due, today, None)
}

/// [`make_round`] with the 4+6 pack's builds and item names (2.9.0): with `rules.items` the round changes items too;
/// without, it carries the item power of the round before unchanged.
pub fn make_round_with(
    meta: &Meta,
    rounds: &[Round],
    rules: Rules,
    due: &Due,
    today: Option<(i32, u32, u32)>,
    items: Option<&crate::items::Inputs>,
) -> Round {
    let last = rounds.last();
    let version = due.version().to_string();
    let merged = matches!(due, Due::Patch(_));
    let label = if merged { version.clone() } else { hotfix_label(rounds, &version) };
    let before = last.map(|r| r.power.clone()).unwrap_or_default();
    let candidates: Vec<Candidate<'_>> = meta
        .champions
        .iter()
        .map(|c| Candidate {
            name: &c.name,
            win: c.win_rate() * 100.0,
            evidence: c.evidence,
            since_change: games_since_change(rounds, &meta.versions, c),
        })
        .collect();
    let could = qualified(&candidates, &before, rules);
    let changes: Vec<Change> = could.iter().take(rules.count).cloned().collect();
    // out of the band with games enough, but not since their last change: next round, maybe
    let mut held: Vec<(f32, String, f32)> = candidates
        .iter()
        .filter(|c| c.evidence >= MIN_EVIDENCE && !c.settled())
        .filter_map(|c| Some((c.gap()?.abs(), c.name.to_string(), c.since_change.unwrap_or(0.0))))
        .collect();
    held.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let summary = summarize(&candidates, &before, rules, could.len(), changes.len());
    let mut power = before;
    for c in &changes {
        if c.total == 0 {
            power.remove(&c.champion);
        } else {
            power.insert(c.champion.clone(), c.total);
        }
    }
    let mut item_power = last.map(|r| r.item_power.clone()).unwrap_or_default();
    let item_changes = match items.filter(|_| rules.items) {
        Some(inputs) => {
            let builders: Vec<Builder> = meta
                .champions
                .iter()
                .filter_map(|c| {
                    let items = inputs.builds.get(&c.name)?.clone();
                    let win = c.win_rate() * 100.0;
                    let step = changes.iter().find(|x| x.champion == c.name).map_or(0, |x| x.step);
                    Some(Builder { champion: c.name.clone(), win, after: win + step as f32 / PER_POINT, evidence: c.evidence, items })
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
    }
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
            let gap = c.gap().unwrap_or(0.0);
            let want = ((-(gap * PER_POINT)).round() as i32).clamp(-rules.max_step, rules.max_step);
            let was = before.get(c.name).copied().unwrap_or(0);
            want != 0 && (was + want).clamp(-CAP, CAP) == was
        })
        .count();
    let close = outside.len().saturating_sub(waiting + at_limit + could);
    format!(
        "{} champions rated, {} with {MIN_EVIDENCE} games or more, {} of those outside {}-{}%: {waiting} waiting for          {MIN_EVIDENCE} games with their new numbers, {at_limit} at the {CAP}% limit, {close} too close to the band for a          step; {could} could change, {changed} changed (balance_count {})",
        candidates.len(),
        rated.len(),
        outside.len(),
        BAND.0,
        BAND.1,
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

/// The patch notes of the rounds for the model: each round gives each champion it changed its direction (+1
/// buffed, -1 nerfed) in its patch, as the game's own notes do (the client adds up the notes of a patch).
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

/// A change's reason as the game's notes give one: codes of `patch.reason.*` texts, comma-separated - the model's
/// win rate (`pma_wr_<n>`, rounded away from the band), and whether the round's or the overall limit held it.
pub fn reason(c: &Change, rules: Rules) -> String {
    let n = if c.step < 0 { c.win.ceil() } else { c.win.floor() }.clamp(0.0, 100.0) as u32;
    let mut codes = vec![format!("pma_wr_{n}")];
    // the power this round and in all (2.9.0; the user: 「一起加上」 - a player read -11% of power as the -5% / -6% of
    // the lines and thought it barely cut)
    if c.step != 0 {
        codes.push(format!("pma_power_{}", signed_code(c.step)));
    }
    if c.total != 0 && c.total != c.step {
        codes.push(format!("pma_sum_{}", signed_code(c.total)));
    }
    if c.step.abs() >= rules.max_step {
        codes.push("pma_cap".to_string());
    }
    if c.total.abs() >= CAP {
        codes.push("pma_total".to_string());
    }
    codes.join(",")
}

/// A signed percent in a text code: `m11` for -11, `p9` for +9 (`pma_power_*`, `pma_sum_*`).
pub fn signed_code(v: i32) -> String {
    format!("{}{}", if v < 0 { "m" } else { "p" }, v.unsigned_abs())
}

/// The round's item changes as news of their own (2.9.0; None: it changed no item): a title and a line an item - its
/// builders' win rate, its step and power, and its numbers before and after (`lines`, [`crate::items::lines`]).
pub fn item_news(round: &Round, chinese: bool, lines: &dyn Fn(&ItemChange) -> Vec<crate::items::Line>) -> Option<(String, String)> {
    if round.items.is_empty() {
        return None;
    }
    let name = if round.label.is_empty() { round.version.as_str() } else { round.label.as_str() };
    let title = if chinese { format!("电竞大脑装备平衡 {name}") } else { format!("Patch Meta AI item balance {name}") };
    let mut out = vec![if chinese {
        "4+6 装备的改动：只改装备给的数值（特效不变，穿透、韧性这类只能加不能减），对所有比赛生效；装备说明里显示的仍是原来的数值。"
            .to_string()
    } else {
        "4+6 item changes: only the numbers an item gives (its effects stay; penetration, tenacity and the like can go up,          never down), in every match; the item tooltips still show the numbers from before."
            .to_string()
    }];
    let mut items: Vec<&ItemChange> = round.items.iter().collect();
    items.sort_by_key(|c| (c.step > 0, c.step));
    for c in items {
        let numbers: Vec<String> = lines(c).into_iter().map(|(label, from, to)| format!("{label} {from} → {to}")).collect();
        out.push(if chinese {
            format!(
                "【{}】{}：出它的 {} 个英雄平均胜率约 {:.1}% → 装备数值 {:+}%（累计 {:+}%）{}{}",
                if c.step < 0 { "削弱" } else { "加强" },
                c.name,
                c.users,
                c.win,
                c.step,
                c.total,
                if numbers.is_empty() { "" } else { "：" },
                numbers.join("，")
            )
        } else {
            format!(
                "[{}] {}: its {} builders win about {:.1}% -> item numbers {:+}% ({:+}% in all){}{}",
                if c.step < 0 { "Nerf" } else { "Buff" },
                c.name,
                c.users,
                c.win,
                c.step,
                c.total,
                if numbers.is_empty() { "" } else { ": " },
                numbers.join(", ")
            )
        });
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
            lines.push(if chinese {
                format!(
                    "{}：胜率 {:.1}%（目标 48%–52%）→ 强度 {:+}%（攻击/法强 {:+}%，生命 {:+}%），累计 {:+}%",
                    label(&c.champion),
                    c.win,
                    c.step,
                    damage,
                    health,
                    c.total
                )
            } else {
                format!(
                    "{}: win rate {:.1}% (target 48-52%) -> power {:+}% (attack / magic power {:+}%, health {:+}%), {:+}% in all",
                    label(&c.champion),
                    c.win,
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
        st.rounds.push(r);
    }
    for (key, id) in book.kept {
        if st.kept.insert(key.clone(), id).is_none() {
            st.order.push_back(key);
        }
    }
    st.items_off = book.items_off.into_iter().filter(|k| st.kept.contains_key(k)).collect();
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
    let book = Book { v: 1, rounds: st.rounds.iter().chain(&st.later).cloned().collect(), kept, items_off };
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

/// Whether a round is due on this save ([`due`]).
pub fn due_now(patch: Option<&str>, today: Option<(i32, u32, u32)>) -> Option<Due> {
    let st = store();
    st.file.as_ref()?;
    due(&st.rounds, patch, today)
}

/// The round `due` asks for on this save ([`make_round`]); not in the book until [`publish`]ed.
pub fn make(meta: &Meta, rules: Rules, due: &Due, today: Option<(i32, u32, u32)>) -> Round {
    let items = if rules.items { crate::items::inputs() } else { None };
    if rules.items && items.is_none() {
        diag::log_once("items-no-inputs", "[items] item_balance is on but the 4+6 builds or catalog are not read (yet): this round changes no item");
    }
    let st = store();
    let mut round = make_round_with(meta, &st.rounds, rules, due, today, items.as_ref());
    round.id = round.id.max(st.done + 1);
    round
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
}

/// What a run plays: every champion's power, and the items' (None: no item change).
#[derive(Clone)]
struct RunPower {
    champions: Arc<BTreeMap<String, i32>>,
    items: Option<Arc<BTreeMap<String, i32>>>,
}

static RUNS: LazyLock<Mutex<VecDeque<Run>>> = LazyLock::new(|| Mutex::new(VecDeque::new()));

fn run_key(sim: &StableSim<'_>) -> RunKey {
    sim.sim_origin().map_or((u32::MAX, u64::MAX, u64::MAX, u64::MAX), |o| (o.kind, o.match_id, o.replay_id, o.set_index))
}

/// This run's balance, decided once it has its champions (None: not yet). The server's run takes the newest round
/// (`on`: the setting) and keeps it; any other run plays what was kept (none when nothing was).
fn decide_run(sim: &StableSim<'_>, on: bool, items_on: bool) -> Option<Option<RunPower>> {
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
    Some(round_for(key, first_run, on, items_on))
}

/// The power a run of the match `key` plays: the server's run (`first_run`) takes the newest round (none with the
/// setting off) and keeps it under the key; any other run plays what was kept (none when nothing was). The items'
/// power with it, unless the server's run had `item_balance` off (kept too, 2.9.0).
fn round_for(key: String, first_run: bool, on: bool, items_on: bool) -> Option<RunPower> {
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
        if items_changed {
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
                        }
                    }
                }
                st.unwritten = true;
                if st.written_at.is_none_or(|t| t.elapsed() >= WRITE_EVERY) {
                    write(&mut st);
                }
            }
        }
        return run_power(&st, id, items_on);
    }
    let id = st.kept.get(&key).copied().unwrap_or(0);
    let items_on = !st.items_off.contains(&key);
    run_power(&st, id, items_on)
}

fn run_power(st: &Store, id: u32, items_on: bool) -> Option<RunPower> {
    let champions = st.powers.get(&id).cloned()?;
    let items = if items_on { st.item_powers.get(&id).filter(|m| !m.is_empty()).cloned() } else { None };
    Some(RunPower { champions, items })
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
pub fn on_tick(sim: &mut StableSim<'_>, on: bool, items_on: bool) {
    let key = run_key(sim);
    let tick = sim.tick();
    let power = {
        let mut runs = RUNS.lock().unwrap_or_else(PoisonError::into_inner);
        let at = match runs.iter().position(|r| r.key == key) {
            Some(at) => at,
            None => {
                runs.push_back(Run { key, last_tick: tick, power: None });
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
        }
        run.last_tick = tick;
        if run.power.is_none() {
            run.power = decide_run(sim, on, items_on);
        }
        match &run.power {
            Some(Some(p)) => p.clone(),
            _ => return,
        }
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
    if let Some(items) = power.items.filter(|_| tick.is_multiple_of(ITEM_EVERY)) {
        item_pass(sim, &items);
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

    /// The champions' power a run plays (the items' left out), as the tests before 2.9.0 read it.
    fn champs(key: String, first_run: bool, on: bool) -> Option<Arc<BTreeMap<String, i32>>> {
        round_for(key, first_run, on, true).map(|p| p.champions)
    }

    const RULES: Rules = Rules { count: 10, max_step: 10, items: false, item_count: 5, item_max: 10 };

    fn cand(name: &str, win: f32, evidence: f32) -> Candidate<'_> {
        Candidate { name, win, evidence, since_change: None }
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
        // 21.3 points over: -32, held to -10; 12 over: -18 -> -10; 4 under: +6; gwen has too few games; 50.5 is fine
        assert_eq!(got, [("maddie", -10), ("zeri", -10), ("weak", 6)]);
        // a round's count keeps the furthest
        assert_eq!(decide(&[cand("a", 60.0, 50.0), cand("b", 70.0, 50.0)], &before, Rules { count: 1, ..RULES })[0].champion, "b");
        // just outside the band: under a step, no change
        assert!(decide(&[cand("a", 52.2, 50.0)], &before, RULES).is_empty());
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
        assert_eq!((out[0].step, out[0].total), (6, -14));
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
        assert_eq!((third.power.get("maddie"), third.power.get("weak")), (Some(&-20), Some(&12)));
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
        assert_eq!((first.power.get("maddie"), first.power.get("weak"), first.power.get("fine")), (Some(&-10), Some(&6), None));
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
    fn the_reason_is_the_win_rate_and_the_limits() {
        let c = |win: f32, step: i32, total: i32| Change { champion: "x".into(), win, evidence: 50.0, step, total };
        assert_eq!(reason(&c(73.3, -10, -10), RULES), "pma_wr_74,pma_power_m10,pma_cap");
        assert_eq!(reason(&c(52.3, -1, -1), RULES), "pma_wr_53,pma_power_m1", "rounded away from the band");
        assert_eq!(reason(&c(44.6, 5, 5), RULES), "pma_wr_44,pma_power_p5");
        // the power in all when other rounds moved it before (2.9.0), the limit in all at CAP
        assert_eq!(reason(&c(70.0, -5, -30), RULES), "pma_wr_70,pma_power_m5,pma_sum_m30");
        assert_eq!(reason(&c(70.0, -5, -CAP), RULES), format!("pma_wr_70,pma_power_m5,pma_sum_m{CAP},pma_total"));
        assert_eq!(reason(&c(40.0, 8, 0), RULES), "pma_wr_40,pma_power_p8", "back to its own numbers: no sum");
        assert_eq!((signed_code(-11), signed_code(9)), ("m11".to_string(), "p9".to_string()));
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
                Change { champion: "tft_maddie".into(), win: 73.3, evidence: 221.0, step: -10, total: -10 },
                Change { champion: "league_warwick".into(), win: 60.0, evidence: 90.0, step: -10, total: -20 },
                Change { champion: "league_kaisa".into(), win: 35.9, evidence: 80.0, step: 10, total: 10 },
            ],
            ..Round::default()
        };
        let row = Row { key: "#asset/base/text/champion?stat.attack".into(), target_action: None, pre_value: "94".into(), after_value: "89.3".into(), is_buff: false };
        let mut lines = BTreeMap::new();
        for c in &round.changes {
            lines.insert(c.champion.clone(), (vec![Row { is_buff: c.step > 0, ..row.clone() }], reason(c, RULES)));
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
        assert_eq!(pc02.title_bind[2].1, "pma_wr_74,pma_power_m10,pma_cap");
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
            changes: vec![Change { champion: "maddie".into(), win: 70.0, evidence: 99.0, step: -10, total: -20 }],
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
        assert_eq!(make(&meta(), RULES, &Due::First("1.1".into()), None).id, 1);
        close();
        // a save whose book was lost (its news has two rounds, no file anywhere): the ids go on after them
        let _ = std::fs::remove_file(&legacy);
        open_book_file(career.clone(), legacy.clone(), 2);
        assert!(rounds().is_empty());
        let next = make(&meta(), RULES, &Due::First("1.1".into()), None);
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
                .map(|(c, s)| Change { champion: c.into(), win: 50.0, evidence: 50.0, step: s, total: s })
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
                Change { champion: "tft_maddie".into(), win: 73.3, evidence: 221.0, step: -10, total: -10 },
                Change { champion: "weak".into(), win: 44.0, evidence: 80.0, step: 6, total: 6 },
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

    const ITEM_RULES: Rules = Rules { count: 10, max_step: 10, items: true, item_count: 5, item_max: 10 };

    fn builder(name: &str, win: f32, after: f32, evidence: f32, items: &[&str]) -> Builder {
        Builder { champion: name.into(), win, after, evidence, items: items.iter().map(|s| s.to_string()).collect() }
    }

    fn names(keys: &[&str]) -> BTreeMap<String, String> {
        keys.iter().map(|k| (k.to_string(), format!("名字{k}"))).collect()
    }

    #[test]
    fn an_item_whose_builders_still_win_too_much_is_cut() {
        let always = |_: &str| true;
        let before = BTreeMap::new();
        // five builders at 62% after this round's own champion changes: 12 over 50, drawn to 12 x 5/9 = 6.7,
        // 4.7 beyond the band, a step of -9
        let five: Vec<Builder> = (0..5).map(|i| builder(&format!("c{i}"), 64.0, 62.0, 100.0, &["rfc"])).collect();
        let out = decide_items(&five, &names(&["rfc"]), &before, &always, ITEM_RULES);
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].item.as_str(), out[0].users, out[0].step, out[0].total), ("rfc", 5, -9, -9));
        assert!((out[0].win - 64.0).abs() < 1e-4, "the notes give the builders' win rate as it is");
        // three builders are too few to judge an item; builders with few games do not count
        let three: Vec<Builder> = (0..3).map(|i| builder(&format!("c{i}"), 70.0, 70.0, 100.0, &["rfc"])).collect();
        assert!(decide_items(&three, &names(&["rfc"]), &before, &always, ITEM_RULES).is_empty());
        let mut thin = three.clone();
        thin.push(builder("new", 70.0, 70.0, MIN_EVIDENCE - 1.0, &["rfc"]));
        assert!(decide_items(&thin, &names(&["rfc"]), &before, &always, ITEM_RULES).is_empty());
        // the builders' win rate once their own nerfs work is what counts: fine after them, no item change
        let fixed: Vec<Builder> = (0..6).map(|i| builder(&format!("c{i}"), 60.0, 51.0, 100.0, &["rfc"])).collect();
        assert!(decide_items(&fixed, &names(&["rfc"]), &before, &always, ITEM_RULES).is_empty());
        // far off: held to item_max a round, CAP in all; weak builders get the item buffed
        let far: Vec<Builder> = (0..20).map(|i| builder(&format!("c{i}"), 80.0, 80.0, 500.0, &["rfc", "weak"])).collect();
        let out = decide_items(&far, &names(&["rfc"]), &before, &always, ITEM_RULES);
        assert_eq!((out[0].step, out[0].total), (-10, -10));
        let was: BTreeMap<String, i32> = [("rfc".to_string(), -(CAP - 4))].into_iter().collect();
        let out = decide_items(&far, &names(&["rfc"]), &was, &always, ITEM_RULES);
        assert_eq!((out[0].step, out[0].total), (-4, -CAP));
        let weak: Vec<Builder> = (0..8).map(|i| builder(&format!("c{i}"), 40.0, 40.0, 100.0, &["staff"])).collect();
        assert!(decide_items(&weak, &names(&["staff"]), &before, &always, ITEM_RULES)[0].step > 0);
        // an item that has to wait, and the round's count keeps the furthest
        let wait = |item: &str| item != "rfc";
        assert!(decide_items(&five, &names(&["rfc"]), &before, &wait, ITEM_RULES).is_empty());
        let mut mixed = five.clone();
        mixed.extend((0..5).map(|i| builder(&format!("d{i}"), 70.0, 70.0, 100.0, &["bc"])));
        let out = decide_items(&mixed, &names(&["rfc", "bc"]), &before, &always, Rules { item_count: 1, ..ITEM_RULES });
        assert_eq!(out.iter().map(|c| c.item.as_str()).collect::<Vec<_>>(), ["bc"]);
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
        let round = make_round_with(&m, &[], ITEM_RULES, &Due::First("2027.4.1".into()), today, Some(&inp));
        assert!(round.changes.iter().any(|c| c.champion == "maddie"), "the champions as before");
        assert_eq!(round.items.len(), 1, "{:?}", round.items);
        assert_eq!(round.items[0].item, "rfc");
        assert_eq!(round.item_power.get("rfc"), Some(&round.items[0].total));
        // the item balance off: no item change, the power so far carried on
        let next = make_round_with(&m, std::slice::from_ref(&round), RULES, &Due::Late("2027.4.1".into()), today, Some(&inp));
        assert!(next.items.is_empty());
        assert_eq!(next.item_power, round.item_power);
        // the item just changed waits for its builders' games
        let again = make_round_with(&m, std::slice::from_ref(&round), ITEM_RULES, &Due::Late("2027.4.1".into()), today, Some(&inp));
        assert!(again.items.is_empty(), "no games since: {:?}", again.items);
        // the log says why these champions and no more
        assert!(round.summary.starts_with("7 champions rated"), "{}", round.summary);
        assert!(round.summary.contains("changed (balance_count 10)"), "{}", round.summary);
    }

    #[test]
    fn the_log_says_why_a_round_changed_fewer_than_balance_count() {
        let c = |name: &'static str, win: f32, evidence: f32, since: Option<f32>| Candidate { name, win, evidence, since_change: since };
        let cands = [
            c("a", 70.0, 100.0, None),
            c("b", 65.0, 100.0, Some(5.0)),
            c("cap", 60.0, 100.0, None),
            c("near", 52.2, 100.0, None),
            c("few", 75.0, 10.0, None),
            c("in", 50.0, 100.0, None),
        ];
        let before: BTreeMap<String, i32> = [("cap".to_string(), -CAP)].into_iter().collect();
        let rules = Rules { count: 60, max_step: 50, ..RULES };
        let could = qualified(&cands, &before, rules);
        let text = summarize(&cands, &before, rules, could.len(), could.len().min(rules.count));
        assert!(text.starts_with("6 champions rated, 5 with 30 games or more, 4 of those outside 48-52%"), "{text}");
        assert!(text.contains("1 waiting") && text.contains("1 at the 50% limit") && text.contains("1 too close"), "{text}");
        assert!(text.contains("1 could change, 1 changed (balance_count 60)"), "{text}");
    }

    #[test]
    fn the_item_news_names_each_item_its_builders_and_numbers() {
        let mut round = Round { id: 4, version: "2027.5.0".into(), label: "2027.5.0".into(), ..Round::default() };
        assert_eq!(item_news(&round, true, &|_| Vec::new()), None, "no item changed: no news");
        round.items = vec![
            ItemChange { item: "staff".into(), name: "大天使之杖".into(), users: 6, win: 43.5, step: 6, total: 6 },
            ItemChange { item: "rfc".into(), name: "疾射火炮".into(), users: 4, win: 64.5, step: -10, total: -20 },
        ];
        let lines = |c: &ItemChange| {
            if c.item == "rfc" {
                vec![("攻击速度".to_string(), "31".to_string(), "28".to_string())]
            } else {
                Vec::new()
            }
        };
        let (title, content) = item_news(&round, true, &lines).unwrap();
        assert_eq!(title, "电竞大脑装备平衡 2027.5.0");
        let rows: Vec<&str> = content.lines().collect();
        assert_eq!(rows.len(), 3);
        assert!(rows[0].contains("特效不变"), "{}", rows[0]);
        assert_eq!(rows[1], "【削弱】疾射火炮：出它的 4 个英雄平均胜率约 64.5% → 装备数值 -10%（累计 -20%）：攻击速度 31 → 28");
        assert!(rows[2].starts_with("【加强】大天使之杖") && !rows[2].ends_with("："), "{}", rows[2]);
        let (title, content) = item_news(&round, false, &lines).unwrap();
        assert_eq!(title, "Patch Meta AI item balance 2027.5.0");
        assert!(
            content.contains("[Nerf] 疾射火炮: its 4 builders win about 64.5% -> item numbers -10% (-20% in all): 攻击速度 31 → 28"),
            "{content}"
        );
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
        assert_eq!(items_of(round_for("on".into(), true, true, true)), Some(Some(-10)));
        assert_eq!(items_of(round_for("on".into(), false, true, false)), Some(Some(-10)), "a later run follows the first");
        // with it off: the champions' power, no items - for its replays too, after the book is read again
        let off = round_for("off".into(), true, true, false).expect("a run power");
        assert_eq!(off.champions.get("maddie"), Some(&-10));
        assert!(off.items.is_none());
        assert_eq!(items_of(round_for("off".into(), false, true, true)), None);
        close();
        open_file(file.clone());
        assert_eq!(items_of(round_for("off".into(), false, true, true)), None);
        assert_eq!(items_of(round_for("on".into(), false, true, true)), Some(Some(-10)));
        let _ = std::fs::remove_file(&file);
        reset_for_tests();
    }
}
