//! Launch the game from within the modkit.
//!
//! We track the child process we spawn in Tauri-managed state so that:
//!   - launching is atomic: the mutex guard spans the is-running check and the
//!     spawn, so we can never start a second instance of the game we own;
//!   - the UI can poll whether our instance is still alive and reflect it;
//!   - the user can stop the instance we started.
//!
//! On Linux the game is a 32-bit Windows D3D9 title, so we run it through Steam
//! Proton *inside the Steam Linux Runtime (sniper) container* — the verified
//! recipe that reaches the world rendering on the discrete GPU. Runtime paths are
//! **auto-discovered** (Steam root, every library in `libraryfolders.vdf`,
//! `compatibilitytools.d` for Proton-GE, the sniper runtime) but each can be
//! **overridden** — explicit arg → `MERCS2_*` env var → autodiscovery — so users
//! on non-Debian / non-SteamOS layouts can point at their own paths.
//!
//! Two host prerequisites are non-obvious and are checked in preflight with an
//! actionable error: unprivileged user namespaces must be allowed (container), and
//! the 32-bit NVIDIA driver libs must be installed and match the running module
//! (else 32-bit DXVK only sees llvmpipe and renders in software). On Windows/macOS
//! we spawn the exe directly.
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

/// The single game process modkit has spawned (if any). Managed by Tauri.
#[derive(Default)]
pub struct GameProcess(pub Mutex<Option<Child>>);

/// User-supplied overrides for runtime discovery (any field may be null).
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchOverrides {
    /// Steam root (the dir that holds `steamapps/`).
    pub steam_root: Option<String>,
    /// Proton dir or the `proton` script itself.
    pub proton: Option<String>,
    /// Steam Linux Runtime `_v2-entry-point`.
    pub sniper: Option<String>,
    /// Proton compat-data prefix.
    pub prefix: Option<String>,
    /// Run through the sniper container (default true; false = bare `proton run`).
    pub use_container: Option<bool>,
}

/// What runtime discovery resolved to — surfaced to the UI so users can confirm
/// or override before launching.
#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInfo {
    pub steam_root: Option<String>,
    pub proton: Option<String>,
    pub sniper: Option<String>,
    /// Whether a launch would run inside the sniper container.
    pub container: bool,
    /// Non-fatal notes (e.g. "no sniper runtime found — will run bare Proton").
    pub notes: Vec<String>,
}

/// ASI-loader config the engine expects next to the exe (mirrors the verified
/// Windows baseline). `DontLoadFromDllMain=0` arms the SecuROM spoof during
/// DllMain, before the entry point. Only written on the Linux/Proton launch path.
#[cfg(target_os = "linux")]
const GLOBAL_INI: &str =
    "[GlobalSets]\nLoadPlugins=1\nDontLoadFromDllMain=0\nLoadFromScriptsOnly=0\nLoadRecursively=1\n";

/// Spawn the game, with the install folder as the working directory so it
/// resolves its data files and side-by-side DLLs. Refuses to start a second
/// instance while the one we launched is still running.
#[tauri::command]
pub fn launch_game(
    state: State<GameProcess>,
    exe_path: String,
    game_root: Option<String>,
    overrides: Option<LaunchOverrides>,
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
    let ov = overrides.unwrap_or_default();
    // Verbose pmc_blackbox log hooks are opt-in per launch (expensive); default off.
    let verbose = verbose_log.unwrap_or(false);

    let mut cmd = build_command(&game_dir, &run_exe, &ov, verbose)?;
    let child = cmd
        .spawn()
        .map_err(|e| format!("Failed to launch game: {e}"))?;
    *guard = Some(child);
    Ok(())
}

/// Report what runtime discovery resolves to (honoring the same overrides), so
/// the UI can display it / let the user correct it before launching.
#[tauri::command]
pub fn discover_runtime(overrides: Option<LaunchOverrides>) -> RuntimeInfo {
    let _ov = overrides.unwrap_or_default();
    #[cfg(target_os = "linux")]
    {
        return resolve_runtime(&_ov);
    }
    #[cfg(not(target_os = "linux"))]
    {
        RuntimeInfo {
            notes: vec!["Direct launch (no Proton) on this OS.".into()],
            ..Default::default()
        }
    }
}

/// Pick the executable to actually launch: prefer `Mercenaries2.cracked.exe`
/// (de-DRM'd, imports the ASI loader) over the detected/stock exe.
fn launch_exe(game_dir: &Path, detected: &Path) -> PathBuf {
    let cracked = game_dir.join("Mercenaries2.cracked.exe");
    if cracked.is_file() {
        cracked
    } else {
        detected.to_path_buf()
    }
}

/// Windows/macOS: spawn the exe directly with the install dir as the cwd.
#[cfg(not(target_os = "linux"))]
fn build_command(
    game_dir: &Path,
    run_exe: &Path,
    _ov: &LaunchOverrides,
    verbose: bool,
) -> Result<Command, String> {
    let mut cmd = Command::new(run_exe);
    cmd.current_dir(game_dir);
    // Gate pmc_blackbox's verbose log hooks; set explicitly so an inherited
    // value can't silently turn it back on.
    cmd.env("PMC_VERBOSE_LOG", if verbose { "1" } else { "0" });
    Ok(cmd)
}

// ----------------------------------------------------------------------------
// Linux: Proton discovery + container launch
// ----------------------------------------------------------------------------

/// True when running inside a Flatpak sandbox.
#[cfg(target_os = "linux")]
fn in_flatpak() -> bool {
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

/// override arg → `MERCS2_*` env var → None (caller falls back to autodiscovery).
#[cfg(target_os = "linux")]
fn overridden(arg: &Option<String>, env: &str) -> Option<PathBuf> {
    arg.as_ref()
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

/// Proton: preferred official builds across all libraries, then custom tools
/// (Proton-GE) in `compatibilitytools.d`, then any `Proton*`.
#[cfg(target_os = "linux")]
fn discover_proton(steam_root: &Path, libs: &[PathBuf]) -> Option<PathBuf> {
    for name in ["Proton - Experimental", "Proton Hotfix"] {
        for lib in libs {
            let p = lib.join("steamapps/common").join(name).join("proton");
            if p.is_file() {
                return Some(p);
            }
        }
    }
    // Custom compat tools (e.g. GE-Proton) — in the Steam root and ~/.steam/root.
    let mut tool_bases = vec![steam_root.join("compatibilitytools.d")];
    if let Some(h) = home() {
        tool_bases.push(h.join(".steam/root/compatibilitytools.d"));
    }
    for base in tool_bases {
        if let Ok(rd) = std::fs::read_dir(&base) {
            for e in rd.flatten() {
                let p = e.path().join("proton");
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    // Any remaining Proton* install.
    for lib in libs {
        if let Ok(rd) = std::fs::read_dir(lib.join("steamapps/common")) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with("Proton") {
                    let p = e.path().join("proton");
                    if p.is_file() {
                        return Some(p);
                    }
                }
            }
        }
    }
    None
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

/// Normalize a Proton override that may be a dir or the `proton` script itself.
#[cfg(target_os = "linux")]
fn normalize_proton(p: PathBuf) -> PathBuf {
    if p.is_dir() {
        p.join("proton")
    } else {
        p
    }
}

/// Resolve every runtime path with the override → env → autodiscovery layering.
#[cfg(target_os = "linux")]
struct Resolved {
    steam_root: PathBuf,
    proton: PathBuf,
    sniper: Option<PathBuf>,
    prefix: PathBuf,
    use_container: bool,
}

#[cfg(target_os = "linux")]
fn resolve(ov: &LaunchOverrides) -> Result<Resolved, String> {
    let steam_root = overridden(&ov.steam_root, "MERCS2_STEAM_ROOT")
        .or_else(discover_steam_root)
        .ok_or("Steam install not found. Set it via overrides or MERCS2_STEAM_ROOT.")?;
    let libs = steam_libraries(&steam_root);
    let proton = overridden(&ov.proton, "MERCS2_PROTON")
        .map(normalize_proton)
        .or_else(|| discover_proton(&steam_root, &libs))
        .ok_or("No Proton found. Install Proton via Steam, or set MERCS2_PROTON.")?;
    let sniper = overridden(&ov.sniper, "MERCS2_SNIPER").or_else(|| discover_sniper(&libs));
    // Container by default; disabled by override, by MERCS2_NO_CONTAINER, or if
    // no sniper runtime exists.
    let use_container = ov.use_container.unwrap_or(true)
        && std::env::var_os("MERCS2_NO_CONTAINER").is_none()
        && sniper.is_some();
    let prefix = overridden(&ov.prefix, "MERCS2_PREFIX")
        .map(Ok)
        .unwrap_or_else(|| crate::commands::paths::app_data_dir().map(|d| d.join("proton-prefix")))?;
    Ok(Resolved {
        steam_root,
        proton,
        sniper,
        prefix,
        use_container,
    })
}

#[cfg(target_os = "linux")]
fn resolve_runtime(ov: &LaunchOverrides) -> RuntimeInfo {
    match resolve(ov) {
        Ok(r) => {
            let mut notes = Vec::new();
            if !r.use_container {
                notes.push(
                    "No sniper runtime (or container disabled) — will run bare Proton; GPU setup may be incomplete.".into(),
                );
            }
            RuntimeInfo {
                steam_root: Some(r.steam_root.to_string_lossy().into()),
                proton: Some(r.proton.to_string_lossy().into()),
                sniper: r.sniper.map(|p| p.to_string_lossy().into()),
                container: r.use_container,
                notes,
            }
        }
        Err(e) => RuntimeInfo {
            notes: vec![e],
            ..Default::default()
        },
    }
}

/// Linux: build the launch command from resolved/overridden runtime paths, after
/// a preflight that fails with an actionable fix for each known blocker.
#[cfg(target_os = "linux")]
fn build_command(
    game_dir: &Path,
    run_exe: &Path,
    ov: &LaunchOverrides,
    verbose: bool,
) -> Result<Command, String> {
    let r = resolve(ov)?;

    if r.use_container {
        preflight_userns()?;
    }
    preflight_nvidia()?;

    // ASI-loader config + the prefix dir.
    let scripts = game_dir.join("scripts");
    let _ = std::fs::create_dir_all(&scripts);
    let global_ini = scripts.join("global.ini");
    if !global_ini.exists() {
        let _ = std::fs::write(&global_ini, GLOBAL_INI);
    }
    std::fs::create_dir_all(&r.prefix).map_err(|e| format!("Failed to create Proton prefix: {e}"))?;

    use std::ffi::OsString;

    // The proton/sniper invocation itself (program + arguments).
    let mut argv: Vec<OsString> = Vec::new();
    if r.use_container {
        let sniper = r.sniper.expect("use_container implies a sniper path");
        argv.push(sniper.into_os_string());
        argv.push("--verb=waitforexitandrun".into());
        argv.push("--".into());
        argv.push(r.proton.clone().into_os_string());
        argv.push("waitforexitandrun".into());
        argv.push(run_exe.as_os_str().to_os_string());
    } else {
        argv.push(r.proton.clone().into_os_string());
        argv.push("waitforexitandrun".into());
        argv.push(run_exe.as_os_str().to_os_string());
    }

    // The pressure-vessel (sniper) container only exposes the home dir, the Steam
    // install, the compat-data prefix and the tool paths by default. A game that
    // lives outside that tree — a second drive, or the Steam Deck's microSD under
    // /run/media — is invisible inside the container unless we add it. Point the
    // install path at the game dir and bind-mount its canonical path so the exe
    // resolves on a Deck regardless of where the library sits.
    let game_mount = std::fs::canonicalize(game_dir).unwrap_or_else(|_| game_dir.to_path_buf());
    let envs: [(&str, OsString); 9] = [
        ("STEAM_COMPAT_CLIENT_INSTALL_PATH", r.steam_root.clone().into_os_string()),
        ("STEAM_COMPAT_DATA_PATH", r.prefix.clone().into_os_string()),
        ("STEAM_COMPAT_INSTALL_PATH", game_mount.clone().into_os_string()),
        ("STEAM_COMPAT_MOUNTS", game_mount.clone().into_os_string()),
        // Manual (non-Steam) launch: give Proton a stable app id so prefix/log
        // naming is deterministic and pressure-vessel doesn't warn. 0 = no app.
        ("SteamAppId", "0".into()),
        ("SteamGameId", "0".into()),
        ("STEAM_COMPAT_APP_ID", "0".into()),
        ("PROTON_LOG", "0".into()),
        // Gate pmc_blackbox's verbose log hooks. Proton forwards the process
        // environment into the Wine process, so the in-game DLL reads this.
        ("PMC_VERBOSE_LOG", if verbose { "1".into() } else { "0".into() }),
    ];

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
