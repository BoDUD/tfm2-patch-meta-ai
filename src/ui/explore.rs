//! The UI explorer: writes the live node tree of a screen to `ui_dump_<n>.txt` in the mod folder:
//! every node's path, kind, visibility, rectangle, text and runner state. It runs when a screen
//! the session has not seen appears (`explore=on` in settings.ini) and whenever F9 is pressed,
//! and walks a few hundred nodes per frame so a big screen costs no visible hitch.

use std::collections::{HashSet, VecDeque};

use super::Ui;
use crate::diag;

const NODES_PER_FRAME: usize = 250;
const MAX_NODES: usize = 6000;
const MAX_DUMPS: u32 = 20;
const LOOK_EVERY: u64 = 30;
pub const HOTKEY: &str = "F9";

#[derive(Default)]
pub struct Explorer {
    seen: HashSet<String>,
    dumps: u32,
    running: Option<Dump>,
    next_look: u64,
}

struct Dump {
    file: String,
    reason: String,
    queue: VecDeque<(String, usize)>,
    lines: Vec<String>,
    nodes: usize,
}

/// The screen's identity: the root's children and theirs.
fn signature(ui: &impl Ui) -> String {
    let mut parts = Vec::new();
    for root in ui.children("") {
        let mut kids = ui.children(&root);
        kids.sort();
        kids.truncate(40);
        parts.push(format!("{root}[{}]", kids.join(",")));
    }
    parts.sort();
    parts.join(" ")
}

fn clip(text: &str, max: usize) -> String {
    diag::clip(&text.replace('\n', "\\n"), max)
}

impl Explorer {
    /// One frame. `auto` = dump screens not seen before; `scene` labels the dump.
    pub fn tick(&mut self, ui: &impl Ui, frame: u64, auto: bool, scene: &str, keys: &[String]) {
        let manual = keys.iter().any(|k| k.eq_ignore_ascii_case(HOTKEY));
        if self.running.is_none() && (manual || (auto && frame >= self.next_look)) {
            self.next_look = frame + LOOK_EVERY;
            let sig = signature(ui);
            let new = !sig.is_empty() && self.seen.insert(sig.clone());
            if (manual || new) && self.dumps < MAX_DUMPS {
                self.dumps += 1;
                let file = format!("ui_dump_{:02}.txt", self.dumps);
                let reason = if manual { format!("{HOTKEY} pressed") } else { "new screen".to_string() };
                let mut queue = VecDeque::new();
                for root in ui.children("") {
                    queue.push_back((root, 0));
                }
                diag::log(&format!("[ui] writing {file} ({reason}, scene {scene}): {}", clip(&sig, 300)));
                self.running = Some(Dump { file, reason: format!("{reason}, scene {scene}"), queue, lines: Vec::new(), nodes: 0 });
            }
        }
        if let Some(dump) = &mut self.running {
            if step(ui, dump) {
                let header = format!(
                    "{} {} - UI tree ({}), {} nodes\npath [kind] visible rect(x,y,w,h) text=... state=...\n\n",
                    crate::MOD_ID,
                    crate::VERSION,
                    dump.reason,
                    dump.nodes
                );
                diag::write_file(&dump.file, &(header + &dump.lines.join("\n")));
                self.running = None;
            }
        }
    }
}

/// Walks some nodes; true when the dump is complete.
fn step(ui: &impl Ui, dump: &mut Dump) -> bool {
    for _ in 0..NODES_PER_FRAME {
        let Some((path, depth)) = dump.queue.pop_front() else { return true };
        if dump.nodes >= MAX_NODES {
            dump.lines.push(format!("... stopped at {MAX_NODES} nodes"));
            return true;
        }
        dump.nodes += 1;
        let mut line = format!("{}{} [{}]", "  ".repeat(depth.min(20)), path, ui.runner(&path).unwrap_or_default());
        if let Some(v) = ui.visible(&path) {
            if !v {
                line.push_str(" hidden");
            }
        }
        if let Some((x, y, w, h)) = ui.rect(&path) {
            line.push_str(&format!(" ({x:.0},{y:.0},{w:.0},{h:.0})"));
        }
        if let Some(t) = ui.text(&path).filter(|t| !t.is_empty()) {
            line.push_str(&format!(" text={:?}", clip(&t, 80)));
        }
        if let Some(s) = ui.state(&path).filter(|s| !s.is_empty() && s != "{}" && s != "null") {
            line.push_str(&format!(" state={}", clip(&s, 400)));
        }
        dump.lines.push(line);
        for child in ui.children(&path) {
            dump.queue.push_back((format!("{path}.{child}"), depth + 1));
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::FakeUi;

    #[test]
    fn dumps_new_screens_once_and_on_the_hotkey() {
        let dir = std::env::temp_dir().join(format!("patch_meta_ai_explore_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let _serial = crate::tests::serial();
        std::env::set_var(crate::paths::DIR_ENV, &dir);
        crate::diag::open(&dir);
        let mut ui = FakeUi::default();
        ui.add("main", "match_ui");
        ui.add("main.champions", "scroll_view");
        ui.add("main.champions.contents", "empty");
        ui.add("main.champions.contents.0", "banpick_champion_slot").state = Some(r#"{"champion":"ahri"}"#.into());
        ui.add("main.champions.contents.0.name", "label").text = Some("Ahri".into());
        let mut ex = Explorer::default();
        for frame in 0..10 {
            ex.tick(&ui, frame, true, "Match", &ui.keys.clone());
        }
        let dump = std::fs::read_to_string(dir.join("ui_dump_01.txt")).unwrap();
        assert!(dump.contains("main.champions.contents.0 [banpick_champion_slot]"), "{dump}");
        assert!(dump.contains("state={\"champion\":\"ahri\"}") && dump.contains("text=\"Ahri\""), "{dump}");
        // the same screen again: no new dump; F9: one
        for frame in 10..100 {
            ex.tick(&ui, frame, true, "Match", &ui.keys.clone());
        }
        assert!(!dir.join("ui_dump_02.txt").exists());
        ui.keys = vec!["F9".into()];
        ex.tick(&ui, 100, true, "Match", &ui.keys.clone());
        ui.keys.clear();
        ex.tick(&ui, 101, true, "Match", &ui.keys.clone());
        assert!(dir.join("ui_dump_02.txt").exists());
    }
}
