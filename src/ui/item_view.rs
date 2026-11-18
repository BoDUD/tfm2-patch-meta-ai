//! The 4+6 items' numbers with the item balance on (2.12.1; the user: 「能真正的改游戏内装备属性的文本吗 不能是吗」, then
//! 「做」). The 4+6 pack registers its items with the game (the stable API's items), and the game's own item page shows
//! them - 游戏百科 -> 装备信息, and the strategy screen's item popup: `data.item_detail.name` names the item and
//! `data.item_detail.desc` gives its numbers one line each, made from the game's templates
//! ([`super::item_spec::SPEC`], e.g. 攻击力 +{Value}) with the numbers the pack registered, then the pack's own text.
//!
//! The pack's files are not this mod's (the user: 「第三方的装备mod不用你管」), so its numbers stay as they are; an
//! item a balance round changed gets, in those lines of its page, the numbers its holders play with - the rule of the
//! correction buff in the matches ([`crate::items::at`]: its power's share of each number, rounded; an unsigned number
//! never down) - in the patch colours of the ban/pick legend. Only lines that show the item's own number change, so
//! a page in any language, or one the game already wrote otherwise, is left as it is when nothing fits. This runs
//! after the game's screens every frame, as the champions' stat panels do ([`super::balance_view`]): a description the
//! game writes again (another item, the same one again) gets the numbers again before the frame is drawn.

use std::collections::{BTreeMap, HashMap};

use super::balance_view::{DOWN, UP};
use super::Ui;
use crate::items::{Catalog, Item, STATS};

/// The strategy screen's item popup.
pub const POPUP: &str = "main.item_info_popup.popup.content.item_info.data.item_detail";
/// The main screen's right side: the encyclopedia's item page is one of its children (`<child>.data.item_detail`).
pub const RIGHT: &str = "main.top.right";
/// The reference the game names an item by (`#asset/base/text/item?<key>.name`).
const NAME_REF: &str = "#asset/base/text/item?";

/// The item pages' descriptions with the item balance on.
#[derive(Default)]
pub struct ItemView {
    /// The encyclopedia's item page (its `item_detail`) when last found.
    page: Option<String>,
    /// Each description written: (the game's text, ours).
    written: HashMap<String, (String, String)>,
}

impl ItemView {
    /// One frame: `catalog` the pack's items, `power` each item's power in the newest round (None: no round).
    pub fn tick(&mut self, ui: &mut impl Ui, catalog: Option<&Catalog>, power: Option<&BTreeMap<String, i32>>) {
        let (Some(catalog), Some(power)) = (catalog, power) else { return };
        if self.page.as_ref().is_none_or(|p| !ui.exists(p)) {
            self.page = ui.children(RIGHT).into_iter().map(|c| format!("{RIGHT}.{c}.data.item_detail")).find(|p| ui.exists(&format!("{p}.desc")));
        }
        let panels: Vec<String> = [Some(POPUP.to_string()), self.page.clone()].into_iter().flatten().collect();
        for panel in panels {
            let desc_path = format!("{panel}.desc");
            let (Some(name), Some(desc)) = (ui.text(&format!("{panel}.name")), ui.text(&desc_path)) else { continue };
            if name.trim().is_empty() || desc.trim().is_empty() {
                continue;
            }
            if self.written.get(&desc_path).is_some_and(|(_, ours)| *ours == desc) {
                continue;
            }
            // once a page and session: what the game writes there (a game update may change it)
            crate::diag::log_once(
                &format!("item-view-seen-{panel}"),
                &format!("[ui] item page {panel}: name {name:?}, desc {:?}", crate::diag::clip(&desc.replace('\n', "\\n"), 240)),
            );
            let Some(item) = item_of(&name, catalog) else { continue };
            let pct = power.get(&item.key).copied().unwrap_or(0);
            if pct == 0 {
                self.written.remove(&desc_path);
                continue;
            }
            let color = super::color(if pct > 0 { UP } else { DOWN });
            let Some(ours) = rewrite(&desc, item, pct, &color) else {
                crate::diag::log_once(
                    &format!("item-view-none-{}", item.key),
                    &format!("[ui] item page {panel}: {} ({}) at {pct:+}%, but no line of its description shows its own numbers", item.name, item.key),
                );
                continue;
            };
            if ui.set_text(&desc_path, &ours) {
                crate::diag::log_once(
                    &format!("item-view-{panel}"),
                    &format!("[ui] item page {panel}: {} at {pct:+}%: {:?} -> {:?}", item.key, crate::diag::clip(&desc, 120), crate::diag::clip(&ours, 160)),
                );
                self.written.insert(desc_path, (desc, ours));
            }
        }
    }
}

/// The item a page's name label names: the game's reference to its name, or the name itself (the catalog's, the
/// pack's Chinese; one item of that name only).
pub fn item_of<'a>(text: &str, catalog: &'a Catalog) -> Option<&'a Item> {
    let text = super::balance_view::plain(text);
    let text = text.trim();
    if let Some(rest) = text.strip_prefix(NAME_REF) {
        return catalog.item(rest.strip_suffix(".name")?).map(|(_, item)| item);
    }
    let mut named = catalog.items.iter().filter(|i| i.name == text);
    let item = named.next()?;
    named.next().is_none().then_some(item)
}

/// The templates of a number field ([`super::item_spec::SPEC`]).
fn templates(field: &str) -> &'static [&'static str] {
    super::item_spec::SPEC.iter().find(|(f, _)| *f == field).map_or(&[], |(_, t)| *t)
}

/// `desc` with `item`'s numbers at `pct` percent of power: each line made from a template of one of its numbers (in
/// any language) that shows the item's own number gets the number its holders play with, in `color`; the other lines
/// stay. None: no line changed (none fits, or no number moves).
pub fn rewrite(desc: &str, item: &Item, pct: i32, color: &str) -> Option<String> {
    let mut changed = false;
    let lines: Vec<String> = desc
        .split('\n')
        .map(|line| {
            let flat = super::balance_view::plain(line);
            for (i, own) in &item.stats {
                let Some((field, signed, ..)) = STATS.get(*i) else { continue };
                let now = crate::items::at(*own, pct, *signed);
                if (now - own).abs() < 0.5 {
                    continue;
                }
                for template in templates(field) {
                    let Some((before, after)) = template.split_once("{Value}") else { continue };
                    let Some(shown) = flat.strip_prefix(before).and_then(|rest| rest.strip_suffix(after)) else { continue };
                    if shown.trim().parse::<f64>().is_ok_and(|n| (n - own).abs() < 0.01) {
                        changed = true;
                        return format!("{before}<{color}>{}<>{after}", crate::balance::number(now));
                    }
                }
            }
            line.to_string()
        })
        .collect();
    changed.then(|| lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::FakeUi;

    const CATALOG: &str = r#"{"version": 1, "sourceModId": "alele_runes_and_items", "items": [
        {"gameKey": "item_t5_134_1_2_101_0_0_0", "name": "黑色切割者", "tier": 5, "itemKind": "equipment",
         "packageValues": {"stats": {"attack": 40, "hp": 400, "skill_cooldown_mult": 20}}},
        {"gameKey": "item_t5_9_0_0_1_0_0_0", "name": "坚定之心", "tier": 5, "itemKind": "equipment",
         "packageValues": {"stats": {"hp": 300, "toughness": 20}}},
        {"gameKey": "item_t5_dup_a", "name": "同名", "tier": 5, "itemKind": "equipment", "packageValues": {"stats": {"hp": 100}}},
        {"gameKey": "item_t5_dup_b", "name": "同名", "tier": 5, "itemKind": "equipment", "packageValues": {"stats": {"hp": 200}}}
    ]}"#;
    const BC: &str = "item_t5_134_1_2_101_0_0_0";
    /// 黑色切割者's page as the game writes it (the user's screenshot, 2026-11-18).
    const BC_DESC: &str = "攻击力 +40\n生命值 +400\n技能急速 +20\n\n总价：3000\n切割\n对敌方英雄造成物理伤害时，使其护甲降低6%，持续6秒，最多5层";

    fn catalog() -> Catalog {
        Catalog::parse(CATALOG).unwrap()
    }

    fn power(items: &[(&str, i32)]) -> BTreeMap<String, i32> {
        items.iter().map(|(k, p)| (k.to_string(), *p)).collect()
    }

    fn page(ui: &mut FakeUi, root: &str, name: &str, desc: &str) {
        ui.add(root, "empty");
        ui.add(&format!("{root}.name"), "label").text = Some(name.into());
        ui.add(&format!("{root}.desc"), "label").text = Some(desc.into());
    }

    #[test]
    fn a_changed_item_shows_the_numbers_its_holders_play_with() {
        let cat = catalog();
        let bc = cat.item(BC).unwrap().1;
        let orange = crate::ui::color(DOWN);
        // -10%: 40 -> 36, 400 -> 360, 20 -> 18, in the patch colour; the pack's own text stays
        let out = rewrite(BC_DESC, bc, -10, &orange).unwrap();
        assert_eq!(
            out,
            format!("攻击力 +<{orange}>36<>\n生命值 +<{orange}>360<>\n技能急速 +<{orange}>18<>\n\n总价：3000\n切割\n对敌方英雄造成物理伤害时，使其护甲降低6%，持续6秒，最多5层")
        );
        // any language's lines: the game's English ones
        let en = rewrite("Attack Damage +40\nHP +400\nAbility Haste +20", bc, 10, "#00ff00ff").unwrap();
        assert!(en.starts_with("Attack Damage +<#00ff00ff>44<>\nHP +<#00ff00ff>440<>"), "{en}");
        // a small step that moves no number, and a description without the item's own numbers: left alone
        assert_eq!(rewrite("攻击力 +40\n技能急速 +20", bc, 1, &orange), None, "1% of 40 and of 20 rounds to 0");
        assert!(rewrite(BC_DESC, bc, 1, &orange).is_some_and(|t| t.contains(">404<>")), "1% of 400 is 4");
        assert_eq!(rewrite("攻击力 +36\n生命值 +360", bc, -10, &orange), None, "numbers that are not the item's own");
        // an unsigned number never goes down (tenacity), a signed one does
        let heart = cat.item("item_t5_9_0_0_1_0_0_0").unwrap().1;
        let out = rewrite("生命值 +300\n韧性 +20%", heart, -20, &orange).unwrap();
        assert_eq!(out, format!("生命值 +<{orange}>240<>\n韧性 +20%"));
        let up = rewrite("生命值 +300\n韧性 +20%", heart, 20, "#0b7f58ff").unwrap();
        assert_eq!(up, "生命值 +<#0b7f58ff>360<>\n韧性 +<#0b7f58ff>24<>%");
    }

    #[test]
    fn the_name_label_by_reference_or_by_name() {
        let cat = catalog();
        assert_eq!(item_of(&format!("{NAME_REF}{BC}.name"), &cat).map(|i| i.key.as_str()), Some(BC));
        assert_eq!(item_of("黑色切割者", &cat).map(|i| i.key.as_str()), Some(BC));
        assert!(item_of("同名", &cat).is_none(), "two items of that name: which one is unknown");
        assert!(item_of("无此装备", &cat).is_none());
    }

    #[test]
    fn the_encyclopedia_page_and_the_popup_are_written_once_and_again_after_the_game() {
        let cat = catalog();
        let mut ui = FakeUi::default();
        ui.add("main.top.right", "empty");
        ui.add("main.top.right.item_info", "item_info_ui");
        ui.add("main.top.right.item_info.data", "empty");
        let page_root = "main.top.right.item_info.data.item_detail";
        page(&mut ui, page_root, &format!("{NAME_REF}{BC}.name"), BC_DESC);
        page(&mut ui, POPUP, "黑色切割者", BC_DESC);
        let mut view = ItemView::default();
        let p = power(&[(BC, -10)]);
        view.tick(&mut ui, Some(&cat), Some(&p));
        let desc = |ui: &FakeUi, root: &str| ui.text(&format!("{root}.desc")).unwrap();
        assert!(desc(&ui, page_root).contains(">36<>"), "{}", desc(&ui, page_root));
        assert!(desc(&ui, POPUP).contains(">360<>"), "{}", desc(&ui, POPUP));
        // the next frame: ours is there, nothing written again
        let before = desc(&ui, page_root);
        view.tick(&mut ui, Some(&cat), Some(&p));
        assert_eq!(desc(&ui, page_root), before);
        // the game writes the page again: ours again
        ui.add(&format!("{page_root}.desc"), "label").text = Some(BC_DESC.into());
        view.tick(&mut ui, Some(&cat), Some(&p));
        assert!(desc(&ui, page_root).contains(">36<>"));
        // an item without power, or no round: the game's text
        ui.add(&format!("{page_root}.desc"), "label").text = Some(BC_DESC.into());
        view.tick(&mut ui, Some(&cat), Some(&power(&[])));
        assert_eq!(desc(&ui, page_root), BC_DESC);
        view.tick(&mut ui, Some(&cat), None);
        assert_eq!(desc(&ui, page_root), BC_DESC);
    }
}
