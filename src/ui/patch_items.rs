//! The item changes in the encyclopedia's patch history (2.12.2; the user: 「等等装备补丁 为什么不会显示在历史补丁里面」).
//! The game's patch notes (`NewsType::PatchNote`: version, added champion, champion and game-rule changes and their
//! reasons) have no place for items, so a round's item changes are news of their own (`balance::item_news`, 2.9.0) and
//! the patch history (游戏百科 -> 补丁历史, the game's `patch_history_info`) never showed them.
//!
//! This adds them under the patch the page shows on its right (`news_component/patch_content`): the champions' list
//! (`scroll`) gives up the room and our block ([`SECTION`]) sits below it, always in sight - one line an item: its
//! icon and name, the step and the total, its numbers before and after with the stat icons, its builders' win rate.
//! Which patch it is comes from its champions: the page's titles are a text reference whose version is bound where the
//! mod cannot read it (`#asset/base/text/news?patch.title`, seen in game 2026-11-18), but each entry's name is the
//! game's reference to its champion, so the champions shown with their side (buff, nerf) find the round
//! (`balance::round_of_champions`). The game makes the content again for another patch; the block follows it. Once a
//! session the right panel's tree goes to `ui_patch_history.txt`, so a game update that moves the page can be seen.

use std::collections::BTreeSet;

use super::names::NameBook;
use super::{color, quote, Ui};
use crate::balance::ItemChange;
use crate::items::{Catalog, STATS};

/// The main screen's right side: the patch history is one of its children (`<child>.data.right_panel`).
pub const RIGHT: &str = "main.top.right";
/// Our block's name in the patch content.
pub const SECTION: &str = "pma_patch_items";
/// The stat icons of the ban/pick screen (4+6's sheet), as the game's change rows show them.
const STAT_ICONS: &str = "asset/base/ui/banpick/champion_stat_icon";
/// The game's patch content: its width, its champions' list's height (`news_component/patch_content`).
const WIDTH: i32 = 1055;
const SCROLL_H: i32 = 830;
/// The list keeps at least this much.
const SCROLL_MIN: i32 = 360;
const TITLE_H: i32 = 44;
const ROW_H: i32 = 28;
const LINE: u32 = 0x4a4c_56ff;
const GREY: u32 = 0x8b8d_9aff;
const WHITE: u32 = 0xe8e8_e8ff;
const UP: u32 = 0x37d5_b3ff;
const DOWN: u32 = 0xe5_4d4dff;
/// The champions shown are read again every this many frames.
const LOOK_EVERY: u32 = 10;
/// Frames after the page's content first appears before its tree is written (laid out by then).
const DUMP_AFTER: u32 = 30;

/// A patch's champions as the page shows them: (id, buff).
pub type Shown = BTreeSet<(String, bool)>;
/// The round a patch's champions belong to: its label and its item changes.
pub type Found = Option<(String, Vec<ItemChange>)>;

/// The patch history's item block.
#[derive(Default)]
pub struct PatchItems {
    page: Option<String>,
    frame: u32,
    /// The content and the champions last read there; whether our block is in it.
    seen: Option<(String, Shown)>,
    placed: bool,
    dump_in: Option<u32>,
    dumped: bool,
}

impl PatchItems {
    /// One frame: `catalog` the pack's items, `names` the champions' names, `find` the round of a patch's champions
    /// ([`crate::balance::round_of_champions`]).
    pub fn tick(&mut self, ui: &mut impl Ui, catalog: Option<&Catalog>, names: &NameBook, find: &dyn Fn(&Shown) -> Found) {
        self.frame = self.frame.wrapping_add(1);
        if self.page.as_ref().is_none_or(|p| !ui.exists(p)) {
            self.page = ui.children(RIGHT).into_iter().map(|c| format!("{RIGHT}.{c}.data.right_panel")).find(|p| ui.exists(p));
        }
        let Some(panel) = self.page.clone() else { return };
        let Some(content) = ui.children(&panel).into_iter().map(|c| format!("{panel}.{c}")).find(|c| ui.exists(&format!("{c}.scroll.contents"))) else {
            return;
        };
        if let Some(n) = self.dump_in.as_mut() {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.dump_in = None;
                super::explore::write_tree(ui, &panel, "ui_patch_history.txt", "patch history");
            }
        } else if !self.dumped {
            self.dumped = true;
            self.dump_in = Some(DUMP_AFTER);
        }
        let ours = format!("{content}.{SECTION}");
        let fresh = self.seen.as_ref().is_none_or(|(c, _)| *c != content) || (self.placed && !ui.exists(&ours));
        if !fresh && !self.frame.is_multiple_of(LOOK_EVERY) {
            return;
        }
        let shown = shown_champions(ui, &format!("{content}.scroll.contents"), names);
        if !fresh && self.seen.as_ref().is_some_and(|(_, s)| *s == shown) {
            return;
        }
        self.seen = Some((content.clone(), shown.clone()));
        // what was there for another patch goes
        if ui.exists(&ours) {
            ui.remove(&ours);
        }
        self.placed = false;
        let found = if shown.is_empty() { None } else { find(&shown) };
        let lines = match (&found, catalog) {
            (Some((_, items)), Some(catalog)) if !items.is_empty() => lines_of(items, catalog, true),
            _ => Vec::new(),
        };
        let scroll = format!("{content}.scroll");
        if lines.is_empty() {
            ui.set_properties(&scroll, &format!("height: {SCROLL_H}px;"));
            return;
        }
        let (source, height) = block(&lines, true);
        ui.set_properties(&scroll, &format!("height: {}px;", (SCROLL_H - height).max(SCROLL_MIN)));
        if ui.spawn(&content, &source) {
            self.placed = true;
            let label = found.as_ref().map_or("", |(l, _)| l.as_str());
            crate::diag::log_once(
                &format!("patch-items-{label}"),
                &format!("[ui] patch history {content}: {} champions shown = round {label}, {} item lines under the list ({height} px)", shown.len(), lines.len()),
            );
        }
    }
}

/// The champions the patch content lists with their side: each entry's name in the buff and nerf columns, a reference
/// to the champion (or its name). The columns are found under the content (two levels down at most).
pub fn shown_champions(ui: &impl Ui, contents: &str, names: &NameBook) -> Shown {
    let mut out = Shown::new();
    let mut level = vec![contents.to_string()];
    for _ in 0..3 {
        let mut next = Vec::new();
        for node in &level {
            if ui.exists(&format!("{node}.buff_column")) || ui.exists(&format!("{node}.nerf_column")) {
                for (column, buff) in [("buff_column", true), ("nerf_column", false)] {
                    let column = format!("{node}.{column}");
                    for entry in ui.children(&column) {
                        let Some(text) = ui.text(&format!("{column}.{entry}.name")) else { continue };
                        if let Some(id) = super::balance_view::champion_of(&text, names) {
                            out.insert((id, buff));
                        }
                    }
                }
            } else {
                next.extend(ui.children(node).into_iter().map(|c| format!("{node}.{c}")));
            }
        }
        if !out.is_empty() || next.is_empty() {
            break;
        }
        level = next;
    }
    out
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

/// A round's item changes, one line each (rich text): the item's icon and name, the step in the patch colour and the
/// total, its numbers before and after (the notes' rule, [`crate::items::lines`]) with the stat icons, and in grey
/// its builders' win rate. Buffs first, the biggest steps first.
pub fn lines_of(changes: &[ItemChange], catalog: &Catalog, chinese: bool) -> Vec<String> {
    let mut changes: Vec<&ItemChange> = changes.iter().collect();
    changes.sort_by_key(|c| (c.step < 0, -c.step.abs()));
    changes
        .into_iter()
        .map(|c| {
            let item = catalog.item(&c.item).map(|(_, item)| item);
            let icon = item.map_or(String::new(), |i| crate::items::icon_text(&i.icon));
            let name = if c.name.is_empty() { c.item.as_str() } else { c.name.as_str() };
            let numbers: Vec<String> = item
                .map(|item| {
                    crate::items::lines(item, c.total - c.step, c.total, chinese)
                        .into_iter()
                        .map(|(label, from, to)| {
                            let field = STATS.iter().find(|(_, _, zh, en)| *zh == label || *en == label).map_or("", |s| s.0);
                            let icon = stat_icon(field).map_or(String::new(), |f| format!("<i#{STAT_ICONS}:{f}>"));
                            format!("{icon}{label} {from} → {to}")
                        })
                        .collect()
                })
                .unwrap_or_default();
            let step = format!("<{}>{:+}%<>", color(if c.step > 0 { UP } else { DOWN }), c.step);
            let grey = |t: String| format!("<{}>{t}<>", color(GREY));
            match (c.reset.is_some(), chinese) {
                (true, true) => format!("{icon}{name}  {step}  {}  {}", numbers.join("，"), grey("设置里列为不调整（item_balance_skip），取消之前的改动".into())),
                (true, false) => format!("{icon}{name}  {step}  {}  {}", numbers.join(", "), grey("item_balance_skip lists it: earlier changes taken off".into())),
                (false, true) => format!(
                    "{icon}{name}  {step}（累计 {:+}%）  {}  {}",
                    c.total,
                    numbers.join("，"),
                    grey(format!("出它的 {} 个英雄近期胜率约 {:.0}%", c.users, c.win))
                ),
                (false, false) => format!(
                    "{icon}{name}  {step} ({:+}% in all)  {}  {}",
                    c.total,
                    numbers.join(", "),
                    grey(format!("its {} builders win about {:.0}%", c.users, c.win))
                ),
            }
        })
        .collect()
}

/// The block's `.ui` source and height: a rule, the title, then a line an item.
pub fn block(lines: &[String], chinese: bool) -> (String, i32) {
    let title = if chinese { "装备（电竞大脑装备平衡）" } else { "Items (Patch Meta AI)" };
    let mut body = format!(
        "#bar:color {{ x: 0px; y: 6px; width: {WIDTH}px; height: 1px; color: {}; ignore_event: true; }} \
         #title:label {{ @\"asset/base/style/main#bold_label\"; x: 8px; y: 8px; width: 1040px; height: {}px; size: 20; align_y: Center; color: {}; text: {}; }} ",
        color(LINE),
        TITLE_H - 8,
        color(WHITE),
        quote(title)
    );
    for (k, line) in lines.iter().enumerate() {
        body.push_str(&format!(
            "#i{k}:label {{ @\"asset/base/style/main#label\"; x: 16px; y: {}px; width: 1030px; height: {ROW_H}px; size: 16; align_y: Center; color: {}; text: {}; }} ",
            TITLE_H + k as i32 * ROW_H,
            color(WHITE),
            quote(line)
        ));
    }
    let height = TITLE_H + lines.len() as i32 * ROW_H + 8;
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

    fn names() -> NameBook {
        let mut n = NameBook::default();
        for id in ["league_diana", "league_tryndamere", "league_tristana", "league_gnar", "league_ahri"] {
            n.learn(&crate::ui::names::name_ref(id), id);
        }
        n
    }

    /// The round of 2028.2.1 by its champions; any other set: none.
    fn find(shown: &Shown) -> Found {
        let want: Shown = [("league_diana", true), ("league_tryndamere", true), ("league_tristana", false), ("league_gnar", false)]
            .into_iter()
            .map(|(c, b)| (c.to_string(), b))
            .collect();
        (*shown == want).then(|| ("2028.2.1".to_string(), vec![change("item_storm", "风暴狂涌", 2, 2), change("item_heart", "心之钢", 1, 1), change("item_rod", "虚空之杖", -6, -6)]))
    }

    /// The patch history as the game lays it out: the page, its right panel, the patch content with the champions'
    /// columns (each entry's name a reference to its champion).
    fn page(ui: &mut FakeUi, buffs: &[&str], nerfs: &[&str]) -> String {
        for p in ["main.top.right", "main.top.right.patch_history_info", "main.top.right.patch_history_info.data"] {
            ui.add(p, "empty");
        }
        let panel = "main.top.right.patch_history_info.data.right_panel";
        ui.add(panel, "color");
        let content = format!("{panel}.patch_content");
        for p in [content.clone(), format!("{content}.top"), format!("{content}.scroll"), format!("{content}.scroll.contents"), format!("{content}.scroll.contents.columns")] {
            ui.add(&p, "empty");
        }
        ui.add(&format!("{content}.top.title"), "label").text = Some("#asset/base/text/news?patch.title".into());
        for (column, ids) in [("buff_column", buffs), ("nerf_column", nerfs)] {
            let col = format!("{content}.scroll.contents.columns.{column}");
            ui.add(&col, "empty");
            for (k, id) in ids.iter().enumerate() {
                ui.add(&format!("{col}.champion_entry{k}"), "empty");
                ui.add(&format!("{col}.champion_entry{k}.name"), "label").text = Some(crate::ui::names::name_ref(id));
                ui.add(&format!("{col}.change_group{k}"), "empty");
            }
        }
        content
    }

    #[test]
    fn the_patch_is_found_by_its_champions_and_its_items_sit_under_the_list() {
        let cat = Catalog::parse(CATALOG).unwrap();
        let n = names();
        let mut ui = FakeUi::default();
        let content = page(&mut ui, &["league_diana", "league_tryndamere"], &["league_tristana", "league_gnar"]);
        let shown = shown_champions(&ui, &format!("{content}.scroll.contents"), &n);
        assert_eq!(shown.len(), 4, "{shown:?}");
        let mut view = PatchItems::default();
        view.tick(&mut ui, Some(&cat), &n, &find);
        let ours = format!("{content}.{SECTION}");
        assert!(ui.exists(&ours) && ui.exists(&format!("{ours}.i2")), "three item lines");
        let props = &ui.nodes[&format!("{content}.scroll")].props;
        assert!(props.last().is_some_and(|p| p.starts_with("height: ") && p != "height: 830px;"), "the list gives up the room: {props:?}");
        // the next frames: nothing again
        let spawned = ui.spawned.len();
        for _ in 0..25 {
            view.tick(&mut ui, Some(&cat), &n, &find);
        }
        assert_eq!(ui.spawned.len(), spawned);
        // another patch in the same content: the block goes, the list has its room again
        let col = format!("{content}.scroll.contents.columns.buff_column");
        ui.add(&format!("{col}.champion_entry0.name"), "label").text = Some(crate::ui::names::name_ref("league_ahri"));
        for _ in 0..LOOK_EVERY {
            view.tick(&mut ui, Some(&cat), &n, &find);
        }
        assert!(!ui.exists(&ours));
        assert_eq!(ui.nodes[&format!("{content}.scroll")].props.last().map(String::as_str), Some("height: 830px;"));
    }

    #[test]
    fn a_line_an_item() {
        let cat = Catalog::parse(CATALOG).unwrap();
        let (_, items) = find(&[("league_diana", true), ("league_tryndamere", true), ("league_tristana", false), ("league_gnar", false)]
            .into_iter()
            .map(|(c, b)| (c.to_string(), b))
            .collect())
        .unwrap();
        let lines = lines_of(&items, &cat, true);
        assert_eq!(lines.len(), 3);
        let green = color(UP);
        let grey = color(GREY);
        assert_eq!(
            lines[0],
            format!("<i#asset/base/aseprite_resources/ingame/item_icons_18x18:i_storm>风暴狂涌  <{green}>+2%<>（累计 +2%）  <i#{STAT_ICONS}:ap_0>法术强度 90 → 92  <{grey}>出它的 4 个英雄近期胜率约 44%<>")
        );
        assert!(lines[1].contains("心之钢") && lines[1].contains("生命值 900 → 909"), "{}", lines[1]);
        assert!(lines[2].contains("虚空之杖") && lines[2].contains(&format!("<{}>-6%<>", color(DOWN))) && lines[2].contains("法术强度 70 → 66"), "{}", lines[2]);
        let (source, height) = block(&lines, true);
        assert!(source.starts_with("pma_patch_items:empty") && source.contains("#i2:label"), "{source}");
        assert_eq!(height, TITLE_H + 3 * ROW_H + 8);
    }
}
