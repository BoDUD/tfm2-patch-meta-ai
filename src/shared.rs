//! The model's output as the draft hook reads it. The client extension publishes a new table
//! after every rebuild; the draft hook (which runs wherever the game drafts: AI matches on the
//! management server, the opponent in your own matches) only reads it.

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tables {
    /// Champion -> amount added to the AI's pick score.
    pub pick: HashMap<String, f32>,
    /// Champion -> amount added to the AI's ban score.
    pub ban: HashMap<String, f32>,
}

static TABLES: RwLock<Option<Arc<Tables>>> = RwLock::new(None);

pub fn publish(tables: Tables) {
    *TABLES.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(tables));
}

pub fn clear() {
    *TABLES.write().unwrap_or_else(PoisonError::into_inner) = None;
}

pub fn get() -> Option<Arc<Tables>> {
    TABLES.read().unwrap_or_else(PoisonError::into_inner).clone()
}
