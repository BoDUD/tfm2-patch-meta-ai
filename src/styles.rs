//! AI 战队的风格（2.14.0）。用户 2026-11-23 说现在玩得有点无聊，在几个方向里选了「AI 战队有风格」。
//!
//! 每支队看它最近 [`WINDOW`] 场比赛选过的英雄：英雄联盟英雄的阵容属性（前排、开团、保护、消耗、突进、后期、控制，
//! 知识库里的 0-2 分）取平均，各职业（战士、射手、法师、辅助、刺客、坦克）取占比，再和全联盟的队伍比成 z 值。
//! 比平均高出一个标准差以上的前一两项就是它的风格（开团型、偏爱刺客……），都没有就是均衡型。
//!
//! AI 替这支队选人时（`ai_bp`），每个英雄按它和这支队风格的方向有多一致（余弦）得一点加分，只用来在差不多好的
//! 几个里挑：每一手最多让出 `ai_style` 个胜率点（`draft::varied_choice`）。风格会自己延续下去：照风格选，风格就更稳。
//! 预测对方选人、赛前前瞻和 BP 界面也用它。

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};

use crate::history::Game;
use crate::meta::Meta;

/// 阵容属性几项，加上六个职业
pub const TRAITS: usize = 7;
pub const CATEGORIES: usize = 6;
pub const FEATURES: usize = TRAITS + CATEGORIES;
/// 每支队看最近这么多场；少于 [`MIN_GAMES`] 场不定风格
pub const WINDOW: usize = 40;
pub const MIN_GAMES: usize = 10;
/// 高出这么多个标准差才算风格
pub const LABEL_Z: f32 = 1.0;
/// 风格加分最多这么多（对数几率）
pub const SCALE: f32 = 0.15;

const TRAIT_ZH: [&str; TRAITS] = ["前排型", "开团型", "保护型", "消耗型", "突进型", "后期型", "控制型"];
const TRAIT_EN: [&str; TRAITS] = ["frontline", "engage", "peel", "poke", "dive", "late game", "crowd control"];
const CATEGORY_ZH: [&str; CATEGORIES] = ["偏爱战士", "偏爱射手", "偏爱法师", "偏爱辅助", "偏爱刺客", "偏爱坦克"];
const CATEGORY_EN: [&str; CATEGORIES] = ["loves fighters", "loves marksmen", "loves mages", "loves supports", "loves assassins", "loves tanks"];

/// 一个英雄的特征：阵容属性（不知道的是 None）和职业
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Features {
    pub traits: Option<[f32; TRAITS]>,
    pub category: Option<u8>,
}

/// 一支队的风格
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TeamStyle {
    pub z: [f32; FEATURES],
    pub label_zh: String,
    pub label_en: String,
    pub games: usize,
    /// 每个英雄的风格加分（对数几率）
    pub bonus: HashMap<u16, f32>,
}

impl TeamStyle {
    pub fn label(&self, chinese: bool) -> &str {
        if chinese {
            &self.label_zh
        } else {
            &self.label_en
        }
    }
}

/// 所有队的风格
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Styles {
    pub teams: HashMap<u32, TeamStyle>,
}

static STYLES: RwLock<Option<Arc<Styles>>> = RwLock::new(None);

pub fn set(styles: Option<Styles>) {
    *STYLES.write().unwrap_or_else(PoisonError::into_inner) = styles.map(Arc::new);
}

pub fn get() -> Option<Arc<Styles>> {
    STYLES.read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// 一个英雄的特征向量：阵容属性（不知道的填全联盟平均）和职业的独热
fn vector(f: &Features, trait_mean: &[f32; TRAITS]) -> [f32; FEATURES] {
    let mut out = [0.0; FEATURES];
    let traits = f.traits.unwrap_or(*trait_mean);
    out[..TRAITS].copy_from_slice(&traits);
    if let Some(c) = f.category.filter(|c| (*c as usize) < CATEGORIES) {
        out[TRAITS + c as usize] = 1.0;
    }
    out
}

fn mean_and_sd(vectors: &[[f32; FEATURES]]) -> ([f32; FEATURES], [f32; FEATURES]) {
    let n = vectors.len().max(1) as f32;
    let mut mu = [0.0f32; FEATURES];
    for v in vectors {
        for k in 0..FEATURES {
            mu[k] += v[k] / n;
        }
    }
    let mut var = [0.0f32; FEATURES];
    for v in vectors {
        for k in 0..FEATURES {
            var[k] += (v[k] - mu[k]).powi(2) / n;
        }
    }
    (mu, var.map(|x| x.sqrt().max(1e-3)))
}

/// 按每支队最近选过的英雄算风格。`picks`：每支队按时间排好的比赛（旧的在前），每场是这支队的英雄；`features` 给每个英雄
/// 的特征；`champions` 是要算加分的英雄。
pub fn build_from(picks: &HashMap<u32, Vec<Vec<u16>>>, features: &dyn Fn(u16) -> Features, champions: &[u16]) -> Styles {
    let recent = |games: &Vec<Vec<u16>>| -> std::ops::Range<usize> { games.len().saturating_sub(WINDOW)..games.len() };
    // 全联盟选过的英雄：平均和标准差（按选的次数算），用来把每个英雄的偏离标准化
    let all: Vec<Features> = picks.values().flat_map(|games| games[recent(games)].iter().flatten().map(|c| features(*c))).collect();
    if all.is_empty() {
        return Styles::default();
    }
    let known: Vec<[f32; TRAITS]> = all.iter().filter_map(|f| f.traits).collect();
    let mut trait_mean = [0.0f32; TRAITS];
    for t in &known {
        for k in 0..TRAITS {
            trait_mean[k] += t[k] / known.len() as f32;
        }
    }
    let vectors: Vec<[f32; FEATURES]> = all.iter().map(|f| vector(f, &trait_mean)).collect();
    let (mu, sd) = mean_and_sd(&vectors);
    // 每支队的平均
    let mut means: Vec<(u32, [f32; FEATURES], usize)> = Vec::new();
    for (team, games) in picks {
        let range = recent(games);
        if range.len() < MIN_GAMES {
            continue;
        }
        let list: Vec<[f32; FEATURES]> = games[range.clone()].iter().flatten().map(|c| vector(&features(*c), &trait_mean)).collect();
        if !list.is_empty() {
            means.push((*team, mean_and_sd(&list).0, range.len()));
        }
    }
    if means.len() < 2 {
        return Styles::default();
    }
    // 队和队之间比
    let team_vectors: Vec<[f32; FEATURES]> = means.iter().map(|m| m.1).collect();
    let (team_mu, team_sd) = mean_and_sd(&team_vectors);
    // 每个英雄偏离平均英雄的方向（标准化）
    let directions: Vec<(u16, [f32; FEATURES])> = champions
        .iter()
        .map(|c| {
            let v = vector(&features(*c), &trait_mean);
            let mut d = [0.0f32; FEATURES];
            for k in 0..FEATURES {
                d[k] = (v[k] - mu[k]) / sd[k];
            }
            (*c, d)
        })
        .collect();
    let mut out = Styles::default();
    for (team, m, games) in means {
        let mut z = [0.0f32; FEATURES];
        for k in 0..FEATURES {
            z[k] = ((m[k] - team_mu[k]) / team_sd[k]).clamp(-3.0, 3.0);
        }
        let (label_zh, label_en) = label(&z);
        let w = z.map(|x| x.clamp(-2.0, 2.0));
        let norm_w = w.iter().map(|x| x * x).sum::<f32>().sqrt();
        let strength = (norm_w / 2.0).min(1.0);
        let mut bonus = HashMap::new();
        if norm_w > 1e-3 {
            for (c, d) in &directions {
                let norm_d = d.iter().map(|x| x * x).sum::<f32>().sqrt();
                if norm_d < 1e-3 {
                    continue;
                }
                let cos = w.iter().zip(d).map(|(a, b)| a * b).sum::<f32>() / (norm_w * norm_d);
                bonus.insert(*c, SCALE * cos * strength);
            }
        }
        out.teams.insert(team, TeamStyle { z, label_zh, label_en, games, bonus });
    }
    out
}

/// 风格的名字：高出 [`LABEL_Z`] 的前两项，都没有就是均衡型
pub fn label(z: &[f32; FEATURES]) -> (String, String) {
    let mut high: Vec<(usize, f32)> = z.iter().copied().enumerate().filter(|(_, v)| *v >= LABEL_Z).collect();
    high.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
    if high.is_empty() {
        return ("均衡型".to_string(), "balanced".to_string());
    }
    let name = |k: usize, zh: bool| -> &'static str {
        match (k < TRAITS, zh) {
            (true, true) => TRAIT_ZH[k],
            (true, false) => TRAIT_EN[k],
            (false, true) => CATEGORY_ZH[k - TRAITS],
            (false, false) => CATEGORY_EN[k - TRAITS],
        }
    };
    let top: Vec<usize> = high.iter().take(2).map(|(k, _)| *k).collect();
    let zh: Vec<&str> = top.iter().map(|k| name(*k, true)).collect();
    let en: Vec<&str> = top.iter().map(|k| name(*k, false)).collect();
    (zh.join(" · "), en.join(", "))
}

/// 从这个存档的比赛和模型算出所有队的风格；`categories` 是每个英雄（名字）的职业
pub fn build<'a>(games: impl Iterator<Item = &'a Game>, meta: &Meta, categories: &HashMap<String, u8>) -> Styles {
    let mut by_team: HashMap<u32, Vec<(usize, Vec<u16>)>> = HashMap::new();
    for g in games.filter(|g| !g.solo) {
        for side in 0..2 {
            let Some(team) = g.teams[side] else { continue };
            let picks: Vec<u16> = g.sides[side].iter().map(|s| s.champ).collect();
            by_team.entry(team).or_default().push((g.record, picks));
        }
    }
    let picks: HashMap<u32, Vec<Vec<u16>>> = by_team
        .into_iter()
        .map(|(t, mut list)| {
            list.sort_by_key(|(record, _)| *record);
            (t, list.into_iter().map(|(_, p)| p).collect())
        })
        .collect();
    let features = |c: u16| Features {
        traits: meta.profile(c).map(|p| [p.front, p.engage, p.peel, p.poke, p.dive, p.carry, p.cc]),
        category: categories.get(meta.names.name(c)).copied(),
    };
    let champions: Vec<u16> = meta.champions.iter().filter_map(|c| meta.names.get(&c.name)).collect();
    build_from(&picks, &features, &champions)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 8 个英雄：0-3 是开团的坦克，4-7 是消耗的法师
    fn features(c: u16) -> Features {
        if c < 4 {
            Features { traits: Some([2.0, 2.0, 0.0, 0.0, 0.0, 0.0, 1.0]), category: Some(5) }
        } else {
            Features { traits: Some([0.0, 0.0, 0.0, 2.0, 0.0, 1.0, 0.0]), category: Some(2) }
        }
    }

    #[test]
    fn a_team_that_keeps_picking_poke_mages_gets_that_style_and_bonus() {
        let mut picks: HashMap<u32, Vec<Vec<u16>>> = HashMap::new();
        // 2 号队老拿消耗法师，别的队多拿坦克
        picks.insert(1, (0..20).map(|_| vec![0, 1, 2, 4, 5]).collect());
        picks.insert(2, (0..20).map(|_| vec![4, 5, 6, 0, 1]).collect());
        picks.insert(3, (0..20).map(|_| vec![0, 1, 4, 5, 2]).collect());
        picks.insert(4, (0..20).map(|_| vec![0, 1, 2, 4, 5]).collect());
        picks.insert(5, (0..5).map(|_| vec![0, 1, 2, 3, 4]).collect());
        let champions: Vec<u16> = (0..8).collect();
        let styles = build_from(&picks, &features, &champions);
        assert!(!styles.teams.contains_key(&5), "too few games");
        let mage = &styles.teams[&2];
        assert!(mage.label_zh.contains("消耗型") || mage.label_zh.contains("偏爱法师"), "{}", mage.label_zh);
        // 给法师加分、坦克减分，最多 SCALE
        assert!(mage.bonus[&6] > 0.05 && mage.bonus[&0] < -0.05, "{:?}", mage.bonus);
        assert!(mage.bonus.values().all(|b| b.abs() <= SCALE + 1e-6));
        assert_eq!(mage.games, 20);
        // 没有比赛就没有风格
        assert!(build_from(&HashMap::new(), &features, &champions).teams.is_empty());
    }

    #[test]
    fn a_team_near_the_middle_is_balanced() {
        assert_eq!(label(&[0.2; FEATURES]), ("均衡型".to_string(), "balanced".to_string()));
        let mut z = [0.0; FEATURES];
        z[1] = 1.5;
        z[TRAITS + 4] = 2.1;
        assert_eq!(label(&z), ("偏爱刺客 · 开团型".to_string(), "loves assassins, engage".to_string()));
    }
}
