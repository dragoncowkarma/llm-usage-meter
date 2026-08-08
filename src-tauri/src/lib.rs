mod collector;
mod commands;
mod error;
mod model;
mod platform;
mod pricing;
mod providers;
mod tray;
mod util;

use std::time::Duration;

use tauri::{Manager, WindowEvent};

use collector::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::get_report,
            commands::list_providers,
            commands::get_settings,
            commands::save_settings,
            commands::reveal_source,
            commands::hide_popup,
            commands::quit_app,
        ])
        .setup(|app| {
            // Accessory policy keeps the app out of the Dock and the ⌘-Tab
            // switcher — it lives in the menu bar only.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            tray::build(app.handle())?;

            if let Some(window) = app.get_webview_window("main") {
                let handle = app.handle().clone();
                window.on_window_event(move |event| {
                    // Click-outside dismissal, the convention for menu bar popovers.
                    if let WindowEvent::Focused(false) = event {
                        if let Some(w) = handle.get_webview_window("main") {
                            let _ = w.hide();
                        }
                    }
                });
            }

            // First collection happens off the main thread so the menu bar icon
            // appears immediately rather than after the first disk scan.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                tray::refresh_and_publish(&handle);
                periodic_refresh(handle);
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running llm-usage-meter");
}

/// Auto-refresh loop.
///
/// Ticks on a short fixed interval and compares elapsed time against the
/// configured period, so changing the interval in settings takes effect within
/// one tick instead of after the current long sleep finishes.
fn periodic_refresh(handle: tauri::AppHandle) {
    const TICK: Duration = Duration::from_secs(5);
    let mut elapsed = 0u64;
    loop {
        std::thread::sleep(TICK);
        elapsed += TICK.as_secs();

        let interval = handle.state::<AppState>().settings().refresh_interval_secs;
        if elapsed >= interval {
            elapsed = 0;
            tray::refresh_and_publish(&handle);
        }
    }
}

/// Guards the IPC payload shape against drifting out of sync with the
/// hand-maintained TypeScript types in `src/types/usage.ts`.
///
/// There is no codegen tying the two sides together, so nothing else catches
/// a Rust field being renamed, added, or removed without a matching edit on
/// the TypeScript side. This module and its TypeScript counterpart
/// (`src/types/usage.contract.test.ts`) both check their own runtime shape
/// against the same checked-in file, `contract/usage-report-keys.json` — so a
/// change to either side that isn't mirrored *there* fails that language's
/// test suite immediately, rather than at runtime in the popover.
///
/// This catches the two costliest kinds of drift — a field renamed or a field
/// added/removed — because it compares key *names*. It does not catch a field
/// changing type while keeping its name (e.g. a string becoming a number);
/// nothing here can express that check without also getting a copy of the
/// Rust type system, and that's what an actual codegen tool (`ts-rs`/`specta`)
/// would be for.
#[cfg(test)]
mod schema_drift {
    use std::collections::BTreeSet;

    use serde_json::Value;

    use crate::collector::{ProviderInfo, Settings};
    use crate::model::*;

    /// The contract file's structure: `{ "$comment": "...", "TypeName": [...] }`.
    /// `$comment` is skipped by `key_sets()` (it's not a payload type).
    const CONTRACT: &str = include_str!("../../contract/usage-report-keys.json");

    fn key_sets() -> std::collections::BTreeMap<String, BTreeSet<String>> {
        let raw: std::collections::BTreeMap<String, Value> =
            serde_json::from_str(CONTRACT).expect("contract/usage-report-keys.json must be valid JSON");
        raw.into_iter()
            .filter(|(name, _)| name != "$comment")
            .map(|(name, value)| {
                let keys = value
                    .as_array()
                    .unwrap_or_else(|| panic!("contract entry {name:?} must be a JSON array"))
                    .iter()
                    .map(|v| v.as_str().unwrap().to_string())
                    .collect();
                (name, keys)
            })
            .collect()
    }

    /// The direct (non-recursive) object keys of one serialized value. Nested
    /// objects/arrays are left alone — their keys belong to *their own*
    /// contract entry, checked by a separate call, not folded into this one.
    fn top_level_keys<T: serde::Serialize>(value: &T) -> BTreeSet<String> {
        match serde_json::to_value(value).expect("value must serialize") {
            Value::Object(map) => map.keys().cloned().collect(),
            other => panic!("expected a JSON object, got {other:?}"),
        }
    }

    fn assert_matches(type_name: &str, actual: BTreeSet<String>, contract: &std::collections::BTreeMap<String, BTreeSet<String>>) {
        let expected = contract
            .get(type_name)
            .unwrap_or_else(|| panic!("no contract entry for {type_name:?} — add one to contract/usage-report-keys.json"));
        assert_eq!(
            &actual, expected,
            "{type_name}'s serialized keys no longer match contract/usage-report-keys.json.\n\
             If this field change was intentional, update the contract file AND \
             src/types/usage.ts in the same change — that's the whole point of this test."
        );
    }

    #[test]
    fn every_ipc_type_matches_the_shared_contract() {
        let contract = key_sets();

        assert_matches(
            "Money",
            top_level_keys(&Money {
                amount_minor: 0,
                currency: "USD".into(),
                exponent: 2,
            }),
            &contract,
        );
        assert_matches("QuotaWindow", top_level_keys(&QuotaWindow::new("id", "label", 0.0)), &contract);
        assert_matches("TokenTotals", top_level_keys(&TokenTotals::default()), &contract);
        assert_matches(
            "ModelUsage",
            top_level_keys(&ModelUsage {
                model: "m".into(),
                tokens: TokenTotals::default(),
                requests: 0,
                cost_usd: None,
            }),
            &contract,
        );
        assert_matches(
            "CostEstimate",
            top_level_keys(&CostEstimate {
                amount: Money {
                    amount_minor: 0,
                    currency: "USD".into(),
                    exponent: 2,
                },
                partial: false,
                basis: String::new(),
            }),
            &contract,
        );
        assert_matches("ActivityStats", top_level_keys(&ActivityStats::default()), &contract);
        assert_matches(
            "SourceRef",
            top_level_keys(&SourceRef {
                kind: "k".into(),
                path: "p".into(),
                observed_at: None,
            }),
            &contract,
        );
        assert_matches(
            "UsageSnapshot",
            top_level_keys(&UsageSnapshot::empty("id", "Name", ProviderStatus::Ok)),
            &contract,
        );
        assert_matches(
            "UsageReport",
            top_level_keys(&UsageReport {
                snapshots: Vec::new(),
                generated_at: 0,
                platform: "macos".into(),
                peak_percent: None,
                app_version: "0.0.0".into(),
            }),
            &contract,
        );
        assert_matches(
            "ProviderInfo",
            top_level_keys(&ProviderInfo {
                id: "id".into(),
                display_name: "Name".into(),
            }),
            &contract,
        );
        assert_matches("Settings", top_level_keys(&Settings::default()), &contract);
    }

    #[test]
    fn every_contract_entry_names_a_type_this_test_actually_checks() {
        // The inverse of the main test: if someone adds an entry to the
        // contract file for a type this module never constructs, that entry
        // is silently unverified. Keep the two lists — the contract file's
        // keys and the type names covered above — in sync.
        let checked = [
            "Money",
            "QuotaWindow",
            "TokenTotals",
            "ModelUsage",
            "CostEstimate",
            "ActivityStats",
            "SourceRef",
            "UsageSnapshot",
            "UsageReport",
            "ProviderInfo",
            "Settings",
        ]
        .into_iter()
        .map(String::from)
        .collect::<BTreeSet<_>>();

        let contract_names = key_sets().into_keys().collect::<BTreeSet<_>>();
        assert_eq!(
            checked, contract_names,
            "contract/usage-report-keys.json and schema_drift's checked-type list have diverged"
        );
    }
}
