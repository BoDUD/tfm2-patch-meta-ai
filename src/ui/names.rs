//! Champion names as the screen shows them, in the game's language, mapped back to champion ids.
//! `i18n` lookups from a mod always answer in English, but a label given the game's name
//! reference (`#asset/base/text/champion?description.<id>.name`) shows the name in the language
//! the game runs in. So once per session every champion's reference is written into hidden
//! labels, a batch per frame, and read back. When the read-back is the reference itself (the
//! host does not resolve it on read), the English names from `i18n` are used instead.

use std::collections::HashMap;

use super::Ui;
use crate::diag;

const BATCH: usize = 25;
const NODE: &str = "pma_names";

#[derive(Default)]
pub struct NameBook {
    state: State,
    by_text: HashMap<String, String>,
    /// Names came back in the game's language (not just the English fallback).
    pub localized: bool,
}

#[derive(Default)]
enum State {
    #[default]
    Idle,
    Writing { parent: String, todo: Vec<String>, batch: Vec<String>, at: u64, resolved: usize, read: usize },
    Ready,
}

/// The game's reference to a champion's display name.
pub fn name_ref(champion: &str) -> String {
    format!("#asset/base/text/champion?description.{champion}.name")
}

fn normalize(text: &str) -> String {
    text.trim().to_lowercase()
}

impl NameBook {
    pub fn ready(&self) -> bool {
        matches!(self.state, State::Ready)
    }

    /// The champion id a name on screen belongs to.
    pub fn lookup(&self, text: &str) -> Option<&str> {
        self.by_text.get(&normalize(text)).map(String::as_str)
    }

    /// Adds names known another way (English `i18n` names, the ids themselves).
    pub fn learn(&mut self, text: &str, champion: &str) {
        if !text.is_empty() {
            self.by_text.entry(normalize(text)).or_insert_with(|| champion.to_string());
        }
    }

    pub fn tick(&mut self, ui: &mut impl Ui, frame: u64, champions: &[String]) {
        match &mut self.state {
            State::Idle => {
                if champions.is_empty() {
                    return;
                }
                let Some(parent) = ui.children("").into_iter().next() else { return };
                let mut source = format!("{NODE}:empty {{ visible: false; ignore_event: true; ");
                for i in 0..BATCH {
                    source.push_str(&format!("#n{i}:label {{ @\"asset/base/style/main#label\"; text: \"\"; }} "));
                }
                source.push('}');
                if !ui.spawn(&parent, &source) {
                    diag::log_once("names-spawn", "[ui] names: could not add hidden labels; using English names");
                    self.state = State::Ready;
                    return;
                }
                let mut todo = champions.to_vec();
                todo.reverse();
                self.state = State::Writing { parent, todo, batch: Vec::new(), at: frame, resolved: 0, read: 0 };
            }
            State::Writing { parent, todo, batch, at, resolved, read } => {
                if !batch.is_empty() {
                    if frame < *at + 2 {
                        return;
                    }
                    for (i, champ) in batch.iter().enumerate() {
                        let got = ui.text(&format!("{parent}.{NODE}.n{i}")).unwrap_or_default();
                        *read += 1;
                        if !got.is_empty() && !got.starts_with('#') {
                            *resolved += 1;
                            self.by_text.insert(normalize(&got), champ.clone());
                        }
                    }
                    batch.clear();
                    // a host that does not resolve references: stop after the first batch
                    if *resolved == 0 {
                        todo.clear();
                    }
                }
                if todo.is_empty() {
                    diag::log(&format!(
                        "[ui] names: {resolved} of {read} champion names read in the game's language{}",
                        if *resolved == 0 { " (references are not resolved on read; using English names)" } else { "" }
                    ));
                    self.localized = *resolved > 0;
                    ui.remove(&format!("{parent}.{NODE}"));
                    self.state = State::Ready;
                    return;
                }
                for i in 0..BATCH {
                    let Some(champ) = todo.pop() else { break };
                    ui.set_text(&format!("{parent}.{NODE}.n{i}"), &name_ref(&champ));
                    batch.push(champ);
                }
                *at = frame;
            }
            State::Ready => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::FakeUi;

    #[test]
    fn learns_names_in_batches_and_falls_back() {
        let champions: Vec<String> = (0..60).map(|i| format!("c{i}")).collect();
        // a host that resolves references (simulated: the test rewrites the text)
        let mut ui = FakeUi::default();
        ui.add("main", "match_ui");
        let mut book = NameBook::default();
        for frame in 0..40 {
            book.tick(&mut ui, frame, &champions);
            for i in 0..BATCH {
                let path = format!("main.{NODE}.n{i}");
                if let Some(t) = ui.text(&path) {
                    if let Some(id) = t.strip_prefix("#asset/base/text/champion?description.").and_then(|r| r.strip_suffix(".name")) {
                        ui.set_text(&path, &format!("名字{id}"));
                    }
                }
            }
        }
        assert!(book.ready() && book.localized);
        assert_eq!(book.lookup("名字c42"), Some("c42"));
        assert!(!ui.exists(&format!("main.{NODE}")), "cleaned up");

        // a host that hands the reference back: English names only
        let mut ui = FakeUi::default();
        ui.add("main", "match_ui");
        let mut book = NameBook::default();
        for frame in 0..10 {
            book.tick(&mut ui, frame, &champions);
        }
        assert!(book.ready() && !book.localized);
        book.learn("Ahri", "c1");
        assert_eq!(book.lookup(" ahri "), Some("c1"));
    }
}
