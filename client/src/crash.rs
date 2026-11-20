//! Crash reports. A panic hook writes a report into `crashes/` next to
//! client.toml; on the next start the start screen offers it. Nothing is
//! sent anywhere: Chumly has no central service.
//!
//! A report has the version, the system, the time, the panic message with
//! its place in the code, the thread, the uptime and a backtrace. Release
//! builds have no symbols, so the backtrace is addresses only.

use std::fmt::Write as _;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Reports kept; older ones are deleted when a new one is written.
const KEEP: usize = 5;
const PREFIX: &str = "crash-";

/// The reports folder for this client.toml.
pub fn dir_for(config_path: &Path) -> PathBuf {
    config_path.with_file_name("crashes")
}

/// Writes a report on every panic, then runs the default hook (the message
/// to stderr).
pub fn install(dir: PathBuf) {
    let started = Instant::now();
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let text = report(info, started.elapsed(), &std::backtrace::Backtrace::force_capture().to_string());
        if let Err(e) = save(&dir, &text, &jiff::Zoned::now()) {
            eprintln!("failed to save the crash report: {e}");
        }
        default(info);
    }));
}

fn report(info: &PanicHookInfo, uptime: Duration, backtrace: &str) -> String {
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".into());
    let place = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
    let thread = std::thread::current().name().unwrap_or("unnamed").to_owned();
    format_report(&message, &place, &thread, uptime, backtrace, &jiff::Zoned::now())
}

fn format_report(
    message: &str,
    place: &str,
    thread: &str,
    uptime: Duration,
    backtrace: &str,
    now: &jiff::Zoned,
) -> String {
    let mut s = String::new();
    let _ =
        writeln!(s, "{} {} · {} {}", shared::APP_NAME, shared::VERSION, std::env::consts::OS, std::env::consts::ARCH);
    let _ = writeln!(s, "{}", now.strftime("%Y-%m-%d %H:%M:%S %:z"));
    let _ = writeln!(s, "panicked at {place}:");
    let _ = writeln!(s, "{message}");
    let mins = uptime.as_secs() / 60;
    let _ = writeln!(s, "thread: {thread} · uptime {} h {} min", mins / 60, mins % 60);
    let _ = writeln!(s);
    let _ = writeln!(s, "backtrace:");
    let _ = write!(s, "{backtrace}");
    s
}

/// Writes the report as crash-YYYYMMDD-HHMMSS.txt and keeps the newest `KEEP`.
fn save(dir: &Path, text: &str, now: &jiff::Zoned) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{PREFIX}{}.txt", now.strftime("%Y%m%d-%H%M%S")));
    shared::fsutil::write_atomic(&path, text.as_bytes())?;
    let mut old = reports(dir);
    old.truncate(old.len().saturating_sub(KEEP));
    for p in old {
        let _ = std::fs::remove_file(p);
    }
    Ok(path)
}

/// Report files, oldest first (the names sort by time).
fn reports(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut list: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(PREFIX) && n.ends_with(".txt"))
        })
        .collect();
    list.sort();
    list
}

/// The newest report: its file name and text.
pub fn latest(dir: &Path) -> Option<(String, String)> {
    let path = reports(dir).pop()?;
    let text = std::fs::read_to_string(&path).ok()?;
    Some((path.file_name()?.to_string_lossy().into_owned(), text))
}

/// Opens the folder in the system file manager.
pub fn open_folder(dir: &Path) {
    #[cfg(windows)]
    let program = "explorer";
    #[cfg(not(windows))]
    let program = "xdg-open";
    if let Err(e) = std::process::Command::new(program).arg(dir).spawn() {
        tracing::warn!("failed to open {}: {e}", dir.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(h: i8, m: i8, s: i8) -> jiff::Zoned {
        jiff::civil::date(2026, 11, 20).at(h, m, s, 0).to_zoned(jiff::tz::TimeZone::fixed(jiff::tz::offset(3))).unwrap()
    }

    #[test]
    fn report_text() {
        let text = format_report(
            "called `Option::unwrap()` on a `None` value",
            "client/src/audio/capture.rs:214:9",
            "audio-capture",
            Duration::from_secs(2 * 3600 + 14 * 60 + 5),
            "   0: 0x7ff6\n",
            &at(14, 2, 17),
        );
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with(&format!("Chumly {} · ", shared::VERSION)), "{text}");
        assert_eq!(lines[1], "2026-11-20 14:02:17 +03:00");
        assert_eq!(lines[2], "panicked at client/src/audio/capture.rs:214:9:");
        assert_eq!(lines[3], "called `Option::unwrap()` on a `None` value");
        assert_eq!(lines[4], "thread: audio-capture · uptime 2 h 14 min");
        assert_eq!(lines[6], "backtrace:");
    }

    #[test]
    fn keeps_the_newest_reports() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(latest(dir.path()), None);
        for i in 0..7 {
            save(dir.path(), &format!("report {i}"), &at(10, 0, i)).unwrap();
        }
        assert_eq!(reports(dir.path()).len(), KEEP);
        let (name, text) = latest(dir.path()).unwrap();
        assert_eq!(name, "crash-20261120-100006.txt");
        assert_eq!(text, "report 6");
        // Other files in the folder are left alone and not taken for reports.
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
        assert_eq!(latest(dir.path()).unwrap().0, name);
    }

    /// The real hook: a panicking thread leaves a report behind.
    #[test]
    fn panic_writes_a_report() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path().to_path_buf());
        let res =
            std::thread::Builder::new().name("crash-test".into()).spawn(|| panic!("test panic {}", 42)).unwrap().join();
        // Back to the default hook for the other tests.
        drop(std::panic::take_hook());
        assert!(res.is_err());
        let (_, text) = latest(dir.path()).expect("a report");
        assert!(text.contains("test panic 42"), "{text}");
        assert!(text.contains("client/src/crash.rs") || text.contains("client\\src\\crash.rs"), "{text}");
        assert!(text.contains("thread: crash-test"), "{text}");
        assert!(text.contains("backtrace:"), "{text}");
    }
}
