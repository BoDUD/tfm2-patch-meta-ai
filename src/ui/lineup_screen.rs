//! The line-up screen before a match: the five players the model would field, one per position
//! (`plan::best_lineup`: own strength, position rating, champion pool there with mastery and
//! proficiency). Each of them gets a chip on their roster row - a star and the position - and,
//! when the roster leaves room below its rows, a panel lists them with their best champions.
//!
//! Roster rows are `main.roaster.scroll.contents.athlete_<id>` (56 px each from y 122), so the
//! athletes are known by id; their records are read by the client a couple per frame.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use super::names::name_ref;
use super::{color, quote, Ui};
use crate::glm::sigmoid;
use crate::history::Role;
use crate::plan::{self, AthleteInfo};
use crate::shared::Snapshot;

const ROWS: &str = "main.roaster.scroll.contents";
const SELECTED: &str = "main.selected.list";
const CHIP: &str = "pma_star";
const PANEL: &str = "main.pma_lineup";
const EVERY: u64 = 15;
/// The roster panel's rows start here and end at 832.
const ROWS_TOP: i32 = 122;
const ROW_H: i32 = 56;
const ROSTER_BOTTOM: i32 = 832;
const PANEL_H: i32 = 300;

#[derive(Default)]
pub struct LineupScreen {
    next: u64,
    drawn: Option<u64>,
}

fn lane_icon(r: Role) -> String {
    format!("asset/base/ui/icons/{}", r.name().to_lowercase())
}

fn chip_source() -> String {
    format!(
        "{CHIP}:color {{ x: 506px; y: 14px; width: 92px; height: 28px; color: #0f3d2fe0; ignore_event: true; \
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
        let healthy = ui.exists(&format!("{ROWS}.{}.{CHIP}", rows[0].0));
        if self.drawn == Some(key) && healthy {
            return;
        }
        let Some(lineup) = plan::best_lineup(&snapshot.meta, &squad) else { return };
        self.drawn = Some(key);
        let role_of: HashMap<u32, Role> = Role::ALL.iter().zip(lineup).map(|(r, (a, _))| (a, *r)).collect();
        for (row, id) in &rows {
            let chip = format!("{ROWS}.{row}.{CHIP}");
            if !ui.exists(&chip) && !ui.spawn(&format!("{ROWS}.{row}"), &chip_source()) {
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
        // the panel, when the roster leaves room below its rows
        ui.remove(PANEL);
        let top = ROWS_TOP + ROW_H * rows.len() as i32 + 16;
        if top + PANEL_H > ROSTER_BOTTOM {
            return;
        }
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
                "pma_lineup:color {{ x: 1412px; y: {top}px; width: 460px; height: {PANEL_H}px; color: #1d1f2cf0; ignore_event: true; \
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
        let meta = build(&Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.1", warm: None }, &Settings::default());
        let snapshot = Snapshot { meta, damage: HashMap::new() };
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
        crate::ui::reset();
    }
}
