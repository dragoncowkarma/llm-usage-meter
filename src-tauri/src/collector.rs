//! Orchestration: run every provider, cache the result, hold settings.

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::model::{ProviderStatus, UsageReport, UsageSnapshot};
use crate::platform::HostEnv;
use crate::pricing::PriceTable;
use crate::providers::{self, Detection, ProviderContext, UsageProvider};
use crate::util;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Auto-refresh period in seconds.
    pub refresh_interval_secs: u64,
    /// How far back token/activity totals reach.
    pub lookback_days: u32,
    /// Provider ids the user has switched off.
    pub disabled_providers: Vec<String>,
    /// Show the peak quota percentage as text next to the menu bar icon.
    pub show_percent_in_menu_bar: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            refresh_interval_secs: 600, // 10 minutes
            lookback_days: 30,
            disabled_providers: Vec::new(),
            show_percent_in_menu_bar: false,
        }
    }
}

impl Settings {
    fn sanitised(mut self) -> Self {
        // A 5-second poll would hammer the disk for no benefit; a 24-hour one
        // makes the meter useless. Clamp rather than reject.
        self.refresh_interval_secs = self.refresh_interval_secs.clamp(30, 6 * 3600);
        self.lookback_days = self.lookback_days.clamp(1, 365);
        self
    }
}

/// Identity of a registered provider, for the settings list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub id: String,
    pub display_name: String,
}

pub struct AppState {
    pub env: HostEnv,
    pub prices: PriceTable,
    pub config_dir: PathBuf,
    settings: Mutex<Settings>,
    cache: Mutex<Option<UsageReport>>,
    registry: Vec<Box<dyn UsageProvider>>,
}

impl AppState {
    pub fn new() -> Self {
        let env = HostEnv::detect();
        let config_dir = env.paths.config_root().join("llm-usage-meter");
        let _ = std::fs::create_dir_all(&config_dir);

        let prices = PriceTable::load(&config_dir.join("pricing.json"));
        let settings = load_settings(&config_dir.join("settings.json"));

        let mut registry = providers::registry();
        // Demo mode appends a synthetic card so the UI can be worked on
        // regardless of which tools are installed.
        if std::env::var("LLM_METER_DEMO").is_ok() {
            registry.push(Box::new(providers::dummy::DummyProvider::new(
                "demo", "Demo Tool",
            )));
        }

        Self {
            env,
            prices,
            config_dir,
            settings: Mutex::new(settings),
            cache: Mutex::new(None),
            registry,
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().expect("settings mutex").clone()
    }

    /// Every registered provider, including ones the user has switched off.
    ///
    /// The report omits disabled providers, so settings needs this separate
    /// list — otherwise switching a provider off would remove the only control
    /// that could switch it back on.
    pub fn provider_list(&self) -> Vec<ProviderInfo> {
        self.registry
            .iter()
            .map(|p| ProviderInfo {
                id: p.id().to_string(),
                display_name: p.display_name().to_string(),
            })
            .collect()
    }

    pub fn update_settings(&self, next: Settings) -> Settings {
        let next = next.sanitised();
        *self.settings.lock().expect("settings mutex") = next.clone();
        let path = self.config_dir.join("settings.json");
        if let Ok(json) = serde_json::to_string_pretty(&next) {
            let _ = std::fs::write(path, json);
        }
        next
    }

    pub fn cached(&self) -> Option<UsageReport> {
        self.cache.lock().expect("cache mutex").clone()
    }

    /// True when `path` exactly matches a `SourceRef.path` this app itself
    /// put in the most recently collected report.
    ///
    /// `reveal_source` is the one IPC command that reaches outside the app's
    /// own state to open something in Finder, so it must not simply trust
    /// whatever string the webview sends — that would let any future bug that
    /// lets untrusted content run in the webview reveal an arbitrary path on
    /// disk. Checking against the report is the narrowest correct rule: it is
    /// exactly the set of paths this app has already told the user about.
    pub fn is_known_source_path(&self, path: &str) -> bool {
        self.cache
            .lock()
            .expect("cache mutex")
            .as_ref()
            .is_some_and(|report| {
                report
                    .snapshots
                    .iter()
                    .any(|s| s.sources.iter().any(|src| src.path == path))
            })
    }

    /// Run every enabled provider and replace the cache.
    pub fn refresh(&self) -> UsageReport {
        let settings = self.settings();
        let now = util::now_unix();
        let ctx = ProviderContext {
            env: &self.env,
            prices: &self.prices,
            now,
            lookback_days: settings.lookback_days,
        };

        let mut snapshots = Vec::with_capacity(self.registry.len());
        for provider in &self.registry {
            if settings.disabled_providers.iter().any(|d| d == provider.id()) {
                continue;
            }
            snapshots.push(collect_one(provider.as_ref(), &ctx));
        }

        let peak_percent = snapshots
            .iter()
            .filter(|s| s.status == ProviderStatus::Ok)
            .filter_map(|s| s.headline_quota())
            .filter(|q| q.is_active)
            .map(|q| q.used_percent)
            .fold(None::<f64>, |acc, v| Some(acc.map_or(v, |a| a.max(v))));

        let report = UsageReport {
            snapshots,
            generated_at: now,
            platform: self.env.paths.platform_tag().to_string(),
            peak_percent,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
        };

        *self.cache.lock().expect("cache mutex") = Some(report.clone());
        report
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// Collect one provider, converting both errors and panics into a snapshot.
///
/// A panic here would otherwise cross the Tauri command boundary and take the
/// whole menu bar app down; one provider tripping over an unexpected file
/// should cost one card, not the app.
fn collect_one(provider: &dyn UsageProvider, ctx: &ProviderContext) -> UsageSnapshot {
    let id = provider.id();
    let name = provider.display_name();

    let detection = std::panic::catch_unwind(AssertUnwindSafe(|| provider.detect(ctx)))
        .unwrap_or(Detection::NotInstalled);

    if matches!(detection, Detection::NotInstalled) {
        let mut snap = UsageSnapshot::empty(id, name, ProviderStatus::NotInstalled);
        snap.notes.push(format!("{name} does not appear to be installed."));
        return snap;
    }

    match std::panic::catch_unwind(AssertUnwindSafe(|| provider.collect(ctx))) {
        Ok(Ok(snap)) => snap,
        Ok(Err(e)) => {
            let mut snap = UsageSnapshot::empty(id, name, ProviderStatus::Error);
            snap.error = Some(e.to_string());
            snap
        }
        Err(_) => {
            let mut snap = UsageSnapshot::empty(id, name, ProviderStatus::Error);
            snap.error = Some("provider panicked while reading local files".into());
            snap
        }
    }
}

fn load_settings(path: &std::path::Path) -> Settings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Settings>(&t).ok())
        .unwrap_or_default()
        .sanitised()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::MeterError;
    use crate::model::QuotaWindow;

    struct Exploding;
    impl UsageProvider for Exploding {
        fn id(&self) -> &'static str {
            "boom"
        }
        fn display_name(&self) -> &'static str {
            "Boom"
        }
        fn detect(&self, _: &ProviderContext) -> Detection {
            Detection::Installed { root: "/".into() }
        }
        fn collect(&self, _: &ProviderContext) -> crate::error::Result<UsageSnapshot> {
            panic!("simulated provider crash");
        }
    }

    struct Failing;
    impl UsageProvider for Failing {
        fn id(&self) -> &'static str {
            "fail"
        }
        fn display_name(&self) -> &'static str {
            "Fail"
        }
        fn detect(&self, _: &ProviderContext) -> Detection {
            Detection::Installed { root: "/".into() }
        }
        fn collect(&self, _: &ProviderContext) -> crate::error::Result<UsageSnapshot> {
            Err(MeterError::Other("nope".into()))
        }
    }

    fn test_ctx<'a>(env: &'a HostEnv, prices: &'a PriceTable) -> ProviderContext<'a> {
        ProviderContext {
            env,
            prices,
            now: util::now_unix(),
            lookback_days: 30,
        }
    }

    #[test]
    fn a_panicking_provider_becomes_an_error_card() {
        let env = HostEnv::detect();
        let prices = PriceTable::builtin();
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // keep the test output clean
        let snap = collect_one(&Exploding, &test_ctx(&env, &prices));
        std::panic::set_hook(prev);

        assert_eq!(snap.status, ProviderStatus::Error);
        assert!(snap.error.unwrap().contains("panicked"));
    }

    #[test]
    fn a_failing_provider_becomes_an_error_card() {
        let env = HostEnv::detect();
        let prices = PriceTable::builtin();
        let snap = collect_one(&Failing, &test_ctx(&env, &prices));
        assert_eq!(snap.status, ProviderStatus::Error);
        assert_eq!(snap.error.as_deref(), Some("nope"));
    }

    #[test]
    fn settings_are_clamped_to_sane_bounds() {
        let s = Settings {
            refresh_interval_secs: 1,
            lookback_days: 100_000,
            ..Default::default()
        }
        .sanitised();
        assert_eq!(s.refresh_interval_secs, 30);
        assert_eq!(s.lookback_days, 365);

        let s = Settings {
            refresh_interval_secs: u64::MAX,
            lookback_days: 0,
            ..Default::default()
        }
        .sanitised();
        assert_eq!(s.refresh_interval_secs, 6 * 3600);
        assert_eq!(s.lookback_days, 1);
    }

    #[test]
    fn default_refresh_matches_the_documented_ten_minutes() {
        assert_eq!(Settings::default().refresh_interval_secs, 600);
    }

    #[test]
    fn refresh_produces_one_snapshot_per_enabled_provider() {
        let state = AppState::new();
        let report = state.refresh();
        assert_eq!(report.snapshots.len(), state.registry.len());
        assert!(!report.platform.is_empty());
        assert!(state.cached().is_some());
    }

    /// Diagnostic: run the real collector against this machine and print what
    /// it found. Ignored by default because the output depends on which tools
    /// are installed; run it when you want to eyeball the numbers against the
    /// tools' own `/usage` and `/status` output.
    ///
    ///     cargo test dump_real_report -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_real_report() {
        let state = AppState::new();
        let report = state.refresh();

        println!("\nplatform: {}   peak: {:?}", report.platform, report.peak_percent);
        for s in &report.snapshots {
            println!("\n=== {} [{:?}/{:?}] {}ms", s.display_name, s.status, s.confidence, s.collect_ms);
            if let Some(p) = &s.plan {
                println!("  plan: {p}");
            }
            for q in &s.quotas {
                println!(
                    "  quota {:<18} {:>6.1}%  active={}  resets_at={:?}",
                    q.label, q.used_percent, q.is_active, q.resets_at
                );
            }
            if let Some(t) = &s.tokens {
                println!(
                    "  tokens in={} out={} cw={} cr={}  total={}",
                    t.input, t.output, t.cache_write, t.cache_read, t.billable_total()
                );
            }
            for m in &s.models {
                println!(
                    "    model {:<24} reqs={:<6} tokens={:<14} cost={:?}",
                    m.model, m.requests, m.tokens.billable_total(), m.cost_usd
                );
            }
            if let Some(c) = &s.cost {
                println!("  cost: {:?} partial={}", c.amount, c.partial);
            }
            if let Some(a) = &s.activity {
                println!(
                    "  activity today={} total={} sessions_today={} rate_limit_hits={}",
                    a.requests_today, a.requests_total, a.sessions_today, a.rate_limit_hits
                );
            }
            for n in &s.notes {
                println!("  note: {n}");
            }
            if let Some(e) = &s.error {
                println!("  ERROR: {e}");
            }
        }
        // The exact bytes the webview receives. Handy for eyeballing that the
        // hand-maintained TypeScript types in src/types/usage.ts still match
        // what serde emits.
        println!("\n---BEGIN JSON---");
        println!("{}", serde_json::to_string(&report).unwrap());
        println!("---END JSON---\n");
    }

    #[test]
    fn unknown_source_path_is_rejected_before_any_refresh() {
        // Before the first collection, nothing is "known" — reveal_source
        // must fail closed, not open, when the cache is empty.
        let state = AppState::new();
        assert!(!state.is_known_source_path("/etc/passwd"));
        assert!(!state.is_known_source_path(""));
    }

    #[test]
    fn only_paths_the_report_actually_cited_are_known() {
        struct WithSource;
        impl UsageProvider for WithSource {
            fn id(&self) -> &'static str {
                "withsource"
            }
            fn display_name(&self) -> &'static str {
                "WithSource"
            }
            fn detect(&self, _: &ProviderContext) -> Detection {
                Detection::Installed { root: "/".into() }
            }
            fn collect(&self, ctx: &ProviderContext) -> crate::error::Result<UsageSnapshot> {
                let mut snap = UsageSnapshot::empty(self.id(), self.display_name(), ProviderStatus::Ok);
                snap.sources.push(crate::model::SourceRef {
                    kind: "test".into(),
                    path: "/known/path".into(),
                    observed_at: Some(ctx.now),
                });
                Ok(snap)
            }
        }

        let env = HostEnv::detect();
        let prices = PriceTable::builtin();
        let ctx = test_ctx(&env, &prices);
        let snap = collect_one(&WithSource, &ctx);

        let state = AppState::new();
        *state.cache.lock().unwrap() = Some(UsageReport {
            snapshots: vec![snap],
            generated_at: ctx.now,
            platform: "test".into(),
            peak_percent: None,
            app_version: "0.0.0".into(),
        });

        assert!(state.is_known_source_path("/known/path"));
        // Not the exact string the report cited — must not match by prefix,
        // substring, or any other loose rule. An attacker-chosen path that
        // merely starts with a known directory is exactly the case this
        // check exists to block.
        assert!(!state.is_known_source_path("/known/path/../../etc/passwd"));
        assert!(!state.is_known_source_path("/known/path/extra"));
        assert!(!state.is_known_source_path("/completely/different"));
        assert!(!state.is_known_source_path(""));
    }

    #[test]
    fn peak_percent_ignores_inactive_and_uninstalled() {
        // Only active windows on healthy providers should drive the menu bar
        // number — an uninstalled tool must not report 0% and mask a real 90%.
        let mut ok = UsageSnapshot::empty("a", "A", ProviderStatus::Ok);
        ok.quotas = vec![QuotaWindow::new("w", "W", 61.0)];
        let mut absent = UsageSnapshot::empty("b", "B", ProviderStatus::NotInstalled);
        absent.quotas = vec![QuotaWindow::new("w", "W", 99.0)];

        let peak = [ok, absent]
            .iter()
            .filter(|s| s.status == ProviderStatus::Ok)
            .filter_map(|s| s.headline_quota())
            .filter(|q| q.is_active)
            .map(|q| q.used_percent)
            .fold(None::<f64>, |acc, v| Some(acc.map_or(v, |a: f64| a.max(v))));
        assert_eq!(peak, Some(61.0));
    }
}
