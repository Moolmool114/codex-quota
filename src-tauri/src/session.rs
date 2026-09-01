use crate::quota::{classify_windows, find_windows, QuotaSnapshot};
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

fn collect_rollouts(dir: &Path, output: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rollouts(&path, output);
            } else if path
                .file_name()
                .and_then(|v| v.to_str())
                .is_some_and(|v| v.starts_with("rollout-") && v.ends_with(".jsonl"))
            {
                output.push(path);
            }
        }
    }
}

fn read_tail(path: &Path, max_bytes: u64) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut text = String::new();
    file.read_to_string(&mut text).ok()?;
    if start > 0 {
        text = text
            .split_once('\n')
            .map(|(_, rest)| rest.to_owned())
            .unwrap_or_default();
    }
    Some(text)
}

pub fn scan_sessions(dir: &Path, now_ms: i64) -> Option<QuotaSnapshot> {
    let mut files = Vec::new();
    collect_rollouts(dir, &mut files);
    files.sort_by_key(|path| fs::metadata(path).and_then(|m| m.modified()).ok());
    for path in files.iter().rev().take(20) {
        let Some(text) = read_tail(path, 2 * 1024 * 1024) else {
            continue;
        };
        let mut windows = Vec::new();
        for line in text.lines().rev() {
            if let Ok(value) = serde_json::from_str(line) {
                find_windows(&value, &mut windows);
            }
        }
        if let Some(mut snapshot) = classify_windows(windows, "session", now_ms) {
            let mtime = fs::metadata(path)
                .ok()
                .and_then(|value| value.modified().ok())
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_millis() as i64)
                .unwrap_or(now_ms);
            snapshot.observed_at_ms = Some(mtime);
            snapshot.source_file = Some(path.to_string_lossy().into_owned());
            return Some(snapshot);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("codex-quota-{name}-{}-{nonce}", std::process::id()))
    }
    #[test]
    fn missing_directory_is_safe() {
        assert!(scan_sessions(&test_dir("missing"), 7).is_none());
    }
    #[test]
    fn valid_rollout_is_found_recursively() {
        let root = test_dir("valid");
        let nested = root.join("2026").join("09");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("rollout-test.jsonl"),"not-json\n{\"rate_limits\":{\"primary\":{\"used_percent\":20,\"window_minutes\":300}}}\n").unwrap();
        let result = scan_sessions(&root, 7).unwrap();
        assert_eq!(result.source, "session");
        assert_eq!(result.five_hour.unwrap().remaining_percent, Some(80.0));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unrelated_files_are_ignored() {
        let root = test_dir("ignored");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("notes.jsonl"),
            "{\"used_percent\":20,\"window_minutes\":300}",
        )
        .unwrap();
        assert!(scan_sessions(&root, 7).is_none());
        fs::remove_dir_all(root).unwrap();
    }
}
