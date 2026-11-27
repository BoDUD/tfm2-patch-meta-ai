//! 收件箱里电竞大脑的长新闻能往下滚（2.14.4；用户：「装备平衡补丁有点bug 不能往下滚动」）。
//!
//! 游戏的普通新闻（Simple）正文是 `news_component/simple_content(_d)` 里一个 728 px 高的文字框（`contents.text`：20 号字，
//! 行高 36），外面没有滚动框，大约 20 行以后就被截掉。2.14.2 照官方排版的装备新闻一件装备五六行，八件就有五十多行。右边
//! 打开的是电竞大脑发的新闻、字又放不下时，把原来的文字框藏起来，在同一个 `contents` 里放一个游戏自己的 scroll_view（和补丁
//! 内容页的滚动框一样），里面一样的字、一样的字号和行高，高度按行数算好（[`height_of`]）。换一条新闻或者离开就收起来、把
//! 原来的框显示回去。
//!
//! 2.15.2：游戏里右边打开的那条新闻，标题、作者、正文三个节点都读不出字（用户按出来的界面结构：`[]` 类型、没有文字），
//! 2.14.4 靠读作者认新闻，所以从来没认出来（用户：「这个还是不能往下滚动」）。现在：
//! - 打开的是第几条：你在左边列表里点的那一行（行的名字就是新闻在队伍新闻里的序号）。还没点过时，右边的时间对得上电竞
//!   大脑的某一条、而且它就在列表最上面，才算是它；点完以后右边的时间变了（用键盘换了一条），就不算了。
//! - 正文：客户端读新闻时把电竞大脑自己写的那几条记下来（[`crate::shared::our_news`]），按序号取。

use std::collections::HashSet;

use super::Ui;

/// 收件箱页面、右边打开的那条新闻、左边的列表（每一行的名字是新闻的序号）
pub const PAGE: &str = "main.top.right.news";
pub const OPEN: &str = "main.top.right.news.contents.simple_container";
pub const ROWS: &str = "main.top.right.news.background.list.contents";
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

/// 收件箱的滚动框
#[derive(Default)]
pub struct NewsScroll {
    frame: u64,
    /// 正在滚的那一条（序号）
    shown: Option<usize>,
    /// 点过的那一行（序号），和点完以后右边显示的时间（变了就不算了）
    clicked: Option<usize>,
    clicked_time: Option<String>,
    /// 登记过点击的行
    hooked: HashSet<String>,
}

impl NewsScroll {
    /// 每帧叫（点击只在当帧）
    pub fn tick(&mut self, ui: &mut impl Ui, clicks: &[String]) {
        for c in clicks {
            if let Some(i) = c.strip_prefix(ROWS).and_then(|r| r.strip_prefix('.')).and_then(|r| r.parse::<usize>().ok()) {
                self.clicked = Some(i);
                self.clicked_time = None;
            }
        }
        self.frame += 1;
        if !self.frame.is_multiple_of(LOOK_EVERY) {
            return;
        }
        if !ui.exists(OPEN) {
            // 离开收件箱（或者打开的不是普通新闻）：从头来
            self.restore(ui);
            self.clicked = None;
            self.clicked_time = None;
            return;
        }
        // 列表里的行：登记点击（同一行只登记一次，游戏按路径记）
        let rows = ui.children(ROWS);
        for row in rows.iter().filter(|r| r.parse::<usize>().is_ok()) {
            if self.hooked.insert(row.clone()) {
                ui.on_click(&format!("{ROWS}.{row}"));
            }
        }
        let time = ui.text(&format!("{OPEN}.top.time")).unwrap_or_default();
        match &self.clicked_time {
            None if self.clicked.is_some() => self.clicked_time = Some(time.clone()),
            Some(t) if *t != time => {
                self.clicked = None;
                self.clicked_time = None;
            }
            _ => {}
        }
        let ours = crate::shared::our_news();
        let open = self.clicked.or_else(|| {
            // 还没点过：时间对得上电竞大脑的某一条，而且它在列表最上面
            let top = rows.iter().find_map(|r| r.parse::<usize>().ok());
            let matching: Vec<usize> = ours.iter().filter(|n| !time.is_empty() && stamp(&n.date) == time).map(|n| n.index).collect();
            match matching.as_slice() {
                [i] if top == Some(*i) => Some(*i),
                _ => None,
            }
        });
        let contents = format!("{OPEN}.contents");
        let Some(news) = open.and_then(|i| ours.into_iter().find(|n| n.index == i)).filter(|n| height_of(&n.content) > VIEW_H) else {
            self.restore(ui);
            return;
        };
        if self.shown == Some(news.index) && ui.exists(&format!("{contents}.{OURS}")) {
            // 游戏可能又把原来的字显示出来
            if ui.visible(&format!("{contents}.text")) == Some(true) {
                ui.set_visible(&format!("{contents}.text"), false);
            }
            return;
        }
        self.restore(ui);
        let height = height_of(&news.content);
        if ui.spawn(&contents, &source(height)) && ui.set_text(&format!("{contents}.{OURS}.contents.text"), &news.content) {
            ui.set_visible(&format!("{contents}.text"), false);
            crate::diag::log_once(
                &format!("news-scroll-{}", news.index),
                &format!("[ui] a long news of this mod scrolls ({height} px in {VIEW_H}): news {} 「{}」", news.index, news.title),
            );
            self.shown = Some(news.index);
        } else {
            crate::diag::log_once("news-scroll-failed", &format!("[ui] a long news of this mod could not be made to scroll: {contents}"));
        }
    }

    /// 收起滚动框，把游戏原来的字放回来
    fn restore(&mut self, ui: &mut impl Ui) {
        if self.shown.take().is_none() {
            return;
        }
        let contents = format!("{OPEN}.contents");
        if ui.exists(&format!("{contents}.{OURS}")) {
            ui.remove(&format!("{contents}.{OURS}"));
        }
        if ui.exists(&format!("{contents}.text")) {
            ui.set_visible(&format!("{contents}.text"), true);
        }
    }
}

/// 新闻的日期（`2027-09-20T13:00:00`）写成右边显示的样子（`9/20 13:00`）
pub fn stamp(date: &str) -> String {
    let (day, clock) = date.split_once('T').unwrap_or((date, ""));
    let mut parts = day.split('-').skip(1).map(|x| x.parse::<u32>().ok());
    match (parts.next().flatten(), parts.next().flatten(), clock.get(..5)) {
        (Some(m), Some(d), Some(hm)) => format!("{m}/{d} {hm}"),
        _ => String::new(),
    }
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
    use crate::shared::OurNews;
    use crate::ui::tests::FakeUi;

    /// 收件箱：左边几行（序号），右边打开了一条（读不出字，只有时间）
    fn inbox(rows: &[usize], time: &str) -> FakeUi {
        let mut ui = FakeUi::default();
        for p in [PAGE, "main.top.right.news.background", "main.top.right.news.background.list", ROWS, "main.top.right.news.contents"] {
            ui.add(p, "empty");
        }
        for r in rows {
            ui.add(&format!("{ROWS}.{r}"), "color_selectable");
        }
        ui.add(&format!("{ROWS}.h_20280218"), "empty");
        for p in [OPEN, &format!("{OPEN}.top"), &format!("{OPEN}.contents")] {
            ui.add(p, "empty");
        }
        ui.add(&format!("{OPEN}.top.time"), "label").text = Some(time.to_string());
        ui.add(&format!("{OPEN}.contents.text"), "");
        ui
    }

    fn long() -> String {
        (0..60).map(|k| format!("第 {k} 行")).collect::<Vec<_>>().join("\n")
    }

    fn ticks(view: &mut NewsScroll, ui: &mut FakeUi, clicks: &[String]) {
        view.tick(ui, clicks);
        for _ in 1..LOOK_EVERY {
            view.tick(ui, &[]);
        }
    }

    fn ours(index: usize, date: &str, content: &str) -> OurNews {
        OurNews { index, title: format!("电竞大脑装备平衡 {index}"), date: date.into(), content: content.into() }
    }

    #[test]
    fn the_date_is_written_as_the_inbox_shows_it() {
        assert_eq!(stamp("2028-02-18T00:00:00"), "2/18 00:00");
        assert_eq!(stamp("2027-09-05T13:30:00"), "9/5 13:30");
        assert_eq!(stamp("junk"), "");
    }

    #[test]
    fn a_long_news_of_ours_opened_by_a_click_scrolls() {
        let _serial = crate::tests::serial();
        // 假界面按名字排孩子：10 在最上面（游戏里是列表的顺序）
        crate::shared::set_our_news(vec![ours(10, "2028-02-18T00:00:00", &long()), ours(80, "2028-02-01T00:00:00", "短短一条。")]);
        let mut ui = inbox(&[10, 57, 80], "2/18 00:00");
        let mut view = NewsScroll::default();
        ticks(&mut view, &mut ui, &[]);
        assert!(ui.handlers.iter().any(|h| h == &format!("{ROWS}.10")), "the rows' clicks are followed");
        assert!(!ui.handlers.iter().any(|h| h.contains("h_2028")), "not the date headers");
        let ours_box = format!("{OPEN}.contents.{OURS}");
        assert!(ui.exists(&ours_box), "unclicked: the top row is ours and the time is its own");
        // 点了一条别人的新闻：收起来，原来的字放回来
        ticks(&mut view, &mut ui, &[format!("{ROWS}.57")]);
        assert!(!ui.exists(&ours_box));
        assert_eq!(ui.visible(&format!("{OPEN}.contents.text")), Some(true));
        // 点回我们那条：滚动框，原来的字藏起来
        ticks(&mut view, &mut ui, &[format!("{ROWS}.10")]);
        assert_eq!(ui.text(&format!("{ours_box}.contents.text")), Some(long()));
        assert_eq!(ui.visible(&format!("{OPEN}.contents.text")), Some(false));
        assert!(ui.spawned.last().unwrap().1.contains(&format!("height: {}px", 61 * LINE_H)));
        let spawned = ui.spawned.len();
        ticks(&mut view, &mut ui, &[]);
        assert_eq!(ui.spawned.len(), spawned, "nothing more while it stays");
        // 我们的短新闻：不用滚
        ticks(&mut view, &mut ui, &[format!("{ROWS}.80")]);
        assert!(!ui.exists(&ours_box));
        crate::shared::set_our_news(Vec::new());
    }

    #[test]
    fn without_a_click_only_a_sure_match_counts() {
        let _serial = crate::tests::serial();
        // 最上面一行不是我们的：不接手
        crate::shared::set_our_news(vec![ours(58, "2028-02-18T00:00:00", &long())]);
        let mut ui = inbox(&[20, 58], "2/18 00:00");
        let mut view = NewsScroll::default();
        ticks(&mut view, &mut ui, &[]);
        assert!(!ui.exists(&format!("{OPEN}.contents.{OURS}")));
        // 我们有两条同一时间的：也不接手
        crate::shared::set_our_news(vec![ours(58, "2028-02-18T00:00:00", &long()), ours(57, "2028-02-18T00:00:00", &long())]);
        let mut ui = inbox(&[58, 57], "2/18 00:00");
        ticks(&mut view, &mut ui, &[]);
        assert!(!ui.exists(&format!("{OPEN}.contents.{OURS}")));
        // 点了一条以后用键盘换了别的（右边的时间变了）：点的那条不算了
        ticks(&mut view, &mut ui, &[format!("{ROWS}.58")]);
        assert!(ui.exists(&format!("{OPEN}.contents.{OURS}")));
        ui.nodes.get_mut(&format!("{OPEN}.top.time")).unwrap().text = Some("2/17 00:00".into());
        ticks(&mut view, &mut ui, &[]);
        assert!(!ui.exists(&format!("{OPEN}.contents.{OURS}")));
        assert_eq!(height_of("一行\n两行"), 3 * LINE_H);
        crate::shared::set_our_news(Vec::new());
    }
}
