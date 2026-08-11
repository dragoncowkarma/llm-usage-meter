//! Menu bar / system tray integration.
//!
//! macOS specifics (activation policy, template icon, title text) are fenced
//! behind `cfg` so the same file drives the Windows tray in Phase 2.

use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, PhysicalPosition, WebviewWindow,
};

use crate::collector::AppState;

pub const TRAY_ID: &str = "llm-usage-meter-tray";
/// Gap between the menu bar and the top of the popup.
const POPUP_GAP: f64 = 8.0;
/// Keep the popup this far from the screen edge when the icon is in a corner.
const SCREEN_MARGIN: f64 = 8.0;

pub fn build(app: &AppHandle) -> tauri::Result<TrayIcon> {
    let refresh = MenuItem::with_id(app, "refresh", "Refresh Now", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit LLM Usage Meter", true, Some("CmdOrCtrl+Q"))?;
    let menu = Menu::with_items(app, &[&refresh, &separator, &quit])?;

    // `mut` is only used under the macOS cfg below.
    #[allow(unused_mut)]
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .icon(tray_icon())
        .tooltip("LLM Usage Meter")
        .menu(&menu)
        // Left click toggles the popup; the menu is right-click only.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "refresh" => {
                let handle = app.clone();
                // Collection does blocking file I/O — never on the UI thread.
                std::thread::spawn(move || {
                    refresh_and_publish(&handle);
                });
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(window) = app.get_webview_window("main") {
                    toggle_popup(&window, Some(rect));
                }
            }
        });

    // A template image is tinted by macOS to match the menu bar and to invert
    // correctly when the icon is selected; a coloured icon cannot do either.
    #[cfg(target_os = "macos")]
    {
        builder = builder.icon_as_template(true);
    }

    builder.build(app)
}

fn tray_icon() -> Image<'static> {
    // Fails only if the bundled PNG is corrupt, which is a build-time problem.
    Image::from_bytes(include_bytes!("../icons/tray-template.png"))
        .expect("bundled tray icon must decode")
}

/// Show or hide the popup, anchoring it under the tray icon when we know where
/// that is.
pub fn toggle_popup(window: &WebviewWindow, anchor: Option<tauri::Rect>) {
    let visible = window.is_visible().unwrap_or(false);
    if visible {
        let _ = window.hide();
        return;
    }

    if let Some(rect) = anchor {
        position_under(window, rect);
    }
    let _ = window.show();
    let _ = window.set_focus();
}

fn position_under(window: &WebviewWindow, rect: tauri::Rect) {
    let scale = window.scale_factor().unwrap_or(1.0);
    let icon_pos = rect.position.to_physical::<f64>(scale);
    let icon_size = rect.size.to_physical::<f64>(scale);
    let Ok(win_size) = window.outer_size() else {
        return;
    };

    let win_w = win_size.width as f64;
    let mut x = icon_pos.x + icon_size.width / 2.0 - win_w / 2.0;
    let y = icon_pos.y + icon_size.height + POPUP_GAP;

    // Keep the popup on screen when the icon sits near a corner — without this
    // a right-most menu bar icon opens a half-offscreen panel.
    if let Ok(Some(monitor)) = window.current_monitor() {
        let m_pos = monitor.position();
        let m_size = monitor.size();
        let min_x = m_pos.x as f64 + SCREEN_MARGIN;
        let max_x = m_pos.x as f64 + m_size.width as f64 - win_w - SCREEN_MARGIN;
        if max_x >= min_x {
            x = x.clamp(min_x, max_x);
        }
    }

    let _ = window.set_position(PhysicalPosition::new(x, y));
}

/// Refresh in the background and push the result to the UI and the menu bar.
pub fn refresh_and_publish(app: &AppHandle) {
    let state = app.state::<AppState>();
    let report = state.refresh();
    apply_title(app, report.peak_percent, state.settings().show_percent_in_menu_bar);
    use tauri::Emitter;
    let _ = app.emit("usage-updated", &report);
}

/// Update the tray tooltip with the current peak quota percentage.
/// The tray title is intentionally left empty so only the icon is displayed.
pub fn apply_title(app: &AppHandle, peak: Option<f64>, _enabled: bool) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    // Always clear any title text — icon-only display.
    let _ = tray.set_title(None);
    let tooltip = match peak {
        Some(p) => format!("LLM Usage Meter — peak quota {}%", p.round() as i64),
        None => "LLM Usage Meter".to_string(),
    };
    let _ = tray.set_tooltip(Some(&tooltip));
}
