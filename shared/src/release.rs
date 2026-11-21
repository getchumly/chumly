//! Releases on GitHub: the latest one and whether it is newer than this
//! build. Blocking: call `fetch_latest` off the UI / async threads.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

/// The GitHub repository releases are published in ("owner/name"). GitHub
/// redirects the API of a renamed repository, so builds with an old name
/// keep working after a rename.
pub const RELEASES_REPO: &str = "getchumly/chumly";

/// The GitHub API address of the latest release (drafts and pre-releases
/// are not "latest").
pub fn latest_release_api() -> String {
    format!("https://api.github.com/repos/{RELEASES_REPO}/releases/latest")
}

/// User-Agent for the request: GitHub requires one. Only the app and its
/// version, nothing about the user.
pub fn user_agent() -> String {
    format!("{}/{}", crate::APP_NAME, crate::VERSION)
}

/// A published release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// "0.2.0", without the tag's "v".
    pub version: String,
    /// The release page on GitHub.
    pub page: String,
    /// Release notes (markdown): the CHANGELOG section of the version.
    pub notes: String,
    /// Publication date, "YYYY-MM-DD".
    pub date: String,
    /// Files: (name, download URL).
    pub assets: Vec<(String, String)>,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

impl Release {
    /// From the GitHub API answer; `None` if it is not a release with a
    /// version tag.
    pub fn parse(json: &str) -> Option<Self> {
        let r: GhRelease = serde_json::from_str(json).ok()?;
        let version = r.tag_name.trim_start_matches('v').to_string();
        parse_version(&version)?;
        let date = r.published_at.unwrap_or_default().chars().take(10).collect();
        let assets = r.assets.into_iter().map(|a| (a.name, a.browser_download_url)).collect();
        Some(Self { version, page: r.html_url, notes: r.body.unwrap_or_default(), date, assets })
    }

    /// The download URL of the file whose name contains `part` and ends
    /// with `ext`, e.g. ("client-windows-x64", ".exe").
    pub fn asset(&self, part: &str, ext: &str) -> Option<&str> {
        self.assets.iter().find(|(n, _)| n.contains(part) && n.ends_with(ext)).map(|(_, u)| u.as_str())
    }
}

/// HTTPS with the system's certificate store, our User-Agent.
fn agent(config: ureq::config::ConfigBuilder<ureq::typestate::AgentScope>) -> ureq::Agent {
    let tls = TlsConfig::builder()
        .provider(TlsProvider::Rustls)
        .root_certs(RootCerts::PlatformVerifier)
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    config.tls_config(tls).user_agent(user_agent()).build().into()
}

/// Asks GitHub for the latest release (one HTTPS request, 10 s at most).
/// The error is for the log: the user is shown only "couldn't check".
pub fn fetch_latest() -> anyhow::Result<Release> {
    let agent = agent(ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))));
    let body = agent
        .get(&latest_release_api())
        .header("Accept", "application/vnd.github+json")
        .call()?
        .body_mut()
        .read_to_string()?;
    Release::parse(&body).ok_or_else(|| anyhow::anyhow!("unreadable release answer"))
}

/// Why a download stopped.
#[derive(Debug)]
pub enum DownloadError {
    /// `progress` asked to stop.
    Cancelled,
    /// Network, HTTP or write error, or the file is larger than allowed.
    Failed(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for DownloadError {
    fn from(e: E) -> Self {
        Self::Failed(e.into())
    }
}

/// Downloads `url` (following redirects) into `out`, at most `max` bytes.
/// `progress(done, total)` is called before every chunk; returning `false`
/// stops the download. Blocking. Returns the number of bytes written.
pub fn download(
    url: &str,
    max: u64,
    out: &mut dyn std::io::Write,
    mut progress: impl FnMut(u64, Option<u64>) -> bool,
) -> Result<u64, DownloadError> {
    use std::io::Read;
    // No overall limit: a slow connection may need minutes for the client.
    // A server that stops answering is caught by the receive timeouts.
    let agent = agent(
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .timeout_recv_body(Some(Duration::from_secs(15 * 60))),
    );
    let mut resp = agent.get(url).call()?;
    let total = resp.body().content_length();
    if total.is_some_and(|t| t > max) {
        return Err(anyhow::anyhow!("file too large: {total:?} bytes").into());
    }
    let mut reader = resp.body_mut().with_config().limit(max).reader();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    loop {
        if !progress(done, total) {
            return Err(DownloadError::Cancelled);
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        done += n as u64;
    }
    progress(done, total);
    Ok(done)
}

/// "1.2.3" (optionally "1.2.3-beta" or with a "v") as numbers plus whether
/// it is a pre-release.
fn parse_version(v: &str) -> Option<([u64; 3], bool)> {
    let v = v.trim().trim_start_matches('v');
    let (core, pre) = match v.split_once('-') {
        Some((c, _)) => (c, true),
        None => (v, false),
    };
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let n = [parts.next()??, parts.next()??, parts.next()??];
    if parts.next().is_some() {
        return None;
    }
    Some((n, pre))
}

/// Whether `candidate` is a newer version than `current`. A pre-release is
/// older than the same version without a suffix. Unreadable versions are
/// never newer.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some((a, a_pre)), Some((b, b_pre))) => a > b || (a == b && b_pre && !a_pre),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("0.10.0", "0.9.0"), "not as strings");
        assert!(is_newer("v1.0.0", "0.99.99"));
        assert!(is_newer("0.2.0", "0.2.0-beta"));
        assert!(!is_newer("0.2.0-beta", "0.2.0"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("dev", "0.1.0"));
        assert!(!is_newer("0.2", "0.1.0"));
    }

    #[test]
    fn github_answer() {
        let json = r####"{
            "tag_name": "v0.2.0", "html_url": "https://github.com/o/r/releases/tag/v0.2.0",
            "body": "### Added\n- Client: x", "published_at": "2026-11-18T10:00:00Z", "draft": false,
            "assets": [
                {"name": "chumly-client-windows-x64-0.2.0.zip", "browser_download_url": "https://x/zip"},
                {"name": "chumly-client-windows-x64-0.2.0.exe", "browser_download_url": "https://x/exe"},
                {"name": "chumly-client-linux-x64-0.2.0.tar.gz", "browser_download_url": "https://x/tgz"}
            ]
        }"####;
        let r = Release::parse(json).unwrap();
        assert_eq!(r.version, "0.2.0");
        assert_eq!(r.date, "2026-11-18");
        assert_eq!(r.asset("client-windows-x64", ".exe"), Some("https://x/exe"));
        assert_eq!(r.asset("client-linux-x64", ".tar.gz"), Some("https://x/tgz"));
        assert_eq!(r.asset("client-macos", ".dmg"), None);
        assert!(Release::parse(r#"{"tag_name": "nightly", "html_url": "x"}"#).is_none());
        assert!(Release::parse("not json").is_none());
    }

    /// The real request: `cargo test -p shared fetch -- --ignored --nocapture`.
    #[test]
    #[ignore = "network"]
    fn fetch_from_github() {
        println!("{:?}", fetch_latest().map(|r| (r.version, r.date, r.assets.len())));
    }
}
