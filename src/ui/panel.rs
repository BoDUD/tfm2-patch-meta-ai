//! The meta panel, on any screen: F8 opens it and turns the page, and closes it after the last
//! page; Esc or its close button (top right) closes it, and so does moving to another screen or
//! tab. Apart from the close button nothing in it takes a click - the mouse goes through to the
//! game underneath - so the game stays usable whatever happens.
//!
//! 1. **Tier list** - the strongest champions this patch: tier, estimated win rate, change since
//!    the last patch, games, pick and ban rate, best lanes.
//! 2. **Scouting** - the team the player is about to face (or faced last): each player's best
//!    champions now, the champions the team plays most, and the bans that hurt them most.
//! 3. **Your players** - each of the player's athletes and their best champions now.
//! 4. **Model** - how well the model predicts, and what it rests on.
//!
//! A page is one block of `.ui` source with its text in it: turning the page replaces the block.

use std::collections::HashMap;

use super::names::name_ref;
use super::{color, quote, team_key, Context, Frame, Ui};
use crate::advisor;
use crate::glm::sigmoid;
use crate::history::Role;
use crate::meta::Meta;
use crate::model::Tier;
use crate::shared::Snapshot;

pub const HOTKEY: &str = "F8";
const NODE: &str = "pma_panel";
const PAGES: usize = 4;
const ROWS: usize = 24;
const HEAL_EVERY: u64 = 20;

const TEXT: u32 = 0xe8e8e8ff;
const DIM: u32 = 0xa3a9b6ff;
const GOOD: u32 = 0x4cc38aff;
const BAD: u32 = 0xef6471ff;
const ACCENT: u32 = 0x5b73ffff;

fn tier_color(t: Tier) -> u32 {
    match t {
        Tier::S => 0xff7a59ff,
        Tier::A => 0xf2c14eff,
        Tier::B => 0x5b73ffff,
        Tier::C => 0x8a8fa3ff,
        _ => 0x5a5d6bff,
    }
}

/// One cell: text, colour.
type Cell = (String, u32);

pub struct Page {
    pub title: String,
    /// (heading, x, width, right-aligned)
    pub columns: Vec<(&'static str, u32, u32, bool)>,
    pub rows: Vec<Vec<Cell>>,
    pub note: String,
}

#[derive(Default)]
pub struct Panel {
    /// Open page (0-based), or closed.
    page: Option<usize>,
    parent: String,
    next_heal: u64,
    /// The screen it was opened on: another screen closes it.
    screen: String,
}

/// Keys that close the panel at once.
const CLOSE_KEYS: [&str; 2] = ["Escape", "Esc"];

fn pts(v: f32) -> f32 {
    (sigmoid(v) - 0.5) * 100.0
}

fn signed(v: f32) -> String {
    format!("{}{:.1}", if v >= 0.0 { "+" } else { "" }, v)
}

fn tone(v: f32) -> u32 {
    if v >= 1.0 {
        GOOD
    } else if v <= -1.0 {
        BAD
    } else {
        DIM
    }
}

/// The page as `.ui` source.
pub fn source(page: &Page, index: usize) -> String {
    let label = |id: String, x: u32, y: u32, w: u32, size: u32, c: u32, right: bool, text: &str| {
        format!(
            "#{id}:label {{ @\"asset/base/style/main#label\"; x: {x}px; y: {y}px; width: {w}px; height: 28px; size: {size}; \
             color: {}; align_x: {}; align_y: Center; text: {}; ignore_event: true; }} ",
            color(c),
            if right { "Right" } else { "Left" },
            quote(text)
        )
    };
    let mut body = String::new();
    body.push_str(&label("title".into(), 28, 16, 1000, 22, TEXT, false, &page.title));
    body.push_str(&label(
        "pages".into(),
        800,
        18,
        500,
        13,
        DIM,
        true,
        &format!("page {}/{PAGES} · {HOTKEY} next 下一页 · Esc close 关闭", index + 1),
    ));
    // the game's own close button look
    body.push_str(
        "#close:button { width: 22px; height: 22px; anchor_x: 1; pivot_x: 1; x: -24px; y: 20px; \
         source: \"asset/base/ui/icons/cross\"; color: #c2c6ceff; hover: { color: #e8e8e8ff; } \
         active: { color: #e8e8e8ff; } } ",
    );
    for (i, (head, x, w, right)) in page.columns.iter().enumerate() {
        body.push_str(&label(format!("h{i}"), 28 + x, 60, *w, 13, DIM, *right, head));
    }
    for (r, row) in page.rows.iter().take(ROWS).enumerate() {
        let y = 92 + r as u32 * 29;
        for (i, (text, c)) in row.iter().enumerate() {
            let Some((_, x, w, right)) = page.columns.get(i) else { continue };
            body.push_str(&label(format!("r{r}c{i}"), 28 + x, y, *w, 15, *c, *right, text));
        }
    }
    body.push_str(&label("note".into(), 28, 806, 1324, 13, DIM, false, &page.note));
    format!(
        "{NODE}:empty {{ width: 100%; height: 100%; ignore_event: true; \
         #box:color {{ anchor_x: 0.5; pivot_x: 0.5; anchor_y: 0.5; pivot_y: 0.5; width: 1380px; height: 850px; \
         color: #161721f0; ignore_event: true; rounding: Uniform {{ rounding: 14; }} {body}}} }}"
    )
}

impl Panel {
    pub fn is_open(&self) -> bool {
        self.page.is_some()
    }

    /// One frame. `opponent` = the team to scout (lower-case name).
    pub fn tick(&mut self, ui: &mut impl Ui, f: &Frame<'_>) {
        let (frame, snapshot, context, opponent, screen, clicks) = (f.frame, f.snapshot, f.context, f.opponent, f.screen, f.clicks);
        let keys = ui.keys_pressed();
        let pressed = keys.iter().any(|k| k.eq_ignore_ascii_case(HOTKEY));
        let clicked = clicks.iter().any(|c| c.ends_with(&format!("{NODE}.box.close")));
        let close = clicked || keys.iter().any(|k| CLOSE_KEYS.iter().any(|c| k.eq_ignore_ascii_case(c)));
        let parent = if ui.exists("main") { "main".to_string() } else { ui.children("").into_iter().next().unwrap_or_default() };
        let moved = self.page.is_some() && screen != self.screen;
        if pressed || ((close || moved) && self.page.is_some()) {
            if !self.parent.is_empty() {
                ui.remove(&format!("{}.{NODE}", self.parent));
            }
            self.page = match self.page {
                _ if close || moved => None,
                None => Some(0),
                Some(p) if p + 1 < PAGES => Some(p + 1),
                Some(_) => None,
            };
            self.screen = screen.to_string();
            self.next_heal = 0;
        }
        let Some(index) = self.page else { return };
        if parent.is_empty() {
            return;
        }
        // drawn when opened or turned, and again when the game rebuilt the screen under it
        if frame < self.next_heal && ui.exists(&format!("{parent}.{NODE}")) {
            return;
        }
        self.next_heal = frame + HEAL_EVERY;
        if ui.exists(&format!("{parent}.{NODE}")) && !pressed && parent == self.parent {
            return;
        }
        self.parent = parent.clone();
        let page = match snapshot {
            None => Page {
                title: "Patch Meta".into(),
                columns: vec![],
                rows: vec![],
                note: "Reading this save's matches... / 正在读取本存档的比赛记录……".into(),
            },
            Some(s) => build(index, s, context, opponent),
        };
        if ui.spawn(&parent, &source(&page, index)) {
            ui.on_click(&format!("{parent}.{NODE}.box.close"));
        }
    }
}

pub fn build(index: usize, snapshot: &Snapshot, context: &Context, opponent: Option<&str>) -> Page {
    match index {
        0 => tier_page(&snapshot.meta),
        1 => scouting_page(snapshot, context, opponent),
        2 => players_page(&snapshot.meta, context),
        _ => model_page(&snapshot.meta, context),
    }
}

/// A champion's name, as the game's reference (shown in the game's language).
fn champ(meta: &Meta, id: u16) -> Cell {
    (name_ref(meta.names.name(id)), TEXT)
}

fn tier_page(meta: &Meta) -> Page {
    let tiers: HashMap<String, Tier> = crate::meta::tiers(meta, 10.0, [10.0, 20.0, 40.0, 20.0]).into_iter().collect();
    let mut list: Vec<_> = meta.champions.iter().filter(|c| tiers.contains_key(&c.name)).collect();
    list.sort_by(|a, b| b.cautious().partial_cmp(&a.cautious()).unwrap_or(std::cmp::Ordering::Equal));
    let rows = list
        .iter()
        .take(ROWS)
        .map(|c| {
            let tier = tiers[&c.name];
            let delta = c.previous.map(|p| (c.win_rate() - sigmoid(p)) * 100.0);
            let mut lanes: Vec<(Role, f32)> = Role::ALL
                .iter()
                .filter(|r| c.role_share()[r.index()] >= 0.15)
                .map(|r| (*r, c.role_rate(*r)))
                .collect();
            lanes.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let change = match &c.last_change {
                Some((v, d)) if *v == meta.current => (if *d > 0 { "▲" } else { "▼" }, if *d > 0 { GOOD } else { BAD }),
                _ => ("", DIM),
            };
            vec![
                (tier.as_str().to_string(), tier_color(tier)),
                (name_ref(&c.name), TEXT),
                (change.0.to_string(), change.1),
                (format!("{:.1}%", c.win_rate() * 100.0), TEXT),
                (format!("±{:.1}", (sigmoid(c.strength + c.sd) - c.win_rate()) * 100.0), DIM),
                (delta.map_or(String::new(), signed), delta.map_or(DIM, tone)),
                (format!("{} / {}", c.current.games, c.window.games), DIM),
                (format!("{:.0}%", c.pick_rate * 100.0), DIM),
                (format!("{:.0}%", c.ban_rate * 100.0), DIM),
                (lanes.iter().take(2).map(|(r, p)| format!("{} {:.0}%", r.name(), p * 100.0)).collect::<Vec<_>>().join("  "), TEXT),
            ]
        })
        .collect();
    Page {
        title: format!("Tier list 梯队 · patch {}", meta.current),
        columns: vec![
            ("Tier", 0, 50, false),
            ("Champion 英雄", 60, 270, false),
            ("", 334, 26, false),
            ("Power 实力胜率", 370, 100, true),
            ("", 476, 70, false),
            ("Δ prev", 556, 90, true),
            ("Games 场次", 656, 140, true),
            ("Pick", 806, 80, true),
            ("Ban", 896, 80, true),
            ("Best lanes 最佳位置", 1000, 320, false),
        ],
        rows,
        note: "Power = win rate with average team-mates and players; games = this patch / kept patches; \
               ▲▼ = changed this patch. 实力胜率 = 队友与选手均为平均水平时的胜率；▲▼ = 本版本被调整。"
            .into(),
    }
}

/// A player's best champions now: champion strength in their lane plus their mastery.
fn best_for(meta: &Meta, athlete: u32, role: Option<Role>, n: usize) -> Vec<(u16, f32, u32)> {
    let mut pool: Vec<(u16, f32, u32)> = meta
        .mastery
        .iter()
        .filter(|((a, _), e)| *a == athlete && e.tally.games > 0)
        .map(|((_, c), e)| {
            let base = match role {
                Some(r) => advisor::strength_in(meta, *c, r),
                None => meta.by_id(*c).map_or(0.0, |x| x.strength),
            };
            (*c, base + e.value, e.tally.games)
        })
        .collect();
    pool.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    pool.truncate(n);
    pool
}

/// Champions shown per row on the roster pages, each as a name and a number.
const PER_ROW: usize = 4;

fn roster_columns(last: &'static str) -> Vec<(&'static str, u32, u32, bool)> {
    let mut columns = vec![("Lane", 0, 80, false), ("Player 选手", 90, 200, false), ("Own", 300, 70, true)];
    for k in 0..PER_ROW as u32 {
        columns.push((if k == 0 { last } else { "" }, 390 + k * 236, 160, false));
        columns.push(("", 390 + k * 236 + 160, 66, true));
    }
    columns
}

fn roster_rows(meta: &Meta, context: &Context, roster: &[(u32, Option<Role>)]) -> Vec<Vec<Cell>> {
    let mut sorted = roster.to_vec();
    sorted.sort_by_key(|(_, r)| r.map_or(9, |r| r.index()));
    sorted
        .iter()
        .map(|(a, role)| {
            let skill = meta.athletes.get(a).copied().unwrap_or_default();
            let mut row = vec![
                (role.map_or("?", |r| r.name()).to_string(), DIM),
                (context.athletes.get(a).cloned().unwrap_or_else(|| format!("#{a}")), TEXT),
                (signed(pts(skill.value)), tone(pts(skill.value))),
            ];
            for (c, v, g) in best_for(meta, *a, *role, PER_ROW) {
                row.push(champ(meta, c));
                row.push((format!("{:.0}% ({g})", sigmoid(v) * 100.0), DIM));
            }
            row
        })
        .collect()
}

fn scouting_page(snapshot: &Snapshot, context: &Context, opponent: Option<&str>) -> Page {
    let meta = &snapshot.meta;
    let Some(team) = opponent.filter(|t| context.rosters.contains_key(*t)) else {
        return Page {
            title: "Scouting 对手侦察".into(),
            columns: vec![],
            rows: vec![],
            note: "No opponent known yet: open a ban/pick screen or play a match. 还不知道对手：进入一次选人界面或打一场比赛后再看。".into(),
        };
    };
    let roster = &context.rosters[team];
    let mut rows = roster_rows(meta, context, roster);
    rows.push(vec![]);
    // their most played champions
    if let Some(recent) = context.team_picks.get(team) {
        let mut row = vec![("Most played".into(), DIM), ("常用英雄".into(), DIM), (String::new(), DIM)];
        for (c, g, w) in recent.iter().take(PER_ROW) {
            row.push((name_ref(c), TEXT));
            row.push((format!("{g}g {:.0}%", *w as f32 * 100.0 / (*g).max(1) as f32), DIM));
        }
        rows.push(row);
    }
    // the bans that hurt them most (nothing picked yet)
    let damage = |c: u16| snapshot.damage_of(c);
    let mut bans: Vec<(u16, f32)> = meta
        .champions
        .iter()
        .filter(|c| c.window.games > 0)
        .map(|c| (c.id, advisor::ban_value(meta, c.id, &[], &[], roster, &damage).total))
        .collect();
    bans.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut row = vec![("Ban".into(), ACCENT), ("建议禁用".into(), ACCENT), (String::new(), DIM)];
    for (c, v) in bans.iter().take(PER_ROW) {
        row.push(champ(meta, *c));
        row.push((signed(pts(*v)), GOOD));
    }
    rows.push(row);
    Page {
        title: format!("Scouting 对手侦察 · {}", context.team_labels.get(team).cloned().unwrap_or_else(|| team.to_string())),
        columns: roster_columns("Best champions now 当前最佳英雄 (games)"),
        rows,
        note: "Own = the player's own strength (win-rate points). Best = champion strength in their lane + their mastery. \
               Own = 选手本人实力；最佳 = 英雄在其位置的强度 + 选手熟练度。"
            .into(),
    }
}

fn players_page(meta: &Meta, context: &Context) -> Page {
    let roster = context.rosters.get(&team_key(&context.team_name)).cloned().unwrap_or_default();
    Page {
        title: format!("Your players 我的选手 · {}", context.team_name),
        columns: roster_columns("Best champions now 当前最佳英雄 (games)"),
        rows: roster_rows(meta, context, &roster),
        note: if roster.is_empty() {
            "No line-up seen yet: play a competition match. 还没有读到阵容：打一场正式比赛后再看。".into()
        } else {
            "Best = champion strength this patch in the player's lane + their own mastery of it. 最佳 = 英雄本版本在该位置的强度 + 选手熟练度。".into()
        },
    }
}

fn model_page(meta: &Meta, context: &Context) -> Page {
    let mut rows: Vec<Vec<Cell>> = vec![
        vec![("Patch 版本".into(), DIM), (format!("{} (kept: {})", meta.current, meta.versions.join(", ")), TEXT)],
        vec![
            ("Matches 比赛".into(), DIM),
            (format!("{} this patch, {} kept, {} solo rank", meta.current_matches, meta.matches, meta.solo_matches), TEXT),
        ],
        vec![("Blue side 蓝方".into(), DIM), (format!("{} win-rate points", signed(pts(meta.side))), TEXT)],
    ];
    match &context.backtest {
        Some(bt) => {
            rows.push(vec![
                ("Check 检验".into(), DIM),
                (format!("on the {} newest matches it was not fitted on, the favourite won {:.0}%", bt.games, bt.accuracy * 100.0), TEXT),
            ]);
            rows.push(vec![
                ("Brier".into(), DIM),
                (format!("{:.3} (a coin flip scores {:.3}; lower is better)", bt.brier, bt.coin_brier), TEXT),
            ]);
        }
        None => rows.push(vec![("Check 检验".into(), DIM), ("not enough matches yet / 比赛还不够".into(), DIM)]),
    }
    rows.push(vec![]);
    rows.push(vec![("Report 报告".into(), DIM), ("meta_report.html in the mod folder (open it in a browser) / Mod 文件夹里的 meta_report.html".into(), TEXT)]);
    Page {
        title: "Model 模型".into(),
        columns: vec![("", 0, 180, false), ("", 190, 1130, false)],
        rows,
        note: format!("{} {}", crate::MOD_ID, crate::VERSION),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::tests::{simulate, NAMES};
    use crate::meta::{build as fit, Inputs, Settings};
    use crate::ui::tests::FakeUi;

    #[test]
    fn pages_turn_and_close() {
        let (names, games) = simulate(1500, "1.1", |n| if n == "c" { 0.5 } else { 0.0 }, 8);
        let champions: Vec<String> = NAMES.iter().map(|s| s.to_string()).collect();
        let meta = fit(
            &Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.1", warm: None },
            &Settings::default(),
        );
        let snapshot = Snapshot { meta, damage: HashMap::new() };
        let mut context = Context { team_name: "Mods FC".into(), ..Default::default() };
        let roster: Vec<(u32, Option<Role>)> = (1..=5).map(|a| (a, Some(Role::ALL[a as usize - 1]))).collect();
        context.rosters.insert("mods fc".into(), roster.clone());
        context.rosters.insert("rivals".into(), (6..=10).map(|a| (a, Some(Role::ALL[a as usize - 6]))).collect());
        context.athletes.insert(1, "Faker".into());

        let tiers = build(0, &snapshot, &context, None);
        assert!(!tiers.rows.is_empty() && tiers.rows[0][1].0 == name_ref("c"), "c on top: {:?}", tiers.rows[0]);
        let scout = build(1, &snapshot, &context, Some("rivals"));
        assert_eq!(scout.rows.len(), 5 + 2, "five players, a gap, the bans");
        assert!(scout.rows.last().unwrap().iter().any(|(t, _)| *t == name_ref("c")), "c is the ban");
        let none = build(1, &snapshot, &context, None);
        assert!(none.rows.is_empty() && none.note.contains("No opponent"));
        let mine = build(2, &snapshot, &context, None);
        assert_eq!(mine.rows[0][1].0, "Faker");

        let mut ui = FakeUi::default();
        ui.add("main", "main_ui");
        let mut panel = Panel::default();
        let press = |ui: &mut FakeUi, panel: &mut Panel, frame: u64| {
            ui.keys = vec!["F8".into()];
            panel.tick(ui, &Frame { frame, snapshot: Some(&snapshot), context: &context, opponent: Some("rivals"), screen: "Main/Home", clicks: &[] });
            ui.keys.clear();
        };
        press(&mut ui, &mut panel, 1);
        assert!(ui.exists("main.pma_panel.box.title") && ui.exists("main.pma_panel.box.r0c1"));
        assert!(ui.spawned.last().unwrap().1.contains("Tier list"));
        for frame in 2..PAGES as u64 + 1 {
            press(&mut ui, &mut panel, frame * 10);
        }
        assert!(ui.spawned.last().unwrap().1.contains("Model"));
        // rebuilt by the game: back after a moment
        ui.remove("main.pma_panel");
        panel.tick(&mut ui, &Frame { frame: 1000, snapshot: Some(&snapshot), context: &context, opponent: None, screen: "Main/Home", clicks: &[] });
        assert!(ui.exists("main.pma_panel"));
        let source = ui.spawned.last().unwrap().1.clone();
        press(&mut ui, &mut panel, 1001);
        assert!(!ui.exists("main.pma_panel") && !panel.is_open());
        // nothing in it takes a click
        assert!(!source.contains("#00000099"), "no full-screen layer");
        assert_eq!(source.matches("ignore_event: true").count(), source.matches(":label").count() + 2);

        // Esc closes it; so does another screen or tab
        press(&mut ui, &mut panel, 1100);
        assert!(panel.is_open());
        ui.keys = vec!["Escape".into()];
        panel.tick(&mut ui, &Frame { frame: 1101, snapshot: Some(&snapshot), context: &context, opponent: None, screen: "Main/Home", clicks: &[] });
        ui.keys.clear();
        assert!(!panel.is_open() && !ui.exists("main.pma_panel"));
        press(&mut ui, &mut panel, 1200);
        assert!(panel.is_open());
        panel.tick(&mut ui, &Frame { frame: 1201, snapshot: Some(&snapshot), context: &context, opponent: None, screen: "Main/Squad", clicks: &[] });
        assert!(!panel.is_open() && !ui.exists("main.pma_panel"));

        // the close button
        press(&mut ui, &mut panel, 1300);
        assert!(ui.exists("main.pma_panel.box.close"));
        ui.click("main.pma_panel.box.close");
        let clicks = crate::ui::take_clicks();
        panel.tick(&mut ui, &Frame { frame: 1301, snapshot: Some(&snapshot), context: &context, opponent: None, screen: "Main/Home", clicks: &clicks });
        assert!(!panel.is_open() && !ui.exists("main.pma_panel"));
        press(&mut ui, &mut panel, 1400);
        assert!(panel.is_open(), "opens again after a click closed it");
    }
}
