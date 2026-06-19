//! Latest-release lookup, used both to self-update modkit and to check mods for
//! newer versions. GitHub only (the catalog repos and modkit live there).

use serde::Serialize;

/// Summary of a repository's latest GitHub release.
#[derive(Debug, Serialize)]
pub struct ReleaseInfo {
    /// Release tag, e.g. `v0.2.0`.
    pub tag: String,
    /// Release title (falls back to the tag).
    pub name: String,
    /// Browser URL of the release page.
    pub url: String,
    /// Release notes (may be empty).
    pub body: String,
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

/// Fetch the latest release of a GitHub repository.
#[tauri::command]
pub async fn latest_release(repo: String) -> Result<ReleaseInfo, String> {
    let owner_repo = github_owner_repo(&repo)
        .ok_or_else(|| format!("Not a GitHub repository: {repo}"))?;

    let client = reqwest::Client::builder()
        .user_agent("mercs2-modkit")
        .build()
        .map_err(|e| e.to_string())?;

    let api = format!("https://api.github.com/repos/{owner_repo}/releases/latest");
    let v: serde_json::Value = client
        .get(&api)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| format!("Release lookup failed for {owner_repo}: {e}"))?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let tag = v["tag_name"].as_str().unwrap_or_default().to_string();
    if tag.is_empty() {
        return Err(format!("{owner_repo} has no published releases"));
    }
    let name = v["name"].as_str().filter(|s| !s.is_empty()).unwrap_or(&tag).to_string();
    let url = v["html_url"].as_str().unwrap_or_default().to_string();
    let body = v["body"].as_str().unwrap_or_default().to_string();

    Ok(ReleaseInfo { tag, name, url, body })
}
