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
//!
//! 2.14.2（用户：「装备的平衡补丁现在文字显示排版很差…排版参考官方英雄补丁那种排版是不是更美观」）：块里照官方英雄补丁
//! 排——左边「增强」右边「削弱」两栏，每一项一个图标格子和名字（粗体），下面竖线边上一句理由（官方理由行的灰）、一行一个
//! 数值「攻击力: 75 → 69」（属性图标，改后的数字带颜色），最后是整体强度。尺寸和颜色抄游戏自己的
//! `news_component/patch_*` 布局（patch_champion_entry、patch_change_group、patch_change_row、patch_reason_row）。块还是放在
//! 英雄列表下面、一直看得到，项多了块里自己滚动（游戏的 scroll_view，内容高度写死，不靠自动排版）。

use super::names::NameBook;
use super::{color, quote, Ui};
use crate::balance::{percent, ItemChange, ARROW};
use crate::items::{field_lines, stat_icon, Catalog, ICON_SHEET, STAT_ICONS};
use crate::objectives::ObjectiveChange;

/// The main screen's right side: the patch history is one of its children (`<child>.data.right_panel`).
pub const RIGHT: &str = "main.top.right";
/// Our block's name in the patch content.
pub const SECTION: &str = "pma_patch_items";
/// The game's patch content: its width, its champions' list's height (`news_component/patch_content`).
const WIDTH: i32 = 1055;
const SCROLL_H: i32 = 830;
/// The list keeps at least this much.
const SCROLL_MIN: i32 = 360;
const TITLE_H: i32 = 44;
/// 块至少这么高（标题加两行）
const BLOCK_MIN: i32 = 150;
const LINE: u32 = 0x4a4c_56ff;
const WHITE: u32 = 0xe8e8_e8ff;
const UP: u32 = 0x37d5_b3ff;
const DOWN: u32 = 0xe5_4d4dff;
/// 官方补丁的尺寸和颜色（`news_component/patch_*`）：两栏的宽、间隔和左边距，栏标题、一项名字那一行的高，改动组的
/// 左边距和上下边距，一行的高和行距，理由的灰，竖线的颜色
const COL_W: i32 = 513;
const COL_GAP: i32 = 12;
const PAD_X: i32 = 8;
const HEADER_H: i32 = 44;
const ENTRY_H: i32 = 44;
const GROUP_LEFT: i32 = 30;
const GROUP_PAD: i32 = 8;
const ROW_H: i32 = 26;
const ROW_GAP: i32 = 4;
const REASON: u32 = 0x6670_85ff;
const RULE: u32 = 0xdce1_e8ff;
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
        let mut entries = match (&found, catalog) {
            (Some((label, items, _)), Some(catalog)) if !items.is_empty() => item_entries(items, catalog, label, true),
            _ => Vec::new(),
        };
        let items = entries.len();
        if let Some((_, _, objectives)) = &found {
            entries.extend(objective_entries(objectives, true));
        }
        let scroll = format!("{content}.scroll");
        if entries.is_empty() {
            ui.set_properties(&scroll, &format!("height: {SCROLL_H}px;"));
            return;
        }
        // the list ends where the block fits in the panel's sight (the game's list runs 57 px past the panel's bottom)
        let room = match (ui.rect(&panel), ui.rect(&scroll)) {
            (Some((_, py, _, ph)), Some((_, sy, _, _))) if ph > 0.0 && sy > py => (py + ph - sy).round() as i32 - 8,
            _ => SCROLL_H,
        };
        let (source, height) = block(&entries, title(items > 0, entries.len() > items, true), (room - SCROLL_MIN).max(BLOCK_MIN), true);
        ui.set_properties(&scroll, &format!("height: {}px;", (room - height).max(SCROLL_MIN)));
        if ui.spawn(&content, &source) {
            self.placed = true;
            let label = found.as_ref().map_or("", |(l, _, _)| l.as_str());
            crate::diag::log_once(
                &format!("patch-items-{label}"),
                &format!(
                    "[ui] patch history {content}: {shown:?} = round {label}, {items} items + {} towers / monsters under the list ({height} px)",
                    entries.len() - items
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

/// 块里的一项（装备，或者塔、龙）：图标格子（图集和帧，没有就不画）、名字、一句理由、几行改动（属性图标的帧和富文本）、
/// 是不是增强（决定放在哪一栏）
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub icon: Option<(String, String)>,
    pub name: String,
    pub reason: String,
    pub rows: Vec<(Option<&'static str>, String)>,
    pub buff: bool,
}

/// 一行改动的文字：「名字: 改前 → 改后」，改后的数字带颜色（游戏的 `patch.stat` 一个样子）
fn change_text(label: &str, from: &str, to: &str, buff: bool) -> String {
    format!("{label}: {from} {ARROW} <{}>{to}<>", color(if buff { UP } else { DOWN }))
}

/// 一轮的装备改动，一件一项：图标、名字、理由（[`crate::balance::item_reason`]，和收件箱新闻同一句）、每个变了的数值
/// （补丁的规则，[`field_lines`]）、整体强度。增强在前，改得多的在前。`label`：这一轮的版本（挑理由用）。
pub fn item_entries(changes: &[ItemChange], catalog: &Catalog, label: &str, chinese: bool) -> Vec<Entry> {
    let mut changes: Vec<&ItemChange> = changes.iter().collect();
    changes.sort_by_key(|c| (c.step < 0, -c.step.abs()));
    let power = if chinese { "整体强度" } else { "Overall power" };
    changes
        .into_iter()
        .map(|c| {
            let item = catalog.item(&c.item).map(|(_, item)| item);
            let buff = c.step > 0;
            let mut rows: Vec<(Option<&'static str>, String)> = item
                .map(|item| {
                    field_lines(item, c.total - c.step, c.total, chinese)
                        .into_iter()
                        .map(|(field, label, from, to)| (stat_icon(field), change_text(&label, &from, &to, buff)))
                        .collect()
                })
                .unwrap_or_default();
            rows.push((None, change_text(power, &percent(c.total - c.step), &percent(c.total), buff)));
            Entry {
                icon: item.filter(|i| crate::items::icon_fits(&i.icon)).map(|i| (ICON_SHEET.to_string(), i.icon.clone())),
                name: if c.name.is_empty() { c.item.clone() } else { c.name.clone() },
                reason: crate::balance::item_reason(c, crate::balance::seed(&c.item, label), chinese),
                rows,
                buff,
            }
        })
        .collect()
}

/// 塔和龙的改动，一样一项：名字、量到了什么（理由）、生命值改前改后、整体强度
pub fn objective_entries(changes: &[ObjectiveChange], chinese: bool) -> Vec<Entry> {
    let mut changes: Vec<&ObjectiveChange> = changes.iter().collect();
    changes.sort_by_key(|c| c.objective);
    let power = if chinese { "整体强度" } else { "Overall power" };
    changes
        .into_iter()
        .map(|c| {
            let (name, _, why) = crate::objectives::history_parts(c, chinese);
            let buff = c.step > 0;
            let mut rows = Vec::new();
            if let Some((a, b)) = crate::objectives::numbers(c) {
                rows.push((stat_icon("hp"), change_text(if chinese { "生命值" } else { "Health" }, &a.to_string(), &b.to_string(), buff)));
            }
            if c.reset.is_none() {
                rows.push((None, change_text(power, &percent(c.total - c.step), &percent(c.total), buff)));
            }
            Entry { icon: None, name, reason: why, rows, buff }
        })
        .collect()
}

/// 块的标题：有什么写什么
pub fn title(items: bool, objectives: bool, chinese: bool) -> &'static str {
    match (items, objectives, chinese) {
        (true, true, true) => "装备、防御塔和大小龙（电竞大脑）",
        (false, true, true) => "防御塔和大小龙（电竞大脑）",
        (_, false, true) => "装备调整（电竞大脑装备平衡）",
        (true, true, false) => "Items, towers and epic monsters (Patch Meta AI)",
        (false, true, false) => "Towers and epic monsters (Patch Meta AI)",
        (_, false, false) => "Items (Patch Meta AI)",
    }
}

/// 一段文字在 `width` 像素宽、`size` 号字里大概占几行：富文本标签不算，汉字一个字宽，别的字一半多，宁多勿少
pub fn text_lines(text: &str, width: i32, size: i32) -> i32 {
    let mut plain = String::new();
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => plain.push(c),
            _ => {}
        }
    }
    let wide = |c: char| c as u32 >= 0x2e80;
    let px: f32 = plain.chars().map(|c| if wide(c) { size as f32 } else { size as f32 * 0.6 }).sum();
    ((px * 1.05 / width.max(1) as f32).ceil() as i32).max(1)
}

/// The block's `.ui` source and height: a rule, the title, then the entries in two columns as the game's patch notes
/// lay out champions (buffs left, nerfs right), in a scroll view of at most `max_h` (the block's whole height).
pub fn block(entries: &[Entry], title: &str, max_h: i32, chinese: bool) -> (String, i32) {
    let mut body = String::new();
    let mut heights = [0i32; 2];
    for (side, buff) in [(0usize, true), (1usize, false)] {
        let list: Vec<&Entry> = entries.iter().filter(|e| e.buff == buff).collect();
        if list.is_empty() {
            continue;
        }
        let x0 = PAD_X + side as i32 * (COL_W + COL_GAP);
        let header = match (buff, chinese) {
            (true, true) => "增强",
            (false, true) => "削弱",
            (true, false) => "Buffs",
            (false, false) => "Nerfs",
        };
        body.push_str(&format!(
            "#h{side}:label {{ x: {}px; y: 0px; width: 200px; height: {HEADER_H}px; align_y: Center; font: \"asset/base/font/set/bold\"; size: 24; color: {}; text: {}; ignore_event: true; }} ",
            x0 + 8,
            color(if buff { UP } else { DOWN }),
            quote(header)
        ));
        let mut y = HEADER_H;
        for (k, e) in list.iter().enumerate() {
            let id = format!("{side}_{k}");
            // 名字那一行（patch_champion_entry）：36×36 的格子（四条边、34×34 的图标），名字粗体 18
            let name_x = match &e.icon {
                Some((sheet, tag)) => {
                    let (sx, sy) = (x0 + 12, y + 4);
                    for (edge, (ex, ey, w, h)) in [("t", (sx, sy, 36, 1)), ("b", (sx, sy + 35, 36, 1)), ("l", (sx, sy, 1, 36)), ("r", (sx + 35, sy, 1, 36))] {
                        body.push_str(&format!(
                            "#s{id}{edge}:color {{ x: {ex}px; y: {ey}px; width: {w}px; height: {h}px; color: {}; ignore_event: true; }} ",
                            color(LINE)
                        ));
                    }
                    body.push_str(&format!(
                        "#p{id}:image {{ x: {}px; y: {}px; width: 34px; height: 34px; sample_linear: false; source: {}; rect_tag: {}; ignore_event: true; }} ",
                        sx + 1,
                        sy + 1,
                        quote(sheet),
                        quote(tag)
                    ));
                    sx + 36 + 8
                }
                None => x0 + 12,
            };
            body.push_str(&format!(
                "#n{id}:label {{ x: {name_x}px; y: {}px; width: {}px; height: 36px; align_y: Center; font: \"asset/base/font/set/bold\"; size: 18; color: {}; text: {}; ignore_event: true; }} ",
                y + 4,
                COL_W - (name_x - x0) - 8,
                color(WHITE),
                quote(&e.name)
            ));
            y += ENTRY_H;
            // 改动组（patch_change_group）：左边一条竖线，右边理由（patch_reason_row）和一行行改动（patch_change_row）
            let line_x = x0 + GROUP_LEFT;
            let text_x = line_x + 1 + 6;
            let text_w = COL_W - (text_x - x0);
            let top = y + GROUP_PAD;
            let mut yy = top;
            if !e.reason.is_empty() {
                let h = text_lines(&e.reason, text_w, 18) * ROW_H;
                body.push_str(&format!(
                    "#r{id}:label {{ x: {text_x}px; y: {yy}px; width: {text_w}px; height: {h}px; align_y: Center; font: \"asset/base/font/set/regular\"; size: 18; line_height: {ROW_H}; color: {}; text: {}; ignore_event: true; }} ",
                    color(REASON),
                    quote(&e.reason)
                ));
                yy += h + ROW_GAP;
            }
            for (j, (icon, text)) in e.rows.iter().enumerate() {
                let mut x = text_x;
                if let Some(frame) = icon {
                    body.push_str(&format!(
                        "#c{id}_{j}i:image {{ x: {x}px; y: {}px; width: 16px; height: 16px; sample_linear: false; source: {}; rect_tag: {}; ignore_event: true; }} ",
                        yy + (ROW_H - 16) / 2,
                        quote(STAT_ICONS),
                        quote(frame)
                    ));
                    x += 20;
                }
                body.push_str(&format!(
                    "#c{id}_{j}:label {{ x: {x}px; y: {yy}px; width: {}px; height: {ROW_H}px; align_y: Center; font: \"asset/base/font/set/regular\"; size: 18; color: {}; text: {}; ignore_event: true; }} ",
                    text_w - (x - text_x),
                    color(WHITE),
                    quote(text)
                ));
                yy += ROW_H + ROW_GAP;
            }
            let inner = (yy - ROW_GAP - top).max(ROW_H);
            body.push_str(&format!(
                "#l{id}:color {{ x: {line_x}px; y: {top}px; width: 1px; height: {inner}px; color: {}; ignore_event: true; }} ",
                color(RULE)
            ));
            y = top + inner + GROUP_PAD;
        }
        heights[side] = y;
    }
    let content_h = heights[0].max(heights[1]);
    let view_h = content_h.min((max_h - TITLE_H - 8).max(ROW_H));
    let height = TITLE_H + view_h + 8;
    let source = format!(
        "{SECTION}:empty {{ width: {WIDTH}px; height: {height}px; \
         #bar:color {{ x: 0px; y: 6px; width: {WIDTH}px; height: 1px; color: {}; ignore_event: true; }} \
         #title:label {{ @\"asset/base/style/main#bold_label\"; x: 8px; y: 8px; width: 1040px; height: {}px; size: 20; align_y: Center; color: {}; text: {}; ignore_event: true; }} \
         #view:scroll_view {{ focusable: true; x: 0px; y: {TITLE_H}px; width: {WIDTH}px; height: {view_h}px; speed: 100; bar_width: 4; \
         bar: {{ source: \"asset/base/sprite/white\"; color: #37d5b3ff; hover: {{ color: #ecfbf8ff; }} }} \
         back: {{ source: \"asset/base/sprite/white\"; color: #4a4c56ff; }} \
         #contents:empty {{ width: {}px; height: {content_h}px; {body}}} }} }}",
        color(LINE),
        TITLE_H - 8,
        color(WHITE),
        quote(title),
        PAD_X + 2 * COL_W + COL_GAP
    );
    (source, height)
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
        // 两个增强（风暴狂涌、心之钢）和塔在左栏，虚空之杖在右栏
        let contents = format!("{ours}.view.contents");
        for node in ["h0", "n0_0", "n0_1", "n0_2", "h1", "n1_0", "p0_0", "r1_0", "c1_0_0", "c1_0_0i"] {
            assert!(ui.exists(&format!("{contents}.{node}")), "{node}");
        }
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
    fn an_item_is_laid_out_like_a_champion_in_the_patch_notes() {
        let cat = Catalog::parse(CATALOG).unwrap();
        let (label, items, _) = find(&shown_221()).unwrap();
        let entries = item_entries(&items, &cat, &label, true);
        assert_eq!(entries.len(), 3);
        let storm = &entries[0];
        assert_eq!((storm.name.as_str(), storm.buff), ("风暴狂涌", true));
        assert_eq!(storm.icon, Some((ICON_SHEET.to_string(), "i_storm".to_string())));
        assert!(storm.reason.contains("44%"), "{}", storm.reason);
        assert_eq!(storm.rows[0], (Some("ap_0"), format!("法术强度: 90 {ARROW} <{}>92<>", color(UP))));
        assert_eq!(storm.rows.last().unwrap().1, format!("整体强度: 0% {ARROW} <{}>+2%<>", color(UP)));
        // 没有图标的装备不画格子；削弱用削弱的颜色
        let rod = &entries[2];
        assert!(rod.icon.is_none() && !rod.buff);
        assert_eq!(rod.rows[0].1, format!("法术强度: 70 {ARROW} <{}>66<>", color(DOWN)));
        let (source, height) = block(&entries, title(true, false, true), 400, true);
        assert!(source.starts_with("pma_patch_items:empty") && source.contains("#view:scroll_view"), "{source}");
        assert!(source.contains("rect_tag: \"i_storm\"") && source.contains("rect_tag: \"ap_0\""), "{source}");
        assert!(source.contains("text: \"增强\"") && source.contains("text: \"削弱\""), "{source}");
        assert!(height <= 400, "{height}");
        // 放不下就在块里滚动：内容比看得见的高
        let (small, small_h) = block(&entries, title(true, false, true), BLOCK_MIN, true);
        assert_eq!(small_h, BLOCK_MIN);
        assert!(small.contains(&format!("height: {}px; speed: 100", BLOCK_MIN - TITLE_H - 8)), "{small}");
    }

    #[test]
    fn the_towers_and_monsters_too() {
        let entries = objective_entries(&tower(), true);
        assert_eq!(entries.len(), 1);
        let t = &entries[0];
        assert_eq!((t.name.as_str(), t.buff, t.icon.is_none()), ("防御塔", true, true));
        assert_eq!(t.reason, "最近 412 场平均 12.6 分钟，目标 19–24 分钟");
        assert_eq!(t.rows[0], (Some("hp_0"), format!("生命值: 3713 {ARROW} <{}>4270<>", color(UP))));
        assert_eq!(t.rows[1].1, format!("整体强度: 0% {ARROW} <{}>+15%<>", color(UP)));
        assert_eq!(title(false, true, true), "防御塔和大小龙（电竞大脑）");
    }

    #[test]
    fn a_reason_takes_as_many_lines_as_it_needs() {
        assert_eq!(text_lines("短短一句。", 476, 18), 1);
        assert_eq!(text_lines(&"字".repeat(40), 476, 18), 2);
        assert_eq!(text_lines(&format!("<#667085ff>{}<>", "字".repeat(20)), 476, 18), 1, "tags take no room");
    }
}
