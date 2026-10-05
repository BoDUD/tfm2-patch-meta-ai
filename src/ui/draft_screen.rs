//! The ban/pick screen overlay:
//!
//! - **win chance** (bottom left, under the player's picks): the model's probability that the
//!   player's line-up beats the enemy's as picked so far, with a bar;
//! - **advice** (bottom right): the best picks and bans for the player right now, each with
//!   what it is worth and why (lane, synergy, matchups);
//! - **grid values**: on every champion card, what picking it is worth to the player now, in
//!   win-rate points;
//! - **enemy lanes**: on each enemy pick, the lane it most likely plays and how sure that is -
//!   from the champions' lane history, so it works in every language.
//!
//! The screen is read from the champion grid: each card says (in its `blue` / `red` badge and
//! `ban` icon) who picked or banned it. A card's champion comes from its runner state when that
//! names one, else from its name label (`names::NameBook`). Both bottom corners of the screen
//! are empty in the game's layout (team names, bans and logos sit in the middle).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use super::names::NameBook;
use super::{color, Ui};
use crate::advisor::{self, PickValue};
use crate::glm::sigmoid;
use crate::history::Role;
use crate::shared::Snapshot;
use crate::diag;

pub const GRID: &str = "main.champions.contents";
const OVERLAY: &str = "main.pma";
const TAG: &str = "pma_tag";
const LANE_TAG: &str = "pma_lane";
const BLUE_NAME: &str = "main.bottom.blue_side.name";
const RED_NAME: &str = "main.bottom.red_side.name";
const PICK_COLUMNS: [&str; 2] = ["main.blue_picks", "main.red_picks"];
const READ_EVERY: u64 = 6;

const BLUE: u32 = 0x5b73ffff;
const RED: u32 = 0xef6471ff;
const GOOD: u32 = 0x4cc38aff;
const BAD: u32 = 0xef6471ff;
const DIM: u32 = 0xa3a9b6ff;
const PANEL: u32 = 0x161721f0;

/// What the overlay shows besides the model.
pub struct View<'a> {
    pub team_name: &'a str,
    /// Each team's players and lanes, by team name (lower case).
    pub rosters: &'a HashMap<String, Vec<(u32, Option<Role>)>>,
    pub grid_values: bool,
    pub lane_tags: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub path: String,
    pub champ: Option<String>,
    /// The name on the card.
    pub label: String,
    /// Picked by blue / red, with the pick number on the badge.
    pub blue: Option<u32>,
    pub red: Option<u32>,
    pub banned: bool,
}

#[derive(Default)]
pub struct DraftScreen {
    /// The other team on the last ban/pick screen (lower case), for scouting.
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

/// The champion on a card: a champion id in its runner state, else its name label.
fn card_champion(ui: &impl Ui, path: &str, label: &str, known: &dyn Fn(&str) -> bool, names: &NameBook) -> Option<String> {
    if let Some(state) = ui.state(path).and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) {
        for key in ["champion", "champion_name", "name", "id", "key"] {
            if let Some(v) = crate::records::find_key(&state, key).and_then(|v| v.as_str()) {
                if known(v) {
                    return Some(v.to_string());
                }
            }
        }
    }
    names.lookup(label).map(str::to_string).filter(|c| known(c))
}

pub fn read_grid(ui: &impl Ui, known: &dyn Fn(&str) -> bool, names: &NameBook) -> Vec<Card> {
    ui.children(GRID)
        .into_iter()
        .map(|child| {
            let path = format!("{GRID}.{child}");
            let label = ui.text(&format!("{path}.name")).unwrap_or_default();
            let badge = |side: &str| -> Option<u32> {
                let p = format!("{path}.{side}");
                visible(ui, &p).then(|| ui.text(&format!("{p}.text")).and_then(|t| t.trim().parse().ok()).unwrap_or(0))
            };
            Card {
                champ: card_champion(ui, &path, &label, known, names),
                blue: badge("blue"),
                red: badge("red"),
                banned: visible(ui, &format!("{path}.ban")),
                label,
                path,
            }
        })
        .collect()
}

/// 0 = the player is blue, 1 = red (from the team names at the bottom of the screen).
pub fn player_side(ui: &impl Ui, team_name: &str) -> Option<usize> {
    let norm = |s: &str| s.trim().to_lowercase();
    let team = norm(team_name);
    if team.is_empty() {
        return None;
    }
    [BLUE_NAME, RED_NAME].iter().position(|p| ui.text(p).is_some_and(|t| norm(&t) == team))
}

fn points(v: f32) -> f32 {
    (sigmoid(v) - 0.5) * 100.0
}

fn signed(v: f32) -> String {
    format!("{}{:.1}", if v >= 0.0 { "+" } else { "" }, v)
}

fn overlay_source() -> String {
    let label = |id: &str, x: u32, y: u32, w: u32, h: u32, size: u32, c: u32, align: &str| {
        format!(
            "#{id}:label {{ @\"asset/base/style/main#label\"; x: {x}px; y: {y}px; width: {w}px; height: {h}px; \
             size: {size}; color: {}; align_x: {align}; align_y: Center; text: \"\"; ignore_event: true; }} ",
            color(c)
        )
    };
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
        label("value", 12, 26, 200, 34, 26, 0xe8e8e8ff, "Left"),
        label("detail", 200, 30, 248, 26, 13, DIM, "Right"),
        color(RED),
        color(BLUE)
    );
    let advice = format!(
        "{}{}{}",
        label("l0", 12, 4, 436, 24, 13, 0xe8e8e8ff, "Left"),
        label("l1", 12, 30, 436, 24, 13, 0xe8e8e8ff, "Left"),
        label("l2", 12, 56, 436, 24, 13, DIM, "Left")
    );
    format!(
        "pma:empty {{ width: 100%; height: 100%; ignore_event: true; {}{}}}",
        panel("win", 20, win),
        panel("advice", 1440, advice)
    )
}

fn tag_source(id: &str, w: u32, x_from_right: bool) -> String {
    let (anchor, x) = if x_from_right { ("anchor_x: 1; pivot_x: 1; ", -6) } else { ("", 6) };
    format!(
        "{id}:color {{ {anchor}x: {x}px; y: 6px; width: {w}px; height: 20px; color: #07080bd0; ignore_event: true; \
         rounding: Uniform {{ rounding: 6; }} #text:label {{ @\"asset/base/style/main#label\"; width: 100%; height: 100%; \
         size: 12; align_x: Center; align_y: Center; text: \"\"; ignore_event: true; }} }}"
    )
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
        let Some(snapshot) = snapshot else { return };
        let meta = &snapshot.meta;
        let known = |c: &str| meta.names.get(c).is_some();
        let cards = read_grid(ui, &known, names);
        let side = player_side(ui, view.team_name);
        if !self.reported {
            self.reported = true;
            let named = cards.iter().filter(|c| c.champ.is_some()).count();
            diag::log(&format!(
                "[ui] grid: {} cards, {named} matched to a champion (e.g. {:?}); player side {}",
                cards.len(),
                cards.iter().take(3).map(|c| (&c.label, &c.champ)).collect::<Vec<_>>(),
                match side {
                    Some(0) => "blue",
                    Some(1) => "red",
                    _ => "unknown (assuming blue)",
                }
            ));
        }
        let side = side.unwrap_or(0);
        self.enemy_team = ui.text([BLUE_NAME, RED_NAME][1 - side]).map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty());

        // what changed since the last drawing: the draft, the model, or our nodes went missing
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for c in &cards {
            (&c.champ, c.blue, c.red, c.banned).hash(&mut h);
        }
        (side, Arc::as_ptr(snapshot) as usize).hash(&mut h);
        let key = h.finish();
        let healthy = ui.exists(OVERLAY) && cards.iter().all(|c| !view.grid_values || ui.exists(&format!("{}.{TAG}", c.path)));
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
            ui.text(path).and_then(|t| view.rosters.get(&t.trim().to_lowercase()).cloned()).unwrap_or_default()
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
        let label_of: HashMap<u16, &str> = cards.iter().filter_map(|c| Some((id(c)?, c.label.as_str()))).collect();
        let label = |c: u16| -> String { label_of.get(&c).map_or_else(|| meta.names.name(c).to_string(), |l| l.to_string()) };
        let damage = |c: u16| snapshot.damage_of(c);
        let open: Vec<&Card> = cards.iter().filter(|c| c.blue.is_none() && c.red.is_none() && !c.banned).collect();

        // win chance
        let p = advisor::win_probability(meta, &ally, &enemy);
        let (mine, theirs) = if side == 0 { (BLUE, RED) } else { (RED, BLUE) };
        ui.set_text(&format!("{OVERLAY}.win.title"), &format!("Win chance 胜率 · {} vs {}", ally.len(), enemy.len()));
        ui.set_text(&format!("{OVERLAY}.win.value"), &format!("{:.0}%", p * 100.0));
        ui.set_properties(&format!("{OVERLAY}.win.value"), &format!("color: {};", color(mine)));
        ui.set_text(
            &format!("{OVERLAY}.win.detail"),
            &format!("patch {} · {} matches", meta.current, meta.current_matches),
        );
        ui.set_properties(&format!("{OVERLAY}.win.back"), &format!("color: {};", color(theirs)));
        ui.set_properties(
            &format!("{OVERLAY}.win.back.fill"),
            &format!("width: {:.1}%; color: {};", (p * 100.0).clamp(2.0, 98.0), color(mine)),
        );

        // advice
        let mut pick_values: Vec<PickValue> = open
            .iter()
            .filter_map(|c| id(c))
            .map(|c| advisor::pick_value(meta, c, &ally, &enemy, players, &damage))
            .collect();
        pick_values.sort_by(|a, b| b.total.partial_cmp(&a.total).unwrap_or(std::cmp::Ordering::Equal));
        let mut ban_values: Vec<PickValue> = open
            .iter()
            .filter_map(|c| id(c))
            .map(|c| advisor::ban_value(meta, c, &ally, &enemy, enemy_players, &damage))
            .collect();
        ban_values.sort_by(|a, b| b.total.partial_cmp(&a.total).unwrap_or(std::cmp::Ordering::Equal));
        let why = |v: &PickValue| -> String {
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
                parts.push("one-sided dmg".to_string());
            }
            parts.join(", ")
        };
        let line = |title: &str, list: &[PickValue], n: usize| -> String {
            let items: Vec<String> = list
                .iter()
                .take(n)
                .map(|v| format!("{} {} ({})", label(v.champ), signed(points(v.total)), why(v)))
                .collect();
            if items.is_empty() {
                format!("{title}: -")
            } else {
                format!("{title}: {}", items.join("  "))
            }
        };
        ui.set_text(&format!("{OVERLAY}.advice.l0"), &line("Pick 选", &pick_values, 2));
        ui.set_text(&format!("{OVERLAY}.advice.l1"), &line("Ban 禁", &ban_values, 2));
        ui.set_text(
            &format!("{OVERLAY}.advice.l2"),
            &match pick_values.get(2) {
                Some(v) => format!("also 备选: {} {} ({})", label(v.champ), signed(points(v.total)), why(v)),
                None => String::new(),
            },
        );

        // grid values
        if view.grid_values {
            let by_champ: HashMap<u16, f32> = pick_values.iter().map(|v| (v.champ, v.total)).collect();
            for c in cards {
                let tag = format!("{}.{TAG}", c.path);
                if !ui.exists(&tag) && !ui.spawn(&c.path, &tag_source(TAG, 52, false)) {
                    continue;
                }
                let value = id(c).and_then(|x| by_champ.get(&x)).copied();
                let shown = value.is_some();
                if self.tagged.get(&tag) != Some(&shown) {
                    ui.set_visible(&tag, shown);
                    self.tagged.insert(tag.clone(), shown);
                }
                if let Some(v) = value {
                    let pts = points(v);
                    ui.set_text(&format!("{tag}.text"), &signed(pts));
                    let c = if pts >= 1.0 { GOOD } else if pts <= -1.0 { BAD } else { DIM };
                    ui.set_properties(&format!("{tag}.text"), &format!("color: {};", color(c)));
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
                let text = probs.get(i).map(|row| {
                    let (best, p) = row
                        .iter()
                        .enumerate()
                        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                        .map(|(r, p)| (Role::ALL[r], *p))
                        .unwrap_or((Role::Top, 0.0));
                    format!("{} {:.0}%", best.name(), p * 100.0)
                });
                if !ui.exists(&tag) && (text.is_none() || !ui.spawn(&path, &tag_source(LANE_TAG, 96, true))) {
                    continue;
                }
                ui.set_visible(&tag, text.is_some());
                if let Some(t) = text {
                    ui.set_text(&format!("{tag}.text"), &t);
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

    fn screen() -> FakeUi {
        let mut ui = FakeUi::default();
        ui.add("main", "match_ui");
        ui.add("main.bottom.blue_side.name", "label").text = Some("Mods FC".into());
        ui.add("main.bottom.red_side.name", "label").text = Some("Rivals".into());
        for col in PICK_COLUMNS {
            for i in 0..5 {
                ui.add(&format!("{col}.{i}"), "blue_pick_slot");
            }
        }
        ui.add(GRID, "empty");
        for (i, n) in NAMES.iter().enumerate() {
            let path = format!("{GRID}.{i}");
            ui.add(&path, "banpick_champion_slot");
            // half the cards name their champion in the state, half only on the label
            if i % 2 == 0 {
                ui.nodes.get_mut(&path).unwrap().state = Some(format!(r#"{{"champion":"{n}"}}"#));
            }
            ui.add(&format!("{path}.name"), "label").text = Some(format!("Name {n}"));
            for badge in ["blue", "red", "ban"] {
                ui.add(&format!("{path}.{badge}"), "color").visible = false;
                ui.add(&format!("{path}.{badge}.text"), "label");
            }
        }
        ui
    }

    fn pick(ui: &mut FakeUi, index: usize, side: &str, order: u32) {
        let path = format!("{GRID}.{index}.{side}");
        ui.nodes.get_mut(&path).unwrap().visible = true;
        ui.nodes.get_mut(&format!("{path}.text")).unwrap().text = Some(order.to_string());
    }

    #[test]
    fn reads_the_draft_and_draws_the_overlay() {
        let (names, games) = simulate(2000, "1.1", |n| match n {
            "c" => 0.5,
            "l" => -0.5,
            _ => 0.0,
        }, 4);
        let champions: Vec<String> = NAMES.iter().map(|s| s.to_string()).collect();
        let meta = build(
            &Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.1", warm: None },
            &Settings::default(),
        );
        let snapshot = Arc::new(Snapshot { meta, damage: HashMap::new() });
        let mut book = NameBook::default();
        for n in NAMES {
            book.learn(&format!("Name {n}"), n);
        }
        let mut ui = screen();
        pick(&mut ui, 0, "red", 1); // the enemy took "a"
        pick(&mut ui, 1, "blue", 1); // we took "b"
        ui.nodes.get_mut(&format!("{GRID}.4.ban")).unwrap().visible = true; // "e" banned
        let rosters = HashMap::new();
        let view = View { team_name: "mods fc", rosters: &rosters, grid_values: true, lane_tags: true };
        let mut screen = DraftScreen::default();
        screen.tick(&mut ui, 0, Some(&snapshot), &book, &view);

        let known = |c: &str| NAMES.contains(&c);
        let cards = read_grid(&ui, &known, &book);
        assert_eq!(cards.len(), 12);
        assert!(cards.iter().all(|c| c.champ.is_some()), "state or label: {cards:?}");
        let card = |i: usize| cards.iter().find(|c| c.path == format!("{GRID}.{i}")).unwrap();
        assert_eq!(card(0).red, Some(1));
        assert!(card(4).banned && !card(3).banned);
        assert_eq!(player_side(&ui, "Mods FC"), Some(0));

        let text = |p: &str| ui.text(p).unwrap_or_default();
        assert!(text("main.pma.win.value").ends_with('%'), "{}", text("main.pma.win.value"));
        assert!(text("main.pma.advice.l0").starts_with("Pick 选: Name "), "{}", text("main.pma.advice.l0"));
        assert!(text("main.pma.advice.l0").contains("Name c"), "the strong champion is advised: {}", text("main.pma.advice.l0"));
        assert!(text("main.pma.advice.l1").contains("Name c"), "{}", text("main.pma.advice.l1"));
        // grid values on open cards only
        assert_eq!(ui.visible(&format!("{GRID}.0.{TAG}")), Some(false), "picked");
        assert_eq!(ui.visible(&format!("{GRID}.4.{TAG}")), Some(false), "banned");
        assert_eq!(ui.visible(&format!("{GRID}.2.{TAG}")), Some(true));
        assert!(text(&format!("{GRID}.2.{TAG}.text")).starts_with('+'), "c is worth picking");
        // a lane read on the enemy's first pick slot
        assert!(text(&format!("main.red_picks.0.{LANE_TAG}.text")).ends_with('%'));
        assert!(!ui.exists(&format!("main.red_picks.1.{LANE_TAG}")));

        // nothing changed: nothing redrawn; the game rebuilt the screen: drawn again
        let spawned = ui.spawned.len();
        screen.tick(&mut ui, READ_EVERY, Some(&snapshot), &book, &view);
        assert_eq!(ui.spawned.len(), spawned);
        ui.remove("main.pma");
        screen.tick(&mut ui, 2 * READ_EVERY, Some(&snapshot), &book, &view);
        assert!(ui.exists("main.pma.win.value"));
    }
}
