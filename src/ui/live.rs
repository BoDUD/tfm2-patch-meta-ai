//! 比赛画面上的实时胜率（2.15.0，[`crate::winprob`]）：比赛时间下面一块面板。
//!
//! - 上面一行：两边的胜率（蓝左红右），中间是标题；十秒里起落十个点以上时，标题换成原因（「蓝方拿下大龙 +18%」）。
//! - 一条蓝红比例条。
//! - 下面一块胜率曲线：从开局到现在，中线以上是蓝方占优、以下是红方，越高越大。
//!
//! 只画，不接点击（`ignore_event`）。离开比赛画面就收起来，下一局从头画。设置 `live_win_chance`。

use std::time::Duration;

use super::{color, Ui};
use crate::{diag, winprob};

/// 比赛画面的根和顶栏（有顶栏才是比赛画面）
pub const ROOT: &str = "ingame";
pub const HEADER: &str = "ingame.header";
pub const NODE: &str = "pma_live";
const PANEL: &str = "ingame.pma_live";
/// 面板宽，曲线一列几像素、多少列，曲线多高
const W: u32 = 440;
const INNER: u32 = W - 16;
const COL_W: u32 = 4;
const COLS: usize = (INNER / COL_W) as usize;
const GRAPH_H: u32 = 34;
/// 游戏顶栏的蓝、红
const BLUE: u32 = 0x5b73_ffff;
const RED: u32 = 0xef64_71ff;
/// 这么久没有新的点就不画了（暂停时模拟不走，所以要长；离开比赛画面本来就收起来）
const STALE: Duration = Duration::from_secs(600);
/// 起落的说明留多久（游戏里的分钟）
const NOTE_MINUTES: f32 = 12.0 / 60.0;
const TITLE: &str = "实时胜率";

/// 面板的 `.ui` 源码
fn source() -> String {
    let mut cols = String::new();
    for i in 0..COLS {
        cols.push_str(&format!(
            "#c{i}:color {{ x: {}px; y: {}px; width: {}px; height: 1px; color: {}; visible: false; ignore_event: true; }} ",
            i as u32 * COL_W,
            GRAPH_H / 2,
            COL_W - 1,
            color(BLUE)
        ));
    }
    let label = |id: &str, style: &str, place: &str, size: u32, align: &str, rgba: u32, text: &str| {
        format!(
            "#{id}:label {{ @\"asset/base/style/main#{style}\"; {place} width: {}px; height: 20px; size: {size}; align_x: {align}; \
             align_y: Center; color: {}; text: \"{text}\"; ignore_event: true; }} ",
            if id == "title" { 280 } else { 70 },
            color(rgba)
        )
    };
    format!(
        "{NODE}:color {{ anchor_x: 0.5; pivot_x: 0.5; y: 72px; width: {W}px; height: 76px; color: #161721d9; ignore_event: true; \
         rounding: Individual {{ top_left: 0; top_right: 0; bottom_left: 10; bottom_right: 10; }} \
         {}{}{}\
         #bar:color {{ x: 8px; y: 26px; width: {INNER}px; height: 6px; color: {}; ignore_event: true; \
           #fill:color {{ width: {}px; height: 6px; color: {}; ignore_event: true; }} }} \
         #graph:empty {{ x: 8px; y: 36px; width: {INNER}px; height: {GRAPH_H}px; ignore_event: true; \
           #mid:color {{ y: {}px; width: {INNER}px; height: 1px; color: #4a4c56ff; ignore_event: true; }} {cols}}} }}",
        label("blue", "bold_label", "x: 10px; y: 2px;", 15, "Left", BLUE, "50%"),
        label("title", "label", "anchor_x: 0.5; pivot_x: 0.5; y: 2px;", 13, "Center", 0xa3a9_b6ff, TITLE),
        label("red", "bold_label", "anchor_x: 1; pivot_x: 1; x: -10px; y: 2px;", 15, "Right", RED, "50%"),
        color(RED),
        INNER / 2,
        color(BLUE),
        GRAPH_H / 2
    )
}

/// 把一路的点（分钟，蓝方胜率）按时间分成至多 `n` 段，每段取平均：开局点少时一个点一列，后面越压越密
pub fn bins(samples: &[(f32, f32)], n: usize) -> Vec<f32> {
    if samples.is_empty() || n == 0 {
        return Vec::new();
    }
    let cols = samples.len().min(n);
    (0..cols)
        .map(|i| {
            let (a, b) = (i * samples.len() / cols, ((i + 1) * samples.len() / cols).max(i * samples.len() / cols + 1));
            let part = &samples[a..b.min(samples.len())];
            part.iter().map(|(_, p)| p).sum::<f32>() / part.len() as f32
        })
        .collect()
}

/// 一列曲线的样子：x、y、高、颜色
fn column(i: usize, chance: f32) -> String {
    let half = (GRAPH_H / 2) as f32;
    let h = ((chance - 0.5).abs() * 2.0 * half).round().clamp(1.0, half) as u32;
    let (y, rgba) = if chance >= 0.5 { (GRAPH_H / 2 - h, BLUE) } else { (GRAPH_H / 2, RED) };
    format!("x: {}px; y: {y}px; height: {h}px; color: {};", i as u32 * COL_W, color(rgba))
}

#[derive(Default)]
pub struct LiveView {
    /// 画到的版本
    version: u64,
    /// 每一列上次写的样子（没变不重写），和哪几列亮着
    cols: Vec<String>,
    lit: usize,
    shown: bool,
    failed: bool,
}

impl LiveView {
    /// 每帧叫：比赛画面上有新的点就画，离开比赛画面就收起来（`on`：设置 `live_win_chance`）
    pub fn tick(&mut self, ui: &mut impl Ui, frame: u64, on: bool) {
        if !frame.is_multiple_of(10) {
            return;
        }
        let live = if on { winprob::live().filter(|l| l.at.elapsed() < STALE) } else { None };
        let Some(live) = live.filter(|_| ui.exists(HEADER)) else {
            if self.shown {
                ui.set_visible(PANEL, false);
                self.shown = false;
            }
            return;
        };
        if !ui.exists(PANEL) {
            if self.failed {
                return;
            }
            if !ui.spawn(ROOT, &source()) {
                self.failed = true;
                diag::log_once("live-spawn", "[ui] could not add the live win chance to the match screen");
                return;
            }
            self.cols.clear();
            self.lit = 0;
            self.version = 0;
            for i in 0..COLS {
                ui.set_visible(&format!("{PANEL}.graph.c{i}"), false);
            }
            diag::log_once("live-shown", "[ui] the live win chance is shown under the match clock");
        }
        if !self.shown {
            ui.set_visible(PANEL, true);
            self.shown = true;
        }
        if live.version == self.version {
            return;
        }
        self.version = live.version;
        self.draw(ui, &live);
    }

    fn draw(&mut self, ui: &mut impl Ui, live: &winprob::Live) {
        let Some(&(now, chance)) = live.samples.last() else { return };
        let blue = (chance * 100.0).round().clamp(0.0, 100.0) as u32;
        ui.set_text(&format!("{PANEL}.blue"), &format!("{blue}%"));
        ui.set_text(&format!("{PANEL}.red"), &format!("{}%", 100 - blue));
        ui.set_properties(&format!("{PANEL}.bar.fill"), &format!("width: {}px;", (chance * INNER as f32).round() as u32));
        let title = live.note.as_ref().filter(|(t, _)| now - t <= NOTE_MINUTES).map_or(TITLE, |(_, n)| n.as_str());
        ui.set_text(&format!("{PANEL}.title"), title);
        let cols = bins(&live.samples, COLS);
        self.cols.resize(COLS, String::new());
        for (i, c) in cols.iter().enumerate() {
            let props = column(i, *c);
            if self.cols[i] != props {
                ui.set_properties(&format!("{PANEL}.graph.c{i}"), &props);
                self.cols[i] = props;
            }
        }
        for i in cols.len()..self.lit {
            ui.set_visible(&format!("{PANEL}.graph.c{i}"), false);
            self.cols[i].clear();
        }
        for i in self.lit..cols.len() {
            ui.set_visible(&format!("{PANEL}.graph.c{i}"), true);
        }
        self.lit = cols.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::FakeUi;

    #[test]
    fn the_curve_packs_the_match_into_its_columns() {
        assert!(bins(&[], 10).is_empty());
        let few: Vec<(f32, f32)> = (0..3).map(|i| (i as f32, 0.5 + i as f32 * 0.1)).collect();
        assert_eq!(bins(&few, 10).len(), 3, "a column a point while there are few");
        let many: Vec<(f32, f32)> = (0..300).map(|i| (i as f32, if i < 150 { 0.6 } else { 0.4 })).collect();
        let b = bins(&many, 100);
        assert_eq!(b.len(), 100);
        assert!((b[0] - 0.6).abs() < 1e-6 && (b[99] - 0.4).abs() < 1e-6);
        // 蓝方占优画在中线上面，红方在下面
        assert!(column(0, 0.75).contains(&format!("color: {};", color(BLUE))));
        assert!(column(0, 0.25).contains(&format!("y: {}px;", GRAPH_H / 2)));
        assert!(column(0, 0.25).contains(&format!("color: {};", color(RED))));
        assert_eq!(COLS, 106);
    }

    #[test]
    fn the_panel_shows_the_chance_and_goes_when_the_match_does() {
        let _serial = crate::tests::serial();
        crate::winprob::reset_for_tests();
        let mut ui = FakeUi::default();
        ui.add(ROOT, "empty");
        ui.add(HEADER, "color");
        let mut view = LiveView::default();
        view.tick(&mut ui, 0, true);
        assert!(!ui.exists(PANEL), "nothing to show yet");
        crate::winprob::publish_for_tests(0.0, 0.5);
        crate::winprob::publish_for_tests(0.5, 0.62);
        view.tick(&mut ui, 10, true);
        assert!(ui.exists(PANEL), "{:?}", ui.spawned);
        assert_eq!(ui.text(&format!("{PANEL}.blue")).as_deref(), Some("62%"));
        assert_eq!(ui.text(&format!("{PANEL}.red")).as_deref(), Some("38%"));
        assert_eq!(ui.text(&format!("{PANEL}.title")).as_deref(), Some("实时胜率"));
        assert!(ui.nodes[&format!("{PANEL}.bar.fill")].props.last().unwrap().contains(&format!("width: {}px;", (0.62 * INNER as f32).round() as u32)));
        assert_eq!(ui.visible(&format!("{PANEL}.graph.c1")), Some(true));
        assert_eq!(ui.visible(&format!("{PANEL}.graph.c2")), Some(false), "two points: two columns");
        // 设置关掉：收起来
        view.tick(&mut ui, 20, false);
        assert_eq!(ui.visible(PANEL), Some(false));
        crate::winprob::reset_for_tests();
    }
}
