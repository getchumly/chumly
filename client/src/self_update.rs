//! Updating the client in place (design sections 45–48).
//!
//! The release file for this system is downloaded next to the executable,
//! its minisign signature is checked against the key built into the client,
//! and the signed comment must name this version and this file, so an older
//! signed file cannot pass for a newer one. Then the file replaces the running
//! executable. The running process goes on as it is; the next start (or
//! `Installer::restart`) runs the new version. Nothing is installed if any
//! step fails.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use shared::release::{self, DownloadError, Release};

/// The release signing key: minisign.pub at the repository root.
const PUBLIC_KEY: &str = include_str!("../../minisign.pub");
/// Larger files are refused: the client is under 20 MB.
const MAX_DOWNLOAD: u64 = 200 * 1024 * 1024;
const MAX_SIGNATURE: u64 = 4 * 1024;
/// Download progress reaches the UI at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(100);
/// Temporary files next to the executable start with this.
const TEMP_PREFIX: &str = ".chumly-update";

/// Where an update got. One state for the start screen banner and the
/// Updates block in Settings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Install {
    #[default]
    Idle,
    Downloading {
        done: u64,
        total: Option<u64>,
    },
    /// Checking the signature of the downloaded file of `total` bytes.
    Checking {
        total: u64,
    },
    /// Installed: the executable on disk is `version`, the running one is
    /// still the old one until restarted.
    Ready {
        version: String,
    },
    Failed(Failure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// No network, GitHub unavailable, the file missing from the release.
    Network,
    /// The file is not signed with our key, or not as this version's file.
    Signature,
    /// The executable's folder is not writable.
    Folder,
}

/// The release file for this system: "chumly-client-windows-x64-0.2.0.exe"
/// or "chumly-client-linux-x64-0.2.0.tar.gz"; `None` where there is none.
pub fn asset_name(version: &str) -> Option<String> {
    let (kind, ext) = if cfg!(windows) {
        ("client-windows-x64", "exe")
    } else if cfg!(target_os = "linux") {
        ("client-linux-x64", "tar.gz")
    } else {
        return None;
    };
    Some(format!("{}-{kind}-{version}.{ext}", shared::APP_NAME.to_lowercase()))
}

/// The comment the release workflow signs every file with.
fn signed_comment(version: &str, file: &str) -> String {
    format!("{} v{version}: {file}", shared::APP_NAME)
}

/// Checks that `data` is signed with the release key, as `file` of `version`.
pub fn verify(data: &[u8], signature: &str, version: &str, file: &str) -> Result<(), String> {
    // The key file is "untrusted comment: …" and the key; its line endings vary.
    let key_line = PUBLIC_KEY.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or_default();
    let key = minisign_verify::PublicKey::from_base64(key_line).map_err(|e| format!("release key: {e}"))?;
    let sig = minisign_verify::Signature::decode(signature).map_err(|e| format!("signature file: {e}"))?;
    key.verify(data, &sig, false).map_err(|e| format!("signature: {e}"))?;
    let expected = signed_comment(version, file);
    if sig.trusted_comment() != expected {
        return Err(format!("signed as {:?}, expected {expected:?}", sig.trusted_comment()));
    }
    Ok(())
}

/// Runs updates in the background and keeps their state.
pub struct Installer {
    state: Arc<Mutex<Install>>,
    cancel: Arc<AtomicBool>,
    /// The executable as it was at start: after the swap, `current_exe` on
    /// Linux points at the deleted old file.
    exe: Option<PathBuf>,
}

impl Default for Installer {
    fn default() -> Self {
        let exe = std::env::current_exe().ok();
        if let Some(dir) = exe.as_deref().and_then(Path::parent) {
            remove_leftovers(dir);
        }
        Self { state: Default::default(), cancel: Default::default(), exe }
    }
}

impl Installer {
    pub fn state(&self) -> Install {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set(&self, s: Install) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = s;
    }

    /// Downloading or checking.
    pub fn busy(&self) -> bool {
        matches!(self.state(), Install::Downloading { .. } | Install::Checking { .. })
    }

    /// Starts updating to `r` in the background (no-op while one is running).
    pub fn start(&mut self, r: &Release, ctx: &egui::Context) {
        if self.busy() || matches!(self.state(), Install::Ready { .. }) {
            return;
        }
        let Some(exe) = self.exe.clone() else {
            self.set(Install::Failed(Failure::Folder));
            return;
        };
        // A fresh flag: a cancelled download that is still running keeps its own.
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        self.set(Install::Downloading { done: 0, total: None });
        let state = self.state.clone();
        let r = r.clone();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("self-update".into()).spawn(move || {
            let result = run(&r, &exe, &state, &cancel, &ctx);
            if cancel.load(Ordering::Relaxed) {
                // The UI went back to "available" already.
                return;
            }
            let s = match result {
                Ok(()) => {
                    tracing::info!("updated to {}, restart to run it", r.version);
                    Install::Ready { version: r.version.clone() }
                }
                Err(f) => Install::Failed(f),
            };
            *state.lock().unwrap_or_else(|e| e.into_inner()) = s;
            ctx.request_repaint();
        });
        if spawned.is_err() {
            self.set(Install::Failed(Failure::Network));
        }
    }

    /// Stops a running download; back to "available".
    pub fn cancel(&mut self) {
        if self.busy() {
            self.cancel.store(true, Ordering::Relaxed);
            self.set(Install::Idle);
        }
    }

    /// Forgets a failure (× on the banner).
    pub fn dismiss(&mut self) {
        if matches!(self.state(), Install::Failed(_)) {
            self.set(Install::Idle);
        }
    }

    /// The folder of the executable, for the "can't write here" message.
    pub fn folder(&self) -> String {
        self.exe.as_deref().and_then(Path::parent).map(|d| d.display().to_string()).unwrap_or_default()
    }

    /// Starts the executable (by now the new version) with the same
    /// arguments. The caller then closes the app.
    pub fn restart(&self) -> std::io::Result<()> {
        let exe = self.exe.as_deref().ok_or_else(|| std::io::Error::other("executable path unknown"))?;
        std::process::Command::new(exe).args(std::env::args_os().skip(1)).spawn().map(|_| ())
    }
}

/// Removes these files when dropped: temporary files of a failed step.
struct Cleanup(Vec<PathBuf>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Temporary files a crash or a killed process left next to the executable.
fn remove_leftovers(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        if e.file_name().to_string_lossy().starts_with(TEMP_PREFIX) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Download, check, (unpack,) replace. The state is `Downloading` /
/// `Checking` while it runs; the caller sets the result.
fn run(
    r: &Release,
    exe: &Path,
    state: &Mutex<Install>,
    cancel: &AtomicBool,
    ctx: &egui::Context,
) -> Result<(), Failure> {
    let set = |s: Install| *state.lock().unwrap_or_else(|e| e.into_inner()) = s;
    let name = asset_name(&r.version).ok_or(Failure::Network)?;
    let find = |n: &str| r.assets.iter().find(|(a, _)| a == n).map(|(_, url)| url.clone());
    let (Some(url), Some(sig_url)) = (find(&name), find(&format!("{name}.minisig"))) else {
        tracing::warn!("update: {name} or its signature is missing from release {}", r.version);
        return Err(Failure::Network);
    };
    let dir = exe.parent().ok_or(Failure::Folder)?;
    let part = dir.join(format!("{TEMP_PREFIX}.part"));
    let mut cleanup = Cleanup(vec![part.clone()]);
    // Creating the file is also the check that the folder is writable.
    let mut file = File::create(&part).map_err(|e| {
        tracing::warn!("update: cannot write to {}: {e}", dir.display());
        Failure::Folder
    })?;

    let net = |e: DownloadError| {
        if let DownloadError::Failed(e) = e {
            tracing::warn!("update download: {e:#}");
        }
        Failure::Network
    };
    let mut sig = Vec::new();
    release::download(&sig_url, MAX_SIGNATURE, &mut sig, |_, _| !cancel.load(Ordering::Relaxed)).map_err(net)?;
    let mut shown = Instant::now() - PROGRESS_EVERY;
    let size = release::download(&url, MAX_DOWNLOAD, &mut file, |done, total| {
        if shown.elapsed() >= PROGRESS_EVERY && !cancel.load(Ordering::Relaxed) {
            shown = Instant::now();
            set(Install::Downloading { done, total });
            ctx.request_repaint();
        }
        !cancel.load(Ordering::Relaxed)
    })
    .map_err(net)?;
    file.sync_all().map_err(|_| Failure::Folder)?;
    drop(file);

    set(Install::Checking { total: size });
    ctx.request_repaint();
    let data = std::fs::read(&part).map_err(|_| Failure::Folder)?;
    let sig = String::from_utf8_lossy(&sig);
    verify(&data, &sig, &r.version, &name).map_err(|e| {
        tracing::error!("update {} rejected: {e}", r.version);
        Failure::Signature
    })?;

    let new = unpack(&data, &part, dir)?;
    cleanup.0.push(new.clone());
    self_replace::self_replace(&new).map_err(|e| {
        tracing::warn!("update: replacing {}: {e}", exe.display());
        Failure::Folder
    })?;
    Ok(())
}

/// The new executable: the downloaded file itself on Windows.
#[cfg(not(target_os = "linux"))]
fn unpack(_data: &[u8], part: &Path, _dir: &Path) -> Result<PathBuf, Failure> {
    Ok(part.to_path_buf())
}

/// The new executable: `chumly` from the downloaded .tar.gz.
#[cfg(target_os = "linux")]
fn unpack(data: &[u8], _part: &Path, dir: &Path) -> Result<PathBuf, Failure> {
    use std::os::unix::fs::PermissionsExt;
    let bin = shared::APP_NAME.to_lowercase();
    let out = dir.join(format!("{TEMP_PREFIX}.new"));
    let fail = |e: std::io::Error| {
        tracing::warn!("update: unpacking: {e}");
        Failure::Network
    };
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(data));
    for entry in archive.entries().map_err(fail)? {
        let mut entry = entry.map_err(fail)?;
        let is_bin = entry.path().map_err(fail)?.file_name().is_some_and(|n| n == bin.as_str());
        if is_bin && entry.header().entry_type().is_file() {
            let mut f = File::create(&out).map_err(|_| Failure::Folder)?;
            std::io::copy(&mut entry, &mut f).map_err(fail)?;
            f.set_permissions(std::fs::Permissions::from_mode(0o755)).map_err(|_| Failure::Folder)?;
            return Ok(out);
        }
    }
    tracing::warn!("update: no {bin} in the archive");
    Err(Failure::Network)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A real file from release v0.1.0 and its signature.
    const SUMS: &[u8] = include_bytes!("../testdata/SHA256SUMS");
    const SUMS_SIG: &str = include_str!("../testdata/SHA256SUMS.minisig");

    #[test]
    fn release_signature_is_accepted() {
        assert_eq!(verify(SUMS, SUMS_SIG, "0.1.0", "SHA256SUMS"), Ok(()));
    }

    #[test]
    fn tampered_file_is_rejected() {
        let mut data = SUMS.to_vec();
        data[0] ^= 1;
        assert!(verify(&data, SUMS_SIG, "0.1.0", "SHA256SUMS").is_err());
    }

    #[test]
    fn signed_file_of_another_version_or_name_is_rejected() {
        // Correctly signed, but as 0.1.0's SHA256SUMS: not a newer release, not the client.
        assert!(verify(SUMS, SUMS_SIG, "0.2.0", "SHA256SUMS").is_err());
        assert!(verify(SUMS, SUMS_SIG, "0.1.0", "chumly-client-windows-x64-0.1.0.exe").is_err());
    }

    #[test]
    fn garbage_signature_is_rejected() {
        assert!(verify(SUMS, "not a signature", "0.1.0", "SHA256SUMS").is_err());
    }

    #[test]
    fn asset_names_match_the_release_workflow() {
        let name = asset_name("1.2.3").unwrap();
        if cfg!(windows) {
            assert_eq!(name, "chumly-client-windows-x64-1.2.3.exe");
        } else {
            assert_eq!(name, "chumly-client-linux-x64-1.2.3.tar.gz");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unpacks_the_executable_from_the_archive() {
        let dir = tempfile::tempdir().unwrap();
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        for (name, body) in [("LICENSE", &b"license"[..]), ("chumly", &b"new binary"[..])] {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(&mut h, name, body).unwrap();
        }
        let gz = tar.into_inner().unwrap().finish().unwrap();
        let out = unpack(&gz, Path::new("unused"), dir.path()).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"new binary");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&out).unwrap().permissions().mode() & 0o777, 0o755);
    }

    #[test]
    fn leftovers_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(format!("{TEMP_PREFIX}.part")), b"x").unwrap();
        std::fs::write(dir.path().join("chumly.toml"), b"keep").unwrap();
        remove_leftovers(dir.path());
        assert!(!dir.path().join(format!("{TEMP_PREFIX}.part")).exists());
        assert!(dir.path().join("chumly.toml").exists());
    }
}
