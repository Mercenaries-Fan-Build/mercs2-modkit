//! Tauri command handlers exposed to the Vue frontend via `invoke`.
//!
//! # Never write a blocking `#[tauri::command]`
//!
//! A bare `#[tauri::command]` on a **non-`async fn`** runs the body *inline on the main
//! thread* — the one driving the window's event loop and WebView2. Tauri calls this the
//! `Blocking` execution context and it is the DEFAULT (`tauri-macros`,
//! `command/wrapper.rs`: `execution_context: ExecutionContext::Blocking`). So every
//! millisecond such a command spends in `read_dir`, `Command::spawn`, or zlib is a
//! millisecond the app cannot repaint, scroll, or respond to a click. It reads as a hang.
//!
//! This bit us twice, and neither had anything to do with the network:
//!
//! * **Startup.** `refreshGame()` fires `check_vcredist` (a `read_dir` over the whole of
//!   `C:\Windows\WinSxS` — tens of thousands of entries) and `read_region` (two blocking
//!   `reg.exe` spawns). Both were blocking commands, both ran while the update check was
//!   in flight, so the freeze *looked* like the update check. The update path was already
//!   off-thread and never was the cause.
//! * **Opening a model.** `model_geometry` opens `vz.wad`, parses FFCS, inflates the
//!   container, and builds the index/group tables — all of it, on the main thread.
//!
//! So: if a command touches the filesystem, the registry, a subprocess, or the network,
//! it must run off the main thread. Either declare it `pub async fn`, or — when the body
//! is ordinary blocking code and you don't want to colour it — keep it sync and write
//! `#[tauri::command(async)]`, which hands it to Tauri's thread pool unchanged. That
//! attribute is the only thing standing between this app and a frozen window; do not drop
//! it when editing a signature. (Commands taking `State` need `State<'_, T>` under it.)
//!
//! Pure in-memory commands — `build_conflict_graph`, `validate_manifest`,
//! `preview_wardrobe_lua`, `is_game_running` — are deliberately left blocking: there is
//! nothing to wait on, and the thread hop would cost more than the work.

pub mod asset_catalog;
pub mod conflict_resolver;
pub mod debug_bundle;
pub mod deploy;
pub mod deploy_wad;
pub mod dxwrapper;
pub mod game;
pub mod human_skins;
pub mod installer;
pub mod language;
pub mod launch;
pub mod license;
pub mod logprobe;
pub mod mercsink;
pub mod mod_loader;
pub mod model_view;
pub mod net;
pub mod paths;
pub mod placement;
pub mod prebuilt;
pub mod proc;
pub mod region;
pub mod registry;
pub mod save_backup;
pub mod setup;
pub mod shipment;
pub mod texture_swap;
pub mod texture_usage;
pub mod toolchain;
pub mod updates;
pub mod validator;
pub mod vcredist;
pub mod verify;
pub mod wad_builder;
pub mod wardrobe;
