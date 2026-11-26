//! Shared by the wardrobe examples: build outfits the way the app does, in a scratch `HOME`.
//!
//! The app builds every outfit through `wad_builder::assemble`, which checks each model against the
//! game, writes the outfits as a Quartermaster Shipment and runs `qm` (preflight, build, link). That
//! writes under Modkit's app-data folder (`qm-work/`), so the examples point `HOME` at a scratch
//! folder first and refuse to run unless the app-data folder resolves inside it.

use std::path::{Path, PathBuf};

use mercs2_formats::patch_wad::read_patch_wad;
use mercs2_formats::scripts_block::ScriptsBlock;
use mercs2_formats::sges::decompress_sges;
use mercs2_modkit_lib::commands::paths::app_data_dir;
use mercs2_modkit_lib::commands::wad_builder::{assemble, BuildOptions};
use mercs2_modkit_lib::commands::wardrobe::WardrobeOutfit;

/// The first three arguments of every wardrobe example: `<game root> <qm> <scratch home>`.
pub struct Args {
    pub game: String,
    pub qm: PathBuf,
    pub scratch: PathBuf,
}

/// Read the arguments and enter the scratch `HOME`. Call it before anything resolves an app-data
/// path.
pub fn enter(usage: &str) -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut next = || args.next().ok_or_else(|| format!("usage: {usage}"));
    let game = next()?;
    let qm = PathBuf::from(next()?);
    let scratch = next()?;

    if !qm.is_file() {
        return Err(format!("no qm at {}", qm.display()));
    }
    std::fs::create_dir_all(&scratch).map_err(|e| format!("creating {scratch}: {e}"))?;
    let scratch = std::fs::canonicalize(&scratch).map_err(|e| format!("{scratch}: {e}"))?;
    std::env::set_var("HOME", &scratch);
    let app = app_data_dir()?;
    if !app.starts_with(&scratch) {
        return Err(format!(
            "with HOME={} Modkit's app data resolves to {}, outside the scratch folder",
            scratch.display(),
            app.display()
        ));
    }
    println!("scratch HOME: {}", scratch.display());
    Ok(Args { game, qm, scratch })
}

/// A wardrobe build: the patch WAD and the `wifpmcinterior` bytecode inside it.
pub struct Built {
    pub wad_path: PathBuf,
    pub sha256: String,
    pub block_count: usize,
    /// The linked `wifpmcinterior` chunk (LuaQ), the script that holds `_tOutfits`.
    pub interior_luaq: Vec<u8>,
}

/// Build `outfits` and nothing else into `<scratch>/out/vz-patch.wad`.
pub fn build(args: &Args, outfits: &[WardrobeOutfit]) -> Result<Built, String> {
    let out = args.scratch.join("out");
    let options: BuildOptions = serde_json::from_value(serde_json::json!({
        "mods": [],
        "output_dir": out.to_string_lossy(),
        "game_path": args.game,
        "wardrobe": outfits,
        "prebuilt": [],
        "textures": [],
        "shipments": [],
    }))
    .map_err(|e| format!("describing the build: {e}"))?;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("starting the async runtime: {e}"))?;
    let result = rt.block_on(assemble(options, Some(&args.qm), &|line| println!("  qm: {line}")))?;
    if result.path.is_empty() {
        return Err("the build wrote no vz-patch.wad".into());
    }

    let wad_path = PathBuf::from(&result.path);
    Ok(Built {
        interior_luaq: interior_luaq(&wad_path)?,
        wad_path,
        sha256: result.sha256,
        block_count: result.block_count,
    })
}

/// The `wifpmcinterior` chunk of the scripts block in the patch WAD at `wad`.
fn interior_luaq(wad: &Path) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(wad).map_err(|e| format!("reading {}: {e}", wad.display()))?;
    let contents = read_patch_wad(&bytes).map_err(|e| format!("{}: {e}", wad.display()))?;
    let block = contents
        .blocks
        .iter()
        .find(|b| b.path_string.to_lowercase().contains("scripts_vz"))
        .ok_or_else(|| format!("{} has no scripts_vz block", wad.display()))?;
    let raw = decompress_sges(&block.compressed_data).map_err(|e| format!("inflating scripts_vz: {e}"))?;
    let scripts = ScriptsBlock::parse(&raw).map_err(|e| format!("parsing scripts_vz: {e}"))?;
    let entry = scripts
        .find_by_name("wifpmcinterior")
        .ok_or("the built scripts_vz has no wifpmcinterior")?;
    let luaq = scripts.extract_lua(entry).map_err(|e| format!("extracting wifpmcinterior: {e}"))?;
    if luaq.get(..4) != Some(b"\x1bLua".as_slice()) {
        return Err("the built wifpmcinterior is not LuaQ bytecode".into());
    }
    Ok(luaq)
}
