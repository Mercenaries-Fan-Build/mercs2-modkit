//! Streaming downloads with progress, so nothing in the app is a silent hang.

use serde::Serialize;
use tauri::{Emitter, Window};

/// Default ceiling on a single download. The largest thing modkit fetches is the
/// ~60 MB engine-backed Workshop app; a response an order of magnitude past that
/// is a redirect to something unexpected, not an artifact, and reading it to
/// completion would sit there filling memory.
pub const DEFAULT_MAX_BYTES: u64 = 512 * 1024 * 1024;

/// Emitted as `download-progress` while bytes are in flight.
///
/// One event shape for every download in the app. Before this, only the toolset
/// reported anything — and it counted *tools*, not bytes, so a single 60 MB file
/// showed one tick and then nothing for a minute.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    /// Stable identifier for the artifact, e.g. `pmc_bb` or `toolset:wad_simulator`.
    pub key: String,
    /// Human label for the UI.
    pub label: String,
    pub done_bytes: u64,
    /// `None` when the server sent no `Content-Length`, which is legal and means
    /// the UI must show an indeterminate bar rather than compute a percentage
    /// against a fabricated total.
    pub total_bytes: Option<u64>,
    /// Set once, on the final event for this download.
    pub done: bool,
}

pub struct DownloadOpts<'a> {
    pub key: &'a str,
    pub label: &'a str,
    /// Where to emit progress. `None` for callers with no window in hand (and for
    /// tests), which downloads exactly the same way, just silently.
    pub window: Option<&'a Window>,
    pub max_bytes: u64,
}

impl<'a> DownloadOpts<'a> {
    pub fn new(key: &'a str, label: &'a str) -> Self {
        Self {
            key,
            label,
            window: None,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }

    pub fn with_window(mut self, window: Option<&'a Window>) -> Self {
        self.window = window;
        self
    }
}

fn emit(opts: &DownloadOpts, done_bytes: u64, total_bytes: Option<u64>, done: bool) {
    if let Some(w) = opts.window {
        let _ = w.emit(
            "download-progress",
            DownloadProgress {
                key: opts.key.to_string(),
                label: opts.label.to_string(),
                done_bytes,
                total_bytes,
                done,
            },
        );
    }
}

/// GET `url` and return its bytes, reporting progress as they arrive.
///
/// Streams via `chunk()` rather than `bytes()`. The difference is not only the
/// progress events: `bytes()` gives one await that resolves after the whole body,
/// so a stalled transfer is indistinguishable from a slow one, and the
/// per-read inactivity timeout on the shared client has nothing to bite on.
///
/// Sends no `Authorization` header. These are public artifacts on third-party
/// hosts and modkit has no credentials to offer them.
pub async fn download(
    client: &reqwest::Client,
    url: &str,
    opts: DownloadOpts<'_>,
) -> Result<Vec<u8>, String> {
    let resp = super::client::get(client, url).await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("Download of {} failed: {status}", opts.label));
    }

    let total = resp.content_length();
    if let Some(t) = total {
        if t > opts.max_bytes {
            return Err(format!(
                "{} is {t} bytes, past the {} byte ceiling — that is not an artifact modkit expects.",
                opts.label, opts.max_bytes
            ));
        }
    }

    let mut out: Vec<u8> = Vec::with_capacity(total.unwrap_or(0).min(8 * 1024 * 1024) as usize);
    let mut resp = resp;
    emit(&opts, 0, total, false);

    loop {
        let chunk = resp
            .chunk()
            .await
            .map_err(|e| format!("Reading {} failed: {e}", opts.label))?;
        let Some(chunk) = chunk else { break };

        out.extend_from_slice(&chunk);
        // Checked against the running total too: a server may send no
        // `Content-Length`, or lie about it.
        if out.len() as u64 > opts.max_bytes {
            return Err(format!(
                "{} exceeded the {} byte ceiling mid-transfer.",
                opts.label, opts.max_bytes
            ));
        }
        emit(&opts, out.len() as u64, total, false);
    }

    // A truncated transfer that ends cleanly is the failure this catches: the
    // socket closed, `chunk()` returned `None`, and without this the caller would
    // write a short file and record it as installed.
    if let Some(t) = total {
        if out.len() as u64 != t {
            return Err(format!(
                "{} ended early — got {} of {t} bytes.",
                opts.label,
                out.len()
            ));
        }
    }

    emit(&opts, out.len() as u64, total, true);
    Ok(out)
}
