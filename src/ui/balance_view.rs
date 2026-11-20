//! The stat panels with the balance patches on (2.8.0, `balance`; the user: 「我希望游戏界面里可以看到智能bp的改动」).
//! The ban/pick screen's champion panel and its tooltip, and the encyclopedia's champion page, show the numbers a
//! champion plays with: the game's own numbers (what its labels say) times the buff of the champion's power
//! ([`crate::balance::factors`]: attack and magic power by the damage half, health by the other), and the numbers
//! the newest round changed in the patch colours of the ban/pick legend (版本加强 green, 削弱 orange). The
//! encyclopedia's "initial" values stay the game's.
//!
//! The labels are the game's own. This runs after the game's screens every frame (`post_update`), so a label the
//! game writes again (another champion, the same one again) gets the balance's number again before the frame is
//! drawn; the colour is rich text in the label's text, so it goes when the game writes the label.
//!
//! 2.12.3：游戏 0.7.3 的百科英雄页把名字栏（stat.header / name / category）在布局里藏起来了，改成在上面的网格里
//! 高亮选中的英雄。以前名字标签不可见就跳过整块面板，所以百科页一直是游戏的原始数值（玩家截图：双弹瓦斯补丁后
//! 攻击力 92，百科「查看当前数值」还是 80）。游戏代码里还在写这个标签（EXE 里 champion_info_ui.rs 有
//! data.champion_info.stat.name），所以这一页改看数值那一块在不在屏幕上，英雄名照旧从名字标签读；
//! 读不出英雄就把这一页的界面树写进 ui_champion_info.txt，方便下次对着改。

use std::collections::{HashMap, HashSet};

use super::names::NameBook;
use super::Ui;
use crate::balance::View;

/// The ban/pick legend's colours: 版本加强 (up) and 削弱 (down).
pub const UP: u32 = 0x0b7f_58ff;
pub const DOWN: u32 = 0xb854_0bff;

/// A panel: the label naming its champion, the labels to change (path, health - else damage), and the button
/// showing the game's initial values.
struct Panel {
    name: &'static str,
    /// 面板在屏幕上时这个节点可见。一般就是名字标签；百科页的名字标签被游戏藏了，看数值那一块。
    shown: &'static str,
    values: Vec<(String, bool)>,
    initial: Option<&'static str>,
    /// 面板在屏幕上却读不出英雄时，把哪棵界面树写进哪个文件（一次游戏写一次）。
    dump: Option<(&'static str, &'static str)>,
}

/// 读不出英雄连续这么多帧才写界面树（刚打开页面的那一两帧游戏可能还没写名字）。
const DUMP_AFTER: u32 = 30;

const LEVELS: [&str; 3] = ["lv1", "lvup", "lvmax"];
const STATS: [(&str, bool); 3] = [("attack", false), ("magic", false), ("hp", true)];

/// The ban/pick screen's panel.
pub const BANPICK: &str = "main.champion_info.stat";
/// The encyclopedia's champion page.
pub const ENCYCLOPEDIA: &str = "main.top.right.champion_info.data.champion_info.stat";
const ENCYCLOPEDIA_NAME: &str = "main.top.right.champion_info.data.champion_info.stat.name";
const ENCYCLOPEDIA_STATS: &str = "main.top.right.champion_info.data.champion_info.stat.stats";
const ENCYCLOPEDIA_INITIAL: &str = "main.top.right.champion_info.data.action_buttons.value_mode.initial_btn";
/// 百科英雄页的界面树（网格、数值、技能都在 data 下面）。
const ENCYCLOPEDIA_PAGE: &str = "main.top.right.champion_info.data";
const ENCYCLOPEDIA_DUMP: &str = "ui_champion_info.txt";
/// The ban/pick screen's champion tooltip.
pub const TOOLTIP: &str = "main.champion_info_tooltip";

fn panels() -> Vec<Panel> {
    let grid = |root: &str| -> Vec<(String, bool)> {
        STATS
            .iter()
            .flat_map(|(stat, health)| LEVELS.iter().map(move |lv| (format!("{root}.stats.{stat}.values.{lv}"), *health)))
            .collect()
    };
    vec![
        Panel { name: "main.champion_info.stat.name", shown: "main.champion_info.stat.name", values: grid(BANPICK), initial: None, dump: None },
        Panel {
            name: ENCYCLOPEDIA_NAME,
            shown: ENCYCLOPEDIA_STATS,
            values: grid(ENCYCLOPEDIA),
            initial: Some(ENCYCLOPEDIA_INITIAL),
            dump: Some((ENCYCLOPEDIA_PAGE, ENCYCLOPEDIA_DUMP)),
        },
        Panel {
            name: "main.champion_info_tooltip.header.name",
            shown: "main.champion_info_tooltip.header.name",
            values: [("attack", false), ("magic", false), ("hp", true)]
                .iter()
                .map(|(stat, health)| (format!("{TOOLTIP}.stat_{stat}.value"), *health))
                .collect(),
            initial: None,
            dump: None,
        },
    ]
}

/// The champion a name label shows: its text is the game's reference to the champion's name (or the name itself).
pub(crate) fn champion_of(text: &str, names: &NameBook) -> Option<String> {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix("#asset/base/text/champion?description.") {
        if let Some(id) = rest.strip_suffix(".name") {
            return Some(id.to_string());
        }
    }
    names.lookup(text).map(str::to_string)
}

/// Whether a selectable's state says it is selected.
fn selected(state: Option<String>) -> bool {
    state.is_some_and(|s| s.replace(' ', "").contains("\"selected\":true"))
}

/// Every number of a label's text times `factor`, written as the game writes it (as many decimals as it had, one
/// more when the product needs it); rich-text tags (`<...>`) are kept as they are. None: no number in it.
pub fn scale(text: &str, factor: f64) -> Option<String> {
    let mut out = String::with_capacity(text.len() + 4);
    let mut found = false;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '<' {
            // a tag: copied up to its end
            while i < chars.len() {
                out.push(chars[i]);
                i += 1;
                if chars[i - 1] == '>' {
                    break;
                }
            }
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || (chars[i] == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit))) {
                i += 1;
            }
            let token: String = chars[start..i].iter().collect();
            let decimals = token.split_once('.').map_or(0, |(_, d)| d.len());
            match token.parse::<f64>() {
                Ok(v) => {
                    found = true;
                    let x = v * factor;
                    let places = if decimals == 0 && (x - x.round()).abs() >= 0.05 { 1 } else { decimals };
                    out.push_str(&format!("{x:.places$}"));
                }
                Err(_) => out.push_str(&token),
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    found.then_some(out)
}

/// A label's text without its rich-text tags.
pub fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// The numbers of the stat panels with the balance on.
#[derive(Default)]
pub struct BalanceView {
    panels: Vec<Panel>,
    /// Each label written: (the game's text, ours, the champion).
    labels: HashMap<String, (String, String, String)>,
    /// 面板在屏幕上却读不出英雄的连续帧数（按名字标签）；已经写过界面树的面板。
    misses: HashMap<&'static str, u32>,
    dumped: HashSet<&'static str>,
}

impl BalanceView {
    /// One frame: `view` is the newest round (None: no round, nothing to show).
    pub fn tick(&mut self, ui: &mut impl Ui, names: &NameBook, view: Option<&View>) {
        let Some(view) = view else { return };
        if self.panels.is_empty() {
            self.panels = panels();
        }
        for panel in &self.panels {
            if !ui.exists(panel.shown) || ui.visible(panel.shown) == Some(false) {
                // 不在屏幕上：读不出英雄的帧数重新数
                self.misses.remove(panel.name);
                continue;
            }
            let name = ui.text(panel.name);
            let Some(champion) = name.as_deref().and_then(|t| champion_of(t, names)) else {
                if let Some((root, file)) = panel.dump {
                    let n = self.misses.entry(panel.name).or_insert(0);
                    *n += 1;
                    if *n >= DUMP_AFTER && self.dumped.insert(panel.name) {
                        crate::diag::log(&format!(
                            "[ui] stat panel {}: on screen for {DUMP_AFTER} frames, but its name label names no champion ({name:?})",
                            panel.name
                        ));
                        super::explore::write_tree(ui, root, file, "stat panel without a champion");
                    }
                }
                continue;
            };
            self.misses.remove(panel.name);
            let initial = panel.initial.is_some_and(|b| selected(ui.state(b)));
            let power = view.power.get(&champion).copied().unwrap_or(0);
            let step = view.steps.get(&champion).copied().unwrap_or(0);
            let (damage, health) = crate::balance::factors(power);
            for (path, is_health) in &panel.values {
                let Some(text) = ui.text(path) else { continue };
                if self.labels.get(path).is_some_and(|(_, ours, c)| *ours == text && *c == champion) {
                    continue;
                }
                // the game's own number: another champion, or written again
                if initial || power == 0 {
                    self.labels.remove(path);
                    continue;
                }
                let factor = if *is_health { health } else { damage };
                let ours = if step != 0 {
                    // this round changed it: the number alone, in the patch colour
                    scale(&plain(&text), factor).map(|n| format!("<{}>{n}<>", super::color(if step > 0 { UP } else { DOWN })))
                } else {
                    scale(&text, factor)
                };
                let Some(ours) = ours else { continue };
                if ui.set_text(path, &ours) {
                    // once a panel and save: that the numbers reach this screen (a game update may move its labels)
                    crate::diag::log_once(
                        &format!("balance-view-{}", panel.name),
                        &format!("[ui] stat panel {}: {champion} at {power:+}% of power, {path} {text:?} -> {ours:?}", panel.name),
                    );
                    self.labels.insert(path.clone(), (text, ours, champion.clone()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::FakeUi;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn view(power: &[(&str, i32)], steps: &[(&str, i32)]) -> View {
        View {
            id: 1,
            power: Arc::new(power.iter().map(|(c, p)| (c.to_string(), *p)).collect::<BTreeMap<_, _>>()),
            steps: steps.iter().map(|(c, p)| (c.to_string(), *p)).collect(),
        }
    }

    fn panel(ui: &mut FakeUi, root: &str, name_label: &str, champion: &str, numbers: [&str; 3]) {
        ui.add(name_label, "label").text = Some(format!("#asset/base/text/champion?description.{champion}.name"));
        ui.add(&format!("{root}.stats"), "empty");
        for (stat, _) in STATS {
            for (lv, n) in LEVELS.iter().zip(numbers) {
                ui.add(&format!("{root}.stats.{stat}.values.{lv}"), "label").text = Some(n.to_string());
            }
        }
    }

    #[test]
    fn the_panel_shows_the_numbers_a_champion_plays_with() {
        let mut ui = FakeUi::default();
        panel(&mut ui, BANPICK, "main.champion_info.stat.name", "tft_maddie", ["94", "6", "160"]);
        let names = NameBook::default();
        let mut bv = BalanceView::default();
        // -10: attack and magic power x0.95, health x0.95; changed this round: the patch's orange
        bv.tick(&mut ui, &names, Some(&view(&[("tft_maddie", -10)], &[("tft_maddie", -10)])));
        let text = |ui: &FakeUi, p: &str| ui.nodes[p].text.clone().unwrap();
        assert_eq!(text(&ui, "main.champion_info.stat.stats.attack.values.lv1"), "<#b8540bff>89.3<>");
        assert_eq!(text(&ui, "main.champion_info.stat.stats.attack.values.lvup"), "<#b8540bff>5.7<>");
        assert_eq!(text(&ui, "main.champion_info.stat.stats.hp.values.lvmax"), "<#b8540bff>152<>");
        // written once: the next frame sees its own text
        ui.nodes.get_mut("main.champion_info.stat.stats.attack.values.lv1").unwrap().text = Some("<#b8540bff>89.3<>".into());
        bv.tick(&mut ui, &names, Some(&view(&[("tft_maddie", -10)], &[("tft_maddie", -10)])));
        assert_eq!(text(&ui, "main.champion_info.stat.stats.attack.values.lv1"), "<#b8540bff>89.3<>");
        // another champion: the game writes its numbers; one changed in an older round only: no colour
        panel(&mut ui, BANPICK, "main.champion_info.stat.name", "league_kaisa", ["50", "4", "94"]);
        bv.tick(&mut ui, &names, Some(&view(&[("tft_maddie", -10), ("league_kaisa", 6)], &[("tft_maddie", -10)])));
        assert_eq!(text(&ui, "main.champion_info.stat.stats.attack.values.lv1"), "51.5");
        assert_eq!(text(&ui, "main.champion_info.stat.stats.hp.values.lv1"), "51.5");
        // a champion the balance never touched: the game's numbers stay
        panel(&mut ui, BANPICK, "main.champion_info.stat.name", "league_ahri", ["50", "4", "94"]);
        bv.tick(&mut ui, &names, Some(&view(&[("tft_maddie", -10)], &[])));
        assert_eq!(text(&ui, "main.champion_info.stat.stats.attack.values.lv1"), "50");
    }

    #[test]
    fn the_encyclopedias_initial_values_stay_the_games() {
        let mut ui = FakeUi::default();
        panel(&mut ui, ENCYCLOPEDIA, "main.top.right.champion_info.data.champion_info.stat.name", "tft_maddie", ["94", "6", "160"]);
        ui.add(ENCYCLOPEDIA_INITIAL, "color_selectable").state = Some(r#"{"selected": true}"#.into());
        let names = NameBook::default();
        let mut bv = BalanceView::default();
        let v = view(&[("tft_maddie", -10)], &[]);
        bv.tick(&mut ui, &names, Some(&v));
        assert_eq!(ui.nodes[&format!("{ENCYCLOPEDIA}.stats.attack.values.lv1")].text.as_deref(), Some("94"));
        ui.nodes.get_mut(ENCYCLOPEDIA_INITIAL).unwrap().state = Some(r#"{"selected":false}"#.into());
        bv.tick(&mut ui, &names, Some(&v));
        assert_eq!(ui.nodes[&format!("{ENCYCLOPEDIA}.stats.attack.values.lv1")].text.as_deref(), Some("89.3"));
        // no round: nothing
        let mut ui2 = FakeUi::default();
        panel(&mut ui2, BANPICK, "main.champion_info.stat.name", "tft_maddie", ["94", "6", "160"]);
        bv.tick(&mut ui2, &names, None);
        assert_eq!(ui2.nodes[&format!("{BANPICK}.stats.attack.values.lv1")].text.as_deref(), Some("94"));
    }

    #[test]
    fn the_encyclopedia_page_reads_its_hidden_name_label() {
        // 游戏 0.7.3：百科英雄页的名字栏藏起来了，数值那一块照常显示，游戏照样往名字标签里写英雄
        let mut ui = FakeUi::default();
        panel(&mut ui, ENCYCLOPEDIA, ENCYCLOPEDIA_NAME, "league_tryndamere", ["80", "6", "146"]);
        ui.nodes.get_mut(ENCYCLOPEDIA_NAME).unwrap().visible = false;
        ui.add(ENCYCLOPEDIA_INITIAL, "color_selectable").state = Some(r#"{"selected":false}"#.into());
        let names = NameBook::default();
        let mut bv = BalanceView::default();
        // 玩家截图里的数：累计 +30%（攻击和法强 +15%，生命 +15%），这一轮 +6 所以是绿色
        let v = view(&[("league_tryndamere", 30)], &[("league_tryndamere", 6)]);
        bv.tick(&mut ui, &names, Some(&v));
        let text = |ui: &FakeUi, p: &str| ui.nodes[p].text.clone().unwrap();
        assert_eq!(text(&ui, &format!("{ENCYCLOPEDIA}.stats.attack.values.lv1")), "<#0b7f58ff>92<>");
        assert_eq!(text(&ui, &format!("{ENCYCLOPEDIA}.stats.attack.values.lvup")), "<#0b7f58ff>6.9<>");
        assert_eq!(text(&ui, &format!("{ENCYCLOPEDIA}.stats.attack.values.lvmax")), "<#0b7f58ff>167.9<>");
        assert_eq!(text(&ui, &format!("{ENCYCLOPEDIA}.stats.hp.values.lv1")), "<#0b7f58ff>92<>");
        assert!(bv.dumped.is_empty());
        // 数值那一块不在屏幕上：游戏写回去的数不动
        ui.nodes.get_mut(ENCYCLOPEDIA_STATS).unwrap().visible = false;
        ui.nodes.get_mut(&format!("{ENCYCLOPEDIA}.stats.attack.values.lv1")).unwrap().text = Some("80".into());
        bv.tick(&mut ui, &names, Some(&v));
        assert_eq!(text(&ui, &format!("{ENCYCLOPEDIA}.stats.attack.values.lv1")), "80");
    }

    #[test]
    fn a_page_whose_name_label_is_empty_is_written_down_once() {
        // 万一游戏不再往藏起来的名字标签里写：数值不动，连续 DUMP_AFTER 帧读不出英雄才写一次界面树
        let mut ui = FakeUi::default();
        panel(&mut ui, ENCYCLOPEDIA, ENCYCLOPEDIA_NAME, "league_tryndamere", ["80", "6", "146"]);
        ui.nodes.get_mut(ENCYCLOPEDIA_NAME).unwrap().text = Some(String::new());
        let names = NameBook::default();
        let mut bv = BalanceView::default();
        let v = view(&[("league_tryndamere", 30)], &[]);
        for _ in 1..DUMP_AFTER {
            bv.tick(&mut ui, &names, Some(&v));
        }
        assert!(bv.dumped.is_empty());
        // 中间离开这一页一帧：重新数
        ui.nodes.get_mut(ENCYCLOPEDIA_STATS).unwrap().visible = false;
        bv.tick(&mut ui, &names, Some(&v));
        ui.nodes.get_mut(ENCYCLOPEDIA_STATS).unwrap().visible = true;
        for _ in 1..DUMP_AFTER {
            bv.tick(&mut ui, &names, Some(&v));
        }
        assert!(bv.dumped.is_empty());
        bv.tick(&mut ui, &names, Some(&v));
        assert!(bv.dumped.contains(ENCYCLOPEDIA_NAME));
        assert_eq!(ui.nodes[&format!("{ENCYCLOPEDIA}.stats.attack.values.lv1")].text.as_deref(), Some("80"));
        // 选人界面的面板没有这一套：名字读不出就跳过，不记
        let mut ui2 = FakeUi::default();
        panel(&mut ui2, BANPICK, "main.champion_info.stat.name", "league_tryndamere", ["80", "6", "146"]);
        ui2.nodes.get_mut("main.champion_info.stat.name").unwrap().text = Some(String::new());
        let mut bv2 = BalanceView::default();
        for _ in 0..DUMP_AFTER * 2 {
            bv2.tick(&mut ui2, &names, Some(&v));
        }
        assert!(bv2.dumped.is_empty() && bv2.misses.is_empty());
    }

    #[test]
    fn numbers_scale_as_the_game_writes_them() {
        assert_eq!(scale("94", 0.95).as_deref(), Some("89.3"));
        assert_eq!(scale("160", 0.95).as_deref(), Some("152"));
        assert_eq!(scale("1.25", 1.05).as_deref(), Some("1.31"));
        assert_eq!(scale("94 (+6)", 0.95).as_deref(), Some("89.3 (+5.7)"));
        // rich-text tags are kept, their colours not read as numbers
        assert_eq!(scale("<#0b7f58ff>94<>", 0.95).as_deref(), Some("<#0b7f58ff>89.3<>"));
        assert_eq!(plain("<#0b7f58ff>94<>"), "94");
        assert_eq!(scale("—", 0.95), None);
        assert_eq!(champion_of("#asset/base/text/champion?description.league_ahri.name", &NameBook::default()).as_deref(), Some("league_ahri"));
        assert!(selected(Some(r#"{"selected": true}"#.into())) && !selected(None));
    }
}
