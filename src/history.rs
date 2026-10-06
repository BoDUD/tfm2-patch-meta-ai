//! Every usable match of the open save, kept compactly: champions are interned to small ids,
//! and a game remembers who played what where, who banned what, and who won. The model
//! (`meta`) is fitted from this list; nothing else is kept per game.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    Top,
    Jungle,
    Mid,
    Bottom,
    Support,
}

impl Role {
    pub const ALL: [Role; 5] = [Role::Top, Role::Jungle, Role::Mid, Role::Bottom, Role::Support];

    /// The game's lane labels (`"Top"`, `"Jungle"`, `"Mid"`, `"Bottom"`, `"Support"`), in any
    /// case, a few common aliases, or an index 0-4.
    pub fn parse(label: &str) -> Option<Role> {
        let l = label.trim().to_ascii_lowercase();
        Some(match l.as_str() {
            "top" | "0" => Role::Top,
            "jungle" | "jg" | "jungler" | "1" => Role::Jungle,
            "mid" | "middle" | "2" => Role::Mid,
            "bottom" | "bot" | "adc" | "carry" | "3" => Role::Bottom,
            "support" | "sup" | "supporter" | "4" => Role::Support,
            _ => return None,
        })
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            Role::Top => "Top",
            Role::Jungle => "Jungle",
            Role::Mid => "Mid",
            Role::Bottom => "Bottom",
            Role::Support => "Support",
        }
    }

    /// The partner this role plays next to (jungle with mid, top/mid with jungle, bottom with
    /// support).
    pub fn duo(self, other: Role) -> bool {
        matches!(
            (self, other),
            (Role::Top, Role::Jungle)
                | (Role::Jungle, Role::Top)
                | (Role::Jungle, Role::Mid)
                | (Role::Mid, Role::Jungle)
                | (Role::Bottom, Role::Support)
                | (Role::Support, Role::Bottom)
        )
    }

    /// The direct lane opponent's role (bottom and support count as one lane).
    pub fn faces(self, other: Role) -> bool {
        match self {
            Role::Bottom | Role::Support => matches!(other, Role::Bottom | Role::Support),
            _ => self == other,
        }
    }
}

/// Champion names <-> small ids, for the lifetime of one save.
#[derive(Clone, Debug, Default)]
pub struct Names {
    list: Vec<String>,
    index: HashMap<String, u16>,
}

impl Names {
    pub fn id(&mut self, name: &str) -> u16 {
        if let Some(id) = self.index.get(name) {
            return *id;
        }
        let id = self.list.len().min(u16::MAX as usize) as u16;
        self.list.push(name.to_string());
        self.index.insert(name.to_string(), id);
        id
    }

    pub fn get(&self, name: &str) -> Option<u16> {
        self.index.get(name).copied()
    }

    pub fn name(&self, id: u16) -> &str {
        self.list.get(id as usize).map_or("?", String::as_str)
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Slot {
    pub champ: u16,
    pub role: Option<Role>,
    pub athlete: Option<u32>,
    /// Gold at the end of the lane phase.
    pub lane_gold: Option<i32>,
}

/// Team-strategy choices ("setting=option") <-> small ids, shared by every save and thread.
static TACTICS: Mutex<Option<Names>> = Mutex::new(None);

/// The id of a strategy choice.
pub fn tactic_id(setting: &str, option: &str) -> u16 {
    TACTICS.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert_with(Names::default).id(&format!("{setting}={option}"))
}

/// The setting and option of a strategy choice id.
pub fn tactic(id: u16) -> (String, String) {
    let guard = TACTICS.lock().unwrap_or_else(PoisonError::into_inner);
    let text = guard.as_ref().map_or("?", |n| n.name(id)).to_string();
    match text.split_once('=') {
        Some((s, o)) => (s.to_string(), o.to_string()),
        None => (text, String::new()),
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Game {
    /// The record it came from.
    pub record: usize,
    pub solo: bool,
    pub version: String,
    pub blue_win: bool,
    /// Blue, red.
    pub teams: [Option<u32>; 2],
    pub sides: [Vec<Slot>; 2],
    pub bans: [Vec<u16>; 2],
    /// Game length in ticks, when the record says.
    pub length: Option<f32>,
    /// Each side's team strategy ([`tactic_id`]s).
    pub tactics: [Vec<u16>; 2],
}

impl Game {
    /// Whether `side` (0 blue, 1 red) won.
    pub fn won(&self, side: usize) -> bool {
        (side == 0) == self.blue_win
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_and_names() {
        assert_eq!(Role::parse("Bottom"), Some(Role::Bottom));
        assert_eq!(Role::parse(" support "), Some(Role::Support));
        assert_eq!(Role::parse("2"), Some(Role::Mid));
        assert_eq!(Role::parse("roam"), None);
        assert!(Role::Jungle.duo(Role::Mid) && !Role::Top.duo(Role::Mid));
        assert!(Role::Support.faces(Role::Bottom) && !Role::Top.faces(Role::Mid));
        let mut names = Names::default();
        assert_eq!(names.id("fighter"), 0);
        assert_eq!(names.id("ninja"), 1);
        assert_eq!(names.id("fighter"), 0);
        assert_eq!(names.name(1), "ninja");
        assert_eq!(names.get("monk"), None);
    }
}
