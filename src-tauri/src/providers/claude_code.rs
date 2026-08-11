//! Claude Code.
//!
//! Two independent sources, deliberately kept separate in the snapshot:
//!
//! * **Quota (authoritative, live)** — `claude --output-format json -p /usage`.
//!   This subprocess call asks the Anthropic backend for the current 5-hour and
//!   weekly window usage, exactly what the Claude Code desktop app shows.
//!   On failure it falls back to the on-disk cache below.
//!
//! * **Quota (authoritative, cached fallback)** — `~/.claude.json →
//!   cachedUsageUtilization`. Claude Code writes a snapshot of the backend's
//!   quota data whenever it runs on this machine. Used when the live call fails
//!   (binary absent, no network, timeout).
//!
//! * **Tokens and cost (derived)** — `~/.claude/projects/**/*.jsonl`. Every
//!   assistant turn carries a full `message.usage` object. Summing it gives an
//!   exact token ledger and, with the price table, an API-equivalent cost.
//!   Note that on a subscription plan that cost is *notional*: it is what the
//!   same traffic would have cost on the API, not what the user is billed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use serde_json::Value;
use walkdir::WalkDir;

use crate::error::{MeterError, Result};
use crate::model::*;
use crate::providers::{Detection, ProviderContext, UsageProvider};
use crate::util;

pub struct ClaudeCodeProvider;

/// Cap on how much of a transcript we read. Transcripts are appended to, so the
/// tail holds the recent turns; a multi-hundred-megabyte history from a long
/// project shouldn't stall a menu-bar refresh.
const MAX_TRANSCRIPT_TAIL_BYTES: u64 = 8 * 1024 * 1024;

impl ClaudeCodeProvider {
    fn config_path(ctx: &ProviderContext) -> PathBuf {
        ctx.env.paths.home().join(".claude.json")
    }

    fn state_dir(ctx: &ProviderContext) -> PathBuf {
        ctx.env.paths.dot_dir("claude")
    }
}

impl UsageProvider for ClaudeCodeProvider {
    fn id(&self) -> &'static str {
        "claude-code"
    }

    fn display_name(&self) -> &'static str {
        "Claude Code"
    }

    fn detect(&self, ctx: &ProviderContext) -> Detection {
        let dir = Self::state_dir(ctx);
        if dir.is_dir() || Self::config_path(ctx).is_file() {
            Detection::Installed {
                root: dir.display().to_string(),
            }
        } else {
            Detection::NotInstalled
        }
    }

    fn collect(&self, ctx: &ProviderContext) -> Result<UsageSnapshot> {
        let started = util::now_millis();
        let mut snap = UsageSnapshot::empty(self.id(), self.display_name(), ProviderStatus::Ok);

        // ---- Quota: live, from claude --output-format json -p /usage ---------
        let config_path = Self::config_path(ctx);
        let live_fetch = fetch_live_quotas(&ctx.env.paths.home());
        match live_fetch {
            LiveQuotaFetch::Ok(live) if !live.is_empty() => {
                snap.confidence = Confidence::Authoritative;
                snap.quotas = live;
                snap.sources.push(SourceRef {
                    kind: "live-quota".into(),
                    path: "claude --output-format json -p /usage".into(),
                    observed_at: Some(ctx.now),
                });
                // Plan still comes from the on-disk config (live output doesn't
                // include it in a machine-readable way).
                if let Ok(cfg) = read_config(&config_path) {
                    snap.plan = cfg.plan;
                }
            }
            // Binary absent or output empty — fall back to the on-disk cache.
            LiveQuotaFetch::NoBinary | LiveQuotaFetch::Ok(_) => {
                match read_config(&config_path) {
                    Ok(cfg) => {
                        snap.plan = cfg.plan;
                        if let Some(util_block) = cfg.utilization {
                            snap.quotas = quota_windows(&util_block);
                            snap.confidence = Confidence::Authoritative;
                            if let Some(age) = util_block.age_seconds(ctx.now) {
                                snap.sources.push(SourceRef {
                                    kind: "quota-cache".into(),
                                    path: config_path.display().to_string(),
                                    observed_at: Some(ctx.now - age),
                                });
                                if age > 1800 {
                                    snap.notes.push(format!(
                                        "Quota figures cached {} ago — open Claude Code to refresh.",
                                        humanise_age(age)
                                    ));
                                }
                            }
                        } else {
                            snap.notes.push(
                                "No cached quota yet — run Claude Code once to populate it."
                                    .into(),
                            );
                        }
                    }
                    Err(e) => {
                        snap.status = ProviderStatus::Degraded;
                        snap.notes.push(format!("Could not read ~/.claude.json: {e}"));
                    }
                }
            }
            // Binary exists but the call failed — surface the reason and fall
            // back to the cache so the UI never goes blank silently.
            LiveQuotaFetch::Failed(reason) => {
                snap.notes.push(format!(
                    "Live quota check via `claude` didn't come back this refresh ({reason}); \
                     showing cached figures instead."
                ));
                match read_config(&config_path) {
                    Ok(cfg) => {
                        snap.plan = cfg.plan;
                        if let Some(util_block) = cfg.utilization {
                            snap.quotas = quota_windows(&util_block);
                            snap.confidence = Confidence::Authoritative;
                            if let Some(age) = util_block.age_seconds(ctx.now) {
                                snap.sources.push(SourceRef {
                                    kind: "quota-cache".into(),
                                    path: config_path.display().to_string(),
                                    observed_at: Some(ctx.now - age),
                                });
                                if age > 1800 {
                                    snap.notes.push(format!(
                                        "Quota figures cached {} ago — open Claude Code to refresh.",
                                        humanise_age(age)
                                    ));
                                }
                            }
                        }
                    }
                    Err(e) => {
                        snap.status = ProviderStatus::Degraded;
                        snap.notes.push(format!("Could not read ~/.claude.json: {e}"));
                    }
                }
            }
        }

        // ---- Tokens and cost: derived from the transcript ledger -------------
        let projects = Self::state_dir(ctx).join("projects");
        if projects.is_dir() {
            let cutoff = ctx.now - (ctx.lookback_days as i64) * 86_400;
            let day_start = util::local_day_start(ctx.now);
            let ledger = scan_transcripts(&projects, cutoff, day_start);

            if ledger.requests > 0 {
                let mut models: Vec<ModelUsage> = ledger
                    .by_model
                    .into_iter()
                    .map(|(model, agg)| {
                        let cost_usd = ctx.prices.cost_usd(
                            &model,
                            agg.tokens.input,
                            agg.tokens.output,
                            agg.cache_write_5m,
                            agg.cache_write_1h,
                            agg.tokens.cache_read,
                        );
                        ModelUsage {
                            model,
                            tokens: agg.tokens,
                            requests: agg.requests,
                            cost_usd,
                        }
                    })
                    .collect();
                models.sort_by(|a, b| {
                    b.cost_usd
                        .unwrap_or(0.0)
                        .total_cmp(&a.cost_usd.unwrap_or(0.0))
                        .then_with(|| b.tokens.billable_total().cmp(&a.tokens.billable_total()))
                });

                // Only an unpriced model that actually consumed tokens makes
                // the total a floor. Claude Code writes zero-token internal
                // turns under placeholder model ids; flagging the whole bill
                // "partial" because of those would cry wolf.
                let unpriced = models
                    .iter()
                    .filter(|m| m.cost_usd.is_none() && m.tokens.billable_total() > 0)
                    .count();
                let total_usd: f64 = models.iter().filter_map(|m| m.cost_usd).sum();

                snap.cost = Some(CostEstimate {
                    amount: Money::from_usd(total_usd),
                    partial: unpriced > 0,
                    basis: format!(
                        "API-equivalent cost of the last {} days of local transcripts{}",
                        ctx.lookback_days,
                        if unpriced > 0 {
                            format!("; {unpriced} model(s) missing from the price table")
                        } else {
                            String::new()
                        }
                    ),
                });
                snap.models = models;
                snap.tokens = Some(ledger.tokens);
                snap.activity = Some(ActivityStats {
                    requests_today: ledger.requests_today,
                    requests_total: ledger.requests,
                    sessions_today: ledger.sessions_today as u64,
                    last_activity_at: ledger.last_activity_at,
                    rate_limit_hits: 0,
                });
                snap.sources.push(SourceRef {
                    kind: "token-ledger".into(),
                    path: projects.display().to_string(),
                    observed_at: ledger.last_activity_at,
                });
                if snap.confidence != Confidence::Authoritative {
                    snap.confidence = Confidence::Derived;
                }
            }
        }

        if snap.quotas.is_empty() && snap.tokens.is_none() {
            snap.status = ProviderStatus::Degraded;
            snap.notes
                .push("Claude Code is installed but has no usage data yet.".into());
        }

        snap.collect_ms = util::now_millis().saturating_sub(started);
        Ok(snap)
    }
}

// ---------------------------------------------------------------------------
// ~/.claude.json
// ---------------------------------------------------------------------------

struct ClaudeConfig {
    plan: Option<String>,
    utilization: Option<Utilization>,
}

#[derive(Debug, Deserialize)]
struct CachedUsageUtilization {
    #[serde(default)]
    fetched_at_ms: Option<i64>,
    #[serde(default)]
    utilization: Option<Utilization>,
}

#[derive(Debug, Deserialize, Default)]
struct Utilization {
    #[serde(default)]
    limits: Vec<LimitEntry>,
    #[serde(default)]
    five_hour: Option<NamedWindow>,
    #[serde(default)]
    seven_day: Option<NamedWindow>,
    #[serde(default)]
    extra_usage: Option<ExtraUsage>,
    /// Injected after parsing; not part of the on-disk shape.
    #[serde(skip)]
    fetched_at_ms: Option<i64>,
}

impl Utilization {
    fn age_seconds(&self, now: i64) -> Option<i64> {
        self.fetched_at_ms.map(|ms| (now - ms / 1000).max(0))
    }
}

#[derive(Debug, Deserialize)]
struct LimitEntry {
    kind: String,
    #[serde(default)]
    group: Option<String>,
    percent: f64,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    resets_at: Option<String>,
    #[serde(default)]
    is_active: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct NamedWindow {
    utilization: f64,
    #[serde(default)]
    resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExtraUsage {
    #[serde(default)]
    monthly_limit: Option<i64>,
    #[serde(default)]
    used_credits: Option<i64>,
    #[serde(default)]
    utilization: Option<f64>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    decimal_places: Option<u8>,
    #[serde(default)]
    is_enabled: Option<bool>,
}

fn read_config(path: &Path) -> Result<ClaudeConfig> {
    let text = std::fs::read_to_string(path).map_err(|e| MeterError::io(path, e))?;
    let root: Value =
        serde_json::from_str(&text).map_err(|e| MeterError::parse(path, e.to_string()))?;

    // Only two keys are read, and neither is a credential. Tokens in this file
    // are never touched.
    let plan = root
        .get("oauthAccount")
        .and_then(|a| a.get("organizationType"))
        .and_then(Value::as_str)
        .map(str::to_owned);

    let utilization = root
        .get("cachedUsageUtilization")
        .cloned()
        .and_then(|v| serde_json::from_value::<CachedUsageUtilization>(v).ok())
        .and_then(|c| {
            c.utilization.map(|mut u| {
                u.fetched_at_ms = c.fetched_at_ms;
                u
            })
        });

    Ok(ClaudeConfig { plan, utilization })
}

/// Human label for the limit kinds the backend emits.
fn limit_label(kind: &str) -> String {
    match kind {
        "session" => "5-hour session".into(),
        "weekly_all" => "Weekly (all models)".into(),
        "weekly_opus" => "Weekly (Opus)".into(),
        "weekly_sonnet" => "Weekly (Sonnet)".into(),
        "weekly_oauth_apps" => "Weekly (OAuth apps)".into(),
        "weekly_cowork" => "Weekly (Cowork)".into(),
        // Unknown kinds still render, with the raw key title-cased: a new
        // limit type should show up rather than vanish.
        other => other
            .split('_')
            .map(|w| {
                let mut c = w.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn severity_from(tag: Option<&str>, percent: f64) -> Severity {
    match tag {
        Some("critical") => Severity::Critical,
        Some("warning") => Severity::Warning,
        Some("normal") => Severity::from_percent(percent),
        _ => Severity::from_percent(percent),
    }
}

// ---------------------------------------------------------------------------
// Live quota via `claude --output-format json -p /usage`
// ---------------------------------------------------------------------------

/// How long to wait for `claude` to respond.
const CLAUDE_TIMEOUT_SECS: u64 = 25;

/// Candidate binary locations.
fn claude_binary(home: &std::path::Path) -> Option<PathBuf> {
    let candidates = [
        home.join(".local").join("bin").join("claude"),
        PathBuf::from("/usr/local/bin/claude"),
        PathBuf::from("/opt/homebrew/bin/claude"),
    ];
    // Also check PATH via `which claude` as a last resort.
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .or_else(|| which_claude())
}

fn which_claude() -> Option<PathBuf> {
    Command::new("which")
        .arg("claude")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| p.is_file())
}

enum LiveQuotaFetch {
    /// No `claude` binary found.
    NoBinary,
    /// Call completed; these are the quota rows we could parse (may be empty).
    Ok(Vec<QuotaWindow>),
    /// Binary exists but the call didn't produce usable output.
    Failed(String),
}

/// Run `claude --output-format json -p /usage` with a timeout and parse the
/// output into `QuotaWindow` entries.
///
/// The JSON wrapper's `result` field contains human-readable text:
/// ```text
/// Current session: 24% used · resets Aug 12, 4:59am (Asia/Seoul)
/// Current week (all models): 16% used · resets Aug 17, 7:59am (Asia/Seoul)
/// ```
fn fetch_live_quotas(home: &std::path::Path) -> LiveQuotaFetch {
    let Some(binary) = claude_binary(home) else {
        return LiveQuotaFetch::NoBinary;
    };

    let mut child = match Command::new(&binary)
        .args(["--output-format", "json", "-p", "/usage"])
        .env("HOME", home)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return LiveQuotaFetch::Failed(format!("couldn't start `claude`: {e}")),
    };

    let stdout = child.stdout.take();
    let (tx, rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut stream) = stdout {
            use std::io::Read;
            let _ = stream.read_to_end(&mut buf);
        }
        let _ = tx.send(buf);
    });

    match rx.recv_timeout(std::time::Duration::from_secs(CLAUDE_TIMEOUT_SECS)) {
        Ok(bytes) => {
            let _ = child.wait();
            match String::from_utf8(bytes) {
                Ok(text) => LiveQuotaFetch::Ok(parse_live_quota_output(&text)),
                Err(_) => {
                    LiveQuotaFetch::Failed("`claude` printed non-UTF-8 output".into())
                }
            }
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            LiveQuotaFetch::Failed(format!(
                "`claude` didn't respond within {CLAUDE_TIMEOUT_SECS}s"
            ))
        }
    }
}

/// Parse the text content of `claude --output-format json -p /usage`.
///
/// The `result` field of the JSON wrapper (or the raw text if JSON parsing
/// fails) contains lines like:
/// ```text
/// Current session: 24% used · resets Aug 12, 4:59am (Asia/Seoul)
/// Current week (all models): 16% used · resets Aug 17, 7:59am (Asia/Seoul)
/// ```
///
/// Returns one `QuotaWindow` per recognised line. An unrecognised line is
/// silently skipped — forward compatibility with new limit types.
fn parse_live_quota_output(raw: &str) -> Vec<QuotaWindow> {
    // The `claude --output-format json` wrapper puts the human text in `result`.
    let text = if let Ok(v) = serde_json::from_str::<Value>(raw) {
        v.get("result")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| raw.to_owned())
    } else {
        raw.to_owned()
    };

    let mut quotas = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        // "Current session: 24% used"
        if let Some(rest) = line.strip_prefix("Current session:") {
            if let Some(pct) = parse_used_percent(rest) {
                let mut q = QuotaWindow::new("session", "5-hour session", pct)
                    .window_minutes(Some(300));
                q.is_active = pct > 0.0;
                quotas.push(q);
            }
            continue;
        }
        // "Current week (all models): 16% used"
        // "Current week (Opus): 34% used" etc.
        if let Some(rest) = line.strip_prefix("Current week (") {
            if let Some(paren) = rest.find(')') {
                let model_group = &rest[..paren];
                let after_paren = rest[paren + 1..].trim_start_matches(':').trim();
                if let Some(pct) = parse_used_percent(after_paren) {
                    let (id, label) = match model_group {
                        "all models" => (
                            "weekly_all".to_owned(),
                            "Weekly (all models)".to_owned(),
                        ),
                        other => (
                            format!("weekly_{}", other.to_lowercase().replace(' ', "_")),
                            format!("Weekly ({})", other),
                        ),
                    };
                    quotas.push(
                        QuotaWindow::new(&id, &label, pct).window_minutes(Some(10_080)),
                    );
                }
            }
            continue;
        }
    }
    quotas
}

/// Extract the leading integer percent from strings like "24% used · resets …".
fn parse_used_percent(s: &str) -> Option<f64> {
    let s = s.trim();
    let end = s.find('%')?;
    s[..end].trim().parse::<f64>().ok()
}

// ---------------------------------------------------------------------------
// Cached quota from ~/.claude.json
// ---------------------------------------------------------------------------

fn quota_windows(u: &Utilization) -> Vec<QuotaWindow> {
    let mut out = Vec::new();

    if !u.limits.is_empty() {
        // Preferred shape: the backend's own list, which is forward-compatible
        // with limit kinds this build hasn't seen.
        for l in &u.limits {
            let mut q = QuotaWindow::new(l.kind.clone(), limit_label(&l.kind), l.percent)
                .resets_at(l.resets_at.as_deref().and_then(util::parse_iso8601))
                .active(l.is_active.unwrap_or(true));
            q.severity = severity_from(l.severity.as_deref(), l.percent);
            if l.group.as_deref() == Some("weekly") {
                q.window_minutes = Some(10_080);
            } else if l.kind == "session" {
                q.window_minutes = Some(300);
            }
            out.push(q);
        }
    } else {
        // Fallback for older Claude Code builds that only wrote the named pair.
        if let Some(w) = &u.five_hour {
            out.push(
                QuotaWindow::new("session", "5-hour session", w.utilization)
                    .resets_at(w.resets_at.as_deref().and_then(util::parse_iso8601))
                    .window_minutes(Some(300)),
            );
        }
        if let Some(w) = &u.seven_day {
            out.push(
                QuotaWindow::new("weekly_all", "Weekly (all models)", w.utilization)
                    .resets_at(w.resets_at.as_deref().and_then(util::parse_iso8601))
                    .window_minutes(Some(10_080)),
            );
        }
    }

    // Extra-usage credits are real money, so they get their own row with
    // amounts rather than being folded into a percentage.
    if let Some(x) = &u.extra_usage {
        if x.used_credits.is_some() || x.monthly_limit.is_some() {
            let currency = x.currency.clone().unwrap_or_else(|| "USD".into());
            let exponent = x.decimal_places.unwrap_or(2);
            let pct = x.utilization.unwrap_or_else(|| {
                match (x.used_credits, x.monthly_limit) {
                    (Some(used), Some(limit)) if limit > 0 => {
                        (used as f64 / limit as f64) * 100.0
                    }
                    _ => 0.0,
                }
            });
            let mut q = QuotaWindow::new("extra_usage", "Extra usage credits", pct).spend(
                Money {
                    amount_minor: x.used_credits.unwrap_or(0),
                    currency: currency.clone(),
                    exponent,
                },
                x.monthly_limit.map(|l| Money {
                    amount_minor: l,
                    currency,
                    exponent,
                }),
            );
            // A disabled credit pool still shows, de-emphasised — hiding it
            // would make a 100% bar disappear the moment it starts mattering.
            q.is_active = x.is_enabled.unwrap_or(false);
            out.push(q);
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Transcript ledger
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ModelAgg {
    tokens: TokenTotals,
    requests: u64,
    cache_write_5m: u64,
    cache_write_1h: u64,
}

#[derive(Default)]
struct Ledger {
    tokens: TokenTotals,
    requests: u64,
    requests_today: u64,
    sessions_today: usize,
    last_activity_at: Option<i64>,
    by_model: std::collections::HashMap<String, ModelAgg>,
}

/// Walk `projects/`, summing assistant-turn usage.
///
/// Deduplication matters: resuming or forking a session copies prior turns into
/// a new transcript, so the same request can appear in several files. Keying on
/// `message.id` + `requestId` — the pair the API itself assigns — counts each
/// billed request exactly once.
fn scan_transcripts(projects: &Path, cutoff: i64, day_start: i64) -> Ledger {
    let mut ledger = Ledger::default();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut sessions_today: HashSet<String> = HashSet::new();

    let files = WalkDir::new(projects)
        .max_depth(3)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"));

    for entry in files {
        let path = entry.path();
        // Skip whole files that can't contain in-window turns.
        if util::mtime_unix(path).is_some_and(|m| m < cutoff) {
            continue;
        }
        let Ok(text) = util::read_tail(path, MAX_TRANSCRIPT_TAIL_BYTES) else {
            continue;
        };

        for line in text.lines() {
            // Cheap reject before paying for a full JSON parse: most lines in a
            // transcript are user turns, tool results, or meta records.
            if !line.contains("\"type\":\"assistant\"") || !line.contains("\"usage\"") {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if v.get("type").and_then(Value::as_str) != Some("assistant") {
                continue;
            }

            let ts = v
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(util::parse_iso8601);
            if ts.is_some_and(|t| t < cutoff) {
                continue;
            }

            let Some(message) = v.get("message") else {
                continue;
            };
            let Some(usage) = message.get("usage") else {
                continue;
            };

            let msg_id = message
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let req_id = v
                .get("requestId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            // A turn with neither id can't be deduplicated; skipping it is the
            // safe direction (undercount beats double-count).
            if msg_id.is_empty() && req_id.is_empty() {
                continue;
            }
            if !seen.insert((msg_id, req_id)) {
                continue;
            }

            let model = message
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();

            let n = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0);
            let cache_creation = usage.get("cache_creation");
            let cw_1h = cache_creation
                .and_then(|c| c.get("ephemeral_1h_input_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let cw_5m = cache_creation
                .and_then(|c| c.get("ephemeral_5m_input_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let cw_total = n("cache_creation_input_tokens");
            // Older transcripts have the total but no TTL split; bill the
            // remainder at the 5-minute rate rather than dropping it.
            let cw_5m = if cw_5m + cw_1h == 0 {
                cw_total
            } else {
                cw_5m + cw_total.saturating_sub(cw_5m + cw_1h)
            };

            let turn = TokenTotals {
                input: n("input_tokens"),
                output: n("output_tokens"),
                cache_write: cw_5m + cw_1h,
                cache_read: n("cache_read_input_tokens"),
                reasoning: 0,
            };

            ledger.tokens.add(&turn);
            ledger.requests += 1;
            if let Some(t) = ts {
                ledger.last_activity_at = Some(ledger.last_activity_at.unwrap_or(t).max(t));
                if t >= day_start {
                    ledger.requests_today += 1;
                    if let Some(sid) = v.get("sessionId").and_then(Value::as_str) {
                        sessions_today.insert(sid.to_string());
                    }
                }
            }

            let agg = ledger.by_model.entry(model).or_default();
            agg.tokens.add(&turn);
            agg.requests += 1;
            agg.cache_write_5m += cw_5m;
            agg.cache_write_1h += cw_1h;
        }
    }

    ledger.sessions_today = sessions_today.len();
    ledger
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

    // -----------------------------------------------------------------------
    // Live quota parser
    // -----------------------------------------------------------------------

    const SAMPLE_USAGE: &str = r#"
{"result":"You are currently using your subscription to power your Claude Code usage\n\nCurrent session: 24% used · resets Aug 12, 4:59am (Asia/Seoul)\nCurrent week (all models): 16% used · resets Aug 17, 7:59am (Asia/Seoul)\n\nWhat's contributing to your limits usage?\n"}
"#;

    #[test]
    fn parses_live_session_and_weekly_quotas() {
        let q = parse_live_quota_output(SAMPLE_USAGE.trim());
        assert_eq!(q.len(), 2, "session + weekly_all");

        let session = &q[0];
        assert_eq!(session.id, "session");
        assert_eq!(session.label, "5-hour session");
        assert_eq!(session.used_percent, 24.0);
        assert_eq!(session.window_minutes, Some(300));
        assert!(session.is_active);

        let weekly = &q[1];
        assert_eq!(weekly.id, "weekly_all");
        assert_eq!(weekly.label, "Weekly (all models)");
        assert_eq!(weekly.used_percent, 16.0);
        assert_eq!(weekly.window_minutes, Some(10_080));
    }

    #[test]
    fn parses_zero_session_as_inactive() {
        let text = r#"{"result":"Current session: 0% used · resets Aug 12, 5am (Asia/Seoul)\nCurrent week (all models): 34% used · resets Aug 17, 8am (Asia/Seoul)\n"}"#;
        let q = parse_live_quota_output(text);
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].used_percent, 0.0);
        assert!(!q[0].is_active, "0% session should be inactive");
        assert_eq!(q[1].used_percent, 34.0);
    }

    #[test]
    fn parses_named_weekly_group() {
        let text = r#"{"result":"Current session: 5% used\nCurrent week (Opus): 78% used\n"}"#;
        let q = parse_live_quota_output(text);
        assert_eq!(q.len(), 2);
        assert_eq!(q[1].id, "weekly_opus");
        assert_eq!(q[1].label, "Weekly (Opus)");
        assert_eq!(q[1].used_percent, 78.0);
    }

    #[test]
    fn returns_empty_on_unrecognised_output() {
        let q = parse_live_quota_output("something completely different");
        assert!(q.is_empty());
    }

    #[test]
    fn falls_back_to_raw_text_when_not_json() {
        // Plain text (no JSON wrapper) still parses correctly.
        let text = "Current session: 10% used\nCurrent week (all models): 50% used\n";
        let q = parse_live_quota_output(text);
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].used_percent, 10.0);
        assert_eq!(q[1].used_percent, 50.0);
    }

    use std::io::Write;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lum-cc-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn parses_the_limits_array_shape() {
        let u: Utilization = serde_json::from_str(
            r#"{"limits":[
                {"kind":"session","group":"session","percent":0,"severity":"normal",
                 "resets_at":"2026-08-07T19:50:00.039004+00:00","is_active":false},
                {"kind":"weekly_all","group":"weekly","percent":34,"severity":"normal",
                 "resets_at":"2026-08-09T22:59:59.039029+00:00","is_active":true}
            ]}"#,
        )
        .unwrap();
        let q = quota_windows(&u);
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].id, "session");
        assert_eq!(q[0].label, "5-hour session");
        assert!(!q[0].is_active);
        assert_eq!(q[0].window_minutes, Some(300));
        assert_eq!(q[1].used_percent, 34.0);
        assert_eq!(q[1].window_minutes, Some(10_080));
        assert!(q[1].resets_at.is_some());
    }

    #[test]
    fn falls_back_to_named_windows_when_limits_absent() {
        let u: Utilization = serde_json::from_str(
            r#"{"five_hour":{"utilization":12,"resets_at":"2026-08-07T19:50:00Z"},
                "seven_day":{"utilization":80,"resets_at":null}}"#,
        )
        .unwrap();
        let q = quota_windows(&u);
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].used_percent, 12.0);
        assert_eq!(q[1].severity, Severity::Warning);
    }

    #[test]
    fn extra_usage_becomes_a_money_row_even_when_disabled() {
        // The real config has is_enabled=false with utilization=100 — the row
        // must survive, because a maxed-out disabled pool is worth seeing.
        let u: Utilization = serde_json::from_str(
            r#"{"extra_usage":{"is_enabled":false,"monthly_limit":10000,
                "used_credits":10069,"utilization":100,"currency":"USD","decimal_places":2}}"#,
        )
        .unwrap();
        let q = quota_windows(&u);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].id, "extra_usage");
        assert_eq!(q[0].used_percent, 100.0);
        assert!(!q[0].is_active);
        assert_eq!(q[0].used_amount.as_ref().unwrap().amount_minor, 10_069);
        assert_eq!(q[0].limit_amount.as_ref().unwrap().amount_minor, 10_000);
        assert_eq!(q[0].severity, Severity::Critical);
    }

    #[test]
    fn unknown_limit_kinds_still_render() {
        let u: Utilization = serde_json::from_str(
            r#"{"limits":[{"kind":"weekly_brand_new_thing","percent":5}]}"#,
        )
        .unwrap();
        let q = quota_windows(&u);
        assert_eq!(q[0].label, "Weekly Brand New Thing");
    }

    #[test]
    fn ledger_dedupes_requests_copied_across_transcripts() {
        let dir = temp_dir("dedupe");
        let proj = dir.join("proj-a");
        std::fs::create_dir_all(&proj).unwrap();

        let turn = |msg: &str, req: &str, out: u64| {
            format!(
                r#"{{"type":"assistant","timestamp":"2026-08-07T15:07:47.235Z","sessionId":"s1","requestId":"{req}","message":{{"id":"{msg}","model":"claude-opus-5","usage":{{"input_tokens":10,"output_tokens":{out},"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}}}}"#
            )
        };

        let mut a = std::fs::File::create(proj.join("a.jsonl")).unwrap();
        writeln!(a, "{}", turn("msg_1", "req_1", 100)).unwrap();
        writeln!(a, "{}", turn("msg_2", "req_2", 200)).unwrap();
        drop(a);

        // Session resume: the same two turns re-appear in a second transcript.
        let mut b = std::fs::File::create(proj.join("b.jsonl")).unwrap();
        writeln!(b, "{}", turn("msg_1", "req_1", 100)).unwrap();
        writeln!(b, "{}", turn("msg_2", "req_2", 200)).unwrap();
        writeln!(b, "{}", turn("msg_3", "req_3", 300)).unwrap();
        drop(b);

        let l = scan_transcripts(&dir, 0, 0);
        assert_eq!(l.requests, 3, "each billed request counted once");
        assert_eq!(l.tokens.output, 600);
        assert_eq!(l.by_model["claude-opus-5"].requests, 3);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ledger_splits_cache_writes_by_ttl() {
        let dir = temp_dir("ttl");
        let proj = dir.join("p");
        std::fs::create_dir_all(&proj).unwrap();
        let mut f = std::fs::File::create(proj.join("t.jsonl")).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","timestamp":"2026-08-07T15:07:47.235Z","requestId":"r","message":{{"id":"m","model":"claude-opus-5","usage":{{"input_tokens":2,"output_tokens":5,"cache_creation_input_tokens":14208,"cache_read_input_tokens":30125,"cache_creation":{{"ephemeral_1h_input_tokens":14208,"ephemeral_5m_input_tokens":0}}}}}}}}"#
        )
        .unwrap();
        drop(f);

        let l = scan_transcripts(&dir, 0, 0);
        let agg = &l.by_model["claude-opus-5"];
        assert_eq!(agg.cache_write_1h, 14_208);
        assert_eq!(agg.cache_write_5m, 0);
        assert_eq!(l.tokens.cache_read, 30_125);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ledger_bills_legacy_cache_writes_without_a_ttl_split() {
        let dir = temp_dir("legacy-ttl");
        let proj = dir.join("p");
        std::fs::create_dir_all(&proj).unwrap();
        let mut f = std::fs::File::create(proj.join("t.jsonl")).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","timestamp":"2026-08-07T15:07:47.235Z","requestId":"r","message":{{"id":"m","model":"claude-opus-5","usage":{{"input_tokens":1,"output_tokens":1,"cache_creation_input_tokens":5000,"cache_read_input_tokens":0}}}}}}"#
        )
        .unwrap();
        drop(f);

        let l = scan_transcripts(&dir, 0, 0);
        let agg = &l.by_model["claude-opus-5"];
        // Nothing is silently dropped just because the TTL split is missing.
        assert_eq!(agg.cache_write_5m, 5_000);
        assert_eq!(agg.cache_write_1h, 0);
        assert_eq!(l.tokens.cache_write, 5_000);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ledger_ignores_non_assistant_records() {
        let dir = temp_dir("filter");
        let proj = dir.join("p");
        std::fs::create_dir_all(&proj).unwrap();
        let mut f = std::fs::File::create(proj.join("t.jsonl")).unwrap();
        writeln!(f, r#"{{"type":"user","message":{{"role":"user"}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"summary","summary":"x"}}"#).unwrap();
        writeln!(f, "not json at all").unwrap();
        drop(f);

        let l = scan_transcripts(&dir, 0, 0);
        assert_eq!(l.requests, 0);
        assert_eq!(l.tokens.billable_total(), 0);

        std::fs::remove_dir_all(&dir).ok();
    }
}
