mod app_server;
mod quota;
mod service;
mod session;
mod storage;

use quota::QuotaSnapshot;
use service::{AppState, Diagnostics, RefreshReason};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use storage::AppSettings;
use tauri::{Emitter, Manager, State};

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[tauri::command]
fn get_quota(state: State<'_, Arc<AppState>>) -> QuotaSnapshot {
    state
        .snapshot
        .lock()
        .ok()
        .and_then(|value| value.clone())
        .unwrap_or_else(|| QuotaSnapshot {
            source: "none".into(),
            received_at_ms: Some(now_ms()),
            ..Default::default()
        })
}

#[tauri::command]
fn refresh_quota(state: State<'_, Arc<AppState>>) -> QuotaSnapshot {
    state.request_refresh(RefreshReason::Manual);
    get_quota(state)
}

#[tauri::command]
fn get_settings(state: State<'_, Arc<AppState>>) -> AppSettings {
    state
        .settings
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default()
}

#[tauri::command]
fn save_settings(
    settings: AppSettings,
    state: State<'_, Arc<AppState>>,
) -> Result<AppSettings, String> {
    let settings = settings.sanitize();
    storage::save_settings(&settings)?;
    *state
        .settings
        .lock()
        .map_err(|_| "settings lock poisoned")? = settings.clone();
    state.request_refresh(RefreshReason::Settings);
    Ok(settings)
}

#[tauri::command]
fn get_diagnostics(state: State<'_, Arc<AppState>>) -> Diagnostics {
    state
        .diagnostics
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default()
}

#[tauri::command]
fn set_window_opacity(window: tauri::WebviewWindow, opacity: f64) -> Result<(), String> {
    let value = opacity.clamp(0.5, 1.0);
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetWindowLongPtrW, SetLayeredWindowAttributes, SetWindowLongPtrW, GWL_EXSTYLE,
            LWA_ALPHA, WS_EX_LAYERED,
        };
        let hwnd = window.hwnd().map_err(|error| error.to_string())?.0;
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            if value >= 0.999 {
                if style & WS_EX_LAYERED as isize != 0 {
                    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style & !(WS_EX_LAYERED as isize));
                }
            } else {
                if style & WS_EX_LAYERED as isize == 0 {
                    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_LAYERED as isize);
                }
                if SetLayeredWindowAttributes(hwnd, 0, (value * 255.0).round() as u8, LWA_ALPHA)
                    == 0
                {
                    return Err("SetLayeredWindowAttributes failed".into());
                }
            }
        }
    }
    #[cfg(not(windows))]
    let _ = value;
    Ok(())
}

#[tauri::command]
fn quit_app(app: tauri::AppHandle, state: State<'_, Arc<AppState>>) {
    service::shutdown(&state);
    app.exit(0);
}

pub fn run() {
    use tauri::{
        menu::{Menu, MenuItem},
        tray::TrayIconBuilder,
    };

    let state = Arc::new(AppState::new(
        storage::load_settings(),
        storage::load_cache(),
    ));
    tauri::Builder::default()
        .manage(state)
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .setup(|app| {
            let open = MenuItem::with_id(app, "open", "Open", true, None::<&str>)?;
            let refresh = MenuItem::with_id(app, "refresh", "Refresh", true, None::<&str>)?;
            let settings_item = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &refresh, &settings_item, &quit])?;
            TrayIconBuilder::with_id("main-tray")
                .menu(&menu)
                .tooltip("Codex Quota · Starting")
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "refresh" => {
                        if let Some(state) = app.try_state::<Arc<AppState>>() {
                            state.request_refresh(RefreshReason::Manual);
                        }
                    }
                    "settings" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                        let _ = app.emit("settings-request", ());
                    }
                    "quit" => {
                        if let Some(state) = app.try_state::<Arc<AppState>>() {
                            service::shutdown(&state);
                        }
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;

            let state = app.state::<Arc<AppState>>().inner().clone();
            if state
                .settings
                .lock()
                .map(|value| value.start_minimized)
                .unwrap_or(false)
            {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }
            service::start(app.handle().clone(), state.clone());
            service::start_tray_tooltip(app.handle().clone(), state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_quota,
            refresh_quota,
            get_settings,
            save_settings,
            get_diagnostics,
            set_window_opacity,
            quit_app
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let close_to_tray = window
                    .app_handle()
                    .state::<Arc<AppState>>()
                    .settings
                    .lock()
                    .map(|value| value.close_to_tray)
                    .unwrap_or(true);
                api.prevent_close();
                if close_to_tray {
                    let _ = window.hide();
                } else {
                    service::shutdown(&window.app_handle().state::<Arc<AppState>>());
                    window.app_handle().exit(0);
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Codex Quota");
}
