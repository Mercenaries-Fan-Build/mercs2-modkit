//! Which exe a launch runs, and whether the install is set up for it.
//!
//! Detection, the launcher, the pmc_bb and dxwrapper installs, and the game bar
//! all read [`resolve`]; none of them picks an exe on its own.
//!
//! `overrides.exe` (runtime.json) pins the exe. Unpinned, identified exes rank by
//! [`AUTO_ORDER`], unrecognised exes last, ties by file name.
//!
//! ```text
//!   exe                       loads pmc_bb via   auto pmc_bb build   dxwrapper
//!   cracked, imports pmc_bb   import table       fully_loaded        optional
//!   cracked by cruise.dll     dxwrapper          log_only            required
//!   DRM-free                  dxwrapper          log_only            required
//!   stock SecuROM             dxwrapper          log_only            required
//! ```
//!
//! Exactly one loader scans `scripts/`: dxwrapper's `LoadPlugins` is the opposite
//! of the installed pmc_bb build's `asi`.
//!
//! [`resolve`] reports mismatches as `problems` and changes nothing; a launch is
//! never refused over them. [`prepare_launch`] fixes them only where the choice is
//! modkit's: nothing while `overrides.exe` is set or the exe is unrecognised, and
//! no pmc_bb swap while `overrides.pmcBb` is set. It only swaps what modkit
//! installed; it never installs a missing component or replaces a file it didn't
//! write.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::Window;

use super::dxwrapper::{self, IniFacts};
use super::exe_identity::{self, ExeIdentity, HashCache};
use super::managed::pmc_bb::{self, Choice, ExeKind};
use super::managed::{Ledger, PlaceOpts};
use super::runtime_settings::{self, Overrides};

/// Automatic preference, best first.
pub const AUTO_ORDER: &[&str] = &[
    "v11-patched-drmfree",
    "v11-cracked-pmcbb",
    "v11-cracked-cruise",
    "v11-patched-securom",
    "v10-ea-signed",
    "v10-unsigned",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Selection {
    Auto,
    Override,
}

#[derive(Debug, Clone, Serialize)]
pub struct LaunchPlan {
    pub exe: ExeIdentity,
    pub selection: Selection,
    /// Whether dxwrapper must load pmc_bb for this exe; `None` when unrecognised.
    pub needs_dxwrapper: Option<bool>,
    /// The build modkit picks for this exe.
    pub pmc_bb_auto: Choice,
    /// The pinned build, else `pmc_bb_auto`.
    pub pmc_bb_wanted: Choice,
    /// The installed build per the ledger; `None` when absent or changed since.
    pub pmc_bb_installed: Option<String>,
    pub problems: Vec<String>,
}

/// What the game folder holds, as far as the plan cares.
pub struct Installed {
    pub pmc_bb_present: bool,
    pub pmc_bb_asset: Option<String>,
    pub dxwrapper_present: bool,
    pub stub_present: bool,
    pub ini: Option<IniFacts>,
}

impl Installed {
    pub fn read(root: &Path, ledger: &Ledger) -> Result<Self, String> {
        let dll = root.join(pmc_bb::INSTALL_NAME);
        let pmc_bb_asset = ledger
            .get(pmc_bb::LEDGER_KEY)
            .filter(|c| c.files.iter().any(|f| Path::new(&f.abs_path) == dll) && c.is_intact())
            .map(|c| c.asset);
        Ok(Self {
            pmc_bb_present: dll.is_file(),
            pmc_bb_asset,
            dxwrapper_present: root.join(dxwrapper::DLL).is_file(),
            stub_present: root.join(dxwrapper::PROXY_DLL).is_file(),
            ini: dxwrapper::read_ini_facts(root)?,
        })
    }
}

fn rank(e: &ExeIdentity) -> usize {
    e.id
        .as_deref()
        .and_then(|id| AUTO_ORDER.iter().position(|o| *o == id))
        .unwrap_or(AUTO_ORDER.len())
}

/// Pick the exe and list how the install disagrees with it.
pub fn resolve(
    root: &Path,
    exes: &[ExeIdentity],
    overrides: &Overrides,
    installed: &Installed,
) -> Result<LaunchPlan, String> {
    let (exe, selection) = match &overrides.exe {
        Some(pinned) => (
            exes.iter()
                .find(|e| Path::new(&e.path) == Path::new(pinned))
                .ok_or_else(|| {
                    format!(
                        "The pinned exe {pinned} is not there any more. Pick another in \
                         Game Info, or set it back to automatic."
                    )
                })?,
            Selection::Override,
        ),
        None => (
            exes.iter()
                .min_by_key(|e| rank(e))
                .ok_or_else(|| format!("No Mercenaries2*.exe in {}", root.display()))?,
            Selection::Auto,
        ),
    };
    let pmc_bb_auto = pmc_bb::resolve(exe.kind, None)?;
    let pmc_bb_wanted = pmc_bb::resolve(exe.kind, overrides.pmc_bb.as_deref())?;
    let needs_dxwrapper = exe.kind.imports_pmc_bb().map(|i| !i);
    let problems = problems(root, exe, needs_dxwrapper, &pmc_bb_wanted, installed);
    Ok(LaunchPlan {
        exe: exe.clone(),
        selection,
        needs_dxwrapper,
        pmc_bb_auto,
        pmc_bb_wanted,
        pmc_bb_installed: installed.pmc_bb_asset.clone(),
        problems,
    })
}

fn problems(
    root: &Path,
    exe: &ExeIdentity,
    needs_dxwrapper: Option<bool>,
    wanted: &Choice,
    installed: &Installed,
) -> Vec<String> {
    let mut out = Vec::new();
    let pmc = pmc_bb::INSTALL_NAME;

    if let Some(dll) = exe.requires.as_deref().filter(|d| !d.eq_ignore_ascii_case(pmc)) {
        if !root.join(dll).is_file() {
            out.push(format!(
                "{} imports {dll}, which isn't in the game folder; it won't start without it.",
                exe.name
            ));
        }
    }

    if !installed.pmc_bb_present {
        out.push(format!("{pmc} is not installed."));
    } else {
        match &installed.pmc_bb_asset {
            None => out.push(format!(
                "{pmc} was not installed by modkit, or has changed since, so its build is unknown."
            )),
            Some(a) if !a.eq_ignore_ascii_case(&wanted.asset) => {
                if wanted.overridden {
                    out.push(format!("{pmc} is {a}, but you chose {}.", wanted.asset));
                } else if exe.recognised() {
                    out.push(format!("{pmc} is {a}, but {} calls for {}.", exe.name, wanted.asset));
                }
            }
            Some(_) => {}
        }
    }

    if needs_dxwrapper == Some(true) {
        match &installed.ini {
            Some(ini) if installed.dxwrapper_present && installed.stub_present => {
                let loads_pmc = ini.custom_dll.as_deref().is_some_and(|d| d.eq_ignore_ascii_case(pmc));
                if !loads_pmc {
                    out.push(format!(
                        "{} doesn't load {pmc}: LoadCustomDllPath is {}.",
                        dxwrapper::INI,
                        ini.custom_dll.as_deref().unwrap_or("unset")
                    ));
                }
            }
            _ => out.push(format!(
                "{} doesn't load {pmc} itself, so dxwrapper has to, and dxwrapper is not fully \
                 installed ({}, {}, {}).",
                exe.name,
                dxwrapper::PROXY_DLL,
                dxwrapper::DLL,
                dxwrapper::INI
            )),
        }
    }

    let asset = installed.pmc_bb_asset.as_deref();
    if let Some((a, v)) = asset.and_then(|a| pmc_bb::Variant::by_asset(a).map(|v| (a, v))) {
        let dx_scans = installed.dxwrapper_present
            && installed.stub_present
            && installed.ini.as_ref().and_then(|i| i.load_plugins) == Some(true);
        match (v.features.asi, dx_scans) {
            (true, true) => out.push(format!(
                "{a} and dxwrapper both scan for plugins, so every ASI mod would load twice."
            )),
            (false, false) => out.push(format!(
                "Nothing scans for plugins: {a} has no ASI loader, and dxwrapper's LoadPlugins \
                 is off or dxwrapper is not installed."
            )),
            _ => {}
        }
    }
    out
}

/// Every exe in the folder, and the plan (or why there is none).
pub struct Detected {
    pub exes: Vec<ExeIdentity>,
    pub plan: Result<LaunchPlan, String>,
}

/// Identify the folder's exes and resolve the plan. Blocking: hashes exes.
pub fn detect(window: &Window, root: &Path) -> Result<Detected, String> {
    let catalog = super::verify::exe_catalog(window)?;
    let mut cache = HashCache::app()?;
    let mut exes = exe_identity::scan(root, catalog, &mut cache)?;
    let plan = runtime_settings::read().and_then(|settings| {
        if let Some(pinned) = settings.overrides.exe.as_deref().map(Path::new) {
            if pinned.is_file() && !exes.iter().any(|e| Path::new(&e.path) == pinned) {
                exes.push(exe_identity::identify(pinned, catalog, &mut cache)?);
            }
        }
        let installed = Installed::read(root, &Ledger::app()?)?;
        resolve(root, &exes, &settings.overrides, &installed)
    });
    cache.save()?;
    Ok(Detected { exes, plan })
}

/// The plan for `root`. Blocking.
pub fn for_root(window: &Window, root: &Path) -> Result<LaunchPlan, String> {
    detect(window, root)?.plan
}

async fn for_root_async(window: &Window, root: &Path) -> Result<LaunchPlan, String> {
    let (window, root) = (window.clone(), root.to_path_buf());
    tauri::async_runtime::spawn_blocking(move || for_root(&window, &root))
        .await
        .map_err(|e| e.to_string())?
}

/// Bring what modkit installed in line with the exe it would launch, then return
/// the plan. Runs before every launch.
#[tauri::command]
pub async fn prepare_launch(window: Window, game_root: String) -> Result<LaunchPlan, String> {
    let root = PathBuf::from(&game_root);
    let plan = for_root_async(&window, &root).await?;
    if plan.selection == Selection::Override || plan.exe.kind == ExeKind::Unknown {
        return Ok(plan);
    }

    let wanted = &plan.pmc_bb_wanted;
    let swap = !wanted.overridden
        && plan
            .pmc_bb_installed
            .as_deref()
            .is_some_and(|a| !a.eq_ignore_ascii_case(&wanted.asset));
    if swap {
        // Also resyncs dxwrapper.ini.
        super::setup::install_pmc_bb_build(&window, &root, wanted).await?;
    } else if let Some(v) = plan.pmc_bb_installed.as_deref().and_then(pmc_bb::Variant::by_asset) {
        dxwrapper::sync_load_plugins(&root, &Ledger::app()?, !v.features.asi, PlaceOpts::default())?;
    }
    for_root_async(&window, &root).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::verify::EXE_IDS;

    fn exe(dir: &Path, name: &str, id: Option<&str>) -> ExeIdentity {
        ExeIdentity {
            path: dir.join(name).to_string_lossy().to_string(),
            name: name.into(),
            size: 1,
            sha256: "00".into(),
            id: id.map(String::from),
            version: None,
            variant: None,
            label: None,
            requires: None,
            kind: ExeKind::from_exe_id(id),
        }
    }

    fn installed(asset: Option<&str>, dx: Option<bool>) -> Installed {
        Installed {
            pmc_bb_present: asset.is_some(),
            pmc_bb_asset: asset.map(String::from),
            dxwrapper_present: dx.is_some(),
            stub_present: dx.is_some(),
            ini: dx.map(|loads| IniFacts {
                load_plugins: Some(loads),
                custom_dll: Some("pmc_bb.dll".into()),
            }),
        }
    }

    #[test]
    fn auto_order_covers_every_catalogue_id() {
        let mut a: Vec<_> = AUTO_ORDER.to_vec();
        let mut b: Vec<_> = EXE_IDS.to_vec();
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b);
    }

    #[test]
    fn auto_prefers_by_order_and_unrecognised_last() {
        let d = Path::new("/g");
        let exes = [
            exe(d, "Mercenaries2.exe", Some("v11-patched-securom")),
            exe(d, "Mercenaries2.cracked.exe", Some("v11-cracked-pmcbb")),
            exe(d, "Mercenaries2.mine.exe", None),
        ];
        let p = resolve(d, &exes, &Overrides::default(), &installed(None, None)).unwrap();
        assert_eq!(p.exe.name, "Mercenaries2.cracked.exe");
        assert_eq!(p.selection, Selection::Auto);
    }

    /// The reported bug: dxwrapper present no longer decides the exe.
    #[test]
    fn dxwrapper_does_not_pick_the_exe() {
        let d = Path::new("/g");
        let exes = [
            exe(d, "Mercenaries2.cracked.exe", Some("v11-cracked-pmcbb")),
            exe(d, "Mercenaries2.exe", Some("v11-patched-securom")),
        ];
        let inst = installed(Some("pmc_bb_fully_loaded.dll"), Some(false));
        let p = resolve(d, &exes, &Overrides::default(), &inst).unwrap();
        assert_eq!(p.exe.name, "Mercenaries2.cracked.exe");
        assert!(p.problems.is_empty(), "{:?}", p.problems);
    }

    #[test]
    fn a_pin_wins_and_a_missing_pin_is_an_error() {
        let d = Path::new("/g");
        let exes = [
            exe(d, "Mercenaries2.cracked.exe", Some("v11-cracked-pmcbb")),
            exe(d, "Mercenaries2.exe", Some("v11-patched-securom")),
        ];
        let pin = |p: &str| Overrides { exe: Some(p.into()), pmc_bb: None };
        let p = resolve(d, &exes, &pin(&exes[1].path), &installed(None, None)).unwrap();
        assert_eq!(p.exe.name, "Mercenaries2.exe");
        assert_eq!(p.selection, Selection::Override);
        assert!(resolve(d, &exes, &pin("/g/gone.exe"), &installed(None, None)).is_err());
    }

    #[test]
    fn a_wrong_build_and_a_missing_dxwrapper_are_reported() {
        let d = Path::new("/g");
        let exes = [exe(d, "Mercenaries2.exe", Some("v11-patched-securom"))];
        let p = resolve(d, &exes, &Overrides::default(), &installed(Some("pmc_bb_fully_loaded.dll"), None))
            .unwrap();
        assert_eq!(p.needs_dxwrapper, Some(true));
        assert!(p.problems.iter().any(|m| m.contains("calls for pmc_bb_log_only.dll")), "{:?}", p.problems);
        assert!(p.problems.iter().any(|m| m.contains("dxwrapper is not fully installed")), "{:?}", p.problems);
    }

    #[test]
    fn a_pinned_build_is_not_reported_against_the_exe() {
        let d = Path::new("/g");
        let exes = [exe(d, "Mercenaries2.exe", Some("v11-patched-securom"))];
        let ov = Overrides { exe: None, pmc_bb: Some("pmc_bb_asi_log.dll".into()) };
        let p = resolve(d, &exes, &ov, &installed(Some("pmc_bb_asi_log.dll"), Some(false))).unwrap();
        assert!(p.problems.is_empty(), "{:?}", p.problems);
    }

    #[test]
    fn two_loaders_and_no_loader_are_reported() {
        let d = Path::new("/g");
        let exes = [exe(d, "Mercenaries2.exe", Some("v11-cracked-pmcbb"))];
        let ov = Overrides::default();
        let twice = resolve(d, &exes, &ov, &installed(Some("pmc_bb_fully_loaded.dll"), Some(true))).unwrap();
        assert!(twice.problems.iter().any(|m| m.contains("both scan")), "{:?}", twice.problems);
        let none = resolve(d, &exes, &ov, &installed(Some("pmc_bb_log_only.dll"), None)).unwrap();
        assert!(none.problems.iter().any(|m| m.contains("Nothing scans")), "{:?}", none.problems);
    }

    #[test]
    fn an_unrecognised_exe_has_no_derived_needs() {
        let d = Path::new("/g");
        let exes = [exe(d, "Mercenaries2.exe", None)];
        let p = resolve(d, &exes, &Overrides::default(), &installed(Some("pmc_bb_fully_loaded.dll"), None))
            .unwrap();
        assert_eq!(p.needs_dxwrapper, None);
        assert!(!p.problems.iter().any(|m| m.contains("calls for")), "{:?}", p.problems);
    }
}
