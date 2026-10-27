//! The model's output as the draft hook (and the draft screen) read it. The client publishes a
//! new snapshot after every fit; the draft hook runs wherever the game drafts (AI matches on
//! the management server, the opponent in your own matches) and only reads it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use crate::advisor::Damage;
use crate::meta::Meta;

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub meta: Meta,
    /// Physical / magic damage per champion id (from the champions' tags).
    pub damage: HashMap<u16, Damage>,
    /// Athletes' and teams' names the client has read (for `diag.log`).
    pub athlete_names: HashMap<u32, String>,
    pub team_names: HashMap<u32, String>,
}

impl Snapshot {
    pub fn new(meta: Meta, damage: HashMap<u16, Damage>) -> Self {
        Self { meta, damage, athlete_names: HashMap::new(), team_names: HashMap::new() }
    }

    pub fn damage_of(&self, champ: u16) -> Option<Damage> {
        self.damage.get(&champ).copied()
    }
}

static SNAPSHOT: RwLock<Option<Arc<Snapshot>>> = RwLock::new(None);

pub fn publish(snapshot: Snapshot) {
    *SNAPSHOT.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(snapshot));
}

pub fn clear() {
    *SNAPSHOT.write().unwrap_or_else(PoisonError::into_inner) = None;
}

pub fn get() -> Option<Arc<Snapshot>> {
    SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// The player's team id in the open save (`usize::MAX`: none), for the coach delegation.
static PLAYER_TEAM: AtomicUsize = AtomicUsize::new(usize::MAX);

pub fn set_player_team(team: Option<usize>) {
    PLAYER_TEAM.store(team.unwrap_or(usize::MAX), Ordering::Relaxed);
}

pub fn player_team() -> Option<usize> {
    Some(PLAYER_TEAM.load(Ordering::Relaxed)).filter(|t| *t != usize::MAX)
}

/// The draft screen's ranking for the player's team's turn now (the panel's 禁 1 / 选 order, the
/// lookahead included), for the coach override (`draft::CoachHook`). Champions are the model's
/// ids; the picks are sorted, so the hook can tell whether the plan is for the draft it is asked
/// about.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CoachPlan {
    pub ally: Vec<u16>,
    pub enemy: Vec<u16>,
    /// Bans made so far, both sides.
    pub bans: usize,
    pub ban: bool,
    /// Best first.
    pub ranked: Vec<u16>,
    /// Cards a position lock holds (the team may not pick them now).
    pub held: Vec<u16>,
}

static COACH_PLAN: RwLock<Option<Arc<CoachPlan>>> = RwLock::new(None);

pub fn set_coach_plan(plan: Option<CoachPlan>) {
    *COACH_PLAN.write().unwrap_or_else(PoisonError::into_inner) = plan.map(Arc::new);
}

pub fn coach_plan() -> Option<Arc<CoachPlan>> {
    COACH_PLAN.read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// What the coach override chose in the player's own draft (model ids, in order), for the
/// review: a choice that does not show up on the screen was overridden by another mod.
static COACH_CHOICES: RwLock<Vec<(bool, u16)>> = RwLock::new(Vec::new());

pub fn note_coach_choice(ban: bool, champ: u16) {
    let mut list = COACH_CHOICES.write().unwrap_or_else(PoisonError::into_inner);
    if list.last() != Some(&(ban, champ)) {
        list.push((ban, champ));
    }
}

/// The coach override's choices since the last call (and forgets them).
pub fn take_coach_choices() -> Vec<(bool, u16)> {
    std::mem::take(&mut *COACH_CHOICES.write().unwrap_or_else(PoisonError::into_inner))
}
