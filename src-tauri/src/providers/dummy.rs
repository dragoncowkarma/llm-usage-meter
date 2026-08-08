//! A synthetic provider.
//!
//! Two jobs: it is the worked example of the Phase 3 plugin contract (copy this
//! file to add a tool), and it lets the popup UI be developed and screenshotted
//! without depending on which CLIs happen to be installed.
//!
//! Enable at runtime with `LLM_METER_DEMO=1`.

use crate::error::Result;
use crate::model::*;
use crate::providers::{Detection, ProviderContext, UsageProvider};
use crate::util;

pub struct DummyProvider {
    id: &'static str,
    name: &'static str,
}

impl DummyProvider {
    pub fn new(id: &'static str, name: &'static str) -> Self {
        Self { id, name }
    }
}

impl UsageProvider for DummyProvider {
    fn id(&self) -> &'static str {
        self.id
    }

    fn display_name(&self) -> &'static str {
        self.name
    }

    fn detect(&self, _ctx: &ProviderContext) -> Detection {
        Detection::Installed {
            root: "<synthetic>".into(),
        }
    }

    fn collect(&self, ctx: &ProviderContext) -> Result<UsageSnapshot> {
        let started = util::now_millis();
        let mut snap = UsageSnapshot::empty(self.id, self.name, ProviderStatus::Ok);

        snap.confidence = Confidence::Authoritative;
        snap.plan = Some("demo_pro".into());
        snap.quotas = vec![
            QuotaWindow::new("session", "5-hour session", 42.0)
                .resets_at(Some(ctx.now + 3 * 3600))
                .window_minutes(Some(300)),
            QuotaWindow::new("weekly", "Weekly (all models)", 78.0)
                .resets_at(Some(ctx.now + 2 * 86_400))
                .window_minutes(Some(10_080)),
            QuotaWindow::new("credits", "Extra usage credits", 12.0)
                .spend(Money::usd_cents(1_200), Some(Money::usd_cents(10_000))),
        ];

        let tokens = TokenTotals {
            input: 145_233,
            output: 61_004,
            cache_write: 2_104_886,
            cache_read: 18_442_017,
            reasoning: 22_310,
        };
        snap.models = vec![
            ModelUsage {
                model: "claude-opus-5".into(),
                tokens: tokens.clone(),
                requests: 812,
                cost_usd: ctx.prices.cost_usd("claude-opus-5", 145_233, 61_004, 2_104_886, 0, 18_442_017),
            },
        ];
        snap.cost = snap.models[0].cost_usd.map(|usd| CostEstimate {
            amount: Money::from_usd(usd),
            partial: false,
            basis: "synthetic data".into(),
        });
        snap.tokens = Some(tokens);
        snap.activity = Some(ActivityStats {
            requests_today: 812,
            requests_total: 24_119,
            sessions_today: 4,
            last_activity_at: Some(ctx.now - 240),
            rate_limit_hits: 0,
        });
        snap.notes.push("Synthetic data — LLM_METER_DEMO is set.".into());
        snap.collect_ms = util::now_millis().saturating_sub(started);
        Ok(snap)
    }
}
