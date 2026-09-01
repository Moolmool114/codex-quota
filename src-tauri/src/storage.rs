use crate::quota::QuotaSnapshot;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct AppSettings {
    pub window_opacity: f64,
    pub background_opacity: f64,
    pub text_opacity: f64,
    pub animations_enabled: bool,
    pub language: String,
    pub always_on_top: bool,
    pub theme: String,
    pub start_with_windows: bool,
    pub start_minimized: bool,
    pub close_to_tray: bool,
    pub refresh_interval_seconds: u64,
    pub custom_session_path: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            window_opacity: 1.0,
            background_opacity: 1.0,
            text_opacity: 1.0,
            animations_enabled: true,
            language: "en".into(),
            always_on_top: false,
            theme: "system".into(),
            start_with_windows: false,
            start_minimized: false,
            close_to_tray: true,
            refresh_interval_seconds: 30,
            custom_session_path: None,
        }
    }
}

impl AppSettings {
    pub fn sanitize(mut self) -> Self {
        self.window_opacity = self.window_opacity.clamp(0.5, 1.0);
        if self.background_opacity == 1.0 && self.text_opacity == 1.0 && self.window_opacity < 1.0 {
            self.background_opacity = self.window_opacity;
            self.text_opacity = self.window_opacity;
        }
        self.background_opacity = self.background_opacity.clamp(0.1, 1.0);
        self.text_opacity = self.text_opacity.clamp(0.2, 1.0);
        self.window_opacity = 1.0;
        self.refresh_interval_seconds = self.refresh_interval_seconds.clamp(15, 600);
        self.custom_session_path = self
            .custom_session_path
            .and_then(|v| (!v.trim().is_empty()).then(|| v.trim().to_owned()));
        if !matches!(self.theme.as_str(), "system" | "dark" | "light" | "glass") {
            self.theme = "system".into();
        }
        if !matches!(self.language.as_str(), "en" | "zh-CN") {
            self.language = "en".into();
        }
        self
    }
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    schema_version: u32,
    snapshot: QuotaSnapshot,
    cached_at: i64,
}

pub fn app_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Codex Quota")
}
pub fn settings_path() -> PathBuf {
    app_data_dir().join("settings.json")
}
pub fn cache_path() -> PathBuf {
    app_data_dir().join("quota-cache.json")
}

fn atomic_write(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let temp = path.with_extension("tmp");
    let mut file = fs::File::create(&temp).map_err(|e| e.to_string())?;
    file.write_all(data).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    if !path.exists() {
        return fs::rename(temp, path).map_err(|e| e.to_string());
    }
    let backup = path.with_extension("bak");
    if backup.exists() {
        fs::remove_file(&backup).map_err(|e| e.to_string())?;
    }
    fs::rename(path, &backup).map_err(|e| e.to_string())?;
    match fs::rename(&temp, path) {
        Ok(()) => {
            let _ = fs::remove_file(backup);
            Ok(())
        }
        Err(error) => {
            let _ = fs::rename(backup, path);
            Err(error.to_string())
        }
    }
}

pub fn load_settings() -> AppSettings {
    fs::read(settings_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<AppSettings>(&bytes).ok())
        .unwrap_or_default()
        .sanitize()
}
pub fn save_settings(settings: &AppSettings) -> Result<(), String> {
    atomic_write(
        &settings_path(),
        &serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?,
    )
}
pub fn load_cache() -> Option<QuotaSnapshot> {
    let mut cache = serde_json::from_slice::<CacheFile>(&fs::read(cache_path()).ok()?).ok()?;
    if cache.schema_version != 1 {
        return None;
    }
    cache.snapshot.source = "cache".into();
    Some(cache.snapshot)
}
pub fn save_cache(snapshot: &QuotaSnapshot, now_ms: i64) -> Result<(), String> {
    let cache = CacheFile {
        schema_version: 1,
        snapshot: snapshot.clone(),
        cached_at: now_ms,
    };
    atomic_write(
        &cache_path(),
        &serde_json::to_vec_pretty(&cache).map_err(|e| e.to_string())?,
    )
}

pub fn save_diagnostics<T: Serialize>(diagnostics: &T) -> Result<(), String> {
    atomic_write(
        &app_data_dir().join("diagnostics.json"),
        &serde_json::to_vec_pretty(diagnostics).map_err(|error| error.to_string())?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_safe() {
        let value = AppSettings::default();
        assert_eq!(value.window_opacity, 1.0);
        assert_eq!(value.refresh_interval_seconds, 30);
        assert!(value.close_to_tray);
    }
    #[test]
    fn sanitize_clamps_limits() {
        let value = AppSettings {
            window_opacity: 0.1,
            refresh_interval_seconds: 2,
            ..Default::default()
        }
        .sanitize();
        assert_eq!(value.window_opacity, 1.0);
        assert_eq!(value.background_opacity, 0.5);
        assert_eq!(value.text_opacity, 0.5);
        assert_eq!(value.refresh_interval_seconds, 15);
    }
    #[test]
    fn sanitize_clamps_upper_limits() {
        let value = AppSettings {
            window_opacity: 2.0,
            refresh_interval_seconds: 9999,
            ..Default::default()
        }
        .sanitize();
        assert_eq!(value.window_opacity, 1.0);
        assert_eq!(value.refresh_interval_seconds, 600);
    }
    #[test]
    fn separate_opacity_limits_are_safe() {
        let value = AppSettings {
            background_opacity: 0.01,
            text_opacity: 0.01,
            ..Default::default()
        }
        .sanitize();
        assert_eq!(value.background_opacity, 0.1);
        assert_eq!(value.text_opacity, 0.2);
    }
    #[test]
    fn sanitize_theme_and_path() {
        let value = AppSettings {
            theme: "neon".into(),
            custom_session_path: Some("  C:\\sessions  ".into()),
            ..Default::default()
        }
        .sanitize();
        assert_eq!(value.theme, "system");
        assert_eq!(value.custom_session_path.as_deref(), Some("C:\\sessions"));
    }
    #[test]
    fn glass_theme_is_supported() {
        let value = AppSettings {
            theme: "glass".into(),
            ..Default::default()
        }
        .sanitize();
        assert_eq!(value.theme, "glass");
    }
    #[test]
    fn blank_path_becomes_none() {
        assert!(AppSettings {
            custom_session_path: Some("   ".into()),
            ..Default::default()
        }
        .sanitize()
        .custom_session_path
        .is_none());
    }
    #[test]
    fn partial_json_uses_defaults() {
        let value: AppSettings = serde_json::from_str(r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(value.theme, "dark");
        assert_eq!(value.refresh_interval_seconds, 30);
    }
}
