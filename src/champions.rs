//! The game's champion list: asked a question at a time, and kept from one session to the next (2.8.3).
//!
//! Listing the champions is what froze the game for 12-17 s when a save was opened (`[perf] … worst frame 17054 ms`;
//! 2.8.2 spread the champions' stats over frames and the freeze stayed: 16.0 s in the first frame). The SDK's
//! `champion_names` asks the game for the count and then twice for every name - 569 questions for 284 champions, all
//! in one call - and on a save with hundreds of (mod) champions a question about the list takes tens of
//! milliseconds; the data of the 283 other champions took 1.4 s in all.
//!
//! So the two questions are asked here one at a time, straight from the game's table ([`count`], [`name_at`] - one
//! question a name, not two), and the list is kept per save in `champions.json` ([`load`], [`store`]): a session
//! takes the kept list when the game still counts as many champions and has data for every name, and asks the game
//! for its names, one a frame, only when it does not.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use mod_api_stable::{ClientCtxV1, DataVtableV1, StableClient};
use serde::{Deserialize, Serialize};

use crate::paths;

pub const FILE: &str = "champions.json";
/// Saves whose lists are kept (the ones written last).
const SAVES_KEPT: usize = 24;

/// The game's data table and state behind the SDK's wrapper, which keeps them to itself: the wrapper is that one
/// pointer (its size is checked when this is compiled).
fn table(ctx: &StableClient<'_>) -> Option<(*const DataVtableV1, *const c_void)> {
    const { assert!(std::mem::size_of::<StableClient<'static>>() == std::mem::size_of::<*mut ClientCtxV1>()) };
    // SAFETY: a `StableClient` is its one pointer and a zero-sized marker (the assert above), so its bytes are that
    // pointer; the SDK made it from the context the game handed to this callback.
    let raw: *mut ClientCtxV1 = unsafe { std::mem::transmute_copy(ctx) };
    if raw.is_null() {
        return None;
    }
    let end = std::mem::offset_of!(ClientCtxV1, data) + std::mem::size_of::<*const DataVtableV1>();
    // SAFETY: `size` leads every context and says how far it goes (the SDK reads it the same way).
    unsafe {
        if (*raw).size < end {
            return None;
        }
        let data = (*raw).data;
        (!data.is_null()).then_some((data, (*raw).state as *const c_void))
    }
}

/// Whether the table is long enough for the slot at `offset` (as the SDK's `slot!`).
fn has(table: *const DataVtableV1, offset: usize) -> bool {
    // SAFETY: `size` leads every table.
    unsafe { (*table).size >= offset + std::mem::size_of::<Option<unsafe extern "C" fn()>>() }
}

/// How many champions the game lists (one question). None: this host does not say.
pub fn count(ctx: &StableClient<'_>) -> Option<usize> {
    let (table, state) = table(ctx)?;
    if !has(table, std::mem::offset_of!(DataVtableV1, champion_count)) {
        return None;
    }
    // SAFETY: the slot is inside the table (checked); the game's own function with its own state.
    let f = unsafe { (*table).champion_count }?;
    Some(unsafe { f(state) })
}

/// The name of the `index`-th champion the game lists - one question, with room for any name (the SDK asks for the
/// length first and then again for the bytes). None: no such champion, or this host does not say.
pub fn name_at(ctx: &StableClient<'_>, index: usize) -> Option<String> {
    let (table, state) = table(ctx)?;
    if !has(table, std::mem::offset_of!(DataVtableV1, champion_name_at)) {
        return None;
    }
    // SAFETY: as in `count`.
    let f = unsafe { (*table).champion_name_at }?;
    let mut buf = vec![0u8; 256];
    let mut len = 0usize;
    // SAFETY: the buffer and its length; the game writes at most that many bytes and says how long the name is.
    if !unsafe { f(state, index, buf.as_mut_ptr(), buf.len(), &mut len) } {
        return None;
    }
    if len > buf.len() {
        // longer than the room given: again, with the length it said
        buf = vec![0u8; len];
        let mut full = 0usize;
        // SAFETY: as above.
        if !unsafe { f(state, index, buf.as_mut_ptr(), buf.len(), &mut full) } {
            return None;
        }
        len = full.min(len);
    }
    buf.truncate(len);
    String::from_utf8(buf).ok().filter(|name| !name.is_empty())
}

#[derive(Default, Serialize, Deserialize)]
struct Kept {
    v: u32,
    /// (save, its champions) - the one written last at the end.
    lists: Vec<(String, Vec<String>)>,
}

static FILE_LOCK: Mutex<()> = Mutex::new(());

pub fn file() -> PathBuf {
    paths::mod_dir().join(FILE)
}

fn read(file: &Path) -> Kept {
    std::fs::read_to_string(file).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

/// The champion list kept for a save (`cache::stem`).
pub fn load(file: &Path, save: &str) -> Option<Vec<String>> {
    let _lock = FILE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    read(file).lists.into_iter().find(|(s, _)| s == save).map(|(_, names)| names).filter(|names| !names.is_empty())
}

/// Keeps a save's champion list (an empty one: forgets it).
pub fn store(file: &Path, save: &str, names: &[String]) {
    let _lock = FILE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let mut kept = read(file);
    kept.v = 1;
    kept.lists.retain(|(s, _)| s != save);
    if !names.is_empty() {
        kept.lists.push((save.to_string(), names.to_vec()));
    }
    let over = kept.lists.len().saturating_sub(SAVES_KEPT);
    kept.lists.drain(..over);
    // equal lists of several saves are written once each all the same: a list is a few kilobytes
    if let Ok(text) = serde_json::to_string(&kept) {
        let _ = std::fs::write(file, text);
    }
}

/// How the lists of two sessions differ, for the log: (names only in `new`, names only in `old`).
pub fn difference(old: &[String], new: &[String]) -> (Vec<String>, Vec<String>) {
    let (a, b): (BTreeMap<&str, ()>, BTreeMap<&str, ()>) =
        (old.iter().map(|n| (n.as_str(), ())).collect(), new.iter().map(|n| (n.as_str(), ())).collect());
    let added = new.iter().filter(|n| !a.contains_key(n.as_str())).cloned().collect();
    let gone = old.iter().filter(|n| !b.contains_key(n.as_str())).cloned().collect();
    (added, gone)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saves_list_is_kept_and_the_oldest_saves_go() {
        let file = std::env::temp_dir().join(format!("pma_champions_test_{}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        assert_eq!(load(&file, "team1-0"), None);
        let names = |n: usize| (0..n).map(|i| format!("c{i}")).collect::<Vec<_>>();
        store(&file, "team1-0", &names(3));
        store(&file, "team2-0", &names(5));
        assert_eq!(load(&file, "team1-0"), Some(names(3)));
        assert_eq!(load(&file, "team2-0").map(|l| l.len()), Some(5));
        // written again: the newer list, once
        store(&file, "team1-0", &names(4));
        assert_eq!(load(&file, "team1-0"), Some(names(4)));
        assert_eq!(read(&file).lists.len(), 2);
        // an empty list forgets the save
        store(&file, "team2-0", &[]);
        assert_eq!(load(&file, "team2-0"), None);
        // only so many saves
        for i in 0..SAVES_KEPT + 3 {
            store(&file, &format!("team9{i}-0"), &names(2));
        }
        assert_eq!(read(&file).lists.len(), SAVES_KEPT);
        assert_eq!(load(&file, "team1-0"), None, "the oldest went");
        assert!(load(&file, &format!("team9{}-0", SAVES_KEPT + 2)).is_some());
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn the_difference_of_two_lists() {
        let l = |s: &[&str]| s.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        assert_eq!(difference(&l(&["a", "b", "c"]), &l(&["b", "c", "d"])), (l(&["d"]), l(&["a"])));
        assert_eq!(difference(&l(&["a"]), &l(&["a"])), (Vec::new(), Vec::new()));
    }
}
