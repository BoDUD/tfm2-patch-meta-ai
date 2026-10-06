//! The model's output as the draft hook (and the draft screen) read it. The client publishes a
//! new snapshot after every fit; the draft hook runs wherever the game drafts (AI matches on
//! the management server, the opponent in your own matches) and only reads it.

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};

use crate::advisor::Damage;
use crate::meta::Meta;

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub meta: Meta,
    /// Physical / magic damage per champion id (from the champions' tags).
    pub damage: HashMap<u16, Damage>,
}

impl Snapshot {
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
