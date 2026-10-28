//! The line-up screen before a match: the five players the model would field, one per position
//! (`plan::best_lineup`: own strength, position rating, champion pool there with mastery and
//! proficiency). Each of them gets a chip on their roster row - a star and the position - and,
//! when the roster leaves room below its rows, a panel lists them with their best champions.
//!
//! Roster rows are `main.roaster.scroll.contents.athlete_<id>`, so the athletes are known by id;
//! their records are read by the client a couple per frame. The chip and the panel are placed from
//! the game's rectangles: the chip over the right end of the row's position column (game 0.7.0 put
//! an OVR column before it), and the console layout's rows are taller and its roster smaller.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use super::names::name_ref;
use super::{color, quote, Ui};
use crate::glm::sigmoid;
use crate::history::Role;
use crate::plan::{self, AthleteInfo};
use crate::shared::Snapshot;

const ROWS: &str = "main.roaster.scroll.contents";
const ROSTER: &str = "main.roaster";
const SELECTED: &str = "main.selected.list";
const CHIP: &str = "pma_star";
const PANEL: &str = "main.pma_lineup";
const EVERY: u64 = 15;
/// The PC layout's roster (game 0.7.0), while the game's rectangles are not known: rows of 56 px
/// from y 122, the roster ending at 832 and 1896, a row's position column ending 824 from its corner.
const ROWS_TOP: i32 = 122;
const ROW_H: i32 = 56;
const ROSTER_BOTTOM: i32 = 832;
const ROSTER_RIGHT: i32 = 1896;
const POSITION_END: i32 = 824;
const CHIP_W: i32 = 92;
const CHIP_H: i32 = 28;
const PANEL_W: i32 = 460;
const PANEL_H: i32 = 300;

#[derive(Default)]
pub struct LineupScreen {
    next: u64,
    drawn: Option<u64>,
    /// Where the chips were put in their rows.
    chip_at: Option<(i32, i32)>,
}

/// Where things go on the roster, from the game's rectangles of its first row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Geometry {
    /// The chip in a row, from the row's corner.
    chip: (i32, i32),
    rows_top: i32,
    row_h: i32,
    bottom: i32,
    right: i32,
}

fn geometry(ui: &impl Ui, row: &str) -> Geometry {
    let known = |r: Option<(f32, f32, f32, f32)>| r.filter(|r| r.2 > 0.0 && r.3 > 0.0);
    let round = |v: f32| v.round() as i32;
    let row_rect = known(ui.rect(row));
    let (rows_top, row_h) = row_rect.map_or((ROWS_TOP, ROW_H), |r| (round(r.1), round(r.3)));
    let end = match (row_rect, known(ui.rect(&format!("{row}.data.main_position")))) {
        (Some(r), Some(p)) => round(p.0 + p.2 - r.0),
        _ => POSITION_END,
    };
    let (bottom, right) = known(ui.rect(ROSTER)).map_or((ROSTER_BOTTOM, ROSTER_RIGHT), |r| (round(r.1 + r.3), round(r.0 + r.2)));
    Geometry { chip: (end - CHIP_W - 6, (row_h - CHIP_H) / 2), rows_top, row_h, bottom, right }
}

fn lane_icon(r: Role) -> String {
    format!("asset/base/ui/icons/{}", r.name().to_lowercase())
}

fn chip_source((x, y): (i32, i32)) -> String {
    format!(
        "{CHIP}:color {{ x: {x}px; y: {y}px; width: {CHIP_W}px; height: {CHIP_H}px; color: #0f3d2fe0; ignore_event: true; \
         rounding: Uniform {{ rounding: 6; }} \
         #icon:image {{ x: 6px; y: 6px; width: 16px; height: 16px; source: \"asset/base/ui/icons/fi-sr-star\"; color: {}; ignore_event: true; }} \
         #lane:label {{ @\"asset/base/style/main#bold_label\"; x: 26px; y: 0px; width: 62px; height: 28px; size: 13; color: {}; \
         align_x: Left; align_y: Center; ignore_event: true; text: \"\"; }} }}",
        color(0x4cc38aff),
        color(0x4cc38aff)
    )
}

impl LineupScreen {
    pub fn tick(&mut self, ui: &mut impl Ui, frame: u64, snapshot: Option<&Snapshot>) {
        if !ui.exists(ROWS) || !ui.exists(SELECTED) {
            self.drawn = None;
            return;
        }
        if frame < self.next {
            return;
        }
        self.next = frame + EVERY;
        let rows: Vec<(String, u32)> = ui
            .children(ROWS)
            .into_iter()
            .filter_map(|r| Some((r.clone(), r.strip_prefix("athlete_")?.parse().ok()?)))
            .collect();
        let ids: Vec<u32> = rows.iter().map(|(_, id)| *id).collect();
        super::want_athletes(&ids);
        let Some(snapshot) = snapshot else { return };
        if rows.len() < 5 {
            return;
        }
        let infos = super::athlete_infos();
        let squad: Vec<(u32, Option<&AthleteInfo>)> = ids.iter().map(|id| (*id, infos.get(id))).collect();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&ids, snapshot.meta.matches, snapshot.meta.solo_matches, squad.iter().filter(|s| s.1.is_some()).count()).hash(&mut h);
        let key = h.finish();
        let first = format!("{ROWS}.{}", rows[0].0);
        let g = geometry(ui, &first);
        let healthy = ui.exists(&format!("{first}.{CHIP}"));
        if self.drawn == Some(key) && healthy && self.chip_at == Some(g.chip) {
            return;
        }
        let Some(lineup) = plan::best_lineup(&snapshot.meta, &squad) else { return };
        self.drawn = Some(key);
        let role_of: HashMap<u32, Role> = Role::ALL.iter().zip(lineup).map(|(r, (a, _))| (a, *r)).collect();
        for (row, id) in &rows {
            let chip = format!("{ROWS}.{row}.{CHIP}");
            if ui.exists(&chip) {
                if self.chip_at != Some(g.chip) {
                    ui.set_properties(&chip, &format!("x: {}px; y: {}px;", g.chip.0, g.chip.1));
                }
            } else if !ui.spawn(&format!("{ROWS}.{row}"), &chip_source(g.chip)) {
                continue;
            }
            match role_of.get(id) {
                Some(r) => {
                    ui.set_visible(&chip, true);
                    ui.set_text(&format!("{chip}.lane"), &format!("#asset/base/text/ui?position.{}", r.name().to_lowercase()));
                }
                None => {
                    ui.set_visible(&chip, false);
                }
            }
        }
        self.chip_at = Some(g.chip);
        // the panel, when the roster leaves room below its rows
        ui.remove(PANEL);
        let top = g.rows_top + g.row_h * rows.len() as i32 + 16;
        if top + PANEL_H > g.bottom {
            return;
        }
        let left = g.right - 24 - PANEL_W;
        let meta = &snapshot.meta;
        let mut body = format!(
            "#title:label {{ @\"asset/base/style/main#bold_label\"; x: 16px; y: 10px; width: 428px; height: 26px; size: 17; \
             color: #e8e8e8ff; align_y: Center; ignore_event: true; text: {}; }} ",
            quote("Suggested starters 推荐首发")
        );
        for (k, (r, (a, value))) in Role::ALL.iter().zip(lineup).enumerate() {
            let y = 44 + k as i32 * 50;
            let name = infos.get(&a).map_or_else(|| format!("#{a}"), |i| i.name.clone());
            let pts = (sigmoid(value) - 0.5) * 100.0;
            body.push_str(&format!(
                "#r{k}:empty {{ x: 0px; y: {y}px; width: 460px; height: 46px; ignore_event: true; \
                 #lane:image {{ x: 16px; y: 13px; width: 20px; height: 20px; source: \"{}\"; color: #c2c6ceff; ignore_event: true; }} \
                 #name:label {{ @\"asset/base/style/main#bold_label\"; x: 44px; y: 0px; width: 130px; height: 46px; size: 16; \
                 color: #e8e8e8ff; align_y: Center; ignore_event: true; text: {}; }} \
                 #v:label {{ @\"asset/base/style/main#label\"; x: 172px; y: 0px; width: 52px; height: 46px; size: 13; color: {}; \
                 align_x: Right; align_y: Center; ignore_event: true; text: {}; }} ",
                lane_icon(*r),
                quote(&name),
                color(if pts >= 0.0 { 0x4cc38aff } else { 0xef6471ff }),
                quote(&format!("{}{pts:.1}", if pts >= 0.0 { "+" } else { "" }))
            ));
            for (j, (c, _)) in plan::pool(meta, a, infos.get(&a), *r, 2).iter().enumerate() {
                body.push_str(&format!(
                    "#c{j}:label {{ @\"asset/base/style/main#label\"; x: {}px; y: 0px; width: 106px; height: 46px; size: 13; \
                     color: #a3a9b6ff; align_y: Center; ignore_event: true; text: {}; }} ",
                    236 + j as i32 * 110,
                    quote(&name_ref(meta.names.name(*c)))
                ));
            }
            body.push_str("} ");
        }
        ui.spawn(
            "main",
            &format!(
                "pma_lineup:color {{ x: {left}px; y: {top}px; width: {PANEL_W}px; height: {PANEL_H}px; color: #1d1f2cf0; ignore_event: true; \
                 rounding: Uniform {{ rounding: 10; }} {body}}}"
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::tests::{simulate, NAMES};
    use crate::meta::{build, Inputs, Settings};
    use crate::ui::tests::FakeUi;

    #[test]
    fn stars_the_suggested_starters() {
        let _serial = crate::tests::serial();
        crate::ui::reset();
        let (names, games) = simulate(1500, "1.1", |_| 0.0, 12);
        let champions: Vec<String> = NAMES.iter().map(|s| s.to_string()).collect();
        let meta = build(&Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.1", warm: None, stats: None }, &Settings::default());
        let snapshot = Snapshot::new(meta, HashMap::new());
        let mut ui = FakeUi::default();
        ui.add("main", "lineup_ui");
        ui.add(SELECTED, "empty");
        ui.add(ROWS, "empty");
        // six athletes; 1..5 rated for one position each, 6 a second top laner
        for (i, id) in (1..=6u32).enumerate() {
            ui.add(&format!("{ROWS}.athlete_{id}"), "empty");
            let mut positions = [0u8; 5];
            positions[i % 5] = 100;
            crate::ui::learn_athlete(id, AthleteInfo { name: format!("P{id}"), positions, proficiency: HashMap::new() });
        }
        let mut screen = LineupScreen::default();
        screen.tick(&mut ui, 0, Some(&snapshot));
        let starred: Vec<u32> = (1..=6)
            .filter(|id| ui.visible(&format!("{ROWS}.athlete_{id}.{CHIP}")) == Some(true))
            .collect();
        assert_eq!(starred.len(), 5, "{starred:?}");
        assert!(starred.contains(&2) && starred.contains(&5), "the only jungler and support start");
        assert!(ui.exists(PANEL), "six rows leave room for the panel");
        assert!(ui.spawned.iter().any(|(_, s)| s.contains("Suggested starters")));
        let chip = &ui.spawned.iter().find(|(p, _)| p.ends_with("athlete_1")).unwrap().1;
        assert!(chip.starts_with("pma_star:color { x: 726px; y: 14px;"), "the PC layout's while nothing is laid out: {chip}");
        let panel = &ui.spawned.iter().find(|(_, s)| s.starts_with("pma_lineup")).unwrap().1;
        assert!(panel.starts_with("pma_lineup:color { x: 1412px; y: 474px;"), "{panel}");
        crate::ui::reset();
    }

    /// The roster as the game lays it out: `rows` rows, the first at `first` (x, y, width, height) and the others
    /// under it, the position column at `position` (x, width) from a row's corner, the roster ending at `bottom`.
    fn laid_out(ui: &mut FakeUi, rows: u32, first: (f32, f32, f32, f32), position: (f32, f32), bottom: f32) {
        let (x, top, width, row_h) = first;
        ui.add(ROSTER, "color").rect = (x, top - 56.0, width, bottom - top + 56.0);
        for (i, id) in (1..=rows).enumerate() {
            let y = top + row_h * i as f32;
            ui.add(&format!("{ROWS}.athlete_{id}"), "empty").rect = (x, y, width, row_h);
            ui.add(&format!("{ROWS}.athlete_{id}.data.main_position"), "empty").rect = (x + position.0, y, position.1, row_h);
            let mut positions = [0u8; 5];
            positions[i % 5] = 100;
            crate::ui::learn_athlete(id, AthleteInfo { name: format!("P{id}"), positions, proficiency: HashMap::new() });
        }
    }

    #[test]
    fn the_chip_sits_on_the_position_column_in_both_layouts() {
        let _serial = crate::tests::serial();
        crate::ui::reset();
        let (names, games) = simulate(1500, "1.1", |_| 0.0, 12);
        let champions: Vec<String> = NAMES.iter().map(|s| s.to_string()).collect();
        let meta = build(&Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.1", warm: None, stats: None }, &Settings::default());
        let snapshot = Snapshot::new(meta, HashMap::new());
        let base = || {
            let mut ui = FakeUi::default();
            ui.add("main", "lineup_ui");
            ui.add(SELECTED, "empty");
            ui.add(ROWS, "empty");
            ui
        };

        // PC (game 0.7.0): OVR before the position column, 664..848 on screen
        let mut ui = base();
        laid_out(&mut ui, 6, (24.0, 122.0, 1872.0, 56.0), (640.0, 184.0), 832.0);
        LineupScreen::default().tick(&mut ui, 0, Some(&snapshot));
        let chip = &ui.spawned.iter().find(|(p, _)| p.ends_with("athlete_1")).unwrap().1;
        assert!(chip.starts_with("pma_star:color { x: 726px; y: 14px;"), "{chip}");
        assert!(ui.exists(PANEL));

        // console: taller rows, a smaller roster - no room for the panel
        let mut ui = base();
        laid_out(&mut ui, 6, (64.0, 168.0, 1792.0, 64.0), (760.0, 210.0), 732.0);
        let mut screen = LineupScreen::default();
        screen.tick(&mut ui, 0, Some(&snapshot));
        let chip = &ui.spawned.iter().find(|(p, _)| p.ends_with("athlete_1")).unwrap().1;
        assert!(chip.starts_with("pma_star:color { x: 872px; y: 18px;"), "{chip}");
        assert!(!ui.exists(PANEL));

        // laid out again (the layout switched under the same rows): the chips move
        for id in 1..=6 {
            let p = ui.nodes.get_mut(&format!("{ROWS}.athlete_{id}.data.main_position")).unwrap();
            p.rect.0 = 64.0 + 640.0;
            p.rect.2 = 184.0;
        }
        let spawned = ui.spawned.len();
        screen.tick(&mut ui, EVERY, Some(&snapshot));
        assert_eq!(ui.nodes[&format!("{ROWS}.athlete_3.{CHIP}")].props.first().map(String::as_str), Some("x: 726px; y: 18px;"));
        assert_eq!(ui.spawned.len(), spawned, "moved, not spawned again");
        crate::ui::reset();
    }
}
