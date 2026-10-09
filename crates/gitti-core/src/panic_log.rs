//! Panics are written to `<config dir>/logs/panic.log`: gitti has no terminal
//! when started from a launcher (and none at all on Windows), so the default hook's stderr
//! message is lost. Shared by gitti and gitty; the previous hook still runs afterwards.
//! The file is owner-only (`0600` in a `0700` directory) and the message is redacted
//! ([`crate::redact`]) — a panic message can quote a remote URL.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The log is rotated (`.1`) beyond this size.
const MAX_BYTES: u64 = 1024 * 1024;

/// Where panics are logged.
pub fn log_file() -> Option<PathBuf> {
    Some(crate::paths::config_dir()?.join("logs").join("panic.log"))
}

/// Installs the hook; `app` and `version` head each entry (`gitti 0.1.0`).
pub fn install(app: &'static str, version: &'static str) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(path) = log_file() {
            let thread = std::thread::current();
            let entry = format_entry(
                &format!("{app} {version}"),
                SystemTime::now(),
                thread.name().unwrap_or("unnamed"),
                info.payload_as_str()
                    .unwrap_or("(non-string panic payload)"),
                info.location().map(|l| l.to_string()).as_deref(),
                &std::backtrace::Backtrace::force_capture().to_string(),
            );
            let _ = append(&path, &entry);
        }
        previous(info);
    }));
}

fn append(path: &Path, entry: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        crate::paths::create_private_dir(dir)?;
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::rename(path, path.with_extension("log.1"));
    }
    crate::paths::open_private_append(path)?.write_all(entry.as_bytes())
}

/// One log entry: a header line, the message, the backtrace, an empty line.
fn format_entry(
    app: &str,
    at: SystemTime,
    thread: &str,
    message: &str,
    location: Option<&str>,
    backtrace: &str,
) -> String {
    let message = crate::redact::redact(message);
    let mut s = format!(
        "=== {} {app} panicked in thread '{thread}' at {}\n{}\n",
        utc_timestamp(at),
        location.unwrap_or("?"),
        message.trim_end()
    );
    s.push_str(backtrace.trim_end());
    s.push_str("\n\n");
    s
}

/// `2026-10-05 14:03:59 UTC` (no date crate in core).
pub(crate) fn utc_timestamp(at: SystemTime) -> String {
    let secs = at
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Howard Hinnant's civil_from_days.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[gitti_test::test]
    fn timestamps() {
        assert_eq!(utc_timestamp(UNIX_EPOCH), "1970-01-01 00:00:00 UTC");
        let t = UNIX_EPOCH + Duration::from_secs(1_791_208_439);
        assert_eq!(utc_timestamp(t), "2026-10-05 13:53:59 UTC");
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(utc_timestamp(leap), "2000-02-29 00:00:00 UTC");
    }

    #[gitti_test::test]
    fn entries_are_redacted_and_appended_owner_only() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("logs/panic.log");
        let entry = format_entry(
            "gitti 0.1.0",
            UNIX_EPOCH,
            "main",
            "fetch from https://u:tok@h/r failed",
            Some("src/x.rs:1:2"),
            "   0: frame\n",
        );
        assert_eq!(
            entry,
            "=== 1970-01-01 00:00:00 UTC gitti 0.1.0 panicked in thread 'main' at \
             src/x.rs:1:2\nfetch from https://u:***@h/r failed\n   0: frame\n\n"
        );
        append(&path, &entry).unwrap();
        append(&path, &entry).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), entry.repeat(2));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(path.parent().unwrap()), 0o700);
        }
    }
}
