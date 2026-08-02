//! Suppress the console window Windows allocates for each helper subprocess.
//!
//! modkit's binary is a GUI-subsystem executable, so it has no console of its own.
//! When it spawns a *console-subsystem* helper — `reg`, `powershell`, `wad_simulator`,
//! the crack tool — Windows allocates a fresh console for that child and flashes it on
//! screen: one window per process, gone the instant the child exits. License detection
//! alone fires ~a dozen `reg query` processes ([`super::license::detect_license`]), so a
//! single Setup pass strobed a dozen console windows — alarming even to a trusting user.
//!
//! `CREATE_NO_WINDOW` tells the loader not to give the child a console at all, which
//! also suppresses the window. It does not change how the command runs; stdout/stderr are
//! still captured via `output()`. On non-Windows platforms this is a no-op.
//!
//! Apply [`NoWindow::no_window`] to every *helper* subprocess. Do **not** apply it to a
//! process the user is meant to see (e.g. toolchain.rs opening an interactive shell, which
//! deliberately uses `CREATE_NEW_CONSOLE`) or to a child that already sets creation flags
//! (`creation_flags` overwrites, not OR-combines — `DETACHED_PROCESS` already has no console).

use std::process::Command;

/// `CREATE_NO_WINDOW` (winbase.h): run the child without allocating a console, so no
/// console window appears for console-subsystem helpers.
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Spawn a helper subprocess without flashing a console window.
pub trait NoWindow {
    /// Suppress the child's console window on Windows; a no-op on other platforms.
    /// Chainable with the other `Command` builder methods.
    fn no_window(&mut self) -> &mut Self;
}

impl NoWindow for Command {
    #[cfg(target_os = "windows")]
    fn no_window(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.creation_flags(CREATE_NO_WINDOW)
    }

    #[cfg(not(target_os = "windows"))]
    fn no_window(&mut self) -> &mut Self {
        self
    }
}
