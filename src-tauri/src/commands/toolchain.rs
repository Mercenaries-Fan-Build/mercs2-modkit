//! The Workshop toolset — modkit-managed installs of the `mercs2-wad-simulator`
//! release binaries.
//!
//! # Why modkit owns this
//!
//! The toolset ships 11 executables across 6 platform triples as *bare* release
//! assets. None of them knows its own version and none can update itself, so a
//! user who downloaded `wad_simulator` once stays on that build forever — which
//! is exactly what the old [`super::validator`] cache did (`if dest.exists() {
//! return }`, no version check, ever). Modkit already updates itself
//! (`tauri-plugin-updater`, signed manifest) and the core components
//! ([`super::updates::latest_release`]); this module extends the same idea to the
//! toolset, which is the only piece of the ecosystem that had no update path at
//! all.
//!
//! # Layout
//!
//! ```text
//! <cache>/mercs2-modkit/toolset/
//!     installed.json      { "tag": "v0.9.3", "tools": { "wad_simulator": "wad_simulator" } }
//!     v0.9.3/             wad_simulator  model_forge  mercs2_workshop  workshop_data/  …
//! ```
//!
//! Binaries are stored under their PLAIN names, not the platform-suffixed asset
//! names they are published under — see [`local_name`]. That is what lets
//! [`open_tool_shell`] put a single directory on `PATH` and have
//! `wad_simulator --help` work.
//!
//! The directory is keyed by release tag and a binary is **never overwritten in
//! place**. On Windows a running executable cannot be replaced, so having
//! `mercs2_workshop` open would otherwise fail the entire update. A new tag
//! downloads into a new directory, the pointer in `installed.json` flips at the
//! end, and the stale directory is pruned afterwards — best-effort, because a
//! still-running binary keeps its file locked; the next prune collects it.
//!
//! The sidecar exists because the asset names carry no version. `wad_simulator-
//! linux-x86_64` is the same string in every release, so the filesystem alone
//! cannot answer "am I current?".
//!
//! # One release, one check
//!
//! Every tool is published by a single release of a single repo, so checking the
//! whole toolset costs ONE GitHub API call — not one per tool. That is what makes
//! "modkit manages the toolset" cheap enough to do on every launch.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Window};

use super::managed::{place, PlaceOpts};
use super::net;

/// GitHub repo whose releases publish the toolset.
const REPO: &str = "Mercenaries-Fan-Build/mercs2-wad-simulator";

/// A toolset binary smaller than this is not a build — a forge error page served
/// with a 200, or a truncated transfer.
const MIN_TOOL_SIZE: u64 = 64 * 1024;

/// Name of the sidecar recording which release is installed.
const STATE_FILE: &str = "installed.json";

// ----------------------------------------------------------------------------
// The tool table
// ----------------------------------------------------------------------------

/// One publishable binary from the toolset release.
struct Tool {
    /// Asset stem — the binary name, without the platform suffix.
    name: &'static str,
    /// Human label for the UI.
    label: &'static str,
    /// One line on what it is for, in Tier-1 language.
    blurb: &'static str,
    /// True for the two windowed programs — the Workshop and the native game.
    /// Everything else is a command-line tool, which is a different thing to
    /// install: you launch these, you invoke those from a terminal.
    ///
    /// Distinct from [`Tool::driven_by_modkit`], which is about who runs it, and
    /// from [`Tool::sixty_four_bit_only`], which is about what the release
    /// builds. They coincide today only because both windowed apps are the
    /// engine-backed ones.
    windowed: bool,
    /// True when modkit shells out to this tool itself, so it must be installed
    /// for a modkit feature to work. False means the user runs it by hand.
    driven_by_modkit: bool,
    /// True for the engine-backed apps. `mercs2_engine` is 64-bit by design (its
    /// gilrs/cpal deps link C system libs that need a full i386 sysroot), so the
    /// release matrix builds `mercs2_game`/`mercs2_workshop` for x86_64 and
    /// arm64 only — there is no i686 asset to install.
    sixty_four_bit_only: bool,
    /// A platform-independent data bundle that must sit next to the binary.
    companion: Option<Companion>,
    /// Not yet faithful to the retail game — offered for testing, not for
    /// playing.
    ///
    /// Flagged in the UI so nobody installs it expecting Mercenaries 2 and
    /// reports the differences as bugs. It stays on the page because it does
    /// need testing; the label is what makes shipping it honest.
    experimental: bool,
    /// This tool cannot start without a Mercenaries 2 install to read.
    ///
    /// Its own fallback, when `--game-dir`/`MERCS2_GAME_DIR` are absent, is the
    /// EA Games *registry key* — which does not exist off Windows. So on macOS
    /// and Linux "no game dir" is not a soft default, it is an immediate exit,
    /// and modkit already knows the answer. Checked before launching so the user
    /// gets a modkit-level instruction instead of engine output about a registry
    /// key their OS does not have.
    requires_game_dir: bool,
}

/// A zip published in the same release that unpacks BESIDE a tool's executable.
///
/// The Workshop resolves its reference data (name pack, registry rows, spawnable
/// templates, ECS schemas, the Lua corpus) as `workshop_data/` next to its own
/// exe — `mercs2_workshop::index::data_home`, which checks `MERCS2_WORKSHOP_DATA`,
/// then exe-relative, then a CWD walk-up. An installed user matches none of the
/// fallbacks, so shipping the bare binary would give them a Workshop with none of
/// its reference data. The bundle is part of the install, not an optional extra.
struct Companion {
    /// Release asset name.
    asset: &'static str,
    /// Top-level directory inside the zip, which is also the name the tool looks
    /// for beside its exe. Extracting at the version dir lands it exactly there.
    dir: &'static str,
}

/// Everything the release publishes, in the order the page lists it.
const TOOLS: &[Tool] = &[
    Tool {
        name: "mercs2_workshop",
        label: "Workshop",
        blurb: "Browse, inspect and remix game assets on the engine renderer.",
        windowed: true,
        driven_by_modkit: false,
        sixty_four_bit_only: true,
        companion: Some(Companion {
            asset: "mercs2-workshop-data.zip",
            dir: "workshop_data",
        }),
        experimental: false,
        // Tolerant: it opens with whatever WADs it can find and is still useful
        // for inspecting the bundled reference data with no install at all.
        requires_game_dir: false,
    },
    Tool {
        name: "mercs2_game",
        label: "Game (native)",
        blurb: "The open-source engine reimplementation running the retail assets. \
                Not yet faithful to the original — expect missing and wrong \
                behaviour. Here to be tested, not played.",
        windowed: true,
        driven_by_modkit: false,
        sixty_four_bit_only: true,
        companion: None,
        experimental: true,
        // Hard requirement: it plays the retail content, so with no install it
        // exits 1 rather than degrading.
        requires_game_dir: true,
    },
    Tool {
        name: "qm",
        label: "Quartermaster",
        blurb: "Checks a mod folder for the mistakes that hang the game, then \
                builds it into a WAD.",
        windowed: false,
        // Modkit builds the end user's Shipment THROUGH qm — it is what packs and
        // composes a mod folder into a game-ready WAD, so a missing qm breaks
        // building, not just a hand-run command. Users also invoke it directly to
        // lint without any game content, which is why it is listed rather than
        // hidden as an implementation detail.
        driven_by_modkit: true,
        // The release publishes `qm` for 64-bit targets only. It is authoring
        // tooling — it reads a mod folder and writes a WAD, never running inside
        // the game's 32-bit process — so unlike an injected tool it has no reason
        // to match the game's bitness, and there is no i686 asset to install.
        sixty_four_bit_only: true,
        companion: None,
        experimental: false,
        // `qm build` needs the game, but `qm lint` deliberately does not — it is
        // manifest text plus the mod folder, no install and no network, which is
        // what lets it run in a public CI job. Requiring a game dir here would
        // block the half that is meant to work without one.
        requires_game_dir: false,
    },
    Tool {
        name: "wad_simulator",
        label: "WAD Simulator",
        blurb: "Validates a built patch WAD the way the engine consumes it.",
        windowed: false,
        driven_by_modkit: true,
        sixty_four_bit_only: false,
        companion: None,
        experimental: false,
        requires_game_dir: false,
    },
    Tool {
        name: "wad_builder",
        label: "WAD Builder",
        blurb: "Builds engine WADs from raw or edited assets.",
        windowed: false,
        driven_by_modkit: true,
        sixty_four_bit_only: false,
        companion: None,
        experimental: false,
        requires_game_dir: false,
    },
    Tool {
        name: "model_forge",
        label: "Model Forge",
        blurb: "Turns meshes into the model blobs the engine loads.",
        windowed: false,
        driven_by_modkit: true,
        sixty_four_bit_only: false,
        companion: None,
        experimental: false,
        requires_game_dir: false,
    },
    Tool {
        name: "mercs2_smuggler",
        label: "Smuggler",
        blurb: "Moves assets between WADs, preserving their ASET rows.",
        windowed: false,
        driven_by_modkit: false,
        sixty_four_bit_only: false,
        companion: None,
        experimental: false,
        requires_game_dir: false,
    },
    Tool {
        name: "inject_parts",
        label: "Inject Parts",
        blurb: "Injects skinned part meshes into an existing model.",
        windowed: false,
        driven_by_modkit: true,
        sixty_four_bit_only: false,
        companion: None,
        experimental: false,
        requires_game_dir: false,
    },
    Tool {
        name: "inject_static",
        label: "Inject Static",
        blurb: "Injects static (unskinned) geometry into an existing model.",
        windowed: false,
        driven_by_modkit: true,
        sixty_four_bit_only: false,
        companion: None,
        experimental: false,
        requires_game_dir: false,
    },
    Tool {
        name: "ucfx_byteswap",
        label: "UCFX Byteswap",
        blurb: "Converts Xbox 360 big-endian UCFX blocks to PC layout.",
        windowed: false,
        driven_by_modkit: false,
        sixty_four_bit_only: false,
        companion: None,
        experimental: false,
        requires_game_dir: false,
    },
    Tool {
        name: "loadprobe",
        label: "Load Probe",
        blurb: "Quantifies world-load progress from pmc_blackbox.log.",
        windowed: false,
        driven_by_modkit: false,
        sixty_four_bit_only: false,
        companion: None,
        experimental: false,
        requires_game_dir: false,
    },
    Tool {
        name: "securom_unwrap",
        label: "SecuROM Unwrap",
        blurb: "Unwraps the SecuROM layer from a retail executable.",
        windowed: false,
        driven_by_modkit: false,
        sixty_four_bit_only: false,
        companion: None,
        experimental: false,
        requires_game_dir: false,
    },
];

fn tool(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.name == name)
}

// ----------------------------------------------------------------------------
// Platform
// ----------------------------------------------------------------------------

/// The release-asset suffix for the host, matching the `suffix:` values in the
/// toolset's release workflow.
///
/// One definition, in [`super::net::release`], shared with the other repos modkit
/// downloads from. It had drifted: this module spelled ARM `arm64` (correct, and
/// what every release publishes) while `setup.rs` spelled it `aarch64`, so an
/// exact-match rule there could never fire on an ARM host. A cross-module test
/// existed to catch exactly that, and now guards one definition instead of two.
///
/// The old validator hardcoded `-macos-x86_64`, so every Apple Silicon user ran
/// the Intel build under Rosetta and the published `-macos-arm64` asset was never
/// once downloaded. Arch is read, not assumed.
///
/// `None` on a host no release builds for (BSD, riscv, 32-bit ARM), which degrades
/// to "unavailable" per tool rather than erroring — the page still renders and
/// says why.
fn platform_suffix() -> Option<String> {
    super::net::release::platform_suffix()
}

/// True when the host is 64-bit, i.e. the engine-backed apps have an asset.
fn host_is_64_bit() -> bool {
    matches!(std::env::consts::ARCH, "x86_64" | "aarch64")
}

/// The asset name this host needs for `t`, or `None` when the release does not
/// publish that combination.
fn asset_name(t: &Tool) -> Option<String> {
    if t.sixty_four_bit_only && !host_is_64_bit() {
        return None;
    }
    Some(format!("{}{}", t.name, platform_suffix()?))
}

/// What the binary is called ON DISK once installed: its plain name, plus `.exe`
/// on Windows.
///
/// Deliberately not the asset name. The release publishes
/// `wad_simulator-linux-x86_64` so that six platforms can coexist as flat files
/// on one release page, but that is a *transport* name — keeping it on disk would
/// mean [`open_tool_shell`] put a directory on `PATH` whose commands are all
/// called `wad_simulator-linux-x86_64`. The platform is already implied by the
/// machine doing the running, so it is dropped at install time.
fn local_name(t: &Tool) -> String {
    let ext = if cfg!(windows) { ".exe" } else { "" };
    format!("{}{ext}", t.name)
}

// ----------------------------------------------------------------------------
// On-disk state
// ----------------------------------------------------------------------------

/// `<cache>/mercs2-modkit/toolset`, created on demand.
fn toolset_root() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| "LOCALAPPDATA not set".to_string())?;
    #[cfg(not(target_os = "windows"))]
    let base = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".cache"))
        .ok_or_else(|| "HOME not set".to_string())?;

    let dir = base.join("mercs2-modkit").join("toolset");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create toolset dir: {e}"))?;
    Ok(dir)
}

/// What modkit last installed. Absent/corrupt reads as "nothing installed",
/// which is recoverable: the next install rewrites it.
#[derive(Debug, Default, Serialize, Deserialize)]
struct InstalledState {
    /// Release tag the installed binaries came from, e.g. `v0.9.3`.
    tag: String,
    /// Tool name -> asset file name, for the tools actually on disk.
    tools: BTreeMap<String, String>,
}

fn read_state(root: &Path) -> InstalledState {
    std::fs::read_to_string(root.join(STATE_FILE))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Write the sidecar via a temp file + rename, so an interrupted write cannot
/// leave a half-parsed state pointing at binaries that do not exist.
fn write_state(root: &Path, state: &InstalledState) -> Result<(), String> {
    let tmp = root.join("installed.json.tmp");
    let json =
        serde_json::to_string_pretty(state).map_err(|e| format!("Failed to encode state: {e}"))?;
    std::fs::write(&tmp, json).map_err(|e| format!("Failed to write state: {e}"))?;
    std::fs::rename(&tmp, root.join(STATE_FILE))
        .map_err(|e| format!("Failed to commit state: {e}"))
}

/// Drop every version directory except `keep`, plus the legacy `../bin` cache the
/// single-tool validator used before this module existed.
///
/// Best-effort throughout: on Windows a tool the user is running right now holds
/// its file open and the removal fails. That is fine — the stale directory is no
/// longer referenced by `installed.json`, and the next prune collects it.
fn prune(root: &Path, keep: &str) {
    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if e.path().is_dir() && name != keep {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    if let Some(legacy) = root.parent().map(|p| p.join("bin")) {
        let _ = std::fs::remove_dir_all(legacy);
    }
}

// ----------------------------------------------------------------------------
// Wire types
// ----------------------------------------------------------------------------

/// One row on the Workshop Tools page.
#[derive(Debug, Serialize)]
pub struct ToolStatus {
    pub name: String,
    pub label: String,
    pub blurb: String,
    /// A windowed program you launch, rather than a command-line tool.
    pub windowed: bool,
    /// Modkit shells out to this tool itself.
    pub driven_by_modkit: bool,
    /// False when the release publishes no asset for this host (a 64-bit-only app
    /// on a 32-bit machine, or an unsupported OS/arch). The UI greys these out
    /// with the reason rather than offering a button that cannot work.
    pub available: bool,
    /// Absolute path to the installed executable, or null.
    pub path: Option<String>,
    /// Size on disk in bytes, when installed. The executable only — a companion
    /// bundle is not counted.
    pub size: Option<u64>,
    /// Not yet faithful to the retail game — labelled so nobody mistakes it for
    /// a finished port.
    pub experimental: bool,
    /// This tool cannot start without a Mercenaries 2 install, so the page can
    /// say so up front rather than after a failed launch.
    pub requires_game_dir: bool,
    /// Name of the data bundle this tool needs beside it, or null if it needs
    /// none. Lets the page say the install is more than one file.
    pub companion_dir: Option<String>,
    /// Whether that bundle is unpacked. False alongside an installed `path`
    /// means a half-finished install — re-running the install repairs it.
    pub companion_ready: bool,
}

/// Toolset-wide status: what is installed, what the latest release is, and
/// whether those differ.
#[derive(Debug, Serialize)]
pub struct ToolsetStatus {
    /// Release tag currently installed, or null if nothing is.
    pub installed_tag: Option<String>,
    /// Latest published tag. Null when the lookup was skipped or failed
    /// (offline) — the page still renders what is installed.
    pub latest_tag: Option<String>,
    /// True when a newer release exists AND at least one tool is installed.
    /// Nothing installed is not "out of date", it is "not set up".
    pub update_available: bool,
    /// Directory "Open folder" opens. The version directory once something is
    /// installed, otherwise the toolset root — which [`toolset_root`] has already
    /// created, so it is always a real directory.
    ///
    /// Never null. It used to be `None` until the first successful install, which
    /// meant the button silently did not render at all — indistinguishable, from
    /// the outside, from a button that does nothing when clicked.
    pub dir: String,
    pub tools: Vec<ToolStatus>,
}

/// Emitted as `toolset-progress` while installing, so a 60 MB `mercs2_game`
/// download is not a silent hang.
#[derive(Clone, Serialize)]
struct ProgressEvent {
    tool: String,
    label: String,
    done: usize,
    total: usize,
}

// ----------------------------------------------------------------------------
// GitHub
// ----------------------------------------------------------------------------

/// `(tag, {asset_name: asset})` for the toolset's latest release.
///
/// Kept as a map rather than using [`Release::pick`]: this is the one caller that
/// wants *every* asset at once, because a single release supplies eleven binaries
/// plus their data bundles and the whole point is that checking the toolset costs
/// one API call rather than eleven.
async fn latest_toolset(
    client: &reqwest::Client,
) -> Result<(String, BTreeMap<String, net::Asset>), String> {
    let release = net::latest_release(client, net::ReleaseHost::GitHub, REPO).await?;
    let assets = release
        .assets
        .into_iter()
        .map(|a| (a.name.clone(), a))
        .collect();
    Ok((release.tag, assets))
}

// ----------------------------------------------------------------------------
// Status
// ----------------------------------------------------------------------------

/// Build the status view from `state`, optionally with a known latest tag.
fn status_from(root: &Path, state: &InstalledState, latest: Option<String>) -> ToolsetStatus {
    let dir = (!state.tag.is_empty()).then(|| root.join(&state.tag));

    let tools = TOOLS
        .iter()
        .map(|t| {
            let installed = state.tools.get(t.name).and_then(|asset| {
                let p = dir.as_ref()?.join(asset);
                p.is_file().then_some(p)
            });
            let size = installed
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .map(|m| m.len());
            let companion_ready = match (&t.companion, &dir) {
                (Some(c), Some(d)) => d.join(c.dir).is_dir(),
                // No bundle to want is trivially "ready".
                (None, _) => true,
                (Some(_), None) => false,
            };
            ToolStatus {
                name: t.name.to_string(),
                label: t.label.to_string(),
                blurb: t.blurb.to_string(),
                windowed: t.windowed,
                driven_by_modkit: t.driven_by_modkit,
                available: asset_name(t).is_some(),
                path: installed.map(|p| p.to_string_lossy().to_string()),
                size,
                experimental: t.experimental,
                requires_game_dir: t.requires_game_dir,
                companion_dir: t.companion.as_ref().map(|c| c.dir.to_string()),
                companion_ready,
            }
        })
        .collect::<Vec<_>>();

    let installed_tag = (!state.tag.is_empty()).then(|| state.tag.clone());
    let anything_installed = tools.iter().any(|t| t.path.is_some());
    let update_available = match (&installed_tag, &latest) {
        (Some(cur), Some(new)) => anything_installed && cur != new,
        _ => false,
    };

    ToolsetStatus {
        installed_tag,
        latest_tag: latest,
        update_available,
        dir: dir
            .filter(|_| anything_installed)
            .unwrap_or_else(|| root.to_path_buf())
            .to_string_lossy()
            .to_string(),
        tools,
    }
}

/// Current toolset status. With `check_remote`, also asks GitHub for the latest
/// tag; a failed lookup degrades to `latest_tag: null` instead of erroring, so
/// the page still works offline.
#[tauri::command]
pub async fn toolset_status(check_remote: bool) -> Result<ToolsetStatus, String> {
    let root = toolset_root()?;
    let state = read_state(&root);

    let latest = if check_remote {
        match net::client() {
            Ok(c) => latest_toolset(&c).await.ok().map(|(tag, _)| tag),
            Err(_) => None,
        }
    } else {
        None
    };

    Ok(status_from(&root, &state, latest))
}

// ----------------------------------------------------------------------------
// Install / update
// ----------------------------------------------------------------------------

/// Download and unpack a tool's companion bundle into `version_dir`, beside the
/// binary that needs it.
///
/// The zip carries its own top-level directory, so extracting at the version dir
/// lands it exactly where the tool's exe-relative lookup expects. Already-present
/// bundles are left alone — a version dir only ever holds one release, so if the
/// directory is there it came from this same tag.
async fn install_companion(
    window: &Window,
    client: &reqwest::Client,
    assets: &BTreeMap<String, net::Asset>,
    version_dir: &Path,
    t: &Tool,
    companion: &Companion,
) -> Result<(), String> {
    let unpacked = version_dir.join(companion.dir);
    if unpacked.is_dir() {
        return Ok(());
    }

    let asset = assets.get(companion.asset).ok_or_else(|| {
        format!(
            "Release publishes no '{}' — {} needs it to find its reference data.",
            companion.asset, t.label
        )
    })?;

    let label = format!("{} data bundle", t.label);
    let bytes = net::download(
        client,
        &asset.url,
        net::DownloadOpts::new(&format!("toolset:{}", companion.asset), &label)
            .with_window(Some(window)),
    )
    .await?;

    // Unpacked straight from memory: staging the archive beside its own output
    // only to delete it again bought nothing, and the guarded extractor rejects
    // an entry that would escape `version_dir` — which the bare `z.extract` here
    // did not.
    net::archive::extract_bytes(bytes, version_dir)
        .map_err(|e| format!("Failed to unpack the {} data bundle: {e}", t.label))?;

    if !unpacked.is_dir() {
        return Err(format!(
            "The {} data bundle did not contain a '{}' directory — its layout may have changed.",
            t.label, companion.dir
        ));
    }
    Ok(())
}

/// Install `names` at the latest release, and bring every already-installed tool
/// up to that same release along with them.
///
/// The union is deliberate. The toolset is published as one release, so a
/// directory holding `wad_simulator` from v0.9.3 next to `model_forge` from
/// v0.10.0 is a configuration nobody tested and nobody asked for. Adding a tool
/// therefore also updates the rest, and the installed set is always internally
/// consistent at a single tag — which is what "keep only the most up-to-date
/// version" actually requires.
///
/// Passing an empty `names` updates what is already installed.
#[tauri::command]
pub async fn install_tools(window: Window, names: Vec<String>) -> Result<ToolsetStatus, String> {
    let root = toolset_root()?;
    let state = read_state(&root);

    let client = net::client()?;
    let (tag, assets) = latest_toolset(&client).await?;

    // Union of "asked for" and "already installed", minus anything this host has
    // no asset for. Ordered by the TOOLS table so progress reads sensibly.
    let wanted: Vec<&Tool> = TOOLS
        .iter()
        .filter(|t| names.iter().any(|n| n == t.name) || state.tools.contains_key(t.name))
        .filter(|t| asset_name(t).is_some())
        .collect();

    if wanted.is_empty() {
        return Err("None of the selected tools have a build for this machine.".into());
    }

    let dir = root.join(&tag);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create {tag} dir: {e}"))?;

    let total = wanted.len();
    let mut installed = BTreeMap::new();

    for (i, t) in wanted.iter().enumerate() {
        let asset = asset_name(t).expect("filtered above");
        // Downloaded by asset name, stored under the plain command name.
        let local = local_name(t);
        let dest = dir.join(&local);

        let _ = window.emit(
            "toolset-progress",
            ProgressEvent {
                tool: t.name.to_string(),
                label: t.label.to_string(),
                done: i,
                total,
            },
        );

        // Already present at this tag — a re-run after a partial failure, or a
        // second tool being added to a current install. Nothing to fetch.
        if dest.is_file() {
            installed.insert(t.name.to_string(), local);
            continue;
        }

        let remote = assets.get(&asset).ok_or_else(|| {
            format!("Release {tag} publishes no asset named '{asset}' — the toolset's release matrix may have changed.")
        })?;

        let bytes = net::download(
            &client,
            &remote.url,
            net::DownloadOpts::new(&format!("toolset:{}", t.name), t.label)
                .with_window(Some(&window)),
        )
        .await?;

        // Staged and renamed into place, so an interrupted download can never be
        // mistaken for an installed tool — the same guarantee this module always
        // had, now with the digest check and the size floor that come with it.
        place(
            &dest,
            &bytes,
            PlaceOpts::default()
                .executable()
                .expecting(remote.sha256())
                .at_least(MIN_TOOL_SIZE)
                // A version dir only ever holds one release, so there is never an
                // incumbent to displace and a `.bak` here would be noise.
                .keeping_no_bak(),
        )
        .map_err(|e| format!("Failed to install {}: {e}", t.label))?;
        installed.insert(t.name.to_string(), local);

        // The bundle unpacks BESIDE the binary, which is where the tool looks for
        // it. Fetched after the exe so a failure here cannot leave a recorded
        // install pointing at a half-made directory.
        if let Some(c) = &t.companion {
            install_companion(&window, &client, &assets, &dir, t, c).await?;
        }
    }

    let _ = window.emit(
        "toolset-progress",
        ProgressEvent {
            tool: String::new(),
            label: String::new(),
            done: total,
            total,
        },
    );

    let new_state = InstalledState {
        tag: tag.clone(),
        tools: installed,
    };
    write_state(&root, &new_state)?;
    prune(&root, &tag);

    Ok(status_from(&root, &new_state, Some(tag)))
}

/// Remove one installed tool. The engine-backed apps are the large ones, so
/// being able to drop them without wiping the whole toolset matters.
#[tauri::command]
pub fn uninstall_tool(name: String) -> Result<ToolsetStatus, String> {
    let root = toolset_root()?;
    let mut state = read_state(&root);

    if let Some(asset) = state.tools.remove(&name) {
        let path = root.join(&state.tag).join(asset);
        // A running binary cannot be deleted on Windows; say so rather than
        // dropping it from the state and leaking the file.
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| {
                format!("Could not remove {name}: {e}. Close it first if it is running.")
            })?;
        }
        write_state(&root, &state)?;
    }

    Ok(status_from(&root, &state, None))
}

// ----------------------------------------------------------------------------
// Open a terminal with the tools on PATH
// ----------------------------------------------------------------------------

/// Open the platform's default terminal in the user's home folder, with the
/// installed command-line tools on `PATH`.
///
/// This is what makes the CLI half of the toolset usable without teaching anyone
/// where modkit's cache lives. Because binaries are installed under their plain
/// names ([`local_name`]), putting one directory on `PATH` is all it takes —
/// `wad_simulator --help` then just works.
///
/// Per platform: PowerShell 7 (`pwsh`) falling back to Windows PowerShell, in a
/// new console; Terminal.app driven through AppleScript on macOS, which is the
/// only way to hand a *new* Terminal window an environment; and the first
/// available terminal emulator on Linux, run on the host when we are inside
/// Flatpak.
#[tauri::command]
pub fn open_tool_shell() -> Result<(), String> {
    let root = toolset_root()?;
    let state = read_state(&root);
    if state.tag.is_empty() || state.tools.is_empty() {
        return Err("No tools are installed yet — install one first.".into());
    }
    let bin_dir = root.join(&state.tag);
    if !bin_dir.is_dir() {
        return Err("The installed tools are missing — reinstall them.".into());
    }

    let home = home_dir()?;
    // Prepend, so the managed copies win over anything already on PATH (a
    // `cargo install`ed build, say) — the whole point is to run what modkit
    // installed and keeps current.
    let path = match std::env::var_os("PATH") {
        Some(existing) => {
            let mut dirs = vec![bin_dir.clone()];
            dirs.extend(std::env::split_paths(&existing));
            std::env::join_paths(dirs).map_err(|e| format!("Could not build PATH: {e}"))?
        }
        None => bin_dir.clone().into_os_string(),
    };

    open_shell_impl(&bin_dir, &home, &path)
}

/// The user's home folder.
fn home_dir() -> Result<PathBuf, String> {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME").map(PathBuf::from);
    home.filter(|p| p.is_dir())
        .ok_or_else(|| "Could not find your home folder.".to_string())
}

/// Single-quote for POSIX sh, so a home folder containing a space or an
/// apostrophe cannot break out of the command Terminal is handed.
#[cfg(target_os = "macos")]
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(windows)]
fn open_shell_impl(_bin_dir: &Path, home: &Path, path: &std::ffi::OsStr) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    // A GUI process has no console, so a plain spawn would run PowerShell
    // invisibly. CREATE_NEW_CONSOLE gives it a window of its own.
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

    // PowerShell 7 first; every Windows install has the 5.1 fallback.
    let mut last = String::new();
    for exe in ["pwsh.exe", "powershell.exe"] {
        match std::process::Command::new(exe)
            .args(["-NoLogo", "-NoExit"])
            .current_dir(home)
            .env("PATH", path)
            .creation_flags(CREATE_NEW_CONSOLE)
            .spawn()
        {
            Ok(_) => return Ok(()),
            Err(e) => last = format!("{exe}: {e}"),
        }
    }
    Err(format!("Could not open PowerShell ({last})."))
}

#[cfg(target_os = "macos")]
fn open_shell_impl(bin_dir: &Path, home: &Path, _path: &std::ffi::OsStr) -> Result<(), String> {
    // Terminal.app spawns its own login shell, so it never inherits an
    // environment we set on the process — the PATH has to be exported by the
    // command Terminal runs. `do script` opens a new window and types it in.
    let script = format!(
        "cd {}; export PATH={}:$PATH; clear",
        shell_quote(&home.to_string_lossy()),
        shell_quote(&bin_dir.to_string_lossy()),
    );
    let osa = format!(
        r#"tell application "Terminal"
            activate
            do script "{}"
        end tell"#,
        // The shell line is embedded in an AppleScript string literal, so its
        // backslashes and quotes need escaping a second time.
        script.replace('\\', r"\\").replace('"', "\\\"")
    );

    let status = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&osa)
        .status()
        .map_err(|e| format!("Could not run osascript: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("Terminal did not open.".into())
    }
}

#[cfg(target_os = "linux")]
fn open_shell_impl(bin_dir: &Path, home: &Path, path: &std::ffi::OsStr) -> Result<(), String> {
    use std::ffi::OsString;

    // `x-terminal-emulator` is the Debian/Ubuntu alternatives entry for whatever
    // the user actually chose; the rest cover the common desktops when it is
    // absent. First one that spawns wins.
    const TERMINALS: &[&str] = &[
        "x-terminal-emulator",
        "gnome-terminal",
        "konsole",
        "xfce4-terminal",
        "mate-terminal",
        "tilix",
        "kitty",
        "alacritty",
        "xterm",
    ];

    for term in TERMINALS {
        // Inside Flatpak the terminal lives on the host, not in the sandbox, so
        // it has to be started through the portal — and flatpak-spawn inherits
        // no environment, so PATH and the working directory are passed as flags.
        let spawned = if super::launch::in_flatpak() {
            let mut env = OsString::from("--env=PATH=");
            env.push(path);
            let mut dir = OsString::from("--directory=");
            dir.push(home);
            std::process::Command::new("flatpak-spawn")
                .arg("--host")
                .arg(env)
                .arg(dir)
                .arg("--")
                .arg(term)
                .spawn()
        } else {
            std::process::Command::new(term)
                .current_dir(home)
                .env("PATH", path)
                .spawn()
        };
        if spawned.is_ok() {
            return Ok(());
        }
    }

    Err(format!(
        "No terminal emulator found. Add this to your PATH by hand:\n{}",
        bin_dir.display()
    ))
}

// ----------------------------------------------------------------------------
// Launching the windowed apps
// ----------------------------------------------------------------------------

/// The tool processes modkit started, keyed by tool name.
///
/// Mirrors [`super::launch::GameProcess`], but a map rather than a single slot:
/// the Workshop and the game are independent programs and running both at once
/// is normal, where there is only ever one game.
#[derive(Default)]
pub struct ToolProcesses(Mutex<ToolProcState>);

#[derive(Default)]
struct ToolProcState {
    /// Live children, by tool name.
    children: BTreeMap<String, std::process::Child>,
    /// Crashes noticed while reaping, waiting to be reported to the UI once.
    failures: Vec<ToolFailure>,
}

/// A tool that exited badly, surfaced on the next poll.
#[derive(Clone, Debug, Serialize)]
pub struct ToolFailure {
    pub name: String,
    pub label: String,
    pub message: String,
}

/// Liveness snapshot for the UI.
#[derive(Debug, Serialize)]
pub struct ToolsRunning {
    /// Names of tools modkit started that are still alive.
    pub running: Vec<String>,
    /// Crashes since the last poll. Drained, so each is reported exactly once.
    pub failures: Vec<ToolFailure>,
}

/// Collect finished children. A non-zero exit becomes a reportable failure with
/// the tail of that tool's log — without this a crash just silently flips the
/// button back to "Open" with no reason given.
fn reap(state: &mut ToolProcState) {
    let mut done = Vec::new();
    for (name, child) in state.children.iter_mut() {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    let (label, log) = tool(name)
                        .map(|t| (t.label.to_string(), log_path_for(name)))
                        .unwrap_or_else(|| (name.clone(), None));
                    let tail = log.map(|p| log_tail(&p)).unwrap_or_default();
                    state.failures.push(ToolFailure {
                        name: name.clone(),
                        label,
                        message: format!("exited ({status}).\n{tail}"),
                    });
                }
                done.push(name.clone());
            }
            Ok(None) => {}
            // Cannot query it — stop tracking rather than pin a dead entry.
            Err(_) => done.push(name.clone()),
        }
    }
    for name in done {
        state.children.remove(&name);
    }
}

/// Does this build accept `flag`, by looking for it in the executable's strings?
///
/// A capability probe rather than a version comparison, because the toolset's
/// releases are not versioned per tool and the release that first carries
/// `--game-dir` does not exist yet — there is no number to compare against.
/// Probing self-heals: the moment a build ships with the flag, this returns true
/// and modkit starts passing it, with no table to update.
///
/// This is load-bearing, not belt-and-braces. Handing `--game-dir <path>` to a
/// build that does not know the flag is WORSE than passing nothing: that build
/// has no value-flag exclusion when it scans for its positional `.profile`
/// argument, so the path is taken as a save file and the game dies parsing a
/// 2.5 GB WAD as a save.
fn binary_accepts_flag(exe: &Path, flag: &str) -> bool {
    let Ok(bytes) = std::fs::read(exe) else {
        return false; // unreadable — assume the older, flagless contract
    };
    let needle = flag.as_bytes();
    bytes.windows(needle.len()).any(|w| w == needle)
}

/// Locate `vz.wad` under a game root — in `data/` or loose at the root.
///
/// Matched case-insensitively on purpose. The retail install is Windows-cased
/// (`Data\VZ.WAD` on some pressings), and macOS and Linux users routinely copy
/// it over to a case-sensitive filesystem where a literal `data/vz.wad` probe
/// misses a perfectly good install.
fn find_vz_wad(root: &Path) -> Option<PathBuf> {
    fn wad_in(dir: &Path) -> Option<PathBuf> {
        std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
            let p = e.path();
            let hit = p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.eq_ignore_ascii_case("vz.wad"));
            hit.then_some(p)
        })
    }

    if let Some(p) = wad_in(root) {
        return Some(p);
    }
    // Any child directory named "data", whatever its casing.
    std::fs::read_dir(root).ok()?.flatten().find_map(|e| {
        let p = e.path();
        let is_data = p.is_dir()
            && p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case("data"));
        is_data.then(|| wad_in(&p)).flatten()
    })
}

/// Where a tool's captured output lives, if it is installed.
fn log_path_for(name: &str) -> Option<PathBuf> {
    let exe = installed_tool_path(name)?;
    Some(exe.parent()?.join(format!("{name}.log")))
}

/// Which tools modkit started are still running, plus any crashes since the last
/// call. Polled by the Tools page the way `is_game_running` is polled by the
/// game bar.
#[tauri::command]
pub fn poll_tools(state: tauri::State<ToolProcesses>) -> ToolsRunning {
    let mut guard = match state.0.lock() {
        Ok(g) => g,
        // A poisoned lock must not take the page down; report "nothing running".
        Err(_) => {
            return ToolsRunning {
                running: Vec::new(),
                failures: Vec::new(),
            }
        }
    };
    reap(&mut guard);
    ToolsRunning {
        running: guard.children.keys().cloned().collect(),
        failures: std::mem::take(&mut guard.failures),
    }
}

/// Stop a tool modkit started. No-op if it is not running.
#[tauri::command]
pub fn stop_tool(state: tauri::State<ToolProcesses>, name: String) -> Result<(), String> {
    let mut guard = state.0.lock().map_err(|_| "Tool process lock poisoned")?;
    if let Some(mut child) = guard.children.remove(&name) {
        if let Ok(Some(_)) = child.try_wait() {
            return Ok(()); // already gone
        }
        let label = tool(&name).map(|t| t.label).unwrap_or(&name);
        child
            .kill()
            .map_err(|e| format!("Failed to stop {label}: {e}"))?;
        let _ = child.wait();
    }
    Ok(())
}

/// Launch one of the windowed programs (the Workshop, the native game).
///
/// `game_dir` is modkit's detected install. Both tools resolve their assets from
/// `--game-dir`, then `MERCS2_GAME_DIR`, then a Windows registry probe — so
/// handing them the path modkit already found saves the user from re-answering a
/// question modkit knows the answer to, and makes them work on macOS and Linux
/// where the registry fallback does not exist.
///
/// The child is tracked in [`ToolProcesses`] so the button can show what is
/// running and offer to stop it, and its output is captured to `<tool>.log`
/// beside the binary rather than thrown away. Discarding it made an instant
/// crash — a bad game path, a missing dylib — look exactly like a successful
/// launch: the button did something, nothing appeared, nothing to read. A crash
/// is now picked up by the next [`poll_tools`] and reported with the log tail.
///
/// Relaunching while a tool is already up is refused rather than silently
/// starting a second copy, the same way [`super::launch::launch_game`] does.
#[tauri::command]
pub fn launch_tool(
    state: tauri::State<ToolProcesses>,
    name: String,
    game_dir: Option<String>,
    saves_dir: Option<String>,
) -> Result<(), String> {
    let name = name.as_str();
    let t = tool(name).ok_or_else(|| format!("Unknown tool '{name}'"))?;
    if !t.windowed {
        return Err(format!(
            "{} is a command-line tool — open a terminal instead.",
            t.label
        ));
    }

    // Hold the lock across the liveness check and the spawn, so two fast clicks
    // cannot both get past the check and start two copies.
    let mut guard = state.0.lock().map_err(|_| "Tool process lock poisoned")?;
    reap(&mut guard);
    if guard.children.contains_key(name) {
        return Err(format!("{} is already running.", t.label));
    }

    let exe = installed_tool_path(name)
        .ok_or_else(|| format!("{} is not installed yet.", t.label))?;

    let game_dir = game_dir.filter(|g| !g.trim().is_empty());

    // `--game-dir` is the tool's documented way in, and the only one that works
    // off Windows: its fallback is the EA Games registry key, which on macOS and
    // Linux is a hard-coded `None`. Passed only when this build knows the flag —
    // see [`binary_accepts_flag`] for why handing it to an older build is worse
    // than passing nothing.
    let accepts_game_dir = binary_accepts_flag(&exe, "--game-dir");
    let accepts_saves_dir = binary_accepts_flag(&exe, "--saves-dir");

    if t.requires_game_dir {
        let root = game_dir.as_deref().ok_or_else(|| {
            format!(
                "{} needs your Mercenaries 2 folder. Set it on the Setup page — \
                 outside Windows there is no registry key for it to fall back on.",
                t.label
            )
        })?;
        if find_vz_wad(Path::new(root)).is_none() {
            return Err(format!(
                "No vz.wad under {root}. {} reads the retail content, so point \
                 modkit at the folder containing data/vz.wad.",
                t.label
            ));
        }
        // Nothing to hand the path to, and no registry to fall back on. Say that,
        // rather than launching into an error that blames the user's install.
        if !accepts_game_dir && !cfg!(windows) {
            return Err(format!(
                "The installed toolset release predates {}'s `--game-dir` option, \
                 and outside Windows it has no registry key to fall back on — so it \
                 cannot be pointed at your install. Update the toolset once a release \
                 carrying that option is published.",
                t.label
            ));
        }
    }

    // Its companion data is found relative to the exe, so a missing bundle is a
    // broken launch, not a degraded one — say so before the tool fails obscurely.
    if let Some(c) = &t.companion {
        let dir = exe.parent().map(|d| d.join(c.dir));
        if !dir.map(|d| d.is_dir()).unwrap_or(false) {
            return Err(format!(
                "{}'s {} data is missing — reinstall it first.",
                t.label, c.dir
            ));
        }
    }

    let dir = exe
        .parent()
        .ok_or_else(|| "Install directory has no parent".to_string())?;
    let log_path = dir.join(format!("{name}.log"));
    let log = std::fs::File::create(&log_path)
        .map_err(|e| format!("Could not create {}: {e}", log_path.display()))?;
    let log_err = log
        .try_clone()
        .map_err(|e| format!("Could not open the log for writing: {e}"))?;

    let mut cmd = std::process::Command::new(&exe);
    if accepts_game_dir {
        if let Some(g) = &game_dir {
            cmd.arg("--game-dir").arg(g);
        }
    }
    if accepts_saves_dir {
        if let Some(s) = saves_dir.as_deref().filter(|s| !s.trim().is_empty()) {
            cmd.arg("--saves-dir").arg(s);
        }
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(log)
        .stderr(log_err)
        // Run from the install dir so any relative lookup resolves next to the exe.
        .current_dir(dir);
    if let Some(g) = game_dir {
        cmd.env("MERCS2_GAME_DIR", g);
    }
    // Saves are resolved separately from the install: `$HOME/Documents/My Games/
    // Mercenaries 2/SaveGames`. The tool's own fallback finds that too, but
    // modkit lets the user point elsewhere (`set_saves_dir`), and only modkit
    // knows about that override — so pass whatever it resolved rather than
    // letting the tool re-guess and land on the default.
    if let Some(s) = saves_dir.filter(|s| !s.trim().is_empty()) {
        cmd.env("MERCS2_SAVES_DIR", s);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Detach: these are console-subsystem binaries, so without this they
        // would either inherit modkit's (absent) console or flash one up.
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }

    let child = cmd
        .spawn()
        .map_err(|e| format!("Could not start {}: {e}", t.label))?;

    guard.children.insert(name.to_string(), child);
    Ok(())
}

/// Last few lines of a tool's log, for an error message. Trimmed hard — this
/// goes in a UI banner, not a terminal.
fn log_tail(path: &Path) -> String {
    let text = match std::fs::read_to_string(path) {
        Ok(t) if !t.trim().is_empty() => t,
        _ => return "It wrote nothing to its log.".into(),
    };
    let tail: Vec<&str> = text.lines().rev().take(6).collect();
    tail.into_iter().rev().collect::<Vec<_>>().join("\n")
}

/// Absolute path to an installed tool, for modkit's own subprocess calls.
/// `None` when it is not installed — callers should prompt for an install rather
/// than silently falling back to a stale copy.
pub fn installed_tool_path(name: &str) -> Option<PathBuf> {
    let root = toolset_root().ok()?;
    let state = read_state(&root);
    let asset = state.tools.get(name)?;
    let path = root.join(&state.tag).join(asset);
    path.is_file().then_some(path)
}

/// Install a single tool on demand and return its path — the "just make it work"
/// entry point for modkit features that shell out.
pub async fn ensure_tool(window: Window, name: &str) -> Result<PathBuf, String> {
    if let Some(p) = installed_tool_path(name) {
        return Ok(p);
    }
    let label = tool(name).map(|t| t.label).unwrap_or(name);
    install_tools(window, vec![name.to_string()]).await?;
    installed_tool_path(name).ok_or_else(|| format!("{label} did not install"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A crashed tool must be forgotten AND reported — the whole point of
    /// tracking children is that a silent death used to look like a launch.
    /// Unix-only: needs a shell that exits on demand.
    #[cfg(unix)]
    #[test]
    fn reap_reports_a_crash_and_stops_tracking_it() {
        let mut st = ToolProcState::default();
        let child = std::process::Command::new("sh")
            .args(["-c", "exit 3"])
            .spawn()
            .unwrap();
        // A name outside TOOLS keeps this off the filesystem: no install to look
        // up, so no log to read.
        st.children.insert("fake_crasher".into(), child);
        std::thread::sleep(std::time::Duration::from_millis(300));

        reap(&mut st);

        assert!(st.children.is_empty(), "a dead child must not stay tracked");
        assert_eq!(st.failures.len(), 1);
        assert_eq!(st.failures[0].name, "fake_crasher");
        assert!(
            st.failures[0].message.contains("exited"),
            "got {:?}",
            st.failures[0].message
        );
    }

    /// A clean exit is someone closing the window — not an error to shout about.
    #[cfg(unix)]
    #[test]
    fn reap_is_quiet_about_a_clean_exit() {
        let mut st = ToolProcState::default();
        st.children.insert(
            "fake_quitter".into(),
            std::process::Command::new("sh")
                .args(["-c", "exit 0"])
                .spawn()
                .unwrap(),
        );
        std::thread::sleep(std::time::Duration::from_millis(300));

        reap(&mut st);

        assert!(st.children.is_empty());
        assert!(st.failures.is_empty(), "closing a window is not a failure");
    }

    /// A live tool stays tracked, or the button would flap back to "Open" while
    /// its window is still on screen.
    #[cfg(unix)]
    #[test]
    fn reap_leaves_a_running_process_alone() {
        let mut st = ToolProcState::default();
        st.children.insert(
            "fake_runner".into(),
            std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .unwrap(),
        );

        reap(&mut st);

        assert_eq!(st.children.len(), 1, "a live child must stay tracked");
        assert!(st.failures.is_empty());

        // Don't leak the process into the test runner's lifetime.
        if let Some(mut c) = st.children.remove("fake_runner") {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    #[test]
    fn tool_names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for t in TOOLS {
            assert!(seen.insert(t.name), "duplicate tool entry: {}", t.name);
        }
    }

    /// Every tool must resolve to an asset on a supported host, and the asset
    /// must be `<name><suffix>` — the naming the release workflow produces.
    #[test]
    fn asset_names_follow_the_release_naming() {
        let Some(suffix) = platform_suffix() else {
            return; // unsupported host; everything is correctly unavailable
        };
        for t in TOOLS {
            match asset_name(t) {
                Some(a) => {
                    assert_eq!(a, format!("{}{suffix}", t.name));
                    assert!(host_is_64_bit() || !t.sixty_four_bit_only);
                }
                // Only the engine-backed apps may be missing, and only on 32-bit.
                None => assert!(
                    t.sixty_four_bit_only && !host_is_64_bit(),
                    "{} has no asset on a supported host",
                    t.name
                ),
            }
        }
    }

    /// The engine-backed apps are the ones the release matrix builds 64-bit only.
    /// The Workshop is the one tool with reference data it must find beside its
    /// exe; installing the bare binary is a broken install.
    #[test]
    fn the_workshop_declares_its_data_bundle() {
        let c = tool("mercs2_workshop")
            .unwrap()
            .companion
            .as_ref()
            .expect("the Workshop needs workshop_data/ next to its exe");
        assert_eq!(c.asset, "mercs2-workshop-data.zip");
        assert_eq!(c.dir, "workshop_data");
        assert!(TOOLS
            .iter()
            .filter(|t| t.name != "mercs2_workshop")
            .all(|t| t.companion.is_none()));
    }

    /// A Workshop install whose bundle never unpacked must not read as complete —
    /// that is the state a failed extraction leaves, and re-installing repairs it.
    #[test]
    fn a_missing_data_bundle_is_reported_as_not_ready() {
        let dir = tempfile::tempdir().unwrap();
        let Some(_) = asset_name(tool("mercs2_workshop").unwrap()) else {
            return; // 32-bit host: the Workshop is correctly unavailable
        };
        std::fs::create_dir_all(dir.path().join("v0.9.3")).unwrap();
        let local = local_name(tool("mercs2_workshop").unwrap());
        std::fs::write(dir.path().join("v0.9.3").join(&local), b"exe").unwrap();

        let mut tools = BTreeMap::new();
        tools.insert("mercs2_workshop".into(), local);
        let state = InstalledState { tag: "v0.9.3".into(), tools };

        let s = status_from(dir.path(), &state, None);
        let ws = s.tools.iter().find(|t| t.name == "mercs2_workshop").unwrap();
        assert!(ws.path.is_some(), "the exe is installed");
        assert!(!ws.companion_ready, "but its data bundle is not");
        assert_eq!(ws.companion_dir.as_deref(), Some("workshop_data"));

        // Unpack it, and the same install now reads as complete.
        std::fs::create_dir_all(dir.path().join("v0.9.3").join("workshop_data")).unwrap();
        let s = status_from(dir.path(), &state, None);
        let ws = s.tools.iter().find(|t| t.name == "mercs2_workshop").unwrap();
        assert!(ws.companion_ready);

        // A tool with no bundle is never "not ready".
        let sim = s.tools.iter().find(|t| t.name == "wad_simulator").unwrap();
        assert!(sim.companion_ready);
        assert_eq!(sim.companion_dir, None);
    }

    #[test]
    fn engine_apps_are_marked_sixty_four_bit_only() {
        for name in ["mercs2_workshop", "mercs2_game"] {
            assert!(tool(name).expect(name).sixty_four_bit_only, "{name}");
        }
        for name in ["wad_simulator", "model_forge", "inject_parts"] {
            assert!(!tool(name).expect(name).sixty_four_bit_only, "{name}");
        }
    }

    /// Every desktop OS on both 64-bit arches must resolve to a suffix — modkit
    /// ships for all three, and a host with no suffix can install nothing at all.
    #[test]
    fn every_supported_desktop_host_has_a_suffix() {
        // (os, arch, expected suffix) — mirrors the toolset's release matrix.
        let expected = [
            ("windows", "x86_64", "-windows-x86_64.exe"),
            ("windows", "aarch64", "-windows-arm64.exe"),
            ("windows", "x86", "-windows-i686.exe"),
            ("macos", "aarch64", "-macos-arm64"),
            ("macos", "x86_64", "-macos-x86_64"),
            ("linux", "x86_64", "-linux-x86_64"),
            ("linux", "aarch64", "-linux-arm64"),
            ("linux", "x86", "-linux-i686"),
        ];
        // The running host must be one of them, and be mapped as documented.
        let here = expected
            .iter()
            .find(|(os, arch, _)| *os == std::env::consts::OS && *arch == std::env::consts::ARCH);
        if let Some((_, _, suffix)) = here {
            assert_eq!(platform_suffix().as_deref(), Some(*suffix));
        }
        // Suffixes are distinct — two hosts sharing one would fetch each other's
        // binaries.
        let mut seen = std::collections::HashSet::new();
        for (_, _, s) in expected {
            assert!(seen.insert(s), "duplicate suffix {s}");
        }
    }

    /// Both 64-bit arches count as 64-bit, so the engine-backed apps are
    /// offered on arm64 and x86_64 alike and withheld only on 32-bit x86.
    #[test]
    fn arm64_hosts_are_offered_the_engine_apps() {
        assert!(host_is_64_bit() == matches!(std::env::consts::ARCH, "x86_64" | "aarch64"));
        if std::env::consts::ARCH == "aarch64" {
            assert!(asset_name(tool("mercs2_workshop").unwrap()).is_some());
        }
    }

    /// On-disk names must be plain commands, or putting the install directory on
    /// PATH would give you `wad_simulator-linux-x86_64` instead of
    /// `wad_simulator`.
    #[test]
    fn installed_names_are_plain_commands() {
        for t in TOOLS {
            let local = local_name(t);
            assert_eq!(local, format!("{}{}", t.name, if cfg!(windows) { ".exe" } else { "" }));
            // The transport name is only ever used to find the download.
            if let Some(asset) = asset_name(t) {
                assert_ne!(local, asset, "{} must shed its platform suffix", t.name);
            }
        }
    }

    /// The flag probe decides whether modkit passes `--game-dir` at all, and a
    /// false positive is the damaging direction: an older build takes the value
    /// as its positional `.profile` and dies parsing a WAD as a save.
    #[test]
    fn flag_probe_finds_a_flag_only_when_the_binary_carries_it() {
        let dir = tempfile::tempdir().unwrap();

        let newer = dir.path().join("newer");
        std::fs::write(&newer, b"\x7fELF...--game-dir...--saves-dir...").unwrap();
        assert!(binary_accepts_flag(&newer, "--game-dir"));
        assert!(binary_accepts_flag(&newer, "--saves-dir"));

        // A build that only knows the dev flags must not look like it takes a
        // game dir just because "--game" appears in some other string.
        let older = dir.path().join("older");
        std::fs::write(&older, b"\x7fELF...--plan...--stream...--gamepad...").unwrap();
        assert!(!binary_accepts_flag(&older, "--game-dir"));

        // Unreadable path degrades to the older, flagless contract.
        assert!(!binary_accepts_flag(&dir.path().join("nope"), "--game-dir"));
    }

    /// The retail WAD is found in `data/` or at the root, whatever the casing —
    /// the install is Windows-cased and often lands on a case-sensitive volume.
    #[test]
    fn vz_wad_is_found_regardless_of_casing_or_depth() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(find_vz_wad(root).is_none(), "empty root has no wad");

        // Windows casing, in a subdirectory.
        std::fs::create_dir_all(root.join("Data")).unwrap();
        std::fs::write(root.join("Data").join("VZ.WAD"), b"x").unwrap();
        assert!(find_vz_wad(root).is_some(), "Data/VZ.WAD must match");

        // Loose at the root, lowercase.
        let flat = tempfile::tempdir().unwrap();
        std::fs::write(flat.path().join("vz.wad"), b"x").unwrap();
        assert!(find_vz_wad(flat.path()).is_some(), "root vz.wad must match");

        // A directory named vz.wad is not the file we want.
        let decoy = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(decoy.path().join("vz.wad")).unwrap();
        assert!(find_vz_wad(decoy.path()).is_none());
    }

    /// The native game is the one unfinished thing on the page. If anything else
    /// ever earns the label, that is a deliberate decision, not a stray edit.
    #[test]
    fn only_the_native_game_is_experimental() {
        let experimental: Vec<_> = TOOLS
            .iter()
            .filter(|t| t.experimental)
            .map(|t| t.name)
            .collect();
        assert_eq!(experimental, ["mercs2_game"]);
        // The label has to reach the UI, or it is decoration in a struct.
        let dir = tempfile::tempdir().unwrap();
        let s = status_from(dir.path(), &InstalledState::default(), None);
        let game = s.tools.iter().find(|t| t.name == "mercs2_game").unwrap();
        assert!(game.experimental);
        assert!(s.tools.iter().filter(|t| t.experimental).count() == 1);
    }

    /// Only the game hard-requires an install; the Workshop stays usable without
    /// one, and nothing that never launches should claim to need it.
    #[test]
    fn only_the_game_requires_a_game_dir() {
        let needs: Vec<_> = TOOLS
            .iter()
            .filter(|t| t.requires_game_dir)
            .map(|t| t.name)
            .collect();
        assert_eq!(needs, ["mercs2_game"]);
        assert!(TOOLS.iter().all(|t| !t.requires_game_dir || t.windowed));
    }

    /// Exactly two tools open a window; the other nine are CLIs. Grouping the
    /// page by "modkit drives it" instead put Smuggler, Byteswap, Load Probe and
    /// SecuROM Unwrap under "applications", which they are not.
    #[test]
    fn only_the_workshop_and_the_game_are_windowed() {
        let windowed: Vec<_> = TOOLS.iter().filter(|t| t.windowed).map(|t| t.name).collect();
        assert_eq!(windowed, ["mercs2_workshop", "mercs2_game"]);
    }

    /// Windowed and modkit-driven are independent axes: the CLI group holds both
    /// tools modkit runs itself and tools the user only ever runs by hand.
    #[test]
    fn windowed_and_driven_by_modkit_are_independent() {
        let clis = || TOOLS.iter().filter(|t| !t.windowed);
        assert!(clis().any(|t| t.driven_by_modkit), "e.g. wad_simulator");
        assert!(clis().any(|t| !t.driven_by_modkit), "e.g. loadprobe");
        // Nothing windowed is driven by modkit — you launch those yourself.
        assert!(TOOLS.iter().filter(|t| t.windowed).all(|t| !t.driven_by_modkit));
    }

    #[test]
    fn state_round_trips_and_missing_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Nothing written yet.
        assert_eq!(read_state(root).tag, "");

        let mut tools = BTreeMap::new();
        tools.insert("wad_simulator".into(), "wad_simulator-linux-x86_64".into());
        write_state(root, &InstalledState { tag: "v0.9.3".into(), tools }).unwrap();

        let back = read_state(root);
        assert_eq!(back.tag, "v0.9.3");
        assert_eq!(back.tools["wad_simulator"], "wad_simulator-linux-x86_64");

        // A corrupt sidecar degrades to "nothing installed" rather than failing
        // the whole page — the next install rewrites it.
        std::fs::write(root.join(STATE_FILE), "{ not json").unwrap();
        assert_eq!(read_state(root).tag, "");
    }

    #[test]
    fn prune_keeps_only_the_current_tag_and_drops_the_legacy_cache() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("toolset");
        std::fs::create_dir_all(root.join("v0.9.2")).unwrap();
        std::fs::create_dir_all(root.join("v0.9.3")).unwrap();
        // The single-tool cache this module replaces.
        let legacy = dir.path().join("bin");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("wad_simulator-linux-x86_64"), b"old").unwrap();

        prune(&root, "v0.9.3");

        assert!(root.join("v0.9.3").is_dir(), "current tag must survive");
        assert!(!root.join("v0.9.2").exists(), "stale tag must go");
        assert!(!legacy.exists(), "legacy bin/ cache must go");
    }

    #[test]
    fn nothing_installed_is_not_an_available_update() {
        let dir = tempfile::tempdir().unwrap();
        let state = InstalledState::default();
        let s = status_from(dir.path(), &state, Some("v0.9.3".into()));
        assert_eq!(s.installed_tag, None);
        // "Not set up" must not render as "out of date".
        assert!(!s.update_available);
        assert_eq!(s.tools.len(), TOOLS.len());
        // Still a folder to open — falls back to the toolset root.
        assert_eq!(s.dir, dir.path().to_string_lossy());
    }

    /// A state pointing at a file that is not there reports the tool as not
    /// installed — a half-deleted cache must not offer a path that fails on exec.
    #[test]
    fn a_missing_binary_reads_as_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        let mut tools = BTreeMap::new();
        tools.insert("wad_simulator".into(), "wad_simulator-linux-x86_64".into());
        let state = InstalledState { tag: "v0.9.3".into(), tools };

        let s = status_from(dir.path(), &state, Some("v0.9.4".into()));
        let ws = s.tools.iter().find(|t| t.name == "wad_simulator").unwrap();
        assert_eq!(ws.path, None);
        assert_eq!(ws.size, None);
        assert!(!s.update_available, "no binaries on disk => nothing to update");
    }

    #[test]
    fn an_installed_binary_is_reported_with_its_size_and_update_state() {
        let dir = tempfile::tempdir().unwrap();
        let local = local_name(tool("wad_simulator").unwrap());
        std::fs::create_dir_all(dir.path().join("v0.9.3")).unwrap();
        std::fs::write(dir.path().join("v0.9.3").join(&local), b"binary").unwrap();

        let mut tools = BTreeMap::new();
        tools.insert("wad_simulator".into(), local);
        let state = InstalledState { tag: "v0.9.3".into(), tools };

        let current = status_from(dir.path(), &state, Some("v0.9.3".into()));
        let ws = current.tools.iter().find(|t| t.name == "wad_simulator").unwrap();
        assert!(ws.path.is_some());
        assert_eq!(ws.size, Some(6));
        assert!(!current.update_available, "same tag is up to date");
        // Something is installed, so it points at the version dir, not the root.
        assert_eq!(current.dir, dir.path().join("v0.9.3").to_string_lossy());

        let stale = status_from(dir.path(), &state, Some("v0.10.0".into()));
        assert!(stale.update_available, "a newer tag is an available update");

        // Offline: no latest tag known, so nothing is claimed about staleness.
        let offline = status_from(dir.path(), &state, None);
        assert!(!offline.update_available);
        assert_eq!(offline.installed_tag.as_deref(), Some("v0.9.3"));
    }
}
