//! Client update check: asks GitHub for the latest release at launch and
//! once a day (if on in the settings), or on demand ("Check now"). Installing
//! a newer release is `crate::self_update`; manual download stays as the
//! fallback.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use shared::release::{self, Release};

use crate::self_update::Installer;

/// Automatic checks: at launch and then this often.
const AUTO_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// Where the last check got.
#[derive(Debug, Clone, Default)]
pub enum Status {
    #[default]
    Idle,
    Checking,
    /// The latest release is not newer than this build.
    UpToDate,
    /// A newer release (it may be skipped by the user, see `Updates::offer`).
    Available(Release),
    /// No network, GitHub unavailable, no releases yet: shown quietly.
    Failed,
}

#[derive(Default)]
pub struct Updates {
    pub status: Status,
    rx: Option<mpsc::Receiver<anyhow::Result<Release>>>,
    last_auto: Option<Instant>,
    /// × on the start screen banner: hidden until the next launch.
    pub banner_closed: bool,
    /// The "What's new" dialog.
    pub notes_open: bool,
    /// Download, check and install of the offered release.
    pub install: Installer,
    /// The latest release, newer or not: "What's new" after an update.
    pub latest: Option<Release>,
    /// This launch runs a newer version than the last one: "Updated to …"
    /// on the start screen until closed or the first connect.
    pub just_updated: bool,
    /// "Restart now? You will be disconnected" (pressed Restart while connected).
    pub confirm_restart: bool,
}

impl Updates {
    /// Picks up a finished check and starts an automatic one when due.
    pub fn tick(&mut self, ctx: &egui::Context, auto: bool) {
        if let Some(rx) = &self.rx
            && let Ok(result) = rx.try_recv()
        {
            self.rx = None;
            if let Ok(r) = &result {
                self.latest = Some(r.clone());
            }
            self.status = match result {
                Ok(r) if release::is_newer(&r.version, shared::VERSION) => {
                    tracing::info!("update available: {} ({})", r.version, r.page);
                    Status::Available(r)
                }
                Ok(r) => {
                    tracing::info!("up to date (latest release {})", r.version);
                    Status::UpToDate
                }
                Err(e) => {
                    tracing::info!("update check failed: {e:#}");
                    Status::Failed
                }
            };
        }
        let due = self.last_auto.is_none_or(|t| t.elapsed() >= AUTO_EVERY);
        if auto && due && self.rx.is_none() {
            self.last_auto = Some(Instant::now());
            self.check(ctx);
        }
        if auto && let Some(t) = self.last_auto {
            ctx.request_repaint_after(AUTO_EVERY.saturating_sub(t.elapsed()));
        }
    }

    /// Starts a check in the background (no-op while one is running).
    pub fn check(&mut self, ctx: &egui::Context) {
        if self.rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            let _ = tx.send(release::fetch_latest());
            ctx.request_repaint();
        });
        if spawned.is_ok() {
            self.rx = Some(rx);
            self.status = Status::Checking;
        }
    }

    /// The newer release, unless the user skipped that version.
    pub fn offer(&self, skipped: Option<&str>) -> Option<&Release> {
        match &self.status {
            Status::Available(r) if skipped != Some(r.version.as_str()) => Some(r),
            _ => None,
        }
    }
}

/// The release page of the version this build is: "What's new" after an
/// update when no check ran yet.
pub fn current_release_page() -> String {
    format!("https://github.com/{}/releases/tag/v{}", release::RELEASES_REPO, shared::VERSION)
}

/// The file to download for this system, or the release page.
pub fn download_url(r: &Release) -> &str {
    let asset = if cfg!(windows) {
        r.asset("client-windows-x64", ".exe")
    } else if cfg!(target_os = "linux") {
        r.asset("client-linux-x64", ".tar.gz")
    } else {
        None
    };
    asset.unwrap_or(&r.page)
}
