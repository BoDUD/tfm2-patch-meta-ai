//! Patch Meta AI - smarter ban/pick and an automatic tier list for Teamfight Manager 2
//! (stable mod API, game 0.6+).
//!
//! The mod watches the current in-game patch: every competition match and solo-rank game of the
//! open save, the previous patch's results and the latest patch notes. From them it estimates
//! each champion's win rate this patch ([`model`]) and
//!
//! 1. nudges the draft AI's ban and pick scores ([`draft`]);
//! 2. keeps the player team's champion tier list up to date (`client` plans it, `server`
//!    writes it - only the management server may change records).
//!
//! The client extension reads the save a few records per frame ([`scan`]); a save switch, a new
//! game or pruned records only make it start over. Nothing in a callback may panic: every lock
//! tolerates poisoning and lists are never indexed with positions from an earlier call.
//!
//! The idea of win-rate driven drafting and tiers was popularised by yudra's "Win-Rate Ban/Pick
//! AI + Champion Tiers" Workshop mod (no longer maintained); this is an independent
//! implementation with its own model.

mod client;
pub mod compat;
pub mod config;
mod diag;
pub mod draft;
pub mod model;
mod paths;
pub mod patchnotes;
pub mod records;
pub mod scan;
pub mod server;
mod shared;

use mod_api_stable::{declare_stable_mod, LogLevel, StableHost, StableMod};

/// Mod id = folder name = DLL file name. Never changes after release.
pub const MOD_ID: &str = "patch_meta_ai";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

fn init(host: &StableHost) -> StableMod {
    let game = host.game_version();
    let dir = paths::mod_dir();
    diag::open(&dir);
    let header = format!(
        "{MOD_ID} {VERSION} loaded (game {}.{}.{}, host ABI level {}, folder {})",
        game.major,
        game.minor,
        game.patch,
        host.abi_level(),
        dir.display()
    );
    diag::log(&header);
    host.log(LogLevel::Info, &header);
    compat::load();
    config::load_now();

    let mut decl = StableMod::new(MOD_ID);
    decl.set_extension(client::ClientExt);
    decl.set_server_extension(server::ServerExt);
    decl.add_draft_score_hook(draft::MetaDraftHook);
    decl
}

declare_stable_mod!(init);

/// Test support: forget everything an earlier test left in the process-wide state.
#[doc(hidden)]
pub fn reset_for_tests() {
    diag::open(&paths::mod_dir());
    compat::set(compat::Others::default());
    client::reset_for_tests();
    shared::clear();
    server::reset_for_tests();
    config::load_now();
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{Mutex, MutexGuard, PoisonError};

    static SERIAL: Mutex<()> = Mutex::new(());

    /// Tests that touch the process-wide state run one at a time.
    pub fn serial() -> MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
