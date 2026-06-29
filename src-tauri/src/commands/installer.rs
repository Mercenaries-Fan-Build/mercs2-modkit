//! Install a catalog mod by pulling its repo's latest release artifacts.
//!
//! Lifecycle: download release assets into `downloading/<id>/`, unpack into
//! `staging/<id>/`, then hand the staged mod root back to the frontend to load.
//! Supports GitHub and GitLab release endpoints.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::commands::paths::{downloading_dir, staging_dir};
use crate::commands::registry::CatalogMod;

/// Outcome of a successful install: where the mod was staged and what kind it is.
#[derive(Debug, Serialize)]
pub struct InstallResult {
    /// Staging directory: the `manifest.json` folder for `"wad"` mods, or the
    /// stage root holding the `.asi` plugin(s) for `"asi"` mods.
    pub mod_root: String,
    /// `"wad"` (manifest + assets, assembled into a patch WAD) or `"asi"`
    /// (binary plugin(s) deployed into the game folder for the ASI loader).
    pub kind: String,
    /// Release tag the artifacts came from.
    pub version: String,
    /// Staged `.asi` files, relative to `mod_root` (empty for `"wad"` mods).
    pub asi_files: Vec<String>,
    pub staged_files: usize,
}

enum Host {
    GitHub,
    GitLab,
}

fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in name.chars() {
        if c.is_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Parse a repo URL into its host and project path (`owner/repo`, or a
/// GitLab group path). Handles `https://`, `git@…:`, and trailing `.git`.
fn parse_repo(url: &str) -> Result<(Host, String), String> {
    let s = url.trim().trim_end_matches('/');
    let s = s.strip_suffix(".git").unwrap_or(s);

    for (host, token) in [(Host::GitHub, "github.com"), (Host::GitLab, "gitlab.com")] {
        if let Some(idx) = s.find(token) {
            let path = s[idx + token.len()..]
                .trim_start_matches([':', '/'])
                .to_string();
            if path.is_empty() {
                return Err(format!("No project path in repository URL: {url}"));
            }
            return Ok((host, path));
        }
    }
    Err(format!("Unsupported repository host (need github.com or gitlab.com): {url}"))
}

async fn download(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    Ok(resp.bytes().await.map_err(|e| e.to_string())?.to_vec())
}

/// `(tag, [(asset_name, download_url)])` for the latest GitHub release.
async fn github_artifacts(
    client: &reqwest::Client,
    owner_repo: &str,
) -> Result<(String, Vec<(String, String)>), String> {
    let api = format!("https://api.github.com/repos/{owner_repo}/releases/latest");
    let v: serde_json::Value = client
        .get(&api)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| format!("GitHub release lookup failed: {e}"))?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let tag = v["tag_name"].as_str().unwrap_or("latest").to_string();
    let mut assets = Vec::new();
    for a in v["assets"].as_array().into_iter().flatten() {
        if let (Some(n), Some(u)) = (a["name"].as_str(), a["browser_download_url"].as_str()) {
            assets.push((n.to_string(), u.to_string()));
        }
    }
    Ok((tag, assets))
}

/// `(tag, [(asset_name, download_url)])` for the latest GitLab release.
async fn gitlab_artifacts(
    client: &reqwest::Client,
    project_path: &str,
) -> Result<(String, Vec<(String, String)>), String> {
    let enc = project_path.replace('/', "%2F");
    let api = format!("https://gitlab.com/api/v4/projects/{enc}/releases");
    let v: serde_json::Value = client
        .get(&api)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| format!("GitLab release lookup failed: {e}"))?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let rel = v
        .as_array()
        .and_then(|a| a.first())
        .ok_or("GitLab project has no releases")?;
    let tag = rel["tag_name"].as_str().unwrap_or("latest").to_string();

    let mut assets = Vec::new();
    for l in rel["assets"]["links"].as_array().into_iter().flatten() {
        if let (Some(n), Some(u)) = (l["name"].as_str(), l["url"].as_str()) {
            assets.push((n.to_string(), u.to_string()));
        }
    }
    // Fall back to the auto-generated source zip if no custom links exist.
    if assets.is_empty() {
        for s in rel["assets"]["sources"].as_array().into_iter().flatten() {
            if s["format"].as_str() == Some("zip") {
                if let Some(u) = s["url"].as_str() {
                    assets.push(("source.zip".to_string(), u.to_string()));
                }
            }
        }
    }
    Ok((tag, assets))
}

fn extract_zip(archive: &Path, dest: &Path) -> Result<(), String> {
    let f = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| format!("Bad zip archive: {e}"))?;
    z.extract(dest).map_err(|e| format!("Failed to extract archive: {e}"))
}

/// Find the directory containing `manifest.json` (at the stage root or one
/// level down, since archives often wrap everything in a top folder).
fn find_manifest_root(stage: &Path) -> Option<PathBuf> {
    if stage.join("manifest.json").is_file() {
        return Some(stage.to_path_buf());
    }
    for e in std::fs::read_dir(stage).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() && p.join("manifest.json").is_file() {
            return Some(p);
        }
    }
    None
}

/// Collect `.asi` plugin files in `dir` (depth ≤ 1), as names relative to it.
fn find_asi_files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let is_asi = |p: &Path| {
        p.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("asi"))
            .unwrap_or(false)
    };
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_file() && is_asi(&p) {
                if let Some(n) = p.file_name().and_then(|n| n.to_str()) {
                    out.push(n.to_string());
                }
            } else if p.is_dir() {
                // one level down, prefixed with the subdir name
                if let Ok(sub) = std::fs::read_dir(&p) {
                    let prefix = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    for se in sub.flatten() {
                        let sp = se.path();
                        if sp.is_file() && is_asi(&sp) {
                            if let Some(n) = sp.file_name().and_then(|n| n.to_str()) {
                                out.push(format!("{prefix}/{n}"));
                            }
                        }
                    }
                }
            }
        }
    }
    out.sort();
    out
}

fn count_files(dir: &Path) -> usize {
    let mut n = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                n += count_files(&p);
            } else {
                n += 1;
            }
        }
    }
    n
}

/// Import local `.asi` plugin file(s) as an ASI mod by staging copies of them.
/// `name` overrides the derived mod name (default: first file's stem).
#[tauri::command]
pub fn import_local_asi(paths: Vec<String>, name: Option<String>) -> Result<InstallResult, String> {
    if paths.is_empty() {
        return Err("No files selected".to_string());
    }
    for p in &paths {
        let pp = Path::new(p);
        let is_asi = pp
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("asi"))
            .unwrap_or(false);
        if !pp.is_file() || !is_asi {
            return Err(format!("Not an .asi plugin file: {p}"));
        }
    }

    let derived = name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
        Path::new(&paths[0])
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("plugin")
            .to_string()
    });
    let id = slugify(&derived);

    let stage = staging_dir()?.join(&id);
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;

    let mut asi_files = Vec::new();
    for p in &paths {
        let src = Path::new(p);
        let base = src
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("Bad file name")?;
        std::fs::copy(src, stage.join(base)).map_err(|e| format!("Failed to copy {base}: {e}"))?;
        asi_files.push(base.to_string());
    }
    asi_files.sort();

    let staged_files = asi_files.len();
    Ok(InstallResult {
        mod_root: stage.to_string_lossy().to_string(),
        kind: "asi".to_string(),
        version: "local".to_string(),
        asi_files,
        staged_files,
    })
}

/// Enable a single catalog mod: download its source repo's latest release, stage
/// only the asset(s) this mod declares (or the whole release if it declares none),
/// and return the loadable mod root.
#[tauri::command]
pub async fn install_catalog_mod(item: CatalogMod) -> Result<InstallResult, String> {
    // Stage per (repo, mod) so two mods from one repo — or same-named mods from
    // different repos — never collide.
    let id = slugify(&format!("{}-{}", item.repo_name, item.slug));
    let (host, path) = parse_repo(&item.repository)?;

    let client = reqwest::Client::builder()
        .user_agent("mercs2-modkit")
        .build()
        .map_err(|e| e.to_string())?;

    let (tag, all_assets) = match host {
        Host::GitHub => github_artifacts(&client, &path).await?,
        Host::GitLab => gitlab_artifacts(&client, &path).await?,
    };
    if all_assets.is_empty() {
        return Err(format!(
            "The latest release of {} has no downloadable artifacts",
            item.name
        ));
    }

    // Select just this mod's declared assets; empty list = whole release (legacy).
    let assets: Vec<(String, String)> = if item.assets.is_empty() {
        all_assets
    } else {
        // First try: every declared asset exists as a loose release file.
        let direct: Vec<(String, String)> = item
            .assets
            .iter()
            .filter_map(|want| {
                all_assets
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case(want))
                    .map(|(n, u)| (n.clone(), u.clone()))
            })
            .collect();
        if direct.len() == item.assets.len() {
            direct
        } else {
            // Second try: a zip whose name contains the mod slug
            // (e.g. "multiplayer-restore.zip" for slug "multiplayer-restore").
            // The zip is extracted and the declared assets are expected inside it.
            let slug_lc = item.slug.to_ascii_lowercase();
            let zip = all_assets.iter().find(|(n, _)| {
                let nl = n.to_ascii_lowercase();
                nl.ends_with(".zip") && nl.contains(&slug_lc)
            });
            match zip {
                Some((n, u)) => vec![(n.clone(), u.clone())],
                None => {
                    return Err(format!(
                        "Release {tag} of {} has no matching loose assets and no zip named after its slug '{}'",
                        item.name, item.slug
                    ))
                }
            }
        }
    };

    // Fresh download + staging dirs for this mod.
    let dl = downloading_dir()?.join(&id);
    let stage = staging_dir()?.join(&id);
    let _ = std::fs::remove_dir_all(&dl);
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&dl).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;

    // Prefer a single .zip artifact; otherwise stage all assets loose.
    if let Some((name, url)) = assets
        .iter()
        .find(|(n, _)| n.to_ascii_lowercase().ends_with(".zip"))
    {
        let bytes = download(&client, url).await?;
        let archive = dl.join(name);
        std::fs::write(&archive, &bytes).map_err(|e| e.to_string())?;
        extract_zip(&archive, &stage)?;
    } else {
        for (name, url) in &assets {
            let bytes = download(&client, url).await?;
            std::fs::write(stage.join(name), &bytes).map_err(|e| e.to_string())?;
        }
    }

    // A WAD-asset mod ships a manifest.json; an ASI mod ships .asi plugin(s).
    let (kind, mod_root, asi_files) = if let Some(root) = find_manifest_root(&stage) {
        let asi = find_asi_files(&root);
        ("wad", root, asi)
    } else {
        let asi = find_asi_files(&stage);
        if asi.is_empty() {
            return Err(format!(
                "Release artifacts of {} contain neither a manifest.json nor an .asi plugin",
                item.name
            ));
        }
        ("asi", stage.clone(), asi)
    };

    Ok(InstallResult {
        staged_files: count_files(&mod_root),
        mod_root: mod_root.to_string_lossy().to_string(),
        kind: kind.to_string(),
        version: tag,
        asi_files,
    })
}
