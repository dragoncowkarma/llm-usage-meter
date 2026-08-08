//! The provider-agnostic domain model.
//!
//! Every provider — no matter how different its underlying storage — normalises
//! into a `UsageSnapshot`. The frontend knows only these types, which is what
//! keeps the plugin system (Phase 3) genuinely pluggable.

use serde::{Deserialize, Serialize};

/// How much the numbers in a snapshot can be trusted.
///
/// This is the single most important field in the model. These three tools
/// expose *wildly* different quality of local data, and quietly rendering a
/// guess next to a server-reported fact would make the whole app untrustworthy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Confidence {
    /// The vendor's own backend computed this and the CLI cached it locally.
    /// Matches what the tool's own `/usage` or `/status` command reports.
    Authoritative,
    /// Computed by us from a complete local ledger of per-request token counts.
    /// Exact for tokens; cost depends on our price table being current.
    Derived,
    /// An activity proxy. No token ledger exists locally, so this counts
    /// requests/steps and surfaces observed rate-limit errors instead.
    Heuristic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderStatus {
    /// Data collected successfully.
    Ok,
    /// The tool does not appear to be installed for this user.
    NotInstalled,
    /// Installed, but some data was unreadable — the snapshot is partial.
    Degraded,
    /// Collection failed outright.
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Normal,
    Warning,
    Critical,
}

impl Severity {
    /// Shared thresholds so every provider colours its bars identically.
    pub fn from_percent(pct: f64) -> Self {
        if pct >= 90.0 {
            Severity::Critical
        } else if pct >= 75.0 {
            Severity::Warning
        } else {
            Severity::Normal
        }
    }
}

/// A monetary amount held in minor units to avoid float drift.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Money {
    pub amount_minor: i64,
    pub currency: String,
    /// Number of decimal places, e.g. 2 for USD cents.
    pub exponent: u8,
}

impl Money {
    pub fn usd_cents(cents: i64) -> Self {
        Self {
            amount_minor: cents,
            currency: "USD".into(),
            exponent: 2,
        }
    }

    pub fn from_usd(dollars: f64) -> Self {
        Self::usd_cents((dollars * 100.0).round() as i64)
    }
}

/// One rate-limit or spend window, e.g. "5-hour session" or "weekly (all models)".
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub id: String,
    pub label: String,
    /// 0–100. The primary number the UI renders.
    pub used_percent: f64,
    /// Unix seconds at which this window rolls over.
    pub resets_at: Option<i64>,
    /// Length of the rolling window, when the vendor reports it.
    pub window_minutes: Option<u32>,
    pub severity: Severity,
    /// False for windows the vendor marks dormant (e.g. a session with no
    /// activity yet); the UI de-emphasises these rather than hiding them.
    pub is_active: bool,
    pub used_amount: Option<Money>,
    pub limit_amount: Option<Money>,
}

impl QuotaWindow {
    pub fn new(id: impl Into<String>, label: impl Into<String>, used_percent: f64) -> Self {
        let pct = used_percent.clamp(0.0, 100.0);
        Self {
            id: id.into(),
            label: label.into(),
            used_percent: pct,
            resets_at: None,
            window_minutes: None,
            severity: Severity::from_percent(pct),
            is_active: true,
            used_amount: None,
            limit_amount: None,
        }
    }

    pub fn resets_at(mut self, ts: Option<i64>) -> Self {
        self.resets_at = ts;
        self
    }

    pub fn window_minutes(mut self, m: Option<u32>) -> Self {
        self.window_minutes = m;
        self
    }

    pub fn active(mut self, active: bool) -> Self {
        self.is_active = active;
        self
    }

    pub fn spend(mut self, used: Money, limit: Option<Money>) -> Self {
        self.used_amount = Some(used);
        self.limit_amount = limit;
        self
    }
}

/// Token counts, split the way modern APIs actually bill them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenTotals {
    pub input: u64,
    pub output: u64,
    /// Tokens written into the prompt cache (billed at a premium).
    pub cache_write: u64,
    /// Tokens served from the prompt cache (billed at a large discount).
    pub cache_read: u64,
    pub reasoning: u64,
}

impl TokenTotals {
    /// Sum of every billable category. Note `input` excludes cache traffic in
    /// both the Anthropic and OpenAI wire formats, so this is a plain sum.
    pub fn billable_total(&self) -> u64 {
        self.input + self.output + self.cache_write + self.cache_read
    }

    pub fn add(&mut self, other: &TokenTotals) {
        self.input += other.input;
        self.output += other.output;
        self.cache_write += other.cache_write;
        self.cache_read += other.cache_read;
        self.reasoning += other.reasoning;
    }
}

/// Per-model breakdown, so the UI can explain *why* a bill is what it is.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    pub model: String,
    pub tokens: TokenTotals,
    pub requests: u64,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostEstimate {
    pub amount: Money,
    /// True when the price table had no entry for some model that was used, so
    /// the total is a floor rather than a full estimate.
    pub partial: bool,
    /// Human-readable note about what the number does and does not include.
    pub basis: String,
}

/// Request/session counters. For providers with no token ledger this is the
/// only thing we can honestly report.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityStats {
    pub requests_today: u64,
    pub requests_total: u64,
    pub sessions_today: u64,
    pub last_activity_at: Option<i64>,
    /// Rate-limit rejections observed in local logs within the lookback window.
    pub rate_limit_hits: u64,
}

/// Where a number came from, surfaced in the UI so the user can audit us.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRef {
    pub kind: String,
    pub path: String,
    /// Unix seconds the underlying file was last written, when known. Lets the
    /// UI say "cached 4h ago" instead of implying the number is live.
    pub observed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    pub provider_id: String,
    pub display_name: String,
    pub status: ProviderStatus,
    pub confidence: Confidence,
    /// Subscription tier as the vendor names it, e.g. "claude_pro", "plus".
    pub plan: Option<String>,
    pub quotas: Vec<QuotaWindow>,
    pub tokens: Option<TokenTotals>,
    pub models: Vec<ModelUsage>,
    pub cost: Option<CostEstimate>,
    pub activity: Option<ActivityStats>,
    pub sources: Vec<SourceRef>,
    /// Unix seconds this snapshot was produced.
    pub collected_at: i64,
    /// How long collection took — the popup surfaces this when a provider is slow.
    pub collect_ms: u64,
    /// Caveats worth showing the user, e.g. "quota cached 3h ago".
    pub notes: Vec<String>,
    pub error: Option<String>,
}

impl UsageSnapshot {
    pub fn empty(provider_id: &str, display_name: &str, status: ProviderStatus) -> Self {
        Self {
            provider_id: provider_id.into(),
            display_name: display_name.into(),
            status,
            confidence: Confidence::Heuristic,
            plan: None,
            quotas: Vec::new(),
            tokens: None,
            models: Vec::new(),
            cost: None,
            activity: None,
            sources: Vec::new(),
            collected_at: crate::util::now_unix(),
            collect_ms: 0,
            notes: Vec::new(),
            error: None,
        }
    }

    /// The window the menu bar title should show: the most-consumed active one.
    pub fn headline_quota(&self) -> Option<&QuotaWindow> {
        self.quotas
            .iter()
            .filter(|q| q.is_active)
            .max_by(|a, b| a.used_percent.total_cmp(&b.used_percent))
            .or_else(|| {
                self.quotas
                    .iter()
                    .max_by(|a, b| a.used_percent.total_cmp(&b.used_percent))
            })
    }
}

/// The full payload handed to the frontend on every refresh.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReport {
    pub snapshots: Vec<UsageSnapshot>,
    pub generated_at: i64,
    pub platform: String,
    /// Highest active quota percentage across all providers — drives the tray title.
    pub peak_percent: Option<f64>,
    pub app_version: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_thresholds() {
        assert_eq!(Severity::from_percent(0.0), Severity::Normal);
        assert_eq!(Severity::from_percent(74.9), Severity::Normal);
        assert_eq!(Severity::from_percent(75.0), Severity::Warning);
        assert_eq!(Severity::from_percent(89.9), Severity::Warning);
        assert_eq!(Severity::from_percent(90.0), Severity::Critical);
        assert_eq!(Severity::from_percent(100.0), Severity::Critical);
    }

    #[test]
    fn quota_percent_is_clamped() {
        // Vendors have been known to report >100 once a limit is blown through.
        assert_eq!(QuotaWindow::new("x", "X", 137.0).used_percent, 100.0);
        assert_eq!(QuotaWindow::new("x", "X", -4.0).used_percent, 0.0);
    }

    #[test]
    fn headline_prefers_active_window() {
        let mut s = UsageSnapshot::empty("t", "T", ProviderStatus::Ok);
        s.quotas = vec![
            QuotaWindow::new("a", "Inactive but high", 99.0).active(false),
            QuotaWindow::new("b", "Active", 40.0),
        ];
        assert_eq!(s.headline_quota().unwrap().id, "b");
    }

    #[test]
    fn headline_falls_back_when_nothing_is_active() {
        let mut s = UsageSnapshot::empty("t", "T", ProviderStatus::Ok);
        s.quotas = vec![QuotaWindow::new("a", "A", 12.0).active(false)];
        assert_eq!(s.headline_quota().unwrap().id, "a");
    }

    #[test]
    fn token_totals_sum_every_billable_bucket() {
        let t = TokenTotals {
            input: 10,
            output: 20,
            cache_write: 30,
            cache_read: 40,
            reasoning: 5, // reasoning is a subset of output, never added twice
        };
        assert_eq!(t.billable_total(), 100);
    }

    #[test]
    fn money_rounds_to_cents() {
        assert_eq!(Money::from_usd(0.0).amount_minor, 0);
        assert_eq!(Money::from_usd(1.25).amount_minor, 125);
        assert_eq!(Money::from_usd(679.204).amount_minor, 67_920);
        assert_eq!(Money::from_usd(679.206).amount_minor, 67_921);

        // Binary floats cannot represent every decimal midpoint, so a value
        // like 1.005 is really 1.00499999999999989 and rounds *down*. That is
        // acceptable here — this is a display conversion applied once to an
        // already-summed total, not money we do arithmetic on — but it is
        // pinned so nobody "fixes" it into a half-cent drift.
        assert_eq!(Money::from_usd(1.005).amount_minor, 100);
    }
}
