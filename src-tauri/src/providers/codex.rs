//! Codex.
//!
//! Codex writes one rollout transcript per session under
//! `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`. Interleaved in it are
//! `token_count` events carrying two useful things:
//!
//! * `rate_limits` — the OpenAI backend's own view of the account's quota
//!   (`used_percent`, `window_minutes`, `resets_at`, `plan_type`). Authoritative,
//!   and exactly what `/status` shows.
//! * `info.total_token_usage` — a session-cumulative token count.
//!
//! Because `total_token_usage` is cumulative *within a session*, only the last
//! `token_count` per rollout matters. That makes the read cheap: seek to the
//! end of each file rather than parsing megabytes of transcript.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;
use walkdir::WalkDir;

use crate::error::Result;
use crate::model::*;
use crate::providers::{Detection, ProviderContext, UsageProvider};
use crate::util;

pub struct CodexProvider;

/// First-pass tail budget. The final `token_count` normally sits within the
/// last few KB; this is generous enough to survive a big trailing tool result.
const TAIL_FIRST_PASS: u64 = 256 * 1024;
/// Second pass, used only when the first finds no `token_count`.
const TAIL_SECOND_PASS: u64 = 4 * 1024 * 1024;

impl CodexProvider {
    fn state_dir(ctx: &ProviderContext) -> PathBuf {
        ctx.env.paths.dot_dir("codex")
    }
}

impl UsageProvider for CodexProvider {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn display_name(&self) -> &'static str {
        "Codex"
    }

    fn detect(&self, ctx: &ProviderContext) -> Detection {
        let dir = Self::state_dir(ctx);
        if dir.is_dir() {
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

        let sessions = Self::state_dir(ctx).join("sessions");
        if !sessions.is_dir() {
            snap.status = ProviderStatus::Degraded;
            snap.notes
                .push("No ~/.codex/sessions directory — run Codex once.".into());
            snap.collect_ms = util::now_millis().saturating_sub(started);
            return Ok(snap);
        }

        let cutoff = ctx.now - (ctx.lookback_days as i64) * 86_400;
        let day_start = util::local_day_start(ctx.now);
        let scan = scan_rollouts(&sessions, cutoff, day_start);

        // ---- Quota: the newest rate_limits observation wins -------------------
        if let Some((observed_at, limits)) = scan.latest_limits {
            snap.confidence = Confidence::Authoritative;
            snap.plan = limits.plan_type.clone();
            snap.quotas = quota_windows(&limits);
            snap.sources.push(SourceRef {
                kind: "rate-limit-snapshot".into(),
                path: sessions.display().to_string(),
                observed_at: Some(observed_at),
            });
            let age = (ctx.now - observed_at).max(0);
            if age > 1800 {
                // Rate limits are only written when Codex runs, so the number
                // ages; say so rather than implying it is live.
                snap.notes.push(format!(
                    "Quota as of {} ago (last Codex activity).",
                    humanise_age(age)
                ));
            }
            if let Some(reached) = &limits.rate_limit_reached_type {
                snap.notes
                    .push(format!("Codex reported hitting the {reached} limit."));
            }
        } else if scan.sessions > 0 {
            snap.notes.push(
                "No rate-limit data in recent sessions — Codex records it only on API turns."
                    .into(),
            );
        }

        // ---- Tokens and cost --------------------------------------------------
        if scan.sessions > 0 {
            // Codex reports `total_token_usage` per *session*, and one session
            // routinely spans several models (a planner, a coder, an
            // auto-reviewer). There is therefore no honest way to split these
            // tokens per model, so no per-model breakdown is published and the
            // models are listed as a note instead.
            let mut models: Vec<&String> = scan.models.iter().collect();
            models.sort();
            if !models.is_empty() {
                snap.notes.push(format!(
                    "Models seen: {}. Codex reports tokens per session, not per model, so the \
                     totals below are not split by model.",
                    models.iter().map(|m| m.as_str()).collect::<Vec<_>>().join(", ")
                ));
            }

            // Pricing the session total is only correct when every model
            // involved bills at the same rate; otherwise the split we don't
            // have would change the answer. Anything else would be a guess
            // wearing a dollar sign.
            snap.cost = uniform_price(ctx, &scan.models)
                .and_then(|p| {
                    ctx.prices.cost_usd(
                        &p,
                        // `input_tokens` here is the whole prompt; the cached
                        // part is reported separately and bills at the
                        // discounted rate, so take it out of full-price input.
                        scan.tokens.input.saturating_sub(scan.tokens.cache_read),
                        scan.tokens.output,
                        0,
                        0,
                        scan.tokens.cache_read,
                    )
                })
                .map(|usd| CostEstimate {
                    amount: Money::from_usd(usd),
                    partial: true,
                    basis: format!(
                        "API-equivalent cost over {} days, using the single rate every model in \
                         these sessions shares.",
                        ctx.lookback_days
                    ),
                });
            if snap.cost.is_none() {
                snap.notes.push(
                    "Cost is not shown: the models in these sessions have no shared published \
                     rate in the price table. Add them to ~/.config/llm-usage-meter/pricing.json \
                     to enable it."
                        .into(),
                );
            }

            snap.tokens = Some(scan.tokens);
            snap.notes.push(
                "Activity counts are Codex sessions, not individual API requests — Codex does \
                 not record a per-request count locally."
                    .into(),
            );
            snap.activity = Some(ActivityStats {
                requests_today: scan.sessions_today,
                requests_total: scan.sessions,
                sessions_today: scan.sessions_today,
                last_activity_at: scan.last_activity_at,
                rate_limit_hits: scan.rate_limit_hits,
            });
            snap.sources.push(SourceRef {
                kind: "rollout-token-count".into(),
                path: sessions.display().to_string(),
                observed_at: scan.last_activity_at,
            });
            if snap.confidence != Confidence::Authoritative {
                snap.confidence = Confidence::Derived;
            }
        } else {
            snap.status = ProviderStatus::Degraded;
            snap.notes
                .push(format!("No Codex sessions in the last {} days.", ctx.lookback_days));
        }

        snap.collect_ms = util::now_millis().saturating_sub(started);
        Ok(snap)
    }
}

// ---------------------------------------------------------------------------
// Rollout parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Default)]
struct RateLimits {
    #[serde(default)]
    primary: Option<LimitWindow>,
    #[serde(default)]
    secondary: Option<LimitWindow>,
    #[serde(default)]
    credits: Option<Credits>,
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    rate_limit_reached_type: Option<String>,
}

impl RateLimits {
    /// True when the backend actually reported numbers. Codex writes a
    /// `token_count` with every field null on local-only turns, and treating
    /// that as "0% used" would be a lie.
    fn has_data(&self) -> bool {
        self.primary.is_some() || self.secondary.is_some()
    }
}

#[derive(Debug, Clone, Deserialize)]
struct LimitWindow {
    used_percent: f64,
    #[serde(default)]
    window_minutes: Option<u32>,
    /// Unix seconds.
    #[serde(default)]
    resets_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
struct Credits {
    #[serde(default)]
    has_credits: Option<bool>,
    #[serde(default)]
    unlimited: Option<bool>,
    /// Serialised as a string by Codex, e.g. `"0"`.
    #[serde(default)]
    balance: Option<String>,
}

/// Label a rolling window from its length, so a new window size still reads well.
fn window_label(minutes: Option<u32>, fallback: &str) -> String {
    match minutes {
        Some(m) if m % 10_080 == 0 && m > 0 => {
            let weeks = m / 10_080;
            if weeks == 1 {
                "Weekly limit".into()
            } else {
                format!("{weeks}-week limit")
            }
        }
        Some(m) if m % 1_440 == 0 && m > 0 => {
            let days = m / 1_440;
            if days == 1 {
                "Daily limit".into()
            } else {
                format!("{days}-day limit")
            }
        }
        Some(m) if m % 60 == 0 && m > 0 => format!("{}-hour limit", m / 60),
        Some(m) => format!("{m}-minute limit"),
        None => fallback.into(),
    }
}

fn quota_windows(l: &RateLimits) -> Vec<QuotaWindow> {
    let mut out = Vec::new();
    if let Some(p) = &l.primary {
        out.push(
            QuotaWindow::new("primary", window_label(p.window_minutes, "Primary limit"), p.used_percent)
                .resets_at(p.resets_at)
                .window_minutes(p.window_minutes),
        );
    }
    if let Some(s) = &l.secondary {
        out.push(
            QuotaWindow::new(
                "secondary",
                window_label(s.window_minutes, "Secondary limit"),
                s.used_percent,
            )
            .resets_at(s.resets_at)
            .window_minutes(s.window_minutes),
        );
    }
    if let Some(c) = &l.credits {
        if c.unlimited.unwrap_or(false) {
            out.push(QuotaWindow::new("credits", "Credits (unlimited)", 0.0).active(false));
        } else if c.has_credits.unwrap_or(false) {
            let balance: f64 = c
                .balance
                .as_deref()
                .and_then(|b| b.parse::<f64>().ok())
                .unwrap_or(0.0);
            out.push(
                QuotaWindow::new("credits", "Credit balance", 0.0)
                    .spend(Money::from_usd(balance), None)
                    .active(true),
            );
        }
    }
    out
}

#[derive(Default)]
struct RolloutScan {
    tokens: TokenTotals,
    sessions: u64,
    sessions_today: u64,
    last_activity_at: Option<i64>,
    rate_limit_hits: u64,
    /// Newest non-null rate-limit observation: (unix seconds, payload).
    latest_limits: Option<(i64, RateLimits)>,
    /// Every model id seen in the window. A single session spans several, which
    /// is why token totals cannot be attributed per model.
    models: BTreeSet<String>,
}

/// The one price shared by every model in the window, if there is one.
///
/// Returns `None` when the models disagree on price or any is unknown — in
/// either case the per-model token split we don't have would change the total,
/// so there is no correct number to show. All four rate components must
/// match, not just input/output: `cost_usd` also applies each model's
/// `cache_write`/`cache_read` multipliers, so two models that bill input and
/// output identically but cache differently would still mis-price the cache
/// portion of the total if only input/output were compared.
fn uniform_price(ctx: &ProviderContext, models: &BTreeSet<String>) -> Option<String> {
    let first = models.iter().next()?;
    let base = ctx.prices.lookup(first)?;
    for m in models.iter().skip(1) {
        let p = ctx.prices.lookup(m)?;
        if p.input != base.input
            || p.output != base.output
            || p.cache_write != base.cache_write
            || p.cache_read != base.cache_read
        {
            return None;
        }
    }
    Some(first.clone())
}

fn scan_rollouts(sessions: &Path, cutoff: i64, day_start: i64) -> RolloutScan {
    let mut scan = RolloutScan::default();

    let files = WalkDir::new(sessions)
        .max_depth(5)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"));

    for entry in files {
        let path = entry.path();
        let Some(mtime) = util::mtime_unix(path) else {
            continue;
        };
        if mtime < cutoff {
            continue;
        }

        let Some(last) = last_token_count(path) else {
            continue;
        };

        scan.sessions += 1;
        scan.last_activity_at = Some(scan.last_activity_at.unwrap_or(mtime).max(mtime));
        if mtime >= day_start {
            scan.sessions_today += 1;
        }

        if let Some(info) = &last.info {
            scan.tokens.add(&TokenTotals {
                input: info.input_tokens,
                output: info.output_tokens,
                cache_write: info.cache_write_input_tokens,
                cache_read: info.cached_input_tokens,
                reasoning: info.reasoning_output_tokens,
            });
        }
        scan.models.extend(last.models);
        if let Some(limits) = last.limits {
            if limits.rate_limit_reached_type.is_some() {
                scan.rate_limit_hits += 1;
            }
            if limits.has_data() {
                let newer = scan
                    .latest_limits
                    .as_ref()
                    .is_none_or(|(seen, _)| last.observed_at > *seen);
                if newer {
                    scan.latest_limits = Some((last.observed_at, limits));
                }
            }
        }
    }

    scan
}

#[derive(Default)]
struct TokenInfo {
    input_tokens: u64,
    cached_input_tokens: u64,
    cache_write_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
}

#[derive(Default)]
struct LastTokenCount {
    observed_at: i64,
    info: Option<TokenInfo>,
    limits: Option<RateLimits>,
    /// Every model id the session's `turn_context` events named.
    models: BTreeSet<String>,
}

/// Find the final `token_count` event in a rollout, reading from the end.
fn last_token_count(path: &Path) -> Option<LastTokenCount> {
    for budget in [TAIL_FIRST_PASS, TAIL_SECOND_PASS] {
        let Ok(text) = util::read_tail(path, budget) else {
            return None;
        };
        let mut found: Option<LastTokenCount> = None;
        let mut models: BTreeSet<String> = BTreeSet::new();

        for line in text.lines() {
            // Collect *every* model named, not just the first: a session
            // routinely uses a planner, a coder and an auto-reviewer, and
            // picking one of them arbitrarily would misattribute the tokens.
            if line.contains("\"model\"") {
                if let Ok(v) = serde_json::from_str::<Value>(line) {
                    for ptr in ["/payload/model", "/payload/turn_context/model"] {
                        if let Some(m) = v.pointer(ptr).and_then(Value::as_str) {
                            models.insert(m.to_owned());
                        }
                    }
                }
            }
            if !line.contains("\"token_count\"") {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let Some(payload) = v.get("payload") else {
                continue;
            };
            if payload.get("type").and_then(Value::as_str) != Some("token_count") {
                continue;
            }

            let observed_at = v
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(util::parse_iso8601)
                .unwrap_or(0);

            let info = payload.pointer("/info/total_token_usage").map(|t| {
                let n = |k: &str| t.get(k).and_then(Value::as_u64).unwrap_or(0);
                TokenInfo {
                    input_tokens: n("input_tokens"),
                    cached_input_tokens: n("cached_input_tokens"),
                    cache_write_input_tokens: n("cache_write_input_tokens"),
                    output_tokens: n("output_tokens"),
                    reasoning_output_tokens: n("reasoning_output_tokens"),
                }
            });

            let limits = payload
                .get("rate_limits")
                .cloned()
                .and_then(|v| serde_json::from_value::<RateLimits>(v).ok());

            found = Some(LastTokenCount {
                observed_at,
                info,
                limits,
                models: BTreeSet::new(),
            });
        }

        if let Some(mut f) = found {
            f.models = models;
            return Some(f);
        }
        // Nothing found and the file was fully read — a bigger budget won't help.
        if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) <= budget {
            return None;
        }
    }
    None
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
        let d = std::env::temp_dir().join(format!("lum-codex-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const POPULATED: &str = r#"{"timestamp":"2026-08-06T05:52:46.232Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":895204,"cached_input_tokens":824064,"cache_write_input_tokens":0,"output_tokens":8326,"reasoning_output_tokens":4069,"total_tokens":903530},"model_context_window":258400},"rate_limits":{"limit_id":"codex","primary":{"used_percent":95.0,"window_minutes":10080,"resets_at":1786532172},"secondary":null,"credits":{"has_credits":false,"unlimited":false,"balance":"0"},"plan_type":"plus","rate_limit_reached_type":null}}}"#;

    const ALL_NULL: &str = r#"{"timestamp":"2026-08-07T03:38:38.913Z","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{"limit_id":"premium","primary":null,"secondary":null,"credits":{"has_credits":false,"unlimited":false,"balance":"0"},"plan_type":null,"rate_limit_reached_type":null}}}"#;

    #[test]
    fn parses_a_populated_rate_limit_payload() {
        let v: Value = serde_json::from_str(POPULATED).unwrap();
        let l: RateLimits =
            serde_json::from_value(v.pointer("/payload/rate_limits").unwrap().clone()).unwrap();
        assert!(l.has_data());
        assert_eq!(l.plan_type.as_deref(), Some("plus"));

        let q = quota_windows(&l);
        assert_eq!(q.len(), 1, "secondary is null and must not become a row");
        assert_eq!(q[0].id, "primary");
        assert_eq!(q[0].label, "Weekly limit");
        assert_eq!(q[0].used_percent, 95.0);
        assert_eq!(q[0].severity, Severity::Critical);
        assert_eq!(q[0].resets_at, Some(1_786_532_172));
    }

    #[test]
    fn all_null_rate_limits_are_not_treated_as_zero_percent() {
        let v: Value = serde_json::from_str(ALL_NULL).unwrap();
        let l: RateLimits =
            serde_json::from_value(v.pointer("/payload/rate_limits").unwrap().clone()).unwrap();
        assert!(!l.has_data(), "a null payload must not look like an empty quota");
        assert!(quota_windows(&l).is_empty());
    }

    fn price_ctx<'a>(
        env: &'a crate::platform::HostEnv,
        prices: &'a crate::pricing::PriceTable,
    ) -> ProviderContext<'a> {
        ProviderContext {
            env,
            prices,
            now: 0,
            lookback_days: 30,
        }
    }

    #[test]
    fn cost_is_only_priced_when_every_model_shares_a_rate() {
        let env = crate::platform::HostEnv::detect();
        let prices = crate::pricing::PriceTable::builtin();
        let ctx = price_ctx(&env, &prices);
        let set = |ids: &[&str]| ids.iter().map(|s| (*s).to_string()).collect::<BTreeSet<_>>();

        // One known model: priceable.
        assert!(uniform_price(&ctx, &set(&["gpt-5-codex"])).is_some());
        // Two models at the same rate: still priceable.
        assert!(uniform_price(&ctx, &set(&["gpt-5", "gpt-5-codex"])).is_some());
        // Different rates — the per-model split we don't have would change the
        // answer, so there is no honest number.
        assert!(uniform_price(&ctx, &set(&["gpt-5-codex", "gpt-5-mini"])).is_none());
        // The real-world case: Codex ships model ids with no published rate.
        assert!(uniform_price(&ctx, &set(&["gpt-5.6-terra", "codex-auto-review"])).is_none());
        // A single unknown model must not fall back to some other model's price.
        assert!(uniform_price(&ctx, &set(&["codex-auto-review"])).is_none());
        assert!(uniform_price(&ctx, &BTreeSet::new()).is_none());
    }

    #[test]
    fn identical_input_output_rates_are_not_enough_if_cache_multipliers_differ() {
        // Two models can bill input/output identically but cache differently —
        // cost_usd() applies cache_write/cache_read multipliers too, so those
        // must also match before a session's tokens can be priced as one rate.
        let dir = temp_dir("cache-mult-mismatch");
        std::fs::create_dir_all(&dir).unwrap();
        let override_path = dir.join("pricing.json");
        std::fs::write(
            &override_path,
            r#"{
                "model-a": {"input": 2.0, "output": 8.0, "cacheWrite": 1.25, "cacheRead": 0.1},
                "model-b": {"input": 2.0, "output": 8.0, "cacheWrite": 2.0,  "cacheRead": 0.1}
            }"#,
        )
        .unwrap();

        let env = crate::platform::HostEnv::detect();
        let prices = crate::pricing::PriceTable::load(&override_path);
        let ctx = price_ctx(&env, &prices);
        let set = |ids: &[&str]| ids.iter().map(|s| (*s).to_string()).collect::<BTreeSet<_>>();

        assert!(
            uniform_price(&ctx, &set(&["model-a"])).is_some(),
            "a single model is always self-consistent"
        );
        assert!(
            uniform_price(&ctx, &set(&["model-a", "model-b"])).is_none(),
            "same input/output but different cache_write must not be treated as uniform"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn every_model_in_a_session_is_collected_not_just_the_first() {
        let dir = temp_dir("multi-model");
        let day = dir.join("2026/08/07");
        std::fs::create_dir_all(&day).unwrap();
        let path = day.join("rollout-multi.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        for m in ["gpt-5.6-terra", "gpt-5.6-sol", "codex-auto-review"] {
            writeln!(
                f,
                r#"{{"timestamp":"2026-08-07T01:00:00.000Z","type":"turn_context","payload":{{"type":"turn_context","model":"{m}"}}}}"#
            )
            .unwrap();
        }
        writeln!(f, "{POPULATED}").unwrap();
        drop(f);

        let last = last_token_count(&path).unwrap();
        assert_eq!(last.models.len(), 3, "a session spans several models");
        assert!(last.models.contains("codex-auto-review"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn window_labels_read_naturally() {
        assert_eq!(window_label(Some(10_080), "x"), "Weekly limit");
        assert_eq!(window_label(Some(20_160), "x"), "2-week limit");
        assert_eq!(window_label(Some(1_440), "x"), "Daily limit");
        assert_eq!(window_label(Some(300), "x"), "5-hour limit");
        assert_eq!(window_label(Some(45), "x"), "45-minute limit");
        assert_eq!(window_label(None, "Primary limit"), "Primary limit");
    }

    #[test]
    fn last_token_count_wins_within_a_rollout() {
        // total_token_usage is session-cumulative, so only the final event is
        // the session total — summing every event would multiply-count.
        let dir = temp_dir("last-wins");
        let day = dir.join("2026/08/06");
        std::fs::create_dir_all(&day).unwrap();
        let path = day.join("rollout-test.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"timestamp":"2026-08-06T05:00:00.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":100,"output_tokens":10}}}},"rate_limits":null}}}}"#).unwrap();
        writeln!(f, "{POPULATED}").unwrap();
        drop(f);

        let last = last_token_count(&path).unwrap();
        let info = last.info.unwrap();
        assert_eq!(info.input_tokens, 895_204);
        assert_eq!(info.output_tokens, 8_326);

        let scan = scan_rollouts(&dir, 0, 0);
        assert_eq!(scan.sessions, 1);
        assert_eq!(scan.tokens.input, 895_204);
        assert_eq!(scan.tokens.cache_read, 824_064);
        assert!(scan.latest_limits.is_some());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn newest_non_null_limits_win_across_sessions() {
        let dir = temp_dir("newest-limits");
        let day = dir.join("2026/08/07");
        std::fs::create_dir_all(&day).unwrap();

        std::fs::write(day.join("rollout-a.jsonl"), format!("{POPULATED}\n")).unwrap();
        // A later session with only null limits must not clobber the real one.
        std::fs::write(day.join("rollout-b.jsonl"), format!("{ALL_NULL}\n")).unwrap();

        let scan = scan_rollouts(&dir, 0, 0);
        assert_eq!(scan.sessions, 2);
        let (_, limits) = scan.latest_limits.expect("real limits survive");
        assert_eq!(limits.plan_type.as_deref(), Some("plus"));
        assert_eq!(limits.primary.unwrap().used_percent, 95.0);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rollouts_without_token_count_are_skipped() {
        let dir = temp_dir("no-tokens");
        let day = dir.join("2026/08/07");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(
            day.join("rollout-x.jsonl"),
            "{\"timestamp\":\"2026-08-07T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{}}\n",
        )
        .unwrap();

        let scan = scan_rollouts(&dir, 0, 0);
        assert_eq!(scan.sessions, 0);
        assert!(scan.latest_limits.is_none());

        std::fs::remove_dir_all(&dir).ok();
    }
}
