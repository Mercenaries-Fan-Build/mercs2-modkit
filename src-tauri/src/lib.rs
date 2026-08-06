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
use commands::deploy_wad::{
    deploy_patch_wad, deployed_wad_record, list_patch_wad_backups, restore_patch_wad,
};
use commands::dxwrapper::install_dxwrapper;
use commands::game::detect_game;
use commands::human_skins::human_skins;
use commands::installer::{import_local_asi, install_catalog_mod};
use commands::language::{scan_languages, set_language};
use commands::launch::{discover_runtime, is_game_running, launch_game, stop_game, GameProcess};
use commands::license::detect_license;
use commands::logprobe::{analyze_log, locate_log};
use commands::mod_loader::{load_mod, validate_manifest};
use commands::model_view::{model_geometry, model_variants, texture_parts};
use commands::region::{normalize_region, read_region};
use commands::registry::{fetch_catalog, get_custom_sources, save_custom_sources};
use commands::save_backup::{
    backup_saves, delete_save_backup, list_save_backups, list_saves, restore_save_backup,
    set_saves_dir,
};
use commands::setup::{crack_game, install_pmc_bb, install_pmc_bb_log, update_game};
use commands::shipment::{inspect_shipment, take_pending_shipment, PendingShipment};
use commands::toolchain::{
    install_tools, launch_tool, open_tool_shell, poll_tools, stop_tool, toolset_status,
    uninstall_tool, ToolProcesses,
};
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

/// Route a `mercs2-modkit://ship?path=…` URL to the frontend: buffer it (for a cold start whose
/// webview isn't listening yet) and emit a live event (for an already-open window).
fn dispatch_deep_link(app: &tauri::AppHandle, url: &str) {
    use tauri::{Emitter, Manager};
    if let Some(path) = commands::shipment::parse_ship_url(url) {
        if let Some(state) = app.try_state::<PendingShipment>() {
            if let Ok(mut slot) = state.0.lock() {
                *slot = Some(path.clone());
            }
        }
        let _ = app.emit("deep-link-shipment", path);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();

    // Single-instance MUST be registered first (Tauri's guidance): it intercepts a second launch —
    // which on Windows/Linux is how a deep link arrives — focuses the existing window, and forwards
    // any `mercs2-modkit://` URL from that launch's argv into the running app.
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            use tauri::Manager;
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_focus();
            }
            for arg in &argv {
                if arg.starts_with("mercs2-modkit://") {
                    dispatch_deep_link(app, arg);
                }
            }
        }));
    }

    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_deep_link::init())
        .setup(|app| {
            // The updater only exists on desktop targets.
            #[cfg(desktop)]
            app.handle()
                .plugin(tauri_plugin_updater::Builder::new().build())?;

            // Register the URL scheme at runtime (covers dev builds and Linux/Windows; the installer
            // registers it for packaged builds), and deliver the launching URL — macOS routes deep
            // links here rather than through argv, as does the initial process on every platform.
            #[cfg(desktop)]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let _ = app.deep_link().register("mercs2-modkit");
                let handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        dispatch_deep_link(&handle, url.as_str());
                    }
                });
            }
            Ok(())
        })
        .manage(GameProcess::default())
        .manage(ToolProcesses::default())
        .manage(PendingShipment::default())
        .invoke_handler(tauri::generate_handler![
            load_mod,
            validate_manifest,
            detect_asset_type,
            build_conflict_graph,
            assemble_patch_wad,
            preview_conflicts,
            deploy_patch_wad,
            list_patch_wad_backups,
            restore_patch_wad,
            deployed_wad_record,
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
            toolset_status,
            install_tools,
            uninstall_tool,
            open_tool_shell,
            launch_tool,
            poll_tools,
            stop_tool,
            detect_game,
            fetch_catalog,
            get_custom_sources,
            save_custom_sources,
            install_catalog_mod,
            import_local_asi,
            deploy_asi,
            trash_paths,
            install_pmc_bb,
            install_pmc_bb_log,
            crack_game,
            update_game,
            detect_license,
            install_dxwrapper,
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
            inspect_shipment,
            take_pending_shipment,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
