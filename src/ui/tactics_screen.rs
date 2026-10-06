//! The tactics (team strategy) screen before a match: on each strategy group, a chip on the
//! option the model rates best for the team's five champions - "★ +1.2", its worth over the
//! group's average option in win-rate points - and a note in the free strip at the top. The
//! screen shows the champions only as images, so the team is the one the ban/pick screen read.
//!
//! The screen's groups are `main.contents.strategy.<column>.<group>` with one
//! `color_selectable` per option at `.bg.<option>`; the option names are the records' option
//! names in snake case (`CounterJungle` -> `counter_jungle`).

use std::hash::{Hash, Hasher};

use super::{color, quote, Ui};
use crate::glm::sigmoid;
use crate::plan::{self, snake};
use crate::shared::Snapshot;

const BASE: &str = "main.contents.strategy";
const NOTE: &str = "main.pma_tactics";
const CHIP: &str = "pma_tip";
const EVERY: u64 = 15;
/// Advice under this many win-rate points is not shown.
const MIN_POINTS: f32 = 0.3;

/// The records' strategy settings and their groups on the screen.
const GROUPS: [(&str, &str); 12] = [
    ("focused", "sub1.focused_area"),
    ("early_jungle", "sub1.early_jungle"),
    ("early_serpen", "sub1.early_serpen"),
    ("early_serpen_top", "sub1.early_serpen_top"),
    ("minion_wave", "sub2.minion_wave"),
    ("object_buildup", "sub2.object_buildup"),
    ("object_battle", "sub2.object_battle"),
    ("object_finish", "sub2.object_finish"),
    ("morgard_use", "sub3.morgard_use"),
    ("tower_press", "sub3.tower_press"),
    ("morgard_defense", "sub3.defense"),
    ("game_finish", "sub4.game_finish"),
];

#[derive(Default)]
pub struct TacticsScreen {
    next: u64,
    drawn: Option<u64>,
    chips: Vec<String>,
}

fn points(v: f32) -> f32 {
    (sigmoid(v) - 0.5) * 100.0
}

fn chip_source(text: &str) -> String {
    format!(
        "{CHIP}:label {{ @\"asset/base/style/main#bold_label\"; anchor_x: 1; pivot_x: 1; x: -10px; y: 0px; width: 110px; \
         height: 32px; size: 14; color: {}; align_x: Right; align_y: Center; ignore_event: true; text: {}; }}",
        color(0x4cc38aff),
        quote(text)
    )
}

impl TacticsScreen {
    /// One frame. `team`: the champions the player's team picked (from the ban/pick screen).
    pub fn tick(&mut self, ui: &mut impl Ui, frame: u64, snapshot: Option<&Snapshot>, team: Option<&[u16]>) {
        if !ui.exists(&format!("{BASE}.sub1")) {
            if self.drawn.is_some() {
                self.drawn = None;
                self.chips.clear();
            }
            return;
        }
        if frame < self.next {
            return;
        }
        self.next = frame + EVERY;
        let (Some(snapshot), Some(team)) = (snapshot, team) else { return };
        if team.len() < 5 {
            return;
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (team, snapshot.meta.matches, snapshot.meta.solo_matches).hash(&mut h);
        let key = h.finish();
        let healthy = self.chips.iter().all(|c| ui.exists(c)) && ui.exists(NOTE);
        if self.drawn == Some(key) && healthy {
            return;
        }
        for c in self.chips.drain(..) {
            ui.remove(&c);
        }
        ui.remove(NOTE);
        let advice = plan::tactics_for(&snapshot.meta, team);
        let mut shown = 0;
        for a in &advice {
            let pts = points(a.gain);
            if pts < MIN_POINTS {
                continue;
            }
            let Some((_, group)) = GROUPS.iter().find(|(s, _)| *s == a.setting) else { continue };
            let option = format!("{BASE}.{group}.bg.{}", snake(&a.option));
            if !ui.exists(&option) || !ui.spawn(&option, &chip_source(&format!("★ +{pts:.1}"))) {
                continue;
            }
            self.chips.push(format!("{option}.{CHIP}"));
            shown += 1;
        }
        let note = if shown == 0 {
            "Patch Meta: no strategy stands out for this line-up / 本局阵容没有明显更优的战术".to_string()
        } else {
            "Patch Meta: ★ = best for this line-up (win-rate points) / ★ = 按本局阵容推荐（胜率百分点）".to_string()
        };
        ui.spawn(
            "main",
            &format!(
                "pma_tactics:label {{ @\"asset/base/style/main#label\"; x: 1060px; y: 128px; width: 810px; height: 32px; size: 14; \
                 color: {}; align_x: Right; align_y: Center; ignore_event: true; text: {}; }}",
                color(0xa3a9b6ff),
                quote(&note)
            ),
        );
        self.drawn = Some(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glm::tests::Lcg;
    use crate::meta::tests::{simulate, NAMES};
    use crate::meta::{build, Inputs, Settings};
    use crate::ui::tests::FakeUi;
    use std::collections::HashMap;

    #[test]
    fn marks_the_best_option_of_each_group() {
        let (names, mut games) = simulate(2500, "1.1", |_| 0.0, 31);
        let (gank, farm) = (crate::history::tactic_id("early_jungle", "Ganking"), crate::history::tactic_id("early_jungle", "GrowthAndCover"));
        let mut rng = Lcg(2);
        for g in &mut games {
            let (b, r) = (rng.next() < 0.5, rng.next() < 0.5);
            g.tactics = [vec![if b { gank } else { farm }], vec![if r { gank } else { farm }]];
            if b != r {
                g.blue_win = rng.next() < if b { 0.65 } else { 0.35 };
            }
        }
        let champions: Vec<String> = NAMES.iter().map(|s| s.to_string()).collect();
        let meta = build(&Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.1", warm: None }, &Settings::default());
        let snapshot = Snapshot { meta, damage: HashMap::new() };
        let team: Vec<u16> = (0..5).collect();
        let mut ui = FakeUi::default();
        ui.add("main", "strategy_ui");
        ui.add(&format!("{BASE}.sub1"), "empty");
        for option in ["growth_and_cover", "ganking", "counter_jungle"] {
            ui.add(&format!("{BASE}.sub1.early_jungle.bg.{option}"), "color_selectable");
        }
        let mut screen = TacticsScreen::default();
        screen.tick(&mut ui, 0, Some(&snapshot), Some(&team));
        assert!(ui.exists(&format!("{BASE}.sub1.early_jungle.bg.ganking.{CHIP}")), "the winning option is marked");
        assert!(!ui.exists(&format!("{BASE}.sub1.early_jungle.bg.growth_and_cover.{CHIP}")));
        let chip = ui.spawned.iter().find(|(p, _)| p.ends_with("ganking")).unwrap();
        assert!(chip.1.contains("★ +"), "{}", chip.1);
        assert!(ui.exists(NOTE));
        // nothing new: nothing spawned again; no team known: nothing at all
        let spawned = ui.spawned.len();
        screen.tick(&mut ui, EVERY, Some(&snapshot), Some(&team));
        assert_eq!(ui.spawned.len(), spawned);
        let mut empty = FakeUi::default();
        empty.add("main", "strategy_ui");
        empty.add(&format!("{BASE}.sub1"), "empty");
        TacticsScreen::default().tick(&mut empty, 0, Some(&snapshot), None);
        assert!(empty.spawned.is_empty());
    }
}
