//! Antigravity (Gemini).
//!
//! **This provider reports an activity proxy, not billed usage — deliberately.**
//!
//! Unlike Claude Code and Codex, Antigravity keeps no local token ledger and no
//! local quota cache. Its conversation store is SQLite whose payloads are
//! protobuf blobs carrying timestamps and session ids but no token counts, and
//! its quota manager (`quota_manager.go`) refreshes from the server at runtime
//! without persisting the result. Inventing a token number here would be a
//! guess dressed up as a measurement, so instead we report what the filesystem
//! genuinely knows:
//!
//! * **Steps and conversations** from `cache/conversation_metadata.json`, which
//!   records `NumSteps` and `UpdatedAt` per conversation — a real request-volume
//!   proxy.
//! * **Observed quota exhaustion** from `RESOURCE_EXHAUSTED (code 429)` lines in
//!   the CLI logs, which is the one hard quota signal available locally.
//!
//! Snapshots are marked `Confidence::Heuristic` so the UI can say so plainly.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use walkdir::WalkDir;

use crate::error::Result;
use crate::model::*;
use crate::platform::first_existing;
use crate::providers::{Detection, ProviderContext, UsageProvider};
use crate::util;

pub struct AntigravityProvider;

/// Only the tail of each log is scanned; these files reach tens of MB.
const LOG_TAIL_BYTES: u64 = 512 * 1024;
/// A 429 older than this is history, not current state.
const EXHAUSTION_FRESH_SECS: i64 = 15 * 60;

/// The call site each rejection is finally reported from. Chosen deliberately
/// narrow: one rejected request produces several log lines as the error
/// propagates through the stack (`stream_handler.go`, `executor.go`, …), and
/// this is the one that appears exactly once per rejection, so it's what
/// makes the hit count mean "requests refused" rather than "lines logged".
const REJECTION_MARKER: &str = "errorreport.go";
/// A looser signal used only to detect that *some* rejection activity is
/// present even when `REJECTION_MARKER` doesn't match — e.g. because a CLI
/// update renamed its internal error-reporting file. This over-counts (many
/// lines per rejection) so it is never used as the hit count itself; it only
/// distinguishes "no rejections happened" from "rejections happened but this
/// build's marker is stale," so a broken marker degrades to a visible warning
/// instead of a silent 0.
const REJECTION_SIGNAL: &str = "RESOURCE_EXHAUSTED";

/// Candidate roots, newest layout first.
///
/// Antigravity has moved its state across releases (`~/.antigravity` →
/// `~/.gemini/antigravity`, then a separate `-cli` tree), and the IDE and CLI
/// keep separate directories. Probing an ordered candidate list through the
/// platform layer is what makes this survive both layout drift and the eventual
/// Windows port.
fn candidate_roots(ctx: &ProviderContext) -> Vec<PathBuf> {
    let p = &ctx.env.paths;
    let gemini = p.dot_dir("gemini");
    vec![
        gemini.join("antigravity-cli"),
        gemini.join("antigravity"),
        p.dot_dir("antigravity"),
        // Windows/macOS app-data locations for the desktop build.
        p.data_root().join("Antigravity"),
        p.config_root().join("Antigravity"),
    ]
}

impl UsageProvider for AntigravityProvider {
    fn id(&self) -> &'static str {
        "antigravity"
    }

    fn display_name(&self) -> &'static str {
        "Antigravity"
    }

    fn detect(&self, ctx: &ProviderContext) -> Detection {
        match first_existing(candidate_roots(ctx)) {
            Some(root) => Detection::Installed {
                root: root.display().to_string(),
            },
            None => Detection::NotInstalled,
        }
    }

    fn collect(&self, ctx: &ProviderContext) -> Result<UsageSnapshot> {
        let started = util::now_millis();
        let mut snap = UsageSnapshot::empty(self.id(), self.display_name(), ProviderStatus::Ok);
        snap.confidence = Confidence::Heuristic;

        let roots: Vec<PathBuf> = candidate_roots(ctx).into_iter().filter(|p| p.is_dir()).collect();
        if roots.is_empty() {
            snap.status = ProviderStatus::NotInstalled;
            snap.collect_ms = util::now_millis().saturating_sub(started);
            return Ok(snap);
        }

        let cutoff = ctx.now - (ctx.lookback_days as i64) * 86_400;
        let day_start = util::local_day_start(ctx.now);

        let mut activity = ActivityStats::default();
        let mut exhaustion: Option<i64> = None;
        let mut marker_possibly_stale = false;

        for root in &roots {
            let stats = read_activity(root, cutoff, day_start);
            activity.requests_total += stats.steps;
            activity.requests_today += stats.steps_today;
            activity.sessions_today += stats.conversations_today;
            activity.last_activity_at = max_opt(activity.last_activity_at, stats.last_activity_at);
            if stats.counted > 0 {
                snap.sources.push(SourceRef {
                    kind: stats.source_kind,
                    path: stats.source_path,
                    observed_at: stats.last_activity_at,
                });
            }

            let logs = scan_logs(&root.join("log"), cutoff);
            activity.rate_limit_hits += logs.hits;
            exhaustion = max_opt(exhaustion, logs.latest_hit_at);
            marker_possibly_stale |= logs.marker_possibly_stale;
            if logs.hits > 0 {
                snap.sources.push(SourceRef {
                    kind: "quota-error-log".into(),
                    path: root.join("log").display().to_string(),
                    observed_at: logs.latest_hit_at,
                });
            }
        }

        // The one genuine quota signal available locally: the backend told
        // Antigravity "no" recently. Reported only while it is still fresh, and
        // labelled as an observation rather than a quota reading.
        if let Some(at) = exhaustion {
            if ctx.now - at <= EXHAUSTION_FRESH_SECS {
                let mut q = QuotaWindow::new("observed_exhaustion", "Quota exhausted (observed)", 100.0);
                q.severity = Severity::Critical;
                snap.quotas.push(q);
                snap.notes.push(
                    "Antigravity was refused with HTTP 429 in the last 15 minutes.".into(),
                );
            } else {
                snap.notes.push(format!(
                    "Last quota rejection was {} ago.",
                    humanise_age(ctx.now - at)
                ));
            }
        }

        if activity.requests_total == 0 && activity.rate_limit_hits == 0 {
            snap.status = ProviderStatus::Degraded;
            snap.notes.push(format!(
                "Installed, but no Antigravity activity in the last {} days.",
                ctx.lookback_days
            ));
        }

        // Never let the one hard quota signal this provider has go silently
        // to zero because the CLI changed its log format underneath us — say
        // so, rather than reporting a clean 0 that looks like "no rejections".
        if marker_possibly_stale {
            snap.notes.push(
                "Some log lines look like quota rejections but didn't match this build's \
                 detection pattern — the rate-limit count above may be understated. If this \
                 persists, Antigravity's log format has likely changed."
                    .into(),
            );
        }

        snap.notes.push(
            "Antigravity stores no local token or quota ledger, so these are request counts, \
             not billed usage. Token totals and cost are unavailable by design rather than \
             estimated."
                .into(),
        );
        snap.activity = Some(activity);

        snap.collect_ms = util::now_millis().saturating_sub(started);
        Ok(snap)
    }
}

fn max_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

// ---------------------------------------------------------------------------
// Conversation metadata
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ConversationMetadata {
    #[serde(default)]
    conversations: BTreeMap<String, ConversationEntry>,
}

#[derive(Debug, Deserialize)]
struct ConversationEntry {
    #[serde(default)]
    summary: Option<ConversationSummary>,
    #[serde(default)]
    is_internal: bool,
}

#[derive(Debug, Deserialize)]
struct ConversationSummary {
    #[serde(rename = "NumSteps", default)]
    num_steps: u64,
    #[serde(rename = "UpdatedAt", default)]
    updated_at: Option<String>,
}

#[derive(Default)]
struct ActivityRead {
    steps: u64,
    steps_today: u64,
    conversations_today: u64,
    last_activity_at: Option<i64>,
    counted: u64,
    source_kind: String,
    source_path: String,
}

fn read_activity(root: &Path, cutoff: i64, day_start: i64) -> ActivityRead {
    let meta_path = root.join("cache").join("conversation_metadata.json");
    if let Ok(text) = std::fs::read_to_string(&meta_path) {
        if let Ok(meta) = serde_json::from_str::<ConversationMetadata>(&text) {
            let mut out = ActivityRead {
                source_kind: "conversation-metadata".into(),
                source_path: meta_path.display().to_string(),
                ..Default::default()
            };
            for entry in meta.conversations.values() {
                if entry.is_internal {
                    continue;
                }
                let Some(summary) = &entry.summary else {
                    continue;
                };
                let ts = summary.updated_at.as_deref().and_then(util::parse_iso8601);
                if ts.is_none_or(|t| t < cutoff) {
                    continue;
                }
                out.counted += 1;
                out.steps += summary.num_steps;
                out.last_activity_at = max_opt(out.last_activity_at, ts);
                if ts.is_some_and(|t| t >= day_start) {
                    out.steps_today += summary.num_steps;
                    out.conversations_today += 1;
                }
            }
            if out.counted > 0 {
                return out;
            }
        }
    }

    // Fallback for layouts with no metadata cache: count conversation databases
    // by mtime. Coarser — one unit per conversation, not per step — but real.
    let conv_dir = root.join("conversations");
    let mut out = ActivityRead {
        source_kind: "conversation-files".into(),
        source_path: conv_dir.display().to_string(),
        ..Default::default()
    };
    if conv_dir.is_dir() {
        for entry in WalkDir::new(&conv_dir)
            .max_depth(1)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_type().is_file())
            .filter(|e| e.path().extension().is_some_and(|x| x == "db"))
        {
            let Some(mtime) = util::mtime_unix(entry.path()) else {
                continue;
            };
            if mtime < cutoff {
                continue;
            }
            out.counted += 1;
            out.steps += 1;
            out.last_activity_at = max_opt(out.last_activity_at, Some(mtime));
            if mtime >= day_start {
                out.steps_today += 1;
                out.conversations_today += 1;
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Quota-rejection log scan
// ---------------------------------------------------------------------------

#[derive(Default)]
struct LogScan {
    hits: u64,
    latest_hit_at: Option<i64>,
    /// True if a file had `REJECTION_SIGNAL` lines but none matched
    /// `REJECTION_MARKER` — i.e. rejections likely happened but this build's
    /// detection pattern didn't catch them.
    marker_possibly_stale: bool,
}

fn scan_logs(log_dir: &Path, cutoff: i64) -> LogScan {
    let mut out = LogScan::default();
    if !log_dir.is_dir() {
        return out;
    }

    for entry in WalkDir::new(log_dir)
        .max_depth(1)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
    {
        let path = entry.path();
        let Some(mtime) = util::mtime_unix(path) else {
            continue;
        };
        if mtime < cutoff {
            continue;
        }
        let Ok(text) = util::read_tail(path, LOG_TAIL_BYTES) else {
            continue;
        };
        // Count only the top-level rejection, not the several downstream lines
        // each one produces, so the counter tracks refused requests.
        let hits = text
            .lines()
            .filter(|l| l.contains(REJECTION_SIGNAL) && l.contains(REJECTION_MARKER))
            .count() as u64;
        if hits > 0 {
            out.hits += hits;
            // Line-level timestamps are glog-formatted without a year; the
            // file's mtime is the reliable upper bound for "when".
            out.latest_hit_at = max_opt(out.latest_hit_at, Some(mtime));
        } else if text.lines().any(|l| l.contains(REJECTION_SIGNAL)) {
            out.marker_possibly_stale = true;
        }
    }
    out
}

fn humanise_age(secs: i64) -> String {
    if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lum-ag-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn sums_steps_and_skips_internal_conversations() {
        let root = temp_dir("meta");
        std::fs::create_dir_all(root.join("cache")).unwrap();
        std::fs::write(
            root.join("cache").join("conversation_metadata.json"),
            r#"{"conversations":{
                "a":{"summary":{"ID":"a","NumSteps":26,"UpdatedAt":"2026-08-07T06:04:11.663647Z"},"is_internal":false},
                "b":{"summary":{"ID":"b","NumSteps":103,"UpdatedAt":"2026-08-07T14:43:39.912908Z"},"is_internal":false},
                "c":{"summary":{"ID":"c","NumSteps":999,"UpdatedAt":"2026-08-07T14:43:39.912908Z"},"is_internal":true}
            }}"#,
        )
        .unwrap();

        let out = read_activity(&root, 0, i64::MAX);
        assert_eq!(out.counted, 2, "internal conversations are excluded");
        assert_eq!(out.steps, 129);
        assert_eq!(out.steps_today, 0, "day_start in the future means none are today");
        assert_eq!(out.source_kind, "conversation-metadata");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn respects_the_lookback_cutoff() {
        let root = temp_dir("cutoff");
        std::fs::create_dir_all(root.join("cache")).unwrap();
        std::fs::write(
            root.join("cache").join("conversation_metadata.json"),
            r#"{"conversations":{
                "old":{"summary":{"NumSteps":50,"UpdatedAt":"2020-01-01T00:00:00Z"},"is_internal":false},
                "new":{"summary":{"NumSteps":7,"UpdatedAt":"2026-08-07T14:00:00Z"},"is_internal":false}
            }}"#,
        )
        .unwrap();

        // Cutoff at 2026-01-01 drops the 2020 conversation.
        let out = read_activity(&root, 1_767_225_600, i64::MAX);
        assert_eq!(out.counted, 1);
        assert_eq!(out.steps, 7);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn falls_back_to_counting_conversation_databases() {
        let root = temp_dir("fallback");
        let conv = root.join("conversations");
        std::fs::create_dir_all(&conv).unwrap();
        std::fs::write(conv.join("one.db"), b"x").unwrap();
        std::fs::write(conv.join("two.db"), b"x").unwrap();
        std::fs::write(conv.join("two.db-wal"), b"x").unwrap();

        let out = read_activity(&root, 0, i64::MAX);
        assert_eq!(out.counted, 2, "only .db files count, not -wal/-shm siblings");
        assert_eq!(out.source_kind, "conversation-files");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn counts_one_hit_per_rejection_not_per_log_line() {
        let root = temp_dir("logs");
        let logs = root.join("log");
        std::fs::create_dir_all(&logs).unwrap();
        let mut f = std::fs::File::create(logs.join("cli-1.log")).unwrap();
        // One real rejection produces several downstream lines; only the
        // errorreport.go line should be counted.
        writeln!(f, "ERROR: E0804 16:38:56.501850 257 stream_handler.go:101] error encountered while processing planner output: RESOURCE_EXHAUSTED (code 429): Individual quota reached.").unwrap();
        writeln!(f, "ERROR: E0804 16:38:57.508857 257 errorreport.go:223] RESOURCE_EXHAUSTED (code 429): Individual quota reached.").unwrap();
        writeln!(f, "ERROR: E0804 16:38:59.934812 257 executor.go:525] error in generator: model unreachable: RESOURCE_EXHAUSTED (code 429)").unwrap();
        writeln!(f, "INFO: quota_manager.go:44] doRefreshQuota: starting reload (force=true)").unwrap();
        drop(f);

        let scan = scan_logs(&logs, 0);
        assert_eq!(scan.hits, 1);
        assert!(scan.latest_hit_at.is_some());
        assert!(
            !scan.marker_possibly_stale,
            "the marker matched, so nothing here should look stale"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn clean_logs_report_no_hits() {
        let root = temp_dir("clean-logs");
        let logs = root.join("log");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("cli-2.log"), "INFO: everything is fine\n").unwrap();

        let scan = scan_logs(&logs, 0);
        assert_eq!(scan.hits, 0);
        assert!(scan.latest_hit_at.is_none());
        assert!(!scan.marker_possibly_stale, "no rejection signal at all — nothing is stale");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_renamed_call_site_degrades_to_a_visible_warning_not_a_silent_zero() {
        // Simulates a future Antigravity release that renamed its internal
        // error-reporting file: RESOURCE_EXHAUSTED activity is present, but
        // none of it comes from errorreport.go anymore.
        let root = temp_dir("stale-marker");
        let logs = root.join("log");
        std::fs::create_dir_all(&logs).unwrap();
        let mut f = std::fs::File::create(logs.join("cli-3.log")).unwrap();
        writeln!(f, "ERROR: E0804 16:38:57.508857 257 report_error.go:99] RESOURCE_EXHAUSTED (code 429): Individual quota reached.").unwrap();
        drop(f);

        let scan = scan_logs(&logs, 0);
        assert_eq!(scan.hits, 0, "the old marker genuinely doesn't match anymore");
        assert!(
            scan.marker_possibly_stale,
            "broader RESOURCE_EXHAUSTED activity must still be flagged, not silently dropped"
        );

        std::fs::remove_dir_all(&root).ok();
    }
}
