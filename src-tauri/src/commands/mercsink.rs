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

use tauri::Window;

use crate::commands::dependencies::{
    installed_from_plan, relayed_shipment_needs, resolve, Action, Candidate, Step,
};
use crate::commands::incompatibility::{parse_index, IncompatibilityIndex, ListState, LoadedList};
use crate::commands::installer::{download_bytes, extract_zip, stage_file_count};
use crate::commands::managed::ledger::now_unix;
use crate::commands::managed::place::sha256_hex;
use crate::commands::net;
use crate::commands::paths::{app_data_dir, downloading_dir, staging_dir};
use crate::commands::shipment::{
    has_manifest, preflight_rows, InstallReason, ShipmentRef, MANIFEST_NAMES,
};
use crate::models::origin::Origin;

/// Where the registry lives when nothing overrides it (spec §1).
const DEFAULT_BASE_URL: &str = "https://mercs.ink";

/// Environment override for [`DEFAULT_BASE_URL`].
///
/// Its first job is testability — every test in this module points it at a loopback listener,
/// so the suite never touches the network — but it is equally the escape hatch for a staging
/// deployment or a self-hosted registry.
const BASE_URL_ENV: &str = "MERCS_INK_BASE_URL";

/// The one Quartermaster manifest `format` this build installs.
///
/// Mirrors `mercs2_quartermaster::manifest::FORMAT_VERSION`. Format 2 is the only manifest format:
/// a release declaring any other value — 1 included — is **refused** before anything
/// is downloaded, because installing something qm 3 will reject is worse than failing.
pub const SUPPORTED_MANIFEST_FORMAT: u32 = 2;

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
/// # The digest comes from GitHub, not from here
///
/// mercs.ink caches release *metadata* and never re-hosts the artifact, so it relays no
/// checksum; `size` is not an integrity check. The chosen Shipment zip is verified against the
/// asset `digest` GitHub itself publishes, looked up by tag: see
/// [`verify_asset_digest`].
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
    /// Required by the mercs.ink API. `None` only if a deployment omits it, which the install
    /// gate refuses.
    #[serde(default)]
    pub format: Option<u32>,
    #[serde(default)]
    pub shipment: ShipmentHead,
    /// The `load` table, kept as JSON. Only the resolver reads it, and only for the releases it
    /// is resolving (see [`super::dependencies::relayed_shipment_needs`]), so one release with a
    /// malformed table fails that install rather than the whole catalogue.
    #[serde(default)]
    pub load: Option<serde_json::Value>,
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
    /// The GitHub repository the mod syncs from, exactly `https://github.com/{owner}/{repo}`.
    /// Never an identity, because a rename or transfer changes it. It **addresses GitHub**
    /// for the digest lookup, and any other form is refused there. mercs.ink
    /// documents this form and refreshes it on every sync.
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

/// Why mercs.ink could not answer a conditional GET. Both are recoverable: a client holding a
/// cached copy uses it instead.
#[derive(Debug)]
enum Unreachable {
    /// The request never got an HTTP answer.
    Network(String),
    /// A `5xx`, or a `429` that was still a `429` after one retry. Carries [`error_detail`].
    Answered(String),
}

impl Unreachable {
    /// Why, as a clause that reads inside parentheses.
    fn reason(&self) -> String {
        match self {
            Unreachable::Network(e) => format!("could not connect: {e}"),
            Unreachable::Answered(detail) => format!("it answered {detail}"),
        }
    }
}

/// What one conditional GET established.
#[derive(Debug)]
enum Revalidated {
    /// A `2xx` with a new body. `etag` is empty when the server sent none.
    Fresh { etag: String, body: String },
    /// A `304`: the body behind the validator sent is still current.
    NotModified,
    Unreachable(Unreachable),
}

/// GET `url`, sending `known_etag` as `If-None-Match`, and honouring the rate limit.
///
/// | Server says | Result |
/// |---|---|
/// | `304` | [`Revalidated::NotModified`] |
/// | `2xx` | [`Revalidated::Fresh`] |
/// | `429` | Sleep `Retry-After` (capped) and retry **once**; a second `429` is unreachable |
/// | `5xx` | [`Unreachable::Answered`] |
/// | network error | [`Unreachable::Network`] |
/// | any other 4xx | `Err`, carrying the server's own `message` — a `404` is a real answer |
///
/// Deciding what a cached copy is worth in each case is the caller's business.
async fn revalidate(
    client: &reqwest::Client,
    url: &str,
    known_etag: Option<&str>,
) -> Result<Revalidated, String> {
    // One retry, which is all a 429 gets: this is on an interactive path, and the second 429
    // means the bucket is genuinely exhausted rather than momentarily tight.
    let mut attempts = 0;
    loop {
        attempts += 1;

        let mut req = client.get(url);
        if let Some(etag) = known_etag.filter(|e| !e.is_empty()) {
            req = req.header(reqwest::header::IF_NONE_MATCH, etag);
        }

        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => return Ok(Revalidated::Unreachable(Unreachable::Network(e.to_string()))),
        };

        let status = resp.status();

        if status == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(Revalidated::NotModified);
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
            return Ok(Revalidated::Fresh { etag, body });
        }

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS && attempts == 1 {
            let wait = retry_after(&resp).min(MAX_BACKOFF);
            tokio::time::sleep(wait).await;
            continue;
        }

        let code = status.as_u16();
        let body = resp.text().await.unwrap_or_default();
        let detail = error_detail(code, &body);

        if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Ok(Revalidated::Unreachable(Unreachable::Answered(detail)));
        }
        return Err(format!("mercs.ink returned {detail} for {url}"));
    }
}

/// Fetch `url`, revalidating against the stored ETag, honouring the rate limit, and falling
/// back to the cache when the server cannot answer.
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

    match revalidate(client, url, known.as_ref().map(|k| k.etag.as_str())).await? {
        Revalidated::NotModified => match known {
            Some(hit) => Ok(Fetched { body: hit.body, stale: false, warning: None }),
            // Only sent when we hold a validator, so this is a misbehaving proxy rather
            // than the server. Read as a plain failure instead of unwrapping.
            None => Err(format!(
                "mercs.ink answered 304 for {url} but modkit had nothing cached to serve"
            )),
        },
        Revalidated::Fresh { etag, body } => {
            // Only a validated body is worth storing; an ETag-less response is served straight
            // through so the next poll never sends `If-None-Match: ""`.
            if !etag.is_empty() {
                cache.insert(url.to_string(), CachedResponse { etag, body: body.clone() });
                write_cache_at(cache_file, &cache);
            }
            Ok(Fetched { body, stale: false, warning: None })
        }
        Revalidated::Unreachable(why) => match (why, known) {
            (Unreachable::Network(e), Some(hit)) => Ok(Fetched {
                body: hit.body,
                stale: true,
                warning: Some(format!(
                    "Couldn't reach mercs.ink — showing the last copy modkit downloaded. ({e})"
                )),
            }),
            (Unreachable::Network(e), None) => Err(format!("Could not reach mercs.ink ({url}): {e}")),
            (Unreachable::Answered(detail), Some(hit)) => Ok(Fetched {
                body: hit.body,
                stale: true,
                warning: Some(format!(
                    "mercs.ink answered {detail} — showing the last copy modkit downloaded."
                )),
            }),
            (Unreachable::Answered(detail), None) => {
                Err(format!("mercs.ink returned {detail} for {url}"))
            }
        },
    }
}

/// The shared client. The conditional-request bookkeeping above stays here
/// because it is mercs.ink's spec, not a general HTTP concern — but the transport
/// underneath it (user agent, timeouts, connection pool) is everyone's.
fn client() -> Result<reqwest::Client, String> {
    crate::commands::net::client()
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
// The community incompatibility list
//
// Cached apart from `FetchCache`, and strictly. That cache treats a bad file as a miss, which
// is right for a catalogue a user browses. This list decides whether a build is refused, so a
// cache that can't be read is an error, and a cached copy records when it was fetched: when
// mercs.ink can't be reached, the build says how old the list it checked against is.
// ---------------------------------------------------------------------------------------

/// One cached copy of the incompatibility list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CachedList {
    etag: String,
    body: String,
    /// Seconds since the epoch of the last `200` or `304`. mercs.ink's own `generated_at` can be
    /// served from its response cache, so it is not the list's age here.
    fetched_at: u64,
}

/// URL → the list last fetched from it.
type ListCache = BTreeMap<String, CachedList>;

fn incompatibility_cache_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("mercsink-incompatibilities.json"))
}

/// No file is an empty cache. A file that exists but can't be read is an error naming it.
fn read_list_cache(path: &Path) -> Result<ListCache, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ListCache::new()),
        Err(e) => {
            return Err(format!(
                "Could not read the cached incompatibility list at {}: {e}",
                path.display()
            ))
        }
    };
    serde_json::from_str(&text).map_err(|e| {
        format!(
            "The cached incompatibility list at {} is not in the form Modkit writes, so nothing \
             was built: {e}",
            path.display()
        )
    })
}

fn write_list_cache(path: &Path, cache: &ListCache) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string(cache)
        .map_err(|e| format!("Could not serialize the incompatibility list cache: {e}"))?;
    std::fs::write(path, text)
        .map_err(|e| format!("Could not write the incompatibility list cache {}: {e}", path.display()))
}

/// Parse a list mercs.ink just sent. A list it can't use refuses the build, and is never
/// cached.
fn parse_fresh_list(body: &str) -> Result<IncompatibilityIndex, String> {
    parse_index(body).map_err(|e| {
        format!("mercs.ink sent an incompatibility list Modkit cannot use, so nothing was built:\n{e}")
    })
}

/// Parse the cached copy for `url`.
fn parse_cached_list(hit: &CachedList, cache_file: &Path) -> Result<IncompatibilityIndex, String> {
    parse_index(&hit.body).map_err(|e| {
        format!(
            "The incompatibility list cached at {} cannot be used, so nothing was built:\n{e}",
            cache_file.display()
        )
    })
}

/// Fetch the incompatibility list at `url`, keeping the cache in `cache_file` current. `now` is
/// seconds since the epoch.
///
/// | mercs.ink | Cached copy | Result |
/// |---|---|---|
/// | `200` | any | parsed, then cached with `fetched_at = now`; current |
/// | `304` | yes | re-parsed, `fetched_at = now`; current |
/// | unreachable | yes | the cached copy, with its age |
/// | unreachable | no | no list: the build goes ahead and says it was not checked |
/// | any other 4xx | any | error |
///
/// A body is parsed before it is cached, so a list Modkit can't use never replaces one it can.
async fn load_incompatibilities(
    client: &reqwest::Client,
    url: &str,
    cache_file: &Path,
    now: u64,
) -> Result<LoadedList, String> {
    let mut cache = read_list_cache(cache_file)?;
    let known = cache.get(url).cloned();

    match revalidate(client, url, known.as_ref().map(|k| k.etag.as_str())).await? {
        Revalidated::Fresh { etag, body } => {
            let index = parse_fresh_list(&body)?;
            cache.insert(url.to_string(), CachedList { etag, body, fetched_at: now });
            write_list_cache(cache_file, &cache)?;
            Ok(LoadedList { state: ListState::Current { fetched_at: now }, index: Some(index) })
        }
        Revalidated::NotModified => {
            let Some(hit) = known else {
                return Err(format!(
                    "mercs.ink answered 304 for {url} but modkit had nothing cached to serve"
                ));
            };
            let index = parse_cached_list(&hit, cache_file)?;
            cache.insert(url.to_string(), CachedList { fetched_at: now, ..hit });
            write_list_cache(cache_file, &cache)?;
            Ok(LoadedList { state: ListState::Current { fetched_at: now }, index: Some(index) })
        }
        Revalidated::Unreachable(why) => match known {
            Some(hit) => {
                let index = parse_cached_list(&hit, cache_file)?;
                let state =
                    ListState::cached(hit.fetched_at, index.generated_at.clone(), why.reason(), now);
                Ok(LoadedList { state, index: Some(index) })
            }
            None => Ok(LoadedList { state: ListState::never_fetched(why.reason()), index: None }),
        },
    }
}

/// The incompatibility list a Shipment build is checked against.
pub(crate) async fn load_incompatibility_list() -> Result<LoadedList, String> {
    let url = format!("{}/api/v1/incompatibilities", base_url());
    load_incompatibilities(&client()?, &url, &incompatibility_cache_path()?, now_unix()).await
}

// ---------------------------------------------------------------------------------------
// Install
// ---------------------------------------------------------------------------------------

/// Refuse any manifest format other than [`SUPPORTED_MANIFEST_FORMAT`].
///
/// Decided from metadata alone, so a refusal costs no download. `None` is refused too: the
/// mercs.ink API makes `format` required, and a release that does not say which format it is
/// cannot be shown to be one qm 3 reads.
fn ensure_supported_format(declared: Option<u32>, what: &str) -> Result<(), String> {
    match declared {
        Some(SUPPORTED_MANIFEST_FORMAT) => Ok(()),
        Some(f) => Err(format!(
            "{what} declares Quartermaster manifest format {f}. Modkit installs only format \
             {SUPPORTED_MANIFEST_FORMAT}, the only format Quartermaster accepts; the Shipment's \
             author needs to publish a format-{SUPPORTED_MANIFEST_FORMAT} release."
        )),
        None => Err(format!(
            "{what} declares no Quartermaster manifest format, so Modkit cannot tell whether \
             Quartermaster can read it. Refusing rather than guessing."
        )),
    }
}

/// The format a release declares, from the two places it appears.
///
/// `ReleaseResource.format` is the column mercs.ink recorded at sync time; `manifest.format` is
/// the value inside the manifest it serves. When both are present they must agree: a release
/// whose own metadata contradicts itself is refused rather than read either way.
fn declared_format(release: &RegistryRelease) -> Result<Option<u32>, String> {
    let inner = release.manifest.as_ref().and_then(|m| m.format);
    match (release.format, inner) {
        (Some(a), Some(b)) if a != b => Err(format!(
            "mercs.ink records manifest format {a} for release {} but serves a manifest declaring \
             format {b}; refusing a release whose format contradicts itself",
            release.version
        )),
        (a, b) => Ok(a.or(b)),
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

/// The staging key of a registry mod. Staged under the opaque id when there is one: two forks
/// legitimately share a slug, and staging them under it would have one silently overwrite
/// the other.
fn stage_key(item: &RegistryMod) -> String {
    format!("mercsink-{}", stage_name(item.id.as_deref().unwrap_or(&item.slug)))
}

// ---------------------------------------------------------------------------------------
// Release-asset selection
// ---------------------------------------------------------------------------------------

/// The asset name selection takes without opening anything:
/// `<shipment_name>-v<version>.zip`, with a leading `v` on the release version stripped so it
/// is never doubled.
fn expected_zip_name(shipment_name: &str, release_version: &str) -> String {
    let v = release_version.strip_prefix('v').unwrap_or(release_version);
    format!("{shipment_name}-v{v}.zip")
}

/// What selection does next.
#[derive(Debug)]
enum ArchiveChoice<'a> {
    /// Exactly one asset has the expected name: take it, and download or open no other.
    Named(&'a ReleaseAsset),
    /// Otherwise: download and open every zip, and take the one holding a manifest.
    Inspect(Vec<&'a ReleaseAsset>),
}

/// Decide from the asset list alone. No `.zip` at all is an error: there is no loose-asset
/// install.
fn plan_archive_choice<'a>(
    assets: &'a [ReleaseAsset],
    expected: &str,
    what: &str,
) -> Result<ArchiveChoice<'a>, String> {
    let zips: Vec<&ReleaseAsset> = assets
        .iter()
        .filter(|a| a.name.to_ascii_lowercase().ends_with(".zip"))
        .collect();
    if zips.is_empty() {
        let have = if assets.is_empty() {
            "no assets at all".to_string()
        } else {
            assets.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")
        };
        return Err(format!(
            "{what} has no Shipment zip: its GitHub release carries {have}. A Shipment installs \
             only from a .zip holding its manifest. (mercs.ink relays GitHub release metadata \
             and never re-hosts artifacts.)"
        ));
    }
    let named: Vec<&ReleaseAsset> = zips.iter().copied().filter(|a| a.name == expected).collect();
    Ok(match named.as_slice() {
        [one] => ArchiveChoice::Named(one),
        _ => ArchiveChoice::Inspect(zips),
    })
}

/// Does this zip hold a manifest qm would read, at its root or one folder down? The same depth
/// [`find_shipment_root`] looks at after extraction. An unreadable zip is an error, not a "no".
fn zip_contains_manifest(bytes: &[u8]) -> Result<bool, String> {
    let zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("not a readable zip ({e})"))?;
    let found = zip.file_names().any(|name| {
        let n = name.replace('\\', "/");
        match n.split('/').collect::<Vec<_>>().as_slice() {
            [file] | [_, file] => MANIFEST_NAMES.contains(file),
            _ => false,
        }
    });
    Ok(found)
}

/// Of the opened zips, the one holding a manifest. None or several is an error that lists every
/// zip and what was found in it.
fn choose_opened(
    opened: Vec<(ReleaseAsset, Vec<u8>)>,
    what: &str,
    expected: &str,
) -> Result<(ReleaseAsset, Vec<u8>), String> {
    let mut qualifying = Vec::new();
    let mut report = Vec::new();
    for (asset, bytes) in opened {
        match zip_contains_manifest(&bytes) {
            Ok(true) => {
                report.push(format!("{}: holds a manifest", asset.name));
                qualifying.push((asset, bytes));
            }
            Ok(false) => report.push(format!(
                "{}: no manifest.yaml/.yml/.json/.toml at its root or one folder down",
                asset.name
            )),
            Err(e) => report.push(format!("{}: {e}", asset.name)),
        }
    }
    match qualifying.len() {
        1 => Ok(qualifying.pop().expect("one element")),
        0 => Err(format!(
            "{what} has no zip named {expected} and none of its zips is a Shipment:\n{}",
            report.join("\n")
        )),
        _ => Err(format!(
            "{what} has no zip named {expected} and several of its zips hold a manifest, so \
             Modkit cannot tell which is the Shipment:\n{}",
            report.join("\n")
        )),
    }
}

// ---------------------------------------------------------------------------------------
// Digest verification
// ---------------------------------------------------------------------------------------

/// `owner/repo` from the relayed `repository`, which must be exactly
/// `https://github.com/{owner}/{repo}`, the form mercs.ink documents. Any other form is
/// refused, not normalised.
fn github_project(repository: &str) -> Result<String, String> {
    let bad = || {
        format!(
            "mercs.ink relays the repository as \"{repository}\", which is not of the form \
             https://github.com/{{owner}}/{{repo}}, so the release's GitHub digest cannot be \
             looked up"
        )
    };
    let rest = repository.strip_prefix("https://github.com/").ok_or_else(bad)?;
    let valid = |s: &str| {
        !s.is_empty()
            && !s.starts_with('.')
            && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    match rest.split('/').collect::<Vec<_>>().as_slice() {
        [owner, repo] if valid(owner) && valid(repo) => Ok(format!("{owner}/{repo}")),
        _ => Err(bad()),
    }
}

/// Check the downloaded bytes against the `digest` GitHub publishes for the asset named `name`
/// in `gh` (the release fetched by tag). Returns the verified sha256 hex.
///
/// Zero or several assets of that name, a missing or `null` digest, a digest that is not sha256
/// and a mismatch are all hard failures. There is no fallback.
fn verify_asset_digest(gh: &net::Release, name: &str, bytes: &[u8]) -> Result<String, String> {
    let matches: Vec<&net::Asset> = gh.assets.iter().filter(|a| a.name == name).collect();
    let asset = match matches.as_slice() {
        [one] => *one,
        [] => {
            return Err(format!(
                "GitHub's release {} has no asset named {name}, so its digest cannot be checked",
                gh.tag
            ))
        }
        _ => {
            return Err(format!(
                "GitHub's release {} has {} assets named {name}; Modkit cannot tell which digest applies",
                gh.tag,
                matches.len()
            ))
        }
    };
    let digest = asset.digest.as_deref().ok_or_else(|| {
        format!(
            "GitHub publishes no digest for {name} in release {}. A Shipment zip must carry one \
             (it has to be uploaded after GitHub began computing digests); Modkit will not \
             install a zip it cannot verify.",
            gh.tag
        )
    })?;
    let want = asset
        .sha256()
        .ok_or_else(|| format!("GitHub's digest for {name} is \"{digest}\", not a sha256 digest"))?
        .to_ascii_lowercase();
    let got = sha256_hex(bytes);
    if want != got {
        return Err(format!(
            "{name} does not match its GitHub digest: GitHub publishes sha256:{want}, the \
             downloaded bytes are sha256:{got}. Nothing was installed."
        ));
    }
    Ok(got)
}

/// A downloaded Shipment zip that passed selection and digest verification.
#[derive(Debug)]
struct VerifiedZip {
    name: String,
    bytes: Vec<u8>,
}

/// Select, download and verify one release's Shipment zip, reading its digest from GitHub.
async fn fetch_verified_zip(
    client: &reqwest::Client,
    item: &RegistryMod,
    release: &RegistryRelease,
) -> Result<VerifiedZip, String> {
    fetch_verified_zip_at(client, item, release, net::release::GITHUB_API).await
}

/// [`fetch_verified_zip`] with the GitHub API root the digest lookup addresses.
/// [`fetch_verified_zip`] passes the real GitHub root; tests pass a loopback listener.
async fn fetch_verified_zip_at(
    client: &reqwest::Client,
    item: &RegistryMod,
    release: &RegistryRelease,
    github_api: &str,
) -> Result<VerifiedZip, String> {
    let what = format!("{} {}", item.slug, release.version);
    let head_name = release
        .manifest
        .as_ref()
        .and_then(|m| non_empty(m.shipment.name.clone()))
        .unwrap_or_else(|| item.slug.clone());
    let expected = expected_zip_name(&head_name, &release.version);

    // Addressing is checked before any download, so a release that can't be verified costs
    // nothing.
    let tag = release
        .tag
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| format!("{what} has no tag on mercs.ink, so its GitHub digest cannot be looked up"))?;
    let repository = item.repository.as_deref().ok_or_else(|| {
        format!("mercs.ink relays no repository for {}, so its GitHub digest cannot be looked up", item.slug)
    })?;
    let project = github_project(repository)?;

    let (asset, bytes) = match plan_archive_choice(&release.assets, &expected, &what)? {
        ArchiveChoice::Named(a) => (a.clone(), download_bytes(client, &a.download_url).await?),
        ArchiveChoice::Inspect(zips) => {
            let mut opened = Vec::with_capacity(zips.len());
            for a in zips {
                opened.push((a.clone(), download_bytes(client, &a.download_url).await?));
            }
            choose_opened(opened, &what, &expected)?
        }
    };

    let lookup = net::release::github_release_by_tag_url(github_api, &project, tag);
    let gh = net::release::github_release_by_tag_at(client, &lookup, &project, tag).await?;
    verify_asset_digest(&gh, &asset.name, &bytes)?;

    // After verification the chosen zip must hold a manifest — a named zip included.
    if !zip_contains_manifest(&bytes).map_err(|e| format!("{}: {e}", asset.name))? {
        return Err(format!(
            "{what}: {} holds no manifest.yaml/.yml/.json/.toml at its root or one folder down, \
             so it is not a Quartermaster Shipment. (A finished vz-patch.wad goes through Import \
             Patch WAD instead.)",
            asset.name
        ));
    }
    Ok(VerifiedZip { name: asset.name, bytes })
}

// ---------------------------------------------------------------------------------------
// Staging
// ---------------------------------------------------------------------------------------

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

/// A verified zip unpacked beside its final place, waiting to be committed.
#[derive(Debug)]
struct Staged {
    key: String,
    incoming: PathBuf,
    /// The Shipment root, relative to the staging directory.
    root_rel: PathBuf,
    asset: String,
}

/// Write the zip under `downloading/<key>/` and unpack it into `staging/<key>.incoming`, so the
/// Shipment currently staged under `<key>` is untouched until every download has succeeded.
fn stage_incoming(key: &str, zip: &VerifiedZip, what: &str) -> Result<Staged, String> {
    let dl = downloading_dir()?.join(key);
    let incoming = staging_dir()?.join(format!("{key}.incoming"));
    for dir in [&dl, &incoming] {
        if dir.exists() {
            std::fs::remove_dir_all(dir)
                .map_err(|e| format!("Could not clear {}: {e}", dir.display()))?;
        }
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    }
    let archive = dl.join(&zip.name);
    std::fs::write(&archive, &zip.bytes).map_err(|e| format!("Failed to write {}: {e}", zip.name))?;
    extract_zip(&archive, &incoming)?;
    let Some(root) = find_shipment_root(&incoming) else {
        let _ = std::fs::remove_dir_all(&incoming);
        return Err(format!(
            "{what}: {} unpacked with no manifest at its root or one folder down",
            zip.name
        ));
    };
    let root_rel = root
        .strip_prefix(&incoming)
        .map_err(|e| format!("{what}: the Shipment root is outside its staging dir: {e}"))?
        .to_path_buf();
    Ok(Staged { key: key.to_string(), incoming, root_rel, asset: zip.name.clone() })
}

/// Move every incoming directory into place, replacing what was staged under the same key. The
/// first failure stops, naming what was committed and what was not. Returns each final root.
fn commit_staged(staged: &[Staged]) -> Result<Vec<PathBuf>, String> {
    let staging = staging_dir()?;
    let mut roots = Vec::with_capacity(staged.len());
    for (i, s) in staged.iter().enumerate() {
        let dest = staging.join(&s.key);
        let step = || -> Result<(), String> {
            if dest.exists() {
                std::fs::remove_dir_all(&dest)
                    .map_err(|e| format!("Could not replace {}: {e}", dest.display()))?;
            }
            std::fs::rename(&s.incoming, &dest)
                .map_err(|e| format!("Could not move {} into place: {e}", s.incoming.display()))
        };
        if let Err(e) = step() {
            let done: Vec<&str> = staged[..i].iter().map(|s| s.key.as_str()).collect();
            let left: Vec<&str> = staged[i..].iter().map(|s| s.key.as_str()).collect();
            return Err(format!(
                "{e}\nStaged: {}. Not staged: {}.",
                if done.is_empty() { "nothing".into() } else { done.join(", ") },
                left.join(", ")
            ));
        }
        roots.push(dest.join(&s.root_rel));
    }
    Ok(roots)
}

/// The load-order row for a release staged at `root`.
fn shipment_ref(
    item: &RegistryMod,
    release: &RegistryRelease,
    key: &str,
    root: &Path,
    reason: InstallReason,
) -> ShipmentRef {
    // Identity comes off the wire, already parsed. `item.slug` is itself `shipment.name`,
    // so the fallback is the same value from a different field rather than a guess.
    let head = release.manifest.clone().unwrap_or_default().shipment;
    let ship_slug = non_empty(head.name).unwrap_or_else(|| item.slug.clone());
    // The contract scopes a `registry` entry's version to the manifest's `shipment.version`;
    // the release version is the fallback for a manifest that declares none.
    let ship_version = non_empty(head.version).or_else(|| Some(release.version.clone()));
    let display = non_empty(item.title.clone())
        .or_else(|| non_empty(head.title))
        .unwrap_or_else(|| ship_slug.clone());
    ShipmentRef {
        // Folder-derived like every other Shipment row: this is the load order's dedupe key and
        // becomes a `ClaimGroup::mod_id`, so it has to be per-checkout. The identity lives in
        // `slug` and `origin`, which is what leaves the machine.
        id: format!("shipment:{key}"),
        name: display,
        path: root.to_string_lossy().to_string(),
        slug: Some(ship_slug),
        version: ship_version.clone(),
        // An entry installed this way records `registry` and the registry's own precomposed
        // identifier, moved across untouched. Absent on the server → `None`.
        origin: Origin::registry(item.id.clone(), ship_version),
        install_reason: reason,
    }
}

// ---------------------------------------------------------------------------------------
// Install, with dependencies
// ---------------------------------------------------------------------------------------

/// The resolver's view of every release of one Shipment.
///
/// A release whose declared format is not 2 is not installable, so it is not a candidate.
/// Every other release must carry a semver `shipment.version` (qm refuses anything else) and
/// a readable `load.requires`; one that does not is an error, not a skipped row.
fn candidates(name: &str, releases: &[RegistryRelease]) -> Result<Vec<Candidate>, String> {
    let mut out = Vec::new();
    for r in releases {
        if declared_format(r)? != Some(SUPPORTED_MANIFEST_FORMAT) {
            continue;
        }
        let what = format!("{name} {}", r.version);
        let head = r.manifest.as_ref().ok_or_else(|| {
            format!("mercs.ink serves no manifest for {what}, so its requirements are unknown")
        })?;
        let v = non_empty(head.shipment.version.clone())
            .ok_or_else(|| format!("{what}'s manifest declares no shipment.version"))?;
        let version = semver::Version::parse(&v)
            .map_err(|e| format!("{what}'s shipment.version \"{v}\" is not semver: {e}"))?;
        out.push(Candidate {
            version,
            release_version: r.version.clone(),
            needs: relayed_shipment_needs(head.load.as_ref(), &what)?,
        });
    }
    Ok(out)
}

/// A Shipment installed or updated because something required it.
#[derive(Debug, Clone, Serialize)]
pub struct DependencyInstall {
    pub shipment: ShipmentRef,
    pub release_version: String,
    /// The version it replaced, when this was an update of an installed row.
    pub updated_from: Option<String>,
    pub asset: String,
}

/// A Shipment installed from mercs.ink, ready for the load order.
#[derive(Debug, Clone, Serialize)]
pub struct MercsInkInstall {
    /// The load-order entry. Its `origin` is `registry` carrying the registry's opaque id —
    /// the point of the whole module. Its `install_reason` is `user`, which also promotes a row
    /// that was installed as a dependency.
    pub shipment: ShipmentRef,
    /// Registry slug. Display and lookup only; not an identity on its own.
    pub slug: String,
    pub title: Option<String>,
    /// The release that was installed, as the registry names it.
    pub release_version: String,
    /// qm's `Target` for this release — shipment compatibility, **not** `game.target`.
    pub target: Option<String>,
    /// The Shipment zip pulled from GitHub, as selected and verified.
    pub assets: Vec<String>,
    pub staged_files: usize,
    /// What the resolver installed or updated alongside it, in the order it resolved them.
    pub dependencies: Vec<DependencyInstall>,
}

/// Install a Shipment from mercs.ink, with every Shipment it requires, and record where each
/// came from.
///
/// `version` picks a release; `None` takes the mod's latest. `installed` is the
/// library's current Shipment rows: their requirements come from a `qm preflight` over them, so
/// ranges they place on a dependency count too. Every zip is downloaded and verified before
/// anything already staged is replaced.
#[tauri::command]
pub async fn install_mercsink_shipment(
    window: Window,
    slug: String,
    version: Option<String>,
    installed: Vec<ShipmentRef>,
    game_path: String,
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

    let what = format!("{slug} {}", release.version);
    ensure_supported_format(declared_format(&release)?, &what)?;

    // Resolve what it requires, against what is installed.
    let root_name = release
        .manifest
        .as_ref()
        .and_then(|m| non_empty(m.shipment.name.clone()))
        .unwrap_or_else(|| item.slug.clone());
    let root_needs =
        relayed_shipment_needs(release.manifest.as_ref().and_then(|m| m.load.as_ref()), &what)?;
    let (installed_rows, installed_needs) = if installed.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        let plan = preflight_rows(window, &installed, &game_path).await?;
        installed_from_plan(&plan, &installed, &root_name)?
    };

    let mut releases: BTreeMap<String, Vec<RegistryRelease>> = BTreeMap::new();
    let mut catalog: BTreeMap<String, Vec<Candidate>> = BTreeMap::new();
    let picks = loop {
        match resolve(&root_name, &root_needs, &installed_rows, &installed_needs, &catalog)? {
            Step::Done(picks) => break picks,
            Step::NeedReleases(name) => {
                let url = format!("{}/releases", mod_url(&name));
                let list: Vec<RegistryRelease> =
                    unwrap_envelope(&fetch(&client, &url, &cache).await?.body, "releases")?;
                catalog.insert(name.clone(), candidates(&name, &list)?);
                releases.insert(name, list);
            }
        }
    };

    // Download and verify everything before any staged Shipment is replaced.
    let reason_of = |name: &str| -> InstallReason {
        installed_rows
            .iter()
            .find(|r| r.name == name)
            .and_then(|r| installed.iter().find(|s| s.id == r.id))
            .map(|s| s.install_reason)
            .unwrap_or(InstallReason::Dependency)
    };
    let mut staged: Vec<Staged> = Vec::new();
    let mut dep_meta: Vec<(RegistryMod, RegistryRelease, InstallReason, Option<String>)> = Vec::new();
    for pick in &picks {
        let dep_item: RegistryMod =
            unwrap_envelope(&fetch(&client, &mod_url(&pick.name), &cache).await?.body, "mod")?;
        let dep_release = releases[&pick.name]
            .iter()
            .find(|r| r.version == pick.release_version)
            .cloned()
            .ok_or_else(|| format!("{} {} vanished from its release list", pick.name, pick.release_version))?;
        let dep_what = format!("{} {}", pick.name, dep_release.version);
        ensure_supported_format(declared_format(&dep_release)?, &dep_what)?;
        let zip = fetch_verified_zip(&client, &dep_item, &dep_release).await?;
        staged.push(stage_incoming(&stage_key(&dep_item), &zip, &dep_what)?);
        let from = match &pick.action {
            Action::Update { from } => Some(from.clone()),
            Action::Install => None,
        };
        dep_meta.push((dep_item, dep_release, reason_of(&pick.name), from));
    }
    let root_key = stage_key(&item);
    let root_zip = fetch_verified_zip(&client, &item, &release).await?;
    staged.push(stage_incoming(&root_key, &root_zip, &what)?);

    let roots = commit_staged(&staged)?;

    let dependencies = dep_meta
        .into_iter()
        .zip(&staged)
        .zip(&roots)
        .map(|(((dep_item, dep_release, reason, from), s), root)| DependencyInstall {
            shipment: shipment_ref(&dep_item, &dep_release, &s.key, root, reason),
            release_version: dep_release.version.clone(),
            updated_from: from,
            asset: s.asset.clone(),
        })
        .collect();
    let root = roots.last().expect("the root is always staged last");
    let shipment = shipment_ref(&item, &release, &root_key, root, InstallReason::User);

    Ok(MercsInkInstall {
        staged_files: stage_file_count(root),
        shipment,
        slug: item.slug,
        title: item.title,
        release_version: release.version,
        target: release.target.or(item.target),
        assets: vec![root_zip.name],
        dependencies,
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

    /// Format 2 is the only manifest format. Format 1 is refused like any other value,
    /// decided from metadata alone so a refusal costs no download.
    #[test]
    fn the_format_gate_admits_only_format_2() {
        assert_eq!(SUPPORTED_MANIFEST_FORMAT, 2);
        let err = ensure_supported_format(Some(1), "x 1.0").unwrap_err();
        assert!(err.contains("format 1"), "got: {err}");
        assert!(ensure_supported_format(Some(2), "x 1.0").is_ok());
        assert!(ensure_supported_format(Some(3), "x 1.0").is_err());
        assert!(ensure_supported_format(None, "x 1.0").is_err(), "no format is not a pass");
    }

    fn rel_with_format(col: Option<u32>, inner: Option<u32>) -> RegistryRelease {
        RegistryRelease {
            version: "1".into(),
            tag: None,
            published_at: None,
            target: None,
            format: col,
            assets: Vec::new(),
            manifest: inner.map(|f| ManifestHead { format: Some(f), ..Default::default() }),
        }
    }

    /// The column and the served manifest must agree; a release that contradicts itself is
    /// refused rather than read either way.
    #[test]
    fn a_disagreeing_format_is_refused() {
        assert!(declared_format(&rel_with_format(Some(1), Some(2))).is_err());
        assert!(declared_format(&rel_with_format(Some(2), Some(1))).is_err());
        assert_eq!(declared_format(&rel_with_format(Some(2), Some(2))).unwrap(), Some(2));
        assert_eq!(declared_format(&rel_with_format(None, Some(2))).unwrap(), Some(2));
        assert_eq!(declared_format(&rel_with_format(None, None)).unwrap(), None);
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
                "target":"retail","format":2,
                "assets":[{"name":"vehicle-pack.zip","download_url":"https://github.com/octocat/vehicle-pack/releases/download/v1.0.0/vehicle-pack.zip","size":12345,"content_type":"application/octet-stream","download_count":42}],
                "manifest":{"format":2,"shipment":{"name":"vehicle-pack","version":"1.0.0","target":"retail"},"load":{"requires":[{"shipment":"lua-bridge","version":"^1.0.0"}]},"contributions":[]}
            }
        }]}"#;
        let mods: Vec<RegistryMod> = unwrap_envelope(body, "registry").unwrap();
        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].id.as_deref(), Some("vehicle-pack-486521234"));
        let rel = mods[0].latest_release.as_ref().unwrap();
        assert_eq!(declared_format(rel).unwrap(), Some(2));
        let needs = relayed_shipment_needs(
            rel.manifest.as_ref().unwrap().load.as_ref(),
            "vehicle-pack 1.0.0",
        )
        .unwrap();
        assert_eq!(needs[0].target, "lua-bridge");
        assert_eq!(
            rel.manifest.as_ref().unwrap().shipment.version.as_deref(),
            Some("1.0.0")
        );
        // The wire carries five asset fields; `download_count` feeds mercs.ink's author
        // dashboard and is skipped rather than mirrored unused. No checksum is relayed; the
        // digest comes from GitHub.
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

    fn asset(n: &str) -> ReleaseAsset {
        ReleaseAsset {
            name: n.into(),
            download_url: format!("https://example.invalid/{n}"),
            size: None,
            content_type: None,
        }
    }

    /// An in-memory zip holding the given file names.
    fn zip_of(files: &[&str]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for f in files {
            w.start_file(*f, zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut w, b"x").unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn the_expected_zip_name_never_doubles_the_v() {
        assert_eq!(expected_zip_name("ess", "0.7.0"), "ess-v0.7.0.zip");
        assert_eq!(expected_zip_name("ess", "v0.7.0"), "ess-v0.7.0.zip");
    }

    /// Exactly one exactly-named zip beside other zips is taken, and selection decides so
    /// from the names alone — no other zip is downloaded or opened.
    #[test]
    fn a_named_zip_beside_other_zips_is_taken_without_opening_any() {
        let assets = vec![
            asset("Ess-0.7.0.zip"),
            asset("ess-v0.7.0.zip"),
            asset("lua_console.py"),
            asset("ess-v0.7.0-debug.zip"),
        ];
        match plan_archive_choice(&assets, "ess-v0.7.0.zip", "ess 0.7.0").unwrap() {
            ArchiveChoice::Named(a) => assert_eq!(a.name, "ess-v0.7.0.zip"),
            other => panic!("expected the named zip, got {other:?}"),
        }
    }

    /// Without the exact name, every zip is a candidate to open.
    #[test]
    fn without_the_exact_name_every_zip_is_opened() {
        let assets = vec![asset("Ess-0.7.0.zip"), asset("notes.md"), asset("extra.ZIP")];
        match plan_archive_choice(&assets, "ess-v0.7.0.zip", "ess 0.7.0").unwrap() {
            ArchiveChoice::Inspect(z) => {
                let names: Vec<&str> = z.iter().map(|a| a.name.as_str()).collect();
                assert_eq!(names, vec!["Ess-0.7.0.zip", "extra.ZIP"]);
            }
            other => panic!("expected inspection, got {other:?}"),
        }
    }

    /// No `.zip` at all is an error, and there is no loose-asset install.
    #[test]
    fn a_release_with_no_zip_is_refused() {
        let err = plan_archive_choice(&[asset("lua_bridge_DEV.asi"), asset("notes.md")], "x-v1.zip", "x 1")
            .unwrap_err();
        assert!(err.contains("has no Shipment zip"), "{err}");
        assert!(err.contains("lua_bridge_DEV.asi"), "lists what is there: {err}");
        let err = plan_archive_choice(&[], "x-v1.zip", "x 1").unwrap_err();
        assert!(err.contains("no assets at all"), "{err}");
    }

    #[test]
    fn a_manifest_counts_at_the_root_or_one_folder_down_only() {
        assert!(zip_contains_manifest(&zip_of(&["manifest.yaml", "src/a.lua"])).unwrap());
        assert!(zip_contains_manifest(&zip_of(&["ess-0.7.0/manifest.json"])).unwrap());
        assert!(!zip_contains_manifest(&zip_of(&["a/b/manifest.yaml"])).unwrap());
        assert!(!zip_contains_manifest(&zip_of(&["README.md"])).unwrap());
        assert!(zip_contains_manifest(b"not a zip").is_err());
    }

    /// No exactly-named zip: a Shipment zip beside a manifest-less zip is the one taken.
    #[test]
    fn the_one_zip_with_a_manifest_is_chosen() {
        let opened = vec![
            (asset("symbols.zip"), zip_of(&["a.pdb"])),
            (asset("ess.zip"), zip_of(&["ess/manifest.yaml"])),
        ];
        let (a, _) = choose_opened(opened, "ess 0.7.0", "ess-v0.7.0.zip").unwrap();
        assert_eq!(a.name, "ess.zip");
    }

    #[test]
    fn zero_qualifying_zips_is_an_error_listing_each() {
        let opened = vec![
            (asset("symbols.zip"), zip_of(&["a.pdb"])),
            (asset("broken.zip"), b"nope".to_vec()),
        ];
        let err = choose_opened(opened, "ess 0.7.0", "ess-v0.7.0.zip").unwrap_err();
        assert!(err.contains("symbols.zip: no manifest"), "{err}");
        assert!(err.contains("broken.zip: not a readable zip"), "{err}");
    }

    #[test]
    fn two_qualifying_zips_is_an_error() {
        let opened = vec![
            (asset("a.zip"), zip_of(&["manifest.yaml"])),
            (asset("b.zip"), zip_of(&["manifest.yaml"])),
        ];
        let err = choose_opened(opened, "ess 0.7.0", "ess-v0.7.0.zip").unwrap_err();
        assert!(err.contains("several"), "{err}");
        assert!(err.contains("a.zip") && err.contains("b.zip"), "{err}");
    }

    fn gh_release(assets: Vec<net::Asset>) -> net::Release {
        net::Release {
            tag: "v0.7.0".into(),
            name: "v0.7.0".into(),
            url: String::new(),
            body: String::new(),
            assets,
        }
    }

    fn gh_asset(name: &str, digest: Option<&str>) -> net::Asset {
        net::Asset {
            name: name.into(),
            url: String::new(),
            size: None,
            digest: digest.map(str::to_string),
            state: Some("uploaded".into()),
        }
    }

    /// Matching bytes are accepted; a one-byte change is refused naming both hashes.
    #[test]
    fn the_digest_is_checked_against_the_downloaded_bytes() {
        let bytes = zip_of(&["manifest.yaml"]);
        let good = format!("sha256:{}", sha256_hex(&bytes));
        let gh = gh_release(vec![gh_asset("ess-v0.7.0.zip", Some(&good))]);
        assert_eq!(verify_asset_digest(&gh, "ess-v0.7.0.zip", &bytes).unwrap(), sha256_hex(&bytes));

        let mut changed = bytes.clone();
        let last = changed.len() - 1;
        changed[last] ^= 0x01;
        let err = verify_asset_digest(&gh, "ess-v0.7.0.zip", &changed).unwrap_err();
        assert!(err.contains(&sha256_hex(&bytes)), "names GitHub's hash: {err}");
        assert!(err.contains(&sha256_hex(&changed)), "names the downloaded hash: {err}");
    }

    /// A missing or `null` digest is a hard failure.
    #[test]
    fn a_missing_digest_is_refused() {
        let gh = gh_release(vec![gh_asset("ess-v0.7.0.zip", None)]);
        let err = verify_asset_digest(&gh, "ess-v0.7.0.zip", b"x").unwrap_err();
        assert!(err.contains("no digest"), "{err}");

        // GitHub's JSON with `"digest": null`, through the real parser.
        let v = serde_json::json!({ "tag_name": "v0.7.0", "assets": [
            { "name": "ess-v0.7.0.zip", "browser_download_url": "u", "digest": null } ] });
        let parsed = net::release::release_from_github(&v).expect("a usable release");
        assert!(verify_asset_digest(&parsed, "ess-v0.7.0.zip", b"x").is_err());

        let gh = gh_release(vec![gh_asset("ess-v0.7.0.zip", Some("md5:abc"))]);
        assert!(verify_asset_digest(&gh, "ess-v0.7.0.zip", b"x").unwrap_err().contains("not a sha256"));
    }

    /// Zero or several assets with the chosen name are hard errors.
    #[test]
    fn the_digest_asset_must_be_found_exactly_once() {
        let gh = gh_release(vec![gh_asset("other.zip", Some("sha256:00"))]);
        assert!(verify_asset_digest(&gh, "ess-v0.7.0.zip", b"x").unwrap_err().contains("no asset named"));
        let gh = gh_release(vec![
            gh_asset("ess-v0.7.0.zip", Some("sha256:00")),
            gh_asset("ess-v0.7.0.zip", Some("sha256:00")),
        ]);
        assert!(verify_asset_digest(&gh, "ess-v0.7.0.zip", b"x").is_err());
    }

    /// `repository` must be exactly `https://github.com/{owner}/{repo}`.
    #[test]
    fn only_the_exact_github_repository_form_is_accepted() {
        assert_eq!(github_project("https://github.com/loganw234/lua-bridge").unwrap(), "loganw234/lua-bridge");
        for bad in [
            "http://github.com/o/r",
            "https://github.com/o/r/",
            "https://github.com/o/r.git/extra",
            "https://github.com/o",
            "git@github.com:o/r.git",
            "https://gitlab.com/o/r",
            "https://github.com/../r",
            "",
        ] {
            assert!(github_project(bad).is_err(), "{bad}");
        }
    }

    /// A release that is not format 2 is not a resolver candidate; a format-2 release without a
    /// semver version is an error rather than a skipped row.
    #[test]
    fn candidates_are_the_format_2_releases() {
        let release = |v: &str, format: u32, version: Option<&str>| RegistryRelease {
            version: v.into(),
            tag: Some(format!("v{v}")),
            published_at: None,
            target: None,
            format: Some(format),
            assets: vec![],
            manifest: Some(ManifestHead {
                format: Some(format),
                shipment: ShipmentHead {
                    name: Some("lua-bridge".into()),
                    title: None,
                    version: version.map(str::to_string),
                    target: None,
                },
                load: None,
            }),
        };
        let c = candidates(
            "lua-bridge",
            &[release("0.5.4", 1, Some("0.5.4")), release("1.0.0", 2, Some("1.0.0"))],
        )
        .unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].version, semver::Version::new(1, 0, 0));
        assert!(candidates("lua-bridge", &[release("1.0", 2, Some("1.0"))]).is_err());
        assert!(candidates("lua-bridge", &[release("1.0.0", 2, None)]).is_err());
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
        serve_bytes(responses.into_iter().map(String::into_bytes).collect())
    }

    /// [`serve`] for responses whose bodies are not text, such as a zip download.
    fn serve_bytes(responses: Vec<Vec<u8>>) -> (String, std::thread::JoinHandle<Vec<String>>) {
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
                let _ = sock.write_all(&body);
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

    /// A `200` whose body is raw bytes, as a release-asset download answers.
    fn ok_bytes(body: &[u8]) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(body);
        out
    }

    /// GitHub's by-tag release JSON with one asset carrying `digest`.
    fn gh_release_json(tag: &str, asset: &str, digest: &str) -> String {
        serde_json::json!({
            "tag_name": tag,
            "name": tag,
            "assets": [{
                "name": asset,
                "browser_download_url": format!("https://example.invalid/{asset}"),
                "digest": digest,
                "state": "uploaded",
            }],
        })
        .to_string()
    }

    /// The by-tag lookup reads the release a tag names, and the asset's digest comes back
    /// exactly as GitHub published it.
    #[test]
    fn a_tag_lookup_resolves_the_asset_digest() {
        let digest = format!("sha256:{}", "ab".repeat(32));
        let body = gh_release_json("v0.7.0", "ess-v0.7.0.zip", &digest);
        let (base, handle) = serve(vec![ok_with_etag("\"r1\"", &body)]);
        let url = net::release::github_release_by_tag_url(&base, "o/r", "v0.7.0");

        let gh = rt()
            .block_on(net::release::github_release_by_tag_at(&client().unwrap(), &url, "o/r", "v0.7.0"))
            .unwrap();
        assert_eq!(gh.tag, "v0.7.0");
        assert_eq!(gh.assets.len(), 1);
        assert_eq!(gh.assets[0].name, "ess-v0.7.0.zip");
        assert_eq!(gh.assets[0].digest.as_deref(), Some(digest.as_str()));
        let _ = handle.join();
    }

    /// A tag GitHub does not know is a hard failure naming the project, the tag and the status,
    /// never an empty release.
    #[test]
    fn a_missing_tag_is_a_hard_failure() {
        let (base, handle) = serve(vec![error_body("404 Not Found", r#"{"message":"Not Found"}"#)]);
        let url = net::release::github_release_by_tag_url(&base, "o/r", "v9.9.9");

        let err = rt()
            .block_on(net::release::github_release_by_tag_at(&client().unwrap(), &url, "o/r", "v9.9.9"))
            .unwrap_err();
        assert_eq!(err, "GitHub release lookup failed for o/r tag v9.9.9: 404 Not Found");
        let _ = handle.join();
    }

    /// An exactly-named zip is taken without being opened, so the manifest check after the
    /// digest is what refuses one that holds no manifest, even though its digest matches.
    #[test]
    fn a_named_zip_without_a_manifest_is_refused_after_its_digest_matches() {
        let zip = zip_of(&["README.md", "src/a.lua"]);
        let digest = format!("sha256:{}", sha256_hex(&zip));
        let gh = gh_release_json("v0.7.0", "ess-v0.7.0.zip", &digest);
        let (base, handle) = serve_bytes(vec![ok_bytes(&zip), ok_with_etag("\"r1\"", &gh).into_bytes()]);

        let item: RegistryMod = serde_json::from_value(serde_json::json!({
            "slug": "ess",
            "repository": "https://github.com/o/r",
        }))
        .unwrap();
        let release: RegistryRelease = serde_json::from_value(serde_json::json!({
            "version": "0.7.0",
            "tag": "v0.7.0",
            "assets": [{
                "name": "ess-v0.7.0.zip",
                "download_url": format!("{base}/download/ess-v0.7.0.zip"),
            }],
        }))
        .unwrap();

        let err = rt()
            .block_on(fetch_verified_zip_at(&client().unwrap(), &item, &release, &base))
            .unwrap_err();
        assert_eq!(
            err,
            "ess 0.7.0: ess-v0.7.0.zip holds no manifest.yaml/.yml/.json/.toml at its root or one \
             folder down, so it is not a Quartermaster Shipment. (A finished vz-patch.wad goes \
             through Import Patch WAD instead.)"
        );
        let _ = handle.join();
    }

    // ------------------------------------------------------------------------------------
    // The incompatibility list, against a loopback listener.
    // ------------------------------------------------------------------------------------

    use crate::commands::incompatibility::tests::EXAMPLE;

    /// A base URL nothing listens on: the port was bound and released.
    fn refused_base() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        format!("http://{addr}")
    }

    fn list_url(base: &str) -> String {
        format!("{base}/api/v1/incompatibilities")
    }

    fn load(url: &str, cache: &Path, now: u64) -> Result<LoadedList, String> {
        rt().block_on(load_incompatibilities(&client().unwrap(), url, cache, now))
    }

    /// Put a cached copy of [`EXAMPLE`] for `url` into `cache`, fetched at `fetched_at`.
    fn seed(cache: &Path, url: &str, fetched_at: u64) {
        let mut c = ListCache::new();
        c.insert(url.into(), CachedList { etag: "\"v1\"".into(), body: EXAMPLE.into(), fetched_at });
        write_list_cache(cache, &c).unwrap();
    }

    #[test]
    fn a_first_200_is_stored_and_current() {
        let (base, handle) = serve(vec![ok_with_etag("\"v1\"", EXAMPLE)]);
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("list.json");
        let url = list_url(&base);

        let got = load(&url, &cache, 100).unwrap();
        assert_eq!(got.state, ListState::Current { fetched_at: 100 });
        assert_eq!(got.index.unwrap().rows.len(), 2);
        let stored = &read_list_cache(&cache).unwrap()[&url];
        assert_eq!((stored.etag.as_str(), stored.body.as_str(), stored.fetched_at), ("\"v1\"", EXAMPLE, 100));
        let _ = handle.join();
    }

    #[test]
    fn a_304_is_current_and_rewrites_fetched_at() {
        let (base, handle) = serve(vec![
            ok_with_etag("\"v1\"", EXAMPLE),
            status_only("304 Not Modified", "ETag: \"v1\"\r\n"),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("list.json");
        let url = list_url(&base);

        load(&url, &cache, 100).unwrap();
        let got = load(&url, &cache, 500).unwrap();
        assert_eq!(got.state, ListState::Current { fetched_at: 500 });
        assert_eq!(got.index.unwrap().rows.len(), 2);
        assert_eq!(read_list_cache(&cache).unwrap()[&url].fetched_at, 500);
        assert_eq!(handle.join().unwrap()[1], "\"v1\"", "the stored validator is sent back");
    }

    #[test]
    fn a_500_with_a_cache_uses_it_and_gives_its_age() {
        let (base, handle) = serve(vec![status_only("500 Internal Server Error", "")]);
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("list.json");
        let url = list_url(&base);
        seed(&cache, &url, 1_000);

        let got = load(&url, &cache, 1_000 + 3 * 3_600).unwrap();
        assert_eq!(got.state, ListState::cached(
            1_000,
            "2026-09-25T10:00:00+00:00".into(),
            "it answered HTTP 500".into(),
            1_000 + 3 * 3_600,
        ));
        match &got.state {
            ListState::Cached { message, .. } => assert!(message.contains("downloaded 3 hours ago"), "{message}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(got.index.unwrap().rows.len(), 2);
        assert_eq!(read_list_cache(&cache).unwrap()[&url].fetched_at, 1_000, "the age is not reset");
        let _ = handle.join();
    }

    #[test]
    fn a_refused_connection_with_a_cache_uses_it() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("list.json");
        let url = list_url(&refused_base());
        seed(&cache, &url, 1_000);

        let got = load(&url, &cache, 1_060).unwrap();
        match &got.state {
            ListState::Cached { fetched_at, reason, .. } => {
                assert_eq!(*fetched_at, 1_000);
                assert!(reason.starts_with("could not connect"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
        assert!(got.index.is_some());
    }

    #[test]
    fn with_no_cache_an_unreachable_server_means_never_fetched() {
        let (base, handle) = serve(vec![status_only("503 Service Unavailable", "")]);
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("list.json");

        let got = load(&list_url(&base), &cache, 100).unwrap();
        assert_eq!(got.state, ListState::never_fetched("it answered HTTP 503".into()));
        assert!(got.index.is_none());
        assert!(!cache.exists(), "nothing was fetched, so nothing is cached");
        let _ = handle.join();

        let got = load(&list_url(&refused_base()), &cache, 100).unwrap();
        assert!(matches!(&got.state, ListState::NeverFetched { reason, .. } if reason.starts_with("could not connect")));
        assert!(got.index.is_none());
    }

    #[test]
    fn a_404_is_an_error() {
        let (base, handle) = serve(vec![error_body("404 Not Found", r#"{"message":"Not found."}"#)]);
        let dir = tempfile::tempdir().unwrap();
        let err = load(&list_url(&base), &dir.path().join("list.json"), 100).unwrap_err();
        assert!(err.contains("404") && err.contains("Not found."), "{err}");
        let _ = handle.join();
    }

    #[test]
    fn a_429_then_a_200_is_current() {
        let (base, handle) = serve(vec![
            status_only("429 Too Many Requests", "Retry-After: 0\r\n"),
            ok_with_etag("\"v1\"", EXAMPLE),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let got = load(&list_url(&base), &dir.path().join("list.json"), 100).unwrap();
        assert_eq!(got.state, ListState::Current { fetched_at: 100 });
        let _ = handle.join();
    }

    #[test]
    fn a_corrupt_cache_is_an_error_naming_it() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("list.json");
        std::fs::write(&cache, "{ not json").unwrap();
        let err = load(&list_url(&refused_base()), &cache, 100).unwrap_err();
        assert!(err.contains(&cache.display().to_string()), "{err}");

        // A key Modkit does not write is not the form Modkit writes.
        std::fs::write(&cache, r#"{"u":{"etag":"","body":"","fetched_at":1,"extra":true}}"#).unwrap();
        assert!(load(&list_url(&refused_base()), &cache, 100).is_err());
    }

    #[test]
    fn a_cache_entry_for_another_base_url_is_never_fetched() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("list.json");
        seed(&cache, "https://staging.example/api/v1/incompatibilities", 1_000);
        let got = load(&list_url(&refused_base()), &cache, 2_000).unwrap();
        assert!(matches!(got.state, ListState::NeverFetched { .. }), "{:?}", got.state);
        assert!(got.index.is_none());
    }

    /// Each list breaks the contract a different way. Every one is refused, and the good cached
    /// copy is left byte for byte as it was.
    #[test]
    fn a_list_modkit_cannot_use_is_an_error_and_never_replaces_the_cache() {
        let bad = [
            EXAMPLE.replace(r#""status": "confirmed""#, r#""status": "maybe""#),
            EXAMPLE.replace(r#""reason": "crash_on_load""#, r#""reason": "slow""#),
            EXAMPLE.replace(r#""subject_range": "^1.0.0""#, r#""subject_range": "one-ish""#),
            EXAMPLE.replace(r#""other_range": ">=0.7.0, <0.8.0""#, r#""other_range": null"#),
        ];
        for b in &bad {
            assert_ne!(b, EXAMPLE, "each fixture must change the body");
        }
        let (base, handle) = serve(bad.iter().map(|b| ok_with_etag("\"v2\"", b)).collect());
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("list.json");
        let url = list_url(&base);
        seed(&cache, &url, 1_000);
        let before = std::fs::read(&cache).unwrap();

        for _ in &bad {
            let err = load(&url, &cache, 2_000).unwrap_err();
            assert!(err.starts_with("mercs.ink sent an incompatibility list Modkit cannot use"), "{err}");
            assert_eq!(std::fs::read(&cache).unwrap(), before, "the cache is untouched");
        }
        let _ = handle.join();
    }
}
