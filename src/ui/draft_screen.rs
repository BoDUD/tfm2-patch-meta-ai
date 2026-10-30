//! The ban/pick screen overlay:
//!
//! - **win chance** (bottom left, under the player's picks): the model's probability that the
//!   player's line-up beats the enemy's as picked so far, with a bar;
//! - **advice** (bottom right): the best picks and the best ban for the player right now, each
//!   with what it is worth and why (lane, synergy, matchups, composition, the player's mastery).
//!   In a ban phase (both 3+2 phases and any other format) the three best bans instead - valued
//!   by how much worse they make the enemy's draft (`advisor::ban_board`) - or, while the enemy
//!   bans, the three bans they would most want against us;
//! - **grid values**: on every champion card, at the right end of its name row (under the
//!   portrait) - an arrow and what picking it is worth to the player now, in win-rate points,
//!   coloured green / red / grey; in a ban phase what banning it is worth (禁);
//! - **enemy lanes**: on each enemy pick, the lane icon it most likely plays and a five-step bar
//!   of how sure that is - from the champions' lane history, so it works in every language;
//! - **composition**: under the win chance, each side's style (engage, poke, dive, protect the
//!   carry, tanky) and its worst gap (no frontline, one damage type...);
//! - **scouting**: the enemy team's habits (its most played champions lately) make it likelier
//!   to take them - in its likely picks (the advice panel's fourth row), and in the bans;
//! - **lookahead**: on our turn in a 3+2 draft, the best few candidates are played out to the end
//!   of the draft (`advisor::simulate`) and ranked by the line-ups they lead to (推演 %);
//! - **Fearless series**: with games left, spending a champion nothing replaces costs a little
//!   (留后局);
//! - **breakdown**: while the game shows a champion's info panel (hover or click a card), a line
//!   above it says what its pick and ban values are made of;
//! - **review**: every pick and ban of the draft with what it was worth and what was advised,
//!   kept for the F8 panel's review page ([`last_review`]);
//! - **swap phase**: the best way to seat the team's five champions on its players
//!   (`plan::best_seating`) in place of the advice, with what it is worth over the usual lanes;
//! - **Fearless**: how many champions the series has locked, next to the win chance;
//! - **position locks** are another mod's job (Smart Position Lock): a card it covered with its
//!   lock layer (`spl_lock`) is not advised as a pick and gets no value.
//!
//! What the game shows (seen in its UI tree): the grid `main.champions.contents` holds one
//! `banpick_champion_slot` per champion, **named by the champion id**; a card's `blue` / `red`
//! badge is visible once that side picked it, with the pick number in `<badge>.text`, its
//! `ban` icon once it is banned, and its `fearless_x` once Fearless rules lock it (used in an
//! earlier game of the series) - such a card is not offered as advice and gets no value. Team names sit in `main.bottom.<side>_side.name` with the
//! league rank appended ("Samsung Galaxy #1"). Labels read back the raw text they were given,
//! so champion names are written as the game's own name reference and the game shows them in
//! its language. Both bottom corners of the screen - the bottom bar under the pick columns, up
//! to the team names - are empty in the game's layout and in Ban/Pick View Plus's.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use super::names::{name_ref, NameBook};
use super::{color, team_key, Ui};
use crate::advisor::{self, PickValue};
use crate::diag;
use crate::glm::sigmoid;
use crate::history::Role;
use crate::plan;
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
/// The panels' background (its alpha comes from `panel_opacity`).
const PANEL: u32 = 0x161721f0;
/// Once the pick column under a panel holds its fifth pick (win panel: blue's, advice panel:
/// red's), the panel's background keeps this share of its opacity, so that pick shows through.
const FADED: f32 = 1.0 / 3.0;

/// The panels' background at `opacity` percent.
fn panel_color(opacity: f32) -> u32 {
    let alpha = (opacity.clamp(0.0, 100.0) / 100.0 * 255.0).round() as u32;
    (PANEL & 0xffff_ff00) | alpha
}

/// The layer Smart Position Lock puts over a card the player's team may not pick.
const POSITION_LOCK: &str = "spl_lock";
const SWAP: &str = "main.swap";
/// The phase label (`banpick.ban_phase` / `banpick.pick_phase`) and the arrow on the side whose
/// turn it is.
const STEP: &str = "main.header.step";
/// The game's champion info panel (shown for the hovered / selected card) and its name label.
const INFO: &str = "main.champion_info";
const INFO_NAME: &str = "main.champion_info.stat.name";
/// Our breakdown line above it.
const BREAKDOWN: &str = "main.pma_info";
/// "Bo3" / "Bo5".
const SERIES: &str = "main.bottom.need_win";
const TURN_ARROWS: [&str; 2] = ["main.header.desc.blue_arrow", "main.header.desc.red_arrow"];
const SWAP_TABLES: [&str; 2] = ["main.swap.blue_table", "main.swap.red_table"];

/// What the overlay shows besides the model.
pub struct View<'a> {
    pub team_name: &'a str,
    /// Each team's players and lanes, by [`team_key`].
    pub rosters: &'a HashMap<String, Vec<(u32, Option<Role>)>>,
    /// Each team's most played champions lately (champion id, games, wins) and the matches
    /// counted, by [`team_key`].
    pub team_picks: &'a HashMap<String, Vec<(String, u32, u32)>>,
    pub team_matches: &'a HashMap<String, u32>,
    pub grid_values: bool,
    pub lane_tags: bool,
    /// The panels' background opacity, percent (`panel_opacity`).
    pub panel_opacity: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub path: String,
    pub champ: Option<String>,
    /// Picked by blue / red, with the pick number on the badge.
    pub blue: Option<u32>,
    pub red: Option<u32>,
    pub banned: bool,
    /// Not pickable now (Fearless).
    pub locked: bool,
    /// Another mod's position lock covers it: the player's team may not pick it now.
    pub held: bool,
}

impl Card {
    /// Still on offer: not picked, banned or locked.
    pub fn open(&self) -> bool {
        self.blue.is_none() && self.red.is_none() && !self.banned && !self.locked
    }
}

/// Which side the player is, and whether it is the swap phase (with the player's side known).
#[derive(Clone, Copy)]
struct Turn {
    side: usize,
    /// The player's side is known (not assumed blue).
    side_known: bool,
    swapping: bool,
    /// A ban phase.
    banning: bool,
    /// The side whose turn it is (`None`: no arrow shown).
    acting: Option<usize>,
}

/// A ban phase, by the header's phase label (a text reference, the same in every language).
fn ban_phase(step: &str) -> bool {
    step.contains("ban_phase")
}

#[derive(Default)]
pub struct DraftScreen {
    /// The other team on the last ban/pick screen ([`team_key`]), for scouting.
    pub enemy_team: Option<String>,
    /// The champions both teams picked on the last ban/pick screen (player's team first), for
    /// the tactics screen after it.
    pub last_picks: Option<(Vec<u16>, Vec<u16>)>,
    on: bool,
    next_read: u64,
    /// The draft and model the overlay was last drawn for.
    drawn: Option<u64>,
    tagged: HashMap<String, bool>,
    reported: bool,
    /// The values of the last drawing, for the breakdown line and the review.
    last: Option<Drawn>,
    /// The champion the breakdown line was written for, and the drawing it came from.
    shown_info: Option<(String, Option<u64>)>,
    /// The draft is being followed for the review (the first read only notes what is there).
    tracking: bool,
    /// The draft as last read (champion, blue pick, red pick, banned), the side acting then, and
    /// the review so far.
    seen: HashMap<String, (Option<u32>, Option<u32>, bool)>,
    seen_acting: Option<usize>,
    review: Review,
    /// Hell Draft AI's set-up, read when the screen opens (`None` inside: not enabled or not
    /// drafting for the AI teams), and its lean for the model it was worked out for.
    hell: Option<Option<crate::hell::Hell>>,
    hell_lean: Option<(usize, Arc<HashMap<u16, f32>>)>,
    /// The panels' background as last drawn (the breakdown line is added later, in it).
    back: Option<u32>,
}

/// What the last drawing worked out.
#[derive(Clone, Default)]
struct Drawn {
    picks: HashMap<u16, PickValue>,
    bans: HashMap<u16, advisor::BanValue>,
    /// The best pick and ban for us then.
    best_pick: Option<(u16, f32)>,
    best_ban: Option<(u16, f32)>,
}

/// One pick or ban of a draft as the review shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewRow {
    /// Our team (`Some(true)`), theirs, or not known (a ban when no turn arrow was seen).
    pub ours: Option<bool>,
    pub ban: bool,
    pub champ: String,
    /// Its value to us then (a pick's or a ban's, log-odds), when it was ours.
    pub value: Option<f32>,
    /// What was advised then and its value.
    pub advised: Option<(String, f32)>,
    /// Our win chance after it (picks).
    pub win_after: Option<f32>,
}

/// A draft's review.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Review {
    pub team: String,
    pub enemy: String,
    pub rows: Vec<ReviewRow>,
}

static LAST_REVIEW: std::sync::Mutex<Option<Review>> = std::sync::Mutex::new(None);

/// The review of the last ban/pick screen (for the F8 panel).
pub fn last_review() -> Option<Review> {
    LAST_REVIEW.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

/// Spending a champion nothing replaces in a Fearless series costs this share of what it is
/// worth over the next option, times the share of the series still to come.
const CONSERVE: f32 = 0.4;

/// A team's style and worst gap, in a few words (its open slots counted as average champions).
fn comp_text(meta: &crate::meta::Meta, picks: &[u16]) -> String {
    use crate::knowledge::COMP_NAMES_ZH;
    let (Some(avg), false) = (meta.open_slot.as_ref(), picks.is_empty()) else { return String::new() };
    let mut have = crate::knowledge::Sums::default();
    for c in picks {
        have.add(meta.profile(*c).unwrap_or(avg), 1.0);
    }
    let n = have.n;
    // the trait the line-up has most above an average one
    let styles = [
        ("开团", have.engage - avg.engage * n),
        ("消耗", have.poke - avg.poke * n),
        ("突进", have.dive - avg.dive * n),
        ("保C", (have.peel + have.carry) - (avg.peel + avg.carry) * n),
        ("坦克", have.front - avg.front * n),
    ];
    let mut parts = Vec::new();
    if let Some((name, _)) = styles.iter().filter(|s| s.1 >= 1.0).max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)) {
        parts.push(name.to_string());
    }
    let (_, gaps) = meta.comp_score(picks);
    if let Some((i, _)) = gaps
        .iter()
        .enumerate()
        .filter(|(_, g)| points(**g) <= -1.0)
        .min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
    {
        parts.push(if i == 2 { "伤害单一".to_string() } else { format!("缺{}", COMP_NAMES_ZH[i]) });
    }
    parts.join(" · ")
}

/// Games left after this one in a Fearless series, and the series length: from "Bo3" / "Bo5"
/// and the champions locked so far (ten a game).
fn series_left(label: &str, locked: usize) -> Option<(usize, usize)> {
    let n: usize = label.trim().strip_prefix("Bo")?.parse().ok()?;
    if locked == 0 || n < 2 {
        return None;
    }
    let played = locked.div_ceil(10);
    Some((n.saturating_sub(1 + played), n))
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
                locked: visible(ui, &format!("{path}.fearless_x")),
                held: visible(ui, &format!("{path}.{POSITION_LOCK}")),
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
/// The likely picks of the enemy shown on the fourth row.
const LIKELY: usize = 3;

/// The panels sit in the bottom bar's corners, below both pick columns: the game's last pick
/// slot ends at y 970 (Ban/Pick View Plus's splash cards at 986), and the bar's team names and
/// bans start 500 px in from either edge.
const PANEL_Y: u32 = 988;
const PANEL_H: u32 = 92;

fn overlay_source(back: u32) -> String {
    let panel = |id: &str, x: u32, inner: String| {
        format!(
            "#{id}:color {{ x: {x}px; y: {PANEL_Y}px; width: 460px; height: {PANEL_H}px; color: {}; ignore_event: true; \
             rounding: Uniform {{ rounding: 10; }} {inner}}} ",
            color(back)
        )
    };
    let win = format!(
        "{}{}{}#back:color {{ x: 12px; y: 54px; width: 436px; height: 7px; color: {}; ignore_event: true; \
         rounding: Uniform {{ rounding: 4; }} #fill:color {{ width: 50%; height: 100%; color: {}; ignore_event: true; \
         rounding: Uniform {{ rounding: 4; }} }} }} ",
        label("title", 12, 2, 300, 20, 13, DIM, "Left"),
        label("value", 12, 20, 200, 32, 24, TEXT, "Left"),
        label("detail", 150, 24, 298, 24, 13, DIM, "Right"),
        color(RED),
        color(BLUE)
    ) + &label("enemy", 12, 64, 436, 24, 13, DIM, "Right");
    // the swap phase's seating, in the advice panel's place
    let mut seating = label("title", 12, 2, 436, 22, 13, 0xe8e8e8ff, "Left");
    for k in 0..5u32 {
        let (x, y) = (12 + (k % 3) as i32 * 148, 30 + (k / 3) as i32 * 26);
        seating.push_str(&format!(
            "#s{k}:empty {{ x: {x}px; y: {y}px; width: 146px; height: 24px; ignore_event: true; \
             #icon:image {{ x: 0px; y: 3px; width: 18px; height: 18px; color: #c2c6ceff; ignore_event: true; \
             source: \"asset/base/ui/icons/top\"; }} {}}} ",
            label("name", 22, 0, 122, 24, 13, TEXT, "Left")
        ));
    }
    let mut advice = format!("#seating:empty {{ width: 100%; height: 100%; visible: false; ignore_event: true; {seating}}} ");
    for r in 0..ADVICE_ROWS {
        let y = 2 + r as u32 * 22;
        advice.push_str(&label(&format!("k{r}"), 12, y, 46, 22, 13, DIM, "Left"));
        advice.push_str(&label(&format!("c{r}"), 58, y, 124, 22, 14, TEXT, "Left"));
        advice.push_str(&label(&format!("v{r}"), 182, y, 50, 22, 14, GOOD, "Right"));
        advice.push_str(&label(&format!("w{r}"), 240, y, 214, 22, 12, DIM, "Left"));
    }
    // the enemy's likely picks
    advice.push_str(&label("k3", 12, 68, 46, 22, 13, DIM, "Left"));
    for i in 0..LIKELY {
        advice.push_str(&label(&format!("n{i}"), 58 + i as u32 * 132, 68, 128, 22, 13, TEXT, "Left"));
    }
    format!(
        "pma:empty {{ width: 100%; height: 100%; ignore_event: true; {}{}}}",
        panel("win", 20, win),
        panel("advice", 1440, advice)
    )
}

/// The breakdown line above the game's champion info panel.
fn breakdown_source(back: u32) -> String {
    format!(
        "pma_info:color {{ x: 310px; y: 646px; width: 1300px; height: 34px; color: {}; visible: false; ignore_event: true; \
         rounding: Uniform {{ rounding: 8; }} {}}}",
        color(back),
        label("text", 14, 4, 1272, 26, 14, TEXT, "Left")
    )
}

/// The value on a card: small coloured text at the right end of the name row, under the
/// portrait (nothing of the champion is covered; the card's own marks are on the portrait - tier
/// letter bottom left, position icons bottom right, pick/ban/Fearless badges top right). The
/// stripe child is kept, hidden, for the colour writes.
fn chip_source() -> String {
    format!(
        "{TAG}:empty {{ anchor_x: 1; pivot_x: 1; anchor_y: 1; pivot_y: 1; x: -6px; y: 0px; width: 46px; height: 37px; \
         ignore_event: true; \
         #stripe:color {{ width: 0px; height: 0px; visible: false; color: {}; ignore_event: true; }} \
         #text:label {{ @\"asset/base/style/main#bold_label\"; width: 100%; height: 100%; size: 13; \
         align_x: Right; align_y: Center; text: \"\"; ignore_event: true; }} }}",
        color(DIM)
    )
}

/// The card's name moves to the left of its row to make room for the value.
const NAME_LEFT: &str = "align_x: Left; x: 8px; width: 74px;";

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
    if let Some(f) = v.fills {
        parts.push(format!("补{}", crate::knowledge::COMP_NAMES_ZH[f]));
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
    if v.exposure >= 0.02 {
        parts.push(format!("counterable {}", signed(-points(v.exposure))));
    }
    if v.contest >= 0.02 {
        parts.push(format!("抢 {}", signed(points(v.contest))));
    }
    if v.conserve >= 0.02 {
        parts.push(format!("留后局 {}", signed(-points(v.conserve))));
    }
    if v.balance > 0.0 {
        parts.push("one-sided".to_string());
    }
    parts.join(" · ")
}

/// Why a ban is worth what it is: the lane it denies, whether anything replaces it there, how
/// many lanes it threatens, what it does against our picks, the enemy player on it, and what it
/// would cost us. At most four parts (the row is narrow).
fn why_ban(b: &advisor::BanValue) -> String {
    let t = &b.threat;
    let mut parts = Vec::new();
    if let Some(r) = t.role {
        parts.push(format!("{}{}", r.name(), if b.irreplaceable { " 无替代" } else { "" }));
    }
    if b.lanes >= 2 {
        parts.push(format!("{}路摇摆", b.lanes));
    }
    if b.habit >= 0.05 {
        parts.push("对方常用".to_string());
    }
    if t.counter >= 0.02 {
        parts.push(format!("克我方 {}", signed(points(t.counter))));
    }
    if t.mastery >= 0.02 {
        parts.push(format!("选手 {}", signed(points(t.mastery))));
    }
    if let Some(f) = t.fills {
        parts.push(format!("补{}", crate::knowledge::COMP_NAMES_ZH[f]));
    }
    if t.synergy >= 0.03 {
        parts.push(format!("配合 {}", signed(points(t.synergy))));
    }
    if b.own >= 0.03 {
        parts.push(format!("我方也想要 {}", signed(-points(b.own))));
    }
    parts.truncate(4);
    parts.join(" · ")
}

/// What a champion's pick and ban values are made of, in win-rate points.
fn breakdown_text(d: &Drawn, champ: u16) -> Option<String> {
    let mut out = Vec::new();
    let part = |name: &str, v: f32| (points(v).abs() >= 0.1).then(|| format!("{name} {}", signed(points(v))));
    if let Some(v) = d.picks.get(&champ) {
        let mut parts = vec![format!("{} {}", v.role.map_or("", |r| r.name()), signed(points(v.strength)))];
        parts.extend(
            [
                part("配合", v.synergy),
                part("克制", v.counter),
                part("阵容", v.comp - v.balance),
                part("选手", v.mastery),
                part("抢", v.contest),
                part("被克", -v.exposure),
                part("留后局", -v.conserve),
            ]
            .into_iter()
            .flatten(),
        );
        out.push(format!("选 {} = {}", signed(points(v.total)), parts.join(" · ")));
    }
    if let Some(b) = d.bans.get(&champ) {
        let mut parts = vec![format!("对方损失 {}", signed(points(b.deny)))];
        parts.extend(part("我方损失", -b.own));
        if b.irreplaceable {
            parts.push("无替代".to_string());
        }
        if b.habit >= 0.05 {
            parts.push("对方常用".to_string());
        }
        out.push(format!("禁 {} = {}", signed(points(b.total)), parts.join(" · ")));
    }
    (!out.is_empty()).then(|| out.join("    |    "))
}

impl DraftScreen {
    /// One frame on a screen that may be the ban/pick screen.
    pub fn tick(&mut self, ui: &mut impl Ui, frame: u64, snapshot: Option<&Arc<Snapshot>>, names: &NameBook, view: &View<'_>) {
        if !ui.exists(GRID) {
            if self.on {
                self.on = false;
                self.drawn = None;
                self.tagged.clear();
                self.finish_review();
                crate::draft::set_live(false);
                crate::shared::set_coach_plan(None);
                self.hell = None;
                self.hell_lean = None;
            }
            return;
        }
        if !self.on {
            self.on = true;
            diag::log("[ui] ban/pick screen");
            // the AI's decisions from now on are the player's own match (diag.log)
            crate::draft::set_live(true);
            crate::shared::take_coach_choices();
            crate::shared::take_ai_choices();
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
            if !ui.exists(OVERLAY) && !ui.spawn("main", &overlay_source(panel_color(view.panel_opacity))) {
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
        let swap_phase = ui.visible(SWAP) == Some(true);
        let banning = ui.text(STEP).is_some_and(|t| ban_phase(&t));
        let acting = (0..2).find(|s| visible(ui, TURN_ARROWS[*s]));
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
        // the seating needs to know which team is the player's
        let side_known = side.is_some();
        let side = side.unwrap_or(0);
        if self.review.team.is_empty() {
            let names = [ui.text(BLUE_NAME).unwrap_or_default(), ui.text(RED_NAME).unwrap_or_default()];
            self.review.team = names[side].clone();
            self.review.enemy = names[1 - side].clone();
        }
        self.track(&cards, side_known.then_some(side), meta, acting);

        // what changed since the last drawing: the draft, the model, or our nodes went missing
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for c in &cards {
            (&c.champ, c.blue, c.red, c.banned, c.locked, c.held).hash(&mut h);
        }
        (side, Arc::as_ptr(snapshot) as usize, swap_phase, banning, acting, view.panel_opacity.to_bits()).hash(&mut h);
        let key = h.finish() | 1 << 63;
        // our nodes still there? the overlay, and the chips of the first and last card (a rebuilt
        // grid loses all of them; asking every card each time would be 131 more calls)
        let tagged: Vec<&Card> = cards.iter().filter(|c| c.champ.is_some()).collect();
        let healthy = ui.exists(OVERLAY)
            && (!view.grid_values
                || [tagged.first(), tagged.last()].into_iter().flatten().all(|c| ui.exists(&format!("{}.{TAG}", c.path))));
        if self.drawn == Some(key) && healthy {
            self.breakdown(ui, meta);
            return;
        }
        if !ui.exists(OVERLAY) && !ui.spawn("main", &overlay_source(panel_color(view.panel_opacity))) {
            diag::log_once("ui-overlay", "[ui] could not add the overlay to the ban/pick screen");
            return;
        }
        self.drawn = Some(key);
        // the players on each side, by the team names at the bottom of the screen
        let roster_of = |path: &str| -> Vec<(u32, Option<Role>)> {
            ui.text(path).and_then(|t| view.rosters.get(&team_key(&t)).cloned()).unwrap_or_default()
        };
        let rosters = [roster_of(BLUE_NAME), roster_of(RED_NAME)];
        let turn = Turn { side, side_known, swapping: swap_phase && side_known, banning, acting };
        self.draw(ui, snapshot, &cards, turn, &rosters, view);
        self.breakdown(ui, meta);
    }

    /// Notes every pick and ban since the last read for the review: who made it (a pick by its
    /// badge, a ban by the turn arrow seen before it), what it was worth to us and what was
    /// advised then (the last drawing's values), and our win chance after it.
    fn track(&mut self, cards: &[Card], side: Option<usize>, meta: &crate::meta::Meta, acting: Option<usize>) {
        let first = !self.tracking;
        self.tracking = true;
        let mut events = Vec::new();
        for c in cards {
            let Some(name) = &c.champ else { continue };
            let now = (c.blue, c.red, c.banned);
            let before = self.seen.get(name).copied().unwrap_or((None, None, false));
            if before == now {
                continue;
            }
            self.seen.insert(name.clone(), now);
            if first {
                continue;
            }
            let picked_by = if now.0.is_some() && before.0.is_none() {
                Some(0)
            } else if now.1.is_some() && before.1.is_none() {
                Some(1)
            } else {
                None
            };
            let ban = now.2 && !before.2;
            if picked_by.is_none() && !ban {
                continue;
            }
            let actor = picked_by.or(self.seen_acting);
            events.push((name.clone(), ban, side.and_then(|s| actor.map(|a| a == s))));
        }
        self.seen_acting = acting;
        if events.is_empty() {
            return;
        }
        // our picks and theirs now, for the win chance after
        let ids = |blue: bool| -> Vec<u16> {
            let mut list: Vec<(u32, u16)> = cards
                .iter()
                .filter_map(|c| Some((if blue { c.blue? } else { c.red? }, meta.names.get(c.champ.as_deref()?)?)))
                .collect();
            list.sort();
            list.into_iter().map(|(_, c)| c).collect()
        };
        let (blue, red) = (ids(true), ids(false));
        let win = side.map(|s| {
            let (ally, enemy) = if s == 0 { (&blue, &red) } else { (&red, &blue) };
            advisor::win_probability(meta, ally, enemy)
        });
        for (name, ban, ours) in events {
            let id = meta.names.get(&name);
            let last = self.last.as_ref();
            let (value, advised) = match (ours, last, id) {
                (Some(true), Some(l), Some(id)) if ban => (l.bans.get(&id).map(|b| b.total), l.best_ban),
                (Some(true), Some(l), Some(id)) => (l.picks.get(&id).map(|v| v.total), l.best_pick),
                _ => (None, None),
            };
            self.review.rows.push(ReviewRow {
                ours,
                ban,
                champ: name,
                value,
                advised: advised.map(|(c, v)| (meta.names.name(c).to_string(), v)),
                win_after: if ban { None } else { win },
            });
        }
    }

    /// The ban/pick screen closed: its review is kept for the F8 panel and summed up in diag.log.
    fn finish_review(&mut self) {
        let review = std::mem::take(&mut self.review);
        self.seen.clear();
        self.tracking = false;
        self.seen_acting = None;
        self.last = None;
        self.shown_info = None;
        if review.rows.is_empty() {
            return;
        }
        let ours: Vec<&ReviewRow> = review.rows.iter().filter(|r| r.ours == Some(true)).collect();
        let followed = ours.iter().filter(|r| r.advised.as_ref().is_some_and(|(c, _)| *c == r.champ)).count();
        // the coach override's choices: each one that is not among our picks and bans was
        // overridden by another mod
        let coach = crate::shared::take_coach_choices();
        let coach_note = if coach.is_empty() {
            String::new()
        } else {
            let names = crate::shared::get().map(|s| s.meta.names.clone());
            let taken = coach
                .iter()
                .filter(|(ban, c)| {
                    let name = names.as_ref().map(|n| n.name(*c).to_string()).unwrap_or_default();
                    ours.iter().any(|r| r.ban == *ban && r.champ == name)
                })
                .count();
            format!(", coach override {} decided, {taken} of them made", coach.len())
        };
        // ai_draft's choices for the other team, the same way
        let ai = crate::shared::take_ai_choices();
        let ai_note = if ai.is_empty() {
            String::new()
        } else {
            let names = crate::shared::get().map(|s| s.meta.names.clone());
            let theirs: Vec<&ReviewRow> = review.rows.iter().filter(|r| r.ours == Some(false)).collect();
            let made = ai
                .iter()
                .filter(|(ban, c)| {
                    let name = names.as_ref().map(|n| n.name(*c).to_string()).unwrap_or_default();
                    theirs.iter().any(|r| r.ban == *ban && r.champ == name)
                })
                .count();
            format!(", ai_draft {} decided, {made} of them made", ai.len())
        };
        diag::log(&format!(
            "[review] {} vs {}: {} picks and bans seen, ours {} ({} as advised){coach_note}{ai_note}, win chance at the end {}",
            review.team,
            review.enemy,
            review.rows.len(),
            ours.len(),
            followed,
            review.rows.iter().rev().find_map(|r| r.win_after).map_or("-".to_string(), |w| format!("{:.0}%", w * 100.0))
        ));
        *LAST_REVIEW.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(review);
    }

    /// The breakdown line above the game's champion info panel: what the shown champion's pick
    /// and ban values are made of (hidden when it is not on offer or no values are known).
    fn breakdown(&mut self, ui: &mut impl Ui, meta: &crate::meta::Meta) {
        let champ = if visible(ui, INFO) {
            ui.text(INFO_NAME).and_then(|t| {
                t.strip_prefix("#asset/base/text/champion?description.").and_then(|r| r.strip_suffix(".name")).map(str::to_string)
            })
        } else {
            None
        };
        let key = champ.clone().map(|c| (c, self.drawn));
        if key == self.shown_info && ui.exists(BREAKDOWN) {
            return;
        }
        if !ui.exists(BREAKDOWN) && !ui.spawn("main", &breakdown_source(self.back.unwrap_or(PANEL))) {
            return;
        }
        self.shown_info = key;
        let text = champ.as_deref().and_then(|c| meta.names.get(c)).and_then(|id| breakdown_text(self.last.as_ref()?, id));
        ui.set_visible(BREAKDOWN, text.is_some());
        if let Some(t) = text {
            ui.set_text(&format!("{BREAKDOWN}.text"), &t);
        }
    }

    /// The best seating of the team's champions in the swap phase, written into the advice
    /// panel. The seats are the team's swap slots in order (their position from the slot's
    /// position label), each with the team's player for that position.
    fn seating(
        &self,
        ui: &mut impl Ui,
        snapshot: &Snapshot,
        ally: &[u16],
        side: usize,
        players: &[(u32, Option<Role>)],
    ) -> Option<()> {
        if ally.len() != 5 {
            return None;
        }
        let table = SWAP_TABLES[side];
        let seats: Vec<plan::Seat> = (0..5)
            .map(|i| {
                let label = ui.text(&format!("{table}.swap_slot_{i}.data.position_name")).unwrap_or_default();
                let role = label.strip_prefix("#asset/base/text/ui?position.").and_then(Role::parse).unwrap_or(Role::ALL[i]);
                (role, players.iter().find(|(_, r)| *r == Some(role)).map(|(a, _)| *a))
            })
            .collect();
        let infos = super::athlete_infos();
        let (perm, gain) = plan::best_seating(&snapshot.meta, ally, &seats, &infos)?;
        let pts = points(gain);
        ui.set_text(&format!("{OVERLAY}.advice.seating.title"), &format!("Best seating 最佳换位  +{pts:.1}"));
        for (k, (champ, (role, _))) in perm.iter().zip(&seats).enumerate() {
            let cell = format!("{OVERLAY}.advice.seating.s{k}");
            ui.set_properties(&format!("{cell}.icon"), &format!("source: \"{}\";", lane_icon(*role)));
            ui.set_text(&format!("{cell}.name"), &name_ref(snapshot.meta.names.name(*champ)));
        }
        Some(())
    }

    fn draw(
        &mut self,
        ui: &mut impl Ui,
        snapshot: &Snapshot,
        cards: &[Card],
        turn: Turn,
        rosters: &[Vec<(u32, Option<Role>)>; 2],
        view: &View<'_>,
    ) {
        let side = turn.side;
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
        // the panels' background: each sits on the bottom slot of a pick column (win on blue's,
        // advice on red's) and turns see-through once that slot holds a pick
        let faded = |picked: usize| if picked >= 5 { view.panel_opacity * FADED } else { view.panel_opacity };
        for (panel, picked) in [("win", blue.len()), ("advice", red.len())] {
            ui.set_properties(&format!("{OVERLAY}.{panel}"), &format!("color: {};", color(panel_color(faded(picked)))));
        }
        let all_in = faded(blue.len().min(red.len()));
        self.back = Some(panel_color(all_in));
        ui.set_properties(BREAKDOWN, &format!("color: {};", color(panel_color(all_in))));
        let (ally, enemy) = if side == 0 { (blue, red) } else { (red, blue) };
        self.last_picks = Some((ally.clone(), enemy.clone()));
        let damage = |c: u16| snapshot.damage_of(c);
        let open: Vec<u16> =
            cards.iter().filter(|c| c.open()).filter_map(&id).collect();
        // the champions the player's team may pick: open, and not held by a position lock
        let pickable: Vec<u16> = cards.iter().filter(|c| c.open() && !c.held).filter_map(&id).collect();

        // win chance, and each side's composition
        let p = advisor::win_probability(meta, &ally, &enemy);
        let (mine, theirs) = if side == 0 { (BLUE, RED) } else { (RED, BLUE) };
        let fearless = cards.iter().filter(|c| c.locked).count();
        let title = if fearless > 0 {
            format!("胜率 Win · {} vs {} · Fearless {fearless}", ally.len(), enemy.len())
        } else {
            format!("胜率 Win · {} vs {} · {} 场", ally.len(), enemy.len(), meta.current_matches)
        };
        ui.set_text(&format!("{OVERLAY}.win.title"), &title);
        ui.set_text(&format!("{OVERLAY}.win.value"), &format!("{:.0}%", p * 100.0));
        ui.set_properties(&format!("{OVERLAY}.win.value"), &format!("color: {};", color(mine)));
        let comp = |label: &str, picks: &[u16]| {
            let t = comp_text(meta, picks);
            if t.is_empty() { String::new() } else { format!("{label} {t}") }
        };
        ui.set_text(&format!("{OVERLAY}.win.detail"), &comp("我方", &ally));
        ui.set_text(&format!("{OVERLAY}.win.enemy"), &comp("敌方", &enemy));
        ui.set_properties(&format!("{OVERLAY}.win.back"), &format!("color: {};", color(theirs)));
        ui.set_properties(
            &format!("{OVERLAY}.win.back.fill"),
            &format!("width: {:.1}%; color: {};", (p * 100.0).clamp(2.0, 98.0), color(mine)),
        );

        // each team's habits (scouting): the share of its recent matches it played a champion in
        let habits_of = |team: Option<&str>| -> Option<HashMap<u16, f32>> {
            let team = team?;
            let list = view.team_picks.get(team)?;
            let matches = view.team_matches.get(team).copied().unwrap_or(0).max(1) as f32;
            Some(list.iter().filter_map(|(c, g, _)| Some((meta.names.get(c)?, *g as f32 / matches))).collect())
        };
        let their_habits = habits_of(self.enemy_team.as_deref());
        let our_habits = habits_of(Some(team_key(view.team_name).as_str()));
        // Hell Draft AI drafts for the other team: it takes what it rates, not what we value
        let hell = self
            .hell
            .get_or_insert_with(|| {
                let on = crate::compat::get().hell();
                on.then(|| crate::hell::dir().as_deref().and_then(crate::hell::read)).flatten().filter(|h| h.enemy)
            })
            .clone();
        let their_lean = hell.map(|h| {
            let model = snapshot as *const Snapshot as usize;
            match &self.hell_lean {
                Some((m, lean)) if *m == model => lean.clone(),
                _ => {
                    let lean = Arc::new(crate::hell::lean(meta, &h));
                    self.hell_lean = Some((model, lean.clone()));
                    lean
                }
            }
        });

        // advice
        let their_left = 5usize.saturating_sub(enemy.len());
        let us = advisor::Team { picks: &ally, players, habits: our_habits.as_ref(), lean: None };
        let them =
            advisor::Team { picks: &enemy, players: enemy_players, habits: their_habits.as_ref(), lean: their_lean.as_deref() };
        let blue = turn.side_known.then_some(side == 0);
        // a Fearless series with games left: spending what nothing replaces costs a little
        let series = series_left(&ui.text(SERIES).unwrap_or_default(), fearless);
        let conserve_share = series.map_or(0.0, |(left, n)| CONSERVE * left as f32 / (n - 1).max(1) as f32);
        let their_turn = turn.acting.is_some_and(|a| a != side);
        let banned = cards.iter().filter(|c| c.banned).count();
        // our bans and picks, the best few played out on our turn of a 3+2 draft (the same
        // ranking the AI drafts by with ai_draft on)
        let plan = advisor::plan_turn(
            meta,
            &advisor::TurnInput {
                us,
                them,
                offer: &open,
                pickable: &pickable,
                banning: turn.banning,
                blue,
                bans_made: banned,
                lookahead: !their_turn && !turn.swapping,
                conserve: conserve_share,
            },
            &damage,
        );
        let advisor::TurnPlan { board, picks: pick_values, outlook } = plan;
        let by_ban: HashMap<u16, advisor::BanValue> = board.iter().map(|b| (b.champ, b.clone())).collect();
        // the enemy's turn to ban: what they would most want to take from us
        let mut their_bans = if turn.banning && their_turn {
            advisor::ban_board(meta, them, us, advisor::Mastery::FULL, &damage, &open, blue.map(|b| !b))
        } else {
            Vec::new()
        };
        // Hell Draft AI bans what it rates highest, whatever it takes from us
        if them.lean.is_some() {
            let rating = |c: u16| them.hell_rating(meta, c).unwrap_or(f32::NEG_INFINITY);
            their_bans.sort_by(|a, b| rating(b.champ).partial_cmp(&rating(a.champ)).unwrap_or(std::cmp::Ordering::Equal));
        }

        // the coach override follows this ranking when the game's coach drafts for us
        if !their_turn && !turn.swapping {
            let (mut a, mut e) = (ally.clone(), enemy.clone());
            a.sort_unstable();
            e.sort_unstable();
            let ranked = if turn.banning { board.iter().map(|b| b.champ).collect() } else { pick_values.iter().map(|v| v.champ).collect() };
            let held = cards.iter().filter(|c| c.held).filter_map(&id).collect();
            crate::shared::set_coach_plan(Some(crate::shared::CoachPlan { ally: a, enemy: e, bans: banned, ban: turn.banning, ranked, held }));
        }

        // the swap phase: the best seating instead of picks and bans
        let seating = if turn.swapping { self.seating(ui, snapshot, &ally, side, players) } else { None };
        ui.set_visible(&format!("{OVERLAY}.advice.seating"), seating.is_some());
        enum Row<'a> {
            Pick(&'a PickValue),
            Ban(&'a advisor::BanValue),
        }
        let rows: Vec<(&str, Option<Row>)> = if seating.is_some() {
            vec![("", None), ("", None), ("", None)]
        } else if turn.banning && their_turn {
            vec![("敌禁?", their_bans.first().map(Row::Ban)), ("敌禁?", their_bans.get(1).map(Row::Ban)), ("敌禁?", their_bans.get(2).map(Row::Ban))]
        } else if turn.banning {
            vec![("禁 1", board.first().map(Row::Ban)), ("禁 2", board.get(1).map(Row::Ban)), ("禁 3", board.get(2).map(Row::Ban))]
        } else {
            vec![("Pick 选", pick_values.first().map(Row::Pick)), ("Alt 备", pick_values.get(1).map(Row::Pick)), ("Ban 禁", board.first().map(Row::Ban))]
        };

        for (r, (what, v)) in rows.iter().enumerate().take(ADVICE_ROWS) {
            let row = |part: &str| format!("{OVERLAY}.advice.{part}{r}");
            ui.set_text(&row("k"), what);
            let shown = match v {
                Some(Row::Pick(v)) => Some((v.champ, v.total, why(v))),
                Some(Row::Ban(b)) => Some((b.champ, b.total, why_ban(b))),
                None => None,
            };
            match shown {
                Some((champ, total, reason)) => {
                    ui.set_text(&row("c"), &name_ref(meta.names.name(champ)));
                    let pts = points(total);
                    ui.set_text(&row("v"), &signed(pts));
                    ui.set_properties(&row("v"), &format!("color: {};", color(if pts >= 0.0 { GOOD } else { BAD })));
                    let reason = match outlook.get(&champ) {
                        Some(w) => format!("推演 {:.0}% · {reason}", w * 100.0),
                        None => reason,
                    };
                    ui.set_text(&row("w"), &reason);
                }
                None => {
                    for part in ["c", "v", "w"] {
                        ui.set_text(&row(part), "");
                    }
                }
            }
        }

        // the enemy's likely next picks (scouting: their habits count)
        let likely = if seating.is_none() && their_left > 0 {
            advisor::likely_picks(meta, them, &ally, advisor::Mastery::FULL, &damage, &open)
        } else {
            Vec::new()
        };
        ui.set_text(&format!("{OVERLAY}.advice.k3"), if likely.is_empty() { "" } else { "敌可能" });
        for i in 0..LIKELY {
            let text = likely.get(i).map(|v| name_ref(meta.names.name(v.champ))).unwrap_or_default();
            ui.set_text(&format!("{OVERLAY}.advice.n{i}"), &text);
        }

        self.last = Some(Drawn {
            best_pick: pick_values.first().map(|v| (v.champ, v.total)),
            best_ban: board.first().map(|b| (b.champ, b.total)),
            picks: pick_values.iter().map(|v| (v.champ, v.clone())).collect(),
            bans: by_ban,
        });

        // grid values
        if view.grid_values {
            // our ban turn: what banning each card is worth; else what picking it is
            let banning = turn.banning && !their_turn;
            let by_champ: HashMap<u16, f32> = if banning {
                board.iter().map(|b| (b.champ, b.total)).collect()
            } else {
                pick_values.iter().map(|v| (v.champ, v.total)).collect()
            };
            for c in cards.iter().filter(|c| c.champ.is_some()) {
                let tag = format!("{}.{TAG}", c.path);
                if !ui.exists(&tag) {
                    if !ui.spawn(&c.path, &chip_source()) {
                        continue;
                    }
                    // a new card (or the game rebuilt it): its name makes room
                    ui.set_properties(&format!("{}.name", c.path), NAME_LEFT);
                }
                let value = id(c).and_then(|x| by_champ.get(&x)).copied();
                let shown = value.is_some();
                if self.tagged.get(&tag) != Some(&shown) || !shown {
                    ui.set_visible(&tag, shown);
                    self.tagged.insert(tag.clone(), shown);
                }
                if let Some(v) = value {
                    let pts = points(v);
                    let (arrow, c) = if banning {
                        ("禁", if pts >= 1.0 { BAD } else { DIM })
                    } else if pts >= 1.0 {
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
            ui.add(&format!("{path}.fearless_x"), "image").visible = false;
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
            &Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.1", warm: None, stats: None },
            &Settings::default(),
        );
        Arc::new(Snapshot::new(meta, HashMap::new()))
    }

    #[test]
    fn coach_features_ban_phase_scouting_breakdown_review() {
        let snapshot = snapshot(2000);
        let book = NameBook::default();
        let mut ui = screen();
        // blue's first ban of a 3+2 draft: the phase label and blue's turn arrow
        ui.add(STEP, "label").text = Some("#asset/base/text/ui?banpick.ban_phase".into());
        ui.add(TURN_ARROWS[0], "image");
        ui.add(TURN_ARROWS[1], "image").visible = false;
        // the game's info panel shows "d"
        ui.add(INFO, "empty");
        ui.add(INFO_NAME, "label").text = Some("#asset/base/text/champion?description.d.name".into());
        let rosters = HashMap::new();
        // the rivals played "e" in every one of their last ten matches
        let team_picks: HashMap<String, Vec<(String, u32, u32)>> = [("rivals".to_string(), vec![("e".to_string(), 10, 6)])].into();
        let team_matches: HashMap<String, u32> = [("rivals".to_string(), 10)].into();
        let view = View { team_name: "Mods FC", rosters: &rosters, team_picks: &team_picks, team_matches: &team_matches, grid_values: true, lane_tags: true, panel_opacity: 60.0 };
        let mut screen = DraftScreen::default();
        screen.tick(&mut ui, 0, Some(&snapshot), &book, &view);
        let text = |ui: &FakeUi, p: &str| ui.text(p).unwrap_or_default();

        assert_eq!(text(&ui, "main.pma.advice.k0"), "禁 1");
        assert!(text(&ui, "main.pma.advice.w0").starts_with("推演 "), "played out: {}", text(&ui, "main.pma.advice.w0"));
        assert_eq!(text(&ui, "main.pma.advice.k3"), "敌可能");
        let likely: Vec<String> = (0..LIKELY).map(|i| text(&ui, &format!("main.pma.advice.n{i}"))).collect();
        assert!(likely.contains(&name_ref("e")), "their favourite is among their likely picks: {likely:?}");
        // the breakdown of the champion in the info panel
        assert_eq!(ui.visible(BREAKDOWN), Some(true));
        let line = text(&ui, &format!("{BREAKDOWN}.text"));
        assert!(line.starts_with("选 ") && line.contains("禁 "), "{line}");
        // ban cards show 禁
        assert!(text(&ui, &format!("{GRID}.d.{TAG}.text")).starts_with('禁'));

        // blue bans "c" as advised or not, then picks "d"; the screen closes: the review has both
        let advised_ban = text(&ui, "main.pma.advice.c0");
        ui.nodes.get_mut(&format!("{GRID}.c.ban")).unwrap().visible = true;
        screen.tick(&mut ui, READ_EVERY, Some(&snapshot), &book, &view);
        ui.nodes.get_mut(STEP).unwrap().text = Some("#asset/base/text/ui?banpick.pick_phase".into());
        pick(&mut ui, "d", "blue", 1);
        screen.tick(&mut ui, 2 * READ_EVERY, Some(&snapshot), &book, &view);
        ui.remove(GRID);
        screen.tick(&mut ui, 3 * READ_EVERY, Some(&snapshot), &book, &view);
        let review = last_review().expect("a review");
        assert_eq!(review.enemy, "Rivals\r #10");
        assert_eq!(review.rows.len(), 2, "{review:?}");
        let (ban, pick_row) = (&review.rows[0], &review.rows[1]);
        assert!(ban.ban && ban.champ == "c" && ban.ours == Some(true) && ban.value.is_some());
        assert_eq!(ban.advised.as_ref().map(|(c, _)| name_ref(c)), Some(advised_ban));
        assert!(!pick_row.ban && pick_row.champ == "d" && pick_row.ours == Some(true) && pick_row.win_after.is_some());
    }

    #[test]
    fn series_and_composition_words() {
        assert_eq!(series_left("Bo3", 10), Some((1, 3)), "one game played, one more possible");
        assert_eq!(series_left("Bo5", 20), Some((2, 5)));
        assert_eq!(series_left("Bo3", 0), None, "not Fearless (or its first game)");
        assert_eq!(series_left("Bo1", 10), None);
        assert_eq!(series_left("", 10), None);
    }

    #[test]
    fn reads_the_draft_and_draws_the_overlay() {
        let snapshot = snapshot(2000);
        let book = NameBook::default();
        let mut ui = screen();
        pick(&mut ui, "a", "red", 1); // the enemy took "a"
        pick(&mut ui, "b", "blue", 1); // we took "b"
        ui.nodes.get_mut(&format!("{GRID}.e.ban")).unwrap().visible = true;
        // Fearless: "g" was played earlier in the series
        ui.add(&format!("{GRID}.g.fearless_x"), "image").visible = true;
        // Smart Position Lock holds "c" (the strongest) for the player's team
        ui.add(&format!("{GRID}.c.{POSITION_LOCK}"), "color_icon_button");
        let mut rosters = HashMap::new();
        rosters.insert("rivals".to_string(), vec![(9u32, Some(Role::Mid))]);
        let (no_picks, no_matches) = (HashMap::new(), HashMap::new());
        let view = View { team_name: "Mods FC", rosters: &rosters, team_picks: &no_picks, team_matches: &no_matches, grid_values: true, lane_tags: true, panel_opacity: 60.0 };
        let mut screen = DraftScreen::default();
        screen.tick(&mut ui, 0, Some(&snapshot), &book, &view);

        let known = |c: &str| NAMES.contains(&c);
        let cards = read_grid(&ui, &known, &book);
        assert_eq!(cards.len(), 12);
        let card = |n: &str| cards.iter().find(|c| c.champ.as_deref() == Some(n)).unwrap();
        assert_eq!(card("a").red, Some(1));
        assert!(card("e").banned && !card("d").banned);
        assert!(card("g").locked && !card("g").open() && card("d").open());
        assert!(card("c").held && card("c").open() && !card("d").held);
        assert_eq!(player_side(&ui, "mods fc"), Some(0), "the rank after the name is ignored");
        assert_eq!(screen.enemy_team.as_deref(), Some("rivals"));

        let text = |p: &str| ui.text(p).unwrap_or_default();
        assert!(text("main.pma.win.value").ends_with('%'), "{}", text("main.pma.win.value"));
        // champion names are the game's name references, shown in its language
        assert!(text("main.pma.advice.c0").starts_with("#asset/base/text/champion?description."));
        let advised = [text("main.pma.advice.c0"), text("main.pma.advice.c1")];
        assert!(!advised.contains(&name_ref("c")), "held by the position lock: {advised:?}");
        assert_eq!(text("main.pma.advice.k2"), "Ban 禁");
        // grid values on open cards only
        assert_eq!(ui.visible(&format!("{GRID}.a.{TAG}")), Some(false), "picked");
        assert_eq!(ui.visible(&format!("{GRID}.e.{TAG}")), Some(false), "banned");
        assert_eq!(ui.visible(&format!("{GRID}.g.{TAG}")), Some(false), "locked by Fearless");
        for row in 0..3 {
            assert_ne!(text(&format!("main.pma.advice.c{row}")), name_ref("g"), "a locked champion is never advised");
        }
        assert_eq!(ui.visible(&format!("{GRID}.c.{TAG}")), Some(false), "held by the position lock");
        assert_eq!(ui.visible(&format!("{GRID}.d.{TAG}")), Some(true));
        // the value sits in the name row: the name moved left to make room
        assert!(ui.nodes[&format!("{GRID}.d.name")].props.iter().any(|p| p == NAME_LEFT));

        // the lock lets go of "c": it is the best pick again
        ui.remove(&format!("{GRID}.c.{POSITION_LOCK}"));
        screen.tick(&mut ui, READ_EVERY, Some(&snapshot), &book, &view);
        let text = |p: &str| ui.text(p).unwrap_or_default();
        let advised = [text("main.pma.advice.c0"), text("main.pma.advice.c1")];
        assert!(advised.contains(&name_ref("c")), "the strong champion is advised: {advised:?}");
        assert!(text(&format!("{GRID}.c.{TAG}.text")).starts_with('▲'), "c is worth picking");
        assert!(ui.nodes[&format!("{GRID}.c.{TAG}.stripe")].props.iter().any(|p| p.contains(&color(GOOD))));
        // a lane read on the enemy's first pick slot: an icon and a confidence bar
        let lane = format!("main.red_picks.pick_slot_0.{LANE_TAG}");
        assert!(ui.nodes[&format!("{lane}.icon")].props.iter().any(|p| p.contains("asset/base/ui/icons/")));
        assert!(ui.exists(&format!("{lane}.s4")));
        assert!(!ui.exists(&format!("main.red_picks.pick_slot_1.{LANE_TAG}")));

        // nothing changed: nothing redrawn; the game rebuilt the screen: drawn again
        let spawned = ui.spawned.len();
        screen.tick(&mut ui, 2 * READ_EVERY, Some(&snapshot), &book, &view);
        assert_eq!(ui.spawned.len(), spawned);
        ui.remove("main.pma");
        screen.tick(&mut ui, 3 * READ_EVERY, Some(&snapshot), &book, &view);
        assert!(ui.exists("main.pma.win.value"));
    }

    #[test]
    fn panel_opacity() {
        assert_eq!(panel_color(100.0), 0x161721ff);
        assert_eq!(panel_color(60.0), 0x16172199);
        assert_eq!(panel_color(0.0), 0x16172100);
        assert_eq!(panel_color(250.0), 0x161721ff, "clamped");
    }

    #[test]
    fn the_swap_phase_shows_the_best_seating() {
        let snapshot = snapshot(1500);
        let book = NameBook::default();
        let rosters = HashMap::new();
        let (no_picks, no_matches) = (HashMap::new(), HashMap::new());
        let view = View { team_name: "Mods FC", rosters: &rosters, team_picks: &no_picks, team_matches: &no_matches, grid_values: true, lane_tags: true, panel_opacity: 60.0 };
        let mut ui = screen();
        for (i, n) in ["a", "b", "c", "d", "e"].iter().enumerate() {
            pick(&mut ui, n, "blue", i as u32 + 1);
        }
        for (i, n) in ["f", "g", "h", "i", "j"].iter().enumerate() {
            pick(&mut ui, n, "red", i as u32 + 1);
        }
        ui.add(SWAP, "empty");
        for (i, r) in Role::ALL.iter().enumerate() {
            ui.add(&format!("{}.swap_slot_{i}.data.position_name", SWAP_TABLES[0]), "label").text =
                Some(format!("#asset/base/text/ui?position.{}", r.name().to_lowercase()));
        }
        let mut screen = DraftScreen::default();
        screen.tick(&mut ui, 0, Some(&snapshot), &book, &view);
        assert_eq!(ui.visible("main.pma.advice.seating"), Some(true));
        // every pick in: both panels' background fades to a third (60% -> 20%, alpha 0x33)
        for panel in ["main.pma.win", "main.pma.advice"] {
            let props = &ui.nodes[panel].props;
            assert_eq!(props.last().map(String::as_str), Some(format!("color: {};", color(0x16172133)).as_str()), "{panel}");
        }
        assert!(ui.text("main.pma.advice.seating.title").unwrap().starts_with("Best seating"));
        let seated: Vec<String> = (0..5).map(|k| ui.text(&format!("main.pma.advice.seating.s{k}.name")).unwrap()).collect();
        for n in ["a", "b", "c", "d", "e"] {
            assert!(seated.contains(&name_ref(n)), "{n} seated: {seated:?}");
        }
        assert_eq!(ui.text("main.pma.advice.c0").unwrap_or_default(), "", "no pick advice in the swap phase");
        assert_eq!(screen.last_picks.as_ref().map(|p| p.0.len()), Some(5), "the picks are kept for the tactics screen");
        // the swap ends: back to advice
        ui.nodes.get_mut(SWAP).unwrap().visible = false;
        screen.tick(&mut ui, READ_EVERY, Some(&snapshot), &book, &view);
        assert_eq!(ui.visible("main.pma.advice.seating"), Some(false));
    }

    #[test]
    fn says_so_when_there_is_too_little_data() {
        let book = NameBook::default();
        let rosters = HashMap::new();
        let (no_picks, no_matches) = (HashMap::new(), HashMap::new());
        let view = View { team_name: "Mods FC", rosters: &rosters, team_picks: &no_picks, team_matches: &no_matches, grid_values: true, lane_tags: true, panel_opacity: 60.0 };
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
