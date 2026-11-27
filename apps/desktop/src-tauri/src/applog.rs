//! The app's log file, so a run on someone else's computer says what happened: start-up, where
//! the engine is, whether a backend is set up (its host only), the windows, sign-in's steps,
//! the engine's start, exit and log lines, every permission check, request and reset with its
//! answer, and errors. Never a token, a pass, a call link or clipboard text: what is written
//! here is only what the lines below choose to write.
//!
//! It is `~/Library/Logs/TwinCursor/twincursor.log` on macOS and
//! `%LOCALAPPDATA%\dev.twincursor.desktop\logs\twincursor.log` on Windows
//! ([`crate::platform::default_log_file`]), or the file `TWINCURSOR_LOG_FILE` names. A log over
//! 2 MB at start-up is moved to `twincursor.log.old` first, so it never grows without end. Each
//! line starts with the time in UTC (`2026-11-18 11:06:40Z`) and the seconds since start.

use std::fmt::Display;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const LOG_FILE_ENV: &str = "TWINCURSOR_LOG_FILE";
const KEEP_BYTES: u64 = 2 * 1024 * 1024;

struct Log {
    file: Mutex<File>,
    start: Instant,
}

static LOG: OnceLock<Option<Log>> = OnceLock::new();

/// Where the log goes for this run, if anywhere.
pub fn path_for_run() -> Option<PathBuf> {
    std::env::var_os(LOG_FILE_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(crate::platform::default_log_file)
}

fn open(path: &Path) -> std::io::Result<File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > KEEP_BYTES) {
        let mut old = path.as_os_str().to_owned();
        old.push(".old");
        let _ = std::fs::rename(path, old);
    }
    OpenOptions::new().create(true).append(true).open(path)
}

/// Opens the log (once). Returns where it is, or why it couldn't be opened.
pub fn init() -> Option<Result<PathBuf, String>> {
    let path = path_for_run()?;
    let mut result = Ok(path.clone());
    LOG.get_or_init(|| match open(&path) {
        Ok(file) => Some(Log {
            file: Mutex::new(file),
            start: Instant::now(),
        }),
        Err(e) => {
            result = Err(format!("{}: {e}", path.display()));
            None
        }
    });
    Some(result)
}

/// The wall clock as UTC `YYYY-MM-DD HH:MM:SSZ`, without a date crate.
pub fn utc(at: SystemTime) -> String {
    let secs = at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Civil from days (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// One line, with the time and the seconds since start. Nothing happens without a log.
pub fn line(text: impl Display) {
    let Some(Some(log)) = LOG.get() else {
        return;
    };
    let elapsed = log.start.elapsed().as_secs_f64();
    let stamp = utc(SystemTime::now());
    let text = text.to_string();
    let mut file = log.file.lock().unwrap_or_else(|e| e.into_inner());
    for (i, part) in text.lines().enumerate() {
        let lead = if i == 0 { "" } else { "  " };
        let _ = writeln!(file, "{stamp} +{elapsed:.3}s {lead}{part}");
    }
    let _ = file.flush();
}

/// The host of an address, for saying which backend without the rest of it.
pub fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?', '#']).next().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn utc_dates() {
        assert_eq!(utc(UNIX_EPOCH), "1970-01-01 00:00:00Z");
        let t = UNIX_EPOCH + Duration::from_secs(1_795_000_000);
        assert_eq!(utc(t), "2026-11-18 11:06:40Z");
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(utc(leap), "2000-02-29 00:00:00Z");
    }

    #[test]
    fn only_the_host_is_kept() {
        assert_eq!(
            host_of("https://abc.supabase.co/rest/v1?x=1"),
            "abc.supabase.co"
        );
        assert_eq!(host_of("http://127.0.0.1:54321"), "127.0.0.1:54321");
        assert_eq!(host_of("abc.test/x"), "abc.test");
    }

    #[test]
    fn a_big_log_is_moved_aside() {
        let dir = std::env::temp_dir().join(format!("twincursor-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("logs").join("twincursor.log");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, vec![b'x'; (KEEP_BYTES + 1) as usize]).unwrap();
        let mut file = open(&path).unwrap();
        writeln!(file, "fresh").unwrap();
        drop(file);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fresh\n");
        assert!(dir.join("logs").join("twincursor.log.old").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
