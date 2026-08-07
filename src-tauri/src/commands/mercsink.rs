//! The mercs.ink registry client — the missing producer of `OriginSource::Registry`.
//!
//! Written against `mercs.ink/.claude/plans/modkit-api-v1.md` (spec v1.1). That document is the
//! authority on the wire; where a comment here restates it, the spec wins.
//!
//! # Why this module exists
//!
//! Modkit has had two mod catalogues in it for a while, and until now only one was wired up.
//! [`super::registry`] reads a curated `registry.json` of GitHub repositories, each holding its
//! own `repository.json` index; that is Modkit's own scheme and it stays (see "Both
//! catalogues"). mercs.ink has served this API since before Modkit had any client for it.
//!
//! The cost was not merely a missing feature. [`Origin::registry`] existed, was tested, and was
//! **unreachable**: no code path could construct one, so every Shipment in the load order
//! reported `source: local` regardless of where the user got it, and the public identity the
//! crash-reporting contract specifies had no producer at all.
//!
//! # The identifier is opaque
//!
//! `ModResource.id` is **precomposed by mercs.ink** and copied verbatim into [`Origin::id`].
//! Modkit never rebuilds it from a slug and a repo id of its own — spec §5.1 asks for exactly
//! that discipline, and the reason is that two implementations of one identity format drift,
//! and the day they disagree a mod's history splits into two buckets where the drop reads as a
//! fix. If the field is absent — an older deployment — the entry records `id: None`. An
//! absence is legible; a guess is not.
//!
//! # Conditional requests are the intended usage
//!
//! Spec §4: strong ETags, `Cache-Control: max-age=60, public`, and a `304` on a matching
//! `If-None-Match`. [`FetchCache`] persists `(etag, body)` per URL, so the steady-state launch
//! poll costs one 304. §3's rate limit (120/min/IP across all of `/api/v1/*`) and §9 step 6's
//! "fall back to cache, surface a banner, do not block" are both handled in [`fetch`].
//!
//! One hazard the client cannot defend against, recorded so nobody debugs it from this side.
//! The server's validator is **not** a hash of the response body — it is derived from a payload
//! version, a registry token, and the path. Content changes bump the token automatically, but a
//! change to the *shape* of a resource only invalidates if someone bumps the payload version by
//! hand. Miss that, and a client holding a matching token is answered `304` and serves a body
//! missing the new field, indefinitely, until an unrelated write. It has been missed once
//! already — on `ModResource.id`, the field this module exists to read, caught and fixed before
//! release. There is nothing correct for a client to do about it: revalidating is exactly what
//! §4 asks for, and ignoring a `304` would defeat the cache the server is built around. So this
//! is a note, not a workaround; if `id` is mysteriously absent against a server that sends it,
//! this is why, and the fix is server-side.
//!
//! # The manifest is served already parsed
//!
//! Spec §6: every release carries the full parsed Quartermaster manifest as JSON. So identity
//! (`shipment.name`, `shipment.version`) and `format` come off the wire, and **nothing here
//! re-parses YAML** to recover them. What is still checked on disk is that the downloaded
//! artifact *is* a Shipment source tree — a manifest file exists — because a release of loose
//! `.wad` files would otherwise stage into a load-order entry that builds nothing.
//!
//! # Both catalogues, side by side
//!
//! The crash-reporting contract calls `catalog` "legacy-but-supported", and it has permanent
//! residents: genuinely third-party mods like `elishacloud/dxwrapper` will never carry a
//! Quartermaster manifest, so they can never appear here. The two are shown together and
//! labelled, never merged — their identities are not comparable, and an `id` from one namespace
//! placed next to an `id` from the other is a category error the UI must not invite.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::commands::installer::{download_bytes, extract_zip, stage_file_count};
use crate::commands::paths::{app_data_dir, downloading_dir, staging_dir};
use crate::commands::shipment::{has_manifest, ShipmentRef};
use crate::models::origin::Origin;

/// Where the registry lives when nothing overrides it (spec §1).
const DEFAULT_BASE_URL: &str = "https://mercs.ink";

/// Environment override for [`DEFAULT_BASE_URL`].
///
/// Its first job is testability — every test in this module points it at a loopback listener,
/// so the suite never touches the network — but it is equally the escape hatch for a staging
/// deployment or a self-hosted registry.
const BASE_URL_ENV: &str = "MERCS_INK_BASE_URL";

/// The highest Quartermaster manifest `format` this build can interpret.
///
/// Mirrors `mercs2_quartermaster::manifest::FORMAT_VERSION` and mercs.ink's own
/// `Manifest\Parser::FORMAT_VERSION`. A release declaring more than this is **refused**, not
/// attempted: house rule 3 on the registry side and "no silent no-ops" on this one agree that
/// installing something you cannot interpret is worse than failing.
pub const SUPPORTED_MANIFEST_FORMAT: u32 = 1;

/// How long a `429` is allowed to park an interactive request before we give up and answer from
/// cache instead. Spec §3 says to use `Retry-After` verbatim, and we do — up to this. A window
/// is a minute, so a longer value means something is wrong at the other end, and blocking a
/// click on it for minutes would be a worse answer than a stale catalogue and a banner.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Used when a `429` arrives without a parseable `Retry-After`.
const DEFAULT_BACKOFF: Duration = Duration::from_secs(5);

/// The configured registry root, without a trailing slash.
fn base_url() -> String {
    let raw = std::env::var(BASE_URL_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    raw.trim_end_matches('/').to_string()
}

/// Percent-encode one path segment. Spec §5.4 allows any non-`/` character in a version, so
/// `1.0/rc1` would otherwise silently address a different route.
fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn mod_url(slug: &str) -> String {
    format!("{}/api/v1/mods/{}", base_url(), encode_segment(slug))
}

fn release_url(slug: &str, version: &str) -> String {
    format!("{}/releases/{}", mod_url(slug), encode_segment(version))
}

// ---------------------------------------------------------------------------------------
// Wire types (spec §5, §6)
//
// Every struct here is a *head*: only the fields modkit uses. Spec §10 makes the surface
// additive-only and says clients must ignore unknown fields, which serde does by default —
// so a field added on the server is silently skipped rather than breaking every install.
// ---------------------------------------------------------------------------------------

/// One downloadable file on a release (spec §5.1, §8).
///
/// The server also sends `download_count`, which feeds mercs.ink's author dashboard and means
/// nothing here; it is left off deliberately rather than mirrored unused.
///
/// # There is no checksum on an asset, and none is invented
///
/// mercs.ink caches release *metadata* and never re-hosts the artifact (§8), so it has nothing
/// to attest to beyond what GitHub told it; `size` is not an integrity check. What is downloaded
/// through this module is trusted exactly as far as GitHub's TLS and the author's account are —
/// the same footing [`super::installer`] has always been on. A hash field nothing produces would
/// be worse than the honest absence.
///
/// **Do not reach for the manifest's digest to fill the gap.** Digests do exist in the API, just
/// not here: a parsed shipment's `load.requires` may carry `{ url, sha256 }` pinning an
/// *external* artifact. That is the **manifest author's** claim about a **third-party URL** —
/// not mercs.ink's claim about this GitHub release asset. They are different trust statements
/// about different bytes, and verifying one against the other would assert an integrity
/// guarantee nobody made.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    /// A GitHub release-asset URL. Followed directly and **without** an `Authorization` header
    /// (§8) — these are public downloads, and sending a token to a third-party host would leak
    /// it for nothing.
    pub download_url: String,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub content_type: Option<String>,
}

/// The head of a parsed Quartermaster manifest as the API serves it (spec §6).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManifestHead {
    /// Required by §6 and currently always `1`. `None` only if a deployment omits it.
    #[serde(default)]
    pub format: Option<u32>,
    #[serde(default)]
    pub shipment: ShipmentHead,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShipmentHead {
    /// `shipment.name` — the declared slug, and half an identity: every fork carries the same.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// `shipment.version`. This is the namespace the crash-reporting contract scopes a
    /// `registry` entry's version to — not the GitHub release tag, which is `catalog`'s.
    #[serde(default)]
    pub version: Option<String>,
    /// qm's `Target` — `retail` | `reimpl`.
    #[serde(default)]
    pub target: Option<String>,
}

/// One synced release of a registered mod (spec §5.3, §5.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryRelease {
    pub version: String,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub published_at: Option<String>,
    /// qm's `Target` — `retail` | `reimpl` | `both`.
    ///
    /// A **shipment compatibility** declaration, and *not* the crash report's `game.target`,
    /// which says what was actually running. They share a name and two of their values, which
    /// is exactly why the distinction is written down: a Shipment declaring compatibility with
    /// both can appear in a convoy whose `game.target` is `retail`. Carried through and
    /// displayed; never used to derive anything about the installed game.
    #[serde(default)]
    pub target: Option<String>,
    /// The manifest `format` mercs.ink parsed. Checked before anything is downloaded.
    #[serde(default)]
    pub format: Option<u32>,
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
    /// The full parsed manifest (§6). Identity and format are read from here rather than by
    /// re-parsing the YAML out of the downloaded artifact.
    #[serde(default)]
    pub manifest: Option<ManifestHead>,
}

/// One registered mod (spec §5.1, §5.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryMod {
    /// mercs.ink's stable public identifier — **opaque**. Copied verbatim into [`Origin::id`]
    /// and never parsed, split, or reconstructed. `None` against a deployment that predates
    /// the field (§12, v1.1), and recorded as `None` rather than guessed at.
    #[serde(default)]
    pub id: Option<String>,
    /// `shipment.name` from the manifest. Half an identity: every fork declares the same one.
    pub slug: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// qm's `Target`, as on [`RegistryRelease::target`] — shipment compatibility, not the game.
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    /// The GitHub repository the mod syncs from. Owner-derived, so it is display only — never
    /// an identity, because a rename or transfer changes it (§5.1).
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub latest_version: Option<String>,
    #[serde(default)]
    pub latest_release: Option<RegistryRelease>,
}

/// Spec §2: every 2xx is wrapped in `data`.
#[derive(Debug, Deserialize)]
struct Envelope<T> {
    data: T,
}

/// Spec §7: errors are **not** wrapped — they are a bare `{"message": …}`.
#[derive(Debug, Default, Deserialize)]
struct ApiError {
    #[serde(default)]
    message: Option<String>,
}

fn unwrap_envelope<T: serde::de::DeserializeOwned>(body: &str, what: &str) -> Result<T, String> {
    serde_json::from_str::<Envelope<T>>(body)
        .map(|e| e.data)
        .map_err(|e| format!("mercs.ink returned a {what} payload modkit could not read: {e}"))
}

/// Pull the server's own explanation out of an error body, falling back to the status code.
fn error_detail(status: u16, body: &str) -> String {
    let msg = serde_json::from_str::<ApiError>(body)
        .ok()
        .and_then(|e| e.message)
        .filter(|m| !m.trim().is_empty());
    match msg {
        Some(m) => format!("HTTP {status}: {m}"),
        None => format!("HTTP {status}"),
    }
}

// ---------------------------------------------------------------------------------------
// Conditional-GET cache, rate limiting, and the offline fallback
// ---------------------------------------------------------------------------------------

/// One cached response: the validator the server issued, and the body it validated.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CachedResponse {
    pub etag: String,
    pub body: String,
}

/// URL → last response. Small by construction: a handful of URL shapes, one row each.
type FetchCache = BTreeMap<String, CachedResponse>;

/// The cache file. One JSON object, rewritten whole — a few kilobytes of text, and a torn write
/// costs a re-fetch rather than anything a user would notice.
fn cache_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("mercsink-cache.json"))
}

// Read/write take an explicit path so the cache is unit-testable without the process-wide env
// vars `app_data_dir` resolves — the same shape `deploy_wad`'s ledger uses.

fn read_cache_at(path: &Path) -> FetchCache {
    // A cache that will not parse is a cache miss, never an error: the whole point of it is
    // that losing it costs one extra request.
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn write_cache_at(path: &Path, cache: &FetchCache) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(cache) {
        let _ = std::fs::write(path, text);
    }
}

/// A response body plus how much to trust it.
#[derive(Debug)]
struct Fetched {
    body: String,
    /// True when the body came out of the cache after the server could not be reached or
    /// answered 5xx/429 — spec §9 step 6. The caller shows a banner and carries on; it does
    /// **not** block, because a cached registry is still a usable registry.
    stale: bool,
    /// Why it is stale, in the user's terms. `None` when it isn't.
    warning: Option<String>,
}

fn retry_after(resp: &reqwest::Response) -> Duration {
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_BACKOFF)
}

/// Fetch `url`, revalidating against the stored ETag, honouring the rate limit, and falling
/// back to the cache when the server cannot answer.
///
/// The outcomes, in the order they are decided:
///
/// | Server says | What happens |
/// |---|---|
/// | `304` (§4) | The stored body is returned, fresh — the steady-state case |
/// | `2xx` | Cache is refreshed and the body returned |
/// | `429` (§3) | Sleep `Retry-After` (capped) and retry **once**; then fall back to cache |
/// | `5xx` (§7) | Fall back to cache; error only if there is nothing cached |
/// | network error | Fall back to cache; error only if there is nothing cached |
/// | any other 4xx | Error, carrying the server's own `message` — a `404` is a real answer |
///
/// A `404` deliberately does **not** fall back: "this slug does not exist" is information, and
/// serving a stale copy over it would hide a mod being taken down.
async fn fetch(client: &reqwest::Client, url: &str, cache_file: &Path) -> Result<Fetched, String> {
    let mut cache = read_cache_at(cache_file);
    let known = cache.get(url).cloned();

    // One retry, which is all a 429 gets: this is on an interactive path, and the second 429
    // means the bucket is genuinely exhausted rather than momentarily tight.
    let mut attempts = 0;
    loop {
        attempts += 1;

        let mut req = client.get(url);
        if let Some(hit) = &known {
            if !hit.etag.is_empty() {
                req = req.header(reqwest::header::IF_NONE_MATCH, hit.etag.clone());
            }
        }

        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                return match known {
                    Some(hit) => Ok(Fetched {
                        body: hit.body,
                        stale: true,
                        warning: Some(format!(
                            "Couldn't reach mercs.ink — showing the last copy modkit downloaded. ({e})"
                        )),
                    }),
                    None => Err(format!("Could not reach mercs.ink ({url}): {e}")),
                }
            }
        };

        let status = resp.status();

        if status == reqwest::StatusCode::NOT_MODIFIED {
            return match known {
                Some(hit) => Ok(Fetched { body: hit.body, stale: false, warning: None }),
                // Only sent when we hold a validator, so this is a misbehaving proxy rather
                // than the server. Read as a plain failure instead of unwrapping.
                None => Err(format!(
                    "mercs.ink answered 304 for {url} but modkit had nothing cached to serve"
                )),
            };
        }

        if status.is_success() {
            let etag = resp
                .headers()
                .get(reqwest::header::ETAG)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let body = resp
                .text()
                .await
                .map_err(|e| format!("Could not read the mercs.ink response for {url}: {e}"))?;

            // Only a validated body is worth storing; an ETag-less response is served straight
            // through so the next poll never sends `If-None-Match: ""`.
            if !etag.is_empty() {
                cache.insert(url.to_string(), CachedResponse { etag, body: body.clone() });
                write_cache_at(cache_file, &cache);
            }
            return Ok(Fetched { body, stale: false, warning: None });
        }

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS && attempts == 1 {
            let wait = retry_after(&resp).min(MAX_BACKOFF);
            tokio::time::sleep(wait).await;
            continue;
        }

        let code = status.as_u16();
        let body = resp.text().await.unwrap_or_default();
        let detail = error_detail(code, &body);

        let recoverable = status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
        return match (recoverable, known) {
            (true, Some(hit)) => Ok(Fetched {
                body: hit.body,
                stale: true,
                warning: Some(format!(
                    "mercs.ink answered {detail} — showing the last copy modkit downloaded."
                )),
            }),
            _ => Err(format!("mercs.ink returned {detail} for {url}")),
        };
    }
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("mercs2-modkit/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("Could not build an HTTP client: {e}"))
}

// ---------------------------------------------------------------------------------------
// The four read endpoints (spec §5)
// ---------------------------------------------------------------------------------------

/// A catalogue read, with the staleness the UI has to disclose.
///
/// The flag is part of the payload rather than a separate query because §9 step 6 requires both
/// halves at once: show the cached data *and* say it is cached. Returning only the data would
/// make "mercs.ink is down" indistinguishable from "nothing changed".
#[derive(Debug, Clone, Serialize)]
pub struct RegistryFeed {
    pub mods: Vec<RegistryMod>,
    /// True when `mods` came from the local cache because the server could not answer.
    pub stale: bool,
    /// A user-facing explanation to put in a banner. `None` when the fetch succeeded.
    pub warning: Option<String>,
}

/// Every mod on mercs.ink with a synced release (§5.1). Safe on every launch — §3.
#[tauri::command]
pub async fn fetch_mercsink_registry() -> Result<RegistryFeed, String> {
    let url = format!("{}/api/v1/registry", base_url());
    let got = fetch(&client()?, &url, &cache_path()?).await?;
    Ok(RegistryFeed {
        mods: unwrap_envelope(&got.body, "registry")?,
        stale: got.stale,
        warning: got.warning,
    })
}

/// One mod by slug (§5.2).
#[tauri::command]
pub async fn fetch_mercsink_mod(slug: String) -> Result<RegistryMod, String> {
    let got = fetch(&client()?, &mod_url(&slug), &cache_path()?).await?;
    unwrap_envelope(&got.body, "mod")
}

/// Every release of one mod, newest first (§5.3).
#[tauri::command]
pub async fn fetch_mercsink_releases(slug: String) -> Result<Vec<RegistryRelease>, String> {
    let url = format!("{}/releases", mod_url(&slug));
    let got = fetch(&client()?, &url, &cache_path()?).await?;
    unwrap_envelope(&got.body, "releases")
}

/// One release of one mod (§5.4).
#[tauri::command]
pub async fn fetch_mercsink_release(
    slug: String,
    version: String,
) -> Result<RegistryRelease, String> {
    let got = fetch(&client()?, &release_url(&slug, &version), &cache_path()?).await?;
    unwrap_envelope(&got.body, "release")
}

// ---------------------------------------------------------------------------------------
// Install
// ---------------------------------------------------------------------------------------

/// Refuse a manifest format this build cannot interpret.
///
/// `None` is not a refusal. §6 makes `format` required and currently always `1`, so an absent
/// value means a deployment older than the field rather than a manifest with no format; and
/// past this gate `qm build` runs with the real parser and is the authority on the schema. What
/// is refused is a *declared* format above ours — the one case where we know we would misread
/// the file, which is house rule 3 and "no silent no-ops" agreeing.
fn ensure_supported_format(declared: Option<u32>, what: &str) -> Result<(), String> {
    match declared {
        Some(f) if f > SUPPORTED_MANIFEST_FORMAT => Err(format!(
            "{what} declares Quartermaster manifest format {f}, and this version of modkit \
             understands only up to {SUPPORTED_MANIFEST_FORMAT}. Update modkit — installing it \
             anyway would mean building from a manifest modkit cannot read correctly."
        )),
        _ => Ok(()),
    }
}

/// The format a release declares, taking the stricter of the two places it appears.
///
/// `ReleaseResource.format` is the column mercs.ink recorded at sync time; `manifest.format` is
/// the value inside the manifest it serves. They should agree. Taking the maximum means a
/// disagreement fails closed rather than letting the lower of the two wave a release through.
fn declared_format(release: &RegistryRelease) -> Option<u32> {
    let inner = release.manifest.as_ref().and_then(|m| m.format);
    match (release.format, inner) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// Reduce a string to a filesystem-safe staging directory name.
///
/// Applied to the registry's opaque id purely as *sanitising*, never as parsing: the result
/// names a folder on this machine. The identity itself travels untouched in [`Origin::id`].
fn stage_name(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// The single archive to unpack, if the release has one.
///
/// Same rule [`super::installer`] applies to a catalog release, for the same reason: a Shipment
/// is a source *tree*, so it almost always arrives as one archive, and a release that also
/// carries a changelog or a screenshot must not stage those into the build.
fn pick_archive(assets: &[ReleaseAsset]) -> Option<&ReleaseAsset> {
    assets
        .iter()
        .find(|a| a.name.to_ascii_lowercase().ends_with(".zip"))
}

/// Find the Shipment root: the stage directory, or one level down (archives habitually wrap
/// everything in a folder named after the tag).
///
/// This tests for a manifest *file*, and does not read it — §6 already gave us the parsed
/// contents. It exists to catch the case where a release is not a Shipment source tree at all.
fn find_shipment_root(stage: &Path) -> Option<PathBuf> {
    if has_manifest(stage) {
        return Some(stage.to_path_buf());
    }
    let mut children: Vec<PathBuf> = std::fs::read_dir(stage)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    // Sorted, so a two-folder archive resolves to the same root on every machine rather than to
    // whatever `read_dir` happened to yield first.
    children.sort();
    children.into_iter().find(|p| has_manifest(p))
}

/// Blank a value that is present but empty — `name: ""` declares no more identity than no name.
fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// A Shipment installed from mercs.ink, ready for the load order.
#[derive(Debug, Clone, Serialize)]
pub struct MercsInkInstall {
    /// The load-order entry. Its `origin` is `registry` carrying the registry's opaque id —
    /// the point of the whole module.
    pub shipment: ShipmentRef,
    /// Registry slug. Display and lookup only; not an identity on its own.
    pub slug: String,
    pub title: Option<String>,
    /// The release that was installed, as the registry names it.
    pub release_version: String,
    /// qm's `Target` for this release — shipment compatibility, **not** `game.target`.
    pub target: Option<String>,
    /// Asset file names pulled from GitHub.
    pub assets: Vec<String>,
    pub staged_files: usize,
}

/// Install a Shipment from mercs.ink and record where it came from.
///
/// `version` picks a release; `None` takes the mod's latest (§9 steps 3–4). The mod resource is
/// fetched first even when the version is known, because that is where the opaque public
/// identifier lives — a release on its own cannot say which mod it belongs to.
#[tauri::command]
pub async fn install_mercsink_shipment(
    slug: String,
    version: Option<String>,
) -> Result<MercsInkInstall, String> {
    let client = client()?;
    let cache = cache_path()?;

    let item: RegistryMod =
        unwrap_envelope(&fetch(&client, &mod_url(&slug), &cache).await?.body, "mod")?;

    let wanted = version
        .as_deref()
        .map(str::to_string)
        .or_else(|| item.latest_version.clone())
        .or_else(|| item.latest_release.as_ref().map(|r| r.version.clone()))
        .ok_or_else(|| format!("{slug} has no released version on mercs.ink yet"))?;

    // §9 step 2: prefer what the mod resource already embedded. The common case — installing
    // the latest — then costs no extra round trip.
    let release = match &item.latest_release {
        Some(r) if r.version == wanted => r.clone(),
        _ => {
            let got = fetch(&client, &release_url(&slug, &wanted), &cache).await?;
            unwrap_envelope::<RegistryRelease>(&got.body, "release")?
        }
    };

    // Refuse before downloading a byte: the declared format is knowable from metadata alone, so
    // a release modkit cannot interpret should cost the user nothing.
    ensure_supported_format(
        declared_format(&release),
        &format!("{slug} {}", release.version),
    )?;

    if release.assets.is_empty() {
        return Err(format!(
            "{slug} {} has no downloadable assets on its GitHub release, so there is nothing to \
             install. (mercs.ink caches release metadata and never re-hosts artifacts.)",
            release.version
        ));
    }

    // Stage under the opaque id when there is one: two forks legitimately share a slug (§5.1),
    // and staging them under it would have one silently overwrite the other.
    let dir_key = format!(
        "mercsink-{}",
        stage_name(item.id.as_deref().unwrap_or(&item.slug))
    );
    let dl = downloading_dir()?.join(&dir_key);
    let stage = staging_dir()?.join(&dir_key);
    let _ = std::fs::remove_dir_all(&dl);
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&dl).map_err(|e| format!("Failed to create the download dir: {e}"))?;
    std::fs::create_dir_all(&stage).map_err(|e| format!("Failed to create the staging dir: {e}"))?;

    let mut staged_names = Vec::new();
    match pick_archive(&release.assets) {
        Some(a) => {
            let bytes = download_bytes(&client, &a.download_url).await?;
            let archive = dl.join(&a.name);
            std::fs::write(&archive, &bytes)
                .map_err(|e| format!("Failed to write {}: {e}", a.name))?;
            extract_zip(&archive, &stage)?;
            staged_names.push(a.name.clone());
        }
        None => {
            for a in &release.assets {
                let bytes = download_bytes(&client, &a.download_url).await?;
                std::fs::write(stage.join(&a.name), &bytes)
                    .map_err(|e| format!("Failed to write {}: {e}", a.name))?;
                staged_names.push(a.name.clone());
            }
        }
    }

    let Some(root) = find_shipment_root(&stage) else {
        let _ = std::fs::remove_dir_all(&stage);
        return Err(format!(
            "The assets of {slug} {} contain no manifest.yaml/.yml/.json/.toml, so this release \
             is not a Quartermaster Shipment source tree and modkit cannot build it. (A finished \
             vz-patch.wad goes through Import Patch WAD instead.)",
            release.version
        ));
    };

    // Identity comes off the wire (§6), already parsed. `item.slug` is itself `shipment.name`,
    // so the fallback is the same value from a different field rather than a guess.
    let head = release.manifest.clone().unwrap_or_default().shipment;
    let ship_slug = non_empty(head.name).unwrap_or_else(|| item.slug.clone());
    // The contract scopes a `registry` entry's version to the manifest's `shipment.version`;
    // the release version is the fallback for a manifest that declares none.
    let ship_version = non_empty(head.version).or_else(|| Some(release.version.clone()));
    let display = non_empty(item.title.clone())
        .or_else(|| non_empty(head.title))
        .unwrap_or_else(|| ship_slug.clone());

    let shipment = ShipmentRef {
        // Folder-derived like every other Shipment row: this is the load order's dedupe key and
        // becomes a `ClaimGroup::mod_id`, so it has to be per-checkout. The identity lives in
        // `slug` and `origin`, which is what leaves the machine.
        id: format!("shipment:{dir_key}"),
        name: display,
        path: root.to_string_lossy().to_string(),
        slug: Some(ship_slug),
        version: ship_version.clone(),
        // The whole exercise: an entry installed this way records `registry` and the registry's
        // own precomposed identifier, moved across untouched. Absent on the server → `None`.
        origin: Origin::registry(item.id.clone(), ship_version),
    };

    Ok(MercsInkInstall {
        staged_files: stage_file_count(&root),
        shipment,
        slug: item.slug,
        title: item.title,
        release_version: release.version,
        target: release.target.or(item.target),
        assets: staged_names,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// `std::env::set_var` is process-wide, so the env-reading tests take a lock rather than
    /// racing each other under the harness's thread pool.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The override is what keeps this suite off the network, so its precedence is pinned
    /// rather than assumed.
    #[test]
    fn base_url_prefers_the_env_override_and_drops_a_trailing_slash() {
        let _g = env_lock();
        std::env::remove_var(BASE_URL_ENV);
        assert_eq!(base_url(), DEFAULT_BASE_URL);

        std::env::set_var(BASE_URL_ENV, "http://127.0.0.1:9/");
        assert_eq!(base_url(), "http://127.0.0.1:9");

        // A blank override is not an override — it would produce a bare `/api/v1/registry`.
        std::env::set_var(BASE_URL_ENV, "   ");
        assert_eq!(base_url(), DEFAULT_BASE_URL);
        std::env::remove_var(BASE_URL_ENV);
    }

    /// §5.4 allows any non-`/` character in a version, and a version is author-chosen text.
    #[test]
    fn path_segments_are_percent_encoded() {
        let _g = env_lock();
        std::env::set_var(BASE_URL_ENV, "http://example.test");
        assert_eq!(mod_url("a b"), "http://example.test/api/v1/mods/a%20b");
        assert_eq!(
            release_url("x", "v1.0/rc1"),
            "http://example.test/api/v1/mods/x/releases/v1.0%2Frc1"
        );
        std::env::remove_var(BASE_URL_ENV);
    }

    /// House rule 3, decided from metadata alone so a refusal costs no download.
    #[test]
    fn a_future_manifest_format_is_refused_loudly() {
        let err =
            ensure_supported_format(Some(SUPPORTED_MANIFEST_FORMAT + 1), "x 1.0").unwrap_err();
        assert!(err.contains("understands only up to"), "got: {err}");
        assert!(ensure_supported_format(Some(SUPPORTED_MANIFEST_FORMAT), "x").is_ok());
        // Unknown is qm's to reject with the real parser, not ours to guess at.
        assert!(ensure_supported_format(None, "x").is_ok());
    }

    /// The column and the served manifest should agree; if they don't, the higher one decides,
    /// so a disagreement fails closed.
    #[test]
    fn a_disagreeing_format_fails_closed() {
        let rel = |col: Option<u32>, inner: Option<u32>| RegistryRelease {
            version: "1".into(),
            tag: None,
            published_at: None,
            target: None,
            format: col,
            assets: Vec::new(),
            manifest: inner.map(|f| ManifestHead { format: Some(f), ..Default::default() }),
        };
        assert_eq!(declared_format(&rel(Some(1), Some(9))), Some(9));
        assert_eq!(declared_format(&rel(Some(9), Some(1))), Some(9));
        assert_eq!(declared_format(&rel(None, Some(2))), Some(2));
        assert_eq!(declared_format(&rel(None, None)), None);
    }

    /// The exact shape §5.1 documents, envelope and all, including the opaque `id` and the
    /// parsed manifest that removes any need to re-read YAML.
    #[test]
    fn a_registry_payload_deserializes() {
        let body = r#"{"data":[{
            "id":"vehicle-pack-486521234",
            "slug":"vehicle-pack",
            "title":"Vehicle Pack",
            "description":"Adds new vehicles to Maracaibo",
            "target":"retail",
            "tags":["vehicles"],
            "authors":["octocat"],
            "homepage":null,
            "license":"MIT",
            "repository":"https://github.com/octocat/vehicle-pack",
            "latest_version":"1.0.0",
            "latest_release":{
                "version":"1.0.0","tag":"v1.0.0","published_at":"2026-08-04T15:22:00+00:00",
                "target":"retail","format":1,
                "assets":[{"name":"vehicle-pack.zip","download_url":"https://github.com/octocat/vehicle-pack/releases/download/v1.0.0/vehicle-pack.zip","size":12345,"content_type":"application/octet-stream","download_count":42}],
                "manifest":{"format":1,"shipment":{"name":"vehicle-pack","version":"1.0.0","target":"retail"},"load":{},"contributions":[]}
            }
        }]}"#;
        let mods: Vec<RegistryMod> = unwrap_envelope(body, "registry").unwrap();
        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].id.as_deref(), Some("vehicle-pack-486521234"));
        let rel = mods[0].latest_release.as_ref().unwrap();
        assert_eq!(declared_format(rel), Some(1));
        assert_eq!(
            rel.manifest.as_ref().unwrap().shipment.version.as_deref(),
            Some("1.0.0")
        );
        // The wire carries five asset fields; `download_count` feeds mercs.ink's author
        // dashboard and is skipped rather than mirrored unused. No checksum exists to read on
        // an asset, and none is synthesized (§8).
        assert_eq!(rel.assets[0].size, Some(12345));
        assert_eq!(rel.assets[0].content_type.as_deref(), Some("application/octet-stream"));
    }

    /// §10: the surface is additive-only and unknown keys must be ignored, never an error.
    #[test]
    fn unknown_fields_are_ignored() {
        let body = r#"{"data":{"slug":"a","id":"a-1","downloads_this_week":9,
            "latest_release":{"version":"1","brand_new_field":{"x":1},"assets":[]}}}"#;
        let item: RegistryMod = unwrap_envelope(body, "mod").unwrap();
        assert_eq!(item.id.as_deref(), Some("a-1"));
        assert_eq!(item.latest_release.unwrap().version, "1");
    }

    /// A deployment predating §12 v1.1 must produce `id: None`, not a value modkit composed for
    /// itself. This is precisely the drift the contract exists to prevent.
    #[test]
    fn a_missing_identifier_stays_missing() {
        let body = r#"{"data":{"slug":"vehicle-pack","latest_version":"1.0.0"}}"#;
        let item: RegistryMod = unwrap_envelope(body, "mod").unwrap();
        assert_eq!(item.id, None);

        let origin = Origin::registry(item.id.clone(), Some("1.0.0".into()));
        assert_eq!(origin.source, crate::models::origin::OriginSource::Registry);
        assert_eq!(origin.id, None, "no id must never become a guessed id");
    }

    /// A registry install carries the server's string byte for byte.
    #[test]
    fn the_identifier_is_copied_verbatim() {
        let item: RegistryMod =
            unwrap_envelope(r#"{"data":{"id":"a-b-c-1","slug":"a-b-c"}}"#, "mod").unwrap();
        assert_eq!(Origin::registry(item.id, None).id.as_deref(), Some("a-b-c-1"));
    }

    /// §7: an error body is unwrapped `{"message": …}`, and the server's own words are what the
    /// user should see.
    #[test]
    fn an_error_body_is_read_unwrapped() {
        assert_eq!(
            error_detail(404, r#"{"message":"No query results for model [App\\Models\\Mod] ghost"}"#),
            "HTTP 404: No query results for model [App\\Models\\Mod] ghost"
        );
        // A body that is not the documented shape still yields the status rather than nothing.
        assert_eq!(error_detail(500, "<html>oops</html>"), "HTTP 500");
    }

    #[test]
    fn a_single_zip_wins_over_loose_assets() {
        let asset = |n: &str| ReleaseAsset {
            name: n.into(),
            download_url: "u".into(),
            size: None,
            content_type: None,
        };
        let assets = vec![asset("README.md"), asset("Vehicle-Pack.ZIP")];
        assert_eq!(
            pick_archive(&assets).map(|a| a.name.as_str()),
            Some("Vehicle-Pack.ZIP")
        );
        assert!(pick_archive(&assets[..1]).is_none());
    }

    /// The staging key sanitises the opaque id into a folder name without ever parsing it.
    #[test]
    fn stage_names_are_filesystem_safe() {
        assert_eq!(stage_name("vehicle-pack-486521234"), "vehicle-pack-486521234");
        assert_eq!(stage_name("../../etc/passwd"), "etc-passwd");
        assert_eq!(stage_name("A B"), "a-b");
    }

    #[test]
    fn a_shipment_root_is_found_at_the_top_or_one_level_down() {
        let top = tempfile::tempdir().unwrap();
        std::fs::write(top.path().join("manifest.yaml"), "shipment:\n  name: a\n").unwrap();
        assert_eq!(find_shipment_root(top.path()).unwrap(), top.path());

        let wrapped = tempfile::tempdir().unwrap();
        let inner = wrapped.path().join("vehicle-pack-1.0.0");
        std::fs::create_dir(&inner).unwrap();
        std::fs::write(inner.join("manifest.json"), r#"{"shipment":{"name":"a"}}"#).unwrap();
        assert_eq!(find_shipment_root(wrapped.path()).unwrap(), inner);

        // A release of loose WAD files is not a Shipment source tree, and must say so rather
        // than staging an entry that would build nothing.
        let bare = tempfile::tempdir().unwrap();
        std::fs::write(bare.path().join("vz-patch.wad"), b"x").unwrap();
        assert!(find_shipment_root(bare.path()).is_none());
    }

    #[test]
    fn the_cache_round_trips_and_a_corrupt_file_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mercsink-cache.json");
        assert!(read_cache_at(&path).is_empty(), "no file is an empty cache");

        let mut c = FetchCache::new();
        c.insert(
            "https://mercs.ink/api/v1/registry".into(),
            CachedResponse { etag: "\"abc\"".into(), body: "{\"data\":[]}".into() },
        );
        write_cache_at(&path, &c);
        assert_eq!(read_cache_at(&path)["https://mercs.ink/api/v1/registry"].etag, "\"abc\"");

        std::fs::write(&path, "{ not json").unwrap();
        assert!(read_cache_at(&path).is_empty(), "a corrupt cache is a miss, not an error");
    }

    // ------------------------------------------------------------------------------------
    // HTTP behaviour, against a loopback listener. No internet is touched.
    // ------------------------------------------------------------------------------------

    /// A one-shot HTTP/1.1 server answering `n` requests from a canned script, recording each
    /// request's `If-None-Match`. Deliberately minimal: enough for reqwest, no more.
    fn serve(responses: Vec<String>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for body in responses {
                let (mut sock, _) = listener.accept().unwrap();
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let inm = req
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("if-none-match:"))
                    .map(|l| l[l.find(':').unwrap() + 1..].trim().to_string())
                    .unwrap_or_default();
                seen.push(inm);
                let _ = sock.write_all(body.as_bytes());
                let _ = sock.flush();
            }
            seen
        });
        (format!("http://{addr}"), handle)
    }

    fn ok_with_etag(etag: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nETag: {etag}\r\nCache-Control: max-age=60, public\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn status_only(line: &str, extra: &str) -> String {
        format!("HTTP/1.1 {line}\r\n{extra}Content-Length: 0\r\nConnection: close\r\n\r\n")
    }

    /// An error response in §7's unwrapped `{"message": …}` shape.
    fn error_body(line: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    /// §4, the steady state: the first call stores the validator, the second sends it back and
    /// is answered `304` with an empty body — and the caller still gets the payload, fresh, out
    /// of the cache.
    #[test]
    fn a_second_fetch_revalidates_and_is_served_from_cache() {
        let body = r#"{"data":[{"id":"a-1","slug":"a"}]}"#;
        let (base, handle) = serve(vec![
            ok_with_etag("\"v1\"", body),
            status_only("304 Not Modified", "ETag: \"v1\"\r\n"),
        ]);

        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache.json");
        let url = format!("{base}/api/v1/registry");
        let rt = rt();
        let c = client().unwrap();

        let first = rt.block_on(fetch(&c, &url, &cache)).unwrap();
        assert_eq!(first.body, body);
        assert!(!first.stale);

        let second = rt.block_on(fetch(&c, &url, &cache)).unwrap();
        assert_eq!(second.body, body, "a 304 must still yield the payload");
        assert!(!second.stale, "revalidated is fresh, not stale");

        let seen = handle.join().unwrap();
        assert_eq!(seen[0], "", "nothing to revalidate on the first request");
        assert_eq!(seen[1], "\"v1\"", "the stored validator is sent back");

        let mods: Vec<RegistryMod> = unwrap_envelope(&second.body, "registry").unwrap();
        assert_eq!(mods[0].id.as_deref(), Some("a-1"));
    }

    /// §9 step 6: a 5xx falls back to the cache and says so, rather than blocking the user out
    /// of a catalogue they already have.
    #[test]
    fn a_server_error_falls_back_to_cache_with_a_warning() {
        let body = r#"{"data":[{"id":"a-1","slug":"a"}]}"#;
        let (base, handle) = serve(vec![
            ok_with_etag("\"v1\"", body),
            status_only("500 Internal Server Error", ""),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache.json");
        let url = format!("{base}/api/v1/registry");
        let rt = rt();
        let c = client().unwrap();

        rt.block_on(fetch(&c, &url, &cache)).unwrap();
        let got = rt.block_on(fetch(&c, &url, &cache)).unwrap();
        assert_eq!(got.body, body);
        assert!(got.stale, "a cached answer to a 500 is stale");
        assert!(got.warning.unwrap().contains("500"));
        let _ = handle.join();
    }

    /// With nothing cached there is nothing to fall back to, and a 5xx must be an error rather
    /// than a silently empty catalogue.
    #[test]
    fn a_server_error_with_no_cache_is_an_error() {
        let (base, handle) = serve(vec![status_only("503 Service Unavailable", "")]);
        let dir = tempfile::tempdir().unwrap();
        let err = rt()
            .block_on(fetch(
                &client().unwrap(),
                &format!("{base}/api/v1/registry"),
                &dir.path().join("c.json"),
            ))
            .unwrap_err();
        assert!(err.contains("503"), "got: {err}");
        let _ = handle.join();
    }

    /// §3: a 429 is honoured with `Retry-After` and retried once. `Retry-After: 0` keeps the
    /// test instant while still exercising the header path.
    #[test]
    fn a_rate_limit_is_retried_after_the_servers_own_delay() {
        let body = r#"{"data":[]}"#;
        let (base, handle) = serve(vec![
            status_only("429 Too Many Requests", "Retry-After: 0\r\n"),
            ok_with_etag("\"v1\"", body),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let got = rt()
            .block_on(fetch(
                &client().unwrap(),
                &format!("{base}/api/v1/registry"),
                &dir.path().join("c.json"),
            ))
            .unwrap();
        assert_eq!(got.body, body, "the retry's payload is what comes back");
        assert!(!got.stale);
        let _ = handle.join();
    }

    /// §7: a 404 is a real answer — the slug does not exist — so it must **not** be papered
    /// over with a stale copy, and it must carry the server's own message.
    #[test]
    fn a_404_is_reported_and_never_served_from_cache() {
        let body = r#"{"data":{"slug":"ghost"}}"#;
        let (base, handle) = serve(vec![
            ok_with_etag("\"v1\"", body),
            error_body("404 Not Found", r#"{"message":"Not found."}"#),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache.json");
        let url = format!("{base}/api/v1/mods/ghost");
        let rt = rt();
        let c = client().unwrap();

        rt.block_on(fetch(&c, &url, &cache)).unwrap();
        let err = rt.block_on(fetch(&c, &url, &cache)).unwrap_err();
        assert!(err.contains("404"), "got: {err}");
        assert!(err.contains("Not found."), "the server's own words: {err}");
        let _ = handle.join();
    }
}
