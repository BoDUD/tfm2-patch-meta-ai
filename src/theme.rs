//! 主题版本（2.14.0）。用户 2026-11-23 说现在玩得有点无聊，在几个方向里选了「补丁预告和主题版本」。
//!
//! 每隔 `balance_theme_every` 个游戏版本（热修复不算），平衡补丁挑一类英雄（战士、射手、法师、辅助、刺客、坦克），
//! 这一类全部加强 `balance_theme_step`%，补丁公告和新闻里写明是主题版本。之后的版本照常按胜率调整：太强的会被
//! 砍回来，本来就弱的就留着，等于每隔一段时间版本风向变一次。最近两次用过的类别不选，剩下的按版本号算出来挑一个，
//! 同一个版本不管什么时候算（提前准备的那一轮和正式的那一轮）都挑同一个。

use crate::balance::Round;

/// 游戏的英雄职业（`ChampionCategoryV1`）：近战、远程、法师、辅助、刺客、坦克
pub const CATEGORIES: [u8; 6] = [0, 1, 2, 3, 4, 5];
/// 一类至少这么多英雄才拿来做主题
pub const MIN_CHAMPIONS: usize = 3;

pub fn name(category: u8, chinese: bool) -> &'static str {
    match (category, chinese) {
        (0, true) => "战士",
        (1, true) => "射手",
        (2, true) => "法师",
        (3, true) => "辅助",
        (4, true) => "刺客",
        (5, true) => "坦克",
        (0, false) => "Fighters",
        (1, false) => "Marksmen",
        (2, false) => "Mages",
        (3, false) => "Supports",
        (4, false) => "Assassins",
        (5, false) => "Tanks",
        (_, true) => "其他",
        (_, false) => "Others",
    }
}

/// 这一轮该不该出主题：上次主题以后的游戏版本（热修复、只取消调整的轮次不算）加上这一轮够 `every` 个
pub fn due(rounds: &[Round], every: u32) -> bool {
    if every == 0 {
        return false;
    }
    let since = rounds.iter().rev().take_while(|r| r.theme.is_none()).filter(|r| r.merged && !r.reset).count();
    since as u32 + 1 >= every
}

/// 挑一类：最近两次主题用过的不要，剩下的里按版本号挑（同一个版本总是同一类）
pub fn pick(rounds: &[Round], version: &str, available: &[u8]) -> Option<u8> {
    let recent: Vec<u8> = rounds.iter().rev().filter_map(|r| r.theme).take(2).collect();
    let mut choices: Vec<u8> = available.iter().copied().filter(|c| !recent.contains(c)).collect();
    if choices.is_empty() {
        choices = available.to_vec();
    }
    choices.sort_unstable();
    choices.dedup();
    if choices.is_empty() {
        return None;
    }
    // 版本号算一个数（FNV-1a），不用随机数，免得两次算出来不一样
    let mut h: u32 = 0x811c_9dc5;
    for b in version.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    Some(choices[h as usize % choices.len()])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(id: u32, merged: bool, theme: Option<u8>) -> Round {
        Round { id, version: format!("1.{id}"), merged, theme, ..Round::default() }
    }

    #[test]
    fn a_theme_comes_every_few_game_patches() {
        assert!(!due(&[], 0), "off");
        assert!(due(&[], 1));
        // 上次主题以后两个游戏版本，加上这一轮够 3 个
        let rounds = vec![round(1, true, Some(4)), round(2, true, None), round(3, false, None), round(4, true, None)];
        assert!(due(&rounds, 3));
        assert!(!due(&rounds, 4));
        // 热修复不算
        assert!(!due(&rounds[..3], 3));
    }

    #[test]
    fn the_same_patch_always_picks_the_same_class_but_not_the_last_two() {
        let rounds = vec![round(1, true, Some(4)), round(2, true, Some(1)), round(3, true, None)];
        let a = pick(&rounds, "2028.7.0", &CATEGORIES).unwrap();
        assert_eq!(pick(&rounds, "2028.7.0", &CATEGORIES), Some(a));
        assert!(a != 4 && a != 1, "{a}");
        // 只剩用过的：也照样挑
        assert!(pick(&rounds, "x", &[4]).is_some());
        assert!(pick(&rounds, "x", &[]).is_none());
        assert_eq!(name(4, true), "刺客");
    }
}
