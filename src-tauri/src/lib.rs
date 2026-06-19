//! mercs2-modkit — Tauri backend.
//!
//! Exposes commands for loading mods, detecting asset types, computing
//! conflicts, assembling patch WADs, and validating them with `wad_simulator`.

mod commands;
mod models;

use commands::asset_catalog::detect_asset_type;
use commands::conflict_resolver::build_conflict_graph;
use commands::deploy::deploy_asi;
use commands::game::detect_game;
use commands::installer::{import_local_asi, install_catalog_mod};
use commands::launch::launch_game;
use commands::mod_loader::{load_mod, validate_manifest};
use commands::registry::fetch_catalog;
use commands::setup::{crack_game, install_pmc_bb};
use commands::validator::{fetch_wad_simulator, validate_wad};
use commands::wad_builder::assemble_patch_wad;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            load_mod,
            validate_manifest,
            detect_asset_type,
            build_conflict_graph,
            assemble_patch_wad,
            fetch_wad_simulator,
            validate_wad,
            detect_game,
            fetch_catalog,
            install_catalog_mod,
            import_local_asi,
            deploy_asi,
            install_pmc_bb,
            crack_game,
            launch_game,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
