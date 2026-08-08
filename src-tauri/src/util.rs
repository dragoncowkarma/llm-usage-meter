//! Small shared helpers: time, file metadata, and bounded tail reads.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local, TimeZone, Utc};

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Last-modified time of a path, in unix seconds.
pub fn mtime_unix(path: &Path) -> Option<i64> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    modified
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

/// Parse an RFC 3339 / ISO 8601 timestamp into unix seconds.
pub fn parse_iso8601(s: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp())
        .or_else(|| {
            // Some writers emit a trailing 'Z' with sub-microsecond precision or
            // a space separator; normalise the common variants before giving up.
            let normalised = s.replacen(' ', "T", 1);
            DateTime::parse_from_rfc3339(&normalised)
                .ok()
                .map(|dt| dt.timestamp())
        })
}

/// Unix seconds for local midnight today — the boundary for "today's usage".
///
/// Deliberately *local* midnight: the user reads this meter against their own
/// working day, not UTC.
pub fn local_day_start(now: i64) -> i64 {
    let dt = match Utc.timestamp_opt(now, 0) {
        chrono::LocalResult::Single(dt) => dt,
        _ => return now - (now.rem_euclid(86_400)),
    };
    let local = dt.with_timezone(&Local);
    let naive_midnight = local.date_naive().and_hms_opt(0, 0, 0);
    match naive_midnight {
        Some(nm) => match local.timezone().from_local_datetime(&nm) {
            chrono::LocalResult::Single(d) => d.timestamp(),
            chrono::LocalResult::Ambiguous(a, _) => a.timestamp(),
            // Midnight can genuinely not exist on a DST spring-forward day.
            chrono::LocalResult::None => now - (now.rem_euclid(86_400)),
        },
        None => now - (now.rem_euclid(86_400)),
    }
}

/// Read at most `max_bytes` from the end of a file.
///
/// Codex writes one rollout file per session and only the *last* `token_count`
/// event matters, so reading a 4 MiB transcript front-to-back for every refresh
/// would be pure waste. Reads are aligned to the first newline so callers never
/// see a partial JSON line.
pub fn read_tail(path: &Path, max_bytes: u64) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();

    if len <= max_bytes {
        // Lossy, like the tail branch below: one stray byte in a transcript
        // should cost that byte, not the whole file's worth of usage data.
        let mut bytes = Vec::with_capacity(len as usize);
        file.read_to_end(&mut bytes)?;
        return Ok(String::from_utf8_lossy(&bytes).into_owned());
    }

    file.seek(SeekFrom::Start(len - max_bytes))?;
    let mut bytes = Vec::with_capacity(max_bytes as usize);
    file.read_to_end(&mut bytes)?;

    // Drop everything before the first newline: it is a partial line.
    let start = bytes
        .iter()
        .position(|b| *b == b'\n')
        .map(|i| i + 1)
        .unwrap_or(0);

    Ok(String::from_utf8_lossy(&bytes[start..]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_the_timestamp_shapes_these_tools_emit() {
        // Claude Code transcripts: UTC with a 'Z' suffix and sub-second digits.
        assert_eq!(parse_iso8601("2026-08-07T15:07:47.235Z"), Some(1_786_115_267));
        // Antigravity metadata carries an explicit offset and nanosecond
        // precision — the same instant must land on the same epoch second.
        assert_eq!(
            parse_iso8601("2026-08-08T00:07:47.235000000+09:00"),
            Some(1_786_115_267),
        );
        assert_eq!(parse_iso8601("not a date"), None);
        assert_eq!(parse_iso8601(""), None);
    }

    #[test]
    fn day_start_is_midnight_and_within_a_day() {
        let now = now_unix();
        let start = local_day_start(now);
        assert!(start <= now, "day start must not be in the future");
        assert!(now - start < 86_400 + 3_600, "day start within ~one day");
    }

    #[test]
    fn tail_read_never_returns_a_partial_line() {
        let dir = std::env::temp_dir().join("lum-tail-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rollout.jsonl");
        let mut f = File::create(&path).unwrap();
        for i in 0..500 {
            writeln!(f, "{{\"n\":{i},\"pad\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}}").unwrap();
        }
        drop(f);

        let tail = read_tail(&path, 512).unwrap();
        assert!(!tail.is_empty());
        for line in tail.lines() {
            // Every surviving line must be complete, parseable JSON.
            serde_json::from_str::<serde_json::Value>(line)
                .unwrap_or_else(|e| panic!("partial line leaked through: {line:?} ({e})"));
        }
        assert!(tail.contains("\"n\":499"), "tail must include the last line");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tail_read_returns_whole_file_when_small() {
        let dir = std::env::temp_dir().join("lum-tail-small");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("small.jsonl");
        std::fs::write(&path, "{\"a\":1}\n").unwrap();
        assert_eq!(read_tail(&path, 1_000_000).unwrap(), "{\"a\":1}\n");
        std::fs::remove_dir_all(&dir).ok();
    }
}
