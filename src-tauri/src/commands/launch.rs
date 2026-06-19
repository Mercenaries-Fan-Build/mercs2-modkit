//! Launch the game executable from within the modkit.
//!
//! We track the child process we spawn in Tauri-managed state so that:
//!   - launching is atomic: the mutex guard spans the is-running check and the
//!     spawn, so we can never start a second instance of the game we own;
//!   - the UI can poll whether our instance is still alive and reflect it;
//!   - the user can stop the instance we started.

use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::Mutex;

use tauri::State;

/// The single game process modkit has spawned (if any). Managed by Tauri.
#[derive(Default)]
pub struct GameProcess(pub Mutex<Option<Child>>);

/// Spawn the game exe detached, with the install folder as the working directory
/// so it resolves its data files and side-by-side DLLs correctly. Refuses to
/// start a second instance while the one we launched is still running.
#[tauri::command]
pub fn launch_game(
    state: State<GameProcess>,
    exe_path: String,
    game_root: Option<String>,
) -> Result<(), String> {
    let mut guard = state.0.lock().map_err(|_| "Game process lock poisoned")?;

    // Atomic: hold the lock across the liveness check and the spawn. Reap our
    // previous child if it has already exited; refuse if it's still alive.
    if let Some(child) = guard.as_mut() {
        match child.try_wait() {
            Ok(Some(_)) => *guard = None, // exited — fall through and relaunch
            Ok(None) => return Err("Mercenaries 2 is already running.".to_string()),
            Err(e) => return Err(format!("Failed to query the running game: {e}")),
        }
    }

    let exe = PathBuf::from(&exe_path);
    if !exe.is_file() {
        return Err(format!("Game exe not found: {exe_path}"));
    }
    let cwd = game_root
        .map(PathBuf::from)
        .or_else(|| exe.parent().map(|p| p.to_path_buf()));

    let mut cmd = Command::new(&exe);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let child = cmd
        .spawn()
        .map_err(|e| format!("Failed to launch game: {e}"))?;
    *guard = Some(child);
    Ok(())
}

/// Whether the instance modkit launched is still running. Reaps the handle if it
/// has exited, so the next launch is allowed.
#[tauri::command]
pub fn is_game_running(state: State<GameProcess>) -> bool {
    let mut guard = match state.0.lock() {
        Ok(g) => g,
        Err(_) => return false,
    };
    match guard.as_mut() {
        Some(child) => match child.try_wait() {
            Ok(Some(_)) => {
                *guard = None;
                false
            }
            Ok(None) => true,
            Err(_) => {
                *guard = None;
                false
            }
        },
        None => false,
    }
}

/// Terminate the instance modkit launched (no-op if none / already exited).
#[tauri::command]
pub fn stop_game(state: State<GameProcess>) -> Result<(), String> {
    let mut guard = state.0.lock().map_err(|_| "Game process lock poisoned")?;
    if let Some(mut child) = guard.take() {
        if let Ok(Some(_)) = child.try_wait() {
            return Ok(()); // already exited
        }
        child
            .kill()
            .map_err(|e| format!("Failed to stop game: {e}"))?;
        let _ = child.wait();
    }
    Ok(())
}
