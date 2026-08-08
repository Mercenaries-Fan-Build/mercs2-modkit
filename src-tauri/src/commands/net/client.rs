//! The one HTTP client, and the one place a rate limit is honoured.

use std::sync::OnceLock;
use std::time::Duration;

/// Sent on every request modkit makes. Carrying the version is not decoration:
/// GitHub and mercs.ink both log it, and "which build is hammering us" is the
/// first question anyone asks. Six of the seven clients this replaced sent a bare
/// `mercs2-modkit` with no version at all.
pub const USER_AGENT: &str = concat!("mercs2-modkit/", env!("CARGO_PKG_VERSION"));

/// How long to wait for a connection. Short: a host that has not answered in this
/// long is down, not slow, and every caller is on an interactive path.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Inactivity timeout **per read**, not per request. This distinction is the whole
/// reason it is spelled this way: `Client::timeout` bounds the entire exchange
/// including the body, which would abort the 60 MB `mercs2_game` download on any
/// slow connection. A stalled socket is the failure worth catching; a large file
/// arriving steadily is not.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// Cap on how long a `429` may park an interactive request. `Retry-After` is
/// honoured verbatim up to this; beyond it something is wrong at the other end and
/// blocking a click for minutes is the worse answer.
pub const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Used when a `429` arrives without a parseable `Retry-After`.
pub const DEFAULT_BACKOFF: Duration = Duration::from_secs(5);

static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();

/// The shared client. Built once; `reqwest::Client` is an `Arc` internally, so
/// handing out clones is how it is meant to be used — and it is what lets every
/// caller share one connection pool instead of opening seven.
pub fn client() -> Result<reqwest::Client, String> {
    CLIENT.get_or_init(build).clone()
}

fn build() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .build()
        .map_err(|e| format!("Could not build an HTTP client: {e}"))
}

/// How long the server asked us to wait, clamped to [`MAX_BACKOFF`].
///
/// Only the delta-seconds form is read. The HTTP-date form is legal and neither
/// GitHub nor mercs.ink sends it; parsing dates to honour a header nobody emits
/// would be more code and more ways to be wrong than falling back to
/// [`DEFAULT_BACKOFF`].
pub fn retry_after(resp: &reqwest::Response) -> Duration {
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_BACKOFF)
        .min(MAX_BACKOFF)
}

/// GET `url`, retrying **once** on a `429` after honouring `Retry-After`.
///
/// One retry, deliberately: every caller is behind a click, and a second `429`
/// means the bucket is genuinely exhausted rather than momentarily tight. Callers
/// that can answer from a cache instead (mercs.ink) decide that for themselves —
/// this returns the response and lets them read the status.
pub async fn get(client: &reqwest::Client, url: &str) -> Result<reqwest::Response, String> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        let resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("Could not reach {url}: {e}"))?;

        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempts == 1 {
            tokio::time::sleep(retry_after(&resp)).await;
            continue;
        }
        return Ok(resp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_agent_carries_a_version() {
        assert!(
            USER_AGENT.starts_with("mercs2-modkit/"),
            "{USER_AGENT} lost its prefix"
        );
        assert!(
            USER_AGENT.len() > "mercs2-modkit/".len(),
            "{USER_AGENT} has no version after the slash — the six clients this \
             replaced sent exactly that, which is what made request logs useless"
        );
    }

    /// The builder is fallible and runs inside a `OnceLock`, so a panic or a
    /// poisoned first call would take out every download in the app. Cheap to pin
    /// that it succeeds, and that asking twice keeps succeeding.
    #[test]
    fn the_client_builds_repeatedly() {
        client().expect("first build");
        client().expect("second build reuses the cached result");
    }

    #[test]
    fn backoff_is_clamped() {
        assert!(DEFAULT_BACKOFF <= MAX_BACKOFF);
    }
}
