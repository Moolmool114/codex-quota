use crate::{
    app_server::AppServerClient,
    now_ms,
    quota::{should_replace, QuotaSnapshot},
    session::scan_sessions,
    storage::{save_cache, save_diagnostics, AppSettings},
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter};

const RECONNECT_SECONDS: [u64; 4] = [1, 3, 10, 30];

#[derive(Clone, Copy)]
pub enum RefreshReason {
    Startup,
    Manual,
    Watcher,
    Settings,
    Shutdown,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Diagnostics {
    pub app_server_status: String,
    pub app_server_error: Option<String>,
    pub app_server_last_read_ms: Option<i64>,
    pub app_server_last_notification_ms: Option<i64>,
    pub codex_executable: String,
    pub app_server_pid: Option<u32>,
    pub app_server_initialized: bool,
    pub bucket: Option<String>,
    pub watcher_status: String,
    pub session_path: String,
    pub source: String,
}

pub struct AppState {
    pub snapshot: Mutex<Option<QuotaSnapshot>>,
    pub settings: Mutex<AppSettings>,
    pub diagnostics: Mutex<Diagnostics>,
    pub refresh: Mutex<Option<Sender<RefreshReason>>>,
    stopped: Mutex<bool>,
    stopped_changed: Condvar,
}

impl AppState {
    pub fn new(settings: AppSettings, snapshot: Option<QuotaSnapshot>) -> Self {
        let source = snapshot
            .as_ref()
            .map(|value| value.source.clone())
            .unwrap_or_else(|| "none".into());
        Self {
            snapshot: Mutex::new(snapshot),
            settings: Mutex::new(settings),
            diagnostics: Mutex::new(Diagnostics {
                app_server_status: "starting".into(),
                codex_executable: "checking".into(),
                source,
                ..Default::default()
            }),
            refresh: Mutex::new(None),
            stopped: Mutex::new(false),
            stopped_changed: Condvar::new(),
        }
    }

    pub fn request_refresh(&self, reason: RefreshReason) {
        if let Some(sender) = self.refresh.lock().ok().and_then(|value| value.clone()) {
            let _ = sender.send(reason);
        }
    }

    fn mark_stopped(&self) {
        if let Ok(mut stopped) = self.stopped.lock() {
            *stopped = true;
            self.stopped_changed.notify_all();
        }
    }

    fn is_stopped(&self) -> bool {
        self.stopped.lock().map(|value| *value).unwrap_or(true)
    }
}

pub fn shutdown(state: &AppState) {
    state.request_refresh(RefreshReason::Shutdown);
    if let Ok(stopped) = state.stopped.lock() {
        if !*stopped {
            let _ = state
                .stopped_changed
                .wait_timeout(stopped, Duration::from_millis(1200));
        }
    }
}

fn session_path(settings: &AppSettings) -> PathBuf {
    settings
        .custom_session_path
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_default()
                .join(".codex")
                .join("sessions")
        })
}

fn apply_snapshot(app: &AppHandle, state: &Arc<AppState>, candidate: QuotaSnapshot) {
    let replaced = if let Ok(mut current) = state.snapshot.lock() {
        if should_replace(current.as_ref(), &candidate) {
            *current = Some(candidate.clone());
            true
        } else {
            false
        }
    } else {
        false
    };
    if replaced {
        let _ = save_cache(&candidate, now_ms());
        if let Ok(mut diagnostics) = state.diagnostics.lock() {
            diagnostics.source = candidate.source.clone();
        }
        let _ = app.emit("quota-updated", candidate);
    }
}

fn demote_disconnected_live(app: &AppHandle, state: &Arc<AppState>) {
    let demoted = state.snapshot.lock().ok().and_then(|mut current| {
        let value = current.as_mut()?;
        if value.source != "app-server" {
            return None;
        }
        value.source = "cache".into();
        Some(value.clone())
    });
    if let Some(snapshot) = demoted {
        if let Ok(mut diagnostics) = state.diagnostics.lock() {
            diagnostics.source = "cache".into();
        }
        let _ = app.emit("quota-updated", snapshot);
    }
}

fn refresh_session(app: &AppHandle, state: &Arc<AppState>) {
    let settings = state
        .settings
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let path = session_path(&settings);
    if let Some(snapshot) = scan_sessions(&path, now_ms()) {
        apply_snapshot(app, state, snapshot);
    }
    if let Ok(mut diagnostics) = state.diagnostics.lock() {
        diagnostics.session_path = path.to_string_lossy().into_owned();
    }
}

fn schedule_reconnect(next_reconnect: &mut Instant, backoff_index: &mut usize) {
    let delay = RECONNECT_SECONDS[(*backoff_index).min(RECONNECT_SECONDS.len() - 1)];
    *next_reconnect = Instant::now() + Duration::from_secs(delay);
    *backoff_index = (*backoff_index + 1).min(RECONNECT_SECONDS.len() - 1);
}

fn refresh_full(
    app: &AppHandle,
    state: &Arc<AppState>,
    client: &mut Option<AppServerClient>,
    next_reconnect: &mut Instant,
    backoff_index: &mut usize,
) {
    let current_time = now_ms();
    let mut live_success = false;
    if client.is_none() && Instant::now() >= *next_reconnect {
        if let Ok(mut diagnostics) = state.diagnostics.lock() {
            diagnostics.app_server_status = "starting".into();
        }
        match AppServerClient::connect(current_time) {
            Ok(value) => {
                if let Ok(mut diagnostics) = state.diagnostics.lock() {
                    diagnostics.app_server_status = "connected".into();
                    diagnostics.app_server_error = None;
                    diagnostics.codex_executable = value.executable().into();
                    diagnostics.app_server_pid = Some(value.pid());
                    diagnostics.app_server_initialized = true;
                }
                *client = Some(value);
                *backoff_index = 0;
            }
            Err(error) => {
                if let Ok(mut diagnostics) = state.diagnostics.lock() {
                    diagnostics.app_server_status = "disconnected".into();
                    diagnostics.codex_executable = if error.contains("not found") {
                        "not found"
                    } else {
                        "found, start failed"
                    }
                    .into();
                    diagnostics.app_server_error = Some(error);
                    diagnostics.app_server_pid = None;
                    diagnostics.app_server_initialized = false;
                }
                schedule_reconnect(next_reconnect, backoff_index);
            }
        }
    }

    let mut disconnected = false;
    if let Some(server) = client.as_mut() {
        match server.read_rate_limits(current_time) {
            Ok(snapshot) => {
                apply_snapshot(app, state, snapshot);
                live_success = true;
                if let Ok(mut diagnostics) = state.diagnostics.lock() {
                    diagnostics.app_server_status = "connected".into();
                    diagnostics.app_server_error = None;
                    diagnostics.app_server_last_read_ms = Some(current_time);
                    diagnostics.app_server_last_notification_ms = server.last_notification_ms;
                    diagnostics.bucket = server.last_bucket.clone();
                }
            }
            Err(error) => {
                if let Ok(mut diagnostics) = state.diagnostics.lock() {
                    diagnostics.app_server_status = if error
                        .to_ascii_lowercase()
                        .contains("authentication required")
                    {
                        "authentication-required"
                    } else {
                        "connected-error"
                    }
                    .into();
                    diagnostics.app_server_error = Some(error);
                    diagnostics.app_server_last_notification_ms = server.last_notification_ms;
                }
            }
        }
        disconnected = !server.is_running();
    }
    if disconnected {
        client.take();
        demote_disconnected_live(app, state);
        if let Ok(mut diagnostics) = state.diagnostics.lock() {
            diagnostics.app_server_status = "disconnected".into();
            diagnostics.app_server_pid = None;
            diagnostics.app_server_initialized = false;
        }
        *backoff_index = 0;
        schedule_reconnect(next_reconnect, backoff_index);
    }
    if !live_success {
        refresh_session(app, state);
    }
    if let Ok(diagnostics) = state.diagnostics.lock().map(|value| value.clone()) {
        let _ = save_diagnostics(&diagnostics);
        let _ = app.emit("refresh-finished", diagnostics);
    }
}

fn start_watcher(path: &Path, sender: Sender<RefreshReason>) -> Option<RecommendedWatcher> {
    let callback = move |event: Result<notify::Event, notify::Error>| {
        if event.is_ok() {
            let _ = sender.send(RefreshReason::Watcher);
        }
    };
    let mut watcher = notify::recommended_watcher(callback).ok()?;
    watcher.watch(path, RecursiveMode::Recursive).ok()?;
    Some(watcher)
}

pub fn start(app: AppHandle, state: Arc<AppState>) {
    let (sender, receiver): (Sender<RefreshReason>, Receiver<RefreshReason>) = mpsc::channel();
    if let Ok(mut slot) = state.refresh.lock() {
        *slot = Some(sender.clone());
    }
    thread::spawn(move || {
        let mut client: Option<AppServerClient> = None;
        let initial_settings = state
            .settings
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default();
        let mut watcher_path = session_path(&initial_settings);
        let mut watcher = start_watcher(&watcher_path, sender.clone());
        if let Ok(mut diagnostics) = state.diagnostics.lock() {
            diagnostics.watcher_status = if watcher.is_some() {
                "running"
            } else {
                "unavailable"
            }
            .into();
            diagnostics.session_path = watcher_path.to_string_lossy().into_owned();
        }
        let mut last_poll = Instant::now() - Duration::from_secs(600);
        let mut last_watch_refresh = Instant::now() - Duration::from_secs(5);
        let mut notification_due: Option<Instant> = None;
        let mut next_reconnect = Instant::now();
        let mut backoff_index = 0;
        let _ = sender.send(RefreshReason::Startup);

        loop {
            let settings = state
                .settings
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default();
            let desired_path = session_path(&settings);
            if desired_path != watcher_path {
                watcher = start_watcher(&desired_path, sender.clone());
                watcher_path = desired_path.clone();
                if let Ok(mut diagnostics) = state.diagnostics.lock() {
                    diagnostics.watcher_status = if watcher.is_some() {
                        "running"
                    } else {
                        "unavailable"
                    }
                    .into();
                    diagnostics.session_path = desired_path.to_string_lossy().into_owned();
                }
            }

            if client
                .as_mut()
                .is_some_and(|server| server.take_update_notification(now_ms()))
            {
                notification_due = Some(Instant::now() + Duration::from_millis(350));
            }
            if client.as_mut().is_some_and(|server| !server.is_running()) {
                client.take();
                demote_disconnected_live(&app, &state);
                if let Ok(mut diagnostics) = state.diagnostics.lock() {
                    diagnostics.app_server_status = "disconnected".into();
                    diagnostics.app_server_pid = None;
                    diagnostics.app_server_initialized = false;
                }
                backoff_index = 0;
                schedule_reconnect(&mut next_reconnect, &mut backoff_index);
            }

            let reason = receiver.recv_timeout(Duration::from_millis(100)).ok();
            if matches!(reason, Some(RefreshReason::Shutdown)) {
                break;
            }
            let notification_ready = notification_due.is_some_and(|due| Instant::now() >= due);
            let poll_due = last_poll.elapsed()
                >= Duration::from_secs(settings.refresh_interval_seconds.max(15));
            let reconnect_due = client.is_none() && Instant::now() >= next_reconnect;
            let full_refresh = matches!(
                reason,
                Some(RefreshReason::Startup | RefreshReason::Manual | RefreshReason::Settings)
            ) || notification_ready
                || poll_due
                || reconnect_due;
            if matches!(reason, Some(RefreshReason::Watcher))
                && last_watch_refresh.elapsed() >= Duration::from_millis(750)
            {
                refresh_session(&app, &state);
                last_watch_refresh = Instant::now();
            }
            if full_refresh {
                refresh_full(
                    &app,
                    &state,
                    &mut client,
                    &mut next_reconnect,
                    &mut backoff_index,
                );
                last_poll = Instant::now();
                notification_due = None;
            }
        }

        drop(watcher);
        drop(client);
        if let Ok(mut slot) = state.refresh.lock() {
            *slot = None;
        }
        state.mark_stopped();
    });
}

pub fn start_tray_tooltip(app: AppHandle, state: Arc<AppState>) {
    thread::spawn(move || {
        while !state.is_stopped() {
            let snapshot = state.snapshot.lock().ok().and_then(|value| value.clone());
            if let Some(tray) = app.tray_by_id("main-tray") {
                let _ = tray.set_tooltip(Some(format_tooltip(snapshot.as_ref(), now_ms())));
            }
            thread::sleep(Duration::from_secs(1));
        }
    });
}

fn countdown(reset: Option<i64>, now: i64) -> String {
    let seconds = reset.map(|value| (value - now).max(0) / 1000).unwrap_or(0);
    if seconds == 0 {
        return "waiting sync".into();
    }
    if seconds >= 86400 {
        return format!("{}d {}h", seconds / 86400, seconds % 86400 / 3600);
    }
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

pub fn format_tooltip(snapshot: Option<&QuotaSnapshot>, now: i64) -> String {
    let Some(snapshot) = snapshot else {
        return "Codex Quota · No data".into();
    };
    let Some(five) = snapshot.five_hour.as_ref() else {
        return "Codex Quota · Waiting for sync".into();
    };
    let percent = five
        .remaining_percent
        .map(|value| format!("{value:.0}%"))
        .unwrap_or_else(|| "N/A".into());
    let mut text = format!(
        "Codex · 5h {percent} · {}",
        countdown(five.resets_at_ms, now)
    );
    if let Some(weekly) = snapshot
        .long_window
        .as_ref()
        .and_then(|value| value.remaining_percent)
    {
        text.push_str(&format!(" · W {weekly:.0}%"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::QuotaWindow;
    fn snapshot(five: Option<f64>, weekly: Option<f64>, reset: Option<i64>) -> QuotaSnapshot {
        QuotaSnapshot {
            source: "session".into(),
            five_hour: five.map(|remaining| QuotaWindow {
                remaining_percent: Some(remaining),
                resets_at_ms: reset,
                ..Default::default()
            }),
            long_window: weekly.map(|remaining| QuotaWindow {
                remaining_percent: Some(remaining),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
    #[test]
    fn empty_tooltip_is_clear() {
        assert_eq!(format_tooltip(None, 0), "Codex Quota · No data");
    }
    #[test]
    fn missing_five_hour_waits() {
        assert_eq!(
            format_tooltip(Some(&snapshot(None, Some(80.0), None)), 0),
            "Codex Quota · Waiting for sync"
        );
    }
    #[test]
    fn tooltip_has_percent_and_time() {
        let value = format_tooltip(Some(&snapshot(Some(63.0), None, Some(3_661_000))), 1_000);
        assert!(value.contains("5h 63%"));
        assert!(value.contains("01:01:00"));
    }
    #[test]
    fn tooltip_includes_weekly() {
        assert!(
            format_tooltip(Some(&snapshot(Some(63.0), Some(42.0), Some(2_000))), 1_000)
                .contains("W 42%")
        );
    }
    #[test]
    fn day_countdown_is_compact() {
        assert_eq!(countdown(Some(90_000_000), 0), "1d 1h");
    }
    #[test]
    fn expired_countdown_waits() {
        assert_eq!(countdown(Some(100), 200), "waiting sync");
    }
    #[test]
    fn reconnect_backoff_is_bounded() {
        assert_eq!(RECONNECT_SECONDS, [1, 3, 10, 30]);
    }
}
