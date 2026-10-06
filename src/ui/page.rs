//! The Meta Analysis page: an entry of its own in the management screen's left menu (under
//! the gaming house), opening a full page over the right-hand content in the look of the
//! game's statistics screen:
//!
//! - **Champions**: every champion this patch - tier, estimated win rate, change since the last
//!   patch, games, pick / ban / presence, best lanes, last balance change. Headers sort, the
//!   lane buttons switch every number to that lane, a row opens the champion's page;
//! - **champion page**: its numbers, a card per lane, its best team-mates and its matchups both
//!   ways (each opens that champion), its strength over the kept patches and the players who
//!   play it best;
//! - **Duos & Matchups**, **Players** (the player's line-up and the next opponent's, with each
//!   player's best champions now), **Model** (how well it predicts).
//!
//! Everything sits under `main` (the screen root has no automatic layout, unlike the left
//! menu, which stacks its children). Another screen or tab (any of the game's menu entries), Esc,
//! or the entry again closes the page and gives the content underneath back. Nothing is attached
//! to the game's own menu entries: their runner panics on anything it does not expect. Texts are the game's own
//! (`#asset/base/text/ui?...`) or this mod's, merged into the same document from
//! `text/ui.i18n`; when the merge is missing the English words are written instead.
//!
//! The champion table's rows are spawned a batch per frame and filled in place on every sort or
//! lane change, so a sort costs some text writes, not a rebuild.

// `.ui` builders take a node's whole geometry and look; splitting that up reads worse
#![allow(clippy::too_many_arguments)]

use std::collections::{HashMap, HashSet};

use super::names::name_ref;
use super::{color, quote, team_key, Context, Frame, Ui};
use crate::glm::sigmoid;
use crate::history::Role;
use crate::meta::{Champion, Effect, Meta};
use crate::model::Tier;
use crate::shared::Snapshot;

pub const NAV: &str = "main.pma_nav";
pub const PAGE: &str = "main.pma_page";
const SCREEN: &str = "main.pma_page.screen";
const BODY: &str = "main.pma_page.screen.body";
const LIST: &str = "main.pma_page.screen.body.data.list.contents";
const LEFT_MENU: &str = "main.top.left";
const RIGHT: &str = "main.top.right";

const ROW_H: u32 = 56;
const ROW_PITCH: u32 = 61;
const ROWS_PER_FRAME: usize = 30;
const HEAL_EVERY: u64 = 10;
const CLOSE_KEYS: [&str; 2] = ["Escape", "Esc"];

const BG: u32 = 0x07080bff;
const PANEL: u32 = 0x161721ff;
const HEADER: u32 = 0x0f1016ff;
const SLOT: u32 = 0x1d1f2cff;
const LINE: u32 = 0x1d1f2cff;
const BORDER: u32 = 0x4a4c56ff;
const TEXT: u32 = 0xe8e8e8ff;
const DIM: u32 = 0xa3a9b6ff;
const GOOD: u32 = 0x4cc38aff;
const BAD: u32 = 0xef6471ff;
const ACCENT: u32 = 0x37d5b3ff;
const LIT: u32 = 0xecfbf8ff;
const LIT_TEXT: u32 = 0x0f5b4dff;

fn tier_color(t: Tier) -> u32 {
    match t {
        Tier::S => 0xff7a59ff,
        Tier::A => 0xf2c14eff,
        Tier::B => 0x5b73ffff,
        Tier::C => 0x8a8fa3ff,
        _ => 0x5a5d6bff,
    }
}

fn lane_icon(r: Role) -> &'static str {
    match r {
        Role::Top => "asset/base/ui/icons/top",
        Role::Jungle => "asset/base/ui/icons/jungle",
        Role::Mid => "asset/base/ui/icons/mid",
        Role::Bottom => "asset/base/ui/icons/bottom",
        Role::Support => "asset/base/ui/icons/support",
    }
}

fn lane_text(r: Role) -> String {
    format!("#asset/base/text/ui?position.{}", r.name().to_lowercase())
}

fn pts(v: f32) -> f32 {
    (sigmoid(v) - 0.5) * 100.0
}

fn signed(v: f32) -> String {
    format!("{}{:.1}", if v >= 0.0 { "+" } else { "" }, v)
}

fn tone(v: f32) -> u32 {
    if v >= 0.5 {
        GOOD
    } else if v <= -0.5 {
        BAD
    } else {
        DIM
    }
}

/// This mod's texts: the merged `patch_meta.*` entry when the game has it, else English.
struct Texts<'a, U: Ui> {
    ui: &'a U,
    cache: HashMap<&'static str, String>,
}

impl<'a, U: Ui> Texts<'a, U> {
    fn new(ui: &'a U) -> Self {
        Self { ui, cache: HashMap::new() }
    }

    fn get(&mut self, key: &'static str, english: &str) -> String {
        if let Some(t) = self.cache.get(key) {
            return t.clone();
        }
        let reference = format!("#asset/base/text/ui?patch_meta.{key}");
        let t = if self.ui.has_text(&reference) { reference } else { english.to_string() };
        self.cache.insert(key, t.clone());
        t
    }
}

// ---------------------------------------------------------------- `.ui` source helpers

/// A label. `text` may be a `#asset/...` reference (resolved in the game's language).
fn label(id: &str, x: i32, y: i32, w: u32, h: u32, size: u32, c: u32, align: &str, text: &str) -> String {
    format!(
        "#{id}:label {{ @\"asset/base/style/main#label\"; x: {x}px; y: {y}px; width: {w}px; height: {h}px; size: {size}; \
         color: {}; align_x: {align}; align_y: Center; ignore_event: true; text: {}; }} ",
        color(c),
        quote(text)
    )
}

fn bold(id: &str, x: i32, y: i32, w: u32, h: u32, size: u32, c: u32, align: &str, text: &str) -> String {
    format!(
        "#{id}:label {{ @\"asset/base/style/main#bold_label\"; x: {x}px; y: {y}px; width: {w}px; height: {h}px; size: {size}; \
         color: {}; align_x: {align}; align_y: Center; ignore_event: true; text: {}; }} ",
        color(c),
        quote(text)
    )
}

fn rect(id: &str, x: i32, y: i32, w: u32, h: u32, c: u32, rounding: u32, inner: &str) -> String {
    format!(
        "#{id}:color {{ x: {x}px; y: {y}px; width: {w}px; height: {h}px; color: {}; ignore_event: true; \
         rounding: Uniform {{ rounding: {rounding}; }} {inner}}} ",
        color(c)
    )
}

fn image(id: &str, x: i32, y: i32, size: u32, source: &str, c: u32) -> String {
    format!(
        "#{id}:image {{ x: {x}px; y: {y}px; width: {size}px; height: {size}px; source: {}; color: {}; ignore_event: true; }} ",
        quote(source),
        color(c)
    )
}

/// A champion portrait slot (the game's look: a rounded square behind the face).
fn portrait(id: &str, x: i32, y: i32, size: u32) -> String {
    let inner = size - 4;
    format!(
        "#{id}:color {{ x: {x}px; y: {y}px; width: {size}px; height: {size}px; color: {}; ignore_event: true; \
         rounding: Uniform {{ rounding: 8; }} #icon:image {{ width: {inner}px; height: {inner}px; anchor_x: 0.5; \
         anchor_y: 0.5; pivot_x: 0.5; pivot_y: 0.5; ignore_event: true; }} }} ",
        color(SLOT)
    )
}

/// A tab / toggle in the game's `strategy_option` look, lit or not.
fn option(id: &str, w: u32, h: u32, size: u32, text: &str, lit: bool) -> String {
    format!(
        "#{id}:color_selectable {{ @\"asset/base/style/main#strategy_option\"; width: {w}px; height: {h}px; text: {}; {} }} ",
        quote(text),
        option_style(size, lit)
    )
}

fn option_style(size: u32, lit: bool) -> String {
    // the game's `strategy_option` colours, lit (selected) or idle
    let (fill, stroke, text, hover_line, hover_text) =
        if lit { (LIT, 0, LIT_TEXT, LIT, LIT_TEXT) } else { (0x00000000, 1, DIM, DIM, 0xe0e2e7ff) };
    format!(
        "image: {{ color: {f}; back_color: {f}; stroke: {stroke}; rounding: Uniform {{ rounding: 8; }} \
         hover: {{ color: {hl}; back_color: {f}; }} }} \
         label: {{ font: \"asset/base/font/set/bold\"; size: {size}; align_x: Center; align_y: Center; color: {t}; \
         hover: {{ color: {ht}; }} }}",
        f = color(fill),
        hl = color(hover_line),
        t = color(text),
        ht = color(hover_text)
    )
}

/// A transparent clickable area with a hover tint.
fn hot(id: &str, x: i32, y: i32, w: u32, h: u32, inner: &str) -> String {
    format!(
        "#{id}:color_icon_button {{ x: {x}px; y: {y}px; width: {w}px; height: {h}px; btn: {{ color: #00000000; }} \
         hover: {{ btn: {{ color: {}; }} }} {inner}}} ",
        color(0x23253380)
    )
}

fn tier_badge(id: &str, x: i32, y: i32, tier: Option<Tier>) -> String {
    match tier {
        Some(t) => rect(id, x, y, 32, 28, tier_color(t), 6, &bold("t", 0, 0, 32, 28, 16, 0x07080bff, "Center", t.as_str())),
        None => label(id, x, y, 32, 28, 16, DIM, "Center", "-"),
    }
}

// ---------------------------------------------------------------- state

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum View {
    Champions,
    Champion(u16),
    Pairs,
    Players,
    Model,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sort {
    Rank,
    Tier,
    Win,
    Delta,
    Games,
    Pick,
    Ban,
    Presence,
}

/// The table's columns: (sort key or none, x, width, heading).
const COLUMNS: [(Option<Sort>, i32, u32); 11] = [
    (Some(Sort::Rank), 0, 68),
    (None, 68, 260),
    (Some(Sort::Tier), 328, 72),
    (Some(Sort::Win), 400, 170),
    (Some(Sort::Delta), 570, 130),
    (Some(Sort::Games), 700, 130),
    (Some(Sort::Pick), 830, 100),
    (Some(Sort::Ban), 930, 100),
    (Some(Sort::Presence), 1030, 120),
    (None, 1150, 300),
    (None, 1450, 150),
];

const SORT_IDS: [(Sort, &str); 8] = [
    (Sort::Rank, "rank"),
    (Sort::Tier, "tier"),
    (Sort::Win, "win"),
    (Sort::Delta, "delta"),
    (Sort::Games, "games"),
    (Sort::Pick, "pick"),
    (Sort::Ban, "ban"),
    (Sort::Presence, "presence"),
];

const TABS: [(&str, &str, &str); 4] =
    [("champions", "tab_champions", "Champions"), ("pairs", "tab_pairs", "Synergy & Matchups"), ("players", "tab_players", "Players"), ("model", "tab_model", "Model")];

pub struct MetaPage {
    open: bool,
    view: View,
    sort: Sort,
    descending: bool,
    lane: Option<Role>,
    /// What the body was last drawn for.
    drawn: Option<u64>,
    /// Champion ids of the table rows, in order; rows spawned so far.
    order: Vec<u16>,
    spawned: usize,
    filled: Option<u64>,
    /// Clickable champions on a champion page: path -> champion.
    links: HashMap<String, u16>,
    screen: String,
    next_heal: u64,
    nav_seen: bool,
    /// The entry's look as last written.
    nav_lit: Option<bool>,
    registered: HashSet<String>,
}

impl Default for MetaPage {
    fn default() -> Self {
        Self {
            open: false,
            view: View::Champions,
            sort: Sort::Rank,
            descending: false,
            lane: None,
            drawn: None,
            order: Vec::new(),
            spawned: 0,
            filled: None,
            links: HashMap::new(),
            screen: String::new(),
            next_heal: 0,
            nav_seen: false,
            nav_lit: None,
            registered: HashSet::new(),
        }
    }
}

/// One champion's row values for the current lane.
struct RowData<'a> {
    c: &'a Champion,
    tier: Option<Tier>,
    win: f32,
    sd: f32,
    delta: Option<f32>,
    games: u32,
}

impl MetaPage {
    pub fn is_open(&self) -> bool {
        self.open
    }

    fn register(&mut self, ui: &mut impl Ui, path: &str) {
        if self.registered.insert(path.to_string()) {
            ui.on_click(path);
        }
    }

    /// One frame on any screen. `screen` identifies the screen and tab (another one closes the
    /// page); `clicks` are this frame's clicks.
    pub fn tick(&mut self, ui: &mut impl Ui, f: &Frame<'_>) {
        let (frame, snapshot, context, opponent, screen, clicks) = (f.frame, f.snapshot, f.context, f.opponent, f.screen, f.clicks);
        // the menu entry, on the management screen
        if !ui.exists(LEFT_MENU) {
            if self.open || self.nav_seen {
                self.forget();
            }
            return;
        }
        if !ui.exists(NAV) {
            // a new management screen: everything registered before belonged to the old one
            self.registered.clear();
            let mut t = Texts::new(ui);
            let text = t.get("menu", "Meta Analysis");
            if !ui.spawn("main", &nav_source(&text)) {
                return;
            }
            self.nav_seen = true;
            self.nav_lit = Some(false);
            // only our own button: nothing is attached to the game's menu entries (their runner
            // is touchy); picking one of them changes the screen, which closes the page
            self.register(ui, &format!("{NAV}.button"));
            if self.open {
                // the screen was rebuilt under an open page
                self.drawn = None;
            }
        }

        let escape = f.keys.iter().any(|k| CLOSE_KEYS.iter().any(|c| k.eq_ignore_ascii_case(c)));
        let was_open = self.open;
        for click in clicks {
            self.on_click(ui, click);
        }
        if self.open && !was_open {
            self.screen = screen.to_string();
        }
        if self.open && (escape || screen != self.screen) {
            self.close(ui);
        }
        if !self.open {
            return;
        }

        // the page frame, put back when the game rebuilt the screen under it
        if !ui.exists(SCREEN) {
            if !ui.spawn("main", &frame_source()) {
                return;
            }
            self.drawn = None;
        }
        if frame >= self.next_heal {
            self.next_heal = frame + HEAL_EVERY;
            // the game drives the content underneath; keep it hidden while the page is up
            if ui.visible(RIGHT) != Some(false) {
                ui.set_visible(RIGHT, false);
            }
            if self.nav_lit != Some(true) {
                self.nav_lit = Some(true);
                ui.set_properties(&format!("{NAV}.button"), &nav_style(true));
            }
        }
        let Some(snapshot) = snapshot.filter(|s| s.meta.matches + s.meta.solo_matches > 0) else {
            if self.drawn != Some(0) {
                self.drawn = Some(0);
                ui.remove(BODY);
                let mut t = Texts::new(ui);
                let tabs = tabs_source(&mut t, self.view);
                let note = t.get("no_data", "Not enough matches yet - play a few match days.");
                ui.spawn(SCREEN, &format!("body:empty {{ width: 100%; height: 100%; {tabs}{} }}", label("note", 0, 80, 1600, 40, 18, DIM, "Center", &note)));
                self.register_tabs(ui);
            }
            return;
        };
        let key = self.state_key(snapshot);
        if self.drawn != Some(key) {
            self.render(ui, snapshot, context, opponent, key);
        }
        if self.view == View::Champions {
            self.grow_rows(ui, snapshot);
        }
    }

    fn state_key(&self, snapshot: &Snapshot) -> u64 {
        use std::hash::{Hash, Hasher};
        // the table is filled in place: its sort and lane are not part of the body
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.view, snapshot.meta.matches, snapshot.meta.solo_matches, &snapshot.meta.current).hash(&mut h);
        h.finish() | 1
    }

    fn on_click(&mut self, ui: &mut impl Ui, path: &str) {
        if path == format!("{NAV}.button") {
            if self.open {
                self.close(ui);
            } else {
                self.open = true;
                self.view = View::Champions;
                self.drawn = None;
            }
            return;
        }
        if !self.open {
            return;
        }
        let Some(rest) = path.strip_prefix(&format!("{BODY}.")) else { return };
        if let Some(tab) = rest.strip_prefix("tabs.") {
            self.view = match tab {
                "pairs" => View::Pairs,
                "players" => View::Players,
                "model" => View::Model,
                _ => View::Champions,
            };
            return;
        }
        if let Some(lane) = rest.strip_prefix("lanes.") {
            self.lane = match lane {
                "top" => Some(Role::Top),
                "jungle" => Some(Role::Jungle),
                "mid" => Some(Role::Mid),
                "bottom" => Some(Role::Bottom),
                "support" => Some(Role::Support),
                _ => None,
            };
            self.filled = None;
            return;
        }
        if let Some(col) = rest.strip_prefix("data.header.") {
            if let Some((sort, _)) = SORT_IDS.iter().find(|(_, id)| *id == col) {
                if self.sort == *sort {
                    self.descending = !self.descending;
                } else {
                    self.sort = *sort;
                    // numbers start biggest first; rank and tier best first
                    self.descending = !matches!(sort, Sort::Rank | Sort::Tier);
                }
                self.filled = None;
            }
            return;
        }
        if let Some(row) = rest.strip_prefix("data.list.contents.r") {
            if let Some(c) = row.parse::<usize>().ok().and_then(|i| self.order.get(i)) {
                self.view = View::Champion(*c);
            }
            return;
        }
        if rest == "detail.back" {
            self.view = View::Champions;
            return;
        }
        if let Some(c) = self.links.get(path) {
            self.view = View::Champion(*c);
        }
    }

    fn close(&mut self, ui: &mut impl Ui) {
        ui.remove(PAGE);
        ui.set_visible(RIGHT, true);
        ui.set_properties(&format!("{NAV}.button"), &nav_style(false));
        self.nav_lit = Some(false);
        self.open = false;
        self.drawn = None;
        self.spawned = 0;
        self.filled = None;
        // handlers of removed nodes: registered again with the next page
        let keep: Vec<String> = self.registered.iter().filter(|p| !p.starts_with(PAGE)).cloned().collect();
        self.registered = keep.into_iter().collect();
    }

    fn forget(&mut self) {
        self.open = false;
        self.drawn = None;
        self.spawned = 0;
        self.filled = None;
        self.nav_seen = false;
        self.registered.clear();
    }

    fn register_tabs(&mut self, ui: &mut impl Ui) {
        for (id, _, _) in TABS {
            self.register(ui, &format!("{BODY}.tabs.{id}"));
        }
    }

    fn render(&mut self, ui: &mut impl Ui, snapshot: &Snapshot, context: &Context, opponent: Option<&str>, key: u64) {
        ui.remove(BODY);
        self.registered.retain(|p| !p.starts_with(BODY));
        self.links.clear();
        self.spawned = 0;
        self.filled = None;
        let meta = &snapshot.meta;
        let tiers: HashMap<String, Tier> = crate::meta::tiers(meta, 10.0, [10.0, 20.0, 40.0, 20.0]).into_iter().collect();
        let mut t = Texts::new(ui);
        let tabs = tabs_source(&mut t, self.view);
        let (inner, icons, links) = match self.view {
            View::Champions => (champions_source(&mut t, self.lane), Vec::new(), Vec::new()),
            View::Champion(c) => champion_source(&mut t, meta, &tiers, c, context),
            View::Pairs => pairs_source(&mut t, meta),
            View::Players => players_source(&mut t, meta, context, opponent),
            View::Model => (model_source(&mut t, meta, context), Vec::new(), Vec::new()),
        };
        if !ui.spawn(SCREEN, &format!("body:empty {{ width: 100%; height: 100%; {tabs}{inner}}}")) {
            return;
        }
        self.drawn = Some(key);
        self.register_tabs(ui);
        for (path, champ) in icons {
            ui.set_champion_icon(&format!("{BODY}.{path}"), &champ, 0.0);
        }
        for (path, c) in links {
            let full = format!("{BODY}.{path}");
            self.register(ui, &full);
            self.links.insert(full, c);
        }
        match self.view {
            View::Champions => {
                for (_, id) in SORT_IDS {
                    self.register(ui, &format!("{BODY}.data.header.{id}"));
                }
                for id in ["all", "top", "jungle", "mid", "bottom", "support"] {
                    self.register(ui, &format!("{BODY}.lanes.{id}"));
                }
            }
            View::Champion(_) => self.register(ui, &format!("{BODY}.detail.back")),
            _ => {}
        }
    }

    /// The table: rows in the current order, spawned a batch per frame, filled in place.
    fn grow_rows(&mut self, ui: &mut impl Ui, snapshot: &Snapshot) {
        let want = self.drawn.unwrap_or(0) ^ (self.sort as u64) << 8 ^ (self.descending as u64) << 16 ^ self.lane.map_or(7, |r| r as u64) << 20;
        // filled for this model, sort and lane, every row spawned: nothing to do this frame
        if self.filled == Some(want) && self.spawned >= self.order.len() {
            return;
        }
        let meta = &snapshot.meta;
        let tiers: HashMap<String, Tier> = crate::meta::tiers(meta, 10.0, [10.0, 20.0, 40.0, 20.0]).into_iter().collect();
        let rows = self.rows(meta, &tiers);
        let order: Vec<u16> = rows.iter().map(|r| r.c.id).collect();
        let refill = self.filled != Some(want) || order != self.order;
        self.order = order;
        if refill {
            self.filled = Some(want);
            ui.set_properties(LIST, &format!("height: {}px;", (rows.len() as u32 * ROW_PITCH).max(ROW_PITCH)));
            self.paint_headers(ui);
            for (i, row) in rows.iter().enumerate().take(self.spawned) {
                fill_row(ui, i, row);
            }
            for i in rows.len()..self.spawned {
                ui.set_visible(&format!("{LIST}.r{i}"), false);
            }
        }
        if self.spawned < rows.len() {
            let end = (self.spawned + ROWS_PER_FRAME).min(rows.len());
            for (i, row) in rows.iter().enumerate().take(end).skip(self.spawned) {
                if !ui.spawn(LIST, &row_source(i)) {
                    return;
                }
                fill_row(ui, i, row);
                self.register(ui, &format!("{LIST}.r{i}"));
            }
            self.spawned = end;
        }
    }

    fn rows<'a>(&self, meta: &'a Meta, tiers: &HashMap<String, Tier>) -> Vec<RowData<'a>> {
        let lane = self.lane;
        let mut rows: Vec<RowData<'a>> = meta
            .champions
            .iter()
            .filter(|c| c.window.games > 0)
            .filter(|c| lane.is_none_or(|r| c.role_share()[r.index()] >= 0.1 || c.roles[r.index()].tally.games >= 5))
            .map(|c| {
                let (win, prev) = match lane {
                    Some(r) => (c.role_rate(r), c.previous.map(|p| sigmoid(p + c.roles[r.index()].value))),
                    None => (c.win_rate(), c.previous.map(sigmoid)),
                };
                let games = match lane {
                    Some(r) => c.roles[r.index()].tally.games,
                    None => c.current.games,
                };
                RowData {
                    c,
                    tier: tiers.get(&c.name).copied(),
                    win,
                    sd: (sigmoid(c.strength + c.sd) - c.win_rate()) * 100.0,
                    delta: prev.map(|p| (win - p) * 100.0),
                    games,
                }
            })
            .collect();
        // rank = the cautious estimate (as the tiers); the others by their own number
        let value = |r: &RowData| -> f32 {
            match self.sort {
                Sort::Rank | Sort::Tier => r.c.cautious() + if self.sort == Sort::Tier { -(r.tier.map_or(9, |t| t as i32) as f32) * 10.0 } else { 0.0 },
                Sort::Win => r.win,
                Sort::Delta => r.delta.unwrap_or(f32::MIN),
                Sort::Games => r.games as f32,
                Sort::Pick => r.c.pick_rate,
                Sort::Ban => r.c.ban_rate,
                Sort::Presence => r.c.presence(),
            }
        };
        rows.sort_by(|a, b| {
            let o = value(b).partial_cmp(&value(a)).unwrap_or(std::cmp::Ordering::Equal).then(a.c.id.cmp(&b.c.id));
            // rank and tier read "best first" by default (ascending rank)
            let best_first = matches!(self.sort, Sort::Rank | Sort::Tier);
            if self.descending != best_first {
                o
            } else {
                o.reverse()
            }
        });
        rows
    }

    fn paint_headers(&self, ui: &mut impl Ui) {
        for (sort, id) in SORT_IDS {
            let lit = sort == self.sort;
            let up = lit && (self.descending == matches!(sort, Sort::Rank | Sort::Tier));
            let source = if up { "asset/base/ui/icons/dropdown_up" } else { "asset/base/ui/icons/dropdown" };
            let w = COLUMNS.iter().find(|c| c.0 == Some(sort)).map_or(100, |c| c.2);
            ui.set_properties(
                &format!("{BODY}.data.header.{id}"),
                &format!(
                    "icon: {{ source: \"{source}\"; rect: {{ x: {}; y: 25.47; w: 8.78; h: 5.06; }} color: {}; }}",
                    w as i32 - 24,
                    color(if lit { LIT } else { 0x00000000 })
                ),
            );
        }
        let lanes = [("all", None), ("top", Some(Role::Top)), ("jungle", Some(Role::Jungle)), ("mid", Some(Role::Mid)), ("bottom", Some(Role::Bottom)), ("support", Some(Role::Support))];
        for (id, lane) in lanes {
            ui.set_properties(&format!("{BODY}.lanes.{id}"), &option_style(16, lane == self.lane));
        }
    }
}

// ---------------------------------------------------------------- sources

/// The entry's look, lit while the page is open: the game's own menu colours.
fn nav_style(lit: bool) -> String {
    let fill = if lit { BORDER } else { 0x00000000 };
    format!("btn: {{ color: {}; }} hover: {{ btn: {{ color: {}; }} }}", color(fill), color(if lit { BORDER } else { 0x23253380 }))
}

/// The left-menu entry: a separator and a button laid out like the game's menu entries (icon at
/// 24, text at 56), under the house. Not the game's `main_left_button` runner: game code expects
/// every one of those to belong to one of its own screens and panics on any other.
fn nav_source(text: &str) -> String {
    format!(
        "pma_nav:empty {{ x: 0px; y: 969px; width: 264px; height: 52px; \
         #bar:color {{ x: 24px; y: 0px; width: 215px; height: 1px; color: #a6a6a6ff; ignore_event: true; }} \
         #button:color_icon_button {{ y: 4px; width: 264px; height: 48px; {} \
         hover_sound: \"asset/base/sound/sfx/UI_mouse_hover\"; click_sound: \"asset/base/sound/sfx/UI_mouse_click\"; \
         #icon:image {{ x: 24px; y: 12px; width: 24px; height: 24px; ignore_event: true; color: #a5a5abff; \
         source: \"asset/base/ui/icons/chart\"; }} \
         #text:label {{ @\"asset/base/style/main#label\"; x: 56px; y: 14px; width: 184px; height: 20px; align_x: Left; \
         align_y: Center; size: 18; fit_width: true; ignore_event: true; color: #a5a5abff; text: {}; }} }} }}",
        nav_style(false),
        quote(text)
    )
}

/// The page: the screen background over the right-hand content, and the 1600x968 screen area
/// in the place of the game's screens.
fn frame_source() -> String {
    format!(
        "pma_page:color {{ x: 264px; y: 0px; width: 1656px; height: 992px; color: {}; \
         #screen:empty {{ x: 32px; y: 24px; width: 1600px; height: 968px; }} }}",
        color(BG)
    )
}

fn tabs_source<U: Ui>(t: &mut Texts<'_, U>, view: View) -> String {
    let current = match view {
        View::Champions | View::Champion(_) => "champions",
        View::Pairs => "pairs",
        View::Players => "players",
        View::Model => "model",
    };
    let mut tabs = String::new();
    for (id, key, english) in TABS {
        let text = t.get(key, english);
        tabs.push_str(&option(id, 232, 32, 18, &text, id == current));
    }
    format!(
        "#tabs:color {{ x: 0px; width: {}px; height: 39px; color: {}; stroke: 1; back_color: #00000000; \
         padding: {{ left: 4px; right: 4px; top: 4px; bottom: 4px; }} child_type: LeftToRight {{ spacing: 0px; }} \
         rounding: Uniform {{ rounding: 8; }} {tabs}}} ",
        8 + 232 * TABS.len(),
        color(BORDER)
    )
}

fn champions_source<U: Ui>(t: &mut Texts<'_, U>, lane: Option<Role>) -> String {
    // lane toggles, right of the tabs
    let mut lanes = String::new();
    let all = t.get("lane_all", "All");
    lanes.push_str(&option("all", 76, 32, 16, &all, lane.is_none()));
    for r in Role::ALL {
        let id = r.name().to_lowercase();
        lanes.push_str(&format!(
            "#{id}:color_selectable {{ @\"asset/base/style/main#strategy_option\"; width: 52px; height: 32px; text: \"\"; {} \
             #icon:image {{ width: 20px; height: 20px; anchor_x: 0.5; anchor_y: 0.5; pivot_x: 0.5; pivot_y: 0.5; \
             source: \"{}\"; color: #c2c6ceff; ignore_event: true; }} }} ",
            option_style(16, lane == Some(r)),
            lane_icon(r)
        ));
    }
    let lane_bar = format!(
        "#lanes:color {{ anchor_x: 1; pivot_x: 1; x: 0px; width: {}px; height: 39px; color: {}; stroke: 1; \
         back_color: #00000000; padding: {{ left: 4px; right: 4px; top: 4px; bottom: 4px; }} \
         child_type: LeftToRight {{ spacing: 0px; }} rounding: Uniform {{ rounding: 8; }} {lanes}}} ",
        8 + 76 + 52 * 5,
        color(BORDER)
    );
    let headings: [String; 11] = [
        "#".into(),
        "#asset/base/text/ui?statistics.champion_name".into(),
        "#asset/base/text/ui?statistics.tier".into(),
        t.get("col_win", "Power Win Rate"),
        t.get("col_delta", "vs Last Patch"),
        t.get("col_games", "Games"),
        t.get("col_pick", "Pick"),
        t.get("col_ban", "Ban"),
        t.get("col_presence", "Contest Rate"),
        t.get("col_lanes", "Best Lanes"),
        t.get("col_patch", "Last Change"),
    ];
    let mut header = String::new();
    for ((sort, x, w), text) in COLUMNS.iter().zip(&headings) {
        let heading = label("text", 21, 18, w - 30, 20, 16, DIM, "Left", text);
        match sort.and_then(|s| SORT_IDS.iter().find(|(k, _)| *k == s)) {
            Some((_, id)) => header.push_str(&format!(
                "#{id}:color_icon_button {{ x: {x}px; width: {w}px; height: 56px; btn: {{ color: #00000000; }} \
                 icon: {{ source: \"asset/base/ui/icons/dropdown\"; rect: {{ x: {}; y: 25.47; w: 8.78; h: 5.06; }} color: #00000000; }} \
                 hover: {{ icon: {{ color: #c2c6ceff; }} }} {heading}}} ",
                *w as i32 - 24
            )),
            None => header.push_str(&format!("#c{x}:empty {{ x: {x}px; width: {w}px; height: 56px; {heading}}} ")),
        }
    }
    let note = t.get("note_win", "Power win rate: how often a team wins with this champion this patch, its team-mates and players being average.");
    format!(
        "{lane_bar}#data:color {{ y: 52px; width: 1600px; height: 916px; color: {}; rounding: Uniform {{ rounding: 12; }} \
         #header:color {{ width: 1600px; height: 56px; color: {}; rounding: Individual {{ top_left: 12; top_right: 12; }} {header}}} \
         #list:scroll_view {{ y: 56px; width: 1600px; height: 828px; speed: 100; bar_width: 4; bar_padding: {{ bottom: 12px; }} \
         bar: {{ source: \"asset/base/sprite/white\"; color: {}; hover: {{ color: #ecfbf8ff; }} }} \
         back: {{ source: \"asset/base/sprite/white\"; color: {}; }} \
         #contents:empty {{ width: 1600px; height: {ROW_PITCH}px; child_type: TopToBottom {{ spacing: 4px; }} }} }} \
         {}}} ",
        color(PANEL),
        color(HEADER),
        color(ACCENT),
        color(BORDER),
        label("note", 21, 886, 1560, 24, 13, DIM, "Left", &note)
    )
}

fn row_source(i: usize) -> String {
    let cells = [
        bold("rank", 21, 18, 40, 20, 18, DIM, "Left", ""),
        portrait("face", 89, 8, 40),
        label("name", 136, 18, 190, 20, 18, TEXT, "Left", ""),
        // tier badge
        format!(
            "#tier:color {{ x: 349px; y: 14px; width: 32px; height: 28px; color: #00000000; ignore_event: true; \
             rounding: Uniform {{ rounding: 6; }} {} }} ",
            bold("t", 0, 0, 32, 28, 16, 0x07080bff, "Center", "")
        ),
        label("win", 421, 18, 72, 20, 18, TEXT, "Left", ""),
        label("sd", 491, 20, 60, 18, 13, DIM, "Left", ""),
        // a bar under the win rate: 40% .. 60%
        rect("bar", 421, 42, 120, 4, SLOT, 2, &rect("fill", 0, 0, 60, 4, ACCENT, 2, "")),
        label("delta", 591, 18, 100, 20, 18, DIM, "Left", ""),
        label("games", 721, 18, 100, 20, 18, TEXT, "Left", ""),
        label("pick", 851, 18, 80, 20, 18, TEXT, "Left", ""),
        label("ban", 951, 18, 80, 20, 18, TEXT, "Left", ""),
        label("presence", 1051, 18, 90, 20, 18, TEXT, "Left", ""),
        // best lanes: up to three lane icons with their win rate
        format!(
            "#lanes:empty {{ x: 1171px; y: 0px; width: 280px; height: 56px; ignore_event: true; {}{}{}{}{}{} }} ",
            image("i0", 0, 19, 18, "asset/base/ui/icons/top", 0xc2c6ceff),
            label("t0", 22, 18, 64, 20, 16, TEXT, "Left", ""),
            image("i1", 92, 19, 18, "asset/base/ui/icons/top", 0xc2c6ceff),
            label("t1", 114, 18, 64, 20, 16, TEXT, "Left", ""),
            image("i2", 184, 19, 18, "asset/base/ui/icons/top", 0xc2c6ceff),
            label("t2", 206, 18, 64, 20, 16, TEXT, "Left", "")
        ),
        image("pi", 1471, 18, 20, "asset/base/ui/icons/up_patch", GOOD),
        label("patch", 1497, 18, 100, 20, 16, DIM, "Left", ""),
    ]
    .concat();
    format!(
        "r{i}:color_icon_button {{ width: 1600px; height: {}px; btn: {{ color: #00000000; }} hover: {{ btn: {{ color: {}; }} }} \
         {cells}#line:color {{ y: {ROW_H}px; width: 100%; height: 1px; color: {}; ignore_event: true; }} }}",
        ROW_H + 1,
        color(0x23253380),
        color(LINE)
    )
}

fn fill_row(ui: &mut impl Ui, i: usize, r: &RowData<'_>) {
    let p = |cell: &str| format!("{LIST}.r{i}.{cell}");
    let c = r.c;
    ui.set_visible(&format!("{LIST}.r{i}"), true);
    ui.set_text(&p("rank"), &(i + 1).to_string());
    ui.set_champion_icon(&p("face.icon"), &c.name, 36.0);
    ui.set_text(&p("name"), &name_ref(&c.name));
    match r.tier {
        Some(t) => {
            ui.set_properties(&p("tier"), &format!("color: {};", color(tier_color(t))));
            ui.set_text(&p("tier.t"), t.as_str());
        }
        None => {
            ui.set_properties(&p("tier"), "color: #00000000;");
            ui.set_text(&p("tier.t"), "");
        }
    }
    ui.set_text(&p("win"), &format!("{:.1}%", r.win * 100.0));
    ui.set_properties(&p("win"), &format!("color: {};", color(r.tier.map_or(TEXT, tier_color))));
    ui.set_text(&p("sd"), &format!("±{:.1}", r.sd));
    let fill = ((r.win - 0.4) / 0.2).clamp(0.02, 1.0) * 120.0;
    ui.set_properties(&p("bar.fill"), &format!("width: {fill:.0}px; color: {};", color(r.tier.map_or(ACCENT, tier_color))));
    match r.delta {
        Some(d) => {
            ui.set_text(&p("delta"), &signed(d));
            ui.set_properties(&p("delta"), &format!("color: {};", color(tone(d))));
        }
        None => {
            ui.set_text(&p("delta"), "-");
            ui.set_properties(&p("delta"), &format!("color: {};", color(DIM)));
        }
    }
    ui.set_text(&p("games"), &format!("{} / {}", r.games, c.window.games));
    ui.set_text(&p("pick"), &format!("{:.1}%", c.pick_rate * 100.0));
    ui.set_text(&p("ban"), &format!("{:.1}%", c.ban_rate * 100.0));
    ui.set_text(&p("presence"), &format!("{:.1}%", c.presence() * 100.0));
    let mut lanes: Vec<(Role, f32)> = Role::ALL
        .iter()
        .filter(|l| c.role_share()[l.index()] >= 0.1)
        .map(|l| (*l, c.role_rate(*l)))
        .collect();
    lanes.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    for k in 0..3 {
        let (icon, text) = (p(&format!("lanes.i{k}")), p(&format!("lanes.t{k}")));
        match lanes.get(k) {
            Some((l, wr)) => {
                ui.set_visible(&icon, true);
                ui.set_properties(&icon, &format!("source: \"{}\";", lane_icon(*l)));
                ui.set_text(&text, &format!("{:.0}%", wr * 100.0));
            }
            None => {
                ui.set_visible(&icon, false);
                ui.set_text(&text, "");
            }
        }
    }
    match &c.last_change {
        Some((v, d)) => {
            ui.set_visible(&p("pi"), true);
            let (source, tint) = if *d > 0 { ("asset/base/ui/icons/up_patch", GOOD) } else { ("asset/base/ui/icons/down_patch", BAD) };
            ui.set_properties(&p("pi"), &format!("source: \"{source}\"; color: {};", color(tint)));
            ui.set_text(&p("patch"), v);
        }
        None => {
            ui.set_visible(&p("pi"), false);
            ui.set_text(&p("patch"), "");
        }
    }
}

/// Icons to fill after spawning (path under the body, champion) and clickable champions.
type Extras = (String, Vec<(String, String)>, Vec<(String, u16)>);

/// A titled list of champions with a number: portraits, names, values, games.
fn champ_list(
    id: &str,
    x: i32,
    y: i32,
    w: u32,
    title: &str,
    items: &[(u16, f32, u32)],
    meta: &Meta,
    icons: &mut Vec<(String, String)>,
    links: &mut Vec<(String, u16)>,
    base: &str,
) -> String {
    let mut inner = bold("title", 20, 12, w - 40, 24, 18, TEXT, "Left", title);
    for (k, (c, v, g)) in items.iter().enumerate() {
        let y0 = 48 + k as i32 * 48;
        let row = format!(
            "{}{}{}{}",
            portrait("face", 20, 4, 40),
            label("name", 70, 14, w - 230, 20, 16, TEXT, "Left", &name_ref(meta.names.name(*c))),
            label("v", w as i32 - 160, 14, 80, 20, 16, tone(*v), "Right", &signed(*v)),
            label("g", w as i32 - 70, 16, 50, 18, 13, DIM, "Right", &format!("{g}")),
        );
        inner.push_str(&hot(&format!("i{k}"), 0, y0, w, 48, &row));
        icons.push((format!("{base}.{id}.i{k}.face.icon"), meta.names.name(*c).to_string()));
        links.push((format!("{base}.{id}.i{k}"), *c));
    }
    rect(id, x, y, w, 48 + 48 * items.len().max(1) as u32 + 12, PANEL, 12, &inner)
}

fn champion_source<U: Ui>(t: &mut Texts<'_, U>, meta: &Meta, tiers: &HashMap<String, Tier>, champ: u16, context: &Context) -> Extras {
    let mut icons = Vec::new();
    let mut links = Vec::new();
    let Some(c) = meta.by_id(champ) else { return (String::new(), icons, links) };
    let tier = tiers.get(&c.name).copied();
    let back = t.get("back", "Back");
    let mut s = String::new();
    s.push_str(&format!(
        "#detail:empty {{ y: 52px; width: 1600px; height: 916px; \
         #back:color_icon_button {{ @\"asset/base/style/main#secondary_button\"; x: 0px; y: 0px; width: 140px; height: 40px; \
         text: {{ font: \"asset/base/font/set/bold\"; text: {}; align_x: Center; align_y: Center; size: 16; }} }} ",
        quote(&format!("← {back}"))
    ));
    // header card
    let delta = c.previous.map(|p| (c.win_rate() - sigmoid(p)) * 100.0);
    let change = match &c.last_change {
        Some((v, d)) => format!("{} {v}", if *d > 0 { "▲" } else { "▼" }),
        None => String::new(),
    };
    let header = format!(
        "{}{}{}{}{}{}{}{}{}",
        portrait("face", 24, 20, 88),
        bold("name", 132, 22, 500, 36, 30, TEXT, "Left", &name_ref(&c.name)),
        tier_badge("tier", 132, 68, tier),
        label("change", 176, 68, 300, 28, 16, c.last_change.as_ref().map_or(DIM, |(_, d)| if *d > 0 { GOOD } else { BAD }), "Left", &change),
        bold("wr", 640, 18, 200, 50, 40, tier.map_or(TEXT, tier_color), "Left", &format!("{:.1}%", c.win_rate() * 100.0)),
        label("wrl", 640, 70, 220, 24, 14, DIM, "Left", &t.get("col_win", "Power Win Rate")),
        label("delta", 860, 30, 160, 30, 22, delta.map_or(DIM, tone), "Left", &delta.map_or("-".into(), signed)),
        label("deltal", 860, 70, 180, 24, 14, DIM, "Left", &t.get("col_delta", "vs Last Patch")),
        [
            stat("games", 1060, &format!("{} / {}", c.current.games, c.window.games), &t.get("col_games", "Games")),
            stat("pick", 1220, &format!("{:.1}%", c.pick_rate * 100.0), &t.get("col_pick", "Pick")),
            stat("ban", 1340, &format!("{:.1}%", c.ban_rate * 100.0), &t.get("col_ban", "Ban")),
            stat("presence", 1460, &format!("{:.1}%", c.presence() * 100.0), &t.get("col_presence", "Contest Rate")),
        ]
        .concat()
    );
    s.push_str(&rect("head", 0, 52, 1600, 128, PANEL, 12, &header));
    icons.push(("detail.head.face.icon".into(), c.name.clone()));
    // lanes
    let mut lanes = bold("title", 20, 10, 400, 24, 18, TEXT, "Left", &t.get("by_lane", "By Lane"));
    let share = c.role_share();
    for (k, r) in Role::ALL.iter().enumerate() {
        let e = c.roles[r.index()];
        let x = 20 + k as i32 * 314;
        let wr = c.role_rate(*r);
        let card = format!(
            "{}{}{}{}{}",
            image("icon", 16, 16, 24, lane_icon(*r), 0xc2c6ceff),
            label("lane", 48, 14, 200, 28, 16, DIM, "Left", &lane_text(*r)),
            bold("wr", 16, 46, 140, 34, 26, if e.tally.games > 0 { TEXT } else { DIM }, "Left", &if e.tally.games > 0 { format!("{:.1}%", wr * 100.0) } else { "-".into() }),
            label("g", 160, 52, 120, 24, 14, DIM, "Right", &format!("{}g · {:.0}%", e.tally.games, share[r.index()] * 100.0)),
            rect("bar", 16, 86, 268, 4, SLOT, 2, &rect("fill", 0, 0, ((share[r.index()] * 268.0) as u32).max(2), 4, ACCENT, 2, "")),
        );
        lanes.push_str(&rect(&format!("l{k}"), x, 42, 300, 100, SLOT, 10, &card));
    }
    s.push_str(&rect("lanes", 0, 192, 1600, 156, PANEL, 12, &lanes));
    // team-mates and matchups
    let mut with: Vec<(u16, f32, u32)> = meta
        .synergy
        .iter()
        .filter(|((a, b), e)| (*a == champ || *b == champ) && e.tally.games >= 2)
        .map(|((a, b), e)| (if *a == champ { *b } else { *a }, pts(e.value), e.tally.games))
        .collect();
    with.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut against: Vec<(u16, f32, u32)> = meta
        .counter
        .keys()
        .filter(|(a, b)| *a == champ || *b == champ)
        .map(|(a, b)| {
            let other = if *a == champ { *b } else { *a };
            let e: Effect = meta.counter(champ, other);
            (other, pts(e.value), e.tally.games)
        })
        .filter(|x| x.2 >= 2)
        .collect();
    against.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let strong: Vec<_> = against.iter().filter(|x| x.1 > 0.0).take(6).copied().collect();
    let weak: Vec<_> = against.iter().rev().filter(|x| x.1 < 0.0).take(6).copied().collect();
    let best: Vec<_> = with.iter().filter(|x| x.1 > 0.0).take(6).copied().collect();
    s.push_str(&champ_list("with", 0, 360, 520, &t.get("best_with", "Best Partners"), &best, meta, &mut icons, &mut links, "detail"));
    s.push_str(&champ_list("strong", 540, 360, 520, &t.get("strong_against", "Edge Over"), &strong, meta, &mut icons, &mut links, "detail"));
    s.push_str(&champ_list("weak", 1080, 360, 520, &t.get("weak_against", "Edge Against"), &weak, meta, &mut icons, &mut links, "detail"));
    // patch history and best players
    let mut history = bold("title", 20, 10, 400, 24, 18, TEXT, "Left", &t.get("patch_history", "Patch History"));
    let n = c.history.len().max(1);
    let bar_w = ((760 / n) as u32).clamp(24, 80);
    for (k, ((strength, tally), version)) in c.history.iter().zip(&meta.versions).enumerate() {
        let wr = sigmoid(*strength);
        let h = (((wr - 0.35) / 0.3).clamp(0.03, 1.0) * 120.0) as u32;
        let x = 20 + k as i32 * (bar_w as i32 + 8);
        let changed = c.last_change.as_ref().is_some_and(|(v, _)| v == version);
        let fill = if wr >= 0.5 { GOOD } else { BAD };
        history.push_str(&format!(
            "{}{}{}",
            rect(&format!("b{k}"), x, 172 - h as i32, bar_w, h, fill, 4, ""),
            label(&format!("w{k}"), x - 10, 150 - h as i32, bar_w + 20, 18, 12, TEXT, "Center", &format!("{:.0}%", wr * 100.0)),
            label(&format!("v{k}"), x - 10, 176, bar_w + 20, 18, 11, if changed { ACCENT } else { DIM }, "Center", &format!("{version} ({})", tally.games)),
        ));
    }
    let mut players: Vec<(u32, Effect)> = meta.mastery.iter().filter(|((_, ch), e)| *ch == champ && e.tally.games >= 2).map(|((a, _), e)| (*a, *e)).collect();
    players.sort_by(|a, b| b.1.value.partial_cmp(&a.1.value).unwrap_or(std::cmp::Ordering::Equal));
    let mut top = bold("title", 20, 10, 400, 24, 18, TEXT, "Left", &t.get("top_players", "Best Players"));
    for (k, (a, e)) in players.iter().take(5).enumerate() {
        let y0 = 42 + k as i32 * 30;
        top.push_str(&label(&format!("n{k}"), 20, y0, 300, 26, 16, TEXT, "Left", &context.athletes.get(a).cloned().unwrap_or_else(|| format!("#{a}"))));
        top.push_str(&label(&format!("v{k}"), 330, y0, 90, 26, 16, tone(pts(e.value)), "Right", &signed(pts(e.value))));
        top.push_str(&label(&format!("g{k}"), 430, y0, 120, 26, 13, DIM, "Right", &format!("{}g · {:.0}%", e.tally.games, e.tally.rate().unwrap_or(0.0) * 100.0)));
    }
    let lists_h: i32 = 48 + 48 * 6 + 12;
    let y_bottom = 360 + lists_h + 12;
    s.push_str(&rect("hist", 0, y_bottom, 1000, 204, PANEL, 12, &history));
    s.push_str(&rect("players", 1020, y_bottom, 580, 204, PANEL, 12, &top));
    s.push_str("} ");
    (s, icons, links)
}

fn stat(id: &str, x: i32, value: &str, caption: &str) -> String {
    format!(
        "{}{}",
        bold(id, x, 30, 120, 30, 22, TEXT, "Left", value),
        label(&format!("{id}l"), x, 70, 140, 24, 14, DIM, "Left", caption)
    )
}

fn pairs_source<U: Ui>(t: &mut Texts<'_, U>, meta: &Meta) -> Extras {
    let mut icons = Vec::new();
    let mut s = String::new();
    let mut duos: Vec<(&(u16, u16), &Effect)> = meta.synergy.iter().filter(|(_, e)| e.tally.games >= 3).collect();
    duos.sort_by(|a, b| b.1.value.partial_cmp(&a.1.value).unwrap_or(std::cmp::Ordering::Equal));
    let mut edges: Vec<((u16, u16), Effect)> = meta
        .counter
        .keys()
        .flat_map(|(a, b)| [(*a, *b), (*b, *a)])
        .map(|(a, b)| ((a, b), meta.counter(a, b)))
        .filter(|(_, e)| e.tally.games >= 3 && e.value > 0.0)
        .collect();
    edges.sort_by(|a, b| b.1.value.partial_cmp(&a.1.value).unwrap_or(std::cmp::Ordering::Equal));
    let column = |id: &str, x: i32, title: &str, joiner: &str, list: Vec<((u16, u16), Effect)>, icons: &mut Vec<(String, String)>| {
        let mut inner = bold("title", 24, 14, 600, 28, 20, TEXT, "Left", title);
        for (k, ((a, b), e)) in list.iter().take(14).enumerate() {
            let y0 = 56 + k as i32 * 56;
            let row = format!(
                "{}{}{}{}{}{}{}",
                portrait("fa", 20, 8, 40),
                label("na", 68, 18, 210, 20, 16, TEXT, "Left", &name_ref(meta.names.name(*a))),
                label("j", 280, 18, 30, 20, 16, DIM, "Center", joiner),
                portrait("fb", 318, 8, 40),
                label("nb", 366, 18, 210, 20, 16, TEXT, "Left", &name_ref(meta.names.name(*b))),
                bold("v", 590, 18, 90, 20, 18, tone(pts(e.value)), "Right", &signed(pts(e.value))),
                label("g", 690, 20, 70, 18, 13, DIM, "Right", &format!("{}g", e.tally.games)),
            );
            inner.push_str(&format!("#i{k}:empty {{ y: {y0}px; width: 780px; height: 56px; {row}#line:color {{ y: 56px; width: 100%; height: 1px; color: {}; ignore_event: true; }} }} ", color(LINE)));
            icons.push((format!("{id}.i{k}.fa.icon"), meta.names.name(*a).to_string()));
            icons.push((format!("{id}.i{k}.fb.icon"), meta.names.name(*b).to_string()));
        }
        rect(id, x, 52, 790, 876, PANEL, 12, &inner)
    };
    let duo_list: Vec<((u16, u16), Effect)> = duos.iter().map(|(k, e)| (**k, **e)).collect();
    s.push_str(&column("duos", 0, &t.get("duos", "Golden Duos"), "+", duo_list, &mut icons));
    s.push_str(&column("edges", 810, &t.get("matchups", "Matchup Edges"), ">", edges, &mut icons));
    let note = t.get("note_pairs", "Synergy and edge: how many more games (in win-rate points) the pair wins than their own strengths predict.");
    s.push_str(&label("note", 4, 938, 1592, 24, 13, DIM, "Left", &note));
    (s, icons, Vec::new())
}

fn best_for(meta: &Meta, athlete: u32, role: Option<Role>, n: usize) -> Vec<(u16, f32, u32)> {
    let mut pool: Vec<(u16, f32, u32)> = meta
        .mastery
        .iter()
        .filter(|((a, _), e)| *a == athlete && e.tally.games > 0)
        .map(|((_, c), e)| {
            let base = match role {
                Some(r) => crate::advisor::strength_in(meta, *c, r),
                None => meta.by_id(*c).map_or(0.0, |x| x.strength),
            };
            (*c, base + e.value, e.tally.games)
        })
        .collect();
    pool.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    pool.truncate(n);
    pool
}

fn players_source<U: Ui>(t: &mut Texts<'_, U>, meta: &Meta, context: &Context, opponent: Option<&str>) -> Extras {
    let mut icons = Vec::new();
    let mut s = String::new();
    let own_key = team_key(&context.team_name);
    let teams: [(&str, String, Option<String>); 2] = [
        ("mine", t.get("your_players", "Your Players"), Some(own_key)),
        ("theirs", t.get("opponent", "Opponent"), opponent.map(str::to_string)),
    ];
    let own_caption = t.get("own", "Own Strength");
    let no_roster = t.get("no_roster", "No line-up seen yet.");
    for (k, (id, title, team)) in teams.iter().enumerate() {
        let x = k as i32 * 810;
        let roster = team.as_ref().and_then(|tm| context.rosters.get(tm)).cloned().unwrap_or_default();
        let name = team.as_ref().and_then(|tm| context.team_labels.get(tm)).cloned().unwrap_or_default();
        let mut inner = bold("title", 24, 14, 740, 28, 20, TEXT, "Left", &if name.is_empty() { title.clone() } else { format!("{title} · {name}") });
        if roster.is_empty() {
            inner.push_str(&label("none", 24, 60, 740, 24, 16, DIM, "Left", &no_roster));
        }
        let mut sorted = roster.clone();
        sorted.sort_by_key(|(_, r)| r.map_or(9, |r| r.index()));
        for (p, (a, role)) in sorted.iter().enumerate() {
            let y0 = 56 + p as i32 * 160;
            let skill = meta.athletes.get(a).copied().unwrap_or_default();
            let mut card = String::new();
            if let Some(r) = role {
                card.push_str(&image("lane", 16, 16, 22, lane_icon(*r), 0xc2c6ceff));
            }
            card.push_str(&bold("name", 46, 12, 400, 28, 20, TEXT, "Left", &context.athletes.get(a).cloned().unwrap_or_else(|| format!("#{a}"))));
            card.push_str(&label("own", 470, 14, 200, 24, 14, DIM, "Right", &own_caption));
            card.push_str(&bold("ownv", 680, 12, 60, 28, 20, tone(pts(skill.value)), "Right", &signed(pts(skill.value))));
            for (j, (c, v, g)) in best_for(meta, *a, *role, 5).iter().enumerate() {
                let cx = 16 + j as i32 * 146;
                card.push_str(&portrait(&format!("c{j}"), cx, 52, 56));
                card.push_str(&bold(&format!("v{j}"), cx + 62, 56, 80, 24, 18, TEXT, "Left", &format!("{:.0}%", sigmoid(*v) * 100.0)));
                card.push_str(&label(&format!("g{j}"), cx + 62, 82, 80, 20, 12, DIM, "Left", &format!("{g}g")));
                icons.push((format!("{id}.p{p}.c{j}.icon"), meta.names.name(*c).to_string()));
            }
            inner.push_str(&rect(&format!("p{p}"), 16, y0, 758, 148, SLOT, 10, &card));
        }
        s.push_str(&rect(id, x, 52, 790, 916, PANEL, 12, &inner));
    }
    (s, icons, Vec::new())
}

fn model_source<U: Ui>(t: &mut Texts<'_, U>, meta: &Meta, context: &Context) -> String {
    let card = |id: &str, x: i32, value: &str, caption: &str| {
        rect(id, x, 52, 380, 150, PANEL, 12, &format!(
            "{}{}",
            bold("v", 24, 24, 330, 56, 44, TEXT, "Left", value),
            label("c", 24, 96, 330, 36, 16, DIM, "Left", caption)
        ))
    };
    let bt = context.backtest;
    let mut s = String::new();
    s.push_str(&card("acc", 0, &bt.map_or("—".into(), |b| format!("{:.0}%", b.accuracy * 100.0)), &t.get("accuracy", "Favourite Won")));
    s.push_str(&card("brier", 406, &bt.map_or("—".into(), |b| format!("{:.3} / {:.3}", b.brier, b.coin_brier)), &t.get("brier", "Brier Score")));
    s.push_str(&card("games", 812, &bt.map_or("—".into(), |b| b.games.to_string()), &t.get("checked_games", "Held-out Matches")));
    s.push_str(&card("side", 1218, &signed(pts(meta.side)), &t.get("blue_side", "Blue Side Edge")));
    let info = format!(
        "{}{}{}",
        bold("m", 24, 20, 1500, 30, 20, TEXT, "Left", &format!("{} · {} ({}: {})", meta.current, meta.versions.join(" · "), t.get("this_patch", "This Patch"), meta.current_matches)),
        label("n", 24, 60, 1500, 28, 16, DIM, "Left", &format!("{}: {} + {} solo", t.get("matches", "Matches"), meta.matches, meta.solo_matches)),
        label("note", 24, 96, 1500, 28, 16, DIM, "Left", &t.get("note_model", "Fitted on all but the newest matches, then scored on those it never saw.")),
    );
    s.push_str(&rect("info", 0, 222, 1600, 140, PANEL, 12, &info));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::tests::{simulate, NAMES};
    use crate::meta::{build, Inputs, Settings};
    use crate::ui::tests::FakeUi;
    use crate::ui::take_clicks;

    fn snapshot() -> Snapshot {
        let (names, games) = simulate(1200, "1.1", |n| match n {
            "c" => 0.5,
            "l" => -0.5,
            _ => 0.0,
        }, 6);
        let champions: Vec<String> = NAMES.iter().map(|s| s.to_string()).collect();
        let meta = build(
            &Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.1", warm: None },
            &Settings::default(),
        );
        Snapshot { meta, damage: HashMap::new() }
    }

    fn management() -> FakeUi {
        let mut ui = FakeUi::default();
        ui.add("main", "main_ui");
        ui.add("main.top.left", "color");
        ui.add("main.top.left.etc_category", "empty");
        ui.add("main.top.left.etc_category.statistics", "main_left_button");
        ui.add("main.top.left.house_category.house", "main_left_button");
        ui.add(RIGHT, "empty");
        ui
    }

    fn frame(page: &mut MetaPage, ui: &mut FakeUi, n: u64, snapshot: &Snapshot, context: &Context, screen: &str) {
        let clicks = take_clicks();
        let keys = ui.keys.clone();
        page.tick(ui, &Frame { frame: n, snapshot: Some(snapshot), context, opponent: Some("rivals"), screen, clicks: &clicks, keys: &keys });
    }

    #[test]
    fn opens_from_the_menu_sorts_and_shows_a_champion() {
        let _serial = crate::tests::serial();
        let _ = take_clicks();
        let s = snapshot();
        let mut context = Context { team_name: "Mods FC".into(), ..Default::default() };
        context.rosters.insert("mods fc".into(), (1..=5).map(|a| (a, Some(Role::ALL[a as usize - 1]))).collect());
        let mut ui = management();
        ui.texts.push("#asset/base/text/ui?patch_meta.menu".into());
        let mut page = MetaPage::default();
        frame(&mut page, &mut ui, 1, &s, &context, "Main/Home");
        assert!(ui.exists(&format!("{NAV}.button.text")));
        assert!(!ui.spawned[0].1.contains("main_left_button"), "the game's menu runner panics on entries it does not own");
        assert!(ui.spawned[0].1.contains("#asset/base/text/ui?patch_meta.menu"), "the merged text when the game has it");
        assert!(!ui.spawned[0].1.contains("Meta Analysis"));
        assert!(!page.is_open());

        // open: the page over the right-hand content, the table filled over a few frames
        ui.click(&format!("{NAV}.button"));
        for n in 2..8 {
            frame(&mut page, &mut ui, n, &s, &context, "Main/Home");
        }
        assert!(page.is_open() && ui.exists(SCREEN));
        assert_eq!(ui.visible(RIGHT), Some(false));
        assert!(ui.spawned.iter().any(|(_, src)| src.contains("Champions")), "English when the merge is missing");
        let rows = page.order.len();
        assert_eq!(rows, 12);
        assert!(ui.exists(&format!("{LIST}.r11.name")));
        let first = ui.text(&format!("{LIST}.r0.name")).unwrap();
        assert_eq!(first, name_ref("c"), "the strongest first");
        assert_eq!(ui.icons.get(&format!("{LIST}.r0.face.icon")).map(String::as_str), Some("c"));

        // sort by pick rate, then by it again (ascending)
        ui.click(&format!("{BODY}.data.header.pick"));
        frame(&mut page, &mut ui, 9, &s, &context, "Main/Home");
        let picks: Vec<f32> = (0..rows)
            .map(|i| ui.text(&format!("{LIST}.r{i}.pick")).unwrap().trim_end_matches('%').parse().unwrap())
            .collect();
        assert!(picks.windows(2).all(|w| w[0] >= w[1]), "{picks:?}");
        let spawned = ui.spawned.len();
        ui.click(&format!("{BODY}.data.header.pick"));
        frame(&mut page, &mut ui, 10, &s, &context, "Main/Home");
        let picks: Vec<f32> = (0..rows)
            .map(|i| ui.text(&format!("{LIST}.r{i}.pick")).unwrap().trim_end_matches('%').parse().unwrap())
            .collect();
        assert!(picks.windows(2).all(|w| w[0] <= w[1]), "{picks:?}");
        assert_eq!(ui.spawned.len(), spawned, "sorting refills, it does not rebuild");

        // a lane: the table shows that lane's numbers
        ui.click(&format!("{BODY}.lanes.mid"));
        frame(&mut page, &mut ui, 11, &s, &context, "Main/Home");
        assert_eq!(page.lane, Some(Role::Mid));

        // a row opens the champion; a team-mate there opens that one; back
        let c = page.order[0];
        ui.click(&format!("{LIST}.r0"));
        frame(&mut page, &mut ui, 12, &s, &context, "Main/Home");
        assert_eq!(page.view, View::Champion(c));
        assert!(ui.exists(&format!("{BODY}.detail.head.face")));
        assert!(ui.exists(&format!("{BODY}.detail.lanes.l4.wr")));
        if let Some((path, other)) = page.links.iter().next().map(|(p, c)| (p.clone(), *c)) {
            ui.click(&path);
            frame(&mut page, &mut ui, 13, &s, &context, "Main/Home");
            assert_eq!(page.view, View::Champion(other));
        }
        ui.click(&format!("{BODY}.detail.back"));
        frame(&mut page, &mut ui, 14, &s, &context, "Main/Home");
        assert_eq!(page.view, View::Champions);

        // the other tabs
        for (tab, view) in [("pairs", View::Pairs), ("players", View::Players), ("model", View::Model)] {
            ui.click(&format!("{BODY}.tabs.{tab}"));
            frame(&mut page, &mut ui, 15, &s, &context, "Main/Home");
            assert_eq!(page.view, view);
            assert!(ui.exists(BODY));
            if view == View::Players {
                assert!(ui.exists(&format!("{BODY}.mine.p0.c0.icon")), "the player's line-up with champions");
                assert!(ui.icons.contains_key(&format!("{BODY}.mine.p0.c0.icon")));
            }
        }

        // one of the game's menu entries (another tab) closes it and gives the content back
        assert!(!ui.handlers.iter().any(|h| h.starts_with(LEFT_MENU)), "nothing attached to the game's menu");
        frame(&mut page, &mut ui, 16, &s, &context, "Main/Statistics");
        assert!(!page.is_open() && !ui.exists(PAGE));
        assert_eq!(ui.visible(RIGHT), Some(true));

        // Esc and another screen close it too
        ui.click(&format!("{NAV}.button"));
        frame(&mut page, &mut ui, 17, &s, &context, "Main/Statistics");
        assert!(page.is_open());
        ui.keys = vec!["Escape".into()];
        frame(&mut page, &mut ui, 18, &s, &context, "Main/Statistics");
        ui.keys.clear();
        assert!(!page.is_open());
        ui.click(&format!("{NAV}.button"));
        frame(&mut page, &mut ui, 19, &s, &context, "Main/Statistics");
        frame(&mut page, &mut ui, 20, &s, &context, "Main/Home");
        assert!(!page.is_open() && !ui.exists(PAGE));
    }
}
