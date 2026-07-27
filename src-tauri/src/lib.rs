//! mercs2-modkit — Tauri backend.
//!
//! Exposes commands for loading mods, detecting asset types, computing
//! conflicts, assembling patch WADs, and validating them with `wad_simulator`.

pub mod commands;
pub mod models;

use commands::asset_catalog::detect_asset_type;
use commands::conflict_resolver::build_conflict_graph;
use commands::debug_bundle::build_debug_zip;
use commands::deploy::{deploy_asi, trash_paths};
use commands::deploy_wad::{deploy_patch_wad, list_patch_wad_backups, restore_patch_wad};
use commands::game::{detect_game, game_icon};
use commands::human_skins::human_skins;
use commands::installer::{import_local_asi, install_catalog_mod};
use commands::language::{scan_languages, set_language};
use commands::launch::{discover_runtime, is_game_running, launch_game, stop_game, GameProcess};
use commands::logprobe::{analyze_log, locate_log};
use commands::mod_loader::{load_mod, validate_manifest};
use commands::model_view::{model_geometry, model_variants, texture_parts};
use commands::region::{normalize_region, read_region};
use commands::registry::{fetch_catalog, get_custom_sources, save_custom_sources};
use commands::save_backup::{
    backup_saves, delete_save_backup, list_save_backups, list_saves, restore_save_backup,
    set_saves_dir,
};
use commands::setup::{crack_game, install_pmc_bb};
use commands::updates::{latest_release, updater_supported};
use commands::validator::{fetch_wad_simulator, validate_wad};
use commands::vcredist::{check_vcredist, install_vcredist};
use commands::verify::{generate_manifest, verify_game};
use commands::wad_builder::{assemble_patch_wad, preview_conflicts};
use commands::prebuilt::inspect_patch_wad;
use commands::texture_swap::{
    export_texture, inspect_texture, list_textures, texture_details, texture_previews,
};
use commands::wardrobe::{list_wardrobe_models, preview_wardrobe_lua};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            // The updater only exists on desktop targets.
            #[cfg(desktop)]
            app.handle()
                .plugin(tauri_plugin_updater::Builder::new().build())?;
            Ok(())
        })
        .manage(GameProcess::default())
        .invoke_handler(tauri::generate_handler![
            load_mod,
            game_icon,
            validate_manifest,
            detect_asset_type,
            build_conflict_graph,
            assemble_patch_wad,
            preview_conflicts,
            deploy_patch_wad,
            list_patch_wad_backups,
            restore_patch_wad,
            list_wardrobe_models,
            preview_wardrobe_lua,
            human_skins,
            inspect_patch_wad,
            inspect_texture,
            list_textures,
            texture_previews,
            texture_details,
            export_texture,
            model_geometry,
            model_variants,
            texture_parts,
            fetch_wad_simulator,
            validate_wad,
            detect_game,
            fetch_catalog,
            get_custom_sources,
            save_custom_sources,
            install_catalog_mod,
            import_local_asi,
            deploy_asi,
            trash_paths,
            install_pmc_bb,
            crack_game,
            launch_game,
            discover_runtime,
            is_game_running,
            stop_game,
            analyze_log,
            locate_log,
            latest_release,
            updater_supported,
            check_vcredist,
            install_vcredist,
            generate_manifest,
            verify_game,
            read_region,
            normalize_region,
            scan_languages,
            set_language,
            build_debug_zip,
            list_saves,
            backup_saves,
            list_save_backups,
            restore_save_backup,
            delete_save_backup,
            set_saves_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
