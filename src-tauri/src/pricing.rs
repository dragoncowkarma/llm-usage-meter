//! Model → price table.
//!
//! Prices are per *million* tokens, in USD. Cache traffic is priced as a
//! multiple of the base input rate rather than as its own rate, because that is
//! how both vendors document it and it keeps the table honest when a model's
//! input price changes.
//!
//! The table is a compiled-in default. `~/.config/llm-usage-meter/pricing.json`
//! overrides it, so a stale build never silently reports a wrong bill.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Cache-write multipliers, applied to the base input rate.
///
/// Anthropic prices a 5-minute-TTL cache write at 1.25x input and a
/// 1-hour-TTL write at 2x. Claude Code transcripts report the two separately,
/// so we can bill them separately instead of averaging.
pub const CACHE_WRITE_5M_MULTIPLIER: f64 = 1.25;
pub const CACHE_WRITE_1H_MULTIPLIER: f64 = 2.0;
/// Cache reads cost ~0.1x the base input rate on both vendors.
pub const CACHE_READ_MULTIPLIER: f64 = 0.1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPrice {
    /// USD per million input tokens.
    pub input: f64,
    /// USD per million output tokens.
    pub output: f64,
    /// Multiplier on `input` for cache writes, when the vendor's ratio differs
    /// from the Anthropic default.
    #[serde(default = "default_cache_write")]
    pub cache_write: f64,
    /// Multiplier on `input` for cache reads.
    #[serde(default = "default_cache_read")]
    pub cache_read: f64,
}

fn default_cache_write() -> f64 {
    CACHE_WRITE_5M_MULTIPLIER
}
fn default_cache_read() -> f64 {
    CACHE_READ_MULTIPLIER
}

impl ModelPrice {
    const fn new(input: f64, output: f64) -> Self {
        Self {
            input,
            output,
            cache_write: CACHE_WRITE_5M_MULTIPLIER,
            cache_read: CACHE_READ_MULTIPLIER,
        }
    }
}

/// Anthropic rates, per million tokens. Verified 2026-08-08.
const ANTHROPIC_PRICES: &[(&str, ModelPrice)] = &[
    ("claude-fable-5", ModelPrice::new(10.0, 50.0)),
    ("claude-mythos-5", ModelPrice::new(10.0, 50.0)),
    ("claude-opus-5", ModelPrice::new(5.0, 25.0)),
    ("claude-opus-4-8", ModelPrice::new(5.0, 25.0)),
    ("claude-opus-4-7", ModelPrice::new(5.0, 25.0)),
    ("claude-opus-4-6", ModelPrice::new(5.0, 25.0)),
    ("claude-opus-4-5", ModelPrice::new(5.0, 25.0)),
    ("claude-sonnet-5", ModelPrice::new(3.0, 15.0)),
    ("claude-sonnet-4-6", ModelPrice::new(3.0, 15.0)),
    ("claude-sonnet-4-5", ModelPrice::new(3.0, 15.0)),
    ("claude-haiku-4-5", ModelPrice::new(1.0, 5.0)),
];

/// OpenAI rates used by Codex.
///
/// These are *not* verified against a first-party source the way the Anthropic
/// table is — treat any Codex dollar figure as an estimate and override via
/// `pricing.json` if it matters. The UI labels Codex cost accordingly.
const OPENAI_PRICES: &[(&str, ModelPrice)] = &[
    ("gpt-5", ModelPrice::new(1.25, 10.0)),
    ("gpt-5-codex", ModelPrice::new(1.25, 10.0)),
    ("gpt-5-mini", ModelPrice::new(0.25, 2.0)),
    ("o4-mini", ModelPrice::new(1.1, 4.4)),
];

pub struct PriceTable {
    prices: HashMap<String, ModelPrice>,
    /// True when a user-supplied override file was loaded.
    pub overridden: bool,
}

impl PriceTable {
    pub fn builtin() -> Self {
        let mut prices = HashMap::new();
        for (id, p) in ANTHROPIC_PRICES.iter().chain(OPENAI_PRICES.iter()) {
            prices.insert((*id).to_string(), *p);
        }
        Self {
            prices,
            overridden: false,
        }
    }

    /// Load the built-in table, then merge any user overrides on top.
    pub fn load(override_path: &Path) -> Self {
        let mut table = Self::builtin();
        if let Ok(text) = std::fs::read_to_string(override_path) {
            if let Ok(user) = serde_json::from_str::<HashMap<String, ModelPrice>>(&text) {
                if !user.is_empty() {
                    table.prices.extend(user);
                    table.overridden = true;
                }
            }
        }
        table
    }

    /// Look up a model, tolerating the version suffixes vendors append.
    ///
    /// Transcripts carry ids like `claude-opus-4-5-20251101` and
    /// `gpt-5-codex-2025-09-15`; the longest matching prefix wins so that
    /// `gpt-5-codex` beats `gpt-5` for `gpt-5-codex-...`.
    pub fn lookup(&self, model: &str) -> Option<ModelPrice> {
        if let Some(p) = self.prices.get(model) {
            return Some(*p);
        }
        self.prices
            .iter()
            .filter(|(id, _)| model.starts_with(id.as_str()))
            .max_by_key(|(id, _)| id.len())
            .map(|(_, p)| *p)
    }

    /// Cost in USD for one request's token counts.
    ///
    /// `cache_write_1h` is split out because Anthropic prices the 1-hour TTL at
    /// a different multiple; pass 0 when the vendor doesn't distinguish.
    pub fn cost_usd(
        &self,
        model: &str,
        input: u64,
        output: u64,
        cache_write_5m: u64,
        cache_write_1h: u64,
        cache_read: u64,
    ) -> Option<f64> {
        let p = self.lookup(model)?;
        let m = 1_000_000.0;
        Some(
            (input as f64 / m) * p.input
                + (output as f64 / m) * p.output
                + (cache_write_5m as f64 / m) * p.input * p.cache_write
                + (cache_write_1h as f64 / m) * p.input * CACHE_WRITE_1H_MULTIPLIER
                + (cache_read as f64 / m) * p.input * p.cache_read,
        )
    }

    /// Table size — used by tests and useful for a future diagnostics view.
    #[allow(dead_code)]
    pub fn known_models(&self) -> usize {
        self.prices.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_id_lookup() {
        let t = PriceTable::builtin();
        assert_eq!(t.lookup("claude-opus-5").unwrap().input, 5.0);
        assert_eq!(t.lookup("claude-opus-5").unwrap().output, 25.0);
        assert_eq!(t.lookup("claude-haiku-4-5").unwrap().output, 5.0);
    }

    #[test]
    fn dated_suffix_resolves_to_base_model() {
        let t = PriceTable::builtin();
        assert_eq!(t.lookup("claude-haiku-4-5-20251001").unwrap().input, 1.0);
    }

    #[test]
    fn longest_prefix_wins() {
        // `gpt-5-codex-2025-09-15` must not resolve to plain `gpt-5` if the
        // codex-specific entry is a longer match.
        let t = PriceTable::builtin();
        let codex = t.lookup("gpt-5-codex-2025-09-15").unwrap();
        let mini = t.lookup("gpt-5-mini-preview").unwrap();
        assert_eq!(codex.input, 1.25);
        assert_eq!(mini.input, 0.25);
    }

    #[test]
    fn unknown_model_is_none_not_zero() {
        // Returning 0.0 here would silently understate a bill.
        let t = PriceTable::builtin();
        assert!(t.lookup("some-model-we-have-never-seen").is_none());
        assert!(t
            .cost_usd("some-model-we-have-never-seen", 1000, 1000, 0, 0, 0)
            .is_none());
    }

    #[test]
    fn cost_applies_each_multiplier() {
        let t = PriceTable::builtin();
        // 1M input at $5, 1M output at $25, 1M 5m-write at 5*1.25,
        // 1M 1h-write at 5*2.0, 1M read at 5*0.1.
        let cost = t
            .cost_usd("claude-opus-5", 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000)
            .unwrap();
        let expected = 5.0 + 25.0 + 6.25 + 10.0 + 0.5;
        assert!((cost - expected).abs() < 1e-9, "got {cost}, want {expected}");
    }

    #[test]
    fn cache_reads_are_an_order_of_magnitude_cheaper_than_input() {
        let t = PriceTable::builtin();
        let input_only = t.cost_usd("claude-opus-5", 1_000_000, 0, 0, 0, 0).unwrap();
        let read_only = t.cost_usd("claude-opus-5", 0, 0, 0, 0, 1_000_000).unwrap();
        assert!((input_only / read_only - 10.0).abs() < 1e-9);
    }
}
