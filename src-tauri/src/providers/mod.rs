//! Phase 3: the plugin system.
//!
//! `UsageProvider` is the Strategy interface. Every tool — today Claude Code,
//! Codex and Antigravity; tomorrow Gemini CLI or Cursor — implements it, and
//! the collector only ever talks to the trait. Adding a tool means writing one
//! file here and adding one line to `registry()`.

pub mod antigravity;
pub mod claude_code;
pub mod codex;
pub mod dummy;

use crate::error::Result;
use crate::model::UsageSnapshot;
use crate::platform::HostEnv;
use crate::pricing::PriceTable;

/// Everything a provider is handed at collection time.
///
/// Providers get read-only access; they must not mutate anything the host tool
/// owns. This app only ever reads the CLIs' own files.
pub struct ProviderContext<'a> {
    pub env: &'a HostEnv,
    pub prices: &'a PriceTable,
    /// Unix seconds; passed in rather than read per-provider so every snapshot
    /// in a report shares one clock.
    pub now: i64,
    /// How far back activity counters look, in days.
    pub lookback_days: u32,
}

/// Result of a cheap "is this tool even here?" check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detection {
    /// Found, with the root directory we'll read from.
    Installed { root: String },
    NotInstalled,
}

/// The plugin interface (Phase 3).
///
/// Implementations must be cheap to construct and safe to call from a
/// background thread; `collect` is expected to do blocking file I/O.
pub trait UsageProvider: Send + Sync {
    /// Stable machine id, used as a settings key and a React list key.
    fn id(&self) -> &'static str;

    /// Human-facing name shown in the popup.
    fn display_name(&self) -> &'static str;

    /// Cheap existence check. Runs before `collect` so an uninstalled tool
    /// costs one `stat`, not a directory walk.
    fn detect(&self, ctx: &ProviderContext) -> Detection;

    /// Read local state and normalise it into the shared domain model.
    fn collect(&self, ctx: &ProviderContext) -> Result<UsageSnapshot>;
}

/// The providers this build ships with, in display order.
pub fn registry() -> Vec<Box<dyn UsageProvider>> {
    vec![
        Box::new(claude_code::ClaudeCodeProvider),
        Box::new(codex::CodexProvider),
        Box::new(antigravity::AntigravityProvider),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProviderStatus;

    fn ctx_for<'a>(env: &'a HostEnv, prices: &'a PriceTable) -> ProviderContext<'a> {
        ProviderContext {
            env,
            prices,
            now: crate::util::now_unix(),
            lookback_days: 30,
        }
    }

    #[test]
    fn registry_ids_are_unique_and_stable() {
        let reg = registry();
        let mut ids: Vec<_> = reg.iter().map(|p| p.id()).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "provider ids must be unique");
        assert!(reg.iter().all(|p| !p.display_name().is_empty()));
    }

    #[test]
    fn dummy_provider_satisfies_the_interface() {
        // Guards the Phase 3 contract: a new plugin needs nothing but this trait.
        let env = HostEnv::detect();
        let prices = PriceTable::builtin();
        let ctx = ctx_for(&env, &prices);

        let p = dummy::DummyProvider::new("demo", "Demo Tool");
        assert!(matches!(p.detect(&ctx), Detection::Installed { .. }));
        let snap = p.collect(&ctx).unwrap();
        assert_eq!(snap.provider_id, "demo");
        assert_eq!(snap.status, ProviderStatus::Ok);
        assert!(!snap.quotas.is_empty());
        assert!(snap.headline_quota().is_some());
    }

    #[test]
    fn real_providers_never_panic_on_this_machine() {
        // Whether or not each tool is installed here, collection must degrade
        // to a snapshot rather than unwinding into the Tauri command layer.
        let env = HostEnv::detect();
        let prices = PriceTable::builtin();
        let ctx = ctx_for(&env, &prices);

        for p in registry() {
            match p.detect(&ctx) {
                Detection::Installed { .. } => {
                    let snap = p.collect(&ctx).expect("collect must not error once detected");
                    assert_eq!(snap.provider_id, p.id());
                }
                Detection::NotInstalled => {}
            }
        }
    }
}
