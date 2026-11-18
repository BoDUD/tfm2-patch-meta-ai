//! The item changes in the encyclopedia's patch history (2.12.2; the user: 「等等装备补丁 为什么不会显示在历史补丁里面」).
//! The game's patch notes (`NewsType::PatchNote`: version, added champion, champion and game-rule changes and their
//! reasons) have no place for items, so a round's item changes are news of their own (`balance::item_news`, 2.9.0) and
//! the patch history (游戏百科 -> 补丁历史, the game's `patch_history_info`) never showed them. This adds them at the
//! end of the patch the page shows on its right (`news_component/patch_content` -> `scroll.contents`), laid out like
//! the game's champion entries: buffs on the left, nerfs on the right; each item its icon and name, its builders' win
//! rate, the step and the total in the reasons' grey, and its numbers before and after with the stat icons. The
//! section is ours ([`SECTION`]); the game makes the content again for another patch, and the section comes again when
//! that patch changed items. Once a session the page's tree (with the section) goes to `ui_patch_history.txt`, so a game
//! update that moves the page can be seen.

use std::collections::HashMap;

use super::{color, quote, Ui};
use crate::balance::ItemChange;
use crate::items::{Catalog, STATS};

/// The main screen's right side: the patch history is one of its children (`<child>.data.right_panel`).
pub const RIGHT: &str = "main.top.right";
/// Our section's name in the patch content's `scroll.contents`.
pub const SECTION: &str = "pma_patch_items";
/// The stat icons of the ban/pick screen (4+6's sheet), as the game's change rows show them.
const STAT_ICONS: &str = "asset/base/ui/banpick/champion_stat_icon";
/// The game's patch content: its width, the columns' (x, width), its greys.
const WIDTH: i32 = 1055;
const COLUMNS: [i32; 2] = [8, 533];
const COLUMN_W: i32 = 513;
const LINE: u32 = 0x4a4c_56ff;
const GREY: u32 = 0x8b8d_9aff;
const WHITE: u32 = 0xe8e8_e8ff;
const UP: u32 = 0x37d5_b3ff;
const DOWN: u32 = 0xe5_4d4dff;
/// Frames after the section first appears before the page's tree is written (its rectangles laid out).
const DUMP_AFTER: u32 = 30;

/// One item of a patch, as the section shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub buff: bool,
    pub name: String,
    pub reasons: Vec<String>,
    pub numbers: Vec<String>,
}

/// The patch history's item section.
#[derive(Default)]
pub struct PatchItems {
    page: Option<String>,
    /// The contents and the patch our section was made for.
    shown: Option<(String, String)>,
    /// Each patch's entries (made once; none while the catalog is not read).
    entries: HashMap<String, Vec<Entry>>,
    /// Frames until the page's tree is written; written once a session.
    dump_in: Option<u32>,
    dumped: bool,
}

impl PatchItems {
    /// One frame: `catalog` the pack's items, `items_of` a patch's item changes (a round's label, as
    /// [`crate::balance::items_of`]).
    pub fn tick(&mut self, ui: &mut impl Ui, catalog: Option<&Catalog>, items_of: &dyn Fn(&str) -> Vec<ItemChange>) {
        if self.page.as_ref().is_none_or(|p| !ui.exists(p)) {
            self.page = ui.children(RIGHT).into_iter().map(|c| format!("{RIGHT}.{c}.data.right_panel")).find(|p| ui.exists(p));
        }
        let Some(panel) = self.page.clone() else { return };
        let Some(content) = ui.children(&panel).into_iter().map(|c| format!("{panel}.{c}")).find(|c| ui.exists(&format!("{c}.scroll.contents"))) else {
            return;
        };
        let contents = format!("{content}.scroll.contents");
        let ours = format!("{contents}.{SECTION}");
        if let Some(n) = self.dump_in.as_mut() {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.dump_in = None;
                self.dumped = true;
                super::explore::write_tree(ui, &panel, "ui_patch_history.txt", "patch history with the item section");
            }
        }
        let title = ui.text(&format!("{content}.top.title")).unwrap_or_default();
        let Some(version) = version_in(&title) else {
            crate::diag::log_once("patch-items-title", &format!("[ui] patch history {content}: no version in its title {title:?}"));
            return;
        };
        if ui.exists(&ours) && self.shown.as_ref().is_some_and(|(c, v)| *c == contents && *v == version) {
            return;
        }
        if ui.exists(&ours) {
            // another patch shown in the same content
            ui.remove(&ours);
        }
        self.shown = Some((contents.clone(), version.clone()));
        let chinese = title.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
        let entries = match self.entries.get(&version) {
            Some(e) => e.clone(),
            None => {
                let Some(catalog) = catalog else { return };
                let e = entries_of(&items_of(&version), catalog, chinese);
                self.entries.insert(version.clone(), e.clone());
                e
            }
        };
        if entries.is_empty() {
            return;
        }
        let (source, height) = section(&entries, chinese);
        if ui.spawn(&contents, &source) {
            crate::diag::log_once(
                "patch-items",
                &format!("[ui] patch history {content}: v{version}, {} item changes, a section of {height} px at the end of {contents}", entries.len()),
            );
            if !self.dumped && self.dump_in.is_none() {
                self.dump_in = Some(DUMP_AFTER);
            }
        }
    }
}

/// The patch version in a patch's title (`补丁公告 v2028.2.1`, a hotfix's `2028.2.0b`): the last run of three numbers
/// with dots, and a letter after it.
pub fn version_in(title: &str) -> Option<String> {
    let chars: Vec<char> = title.chars().collect();
    let mut found = None;
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        let mut dots = 0;
        while i < chars.len() && (chars[i].is_ascii_digit() || (chars[i] == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit))) {
            dots += usize::from(chars[i] == '.');
            i += 1;
        }
        if dots == 2 {
            if i < chars.len() && chars[i].is_ascii_lowercase() && !chars.get(i + 1).is_some_and(char::is_ascii_alphanumeric) {
                i += 1;
            }
            found = Some(chars[start..i].iter().collect());
        }
    }
    found
}

/// The stat icon of an item number field (4+6's sheet), if it has one.
fn stat_icon(field: &str) -> Option<&'static str> {
    Some(match field {
        "attack" => "ad_0",
        "magic_power" => "ap_0",
        "defence" => "armor_0",
        "hp" => "hp_0",
        "hp_regen" => "hp_regen_0",
        "attack_speed_mult" => "attack_speed_0",
        "skill_cooldown_mult" => "cdr_0",
        "crit_chance" => "crit_chance_0",
        "move_speed_mult" => "speed_0",
        "vamp" => "vamp_0",
        "defence_penetration" => "armor_pen_0",
        "magic_resistance_penetration" => "magic_pen_0",
        "toughness" => "tenacity_0",
        "range" => "range_0",
        _ => return None,
    })
}

/// A round's item changes as the section shows them: each item's icon and name, the reasons, its numbers before and
/// after (the notes' rule, [`crate::items::lines`]). Buffs first, the biggest steps first.
pub fn entries_of(changes: &[ItemChange], catalog: &Catalog, chinese: bool) -> Vec<Entry> {
    let mut changes: Vec<&ItemChange> = changes.iter().collect();
    changes.sort_by_key(|c| (c.step < 0, -c.step.abs()));
    changes
        .into_iter()
        .map(|c| {
            let item = catalog.item(&c.item).map(|(_, item)| item);
            let icon = item.map_or(String::new(), |i| crate::items::icon_text(&i.icon));
            let reasons = match (c.reset.is_some(), chinese) {
                (true, true) => vec![format!("设置里列为不调整（item_balance_skip），取消电竞大脑之前的 {:+}%。", -c.step)],
                (true, false) => vec![format!("item_balance_skip lists it: this mod's earlier {:+}% taken off.", -c.step)],
                (false, true) => vec![
                    format!("出它的 {} 个英雄近期平均胜率约 {:.0}%。", c.users, c.win),
                    format!("本次装备数值 {:+}%（只改数值，特效不变）。", c.step),
                    format!("累计 {:+}%。", c.total),
                ],
                (false, false) => vec![
                    format!("Its {} builders win about {:.0}% of late.", c.users, c.win),
                    format!("Item numbers {:+}% this patch (numbers only, its effects stay).", c.step),
                    format!("{:+}% in all.", c.total),
                ],
            };
            let numbers = item
                .map(|item| {
                    crate::items::lines(item, c.total - c.step, c.total, chinese)
                        .into_iter()
                        .map(|(label, from, to)| {
                            let field = STATS.iter().find(|(_, _, zh, en)| *zh == label || *en == label).map_or("", |s| s.0);
                            let icon = stat_icon(field).map_or(String::new(), |f| format!("<i#{STAT_ICONS}:{f}> "));
                            format!("{icon}{label}: {from} → {to}")
                        })
                        .collect()
                })
                .unwrap_or_default();
            Entry { buff: c.step > 0, name: format!("{icon}{}", if c.name.is_empty() { &c.item } else { &c.name }), reasons, numbers }
        })
        .collect()
}

/// The section's `.ui` source and height: a rule, the title, the columns' headers, then each column's entries laid out
/// as the game's (a 44 px name row, then a group 30 px in with a line: reasons of 20 px, numbers of 26 px).
pub fn section(entries: &[Entry], chinese: bool) -> (String, i32) {
    let mut body = String::new();
    let (title, buffs, nerfs) = if chinese { ("装备（电竞大脑装备平衡）", "增强", "削弱") } else { ("Items (Patch Meta AI)", "Buffs", "Nerfs") };
    let top = 76;
    body.push_str(&format!("#bar:color {{ x: 0px; y: 12px; width: {WIDTH}px; height: 1px; color: {}; ignore_event: true; }} ", color(LINE)));
    body.push_str(&format!(
        "#title:label {{ @\"asset/base/style/main#bold_label\"; x: 8px; y: 24px; width: 1000px; height: 44px; size: 24; align_y: Center; text: {}; }} ",
        quote(title)
    ));
    let mut bottom = top;
    for (side, x) in COLUMNS.iter().enumerate() {
        let mine: Vec<&Entry> = entries.iter().filter(|e| e.buff == (side == 0)).collect();
        if mine.is_empty() {
            continue;
        }
        let mut y = top;
        body.push_str(&format!(
            "#h{side}:label {{ @\"asset/base/style/main#bold_label\"; x: {}px; y: {y}px; width: 200px; height: 44px; size: 24; align_y: Center; color: {}; text: {}; }} ",
            x + 8,
            color(if side == 0 { UP } else { DOWN }),
            quote(if side == 0 { buffs } else { nerfs })
        ));
        y += 44;
        for (k, e) in mine.iter().enumerate() {
            body.push_str(&format!(
                "#e{side}_{k}:label {{ @\"asset/base/style/main#bold_label\"; x: {}px; y: {}px; width: {}px; height: 36px; size: 18; align_y: Center; color: {}; text: {}; }} ",
                x + 12,
                y + 4,
                COLUMN_W - 24,
                color(WHITE),
                quote(&e.name)
            ));
            y += 44;
            let group_top = y;
            y += 8;
            for (j, r) in e.reasons.iter().enumerate() {
                body.push_str(&format!(
                    "#r{side}_{k}_{j}:label {{ @\"asset/base/style/main#label\"; x: {}px; y: {y}px; width: {}px; height: 20px; size: 13; align_y: Center; color: {}; text: {}; }} ",
                    x + 37,
                    COLUMN_W - 45,
                    color(GREY),
                    quote(r)
                ));
                y += 24;
            }
            for (j, n) in e.numbers.iter().enumerate() {
                body.push_str(&format!(
                    "#n{side}_{k}_{j}:label {{ @\"asset/base/style/main#label\"; x: {}px; y: {y}px; width: {}px; height: 26px; size: 18; align_y: Center; color: {}; text: {}; }} ",
                    x + 37,
                    COLUMN_W - 45,
                    color(WHITE),
                    quote(n)
                ));
                y += 30;
            }
            y += 4;
            body.push_str(&format!(
                "#l{side}_{k}:color {{ x: {}px; y: {}px; width: 1px; height: {}px; color: {}; ignore_event: true; }} ",
                x + 30,
                group_top + 8,
                y - group_top - 12,
                color(LINE)
            ));
        }
        bottom = bottom.max(y);
    }
    let height = bottom + 16;
    (format!("{SECTION}:empty {{ width: {WIDTH}px; height: {height}px; ignore_event: true; {body}}}"), height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::FakeUi;

    const CATALOG: &str = r#"{"version": 1, "sourceModId": "alele_runes_and_items", "items": [
        {"gameKey": "item_storm", "name": "风暴狂涌", "tier": 5, "itemKind": "equipment", "icon": "i_storm",
         "packageValues": {"stats": {"magic_power": 90, "move_speed_mult": 6}}},
        {"gameKey": "item_heart", "name": "心之钢", "tier": 5, "itemKind": "equipment", "icon": "i_heart",
         "packageValues": {"stats": {"hp": 900}}},
        {"gameKey": "item_rod", "name": "虚空之杖", "tier": 5, "itemKind": "equipment",
         "packageValues": {"stats": {"magic_power": 70}}}
    ]}"#;

    fn change(item: &str, name: &str, step: i32, total: i32) -> ItemChange {
        ItemChange { item: item.into(), name: name.into(), users: 4, win: 44.1, step, total, reset: None }
    }

    fn patch(version: &str) -> Vec<ItemChange> {
        match version {
            "2028.2.1" => vec![change("item_storm", "风暴狂涌", 2, 2), change("item_heart", "心之钢", 1, 1), change("item_rod", "虚空之杖", -6, -6)],
            _ => Vec::new(),
        }
    }

    /// The patch history as the game lays it out: the page, its right panel, the patch content it made there.
    fn page(ui: &mut FakeUi, title: &str) -> String {
        for p in ["main.top.right", "main.top.right.patch_history_info", "main.top.right.patch_history_info.data"] {
            ui.add(p, "empty");
        }
        let panel = "main.top.right.patch_history_info.data.right_panel";
        ui.add(panel, "color");
        let content = format!("{panel}.patch_content");
        for p in [content.clone(), format!("{content}.top"), format!("{content}.scroll"), format!("{content}.scroll.contents")] {
            ui.add(&p, "empty");
        }
        ui.add(&format!("{content}.top.title"), "label").text = Some(title.into());
        format!("{content}.scroll.contents")
    }

    #[test]
    fn the_version_in_a_patch_title() {
        assert_eq!(version_in("补丁公告 v2028.2.1").as_deref(), Some("2028.2.1"));
        assert_eq!(version_in("Patch notes v2028.2.0b").as_deref(), Some("2028.2.0b"));
        assert_eq!(version_in("v2027.4.1b 补丁公告").as_deref(), Some("2027.4.1b"));
        assert_eq!(version_in("#asset/base/text/news?patch.title"), None);
        assert_eq!(version_in("第2028年 7.28"), None);
    }

    #[test]
    fn a_patch_with_item_changes_gets_the_section_once_and_again_for_another() {
        let cat = Catalog::parse(CATALOG).unwrap();
        let mut ui = FakeUi::default();
        let contents = page(&mut ui, "补丁公告 v2028.2.1");
        let mut view = PatchItems::default();
        view.tick(&mut ui, Some(&cat), &patch);
        let ours = format!("{contents}.{SECTION}");
        assert!(ui.exists(&ours), "the section");
        assert!(ui.exists(&format!("{ours}.h0")) && ui.exists(&format!("{ours}.h1")), "buffs and nerfs");
        assert!(ui.exists(&format!("{ours}.e0_0")) && ui.exists(&format!("{ours}.e1_0")));
        let spawned = ui.spawned.len();
        view.tick(&mut ui, Some(&cat), &patch);
        assert_eq!(ui.spawned.len(), spawned, "made once");
        // another patch without item changes in the same content: the section goes
        ui.add(&format!("{}.top.title", contents.trim_end_matches(".scroll.contents")), "label").text = Some("补丁公告 v2028.2.2".into());
        view.tick(&mut ui, Some(&cat), &patch);
        assert!(!ui.exists(&ours));
        // the catalog not read yet: nothing, and tried again later
        let mut ui = FakeUi::default();
        let contents = page(&mut ui, "补丁公告 v2028.2.1");
        let mut view = PatchItems::default();
        view.tick(&mut ui, None, &patch);
        assert!(!ui.exists(&format!("{contents}.{SECTION}")));
        view.tick(&mut ui, Some(&cat), &patch);
        assert!(ui.exists(&format!("{contents}.{SECTION}")));
    }

    #[test]
    fn the_entries_say_what_changed() {
        let cat = Catalog::parse(CATALOG).unwrap();
        let e = entries_of(&patch("2028.2.1"), &cat, true);
        assert_eq!(e.iter().map(|e| (e.buff, e.name.as_str())).collect::<Vec<_>>()[..2], [(true, "<i#asset/base/aseprite_resources/ingame/item_icons_18x18:i_storm>风暴狂涌"), (true, "<i#asset/base/aseprite_resources/ingame/item_icons_18x18:i_heart>心之钢")]);
        assert_eq!(e[0].reasons, ["出它的 4 个英雄近期平均胜率约 44%。", "本次装备数值 +2%（只改数值，特效不变）。", "累计 +2%。"]);
        assert_eq!(e[0].numbers, [format!("<i#{STAT_ICONS}:ap_0> 法术强度: 90 → 92")], "6% of move speed does not move");
        assert_eq!(e[1].numbers, [format!("<i#{STAT_ICONS}:hp_0> 生命值: 900 → 909")]);
        assert!(!e[2].buff && e[2].numbers == [format!("<i#{STAT_ICONS}:ap_0> 法术强度: 70 → 66")], "{:?}", e[2]);
        let (source, height) = section(&e, true);
        assert!(source.starts_with("pma_patch_items:empty") && source.contains("增强") && source.contains("削弱"), "{source}");
        assert!(height > 200, "{height}");
    }
}
