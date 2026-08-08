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
