//! Mod catalog — a list of mod **repositories** (Home-Assistant-add-on style).
//!
//! `registry.json` is a hand-maintained list of repository sources (fetched from a
//! remote URL the curator maintains, falling back to the bundled copy offline).
//! Each source repo is itself an **index of mods**: a root `repository.json` whose
//! `mods` array lists objects — `{ slug, name, description, type, assets, version }` —
//! one per enableable mod (a bare slug string is also tolerated, legacy). We scan
//! every source into per-mod rows, deduped, so the user enables individual mods
//! rather than installing a repo wholesale. Parsing is per-entry: a single bad mod
//! is skipped, never collapsing the repo to one bundle row.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Remote list of repository sources the curator edits.
const REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/Mercenaries-Fan-Build/mercs2-modkit/main/registry.json";

/// Compiled-in fallback used when the remote fetch fails.
const BUNDLED: &str = include_str!("../../registry.json");

/// A repository source as listed in `registry.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct RepoSource {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Git repository hosting an index of mods.
    pub repository: String,
}

/// One enableable mod, expanded from a source repo's index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogMod {
    /// Source repository URL this mod came from.
    pub repository: String,
    /// Display name of the source repository.
    pub repo_name: String,
    /// Mod identifier, unique within its repository.
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// `"asi"` or `"wad"` (informational; the installer confirms at enable time).
    #[serde(default)]
    pub kind: String,
    /// Release asset filenames this mod deploys. Empty = whole-release (legacy).
    #[serde(default)]
    pub assets: Vec<String>,
    #[serde(default)]
    pub version: Option<String>,
}

/// The flattened, deduped catalog plus where the source list came from.
#[derive(Debug, Serialize)]
pub struct Catalog {
    pub mods: Vec<CatalogMod>,
    /// `"remote"` or `"bundled"`.
    pub source: String,
}

/// Root `repository.json` in a mod repo: metadata + the mods it indexes.
#[derive(Debug, Deserialize)]
struct RepoIndex {
    #[serde(default)]
    name: Option<String>,
    /// Left as raw JSON so a single malformed entry can be skipped without
    /// failing the whole index (which would collapse the repo to one bundle row).
    #[serde(default)]
    mods: Vec<serde_json::Value>,
}

/// One mod entry inside `repository.json`'s `mods` array.
#[derive(Debug, Deserialize)]
struct RepoMod {
    slug: String,
    name: String,
    #[serde(default)]
    description: String,
    /// Mod kind, e.g. `"asi"` or `"wad"`.
    #[serde(default, rename = "type")]
    kind: Option<String>,
    /// Release asset filenames this mod deploys.
    #[serde(default)]
    assets: Vec<String>,
    #[serde(default)]
    version: Option<String>,
}

/// Coerce one `mods` entry — an object, or a bare slug string (legacy) — into a
/// `CatalogMod`. Returns `None` for entries we can't make sense of (skipped).
fn repo_mod_to_catalog(value: serde_json::Value, src: &RepoSource, repo_name: &str) -> Option<CatalogMod> {
    let base = |slug: String, name: String, description: String, kind: String, assets: Vec<String>, version: Option<String>| {
        CatalogMod {
            repository: src.repository.clone(),
            repo_name: repo_name.to_string(),
            slug,
            name,
            description,
            kind,
            assets,
            version,
        }
    };
    if let Ok(m) = serde_json::from_value::<RepoMod>(value.clone()) {
        return Some(base(
            m.slug,
            m.name,
            m.description,
            m.kind.unwrap_or_else(|| "asi".to_string()),
            m.assets,
            m.version,
        ));
    }
    // Legacy form: a bare slug string. No assets known → whole-release download.
    if let Some(slug) = value.as_str() {
        if !slug.is_empty() {
            return Some(base(
                slug.to_string(),
                slug.to_string(),
                String::new(),
                "asi".to_string(),
                Vec::new(),
                None,
            ));
        }
    }
    None
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

/// Normalized repository URL used as a dedup key.
fn normalize_repo(url: &str) -> String {
    let s = url.trim().trim_end_matches('/');
    s.strip_suffix(".git").unwrap_or(s).to_lowercase()
}

/// `owner/repo` for a GitHub URL, or `None` for other hosts.
fn github_owner_repo(url: &str) -> Option<String> {
    let s = url.trim().trim_end_matches('/');
    let s = s.strip_suffix(".git").unwrap_or(s);
    let idx = s.find("github.com")?;
    let path = s[idx + "github.com".len()..].trim_start_matches([':', '/']);
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

async fn fetch_text(client: &reqwest::Client, url: &str) -> Option<String> {
    client
        .get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .await
        .ok()
}

async fn fetch_raw(
    client: &reqwest::Client,
    owner_repo: &str,
    branch: &str,
    path: &str,
) -> Option<String> {
    let url = format!("https://raw.githubusercontent.com/{owner_repo}/{branch}/{path}");
    fetch_text(client, &url).await
}

/// A single fallback row representing the whole repo (used when it has no index,
/// or isn't on GitHub). Empty `assets` makes the installer pull the whole release.
fn whole_repo_fallback(src: &RepoSource) -> CatalogMod {
    CatalogMod {
        repository: src.repository.clone(),
        repo_name: src.name.clone(),
        slug: slugify(&src.name),
        name: src.name.clone(),
        description: src.description.clone(),
        kind: String::new(),
        assets: Vec::new(),
        version: None,
    }
}

/// Expand one source repository into its individual mods by reading its index.
async fn scan_repo(client: &reqwest::Client, src: &RepoSource) -> Vec<CatalogMod> {
    let owner_repo = match github_owner_repo(&src.repository) {
        Some(o) => o,
        None => return vec![whole_repo_fallback(src)],
    };

    // Find the default branch carrying repository.json.
    let mut branch = "main";
    let mut index_txt = None;
    for b in ["main", "master"] {
        if let Some(t) = fetch_raw(client, &owner_repo, b, "repository.json").await {
            branch = b;
            index_txt = Some(t);
            break;
        }
    }
    // branch is resolved above (kept for any future per-mod fetches).
    let _ = branch;
    let index: RepoIndex = match index_txt.and_then(|t| serde_json::from_str(&t).ok()) {
        Some(i) => i,
        None => return vec![whole_repo_fallback(src)],
    };
    let repo_name = index.name.unwrap_or_else(|| src.name.clone());

    // Coerce each entry independently; skip malformed ones rather than collapsing
    // the whole repo to a single bundle row.
    let mods: Vec<CatalogMod> = index
        .mods
        .into_iter()
        .filter_map(|v| repo_mod_to_catalog(v, src, &repo_name))
        .collect();

    if mods.is_empty() {
        vec![whole_repo_fallback(src)]
    } else {
        mods
    }
}

fn parse_sources(text: &str) -> Vec<RepoSource> {
    serde_json::from_str::<Vec<RepoSource>>(text).unwrap_or_default()
}

/// Fetch the repository sources, preferring the remote list, falling back to the
/// bundled copy. Returns `(sources, label)`.
async fn fetch_sources(client: &reqwest::Client) -> (Vec<RepoSource>, &'static str) {
    if let Some(text) = fetch_text(client, REGISTRY_URL).await {
        let sources = parse_sources(&text);
        if !sources.is_empty() {
            return (sources, "remote");
        }
    }
    (parse_sources(BUNDLED), "bundled")
}

/// Build the catalog: scan every (deduped) source repo into per-mod rows, then
/// dedupe the mods themselves by `(repository, slug)`.
#[tauri::command]
pub async fn fetch_catalog() -> Catalog {
    let client = match reqwest::Client::builder()
        .user_agent("mercs2-modkit")
        .build()
    {
        Ok(c) => c,
        Err(_) => {
            return Catalog {
                mods: Vec::new(),
                source: "error".to_string(),
            }
        }
    };

    let (sources, label) = fetch_sources(&client).await;

    // Dedupe source repos by normalized URL (registry entries are indexes, not bundles).
    let mut seen_repos = HashSet::new();
    let sources: Vec<RepoSource> = sources
        .into_iter()
        .filter(|s| seen_repos.insert(normalize_repo(&s.repository)))
        .collect();

    let mut mods = Vec::new();
    let mut seen_mods = HashSet::new();
    for src in &sources {
        for m in scan_repo(&client, src).await {
            if seen_mods.insert((normalize_repo(&m.repository), m.slug.clone())) {
                mods.push(m);
            }
        }
    }

    Catalog {
        mods,
        source: label.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src() -> RepoSource {
        RepoSource {
            name: "Fallback".into(),
            description: String::new(),
            repository: "https://github.com/owner/repo".into(),
        }
    }

    #[test]
    fn parses_object_mods_into_per_mod_rows() {
        let json = r#"{
            "name": "QoL",
            "mods": [
                {"slug":"windowed-mode","name":"Windowed Mode","type":"asi","assets":["windowed_mode.asi"]},
                {"slug":"quiet-freeplay-vo","name":"Quiet Freeplay VO","type":"asi","assets":["quiet_freeplay_vo.asi","quiet_freeplay_vo.ini"]}
            ]
        }"#;
        let idx: RepoIndex = serde_json::from_str(json).unwrap();
        let mods: Vec<CatalogMod> = idx
            .mods
            .into_iter()
            .filter_map(|v| repo_mod_to_catalog(v, &src(), "QoL"))
            .collect();
        assert_eq!(mods.len(), 2, "expected per-mod rows, not a bundle");
        assert_eq!(mods[0].name, "Windowed Mode");
        assert_eq!(mods[0].kind, "asi");
        assert_eq!(mods[1].assets, vec!["quiet_freeplay_vo.asi", "quiet_freeplay_vo.ini"]);
    }

    #[test]
    fn tolerates_legacy_string_and_skips_bad_entries() {
        // A mix of a legacy bare-slug string, a valid object, and a junk entry.
        let json = r#"{"mods": ["windowed-mode", {"slug":"b","name":"B"}, 123]}"#;
        let idx: RepoIndex = serde_json::from_str(json).unwrap();
        let mods: Vec<CatalogMod> = idx
            .mods
            .into_iter()
            .filter_map(|v| repo_mod_to_catalog(v, &src(), "R"))
            .collect();
        assert_eq!(mods.len(), 2);
        assert_eq!(mods[0].slug, "windowed-mode");
        assert_eq!(mods[1].slug, "b");
    }
}
