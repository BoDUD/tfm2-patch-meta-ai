//! 收件箱里电竞大脑的长新闻能往下滚（2.14.4；用户：「装备平衡补丁有点bug 不能往下滚动」）。
//!
//! 游戏的普通新闻（Simple）正文是 `news_component/simple_content(_d)` 里一个 728 px 高的文字框（`contents.text`：20 号字，
//! 行高 36），外面没有滚动框，大约 20 行以后就被截掉。2.14.2 照官方排版的装备新闻一件装备五六行，八件就有五十多行。右边
//! 打开的是电竞大脑发的新闻、字又放不下时，把原来的文字框藏起来，在同一个 `contents` 里放一个游戏自己的 scroll_view（和补丁
//! 内容页的滚动框一样），里面一样的字、一样的字号和行高，高度按行数算好（[`height_of`]）。换一条新闻或者离开就收起来、把
//! 原来的框显示回去。
//!
//! 打开的是哪一条：右边的标题、作者、正文三个节点都读不出字，只有时间（`2/25 00:02`）读得到。
//! - 2.15.2 看你在左边列表里点的那一行；没点过时，要求时间只对得上电竞大脑的一条、而且它在最上面。可补丁日那天电竞大脑
//!   同一时刻发好几条（装备平衡、两张英雄新闻卡），一进收件箱默认打开最上面那条、不用点，所以从来没接手过（用户：「装备的
//!   滚轮还是没修好啊」；界面导出：2/25 00:00 列表里有五条，三条是电竞大脑的）。
//! - 2.15.4：左边列表每一行的日期（它上面最近的日期标题 `h_20260225`，按位置找）和时间（`00:02`）去对右边的时间，只有
//!   一行对得上就是它；几行同一时刻时才看你点的是哪一行。服务端写电竞大脑的长新闻时把它挪到一个没有别的新闻的分钟
//!   （`server::spread_long_news`，以前写下的读档时挪一次），所以它总能认出来。
//! - 正文：电竞大脑自己写的那几条（[`crate::shared::our_news`]），按序号取；它的时间和列表那一行的标题也得对得上。

use std::collections::HashSet;

use super::Ui;

/// 收件箱页面、右边打开的那条新闻、左边的列表（每一行的名字是新闻的序号，日期标题是 `h_YYYYMMDD`）
pub const PAGE: &str = "main.top.right.news";
pub const OPEN: &str = "main.top.right.news.contents.simple_container";
pub const ROWS: &str = "main.top.right.news.background.list.contents";
/// 我们放进正文的滚动框
pub const OURS: &str = "pma_scroll";
/// 游戏正文框的尺寸、字号、行高（`simple_content_d`）
pub const VIEW_H: i32 = 728;
const WIDTH: i32 = 1055;
const SIZE: i32 = 20;
const LINE_H: i32 = 36;
/// 滚动条占的宽
const BAR_ROOM: i32 = 15;
/// 每隔这么多帧看一次
const LOOK_EVERY: u64 = 15;

/// 月、日、时、分
pub type Stamp = (u32, u32, u32, u32);

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
    /// 列表的孩子，和从它们读出来的每一行（序号, 日期时间）；孩子没变、读全了就不再读
    rows: Vec<String>,
    listed: Vec<(usize, Stamp)>,
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
        let numbered = rows.iter().filter(|r| r.parse::<usize>().is_ok()).count();
        for row in rows.iter().filter(|r| r.parse::<usize>().is_ok()) {
            if self.hooked.insert(row.clone()) {
                ui.on_click(&format!("{ROWS}.{row}"));
            }
        }
        if rows != self.rows || self.listed.len() < numbered {
            self.listed = listed(ui, &rows);
            if !self.listed.is_empty() {
                crate::diag::log_once(
                    "news-scroll-list",
                    &format!("[ui] the inbox list read: {numbered} news rows, {} with a day and a time", self.listed.len()),
                );
            }
            self.rows = rows;
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
        let at = shown_stamp(&time);
        let open = at.and_then(|at| {
            let same: Vec<usize> = self.listed.iter().filter(|(_, s)| *s == at).map(|(i, _)| *i).collect();
            match same.as_slice() {
                [i] => Some(*i),
                _ => {
                    let clicked = self.clicked.filter(|c| same.contains(c));
                    if clicked.is_none() && ours.iter().any(|n| same.contains(&n.index) && height_of(&n.content) > VIEW_H) {
                        crate::diag::log_once(
                            &format!("news-scroll-unsure-{time}"),
                            &format!("[ui] {} news in the inbox list at {time}, a long one of this mod among them: which one is open is known once one is clicked", same.len()),
                        );
                    }
                    clicked
                }
            }
        });
        let contents = format!("{OPEN}.contents");
        let news = open.and_then(|i| ours.into_iter().find(|n| n.index == i)).filter(|n| {
            date_stamp(&n.date) == at && title_fits(ui.text(&format!("{ROWS}.{}.title", n.index)).as_deref(), &n.title)
        });
        let Some(news) = news.filter(|n| height_of(&n.content) > VIEW_H) else {
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
                &format!("[ui] a long news of this mod scrolls ({height} px in {VIEW_H}): news {} 「{}」 at {time}", news.index, news.title),
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

/// 列表里每一行新闻（序号）的日期和时间：日期是它上面最近的日期标题（按位置找，不靠孩子的先后）。还没排好位置（高度是 0）
/// 的行先不算，下次再读。
fn listed(ui: &impl Ui, rows: &[String]) -> Vec<(usize, Stamp)> {
    let top = |name: &str| ui.rect(&format!("{ROWS}.{name}")).filter(|r| r.3 > 0.0).map(|r| r.1);
    let mut days: Vec<(f32, (u32, u32))> = rows.iter().filter_map(|r| Some((top(r)?, header_day(r)?))).collect();
    days.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.iter()
        .filter_map(|r| {
            let index = r.parse::<usize>().ok()?;
            let y = top(r)?;
            let (month, day) = days.iter().rev().find(|(at, _)| *at <= y)?.1;
            let clock = ui.text(&format!("{ROWS}.{r}.time"))?;
            let (h, m) = clock.trim().split_once(':')?;
            Some((index, (month, day, h.trim().parse().ok()?, m.trim().parse().ok()?)))
        })
        .collect()
}

/// 列表里的日期标题 `h_20260225`：(月, 日)
fn header_day(name: &str) -> Option<(u32, u32)> {
    let digits = name.strip_prefix("h_")?;
    if digits.len() != 8 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((digits[4..6].parse().ok()?, digits[6..8].parse().ok()?))
}

/// 右边显示的时间（`2/25 00:02`）
pub fn shown_stamp(time: &str) -> Option<Stamp> {
    let (day, clock) = time.trim().split_once(' ')?;
    let (month, d) = day.split_once('/')?;
    let (h, m) = clock.trim().split_once(':')?;
    Some((month.trim().parse().ok()?, d.trim().parse().ok()?, h.trim().parse().ok()?, m.trim().parse().ok()?))
}

/// 新闻的日期（`2027-09-20T13:00:00`）
pub fn date_stamp(date: &str) -> Option<Stamp> {
    let (day, clock) = date.split_once('T')?;
    let mut parts = day.split('-').skip(1);
    let (month, d) = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
    let mut clock = clock.split(':');
    Some((month, d, clock.next()?.parse().ok()?, clock.next()?.parse().ok()?))
}

/// 列表那一行的标题（太长时游戏在后面加「...」截掉）是不是这条新闻的；读不出就不拦
fn title_fits(row: Option<&str>, title: &str) -> bool {
    let Some(row) = row else { return true };
    let shown = row.trim().trim_end_matches("...").trim_end_matches('…').trim();
    shown.is_empty() || title.starts_with(shown)
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

    /// 收件箱：左边一个日期（2/18）下面几行（序号, 时间），从上往下；右边打开了一条（读不出字，只有时间）
    fn inbox(rows: &[(usize, &str)], time: &str) -> FakeUi {
        let mut ui = FakeUi::default();
        for p in [PAGE, "main.top.right.news.background", "main.top.right.news.background.list", ROWS, "main.top.right.news.contents"] {
            ui.add(p, "empty");
        }
        ui.add(&format!("{ROWS}.h_20280218"), "color").rect = (309.0, 228.0, 461.0, 28.0);
        for (k, (r, clock)) in rows.iter().enumerate() {
            let row = format!("{ROWS}.{r}");
            ui.add(&row, "color_selectable").rect = (309.0, 260.0 + 64.0 * k as f32, 461.0, 60.0);
            ui.add(&format!("{row}.time"), "label").text = Some(clock.to_string());
            ui.add(&format!("{row}.title"), "label").text = Some(format!("电竞大脑装备平衡 {r}"));
        }
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

    fn open_at(ui: &mut FakeUi, time: &str) {
        ui.nodes.get_mut(&format!("{OPEN}.top.time")).unwrap().text = Some(time.into());
    }

    #[test]
    fn dates_and_times_are_read_as_the_inbox_shows_them() {
        assert_eq!(shown_stamp("2/25 00:02"), Some((2, 25, 0, 2)));
        assert_eq!(shown_stamp("12/5 13:30"), Some((12, 5, 13, 30)));
        assert_eq!(shown_stamp("junk"), None);
        assert_eq!(date_stamp("2028-02-18T00:02:00"), Some((2, 18, 0, 2)));
        assert_eq!(date_stamp("2027-12-05T13:30:00"), Some((12, 5, 13, 30)));
        assert_eq!(header_day("h_20260225"), Some((2, 25)));
        assert_eq!(header_day("79"), None);
        assert!(title_fits(Some("赶上补丁东风的 TheShy，本命英雄..."), "赶上补丁东风的 TheShy，本命英雄 食人魔 甚至获得了加强"));
        assert!(!title_fits(Some("别的新闻"), "电竞大脑装备平衡 2026.1.1"));
        assert_eq!(height_of("一行\n两行"), 3 * LINE_H);
    }

    #[test]
    fn the_open_news_is_the_only_row_at_the_time_shown() {
        let _serial = crate::tests::serial();
        // 补丁日：电竞大脑三条、别人两条都在 00:00，装备平衡挪到了 00:02（00:01 是别人的）
        crate::shared::set_our_news(vec![
            ours(79, "2028-02-18T00:02:00", &long()),
            ours(77, "2028-02-18T00:00:00", "短短一条。"),
            ours(76, "2028-02-18T00:00:00", &long()),
        ]);
        let mut ui = inbox(&[(79, "00:02"), (78, "00:01"), (77, "00:00"), (76, "00:00"), (75, "00:00")], "2/18 00:02");
        let mut view = NewsScroll::default();
        ticks(&mut view, &mut ui, &[]);
        assert!(ui.handlers.iter().any(|h| h == &format!("{ROWS}.79")), "the rows' clicks are followed");
        assert!(!ui.handlers.iter().any(|h| h.contains("h_2028")), "not the date headers");
        let ours_box = format!("{OPEN}.contents.{OURS}");
        assert!(ui.exists(&ours_box), "no click needed: 00:02 is only the item news");
        assert_eq!(ui.text(&format!("{ours_box}.contents.text")), Some(long()));
        assert_eq!(ui.visible(&format!("{OPEN}.contents.text")), Some(false));
        assert!(ui.spawned.last().unwrap().1.contains(&format!("height: {}px", 61 * LINE_H)));
        let spawned = ui.spawned.len();
        ticks(&mut view, &mut ui, &[]);
        assert_eq!(ui.spawned.len(), spawned, "nothing more while it stays");
        // 打开别人的（00:01）：收起来，原来的字放回来
        open_at(&mut ui, "2/18 00:01");
        ticks(&mut view, &mut ui, &[]);
        assert!(!ui.exists(&ours_box));
        assert_eq!(ui.visible(&format!("{OPEN}.contents.text")), Some(true));
        // 00:00 有三行：没点过就不接手（哪怕其中 76 是我们的长新闻）
        open_at(&mut ui, "2/18 00:00");
        ticks(&mut view, &mut ui, &[]);
        assert!(!ui.exists(&ours_box));
        // 点了 76：就是它
        ticks(&mut view, &mut ui, &[format!("{ROWS}.76")]);
        assert!(ui.exists(&ours_box), "clicked among the rows at 00:00");
        // 点了 77（我们的短新闻）：不用滚
        ticks(&mut view, &mut ui, &[format!("{ROWS}.77")]);
        assert!(!ui.exists(&ours_box));
        crate::shared::set_our_news(Vec::new());
    }

    #[test]
    fn news_not_known_or_not_matching_are_left_alone() {
        let _serial = crate::tests::serial();
        // 还没读到这条新闻：不接手
        crate::shared::set_our_news(Vec::new());
        let mut ui = inbox(&[(79, "00:02"), (78, "00:00")], "2/18 00:02");
        let mut view = NewsScroll::default();
        ticks(&mut view, &mut ui, &[]);
        assert!(!ui.exists(&format!("{OPEN}.contents.{OURS}")));
        // 记下的时间对不上（新闻被挪过、记的是旧的）：不接手
        crate::shared::set_our_news(vec![ours(79, "2028-02-18T00:00:00", &long())]);
        ticks(&mut view, &mut ui, &[]);
        assert!(!ui.exists(&format!("{OPEN}.contents.{OURS}")));
        // 对上了：接手；位置还没排好（高度 0）的时候不读
        crate::shared::set_our_news(vec![ours(79, "2028-02-18T00:02:00", &long())]);
        let mut fresh = inbox(&[(79, "00:02"), (78, "00:00")], "2/18 00:02");
        fresh.nodes.get_mut(&format!("{ROWS}.79")).unwrap().rect = (0.0, 0.0, 0.0, 0.0);
        let mut view = NewsScroll::default();
        ticks(&mut view, &mut fresh, &[]);
        assert!(!fresh.exists(&format!("{OPEN}.contents.{OURS}")));
        fresh.nodes.get_mut(&format!("{ROWS}.79")).unwrap().rect = (309.0, 260.0, 461.0, 60.0);
        ticks(&mut view, &mut fresh, &[]);
        assert!(fresh.exists(&format!("{OPEN}.contents.{OURS}")), "read again once laid out");
        // 点了一条以后用键盘换了别的（右边的时间变了）：点的那条不算了
        crate::shared::set_our_news(vec![ours(78, "2028-02-18T00:00:00", &long())]);
        let mut ui = inbox(&[(78, "00:00"), (77, "00:00")], "2/18 00:00");
        let mut view = NewsScroll::default();
        ticks(&mut view, &mut ui, &[format!("{ROWS}.78")]);
        assert!(ui.exists(&format!("{OPEN}.contents.{OURS}")));
        open_at(&mut ui, "2/17 00:00");
        ticks(&mut view, &mut ui, &[]);
        open_at(&mut ui, "2/18 00:00");
        ticks(&mut view, &mut ui, &[]);
        assert!(!ui.exists(&format!("{OPEN}.contents.{OURS}")), "the click was before the time changed");
        crate::shared::set_our_news(Vec::new());
    }
}
