//! The item changes in the encyclopedia's patch history (2.12.2; the user: 「等等装备补丁 为什么不会显示在历史补丁里面」).
//! The game's patch notes (`NewsType::PatchNote`: version, added champion, champion and game-rule changes and their
//! reasons) have no place for items, so a round's item changes are news of their own (`balance::item_news`, 2.9.0) and
//! the patch history (游戏百科 -> 补丁历史, the game's `patch_history_info`) never showed them.
//!
//! This adds them under the patch the page shows on its right (`news_component/patch_content`): the champions' list
//! (`scroll`) gives up the room and our block ([`SECTION`]) sits below it, always in sight - one line an item: its
//! icon and name, the step and the total, its numbers before and after with the stat icons, its builders' win rate.
//! Which patch it is comes from its champions: the page's titles are a text reference whose version is bound where the
//! mod cannot read it (`#asset/base/text/news?patch.title`, seen in game 2026-11-18), and the game names every entry
//! of a column `champion_entry`, so a path reaches the first one only (`ui_patch_history.txt` of 2026-11-18: 40 buff
//! entries, one path). But a column's children are listed one by one, and the first entry's name is the game's
//! reference to its champion: each column's count and first champion find the round (`balance::round_of_patch`; the
//! game lists a round's champions in the round's order). The game makes the content again for another patch; the
//! block follows it. Once a session the right panel's tree goes to `ui_patch_history.txt`, so a game update that moves
//! the page can be seen.
//!
//! 2.13.0 起防御塔、小龙、大龙的改动也写在这个块里（装备后面，没有装备改动就只有它们）。

use super::names::NameBook;
use super::{color, quote, Ui};
use crate::balance::ItemChange;
use crate::items::{Catalog, STATS};
use crate::objectives::ObjectiveChange;

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

/// A patch as the page shows it: each column's count of entries and its first champion (buffs, nerfs).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    pub buffs: usize,
    pub first_buff: Option<String>,
    pub nerfs: usize,
    pub first_nerf: Option<String>,
}

impl Shown {
    pub fn is_empty(&self) -> bool {
        self.buffs + self.nerfs == 0
    }
}
/// The round a patch's champions belong to: its label, its item changes and (2.13.0) its tower / monster changes.
pub type Found = Option<(String, Vec<ItemChange>, Vec<ObjectiveChange>)>;

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
        let shown = shown_patch(ui, &format!("{content}.scroll.contents"), names);
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
        let mut lines = match (&found, catalog) {
            (Some((_, items, _)), Some(catalog)) if !items.is_empty() => lines_of(items, catalog, true),
            _ => Vec::new(),
        };
        let items = lines.len();
        if let Some((_, _, objectives)) = &found {
            lines.extend(objective_lines(objectives, true));
        }
        let scroll = format!("{content}.scroll");
        if lines.is_empty() {
            ui.set_properties(&scroll, &format!("height: {SCROLL_H}px;"));
            return;
        }
        let (source, height) = block(&lines, title(items > 0, lines.len() > items, true));
        // the list ends where the block fits in the panel's sight (the game's list runs 57 px past the panel's bottom)
        let room = match (ui.rect(&panel), ui.rect(&scroll)) {
            (Some((_, py, _, ph)), Some((_, sy, _, _))) if ph > 0.0 && sy > py => (py + ph - sy).round() as i32 - 8,
            _ => SCROLL_H,
        };
        ui.set_properties(&scroll, &format!("height: {}px;", (room - height).max(SCROLL_MIN)));
        if ui.spawn(&content, &source) {
            self.placed = true;
            let label = found.as_ref().map_or("", |(l, _, _)| l.as_str());
            crate::diag::log_once(
                &format!("patch-items-{label}"),
                &format!(
                    "[ui] patch history {content}: {shown:?} = round {label}, {items} item lines + {} tower / monster lines under the list ({height} px)",
                    lines.len() - items
                ),
            );
        }
    }
}

/// The patch the content lists: each column's count of entries (the game names them all `champion_entry`) and the
/// champion of its first (its name a reference to the champion, or the name). The columns are found under the
/// content, two levels down at most.
pub fn shown_patch(ui: &impl Ui, contents: &str, names: &NameBook) -> Shown {
    let mut level = vec![contents.to_string()];
    for _ in 0..3 {
        let mut next = Vec::new();
        for node in &level {
            if !(ui.exists(&format!("{node}.buff_column")) || ui.exists(&format!("{node}.nerf_column"))) {
                next.extend(ui.children(node).into_iter().map(|c| format!("{node}.{c}")));
                continue;
            }
            let column = |side: &str| -> (usize, Option<String>) {
                let column = format!("{node}.{side}");
                let entries: Vec<String> = ui.children(&column).into_iter().filter(|c| c.starts_with("champion_entry")).collect();
                let first = entries.first().and_then(|e| ui.text(&format!("{column}.{e}.name"))).and_then(|t| super::balance_view::champion_of(&t, names));
                (entries.len(), first)
            };
            let ((buffs, first_buff), (nerfs, first_nerf)) = (column("buff_column"), column("nerf_column"));
            return Shown { buffs, first_buff, nerfs, first_nerf };
        }
        if next.is_empty() {
            break;
        }
        level = next;
    }
    Shown::default()
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

/// 塔和龙的改动，一样一行：名字、改了多少（颜色）和累计、生命值改前改后、灰字写量到了什么
pub fn objective_lines(changes: &[ObjectiveChange], chinese: bool) -> Vec<String> {
    let mut changes: Vec<&ObjectiveChange> = changes.iter().collect();
    changes.sort_by_key(|c| c.objective);
    changes
        .into_iter()
        .map(|c| {
            let (name, hp, why) = crate::objectives::history_parts(c, chinese);
            let step = format!("<{}>{:+}%<>", color(if c.step > 0 { UP } else { DOWN }), c.step);
            let grey = format!("<{}>{why}<>", color(GREY));
            let total = match (c.reset.is_some(), chinese) {
                (true, _) => String::new(),
                (false, true) => format!("（累计 {:+}%）", c.total),
                (false, false) => format!(" ({:+}% in all)", c.total),
            };
            format!("{name}  {step}{total}  {hp}  {grey}")
        })
        .collect()
}

/// 块的标题：有什么写什么
pub fn title(items: bool, objectives: bool, chinese: bool) -> &'static str {
    match (items, objectives, chinese) {
        (true, true, true) => "装备、防御塔和大小龙（电竞大脑）",
        (false, true, true) => "防御塔和大小龙（电竞大脑）",
        (_, false, true) => "装备（电竞大脑装备平衡）",
        (true, true, false) => "Items, towers and epic monsters (Patch Meta AI)",
        (false, true, false) => "Towers and epic monsters (Patch Meta AI)",
        (_, false, false) => "Items (Patch Meta AI)",
    }
}

/// The block's `.ui` source and height: a rule, the title, then a line an item.
pub fn block(lines: &[String], title: &str) -> (String, i32) {
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

    /// 2028.2.1 as the page shows it: two buffs from Diana, two nerfs from Tristana.
    fn shown_221() -> Shown {
        Shown { buffs: 2, first_buff: Some("league_diana".into()), nerfs: 2, first_nerf: Some("league_tristana".into()) }
    }

    /// The round of 2028.2.1 by its champions; any other patch: none.
    fn find(shown: &Shown) -> Found {
        (*shown == shown_221()).then(|| {
            ("2028.2.1".to_string(), vec![change("item_storm", "风暴狂涌", 2, 2), change("item_heart", "心之钢", 1, 1), change("item_rod", "虚空之杖", -6, -6)], tower())
        })
    }

    fn tower() -> Vec<ObjectiveChange> {
        vec![ObjectiveChange {
            objective: crate::objectives::Objective::Tower,
            measured: 12.6,
            evidence: 412,
            step: 15,
            total: 15,
            reset: None,
            band: (19.0, 24.0),
            base: 3713,
        }]
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
        let shown = shown_patch(&ui, &format!("{content}.scroll.contents"), &n);
        assert_eq!(shown, shown_221());
        let mut view = PatchItems::default();
        view.tick(&mut ui, Some(&cat), &n, &find);
        let ours = format!("{content}.{SECTION}");
        assert!(ui.exists(&ours) && ui.exists(&format!("{ours}.i3")), "three item lines and the towers'");
        let props = &ui.nodes[&format!("{content}.scroll")].props;
        assert!(props.last().is_some_and(|p| p.starts_with("height: ") && p != "height: 830px;"), "the list gives up the room: {props:?}");
        // the next frames: nothing again
        let spawned = ui.spawned.len();
        for _ in 0..25 {
            view.tick(&mut ui, Some(&cat), &n, &find);
        }
        assert_eq!(ui.spawned.len(), spawned);
        // another patch in the same content (another first buff): the block goes, the list has its room again
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
        let (_, items, _) = find(&shown_221()).unwrap();
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
        let (source, height) = block(&lines, title(true, false, true));
        assert!(source.starts_with("pma_patch_items:empty") && source.contains("#i2:label"), "{source}");
        assert_eq!(height, TITLE_H + 3 * ROW_H + 8);
    }

    #[test]
    fn a_line_for_the_towers_and_monsters() {
        let lines = objective_lines(&tower(), true);
        assert_eq!(
            lines,
            [format!("防御塔  <{}>+15%<>（累计 +15%）  生命值 3713 → 4270  <{}>最近 412 场平均 12.6 分钟，目标 19–24 分钟<>", color(UP), color(GREY))]
        );
        assert_eq!(title(false, true, true), "防御塔和大小龙（电竞大脑）");
    }
}
