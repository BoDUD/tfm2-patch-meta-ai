//! The ban/pick screen overlay:
//!
//! - **win chance** (bottom left, under the player's picks): the model's probability that the
//!   player's line-up beats the enemy's as picked so far, with a bar;
//! - **advice** (bottom right): the best picks and the best ban for the player right now, each
//!   with what it is worth and why (lane, synergy, matchups, the player's mastery);
//! - **grid values**: on every champion card, a value chip at the foot of the portrait - an
//!   arrow and what picking it is worth to the player now, in win-rate points, with a stripe in
//!   its colour;
//! - **enemy lanes**: on each enemy pick, the lane icon it most likely plays and a five-step bar
//!   of how sure that is - from the champions' lane history, so it works in every language.
//!
//! What the game shows (seen in its UI tree): the grid `main.champions.contents` holds one
//! `banpick_champion_slot` per champion, **named by the champion id**; a card's `blue` / `red`
//! badge is visible once that side picked it, with the pick number in `<badge>.text`, and its
//! `ban` icon once it is banned. Team names sit in `main.bottom.<side>_side.name` with the
//! league rank appended ("Samsung Galaxy #1"). Labels read back the raw text they were given,
//! so champion names are written as the game's own name reference and the game shows them in
//! its language. Both bottom corners of the screen are empty in the game's layout.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use super::names::{name_ref, NameBook};
use super::{color, team_key, Ui};
use crate::advisor::{self, PickValue};
use crate::diag;
use crate::glm::sigmoid;
use crate::history::Role;
use crate::shared::Snapshot;

pub const GRID: &str = "main.champions.contents";
const OVERLAY: &str = "main.pma";
const TAG: &str = "pma_tag";
const LANE_TAG: &str = "pma_lane";
const BLUE_NAME: &str = "main.bottom.blue_side.name";
const RED_NAME: &str = "main.bottom.red_side.name";
const PICK_COLUMNS: [&str; 2] = ["main.blue_picks", "main.red_picks"];
/// Frames between two reads of the grid (131 cards, three node reads each).
const READ_EVERY: u64 = 10;
/// Matches the model needs before its advice is shown.
const MIN_MATCHES: u32 = 10;

const BLUE: u32 = 0x5b73ffff;
const RED: u32 = 0xef6471ff;
const GOOD: u32 = 0x4cc38aff;
const BAD: u32 = 0xef6471ff;
const DIM: u32 = 0xa3a9b6ff;
const TEXT: u32 = 0xe8e8e8ff;
const PANEL: u32 = 0x161721f0;

/// What the overlay shows besides the model.
pub struct View<'a> {
    pub team_name: &'a str,
    /// Each team's players and lanes, by [`team_key`].
    pub rosters: &'a HashMap<String, Vec<(u32, Option<Role>)>>,
    pub grid_values: bool,
    pub lane_tags: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub path: String,
    pub champ: Option<String>,
    /// Picked by blue / red, with the pick number on the badge.
    pub blue: Option<u32>,
    pub red: Option<u32>,
    pub banned: bool,
}

#[derive(Default)]
pub struct DraftScreen {
    /// The other team on the last ban/pick screen ([`team_key`]), for scouting.
    pub enemy_team: Option<String>,
    on: bool,
    next_read: u64,
    /// The draft and model the overlay was last drawn for.
    drawn: Option<u64>,
    tagged: HashMap<String, bool>,
    reported: bool,
}

fn visible(ui: &impl Ui, path: &str) -> bool {
    ui.visible(path) == Some(true)
}

/// The champion on a card: its node name (the champion id), else a champion id in its runner
/// state, else its name label.
fn card_champion(ui: &impl Ui, child: &str, path: &str, known: &dyn Fn(&str) -> bool, names: &NameBook) -> Option<String> {
    if known(child) {
        return Some(child.to_string());
    }
    if let Some(state) = ui.state(path).and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) {
        for key in ["champion", "champion_name", "name", "id", "key"] {
            if let Some(v) = crate::records::find_key(&state, key).and_then(|v| v.as_str()) {
                if known(v) {
                    return Some(v.to_string());
                }
            }
        }
    }
    let label = ui.text(&format!("{path}.name")).unwrap_or_default();
    names.lookup(&label).map(str::to_string).filter(|c| known(c))
}

pub fn read_grid(ui: &impl Ui, known: &dyn Fn(&str) -> bool, names: &NameBook) -> Vec<Card> {
    ui.children(GRID)
        .into_iter()
        .map(|child| {
            let path = format!("{GRID}.{child}");
            let badge = |side: &str| -> Option<u32> {
                let p = format!("{path}.{side}");
                visible(ui, &p).then(|| ui.text(&format!("{p}.text")).and_then(|t| t.trim().parse().ok()).unwrap_or(0))
            };
            Card {
                champ: card_champion(ui, &child, &path, known, names),
                blue: badge("blue"),
                red: badge("red"),
                banned: visible(ui, &format!("{path}.ban")),
                path,
            }
        })
        .collect()
}

/// 0 = the player is blue, 1 = red (from the team names at the bottom of the screen).
pub fn player_side(ui: &impl Ui, team_name: &str) -> Option<usize> {
    let team = team_key(team_name);
    if team.is_empty() {
        return None;
    }
    [BLUE_NAME, RED_NAME].iter().position(|p| ui.text(p).is_some_and(|t| team_key(&t) == team))
}

fn points(v: f32) -> f32 {
    (sigmoid(v) - 0.5) * 100.0
}

fn signed(v: f32) -> String {
    format!("{}{:.1}", if v >= 0.0 { "+" } else { "" }, v)
}

#[allow(clippy::too_many_arguments)]
fn label(id: &str, x: u32, y: u32, w: u32, h: u32, size: u32, c: u32, align: &str) -> String {
    format!(
        "#{id}:label {{ @\"asset/base/style/main#label\"; x: {x}px; y: {y}px; width: {w}px; height: {h}px; \
         size: {size}; color: {}; align_x: {align}; align_y: Center; text: \"\"; ignore_event: true; }} ",
        color(c)
    )
}

/// Advice rows: what (pick / ban), the champion's name, its value, why.
const ADVICE_ROWS: usize = 3;

fn overlay_source() -> String {
    let panel = |id: &str, x: u32, inner: String| {
        format!(
            "#{id}:color {{ x: {x}px; y: 988px; width: 460px; height: 84px; color: {}; ignore_event: true; \
             rounding: Uniform {{ rounding: 10; }} {inner}}} ",
            color(PANEL)
        )
    };
    let win = format!(
        "{}{}{}#back:color {{ x: 12px; y: 64px; width: 436px; height: 8px; color: {}; ignore_event: true; \
         rounding: Uniform {{ rounding: 4; }} #fill:color {{ width: 50%; height: 100%; color: {}; ignore_event: true; \
         rounding: Uniform {{ rounding: 4; }} }} }} ",
        label("title", 12, 4, 300, 22, 13, DIM, "Left"),
        label("value", 12, 26, 200, 34, 26, TEXT, "Left"),
        label("detail", 200, 30, 248, 26, 13, DIM, "Right"),
        color(RED),
        color(BLUE)
    );
    let mut advice = String::new();
    for r in 0..ADVICE_ROWS {
        let y = 4 + r as u32 * 26;
        advice.push_str(&label(&format!("k{r}"), 12, y, 40, 24, 13, DIM, "Left"));
        advice.push_str(&label(&format!("c{r}"), 56, y, 150, 24, 14, TEXT, "Left"));
        advice.push_str(&label(&format!("v{r}"), 208, y, 56, 24, 14, GOOD, "Right"));
        advice.push_str(&label(&format!("w{r}"), 274, y, 178, 24, 12, DIM, "Left"));
    }
    format!(
        "pma:empty {{ width: 100%; height: 100%; ignore_event: true; {}{}}}",
        panel("win", 20, win),
        panel("advice", 1440, advice)
    )
}

/// The value chip at the foot of a card's portrait: a coloured stripe and an arrow with the value.
fn chip_source() -> String {
    format!(
        "{TAG}:color {{ x: 6px; y: 62px; width: 58px; height: 20px; color: #07080be0; ignore_event: true; \
         rounding: Uniform {{ rounding: 4; }} \
         #stripe:color {{ width: 3px; height: 100%; color: {}; ignore_event: true; }} \
         #text:label {{ @\"asset/base/style/main#bold_label\"; x: 7px; width: 49px; height: 100%; size: 12; \
         align_x: Left; align_y: Center; text: \"\"; ignore_event: true; }} }}",
        color(DIM)
    )
}

/// Steps of the lane read's confidence bar.
const LANE_STEPS: usize = 5;

/// The lane read on an enemy pick: the lane icon and a five-step confidence bar.
fn lane_source() -> String {
    let mut steps = String::new();
    for k in 0..LANE_STEPS {
        steps.push_str(&format!(
            "#s{k}:color {{ x: {}px; y: 9px; width: 7px; height: 5px; color: #4a4c56ff; ignore_event: true; \
             rounding: Uniform {{ rounding: 2; }} }} ",
            30 + k * 9
        ));
    }
    format!(
        "{LANE_TAG}:color {{ anchor_x: 1; pivot_x: 1; x: -6px; y: 6px; width: {}px; height: 24px; color: #07080be0; \
         ignore_event: true; rounding: Uniform {{ rounding: 6; }} \
         #icon:image {{ x: 5px; y: 3px; width: 18px; height: 18px; color: #e8e8e8ff; ignore_event: true; \
         source: \"asset/base/ui/icons/top\"; }} {steps}}}",
        30 + LANE_STEPS * 9 + 2
    )
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

/// Why a pick or ban is worth what it is, in a few words.
fn why(v: &PickValue) -> String {
    let mut parts = Vec::new();
    if let Some(r) = v.role {
        parts.push(r.name().to_string());
    }
    if v.synergy.abs() >= 0.02 {
        parts.push(format!("syn {}", signed(points(v.synergy))));
    }
    if v.counter.abs() >= 0.02 {
        parts.push(format!("vs {}", signed(points(v.counter))));
    }
    if v.mastery.abs() >= 0.02 {
        parts.push(format!("player {}", signed(points(v.mastery))));
    }
    if v.balance > 0.0 {
        parts.push("one-sided".to_string());
    }
    parts.join(" · ")
}

impl DraftScreen {
    /// One frame on a screen that may be the ban/pick screen.
    pub fn tick(&mut self, ui: &mut impl Ui, frame: u64, snapshot: Option<&Arc<Snapshot>>, names: &NameBook, view: &View<'_>) {
        if !ui.exists(GRID) {
            if self.on {
                self.on = false;
                self.drawn = None;
                self.tagged.clear();
            }
            return;
        }
        if !self.on {
            self.on = true;
            diag::log("[ui] ban/pick screen");
        }
        if frame < self.next_read {
            return;
        }
        self.next_read = frame + READ_EVERY;
        let side = player_side(ui, view.team_name);
        self.enemy_team = ui.text([BLUE_NAME, RED_NAME][1 - side.unwrap_or(0)]).map(|t| team_key(&t)).filter(|t| !t.is_empty());

        // too little to go on: say so, nothing else
        let ready = snapshot.filter(|s| s.meta.matches + s.meta.solo_matches >= MIN_MATCHES);
        let Some(snapshot) = ready else {
            let matches = snapshot.map_or(0, |s| s.meta.matches + s.meta.solo_matches);
            if self.drawn == Some(matches as u64) && ui.exists(OVERLAY) {
                return;
            }
            if !ui.exists(OVERLAY) && !ui.spawn("main", &overlay_source()) {
                return;
            }
            self.drawn = Some(matches as u64);
            ui.set_text(&format!("{OVERLAY}.win.title"), "Patch Meta");
            ui.set_text(&format!("{OVERLAY}.win.value"), "—");
            ui.set_text(&format!("{OVERLAY}.win.detail"), &format!("{matches}/{MIN_MATCHES} matches"));
            ui.set_text(&format!("{OVERLAY}.advice.k0"), "");
            ui.set_text(&format!("{OVERLAY}.advice.w0"), "");
            ui.set_text(&format!("{OVERLAY}.advice.c0"), "Not enough matches yet");
            ui.set_text(&format!("{OVERLAY}.advice.c1"), "比赛数据还不够");
            return;
        };
        let meta = &snapshot.meta;
        let known = |c: &str| meta.names.get(c).is_some();
        let cards = read_grid(ui, &known, names);
        if !self.reported {
            self.reported = true;
            let named = cards.iter().filter(|c| c.champ.is_some()).count();
            diag::log(&format!(
                "[ui] grid: {} cards, {named} matched to a champion; player side {}",
                cards.len(),
                match side {
                    Some(0) => "blue",
                    Some(1) => "red",
                    _ => "unknown (assuming blue)",
                }
            ));
        }
        let side = side.unwrap_or(0);

        // what changed since the last drawing: the draft, the model, or our nodes went missing
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for c in &cards {
            (&c.champ, c.blue, c.red, c.banned).hash(&mut h);
        }
        (side, Arc::as_ptr(snapshot) as usize).hash(&mut h);
        let key = h.finish() | 1 << 63;
        // our nodes still there? the overlay, and the chips of the first and last card (a rebuilt
        // grid loses all of them; asking every card each time would be 131 more calls)
        let tagged: Vec<&Card> = cards.iter().filter(|c| c.champ.is_some()).collect();
        let healthy = ui.exists(OVERLAY)
            && (!view.grid_values
                || [tagged.first(), tagged.last()].into_iter().flatten().all(|c| ui.exists(&format!("{}.{TAG}", c.path))));
        if self.drawn == Some(key) && healthy {
            return;
        }
        if !ui.exists(OVERLAY) && !ui.spawn("main", &overlay_source()) {
            diag::log_once("ui-overlay", "[ui] could not add the overlay to the ban/pick screen");
            return;
        }
        self.drawn = Some(key);
        // the players on each side, by the team names at the bottom of the screen
        let roster_of = |path: &str| -> Vec<(u32, Option<Role>)> {
            ui.text(path).and_then(|t| view.rosters.get(&team_key(&t)).cloned()).unwrap_or_default()
        };
        let rosters = [roster_of(BLUE_NAME), roster_of(RED_NAME)];
        self.draw(ui, snapshot, &cards, side, &rosters, view);
    }

    fn draw(
        &mut self,
        ui: &mut impl Ui,
        snapshot: &Snapshot,
        cards: &[Card],
        side: usize,
        rosters: &[Vec<(u32, Option<Role>)>; 2],
        view: &View<'_>,
    ) {
        let (players, enemy_players) = (&rosters[side], &rosters[1 - side]);
        let meta = &snapshot.meta;
        let id = |c: &Card| c.champ.as_deref().and_then(|n| meta.names.get(n));
        let picks = |blue: bool| -> Vec<u16> {
            let mut list: Vec<(u32, u16)> = cards
                .iter()
                .filter_map(|c| Some((if blue { c.blue? } else { c.red? }, id(c)?)))
                .collect();
            list.sort();
            list.into_iter().map(|(_, c)| c).collect()
        };
        let (blue, red) = (picks(true), picks(false));
        let (ally, enemy) = if side == 0 { (blue, red) } else { (red, blue) };
        let damage = |c: u16| snapshot.damage_of(c);
        let open: Vec<u16> =
            cards.iter().filter(|c| c.blue.is_none() && c.red.is_none() && !c.banned).filter_map(&id).collect();

        // win chance
        let p = advisor::win_probability(meta, &ally, &enemy);
        let (mine, theirs) = if side == 0 { (BLUE, RED) } else { (RED, BLUE) };
        ui.set_text(&format!("{OVERLAY}.win.title"), &format!("Win chance 胜率 · {} vs {}", ally.len(), enemy.len()));
        ui.set_text(&format!("{OVERLAY}.win.value"), &format!("{:.0}%", p * 100.0));
        ui.set_properties(&format!("{OVERLAY}.win.value"), &format!("color: {};", color(mine)));
        ui.set_text(&format!("{OVERLAY}.win.detail"), &format!("patch {} · {} matches", meta.current, meta.current_matches));
        ui.set_properties(&format!("{OVERLAY}.win.back"), &format!("color: {};", color(theirs)));
        ui.set_properties(
            &format!("{OVERLAY}.win.back.fill"),
            &format!("width: {:.1}%; color: {};", (p * 100.0).clamp(2.0, 98.0), color(mine)),
        );

        // advice: the two best picks and the best ban
        let (ally_open, enemy_open) = (advisor::open_lanes(meta, &ally), advisor::open_lanes(meta, &enemy));
        let mut pick_values: Vec<PickValue> = open
            .iter()
            .map(|c| advisor::pick_value_in(meta, *c, &ally, &enemy, players, &damage, &ally_open))
            .collect();
        pick_values.sort_by(|a, b| b.total.partial_cmp(&a.total).unwrap_or(std::cmp::Ordering::Equal));
        let mut ban_values: Vec<PickValue> = open
            .iter()
            .map(|c| advisor::ban_value_in(meta, *c, &ally, &enemy, enemy_players, &damage, &enemy_open))
            .collect();
        ban_values.sort_by(|a, b| b.total.partial_cmp(&a.total).unwrap_or(std::cmp::Ordering::Equal));
        let rows: [(&str, Option<&PickValue>); ADVICE_ROWS] =
            [("Pick 选", pick_values.first()), ("Alt 备", pick_values.get(1)), ("Ban 禁", ban_values.first())];
        for (r, (what, v)) in rows.iter().enumerate() {
            let row = |part: &str| format!("{OVERLAY}.advice.{part}{r}");
            ui.set_text(&row("k"), what);
            match v {
                Some(v) => {
                    ui.set_text(&row("c"), &name_ref(meta.names.name(v.champ)));
                    let pts = points(v.total);
                    ui.set_text(&row("v"), &signed(pts));
                    ui.set_properties(&row("v"), &format!("color: {};", color(if pts >= 0.0 { GOOD } else { BAD })));
                    ui.set_text(&row("w"), &why(v));
                }
                None => {
                    for part in ["c", "v", "w"] {
                        ui.set_text(&row(part), "");
                    }
                }
            }
        }

        // grid values
        if view.grid_values {
            let by_champ: HashMap<u16, f32> = pick_values.iter().map(|v| (v.champ, v.total)).collect();
            for c in cards.iter().filter(|c| c.champ.is_some()) {
                let tag = format!("{}.{TAG}", c.path);
                if !ui.exists(&tag) && !ui.spawn(&c.path, &chip_source()) {
                    continue;
                }
                let value = id(c).and_then(|x| by_champ.get(&x)).copied();
                let shown = value.is_some();
                if self.tagged.get(&tag) != Some(&shown) || !shown {
                    ui.set_visible(&tag, shown);
                    self.tagged.insert(tag.clone(), shown);
                }
                if let Some(v) = value {
                    let pts = points(v);
                    let (arrow, c) = if pts >= 1.0 {
                        ("▲", GOOD)
                    } else if pts <= -1.0 {
                        ("▼", BAD)
                    } else {
                        ("·", DIM)
                    };
                    ui.set_text(&format!("{tag}.text"), &format!("{arrow}{:.1}", pts.abs()));
                    ui.set_properties(&format!("{tag}.text"), &format!("color: {};", color(c)));
                    ui.set_properties(&format!("{tag}.stripe"), &format!("color: {};", color(c)));
                }
            }
        }

        // enemy lanes, on the enemy's pick slots in pick order
        if view.lane_tags {
            let column = PICK_COLUMNS[1 - side];
            let slots = ui.children(column);
            let probs = advisor::lane_probabilities(meta, &enemy);
            for (i, slot) in slots.iter().enumerate() {
                let path = format!("{column}.{slot}");
                let tag = format!("{path}.{LANE_TAG}");
                let read = probs.get(i).map(|row| {
                    row.iter()
                        .enumerate()
                        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                        .map(|(r, p)| (Role::ALL[r], *p))
                        .unwrap_or((Role::Top, 0.0))
                });
                if !ui.exists(&tag) && (read.is_none() || !ui.spawn(&path, &lane_source())) {
                    continue;
                }
                ui.set_visible(&tag, read.is_some());
                if let Some((lane, p)) = read {
                    ui.set_properties(&format!("{tag}.icon"), &format!("source: \"{}\";", lane_icon(lane)));
                    // 20% is a blind guess among five lanes: the bar starts there
                    let lit = (((p - 0.2) / 0.8).clamp(0.0, 1.0) * LANE_STEPS as f32).ceil() as usize;
                    for k in 0..LANE_STEPS {
                        let c = if k < lit { if lit >= 4 { GOOD } else { 0xf2c14eff } } else { 0x4a4c56ff };
                        ui.set_properties(&format!("{tag}.s{k}"), &format!("color: {};", color(c)));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::tests::{simulate, NAMES};
    use crate::meta::{build, Inputs, Settings};
    use crate::ui::tests::FakeUi;

    /// The screen as the game builds it: cards named by champion id, a rank after team names.
    fn screen() -> FakeUi {
        let mut ui = FakeUi::default();
        ui.add("main", "match_ui");
        ui.add(BLUE_NAME, "label").text = Some("Mods FC #1".into());
        ui.add(RED_NAME, "label").text = Some("Rivals\r #10".into());
        for col in PICK_COLUMNS {
            for i in 0..5 {
                ui.add(&format!("{col}.pick_slot_{i}"), "color_icon_button");
            }
        }
        ui.add(GRID, "empty");
        for n in NAMES {
            let path = format!("{GRID}.{n}");
            ui.add(&path, "banpick_champion_slot");
            ui.add(&format!("{path}.name"), "label").text = Some(name_ref(n));
            for badge in ["blue", "red", "ban"] {
                ui.add(&format!("{path}.{badge}"), "color").visible = false;
                ui.add(&format!("{path}.{badge}.text"), "label");
            }
        }
        ui
    }

    fn pick(ui: &mut FakeUi, champ: &str, side: &str, order: u32) {
        let path = format!("{GRID}.{champ}.{side}");
        ui.nodes.get_mut(&path).unwrap().visible = true;
        ui.nodes.get_mut(&format!("{path}.text")).unwrap().text = Some(order.to_string());
    }

    fn snapshot(games: usize) -> Arc<Snapshot> {
        let (names, games) = simulate(games, "1.1", |n| match n {
            "c" => 0.5,
            "l" => -0.5,
            _ => 0.0,
        }, 4);
        let champions: Vec<String> = NAMES.iter().map(|s| s.to_string()).collect();
        let meta = build(
            &Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.1", warm: None },
            &Settings::default(),
        );
        Arc::new(Snapshot { meta, damage: HashMap::new() })
    }

    #[test]
    fn reads_the_draft_and_draws_the_overlay() {
        let snapshot = snapshot(2000);
        let book = NameBook::default();
        let mut ui = screen();
        pick(&mut ui, "a", "red", 1); // the enemy took "a"
        pick(&mut ui, "b", "blue", 1); // we took "b"
        ui.nodes.get_mut(&format!("{GRID}.e.ban")).unwrap().visible = true;
        let mut rosters = HashMap::new();
        rosters.insert("rivals".to_string(), vec![(9u32, Some(Role::Mid))]);
        let view = View { team_name: "Mods FC", rosters: &rosters, grid_values: true, lane_tags: true };
        let mut screen = DraftScreen::default();
        screen.tick(&mut ui, 0, Some(&snapshot), &book, &view);

        let known = |c: &str| NAMES.contains(&c);
        let cards = read_grid(&ui, &known, &book);
        assert_eq!(cards.len(), 12);
        let card = |n: &str| cards.iter().find(|c| c.champ.as_deref() == Some(n)).unwrap();
        assert_eq!(card("a").red, Some(1));
        assert!(card("e").banned && !card("d").banned);
        assert_eq!(player_side(&ui, "mods fc"), Some(0), "the rank after the name is ignored");
        assert_eq!(screen.enemy_team.as_deref(), Some("rivals"));

        let text = |p: &str| ui.text(p).unwrap_or_default();
        assert!(text("main.pma.win.value").ends_with('%'), "{}", text("main.pma.win.value"));
        // champion names are the game's name references, shown in its language
        assert!(text("main.pma.advice.c0").starts_with("#asset/base/text/champion?description."));
        let advised = [text("main.pma.advice.c0"), text("main.pma.advice.c1")];
        assert!(advised.contains(&name_ref("c")), "the strong champion is advised: {advised:?}");
        assert_eq!(text("main.pma.advice.k2"), "Ban 禁");
        // grid values on open cards only
        assert_eq!(ui.visible(&format!("{GRID}.a.{TAG}")), Some(false), "picked");
        assert_eq!(ui.visible(&format!("{GRID}.e.{TAG}")), Some(false), "banned");
        assert_eq!(ui.visible(&format!("{GRID}.c.{TAG}")), Some(true));
        assert!(text(&format!("{GRID}.c.{TAG}.text")).starts_with('▲'), "c is worth picking");
        assert!(ui.nodes[&format!("{GRID}.c.{TAG}.stripe")].props.iter().any(|p| p.contains(&color(GOOD))));
        // a lane read on the enemy's first pick slot: an icon and a confidence bar
        let lane = format!("main.red_picks.pick_slot_0.{LANE_TAG}");
        assert!(ui.nodes[&format!("{lane}.icon")].props.iter().any(|p| p.contains("asset/base/ui/icons/")));
        assert!(ui.exists(&format!("{lane}.s4")));
        assert!(!ui.exists(&format!("main.red_picks.pick_slot_1.{LANE_TAG}")));

        // nothing changed: nothing redrawn; the game rebuilt the screen: drawn again
        let spawned = ui.spawned.len();
        screen.tick(&mut ui, READ_EVERY, Some(&snapshot), &book, &view);
        assert_eq!(ui.spawned.len(), spawned);
        ui.remove("main.pma");
        screen.tick(&mut ui, 2 * READ_EVERY, Some(&snapshot), &book, &view);
        assert!(ui.exists("main.pma.win.value"));
    }

    #[test]
    fn says_so_when_there_is_too_little_data() {
        let book = NameBook::default();
        let rosters = HashMap::new();
        let view = View { team_name: "Mods FC", rosters: &rosters, grid_values: true, lane_tags: true };
        for snapshot in [None, Some(snapshot(5))] {
            let mut ui = screen();
            let mut screen = DraftScreen::default();
            screen.tick(&mut ui, 0, snapshot.as_ref(), &book, &view);
            assert_eq!(ui.text("main.pma.win.value").as_deref(), Some("—"));
            assert!(ui.text("main.pma.advice.c0").unwrap().contains("Not enough"));
            assert!(!ui.exists(&format!("{GRID}.c.{TAG}")), "no values on the cards");
        }
    }
}
