//! Everything on screen: the draft-screen overlay (`draft_screen`), champion names in the
//! game's language (`names`) and the UI explorer that writes the live node tree to a file
//! (`explore`).
//!
//! UI paths are dot-separated node ids from the root of the loaded layout (the ban/pick screen
//! is `main`, its grid `main.champions.contents`). The game rebuilds parts of its screens on
//! its own, so every node this mod adds is checked for every few frames and put back when it
//! is gone, and nothing assumes a node exists without asking.

pub mod draft_screen;
pub mod explore;
pub mod names;
pub mod panel;

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::history::Role;

use mod_api_stable::{ClientSceneKindV1, InputEventKindV1, StableClient};

/// The UI calls the overlay needs (the game's client context, or a test double).
pub trait Ui {
    fn exists(&self, path: &str) -> bool;
    fn children(&self, path: &str) -> Vec<String>;
    fn text(&self, path: &str) -> Option<String>;
    fn visible(&self, path: &str) -> Option<bool>;
    fn runner(&self, path: &str) -> Option<String>;
    fn rect(&self, path: &str) -> Option<(f32, f32, f32, f32)>;
    fn state(&self, path: &str) -> Option<String>;
    fn spawn(&mut self, parent: &str, source: &str) -> bool;
    fn set_properties(&mut self, path: &str, source: &str) -> bool;
    fn set_text(&mut self, path: &str, text: &str) -> bool;
    fn remove(&mut self, path: &str) -> bool;
    fn set_visible(&mut self, path: &str, visible: bool) -> bool;
    /// Keys pressed this frame (engine key names, e.g. "F9").
    fn keys_pressed(&self) -> Vec<String>;
    /// The management screen's tab ("Home", "Squad", ...), when on it.
    fn main_tab(&self) -> Option<String> {
        None
    }
}

impl Ui for StableClient<'_> {
    fn exists(&self, path: &str) -> bool {
        self.ui_exists(path)
    }
    fn children(&self, path: &str) -> Vec<String> {
        self.ui_child_names(path)
    }
    fn text(&self, path: &str) -> Option<String> {
        self.ui_text(path)
    }
    fn visible(&self, path: &str) -> Option<bool> {
        self.ui_visible(path)
    }
    fn runner(&self, path: &str) -> Option<String> {
        self.ui_runner_name(path)
    }
    fn rect(&self, path: &str) -> Option<(f32, f32, f32, f32)> {
        self.ui_node_rect(path)
    }
    fn state(&self, path: &str) -> Option<String> {
        self.ui_state_json(path)
    }
    fn spawn(&mut self, parent: &str, source: &str) -> bool {
        self.ui_spawn_source(parent, source)
    }
    fn set_properties(&mut self, path: &str, source: &str) -> bool {
        self.ui_set_properties(path, source)
    }
    fn set_text(&mut self, path: &str, text: &str) -> bool {
        self.ui_set_text(path, text)
    }
    fn remove(&mut self, path: &str) -> bool {
        self.ui_remove_node(path)
    }
    fn set_visible(&mut self, path: &str, visible: bool) -> bool {
        self.ui_set_visible(path, visible)
    }
    fn keys_pressed(&self) -> Vec<String> {
        self.input_events()
            .into_iter()
            .filter(|e| e.kind == Some(InputEventKindV1::KeyPressed))
            .map(|e| e.key)
            .collect()
    }
    fn main_tab(&self) -> Option<String> {
        self.client_main_tab()
    }
}

/// What the screens need from the save (set by the client when a save is read).
#[derive(Clone, Debug, Default)]
pub struct Context {
    pub team_name: String,
    pub champions: Vec<String>,
    /// (English display name, champion id).
    pub english: Vec<(String, String)>,
    /// Each team's players and their lanes (newest match), by team name in lower case.
    pub rosters: HashMap<String, Vec<(u32, Option<Role>)>>,
    /// Team names as the game writes them, by lower-case name.
    pub team_labels: HashMap<String, String>,
    /// Each team's most played champions lately: (champion id, games, wins), most first.
    pub team_picks: HashMap<String, Vec<(String, u32, u32)>>,
    pub athletes: HashMap<u32, String>,
    /// The player's opponent in their newest competition match (lower case).
    pub last_opponent: Option<String>,
    pub backtest: Option<crate::meta::Backtest>,
}

#[derive(Default)]
struct State {
    frame: u64,
    explorer: explore::Explorer,
    names: names::NameBook,
    draft: draft_screen::DraftScreen,
    panel: panel::Panel,
    context: Context,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn lock() -> MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// After every rebuild: names to match, the player's team and every team's players.
pub fn set_context(context: Context) {
    let mut guard = lock();
    let st = guard.get_or_insert_with(State::default);
    for (label, id) in &context.english {
        st.names.learn(label, id);
    }
    for id in &context.champions {
        st.names.learn(id, id);
        st.names.learn(&names::name_ref(id), id);
    }
    st.context = context;
}

/// The save was closed.
pub fn reset() {
    *lock() = None;
}

/// One frame of everything on screen (a save is open).
pub fn tick(ui: &mut impl Ui, scene: Option<ClientSceneKindV1>, cfg: &crate::config::Config) {
    let mut guard = lock();
    let st = guard.get_or_insert_with(State::default);
    st.frame += 1;
    let frame = st.frame;
    st.explorer.tick(ui, frame, cfg.explore, &format!("{scene:?}"));
    if cfg.draft_overlay {
        let view = draft_screen::View {
            team_name: &st.context.team_name,
            rosters: &st.context.rosters,
            grid_values: cfg.grid_values,
            lane_tags: cfg.lane_tags,
        };
        let snapshot = crate::shared::get();
        st.draft.tick(ui, frame, snapshot.as_ref(), &st.names, &view);
    }
    let snapshot = crate::shared::get();
    let opponent = st.draft.enemy_team.clone().or_else(|| st.context.last_opponent.clone());
    let screen = format!("{scene:?}/{}", ui.main_tab().unwrap_or_default());
    st.panel.tick(ui, frame, snapshot.as_deref(), &st.context, opponent.as_deref(), &screen);
}

/// A team name as a key: lower case, without the league rank the ban/pick screen appends
/// ("Samsung Galaxy #1") or stray line breaks.
pub fn team_key(name: &str) -> String {
    let clean: String = name.chars().filter(|c| *c != '\r' && *c != '\n').collect();
    let mut key = clean.trim();
    if let Some(at) = key.rfind(" #") {
        if key[at + 2..].chars().all(|c| c.is_ascii_digit()) && at + 2 < key.len() {
            key = key[..at].trim_end();
        }
    }
    key.to_lowercase()
}

/// Text for a `.ui` string literal.
pub fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `#rrggbbaa` for a `.ui` colour.
pub fn color(rgba: u32) -> String {
    format!("#{rgba:08x}")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A UI tree in memory: nodes by path, with text / visibility / state, and every spawn.
    #[derive(Default)]
    pub struct FakeUi {
        pub nodes: BTreeMap<String, Node>,
        pub spawned: Vec<(String, String)>,
        pub keys: Vec<String>,
    }

    #[derive(Default, Clone)]
    pub struct Node {
        pub text: Option<String>,
        pub visible: bool,
        pub runner: String,
        pub state: Option<String>,
        pub rect: (f32, f32, f32, f32),
        pub props: Vec<String>,
    }

    impl FakeUi {
        pub fn add(&mut self, path: &str, runner: &str) -> &mut Node {
            self.nodes.entry(path.to_string()).or_insert(Node { visible: true, runner: runner.into(), ..Default::default() })
        }
    }

    /// The `#id` of every node in `.ui` source, with its parent path.
    fn ids(source: &str, parent: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack: Vec<String> = vec![parent.to_string()];
        let mut chars = source.char_indices().peekable();
        let mut token = String::new();
        let mut pending: Option<String> = None;
        let mut first = true;
        while let Some((_, c)) = chars.next() {
            match c {
                '{' => {
                    let name = pending.take();
                    match name {
                        Some(n) => {
                            let path = format!("{}.{}", stack.last().unwrap(), n);
                            out.push(path.clone());
                            stack.push(path);
                        }
                        None => stack.push(stack.last().unwrap().clone()),
                    }
                    token.clear();
                }
                '}' => {
                    stack.pop();
                    token.clear();
                }
                '"' => {
                    for (_, d) in chars.by_ref() {
                        if d == '"' {
                            break;
                        }
                    }
                }
                ';' => token.clear(),
                c if c.is_whitespace() => {}
                c => {
                    token.push(c);
                    if let Some((_, ':')) = chars.peek() {
                        // `#name:runner` or the root `name:runner`
                        let name = token.trim_start_matches('#').to_string();
                        if token.starts_with('#') || first {
                            pending = Some(name);
                        }
                        first = false;
                    }
                }
            }
        }
        out
    }

    impl Ui for FakeUi {
        fn exists(&self, path: &str) -> bool {
            self.nodes.contains_key(path)
        }
        fn children(&self, path: &str) -> Vec<String> {
            let prefix = if path.is_empty() { String::new() } else { format!("{path}.") };
            self.nodes
                .keys()
                .filter_map(|k| k.strip_prefix(&prefix))
                .filter(|rest| !rest.is_empty() && !rest.contains('.'))
                .map(str::to_string)
                .collect()
        }
        fn text(&self, path: &str) -> Option<String> {
            self.nodes.get(path)?.text.clone()
        }
        fn visible(&self, path: &str) -> Option<bool> {
            Some(self.nodes.get(path)?.visible)
        }
        fn runner(&self, path: &str) -> Option<String> {
            Some(self.nodes.get(path)?.runner.clone())
        }
        fn rect(&self, path: &str) -> Option<(f32, f32, f32, f32)> {
            Some(self.nodes.get(path)?.rect)
        }
        fn state(&self, path: &str) -> Option<String> {
            self.nodes.get(path)?.state.clone()
        }
        fn spawn(&mut self, parent: &str, source: &str) -> bool {
            if !self.nodes.contains_key(parent) {
                return false;
            }
            for path in ids(source, parent) {
                self.add(&path, "spawned");
            }
            self.spawned.push((parent.to_string(), source.to_string()));
            true
        }
        fn set_properties(&mut self, path: &str, source: &str) -> bool {
            match self.nodes.get_mut(path) {
                Some(n) => {
                    n.props.push(source.to_string());
                    true
                }
                None => false,
            }
        }
        fn set_text(&mut self, path: &str, text: &str) -> bool {
            match self.nodes.get_mut(path) {
                Some(n) => {
                    n.text = Some(text.to_string());
                    true
                }
                None => false,
            }
        }
        fn remove(&mut self, path: &str) -> bool {
            let prefix = format!("{path}.");
            let before = self.nodes.len();
            self.nodes.retain(|k, _| k != path && !k.starts_with(&prefix));
            before != self.nodes.len()
        }
        fn set_visible(&mut self, path: &str, visible: bool) -> bool {
            match self.nodes.get_mut(path) {
                Some(n) => {
                    n.visible = visible;
                    true
                }
                None => false,
            }
        }
        fn keys_pressed(&self) -> Vec<String> {
            self.keys.clone()
        }
    }

    #[test]
    fn team_keys() {
        assert_eq!(team_key("Samsung Galaxy #1"), "samsung galaxy");
        assert_eq!(team_key("Jin Air Green Wings\r #10"), "jin air green wings");
        assert_eq!(team_key(" T1 "), "t1");
        assert_eq!(team_key("Team #Blue"), "team #blue");
    }

    #[test]
    fn spawned_source_creates_its_nodes() {
        let mut ui = FakeUi::default();
        ui.add("main", "match_ui");
        assert!(ui.spawn("main", r#"pma:empty { #bar:color { #text:label { text: "a{b"; } } #tag:label { } }"#));
        assert!(ui.exists("main.pma.bar.text") && ui.exists("main.pma.tag"));
        assert_eq!(ui.children("main.pma"), ["bar", "tag"]);
        assert!(ui.remove("main.pma") && !ui.exists("main.pma.bar"));
        assert_eq!(quote("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(color(0x5b73ffff), "#5b73ffff");
    }
}
