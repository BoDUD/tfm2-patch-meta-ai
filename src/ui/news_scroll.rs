//! 收件箱里电竞大脑的长新闻能往下滚（2.14.4；用户：「装备平衡补丁有点bug 不能往下滚动」）。
//!
//! 游戏的普通新闻（Simple）正文是 `news_component/simple_content(_d)` 里一个 728 px 高的文字框（`contents.text`：20 号字，
//! 行高 36），外面没有滚动框，大约 20 行以后就被截掉。2.14.2 照官方排版的装备新闻一件装备五六行，八件就有五十多行；补丁
//! 预告、赛前前瞻也可能超过。这里在右边打开的是电竞大脑发的新闻（作者那一行是 [`crate::balance::AUTHOR`]）、字又放不下时，
//! 把原来的文字框藏起来，在同一个 `contents` 里放一个游戏自己的 scroll_view（和补丁内容页的滚动框一样），里面一样的字、
//! 一样的字号和行高，高度按行数算好（[`height_of`]）。换一条新闻或者离开就收起来、把原来的框显示回去。
//!
//! 找正文：右边每一页（`main.top.right` 的孩子）的 `data.right_panel` 下面，带 `top.author` 和 `contents.text` 的那一块。
//! 这一页的路径没在游戏里抓过；第一次找到时 diag.log 记一行路径，找到过电竞大脑的新闻却放不进去也记一行。

use super::Ui;

/// 右边的页面都在这下面（百科、收件箱……）
pub const RIGHT: &str = "main.top.right";
/// 我们放进正文的滚动框
pub const OURS: &str = "pma_scroll";
/// 游戏正文框的尺寸、字号、行高（`simple_content_d`）
const VIEW_H: i32 = 728;
const WIDTH: i32 = 1055;
const SIZE: i32 = 20;
const LINE_H: i32 = 36;
/// 滚动条占的宽
const BAR_ROOM: i32 = 15;
/// 每隔这么多帧看一次
const LOOK_EVERY: u64 = 15;
/// 按路径找不到时，每隔这么多次在右边页面里广搜一遍（深度、节点数都有上限）
const SEARCH_EVERY: u64 = 8;
const SEARCH_DEPTH: usize = 7;
const SEARCH_NODES: usize = 800;

/// 收件箱的滚动框
#[derive(Default)]
pub struct NewsScroll {
    frame: u64,
    /// 正文那一块（`…contents`）和放进滚动框的字
    shown: Option<(String, String)>,
    /// 广搜找到过的正文块（下次先看它）
    seen_at: Option<String>,
}

impl NewsScroll {
    pub fn tick(&mut self, ui: &mut impl Ui) {
        self.frame += 1;
        if !self.frame.is_multiple_of(LOOK_EVERY) {
            return;
        }
        let mut found = find(ui).or_else(|| self.seen_at.as_deref().and_then(|b| block_text(ui, b)));
        if found.is_none() && (self.frame / LOOK_EVERY).is_multiple_of(SEARCH_EVERY) {
            if let Some(block) = search(ui) {
                crate::diag::log_once(&format!("news-scroll-found-{block}"), &format!("[ui] this mod's news is shown at {block}"));
                found = block_text(ui, &block);
                self.seen_at = Some(block);
            }
        }
        if let (Some((was, old)), Some((now, text))) = (&self.shown, &found) {
            if was == now && old == text && ui.exists(&format!("{was}.{OURS}")) {
                // 游戏可能又把原来的字显示出来
                if ui.visible(&format!("{was}.text")) == Some(true) {
                    ui.set_visible(&format!("{was}.text"), false);
                }
                return;
            }
        }
        // 换了一条新闻、离开了：上次的收起来
        if let Some((was, _)) = self.shown.take() {
            if ui.exists(&format!("{was}.{OURS}")) {
                ui.remove(&format!("{was}.{OURS}"));
            }
            if ui.exists(&format!("{was}.text")) {
                ui.set_visible(&format!("{was}.text"), true);
            }
        }
        let Some((contents, text)) = found else { return };
        let height = height_of(&text);
        if height <= VIEW_H {
            return;
        }
        if ui.spawn(&contents, &source(height)) && ui.set_text(&format!("{contents}.{OURS}.contents.text"), &text) {
            ui.set_visible(&format!("{contents}.text"), false);
            crate::diag::log_once(
                &format!("news-scroll-{contents}"),
                &format!("[ui] a long news of this mod scrolls ({height} px in {VIEW_H}): {contents}"),
            );
            self.shown = Some((contents, text));
        } else {
            crate::diag::log_once("news-scroll-failed", &format!("[ui] a long news of this mod could not be made to scroll: {contents}"));
        }
    }
}

/// 右边正打开的电竞大脑新闻：(正文那一块 `…contents` 的路径, 正文)
pub fn find(ui: &impl Ui) -> Option<(String, String)> {
    for page in ui.children(RIGHT) {
        let panel = format!("{RIGHT}.{page}.data.right_panel");
        if !ui.exists(&panel) {
            continue;
        }
        for block in ui.children(&panel) {
            let ours = ui.text(&format!("{panel}.{block}.top.author")).is_some_and(|a| crate::balance::is_ours(a.trim()));
            if !ours {
                continue;
            }
            let contents = format!("{panel}.{block}.contents");
            if let Some(text) = ui.text(&format!("{contents}.text")) {
                return Some((contents, text));
            }
        }
    }
    None
}

/// 一块新闻（有 `top.author`、`contents.text` 的那一块）是电竞大脑的就给 (正文那一块, 正文)
fn block_text(ui: &impl Ui, block: &str) -> Option<(String, String)> {
    let author = ui.text(&format!("{block}.top.author"))?;
    if !crate::balance::is_ours(author.trim()) {
        return None;
    }
    let contents = format!("{block}.contents");
    Some((contents.clone(), ui.text(&format!("{contents}.text"))?))
}

/// 在右边页面里一层层找作者是电竞大脑的那一块（`…top.author`），深度和节点数都有上限
fn search(ui: &impl Ui) -> Option<String> {
    let mut level = vec![RIGHT.to_string()];
    let mut seen = 0usize;
    for _ in 0..SEARCH_DEPTH {
        let mut next = Vec::new();
        for node in &level {
            for child in ui.children(node) {
                let path = format!("{node}.{child}");
                seen += 1;
                if seen > SEARCH_NODES {
                    return None;
                }
                if child == "author" && node.ends_with(".top") {
                    let block = node.trim_end_matches(".top").to_string();
                    if block_text(ui, &block).is_some() {
                        return Some(block);
                    }
                }
                next.push(path);
            }
        }
        if next.is_empty() {
            break;
        }
        level = next;
    }
    None
}

/// 正文要多高：每一行按 [`super::patch_items::text_lines`] 算折几行，乘行高，再多留一行
pub fn height_of(text: &str) -> i32 {
    let lines: i32 = text.split('\n').map(|line| super::patch_items::text_lines(line, WIDTH - BAR_ROOM, SIZE)).sum();
    (lines + 1) * LINE_H
}

/// 滚动框：游戏补丁内容页的那种（`news_component/patch_content` 的 `#scroll`），里面的文字框和原来的一样
fn source(height: i32) -> String {
    let w = WIDTH - BAR_ROOM;
    format!(
        "{OURS}:scroll_view {{ focusable: true; x: 0px; y: 0px; width: {WIDTH}px; height: {VIEW_H}px; speed: 100; bar_width: 4; \
         bar: {{ source: \"asset/base/sprite/white\"; color: #37d5b3ff; hover: {{ color: #ecfbf8ff; }} }} \
         back: {{ source: \"asset/base/sprite/white\"; color: #4a4c56ff; }} \
         #contents:empty {{ width: {w}px; height: {height}px; \
         #text:label {{ @\"asset/base/style/main#label\"; width: {w}px; height: {height}px; size: {SIZE}; line_height: {LINE_H}; text: \"\"; }} }} }}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::FakeUi;

    /// 收件箱打开了一条新闻：作者、正文
    fn inbox(ui: &mut FakeUi, author: &str, text: &str) -> String {
        for p in [RIGHT, "main.top.right.news", "main.top.right.news.data"] {
            ui.add(p, "empty");
        }
        let block = "main.top.right.news.data.right_panel.simple_container";
        for p in ["main.top.right.news.data.right_panel", block, &format!("{block}.top"), &format!("{block}.contents")] {
            ui.add(p, "empty");
        }
        ui.add(&format!("{block}.top.author"), "label").text = Some(author.to_string());
        ui.add(&format!("{block}.contents.text"), "label").text = Some(text.to_string());
        format!("{block}.contents")
    }

    fn ticks(view: &mut NewsScroll, ui: &mut FakeUi) {
        for _ in 0..LOOK_EVERY {
            view.tick(ui);
        }
    }

    #[test]
    fn a_long_news_of_ours_scrolls() {
        let long: String = (0..60).map(|k| format!("第 {k} 行")).collect::<Vec<_>>().join("\n");
        let mut ui = FakeUi::default();
        let contents = inbox(&mut ui, crate::balance::AUTHOR, &long);
        let mut view = NewsScroll::default();
        ticks(&mut view, &mut ui);
        let ours = format!("{contents}.{OURS}");
        assert!(ui.exists(&format!("{ours}.contents.text")));
        assert_eq!(ui.text(&format!("{ours}.contents.text")).as_deref(), Some(long.as_str()));
        assert_eq!(ui.visible(&format!("{contents}.text")), Some(false), "the game's own box is hidden");
        assert!(ui.spawned.last().unwrap().1.contains(&format!("height: {}px", 61 * LINE_H)));
        // 下一帧什么都不再放
        let spawned = ui.spawned.len();
        ticks(&mut view, &mut ui);
        assert_eq!(ui.spawned.len(), spawned);
        // 换成一条短新闻：收起来，原来的框显示回去
        ui.nodes.get_mut(&format!("{contents}.text")).unwrap().text = Some("短短一条。".into());
        ticks(&mut view, &mut ui);
        assert!(!ui.exists(&ours));
        assert_eq!(ui.visible(&format!("{contents}.text")), Some(true));
    }

    #[test]
    fn a_news_page_elsewhere_is_found_by_its_author() {
        let long: String = (0..60).map(|k| format!("第 {k} 行")).collect::<Vec<_>>().join("\n");
        let mut ui = FakeUi::default();
        let block = "main.top.right.inbox.view.detail.simple_container";
        for p in [RIGHT, "main.top.right.inbox", "main.top.right.inbox.view", "main.top.right.inbox.view.detail", block, &format!("{block}.top"), &format!("{block}.contents")] {
            ui.add(p, "empty");
        }
        ui.add(&format!("{block}.top.author"), "label").text = Some(crate::balance::AUTHOR.to_string());
        ui.add(&format!("{block}.contents.text"), "label").text = Some(long.clone());
        let mut view = NewsScroll::default();
        for _ in 0..(LOOK_EVERY * SEARCH_EVERY) {
            view.tick(&mut ui);
        }
        assert!(ui.exists(&format!("{block}.contents.{OURS}.contents.text")));
        assert_eq!(view.seen_at.as_deref(), Some(block));
    }

    #[test]
    fn other_news_and_short_news_are_left_alone() {
        let long: String = (0..60).map(|k| format!("第 {k} 行")).collect::<Vec<_>>().join("\n");
        let mut ui = FakeUi::default();
        let contents = inbox(&mut ui, "联盟官方", &long);
        let mut view = NewsScroll::default();
        ticks(&mut view, &mut ui);
        assert!(!ui.exists(&format!("{contents}.{OURS}")), "the game's own news");
        let mut ui = FakeUi::default();
        let contents = inbox(&mut ui, crate::balance::AUTHOR, "一行\n两行");
        ticks(&mut view, &mut ui);
        assert!(!ui.exists(&format!("{contents}.{OURS}")), "it fits");
        assert_eq!(height_of("一行\n两行"), 3 * LINE_H);
    }
}
