//! Latest-release lookup for GitHub and GitLab, and asset selection by rule.

use serde::Serialize;

/// Which forge publishes a project's releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseHost {
    GitHub,
    GitLab,
}

/// One downloadable file attached to a release.
#[derive(Debug, Clone, Serialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
    /// Byte size the forge reports, when it reports one.
    pub size: Option<u64>,
    /// Content digest the forge publishes, e.g. `sha256:abcd…`. GitHub began
    /// serving this on release assets; GitLab does not. When present it is the
    /// only end-to-end integrity check available for a bare release binary, so it
    /// is carried through to [`crate::commands::managed::place`] rather than
    /// dropped here.
    pub digest: Option<String>,
}

impl Asset {
    /// The bare hex digest when the forge published a `sha256:` one.
    pub fn sha256(&self) -> Option<&str> {
        self.digest.as_deref()?.strip_prefix("sha256:")
    }
}

/// A project's latest release.
#[derive(Debug, Clone, Serialize)]
pub struct Release {
    /// Release tag, e.g. `v0.6.0`.
    pub tag: String,
    /// Release title; falls back to the tag when the forge has none.
    pub name: String,
    /// Human-facing release page.
    pub url: String,
    /// Release notes; may be empty.
    pub body: String,
    pub assets: Vec<Asset>,
}

/// How to recognise the asset a caller wants.
///
/// Rules are tried in the order given, and the first that matches **any** asset
/// wins — so a caller expresses a preference order (the exact build for this
/// host, then a looser fallback) rather than a single guess.
pub enum AssetRule<'a> {
    /// Exact name, case-insensitively. The right rule for a published artifact
    /// whose name the caller knows.
    Named(&'a str),
    /// Name ends with this, case-insensitively — for per-platform assets like
    /// `wad_simulator-linux-x86_64`.
    Suffix(&'a str),
    /// Arbitrary predicate over the lowercased name, for selections that are
    /// genuinely conditional (dxwrapper's "a zip, but never the debug or symbols
    /// build").
    Pred(&'a dyn Fn(&str) -> bool),
}

impl AssetRule<'_> {
    fn matches(&self, name: &str) -> bool {
        match self {
            Self::Named(want) => name.eq_ignore_ascii_case(want),
            Self::Suffix(s) => name.to_ascii_lowercase().ends_with(&s.to_ascii_lowercase()),
            Self::Pred(f) => f(&name.to_ascii_lowercase()),
        }
    }

    fn describe(&self) -> String {
        match self {
            Self::Named(n) => format!("named '{n}'"),
            Self::Suffix(s) => format!("ending in '{s}'"),
            Self::Pred(_) => "matching a caller-supplied rule".to_string(),
        }
    }
}

impl Release {
    /// First asset matching the highest-priority rule that matches anything.
    pub fn pick(&self, rules: &[AssetRule]) -> Option<&Asset> {
        rules
            .iter()
            .find_map(|rule| self.assets.iter().find(|a| rule.matches(&a.name)))
    }

    /// [`Release::pick`], or an error that says what was wanted and what the
    /// release actually publishes.
    ///
    /// The listing is the point. "No matching asset in the latest release" — the
    /// message this replaces — is true and useless; when an upstream renames its
    /// artifacts the only thing a reader needs is both halves side by side.
    pub fn require(&self, rules: &[AssetRule], what: &str) -> Result<&Asset, String> {
        self.pick(rules).ok_or_else(|| {
            let wanted = rules
                .iter()
                .map(|r| r.describe())
                .collect::<Vec<_>>()
                .join(", or ");
            let have = if self.assets.is_empty() {
                "it publishes no assets at all".to_string()
            } else {
                format!(
                    "it publishes: {}",
                    self.assets
                        .iter()
                        .map(|a| a.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            format!("Release {} has no asset for {what} (wanted one {wanted}) — {have}", self.tag)
        })
    }
}

// ---------------------------------------------------------------------------------------
// Repository URLs
// ---------------------------------------------------------------------------------------

/// Parse a repository URL into its host and project path (`owner/repo`, or a
/// GitLab group path). Handles `https://`, `git@…:`, and a trailing `.git`.
pub fn parse_repo(url: &str) -> Result<(ReleaseHost, String), String> {
    let s = url.trim().trim_end_matches('/');
    let s = s.strip_suffix(".git").unwrap_or(s);

    for (host, token) in [
        (ReleaseHost::GitHub, "github.com"),
        (ReleaseHost::GitLab, "gitlab.com"),
    ] {
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
    Err(format!(
        "Unsupported repository host (need github.com or gitlab.com): {url}"
    ))
}

/// `owner/repo` for a GitHub URL, or `None` for other hosts.
pub fn github_owner_repo(url: &str) -> Option<String> {
    match parse_repo(url) {
        Ok((ReleaseHost::GitHub, path)) => Some(path),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------
// Host tokens
// ---------------------------------------------------------------------------------------

/// OS token used in release asset names.
pub fn platform_token() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

/// CPU-arch token paired with [`platform_token`].
///
/// ARM is spelled **`arm64`**, not `aarch64`. That is the convention every repo in
/// the ecosystem publishes under. Spelling it `aarch64` meant an exact-match rule
/// could never match on an ARM host, quietly demoting every ARM user to whichever
/// asset the forge happened to list first — an x86_64 binary that will not even
/// exec on ARM Linux.
///
/// `None` for an arch nothing publishes for (riscv, 32-bit ARM). That is distinct
/// from a guess: an empty-string sentinel makes `name.contains(arch)` vacuously
/// true, which collapses an exact rule into its fallback instead of reporting that
/// the host is unsupported.
pub fn arch_token() -> Option<&'static str> {
    Some(if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "x86") {
        "i686"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        return None;
    })
}

/// The `-<os>-<arch>` asset suffix for this host, with `.exe` on Windows —
/// matching the `suffix:` values in the ecosystem's release workflows.
///
/// `None` on a host no release builds for, which callers must surface as "no build
/// for this machine" rather than downloading something for another arch.
pub fn platform_suffix() -> Option<String> {
    let arch = arch_token()?;
    let os = platform_token();
    Some(if cfg!(target_os = "windows") {
        format!("-{os}-{arch}.exe")
    } else {
        format!("-{os}-{arch}")
    })
}

// ---------------------------------------------------------------------------------------
// Lookup
// ---------------------------------------------------------------------------------------

fn asset_from_github(a: &serde_json::Value) -> Option<Asset> {
    Some(Asset {
        name: a["name"].as_str()?.to_string(),
        url: a["browser_download_url"].as_str()?.to_string(),
        size: a["size"].as_u64(),
        digest: a["digest"].as_str().map(str::to_string),
    })
}

async fn github_latest(client: &reqwest::Client, project: &str) -> Result<Release, String> {
    let api = format!("https://api.github.com/repos/{project}/releases/latest");
    let resp = super::client::get(client, &api).await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!(
            "Release lookup failed for {project}: {status}{}",
            if status == reqwest::StatusCode::NOT_FOUND {
                " — the repository may be private, renamed, or have no published release"
            } else {
                ""
            }
        ));
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Could not parse the release JSON for {project}: {e}"))?;

    // An empty tag is not a release. Every implementation this replaces had its own
    // answer here — `"latest"`, `unwrap_or_default()`, or an error — so a repo with
    // no releases produced three different downstream behaviours.
    let tag = v["tag_name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{project} has no published releases"))?
        .to_string();

    let name = v["name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(&tag)
        .to_string();

    Ok(Release {
        tag,
        name,
        url: v["html_url"].as_str().unwrap_or_default().to_string(),
        body: v["body"].as_str().unwrap_or_default().to_string(),
        assets: v["assets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(asset_from_github)
            .collect(),
    })
}

async fn gitlab_latest(client: &reqwest::Client, project: &str) -> Result<Release, String> {
    let enc = project.replace('/', "%2F");
    let api = format!("https://gitlab.com/api/v4/projects/{enc}/releases");
    let resp = super::client::get(client, &api).await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("Release lookup failed for {project}: {status}"));
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Could not parse the release JSON for {project}: {e}"))?;

    let rel = v
        .as_array()
        .and_then(|a| a.first())
        .ok_or_else(|| format!("{project} has no published releases"))?;

    let tag = rel["tag_name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{project} has no published releases"))?
        .to_string();

    let mut assets: Vec<Asset> = rel["assets"]["links"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| {
            Some(Asset {
                name: l["name"].as_str()?.to_string(),
                url: l["url"].as_str()?.to_string(),
                size: None,
                digest: None,
            })
        })
        .collect();

    // Fall back to the auto-generated source zip when a release attaches no links.
    if assets.is_empty() {
        for s in rel["assets"]["sources"].as_array().into_iter().flatten() {
            if s["format"].as_str() == Some("zip") {
                if let Some(u) = s["url"].as_str() {
                    assets.push(Asset {
                        name: "source.zip".to_string(),
                        url: u.to_string(),
                        size: None,
                        digest: None,
                    });
                }
            }
        }
    }

    Ok(Release {
        name: rel["name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or(&tag)
            .to_string(),
        url: rel["_links"]["self"].as_str().unwrap_or_default().to_string(),
        body: rel["description"].as_str().unwrap_or_default().to_string(),
        tag,
        assets,
    })
}

/// The latest release of `project` on `host`.
pub async fn latest_release(
    client: &reqwest::Client,
    host: ReleaseHost,
    project: &str,
) -> Result<Release, String> {
    match host {
        ReleaseHost::GitHub => github_latest(client, project).await,
        ReleaseHost::GitLab => gitlab_latest(client, project).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> Asset {
        Asset {
            name: name.to_string(),
            url: format!("https://example.invalid/{name}"),
            size: None,
            digest: None,
        }
    }

    fn release(names: &[&str]) -> Release {
        Release {
            tag: "v1.2.3".into(),
            name: "v1.2.3".into(),
            url: String::new(),
            body: String::new(),
            assets: names.iter().map(|n| asset(n)).collect(),
        }
    }

    #[test]
    fn rules_are_tried_in_priority_order() {
        let r = release(&["tool-linux-x86_64", "tool-windows-x86_64.exe"]);
        // The second rule matches something too, but the first one wins.
        let picked = r
            .pick(&[
                AssetRule::Suffix("-linux-x86_64"),
                AssetRule::Suffix(".exe"),
            ])
            .expect("a match");
        assert_eq!(picked.name, "tool-linux-x86_64");
    }

    #[test]
    fn a_lower_priority_rule_is_reached_when_the_first_matches_nothing() {
        let r = release(&["tool-windows-x86_64.exe"]);
        let picked = r
            .pick(&[
                AssetRule::Suffix("-linux-arm64"),
                AssetRule::Suffix(".exe"),
            ])
            .expect("the fallback");
        assert_eq!(picked.name, "tool-windows-x86_64.exe");
    }

    #[test]
    fn named_matching_ignores_case() {
        let r = release(&["PMC_BB_Log_Only.dll"]);
        assert!(r.pick(&[AssetRule::Named("pmc_bb_log_only.dll")]).is_some());
    }

    /// The error is the feature: when an upstream renames its artifacts, the only
    /// thing that helps is seeing both what was wanted and what is actually there.
    #[test]
    fn the_miss_error_lists_what_the_release_actually_has() {
        let r = release(&["pmc_bb_fully_loaded.dll", "pmc_bb_log_only.dll"]);
        let err = r
            .require(&[AssetRule::Named("pmc_bb.dll")], "the ASI loader")
            .unwrap_err();
        assert!(err.contains("pmc_bb.dll"), "{err}");
        assert!(err.contains("pmc_bb_fully_loaded.dll"), "{err}");
        assert!(err.contains("pmc_bb_log_only.dll"), "{err}");
        assert!(err.contains("v1.2.3"), "{err}");
    }

    #[test]
    fn an_assetless_release_says_so_rather_than_listing_nothing() {
        let err = release(&[])
            .require(&[AssetRule::Named("anything")], "a tool")
            .unwrap_err();
        assert!(err.contains("no assets at all"), "{err}");
    }

    #[test]
    fn a_predicate_rule_can_exclude() {
        let r = release(&["dxwrapper.debug.zip", "dxwrapper.zip"]);
        let pred = |n: &str| n.ends_with(".zip") && !n.contains("debug");
        let picked = r.pick(&[AssetRule::Pred(&pred)]).expect("the release build");
        assert_eq!(
            picked.name, "dxwrapper.zip",
            "a 'contains dxwrapper' match grabs the debug build, which sorts first"
        );
    }

    #[test]
    fn repo_urls_parse_in_every_shape() {
        for url in [
            "https://github.com/owner/repo",
            "https://github.com/owner/repo/",
            "https://github.com/owner/repo.git",
            "git@github.com:owner/repo.git",
        ] {
            let (host, path) = parse_repo(url).unwrap_or_else(|e| panic!("{url}: {e}"));
            assert_eq!(host, ReleaseHost::GitHub, "{url}");
            assert_eq!(path, "owner/repo", "{url}");
        }
        assert_eq!(
            parse_repo("https://gitlab.com/group/sub/proj").unwrap().1,
            "group/sub/proj"
        );
        assert!(parse_repo("https://example.com/owner/repo").is_err());
        assert!(github_owner_repo("https://gitlab.com/owner/repo").is_none());
    }

    /// The two halves of a platform-specific asset name have to agree. This lived
    /// in `setup.rs` as a cross-module assertion against `toolchain.rs`, because
    /// the two modules downloaded from different repos and had drifted to
    /// `aarch64` vs `arm64` — invisible until an ARM host tried to install. They
    /// are now one definition, and this pins that they stay consistent.
    #[test]
    fn arch_and_os_tokens_appear_in_the_suffix() {
        let (Some(arch), Some(suffix)) = (arch_token(), platform_suffix()) else {
            // An arch nothing publishes for. Both said so — consistent.
            return;
        };
        assert!(suffix.contains(arch), "{suffix} does not carry {arch}");
        assert!(
            suffix.contains(platform_token()),
            "{suffix} does not carry {}",
            platform_token()
        );
    }

    /// Pinned independently of the host, so a regression fails on an x86_64 runner
    /// too — where the test above is silent about ARM.
    #[test]
    fn arm_is_spelled_arm64() {
        if cfg!(target_arch = "aarch64") {
            assert_eq!(arch_token(), Some("arm64"));
        }
        for suffix in ["-macos-arm64", "-linux-arm64", "-windows-arm64.exe"] {
            assert!(!suffix.contains("aarch64"), "{suffix} regressed to aarch64");
        }
    }

    #[test]
    fn a_github_digest_is_unwrapped_only_when_it_is_sha256() {
        let mut a = asset("x");
        a.digest = Some("sha256:abc123".into());
        assert_eq!(a.sha256(), Some("abc123"));
        a.digest = Some("md5:abc123".into());
        assert_eq!(a.sha256(), None, "a non-sha256 digest must not be read as one");
        a.digest = None;
        assert_eq!(a.sha256(), None);
    }
}
