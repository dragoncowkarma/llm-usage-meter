//! The IPC surface. The webview can do exactly these four things.
//!
//! Deliberately narrow: the frontend has no filesystem capability at all, so
//! every path this app reads is decided in Rust and auditable in one place.

use tauri::{AppHandle, Manager, State};

use crate::collector::{AppState, ProviderInfo, Settings};
use crate::model::UsageReport;

/// Every registered provider, enabled or not — the settings list needs the
/// disabled ones so they can be switched back on.
#[tauri::command]
pub fn list_providers(state: State<'_, AppState>) -> Vec<ProviderInfo> {
    state.provider_list()
}

/// Collection walks hundreds of transcript files, so it must not run on the
/// async runtime's thread (which would stall every other command) or on the
/// main thread (which would freeze the popup). `spawn_blocking` puts it where
/// blocking I/O belongs; the cached path stays synchronous and instant.
#[tauri::command]
pub async fn get_report(app: AppHandle, force: bool) -> Result<UsageReport, String> {
    if !force {
        if let Some(cached) = app.state::<AppState>().cached() {
            return Ok(cached);
        }
    }
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || handle.state::<AppState>().refresh())
        .await
        .map_err(|e| format!("collection task failed: {e}"))
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings()
}

#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, settings: Settings) -> Settings {
    state.update_settings(settings)
}

/// Reveal one of the source files a snapshot cites, so the user can check our
/// arithmetic against the raw data.
#[tauri::command]
pub fn reveal_source(app: AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn hide_popup(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}
