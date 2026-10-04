//! Launch the game from within the modkit.
//!
//! We track the child process we spawn in Tauri-managed state so that:
//!   - launching is atomic: the mutex guard spans the is-running check and the
//!     spawn, so we can never start a second instance of the game we own;
//!   - the UI can poll whether our instance is still alive and reflect it;
//!   - the user can stop the instance we started.
//!
//! How the game is run on each host comes from one place,
//! [`super::runtime_settings`] (`<app-data>/runtime.json`): the runner, the prefix,
//! and the Wine environment, DLL overrides, registry values, `WINEDEBUG` and game
//! arguments. The launcher reads it itself, so it cannot run a configuration other
//! than the one the UI shows.
//!
//! On Windows we spawn the exe directly, declining UAC elevation, which the game
//! never asks for but Windows can impose on it (see `build_command`).
//!
//! On macOS the game runs under the modkit-managed Wine build
//! ([`super::managed::wine`]): `WINEPREFIX=<prefix> wine64 <exe>`, from the game
//! folder wherever it lives — the prefix reaches it through Wine's `z:` → `/`
//! drive, so a game in `~/Downloads` needs no copying into `drive_c`. Registry
//! values from the settings are imported into the prefix with `reg import` before
//! every launch. On Apple Silicon the Wine build is x86_64, so Rosetta 2 is checked
//! in preflight.
//!
//! On Linux the game is a 32-bit Windows D3D9 title, so we run it through Steam
//! Proton *inside the Steam Linux Runtime (sniper) container* — the verified
//! recipe that reaches the world rendering on the discrete GPU. Every installed
//! Proton is **listed** (Steam root, every library in `libraryfolders.vdf`,
//! `compatibilitytools.d` for Proton-GE) and the user picks one; the pick is
//! stored in the settings and a launch fails if it has gone missing. Unset, the
//! layering is `MERCS2_*` env var → autodiscovery's first match. Registry values
//! are not applied on Linux; a settings file that sets them fails the launch.
//!
//! Two Linux host prerequisites are non-obvious and are checked in preflight with
//! an actionable error: unprivileged user namespaces must be allowed (container),
//! and the 32-bit NVIDIA driver libs must be installed and match the running
//! module (else 32-bit DXVK only sees llvmpipe and renders in software).
//!
//! When the modkit itself runs as a Flatpak (e.g. on a Steam Deck), Proton can't
//! drive its pressure-vessel/bwrap container from *inside* our sandbox, so the
//! whole invocation is run on the host via `flatpak-spawn --host`. Discovery
//! resolves the real host `$HOME` (Flatpak rewrites it to a per-app dir) so it
//! still finds the host Steam install through `--filesystem=host`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Mutex;

use tauri::State;

use super::runtime_settings::{self, RuntimeSettings};

/// The single game process modkit has spawned (if any). Managed by Tauri.
#[derive(Default)]
pub struct GameProcess(pub Mutex<Option<Child>>);

/// What the runtime resolves to on this host — surfaced to the UI so users can
/// see what a launch would use.
#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInfo {
    /// `windows`, `macos` or `linux`.
    pub host: String,
    /// The Wine/Proton prefix a launch would use (non-Windows hosts).
    pub prefix: Option<String>,
    /// macOS: the selected Wine build's `wine64`.
    pub wine: Option<String>,
    pub steam_root: Option<String>,
    /// Linux: the Proton a launch would use.
    pub proton: Option<String>,
    /// Linux: every Proton discovery found, in preference order.
    pub protons: Vec<String>,
    pub sniper: Option<String>,
    /// Whether a launch would run inside the sniper container.
    pub container: bool,
    /// Why something could not be resolved, or what a launch would do differently.
    pub notes: Vec<String>,
}

/// ASI-loader config the engine expects next to the exe (mirrors the verified
/// Windows baseline). `DontLoadFromDllMain=0` arms the SecuROM spoof during
/// DllMain, before the entry point. Written on the Wine/Proton launch paths.
#[cfg(any(target_os = "linux", target_os = "macos"))]
const GLOBAL_INI: &str =
    "[GlobalSets]\nLoadPlugins=1\nDontLoadFromDllMain=0\nLoadFromScriptsOnly=0\nLoadRecursively=1\n";

/// Write `scripts/global.ini` next to the exe when it is absent.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn write_global_ini(game_dir: &Path) {
    let scripts = game_dir.join("scripts");
    let _ = std::fs::create_dir_all(&scripts);
    let global_ini = scripts.join("global.ini");
    if !global_ini.exists() {
        let _ = std::fs::write(&global_ini, GLOBAL_INI);
    }
}

/// Spawn the game, with the install folder as the working directory so it
/// resolves its data files and side-by-side DLLs. Refuses to start a second
/// instance while the one we launched is still running.
#[tauri::command(async)]
pub fn launch_game(
    state: State<'_, GameProcess>,
    exe_path: String,
    game_root: Option<String>,
    verbose_log: Option<bool>,
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
    let game_dir = game_root
        .map(PathBuf::from)
        .or_else(|| exe.parent().map(|p| p.to_path_buf()))
        .ok_or("Could not resolve the game directory")?;

    // Note: the VC++ 2008 runtime is offered as an optional install in Game Info,
    // but it is NOT a launch precondition — the game ships its CRTs (msvcr71/80)
    // app-locally, so a missing system VC90 must not block launch. The real cause
    // of a "binkw32.dll was not found" dialog is usually a missing/damaged game
    // file; Diagnostics → "Verify game files" pinpoints that.

    // Prefer the de-DRM'd exe (it imports pmc_bb.dll); the stock SecuROM exe
    // won't run under Wine.
    let run_exe = launch_exe(&game_dir, &exe);
    let settings = runtime_settings::read()?;
    // Verbose pmc_blackbox log hooks are opt-in per launch (expensive); default off.
    let verbose = verbose_log.unwrap_or(false);

    // Snapshot the player's saves before the game can touch them. Best-effort:
    // a failed backup (no saves yet, unreadable dir) must never block a launch.
    let _ = crate::commands::save_backup::backup_before_launch(None);

    let mut cmd = build_command(&game_dir, &run_exe, &settings, verbose)?;
    let child = cmd
        .spawn()
        .map_err(|e| format!("Failed to launch game: {e}"))?;
    *guard = Some(child);
    Ok(())
}

/// Report what the runtime resolves to from the saved settings, so the UI can
/// display it before launching.
#[tauri::command(async)]
pub fn discover_runtime() -> RuntimeInfo {
    let settings = match runtime_settings::read() {
        Ok(s) => s,
        Err(e) => {
            return RuntimeInfo {
                host: crate::commands::net::release::platform_token().into(),
                notes: vec![e],
                ..Default::default()
            }
        }
    };
    resolve_runtime(&settings)
}

/// Pick the executable to actually launch.
///
/// Licensed (dxwrapper) path: if `dxwrapper.dll` is installed, the copy loads
/// mods without touching the exe, so we launch the **stock** exe and never a
/// cracked sibling — even if one happens to be lying around. Otherwise (crack
/// path) prefer the cracked build (de-DRM'd, imports the ASI loader) over the
/// detected/stock exe. Shares `game::resolve_exes` with detection so what we
/// launch matches what Game Info reports as `launch_exe_path`.
fn launch_exe(game_dir: &Path, detected: &Path) -> PathBuf {
    if game_dir.join("dxwrapper.dll").is_file() {
        return detected.to_path_buf();
    }
    match crate::commands::game::resolve_exes(game_dir) {
        Some((_, Some(cracked))) => PathBuf::from(cracked.path),
        _ => detected.to_path_buf(),
    }
}

// ----------------------------------------------------------------------------
// Windows: direct launch
// ----------------------------------------------------------------------------

#[cfg(target_os = "windows")]
fn resolve_runtime(_settings: &RuntimeSettings) -> RuntimeInfo {
    RuntimeInfo {
        host: "windows".into(),
        notes: vec!["Direct launch (no Wine/Proton) on Windows.".into()],
        ..Default::default()
    }
}

/// Windows: spawn the exe directly with the install dir as the cwd.
#[cfg(target_os = "windows")]
fn build_command(
    game_dir: &Path,
    run_exe: &Path,
    _settings: &RuntimeSettings,
    verbose: bool,
) -> Result<Command, String> {
    let mut cmd = Command::new(run_exe);
    cmd.current_dir(game_dir);
    // Gate pmc_blackbox's verbose log hooks; set explicitly so an inherited
    // value can't silently turn it back on.
    cmd.env("PMC_VERBOSE_LOG", if verbose { "1" } else { "0" });
    // Every known Mercenaries2 exe — retail, cracked, and apply_crack's output —
    // carries a manifest that declares only a VC80 CRT dependency, with no
    // `requestedExecutionLevel`. That is exactly the eligibility condition for UAC
    // installer detection (32-bit + no declared level + interactive standard user),
    // so Windows decides about elevation *for* the game: a stray RUNASADMIN compat
    // layer, a shim-database entry, or an inherited __COMPAT_LAYER can all mark it
    // as needing admin. `Command::spawn` is CreateProcessW, which never elevates —
    // it just fails with ERROR_ELEVATION_REQUIRED (os error 740) and the user sees
    // "Failed to launch game". Pin the layer to RunAsInvoker to decline the
    // elevation instead of prompting for admin the game doesn't need; it overrides
    // all three sources, and there is no manifested requireAdministrator that could
    // outrank it. Set explicitly so an inherited value can't reintroduce the
    // problem.
    cmd.env("__COMPAT_LAYER", "RunAsInvoker");
    Ok(cmd)
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    /// Look up a var on a built `Command` (`None` = not set by us).
    fn env_of<'a>(cmd: &'a Command, key: &str) -> Option<&'a OsStr> {
        cmd.get_envs()
            .find(|(k, _)| *k == OsStr::new(key))
            .and_then(|(_, v)| v)
    }

    fn build(verbose: bool) -> Command {
        build_command(
            Path::new("/games/Mercs2"),
            Path::new("/games/Mercs2/Mercenaries2.cracked.exe"),
            &RuntimeSettings::default(),
            verbose,
        )
        .expect("direct launch never fails to build")
    }

    #[test]
    fn runs_the_exe_from_the_install_dir() {
        let cmd = build(false);
        assert_eq!(
            cmd.get_program(),
            OsStr::new("/games/Mercs2/Mercenaries2.cracked.exe")
        );
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/games/Mercs2")));
    }

    #[test]
    fn verbose_log_is_pinned_either_way() {
        assert_eq!(env_of(&build(false), "PMC_VERBOSE_LOG"), Some(OsStr::new("0")));
        assert_eq!(env_of(&build(true), "PMC_VERBOSE_LOG"), Some(OsStr::new("1")));
    }

    /// Regression: without this the game can fail to launch with os error 740 on a
    /// machine that has a RUNASADMIN compat layer or shim entry for the exe.
    #[test]
    fn declines_uac_elevation() {
        assert_eq!(
            env_of(&build(false), "__COMPAT_LAYER"),
            Some(OsStr::new("RunAsInvoker"))
        );
    }
}

// ----------------------------------------------------------------------------
// macOS: modkit-managed Wine
// ----------------------------------------------------------------------------

/// Variables the macOS launcher sets itself; `env` may not set them.
#[cfg(target_os = "macos")]
const MAC_RESERVED: &[&str] = &["WINEPREFIX", "PMC_VERBOSE_LOG"];

/// The file registry values are written to inside the prefix before `reg import`.
#[cfg(target_os = "macos")]
const REGISTRY_FILE: &str = "modkit-registry.reg";

#[cfg(target_os = "macos")]
fn resolve_runtime(settings: &RuntimeSettings) -> RuntimeInfo {
    let mut info = RuntimeInfo {
        host: "macos".into(),
        ..Default::default()
    };
    match runtime_settings::resolve_prefix(settings, None) {
        Ok(p) => info.prefix = Some(p.to_string_lossy().into_owned()),
        Err(e) => info.notes.push(e),
    }
    match crate::commands::managed::wine::selected_wine64(settings) {
        Ok(p) => info.wine = Some(p.to_string_lossy().into_owned()),
        Err(e) => info.notes.push(e),
    }
    if let Err(e) = preflight_rosetta() {
        info.notes.push(e);
    }
    info
}

/// macOS: run the exe under the selected Wine build, after importing the
/// settings' registry values into the prefix.
#[cfg(target_os = "macos")]
fn build_command(
    game_dir: &Path,
    run_exe: &Path,
    settings: &RuntimeSettings,
    verbose: bool,
) -> Result<Command, String> {
    preflight_rosetta()?;
    let wine64 = crate::commands::managed::wine::selected_wine64(settings)?;
    let prefix = runtime_settings::resolve_prefix(settings, None)?;
    let env = settings.wine_env(MAC_RESERVED)?;
    std::fs::create_dir_all(&prefix)
        .map_err(|e| format!("Failed to create the Wine prefix {}: {e}", prefix.display()))?;

    write_global_ini(game_dir);
    if !settings.registry.is_empty() {
        import_registry(&wine64, &prefix, &env, settings)?;
    }
    Ok(wine_command(
        &wine64,
        &prefix,
        &env,
        game_dir,
        run_exe,
        &settings.exe_args,
        verbose,
    ))
}

/// The game command itself: `wine64 <exe> <args…>` in the game folder.
#[cfg(target_os = "macos")]
fn wine_command(
    wine64: &Path,
    prefix: &Path,
    env: &[(String, String)],
    game_dir: &Path,
    run_exe: &Path,
    exe_args: &[String],
    verbose: bool,
) -> Command {
    let mut cmd = Command::new(wine64);
    cmd.arg(run_exe).args(exe_args).current_dir(game_dir);
    cmd.env("WINEPREFIX", prefix);
    for (k, v) in env {
        cmd.env(k, v);
    }
    // Gate pmc_blackbox's verbose log hooks; Wine forwards the environment into
    // the game process, so the in-game DLL reads this.
    cmd.env("PMC_VERBOSE_LOG", if verbose { "1" } else { "0" });
    cmd
}

/// A host path as Wine sees it through the default `z:` → `/` drive.
#[cfg(target_os = "macos")]
fn z_drive_path(p: &Path) -> String {
    format!("Z:{}", p.to_string_lossy().replace('/', "\\"))
}

/// Write the registry values into the prefix with `wine64 reg import`, waiting for
/// it to finish. A failed import fails the launch with Wine's own output.
#[cfg(target_os = "macos")]
fn import_registry(
    wine64: &Path,
    prefix: &Path,
    env: &[(String, String)],
    settings: &RuntimeSettings,
) -> Result<(), String> {
    let reg = prefix.join(REGISTRY_FILE);
    std::fs::write(&reg, settings.registry_file())
        .map_err(|e| format!("Could not write {}: {e}", reg.display()))?;
    let mut cmd = Command::new(wine64);
    cmd.arg("reg").arg("import").arg(z_drive_path(&reg));
    cmd.env("WINEPREFIX", prefix);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("Could not run Wine to import registry values: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "Importing the registry values from {} into the prefix failed ({}):\n{}{}",
            reg.display(),
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

/// The Wine build is x86_64. On Apple Silicon it needs Rosetta 2.
#[cfg(target_os = "macos")]
fn preflight_rosetta() -> Result<(), String> {
    if !cfg!(target_arch = "aarch64") {
        return Ok(());
    }
    let ok = Command::new("/usr/bin/arch")
        .args(["-x86_64", "/usr/bin/true"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        Err("Rosetta 2 is not installed, and the Wine build needs it on Apple Silicon. Install it:\n  \
             softwareupdate --install-rosetta --agree-to-license"
            .into())
    }
}

/// Lists Proton builds; Linux only.
#[cfg(not(target_os = "linux"))]
#[tauri::command(async)]
pub fn select_proton(_proton: Option<String>) -> Result<(), String> {
    Err("Proton is selected only on Linux.".into())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn env_of<'a>(cmd: &'a Command, key: &str) -> Option<&'a OsStr> {
        cmd.get_envs()
            .find(|(k, _)| *k == OsStr::new(key))
            .and_then(|(_, v)| v)
    }

    fn build(verbose: bool, args: &[String]) -> Command {
        wine_command(
            Path::new("/w/bin/wine64"),
            Path::new("/p/wine-prefix"),
            &[("WINE_LARGE_ADDRESS_AWARE".into(), "1".into())],
            Path::new("/Users/me/Downloads/Mercs2"),
            Path::new("/Users/me/Downloads/Mercs2/Mercenaries2.cracked.exe"),
            args,
            verbose,
        )
    }

    #[test]
    fn runs_the_exe_under_wine64_from_the_game_folder() {
        let cmd = build(false, &["-windowed".into()]);
        assert_eq!(cmd.get_program(), OsStr::new("/w/bin/wine64"));
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(
            args,
            vec![
                OsStr::new("/Users/me/Downloads/Mercs2/Mercenaries2.cracked.exe"),
                OsStr::new("-windowed")
            ]
        );
        assert_eq!(
            cmd.get_current_dir(),
            Some(Path::new("/Users/me/Downloads/Mercs2"))
        );
    }

    #[test]
    fn sets_the_prefix_the_settings_env_and_verbose_flag() {
        let cmd = build(true, &[]);
        assert_eq!(env_of(&cmd, "WINEPREFIX"), Some(OsStr::new("/p/wine-prefix")));
        assert_eq!(env_of(&cmd, "WINE_LARGE_ADDRESS_AWARE"), Some(OsStr::new("1")));
        assert_eq!(env_of(&cmd, "PMC_VERBOSE_LOG"), Some(OsStr::new("1")));
        assert_eq!(env_of(&build(false, &[]), "PMC_VERBOSE_LOG"), Some(OsStr::new("0")));
    }

    #[test]
    fn a_host_path_maps_through_the_z_drive() {
        assert_eq!(
            z_drive_path(Path::new("/Users/me/p/modkit-registry.reg")),
            r"Z:\Users\me\p\modkit-registry.reg"
        );
    }
}

// ----------------------------------------------------------------------------
// Linux: Proton discovery + container launch
// ----------------------------------------------------------------------------

/// True when running inside a Flatpak sandbox.
#[cfg(target_os = "linux")]
pub(crate) fn in_flatpak() -> bool {
    Path::new("/.flatpak-info").exists() || std::env::var_os("FLATPAK_ID").is_some()
}

/// The real host home. Inside Flatpak `$HOME` is `<real_home>/.var/app/<id>`, but
/// Steam/Proton live under the real home (reachable via `--filesystem=host`), so
/// strip the per-app suffix. Outside Flatpak this is just `$HOME`.
#[cfg(target_os = "linux")]
fn home() -> Option<PathBuf> {
    let h = std::env::var_os("HOME").map(PathBuf::from)?;
    if let Some(s) = h.to_str() {
        if let Some(idx) = s.find("/.var/app/") {
            return Some(PathBuf::from(&s[..idx]));
        }
    }
    Some(h)
}

/// settings value → `MERCS2_*` env var → None (caller falls back to autodiscovery).
#[cfg(target_os = "linux")]
fn overridden(setting: &Option<String>, env: &str) -> Option<PathBuf> {
    setting
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| std::env::var_os(env).map(PathBuf::from))
}

/// Locate the Steam root that holds installed runtimes.
#[cfg(target_os = "linux")]
fn discover_steam_root() -> Option<PathBuf> {
    let home = home()?;
    for rel in [
        ".steam/debian-installation", // Debian/Ubuntu .deb Steam
        ".local/share/Steam",         // native runtime / SteamOS
        ".steam/steam",               // common symlink target
        ".steam/root",
        ".var/app/com.valvesoftware.Steam/.local/share/Steam", // Flatpak
    ] {
        let p = PathBuf::from(&home).join(rel);
        if p.join("steamapps").is_dir() {
            return Some(p);
        }
    }
    None
}

/// All Steam library roots: the Steam root plus every `"path"` in
/// `libraryfolders.vdf` (games/Proton can live on other drives / the SD card).
#[cfg(target_os = "linux")]
fn steam_libraries(steam_root: &Path) -> Vec<PathBuf> {
    let mut libs = vec![steam_root.to_path_buf()];
    for rel in ["steamapps/libraryfolders.vdf", "config/libraryfolders.vdf"] {
        if let Ok(text) = std::fs::read_to_string(steam_root.join(rel)) {
            for line in text.lines() {
                let t = line.trim();
                if let Some(rest) = t.strip_prefix("\"path\"") {
                    if let Some(s) = rest.find('"') {
                        if let Some(e) = rest[s + 1..].find('"') {
                            let p = PathBuf::from(&rest[s + 1..s + 1 + e]);
                            if !libs.contains(&p) {
                                libs.push(p);
                            }
                        }
                    }
                }
            }
        }
    }
    libs
}

/// Every Proton install, in preference order: official Experimental and Hotfix
/// across all libraries, then custom tools (Proton-GE) in `compatibilitytools.d`,
/// then any other `Proton*`. Each entry is a `proton` script. `home` is the real
/// host home ([`home`]), passed in so discovery is testable.
#[cfg(target_os = "linux")]
fn list_protons(steam_root: &Path, libs: &[PathBuf], home: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if p.is_file() && !out.contains(&p) {
            out.push(p);
        }
    };
    for name in ["Proton - Experimental", "Proton Hotfix"] {
        for lib in libs {
            push(lib.join("steamapps/common").join(name).join("proton"));
        }
    }
    // Custom compat tools (e.g. GE-Proton) — in the Steam root and ~/.steam/root.
    let mut tool_bases = vec![steam_root.join("compatibilitytools.d")];
    if let Some(h) = home {
        tool_bases.push(h.join(".steam/root/compatibilitytools.d"));
    }
    for base in tool_bases {
        if let Ok(rd) = std::fs::read_dir(&base) {
            let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            dirs.sort();
            for d in dirs {
                push(d.join("proton"));
            }
        }
    }
    // Any remaining Proton* install.
    for lib in libs {
        if let Ok(rd) = std::fs::read_dir(lib.join("steamapps/common")) {
            let mut dirs: Vec<PathBuf> = rd
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("Proton"))
                .map(|e| e.path())
                .collect();
            dirs.sort();
            for d in dirs {
                push(d.join("proton"));
            }
        }
    }
    out
}

/// Steam Linux Runtime (sniper) entry point, across all libraries.
#[cfg(target_os = "linux")]
fn discover_sniper(libs: &[PathBuf]) -> Option<PathBuf> {
    for lib in libs {
        let p = lib.join("steamapps/common/SteamLinuxRuntime_sniper/_v2-entry-point");
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Normalize a Proton setting that may be a dir or the `proton` script itself.
#[cfg(target_os = "linux")]
fn normalize_proton(p: PathBuf) -> PathBuf {
    if p.is_dir() {
        p.join("proton")
    } else {
        p
    }
}

/// Every runtime path, resolved with the settings → env → autodiscovery layering.
#[cfg(target_os = "linux")]
struct Resolved {
    steam_root: PathBuf,
    proton: PathBuf,
    protons: Vec<PathBuf>,
    sniper: Option<PathBuf>,
    prefix: PathBuf,
    use_container: bool,
}

#[cfg(target_os = "linux")]
fn resolve(settings: &RuntimeSettings) -> Result<Resolved, String> {
    let steam_root = overridden(&settings.steam_root, "MERCS2_STEAM_ROOT")
        .or_else(discover_steam_root)
        .ok_or("Steam install not found. Set steamRoot in the runtime settings or MERCS2_STEAM_ROOT.")?;
    let libs = steam_libraries(&steam_root);
    let protons = list_protons(&steam_root, &libs, home().as_deref());
    let proton = match &settings.proton {
        // A selected Proton is used or the launch fails — never swapped for another.
        Some(sel) => {
            let p = normalize_proton(PathBuf::from(sel));
            if !p.is_file() {
                return Err(format!(
                    "The selected Proton ({sel}) is no longer installed. Pick another in Game Info → Runtime."
                ));
            }
            p
        }
        None => std::env::var_os("MERCS2_PROTON")
            .map(|p| normalize_proton(PathBuf::from(p)))
            .or_else(|| protons.first().cloned())
            .ok_or("No Proton found. Install Proton via Steam, or set MERCS2_PROTON.")?,
    };
    let sniper = overridden(&settings.sniper, "MERCS2_SNIPER").or_else(|| discover_sniper(&libs));
    // Container by default; disabled by the setting, by MERCS2_NO_CONTAINER, or
    // if no sniper runtime exists.
    let use_container = settings.use_container.unwrap_or(true)
        && std::env::var_os("MERCS2_NO_CONTAINER").is_none()
        && sniper.is_some();
    let prefix = runtime_settings::resolve_prefix(settings, None)?;
    Ok(Resolved {
        steam_root,
        proton,
        protons,
        sniper,
        prefix,
        use_container,
    })
}

#[cfg(target_os = "linux")]
fn resolve_runtime(settings: &RuntimeSettings) -> RuntimeInfo {
    let lossy = |p: &Path| p.to_string_lossy().into_owned();
    match resolve(settings) {
        Ok(r) => {
            let mut notes = Vec::new();
            if !r.use_container {
                notes.push(
                    "No sniper runtime (or container disabled) — will run bare Proton; GPU setup may be incomplete.".into(),
                );
            }
            RuntimeInfo {
                host: "linux".into(),
                prefix: Some(lossy(&r.prefix)),
                steam_root: Some(lossy(&r.steam_root)),
                proton: Some(lossy(&r.proton)),
                protons: r.protons.iter().map(|p| lossy(p)).collect(),
                sniper: r.sniper.as_deref().map(lossy),
                container: r.use_container,
                notes,
                ..Default::default()
            }
        }
        Err(e) => {
            // Still list what is installed, so a stale selection can be replaced.
            let protons = overridden(&settings.steam_root, "MERCS2_STEAM_ROOT")
                .or_else(discover_steam_root)
                .map(|root| list_protons(&root, &steam_libraries(&root), home().as_deref()))
                .unwrap_or_default();
            RuntimeInfo {
                host: "linux".into(),
                protons: protons.iter().map(|p| lossy(p)).collect(),
                notes: vec![e],
                ..Default::default()
            }
        }
    }
}

/// Make `proton` (a discovered `proton` script) the one launches use, or clear
/// the selection with `None` to go back to `MERCS2_PROTON` / autodiscovery.
#[cfg(target_os = "linux")]
#[tauri::command(async)]
pub fn select_proton(proton: Option<String>) -> Result<(), String> {
    if let Some(p) = &proton {
        let script = normalize_proton(PathBuf::from(p));
        if !script.is_file() {
            return Err(format!("{p} is not a Proton install (no proton script)."));
        }
    }
    runtime_settings::update(|s| s.proton = proton)?;
    Ok(())
}

/// Linux: build the launch command from resolved runtime paths, after a preflight
/// that fails with an actionable fix for each known blocker.
#[cfg(target_os = "linux")]
fn build_command(
    game_dir: &Path,
    run_exe: &Path,
    settings: &RuntimeSettings,
    verbose: bool,
) -> Result<Command, String> {
    let r = resolve(settings)?;
    if !settings.registry.is_empty() {
        return Err(
            "The runtime settings set registry values, which modkit applies only on macOS so far. \
             Remove `registry` from runtime.json to launch."
                .into(),
        );
    }

    if r.use_container {
        preflight_userns()?;
    }
    preflight_nvidia()?;

    // ASI-loader config + the prefix dir.
    write_global_ini(game_dir);
    std::fs::create_dir_all(&r.prefix).map_err(|e| format!("Failed to create Proton prefix: {e}"))?;

    use std::ffi::OsString;

    // The proton/sniper invocation itself (program + arguments).
    let mut argv: Vec<OsString> = Vec::new();
    if r.use_container {
        let sniper = r.sniper.expect("use_container implies a sniper path");
        argv.push(sniper.into_os_string());
        argv.push("--verb=waitforexitandrun".into());
        argv.push("--".into());
    }
    argv.push(r.proton.clone().into_os_string());
    argv.push("waitforexitandrun".into());
    argv.push(run_exe.as_os_str().to_os_string());
    argv.extend(settings.exe_args.iter().map(OsString::from));

    // The pressure-vessel (sniper) container only exposes the home dir, the Steam
    // install, the compat-data prefix and the tool paths by default. A game that
    // lives outside that tree — a second drive, or the Steam Deck's microSD under
    // /run/media — is invisible inside the container unless we add it. Point the
    // install path at the game dir and bind-mount its canonical path so the exe
    // resolves on a Deck regardless of where the library sits.
    let game_mount = std::fs::canonicalize(game_dir).unwrap_or_else(|_| game_dir.to_path_buf());
    let mut envs: Vec<(String, OsString)> = vec![
        ("STEAM_COMPAT_CLIENT_INSTALL_PATH".into(), r.steam_root.clone().into_os_string()),
        ("STEAM_COMPAT_DATA_PATH".into(), r.prefix.clone().into_os_string()),
        ("STEAM_COMPAT_INSTALL_PATH".into(), game_mount.clone().into_os_string()),
        ("STEAM_COMPAT_MOUNTS".into(), game_mount.clone().into_os_string()),
        // Manual (non-Steam) launch: give Proton a stable app id so prefix/log
        // naming is deterministic and pressure-vessel doesn't warn. 0 = no app.
        ("SteamAppId".into(), "0".into()),
        ("SteamGameId".into(), "0".into()),
        ("STEAM_COMPAT_APP_ID".into(), "0".into()),
        ("PROTON_LOG".into(), "0".into()),
        // Gate pmc_blackbox's verbose log hooks. Proton forwards the process
        // environment into the Wine process, so the in-game DLL reads this.
        ("PMC_VERBOSE_LOG".into(), if verbose { "1".into() } else { "0".into() }),
    ];
    let reserved: Vec<&str> = envs.iter().map(|(k, _)| k.as_str()).collect();
    let user_env = settings.wine_env(&reserved)?;
    envs.extend(user_env.into_iter().map(|(k, v)| (k, OsString::from(v))));

    let cmd = if in_flatpak() {
        // Proton drives pressure-vessel/bwrap, which can't create the nested user
        // namespaces it needs from inside the Flatpak sandbox. Run it on the host
        // through the Flatpak portal instead (needs --talk-name=org.freedesktop.Flatpak
        // in the manifest). flatpak-spawn doesn't inherit our env, so pass each var
        // explicitly and set the host-side working directory.
        let mut c = Command::new("flatpak-spawn");
        // --watch-bus ties the host process to this proxy's D-Bus connection, so
        // stop_game (which kills the proxy) and modkit exiting also stop the game
        // instead of orphaning it on the host.
        c.arg("--host").arg("--watch-bus");
        let mut dir = OsString::from("--directory=");
        dir.push(&game_mount);
        c.arg(dir);
        for (k, v) in &envs {
            let mut e = OsString::from("--env=");
            e.push(k);
            e.push("=");
            e.push(v);
            c.arg(e);
        }
        c.arg("--");
        c.args(&argv);
        c
    } else {
        let mut c = Command::new(&argv[0]);
        c.args(&argv[1..]);
        c.current_dir(game_dir);
        for (k, v) in &envs {
            c.env(k, v);
        }
        c
    };
    Ok(cmd)
}

/// The Proton container (pressure-vessel/bwrap) needs unprivileged user
/// namespaces. Ubuntu 24.04 restricts them by default; SteamOS does not.
#[cfg(target_os = "linux")]
fn preflight_userns() -> Result<(), String> {
    let path = "/proc/sys/kernel/apparmor_restrict_unprivileged_userns";
    if let Ok(v) = std::fs::read_to_string(path) {
        if v.trim() != "0" {
            return Err(
                "Unprivileged user namespaces are restricted, so the Proton container can't \
                 start. Fix:\n  sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0\n\
                 Persist:\n  echo 'kernel.apparmor_restrict_unprivileged_userns=0' | \
                 sudo tee /etc/sysctl.d/60-steam-userns.conf"
                    .into(),
            );
        }
    }
    Ok(())
}

/// A 32-bit game needs the 32-bit NVIDIA Vulkan ICD, matching the running
/// driver. Without it, DXVK only sees llvmpipe and renders in software. This is
/// NVIDIA-on-Debian/Ubuntu-specific (AMD/Intel/SteamOS ship 32-bit Mesa), so the
/// check no-ops unless a 64-bit NVIDIA GL lib is present.
#[cfg(target_os = "linux")]
fn preflight_nvidia() -> Result<(), String> {
    // Common multiarch (Debian/Ubuntu) and flat (Arch) 64-bit NVIDIA lib paths.
    let lib64 = ["/usr/lib/x86_64-linux-gnu/libGLX_nvidia.so.0", "/usr/lib/libGLX_nvidia.so.0"]
        .iter()
        .map(Path::new)
        .find(|p| p.exists());
    let Some(lib64) = lib64 else {
        return Ok(()); // not an NVIDIA system — nothing to check
    };
    let branch = nvidia_branch(lib64).unwrap_or_else(|| "PPP".to_string());

    // Corresponding 32-bit paths (Debian multiarch / Arch lib32).
    let lib32_present = ["/usr/lib/i386-linux-gnu/libGLX_nvidia.so.0", "/usr/lib32/libGLX_nvidia.so.0"]
        .iter()
        .any(|p| Path::new(p).exists());
    if !lib32_present {
        return Err(format!(
            "The 32-bit NVIDIA driver is missing, so the 32-bit game renders in software. \
             Install it and reboot (Debian/Ubuntu shown):\n  sudo apt install libnvidia-gl-{branch}:i386\n  sudo reboot"
        ));
    }

    // Driver/library mismatch (upgraded but not rebooted) breaks NVIDIA entirely.
    if let Ok(out) = Command::new("nvidia-smi").arg("-L").output() {
        let txt = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if !out.status.success() || txt.contains("mismatch") {
            return Err("NVIDIA driver/library version mismatch — reboot to load the matching kernel module before launching.".into());
        }
    }
    Ok(())
}

/// Driver branch (e.g. "595") from `libGLX_nvidia.so.0 -> libGLX_nvidia.so.595.71.05`.
#[cfg(target_os = "linux")]
fn nvidia_branch(lib64: &Path) -> Option<String> {
    let target = std::fs::read_link(lib64).ok()?;
    let name = target.file_name()?.to_string_lossy().into_owned();
    let ver = name.rsplit(".so.").next()?; // "595.71.05"
    ver.split('.').next().map(|s| s.to_string()) // "595"
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"#!/bin/sh").unwrap();
    }

    /// Every install is listed (that is what makes Proton selectable), official
    /// builds first, each once.
    #[test]
    fn every_proton_is_listed_in_preference_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Steam");
        let other = dir.path().join("Library2");
        touch(&root.join("steamapps/common/Proton 9.0/proton"));
        touch(&other.join("steamapps/common/Proton - Experimental/proton"));
        touch(&root.join("compatibilitytools.d/GE-Proton10-1/proton"));
        std::fs::create_dir_all(root.join("steamapps/common/Proton Broken")).unwrap();

        let got = list_protons(&root, &[root.clone(), other.clone()], None);
        assert_eq!(
            got,
            vec![
                other.join("steamapps/common/Proton - Experimental/proton"),
                root.join("compatibilitytools.d/GE-Proton10-1/proton"),
                root.join("steamapps/common/Proton 9.0/proton"),
            ]
        );
    }

    #[test]
    fn a_selected_proton_that_is_gone_fails_rather_than_being_swapped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Steam");
        touch(&root.join("steamapps/common/Proton 9.0/proton"));
        std::fs::create_dir_all(root.join("steamapps")).unwrap();
        let settings = RuntimeSettings {
            steam_root: Some(root.to_string_lossy().into_owned()),
            proton: Some(dir.path().join("gone/proton").to_string_lossy().into_owned()),
            prefix: Some(dir.path().join("pfx").to_string_lossy().into_owned()),
            ..Default::default()
        };
        let err = resolve(&settings).err().expect("must fail");
        assert!(err.contains("no longer installed"), "{err}");
    }
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
