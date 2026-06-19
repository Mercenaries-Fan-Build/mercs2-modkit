//! Launch the game executable from within the modkit.

use std::path::PathBuf;
use std::process::Command;

/// Spawn the game exe detached, with the install folder as the working
/// directory so it resolves its data files and side-by-side DLLs correctly.
#[tauri::command]
pub fn launch_game(exe_path: String, game_root: Option<String>) -> Result<(), String> {
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
    cmd.spawn()
        .map_err(|e| format!("Failed to launch game: {e}"))?;
    Ok(())
}
