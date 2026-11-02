//! Fearless series as the draft hooks see them. The game tells a deciding hook only the champions
//! the acting team may pick now, not which game of a series it is. So each pair of teams' drafts
//! are kept here (by the game's champion ids): a new draft between the same two teams in which
//! every champion a team played in the one before is off its list is the next game of a Fearless
//! series (`无畏`: its own champions; `无畏(硬核)`: both teams'). A draft where they are back on
//! offer starts over (a new series, or classic rules).
//!
//! The series' length is what the ban/pick screen last showed ("Bo3" / "Bo5", [`set_length`]);
//! a stage's matches share their format. Before any: Bo3.
//!
//! With games left, spending a champion nothing replaces costs a little ([`share`]): the panel,
//! the coach and the AI teams count it alike (留后局).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex, PoisonError};

/// Spending a champion nothing replaces in a Fearless series costs this share of what it is
/// worth, times the share of the series still to come.
pub const CONSERVE: f32 = 0.4;
/// A series' length before the ban/pick screen showed one.
const DEFAULT_LENGTH: usize = 3;
/// Pairs of teams kept at most (the oldest go first).
const KEPT: usize = 256;

static LENGTH: AtomicUsize = AtomicUsize::new(DEFAULT_LENGTH);

/// The share of a champion's worth that spending it costs, with `left` games after this one in
/// a series of `n`.
pub fn share(left: usize, n: usize) -> f32 {
    if left == 0 || n < 2 {
        0.0
    } else {
        CONSERVE * left as f32 / (n - 1) as f32
    }
}

/// The series' length the ban/pick screen shows ("Bo5" -> 5).
pub fn set_length(n: usize) {
    if n >= 2 {
        LENGTH.store(n, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct Pair {
    /// The earlier drafts of a Fearless series (or the last one): team -> its picks.
    done: Vec<HashMap<usize, Vec<usize>>>,
    /// This draft so far.
    now: HashMap<usize, Vec<usize>>,
    /// Bans and picks made in this draft at the last call.
    progress: usize,
    /// The last call's order (to forget the oldest pairs).
    used: u64,
}

#[derive(Default)]
struct Store {
    pairs: HashMap<(usize, usize), Pair>,
    clock: u64,
}

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| Mutex::new(Store::default()));

/// One deciding call as the game gives it (game champion ids).
pub struct Call<'a> {
    pub team: usize,
    pub enemy: usize,
    pub ally_picks: &'a [usize],
    pub enemy_picks: &'a [usize],
    pub ally_bans: &'a [usize],
    pub enemy_bans: &'a [usize],
    pub available: &'a [usize],
}

/// Picks a finished draft must show to count as a game of the series (the hooks do not see the
/// picks made after the last call).
const SEEN_PICKS: usize = 6;

/// Notes this call and, in a Fearless series, says how many games may follow this one and the
/// series' length.
pub fn observe(call: &Call<'_>) -> Option<(usize, usize)> {
    let mut st = STORE.lock().unwrap_or_else(PoisonError::into_inner);
    st.clock += 1;
    let clock = st.clock;
    let key = (call.team.min(call.enemy), call.team.max(call.enemy));
    let p = st.pairs.entry(key).or_default();
    p.used = clock;
    let progress = call.ally_picks.len() + call.enemy_picks.len() + call.ally_bans.len() + call.enemy_bans.len();
    if progress < p.progress {
        // a new draft between the same two teams
        let last = std::mem::take(&mut p.now);
        if last.values().map(Vec::len).sum::<usize>() >= SEEN_PICKS {
            p.done.push(last);
        }
        let taken: Vec<usize> =
            [call.ally_picks, call.enemy_picks, call.ally_bans, call.enemy_bans].concat();
        // this team's champions of the last game, unless taken in this draft already
        let theirs: Vec<usize> = p
            .done
            .last()
            .and_then(|d| d.get(&call.team))
            .map(|list| list.iter().copied().filter(|c| !taken.contains(c)).collect())
            .unwrap_or_default();
        let locked = !theirs.is_empty() && theirs.iter().all(|c| !call.available.contains(c));
        if !locked {
            // classic rules, or a new series: this draft is its first game
            p.done.clear();
        }
    }
    p.progress = progress;
    p.now.insert(call.team, call.ally_picks.to_vec());
    p.now.insert(call.enemy, call.enemy_picks.to_vec());
    let played = p.done.len();
    if st.pairs.len() > KEPT {
        if let Some(oldest) = st.pairs.iter().min_by_key(|(_, p)| p.used).map(|(k, _)| *k) {
            st.pairs.remove(&oldest);
        }
    }
    if played == 0 {
        return None;
    }
    let n = LENGTH.load(Ordering::Relaxed);
    Some((n.saturating_sub(1 + played), n))
}

/// Test support: forget every pair and the length.
#[doc(hidden)]
pub fn reset() {
    *STORE.lock().unwrap_or_else(PoisonError::into_inner) = Store::default();
    LENGTH.store(DEFAULT_LENGTH, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tests share the store: one at a time.
    static ONE: Mutex<()> = Mutex::new(());

    /// A draft between teams 901 and 902 (no other test's), 901 deciding: `ours` / `theirs`
    /// picked so far, `pool` on offer to 901.
    fn call<'a>(ours: &'a [usize], theirs: &'a [usize], pool: &'a [usize]) -> Call<'a> {
        Call { team: 901, enemy: 902, ally_picks: ours, enemy_picks: theirs, ally_bans: &[], enemy_bans: &[], available: pool }
    }

    #[test]
    fn games_of_a_fearless_series() {
        let _one = ONE.lock().unwrap_or_else(PoisonError::into_inner);
        reset();
        let all: Vec<usize> = (0..40).collect();
        let without = |gone: &[usize]| -> Vec<usize> { all.iter().copied().filter(|c| !gone.contains(c)).collect() };
        // game 1: nothing known yet
        assert_eq!(observe(&call(&[], &[], &all)), None);
        assert_eq!(observe(&call(&[0, 1, 2, 3], &[10, 11, 12, 13, 14], &without(&[0, 1, 2, 3, 10, 11, 12, 13, 14]))), None);
        // game 2: team 1's champions of game 1 are off its list -> Fearless, one game played
        let pool = without(&[0, 1, 2, 3]);
        assert_eq!(observe(&call(&[], &[], &pool)), Some((1, 3)));
        assert_eq!(observe(&call(&[4, 5, 6, 7], &[15, 16, 17, 18, 19], &pool)), Some((1, 3)), "the same draft");
        // game 3 of a Bo5 (the screen showed it)
        set_length(5);
        assert_eq!(observe(&call(&[], &[], &without(&[0, 1, 2, 3, 4, 5, 6, 7]))), Some((2, 5)));
        // a new series: the champions are back
        assert_eq!(observe(&call(&[20, 21, 22, 23], &[24, 25, 26, 27, 28], &all)), Some((2, 5)));
        assert_eq!(observe(&call(&[], &[], &all)), None, "back on offer: a first game");
    }

    #[test]
    fn classic_rules_are_no_series() {
        let _one = ONE.lock().unwrap_or_else(PoisonError::into_inner);
        reset();
        let all: Vec<usize> = (0..40).collect();
        assert_eq!(observe(&call(&[0, 1, 2, 3], &[10, 11, 12, 13], &all)), None);
        assert_eq!(observe(&call(&[], &[], &all)), None, "the last game's champions still on offer");
        assert!(share(0, 3) == 0.0 && share(1, 3) > 0.0 && share(2, 5) > share(1, 5));
    }
}
