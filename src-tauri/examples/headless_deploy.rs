//! Headless install / uninstall of Quartermaster Shipments through Modkit's own build and deploy
//! code: `wad_builder::assemble` (qm preflight, build, link, collapse, stage) then
//! `deploy_wad::deploy_patch_wad` (WAD swap with backup, loose files, data files, ledgers).
//!
//! ```text
//! cargo run --example headless_deploy -- <subcommand> (--scratch-home H | --real) [flags...]
//!
//! snapshot    --game-root R --out S.json
//! install     --game-root R --qm Q --rows ROWS.json --shipment DIR [--asi-target scripts]
//! uninstall   --game-root R --qm Q --rows ROWS.json [--asi-target scripts]
//! verify      --game-root R --snapshot S.json
//! restore-wad --game-root R --file vz-patch.<hash16>.wad
//! ledger
//! blocks      --wad FINAL.wad --overlay OVERLAY.wad
//! ```
//!
//! Every subcommand except `blocks` names where it writes, with exactly one of:
//!
//! * `--scratch-home H`: `HOME` is set to `H` before anything resolves Modkit's app-data folder
//!   (ledgers, WAD backups, build output, qm work dirs, trash), and the run refuses unless that
//!   folder, the canonical `--game-root`, and every absolute path the ledgers in `H` record all lie
//!   inside `H`. Modkit's writes stay inside `H`; outside it this tool writes only the `--out` file
//!   it is given.
//! * `--real`: the app-data folder of the caller's own `HOME` and the given game root. The run
//!   prints what it will touch, then refuses while the game, a Wine process of Modkit's prefix, or
//!   the `mercs2-modkit` app runs: the game reads the files and the app writes the same ledgers.
//!
//! `blocks` only reads the two WADs it is given.
//!
//! `ROWS.json` is the load order to keep (a JSON array of Modkit `ShipmentRef` rows). `install`
//! builds those rows plus the Shipment at `DIR` (appended last, described by `inspect_shipment`),
//! exactly the set the UI builds after a Shipment is added. `uninstall` builds the rows alone, the
//! set the UI builds after that Shipment is removed; with no rows it removes the patch the way the
//! UI's "remove patch" does. Every step fails hard and exits nonzero.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mercs2_formats::patch_wad::read_patch_wad;
use mercs2_modkit_lib::commands::deploy_wad::{
    deploy_patch_wad, deployed_wad_record, list_patch_wad_backups, placed_files, restore_patch_wad,
    DeployWadArgs, RestoreWadArgs,
};
use mercs2_modkit_lib::commands::paths::{app_data_dir, deployed_dir};
use mercs2_modkit_lib::commands::shipment::{inspect_shipment, ShipmentRef};
use mercs2_modkit_lib::commands::wad_builder::{assemble, BuildOptions};
use serde_json::{json, Value};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("FAILED: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let (cmd, rest) = args.split_first().ok_or("no subcommand given")?;
    let (flags, real) = parse_flags(rest)?;
    if cmd == "blocks" {
        if real || flags.contains_key("scratch-home") {
            return Err("blocks reads two WADs only; it takes neither --scratch-home nor --real".into());
        }
        return check_blocks(Path::new(need(&flags, "wad")?), Path::new(need(&flags, "overlay")?));
    }
    let mode = match (flags.get("scratch-home"), real) {
        (Some(_), true) => return Err("give --scratch-home or --real, not both".into()),
        (None, false) => {
            return Err("give --scratch-home H to work in a scratch HOME, or --real to work on the \
                        caller's own Modkit app data"
                .into())
        }
        (Some(h), false) => Mode::Scratch(enter_scratch_home(h)?),
        (None, true) => Mode::Real,
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("starting the async runtime: {e}"))?;
    println!("app data: {}", app_data_dir()?.display());
    let root = flags.get("game-root").map(|r| game_root(r, &mode)).transpose()?;
    if let Mode::Real = mode {
        announce_real(cmd, root.as_deref())?;
        refuse_while_game_or_modkit_runs()?;
    }
    match cmd.as_str() {
        "snapshot" => {
            let root = root.as_deref().ok_or("--game-root is required")?;
            let snap = snapshot(root)?;
            let out = need(&flags, "out")?;
            std::fs::write(out, serde_json::to_string_pretty(&snap).map_err(|e| e.to_string())?)
                .map_err(|e| format!("writing {out}: {e}"))?;
            println!("snapshot: {} file(s) under the game root, {} app-data file(s) -> {out}",
                snap["game"].as_object().map(|m| m.len()).unwrap_or(0),
                snap["app_data"].as_object().map(|m| m.len()).unwrap_or(0));
            Ok(())
        }
        "install" => {
            let root = root.as_deref().ok_or("--game-root is required")?;
            let mut rows = read_rows(need(&flags, "rows")?)?;
            let ship = inspect_shipment(need(&flags, "shipment")?.to_string())?;
            println!("shipment row: {}", serde_json::to_string(&ship).map_err(|e| e.to_string())?);
            if rows.iter().any(|r| r.id == ship.id) {
                return Err(format!("{} is already in the rows", ship.id));
            }
            rows.push(ship);
            rt.block_on(build_and_deploy(root, rows, &flags))
        }
        "uninstall" => {
            let root = root.as_deref().ok_or("--game-root is required")?;
            let rows = read_rows(need(&flags, "rows")?)?;
            if rows.is_empty() {
                let res = restore_patch_wad(RestoreWadArgs {
                    file: None,
                    data_dir: root.join("data").to_string_lossy().to_string(),
                    game_root: root.to_string_lossy().to_string(),
                })?;
                println!("removed the patch: {}", pretty(&res));
                print_ledger()
            } else {
                rt.block_on(build_and_deploy(root, rows, &flags))
            }
        }
        "verify" => {
            let root = root.as_deref().ok_or("--game-root is required")?;
            let path = need(&flags, "snapshot")?;
            let before: Value = serde_json::from_slice(
                &std::fs::read(path).map_err(|e| format!("reading {path}: {e}"))?,
            )
            .map_err(|e| format!("{path}: {e}"))?;
            let now = snapshot(root)?;
            let game = diff(&before["game"], &now["game"]);
            let app = diff(&before["app_data"], &now["app_data"]);
            for line in &app {
                println!("app-data differs: {line}");
            }
            if game.is_empty() {
                println!("verify: the game root matches the snapshot byte for byte");
                Ok(())
            } else {
                for line in &game {
                    println!("game differs: {line}");
                }
                Err(format!("{} game-root difference(s) from the snapshot", game.len()))
            }
        }
        "restore-wad" => {
            let root = root.as_deref().ok_or("--game-root is required")?;
            let res = restore_patch_wad(RestoreWadArgs {
                file: Some(need(&flags, "file")?.to_string()),
                data_dir: root.join("data").to_string_lossy().to_string(),
                game_root: root.to_string_lossy().to_string(),
            })?;
            println!("restored: {}", pretty(&res));
            print_ledger()
        }
        "ledger" => print_ledger(),
        other => Err(format!("unknown subcommand {other}")),
    }
}

async fn build_and_deploy(
    root: &Path,
    rows: Vec<ShipmentRef>,
    flags: &BTreeMap<String, String>,
) -> Result<(), String> {
    let qm = PathBuf::from(need(flags, "qm")?);
    if !qm.is_file() {
        return Err(format!("no qm at {}", qm.display()));
    }
    let asi_target = flags.get("asi-target").cloned().unwrap_or_else(|| "scripts".into());
    println!("load order:");
    for r in &rows {
        println!("  {} ({}) {}", r.id, r.name, r.path);
    }
    let options: BuildOptions = serde_json::from_value(json!({
        "mods": [],
        "output_dir": null,
        "game_path": root.to_string_lossy(),
        "wardrobe": [],
        "prebuilt": [],
        "textures": [],
        "shipments": rows,
    }))
    .map_err(|e| format!("describing the build: {e}"))?;
    let build = assemble(options, Some(&qm)).await?;
    println!(
        "built: path={} sha256={} blocks={} bytes={} staging={}",
        build.path, build.sha256, build.block_count, build.byte_size, build.staging_dir
    );
    for w in &build.warnings {
        println!("build warning: {w}");
    }
    println!("build outcomes: {}", pretty(&build.outcomes));
    println!("staged files: {}", pretty(&build.placed_files));
    println!("staged data files: {}", pretty(&build.data_files));
    println!("stream copies: {}", pretty(&build.stream_copies));

    let res = deploy_patch_wad(DeployWadArgs {
        wad_path: build.path.clone(),
        data_dir: root.join("data").to_string_lossy().to_string(),
        staging_dir: Some(build.staging_dir.clone()),
        game_root: Some(root.to_string_lossy().to_string()),
        asi_target: Some(asi_target),
    })?;
    println!("deployed: {}", pretty(&res));
    let on_disk = sha256_file(&root.join("data").join("vz-patch.wad"))?;
    println!("vz-patch.wad on disk sha256 {on_disk}");
    if !build.sha256.is_empty() && on_disk != build.sha256 {
        return Err(format!("vz-patch.wad on disk is {on_disk}, the build was {}", build.sha256));
    }
    print_ledger()
}

/// Every overlay block (outside the linker-owned script blocks, which link re-emits) must appear in
/// the final WAD with the same path, compressed bytes and asset hashes.
fn check_blocks(wad: &Path, overlay: &Path) -> Result<(), String> {
    let read = |p: &Path| -> Result<_, String> {
        read_patch_wad(&std::fs::read(p).map_err(|e| format!("reading {}: {e}", p.display()))?)
            .map_err(|e| format!("{}: {e}", p.display()))
    };
    let final_wad = read(wad)?;
    let over = read(overlay)?;
    println!("final WAD: {} block(s); overlay: {} block(s)", final_wad.blocks.len(), over.blocks.len());
    let mut missing = 0usize;
    for b in &over.blocks {
        let hashes: Vec<u32> = b.aset_entries.iter().map(|a| a.asset_hash).collect();
        let found = final_wad.blocks.iter().find(|f| f.path_string == b.path_string);
        match found {
            Some(f) => {
                let fh: Vec<u32> = f.aset_entries.iter().map(|a| a.asset_hash).collect();
                let same_bytes = f.compressed_data == b.compressed_data;
                let same_rows = fh == hashes;
                println!(
                    "block {}: in final WAD, bytes {}, {} ASET row(s) {} (first {:#010X}), sha256 {}",
                    b.path_string,
                    if same_bytes { "identical" } else { "DIFFERENT" },
                    hashes.len(),
                    if same_rows { "identical" } else { "DIFFERENT" },
                    hashes.first().copied().unwrap_or(0),
                    loadprobe::sha256::sha256_hex(&f.compressed_data)
                );
                if !same_bytes || !same_rows {
                    missing += 1;
                }
            }
            None => {
                println!("block {}: NOT in the final WAD", b.path_string);
                missing += 1;
            }
        }
    }
    if missing == 0 {
        Ok(())
    } else {
        Err(format!("{missing} overlay block(s) missing or different in the final WAD"))
    }
}

/// Refuse to touch the caller's own Modkit app data or a game folder while the game, any Wine
/// process of Modkit's prefix, or the `mercs2-modkit` app runs.
///
/// Matches every process whose command line names a `Mercenaries2*.exe`; every Wine process
/// (`.../bin/wine*`, `*-preloader`) whose binary lives under Modkit's `runtimes/` or whose
/// environment sets `WINEPREFIX` to Modkit's `wine-prefix`; and every process whose executable is
/// named `mercs2-modkit`, the app, which writes the same ledgers this tool does.
fn refuse_while_game_or_modkit_runs() -> Result<(), String> {
    let app = app_data_dir()?;
    let prefix = app.join("wine-prefix").to_string_lossy().to_string();
    let runtimes = app.join("runtimes").to_string_lossy().to_string();
    let ps = |args: &[&str]| -> Result<String, String> {
        let out = std::process::Command::new("/bin/ps")
            .args(args)
            .output()
            .map_err(|e| format!("running ps: {e}"))?;
        if !out.status.success() {
            return Err(format!("ps {args:?} exited {}", out.status));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let mut hits = Vec::new();
    for line in ps(&["-axww", "-o", "pid=,command="])?.lines() {
        let line = line.trim();
        let Some((pid, command)) = line.split_once(' ') else { continue };
        let lower = command.to_ascii_lowercase();
        let game = lower
            .split(['\\', '/', ' '])
            .any(|part| part.starts_with("mercenaries2") && part.ends_with(".exe"));
        // The binary path can hold spaces (`Application Support`), so the command is searched
        // rather than split.
        let wine = lower.contains("/bin/wine") || lower.contains("-preloader");
        if game {
            hits.push(format!("pid {pid}: {command}"));
        } else if wine {
            if command.contains(&runtimes) {
                hits.push(format!("pid {pid}: {command}"));
                continue;
            }
            let env = ps(&["-wwE", "-o", "command=", "-p", pid])?;
            if env.contains(&format!("WINEPREFIX={prefix}")) {
                hits.push(format!("pid {pid} (WINEPREFIX={prefix}): {command}"));
            }
        }
    }
    // `comm` is the executable's path, which can hold spaces, so only its last segment is read.
    for line in ps(&["-axww", "-o", "pid=,comm="])?.lines() {
        let line = line.trim();
        let Some((pid, exe)) = line.split_once(' ') else { continue };
        let exe = exe.trim();
        if exe.rsplit('/').next() == Some("mercs2-modkit") {
            hits.push(format!("pid {pid}: the Modkit app, {exe}"));
        }
    }
    if hits.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the game, a Wine process of Modkit's prefix, or the Modkit app is running; close it \
             before a --real run:\n  {}",
            hits.join("\n  ")
        ))
    }
}

fn print_ledger() -> Result<(), String> {
    println!("ledger deployed-wad: {}", pretty(&deployed_wad_record()?));
    println!("ledger placed-files: {}", pretty(&placed_files()?));
    let backups: Vec<String> = list_patch_wad_backups()?.into_iter().map(|b| format!("{} {}", b.file, b.sha256)).collect();
    println!("wad backups: {}", pretty(&backups));
    Ok(())
}

/// sha256 of every file under the game root, and of every file in Modkit's `deployed/` folder
/// except the build output.
fn snapshot(root: &Path) -> Result<Value, String> {
    let mut game = serde_json::Map::new();
    walk(root, root, &mut game)?;
    let dep = deployed_dir()?;
    let mut app = serde_json::Map::new();
    walk(&dep, &dep, &mut app)?;
    app.retain(|k, _| !k.starts_with("build/"));
    Ok(json!({ "game_root": root.to_string_lossy(), "game": game, "app_data": app }))
}

fn walk(base: &Path, dir: &Path, out: &mut serde_json::Map<String, Value>) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("reading {}: {e}", dir.display()))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("reading {}: {e}", dir.display()))?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let ft = e.file_type().map_err(|err| format!("{}: {err}", path.display()))?;
        let rel = path.strip_prefix(base).map_err(|err| err.to_string())?.to_string_lossy().to_string();
        if ft.is_dir() {
            walk(base, &path, out)?;
        } else if ft.is_file() {
            out.insert(rel, json!(sha256_file(&path)?));
        } else {
            return Err(format!("{} is neither a file nor a folder", path.display()));
        }
    }
    Ok(())
}

fn diff(a: &Value, b: &Value) -> Vec<String> {
    let empty = serde_json::Map::new();
    let a = a.as_object().unwrap_or(&empty);
    let b = b.as_object().unwrap_or(&empty);
    let mut out = Vec::new();
    for (k, v) in a {
        match b.get(k) {
            None => out.push(format!("{k}: gone (was {v})")),
            Some(w) if w != v => out.push(format!("{k}: {v} -> {w}")),
            _ => {}
        }
    }
    for (k, w) in b {
        if !a.contains_key(k) {
            out.push(format!("{k}: new ({w})"));
        }
    }
    out
}

/// Files at least this large report hashing progress on stderr.
const PROGRESS_MIN_FILE: u64 = 64 << 20;
/// Bytes hashed between two progress lines.
const PROGRESS_STEP: u64 = 256 << 20;

fn sha256_file(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("opening {}: {e}", path.display()))?;
    let total = f.metadata().map_err(|e| format!("reading {}: {e}", path.display()))?.len();
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut done: u64 = 0;
    let mut next_report: u64 = PROGRESS_STEP;
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("reading {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        done += n as u64;
        if total >= PROGRESS_MIN_FILE && done >= next_report {
            eprintln!(
                "hashing {}: {} / {} MiB ({}%)",
                path.display(),
                done >> 20,
                total >> 20,
                done * 100 / total
            );
            next_report += PROGRESS_STEP;
        }
    }
    Ok(format!("{:x}", h.finalize()))
}

fn read_rows(path: &str) -> Result<Vec<ShipmentRef>, String> {
    serde_json::from_slice(&std::fs::read(path).map_err(|e| format!("reading {path}: {e}"))?)
        .map_err(|e| format!("{path}: {e}"))
}

enum Mode {
    /// The canonical scratch HOME.
    Scratch(PathBuf),
    Real,
}

/// Point `HOME` at `home` and prove Modkit's app-data folder now resolves inside it.
fn enter_scratch_home(home: &str) -> Result<PathBuf, String> {
    let home = std::fs::canonicalize(home).map_err(|e| format!("--scratch-home {home}: {e}"))?;
    if !home.is_dir() {
        return Err(format!("--scratch-home {} is not a folder", home.display()));
    }
    std::env::set_var("HOME", &home);
    let app = app_data_dir()?;
    if !app.starts_with(&home) {
        return Err(format!(
            "with HOME={} Modkit's app data resolves to {}, outside the scratch HOME",
            home.display(),
            app.display()
        ));
    }
    println!("scratch HOME: {}", home.display());
    refuse_ledger_paths_outside(&home)?;
    Ok(home)
}

/// The ledgers record absolute paths, and an uninstall acts on them: a ledger copied from another
/// HOME still names that HOME's game folder. Refuse unless every recorded path lies inside `home`.
fn refuse_ledger_paths_outside(home: &Path) -> Result<(), String> {
    let mut outside = Vec::new();
    for f in placed_files()? {
        let mut paths = vec![f.abs_path.clone()];
        if let Some(d) = &f.displaced {
            paths.push(d.original.clone());
            paths.push(d.backup.clone());
        }
        outside.extend(paths.into_iter().filter(|p| !Path::new(p).starts_with(home)));
    }
    if let Some(rec) = deployed_wad_record()? {
        if !Path::new(&rec.installed_at).starts_with(home) {
            outside.push(rec.installed_at);
        }
    }
    if outside.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the scratch HOME's ledgers record path(s) outside {}; rewrite them to the scratch game \
             root first:\n  {}",
            home.display(),
            outside.join("\n  ")
        ))
    }
}

/// The canonical game root. It must hold `data/vz.wad`, and in scratch mode it must lie inside the
/// scratch HOME.
fn game_root(given: &str, mode: &Mode) -> Result<PathBuf, String> {
    let root = std::fs::canonicalize(given).map_err(|e| format!("--game-root {given}: {e}"))?;
    if !root.join("data").join("vz.wad").is_file() {
        return Err(format!("{} has no data/vz.wad", root.display()));
    }
    if let Mode::Scratch(home) = mode {
        if !root.starts_with(home) {
            return Err(format!(
                "--game-root {} is outside the scratch HOME {}",
                root.display(),
                home.display()
            ));
        }
    }
    Ok(root)
}

/// Print what a `--real` run of `cmd` writes.
fn announce_real(cmd: &str, root: Option<&Path>) -> Result<(), String> {
    let app = app_data_dir()?;
    println!("--real {cmd} uses the caller's own Modkit app data: {}", app.display());
    let writes: &[&str] = match cmd {
        "install" | "uninstall" | "restore-wad" => &[
            "deployed/ (deployed-wad.json, placed-files.json, the data-file ledger and bank, \
             wad-backups/, build/)",
            "qm-work/ (replaced per qm run)",
            "trash/ (files taken out of the game folder)",
        ],
        "snapshot" | "verify" | "ledger" => &["deployed/ and trash/ are created when missing"],
        _ => &[],
    };
    for w in writes {
        println!("  app data: {w}");
    }
    if let Some(root) = root {
        match cmd {
            "install" | "uninstall" | "restore-wad" => println!(
                "  game root {}: data/vz-patch.wad, the loose files in the placed-files ledger and \
                 any they displace, and the data files in the data-file ledger",
                root.display()
            ),
            _ => println!("  game root {}: read only", root.display()),
        }
    }
    Ok(())
}

fn need<'a>(flags: &'a BTreeMap<String, String>, key: &str) -> Result<&'a str, String> {
    flags.get(key).map(String::as_str).ok_or_else(|| format!("--{key} is required"))
}

/// `--key value` pairs, plus the bare `--real`.
fn parse_flags(rest: &[String]) -> Result<(BTreeMap<String, String>, bool), String> {
    let mut out = BTreeMap::new();
    let mut real = false;
    let mut it = rest.iter();
    while let Some(k) = it.next() {
        let key = k.strip_prefix("--").ok_or_else(|| format!("expected a --flag, got {k}"))?;
        if key == "real" {
            real = true;
            continue;
        }
        let v = it.next().ok_or_else(|| format!("--{key} has no value"))?;
        if out.insert(key.to_string(), v.clone()).is_some() {
            return Err(format!("--{key} is given twice"));
        }
    }
    Ok((out, real))
}

fn pretty<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|e| format!("<unserializable: {e}>"))
}
