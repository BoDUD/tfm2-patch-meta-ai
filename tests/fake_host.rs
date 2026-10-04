//! The scenario in `common/scenario.rs`, against the entry points linked into this test.

#[path = "common/scenario.rs"]
mod scenario;

#[test]
fn the_entry_points_work_together() {
    let dir = std::env::temp_dir().join(format!("patch_meta_ai_ffi_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("PATCH_META_AI_DIR", &dir);
    scenario::run(patch_meta_ai::tfm2_mod_required_abi_level, patch_meta_ai::tfm2_mod_entry_stable, &dir);
}
