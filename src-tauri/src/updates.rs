//! Update check against the repo's GitHub releases.
//!
//! Pure core ([`parse_latest_release`], [`is_update_available`],
//! [`check_with`]) with the network fetch injected, so the unit tests
//! feed canned GitHub JSON and never touch the network. The production
//! fetcher ([`fetch_latest_release`]) is a thin reqwest wrapper used by
//! the `check_for_updates` Tauri command.

use serde::{Deserialize, Serialize};
use std::future::Future;
use std::time::Duration;

/// GitHub API endpoint answering with the latest release object.
pub const RELEASES_API_URL: &str =
    "https://api.github.com/repos/Rastrian/spore-tunnel-gui/releases/latest";
/// User-Agent GitHub requires on API calls (also identifies this app).
const USER_AGENT: &str = "spore-tunnel-gui";
/// Request budget for the update check; the UI blocks on it.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// Result of a check, rendered by the settings screen and the shell
/// update banner. Serialized camelCase for the frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// Version of the running app (e.g. `1.0.0`).
    pub current: String,
    /// Latest release tag, `v` prefix stripped (e.g. `1.1.0`).
    pub latest: String,
    pub update_available: bool,
    /// Browser-facing release page (`html_url` of the release).
    pub url: String,
}

/// The two fields of the GitHub `releases/latest` object we use.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LatestRelease {
    pub tag_name: String,
    pub html_url: String,
}

/// Parse a GitHub `releases/latest` response body.
pub fn parse_latest_release(body: &str) -> Result<LatestRelease, String> {
    serde_json::from_str(body).map_err(|e| format!("Invalid release response: {e}"))
}

/// Strip the usual `v`/`V` tag prefix ("v1.2.3" -> "1.2.3").
fn strip_tag_prefix(tag: &str) -> &str {
    tag.trim()
        .trim_start_matches(['v', 'V'])
        .trim_start()
}

/// Whether `latest_tag` is strictly newer than `current` in semver terms.
///
/// * Both sides get their `v` prefix stripped; current must parse (our
///   own version is always semver — a failure is a bug worth an error).
/// * A non-semver latest tag (e.g. "nightly") is conservatively *not* an
///   update: it may or may not be newer, so don't nag the user.
/// * Semver ordering applies, so a prerelease latest ("1.1.0-beta.1")
///   never triggers a banner over a stable current.
pub fn is_update_available(current: &str, latest_tag: &str) -> Result<bool, String> {
    let current = semver::Version::parse(strip_tag_prefix(current))
        .map_err(|e| format!("Current version \"{current}\" is not semver: {e}"))?;
    let Some(latest) = semver::Version::parse(strip_tag_prefix(latest_tag)).ok() else {
        return Ok(false);
    };
    Ok(latest > current)
}

/// Run a check: `fetch` produces the raw GitHub response body (or an
/// error string). Injected so tests feed canned JSON without network.
pub async fn check_with<F, Fut>(current: &str, fetch: F) -> Result<UpdateStatus, String>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<String, String>>,
{
    let body = fetch().await?;
    let release = parse_latest_release(&body)?;
    Ok(UpdateStatus {
        update_available: is_update_available(current, &release.tag_name)?,
        current: strip_tag_prefix(current).to_string(),
        latest: strip_tag_prefix(&release.tag_name).to_string(),
        url: release.html_url,
    })
}

/// Production fetcher: GET [`RELEASES_API_URL`] with the required
/// User-Agent header, return the response body. A 404 means the repo has
/// no releases yet (nothing to check against) — reported as such.
pub async fn fetch_latest_release() -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|e| format!("Update check failed: {e}"))?;
    let response = client
        .get(RELEASES_API_URL)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .map_err(|e| format!("Update check failed: {e}"))?;
    if response.status().as_u16() == 404 {
        return Err("No releases have been published yet.".to_string());
    }
    if !response.status().is_success() {
        return Err(format!(
            "Update check failed: HTTP {}",
            response.status().as_u16()
        ));
    }
    response
        .text()
        .await
        .map_err(|e| format!("Update check failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sample GitHub `releases/latest` body.
    const RELEASE_JSON: &str = r#"{
        "url": "https://api.github.com/repos/Rastrian/spore-tunnel-gui/releases/1",
        "html_url": "https://github.com/Rastrian/spore-tunnel-gui/releases/tag/v1.2.0",
        "tag_name": "v1.2.0",
        "name": "v1.2.0",
        "draft": false,
        "prerelease": false
    }"#;

    #[test]
    fn parse_extracts_tag_and_url() {
        let release = parse_latest_release(RELEASE_JSON).unwrap();
        assert_eq!(release.tag_name, "v1.2.0");
        assert_eq!(
            release.html_url,
            "https://github.com/Rastrian/spore-tunnel-gui/releases/tag/v1.2.0"
        );
    }

    #[test]
    fn parse_rejects_garbage_and_incomplete_bodies() {
        assert!(parse_latest_release("not json").is_err());
        // API error payloads parse as JSON but lack our fields.
        assert!(parse_latest_release(r#"{"message": "Not Found"}"#).is_err());
        // tag without url is incomplete.
        assert!(parse_latest_release(r#"{"tag_name": "v1.0.0"}"#).is_err());
    }

    #[test]
    fn strip_tag_prefix_handles_v_and_whitespace_only() {
        assert_eq!(strip_tag_prefix("v1.2.3"), "1.2.3");
        assert_eq!(strip_tag_prefix("  V2.0.0 "), "2.0.0");
        assert_eq!(strip_tag_prefix("1.0.0"), "1.0.0");
    }

    #[test]
    fn newer_versions_trigger_an_update() {
        for (current, latest) in [("1.0.0", "v1.1.0"), ("1.0.0", "v2.0.0"), ("1.2.0", "1.2.1")] {
            assert!(
                is_update_available(current, latest).unwrap(),
                "{latest} must be newer than {current}"
            );
        }
    }

    #[test]
    fn same_older_and_prerelease_versions_do_not() {
        let cases = [
            ("1.2.0", "v1.2.0"), // same
            ("1.1.0", "v1.0.0"), // older
            ("1.0.0", "v1.0.0-beta.1"), // prerelease latest < stable current
            ("1.0.0", "nightly"),        // non-semver tag: never nag
        ];
        for (current, latest) in cases {
            assert!(
                !is_update_available(current, latest).unwrap(),
                "{latest} must not be newer than {current}"
            );
        }
    }

    #[test]
    fn prerelease_current_upgrades_to_stable() {
        assert!(is_update_available("1.0.0-rc.1", "v1.0.0").unwrap());
    }

    #[test]
    fn non_semver_current_is_an_error() {
        assert!(is_update_available("dev", "v1.0.0").is_err());
        assert!(is_update_available("", "v1.0.0").is_err());
    }

    // Injected-fetch checks: the full pipeline over canned bodies.

    #[tokio::test]
    async fn check_with_reports_an_available_update() {
        let status = check_with("1.0.0", || async { Ok(RELEASE_JSON.to_string()) })
            .await
            .unwrap();
        assert_eq!(status.current, "1.0.0");
        assert_eq!(status.latest, "1.2.0");
        assert!(status.update_available);
        assert_eq!(
            status.url,
            "https://github.com/Rastrian/spore-tunnel-gui/releases/tag/v1.2.0"
        );
    }

    #[tokio::test]
    async fn check_with_on_the_latest_version_is_calm() {
        let status = check_with("1.2.0", || async { Ok(RELEASE_JSON.to_string()) })
            .await
            .unwrap();
        assert!(!status.update_available);
        // url still points at the release page (About/help links use it).
        assert!(status.url.contains("releases/tag/v1.2.0"));
    }

    #[tokio::test]
    async fn check_with_propagates_fetch_and_parse_errors() {
        let fetch_err = check_with("1.0.0", || async {
            Err("Update check failed: HTTP 503".to_string())
        })
        .await
        .unwrap_err();
        assert_eq!(fetch_err, "Update check failed: HTTP 503");

        let parse_err = check_with("1.0.0", || async { Ok("nope".to_string()) })
            .await
            .unwrap_err();
        assert!(parse_err.contains("Invalid release response"));
    }
}
