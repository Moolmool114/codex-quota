use crate::quota::{app_server_bucket_source, parse_app_server_response, QuotaSnapshot};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

const CREATE_NO_WINDOW: u32 = 0x08000000;
const RATE_LIMITS_UPDATED: &str = "account/rateLimits/updated";

pub(crate) fn response_matches_id(message: &Value, id: i64) -> bool {
    message.get("id").and_then(Value::as_i64) == Some(id)
}

pub(crate) fn is_rate_limits_notification(message: &Value) -> bool {
    message.get("method").and_then(Value::as_str) == Some(RATE_LIMITS_UPDATED)
}

pub struct AppServerClient {
    child: Child,
    executable: String,
    stdin: Option<ChildStdin>,
    messages: Receiver<Value>,
    next_id: i64,
    pub last_notification_ms: Option<i64>,
    pub last_bucket: Option<String>,
}

impl AppServerClient {
    pub fn connect(now_ms: i64) -> Result<Self, String> {
        let mut failures = Vec::new();
        for executable in codex_candidates() {
            let label = executable.to_string_lossy().into_owned();
            match Self::connect_candidate(now_ms, &executable, label.clone()) {
                Ok(client) => return Ok(client),
                Err(error) => failures.push(format!("{label}: {error}")),
            }
        }
        Err(format!(
            "Codex executable not found or could not initialize. Tried: {}",
            failures.join("; ")
        ))
    }

    fn connect_candidate(
        now_ms: i64,
        executable_path: &Path,
        executable: String,
    ) -> Result<Self, String> {
        let mut child = spawn_app_server(executable_path)?;
        let stdin = child.stdin.take().ok_or("app-server stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("app-server stdout unavailable")?;
        if let Some(stderr) = child.stderr.take() {
            thread::spawn(
                move || for _line in BufReader::new(stderr).lines().map_while(Result::ok) {},
            );
        }
        let (tx, messages) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(message) = serde_json::from_str(&line) {
                    if tx.send(message).is_err() {
                        break;
                    }
                }
            }
        });
        let mut client = Self {
            child,
            executable,
            stdin: Some(stdin),
            messages,
            next_id: 1,
            last_notification_ms: None,
            last_bucket: None,
        };
        client.request(
            "initialize",
            json!({"clientInfo":{"name":"codex_quota","title":"Codex Quota","version":"0.6.1"}}),
            Duration::from_secs(5),
            now_ms,
        )?;
        client.notify("initialized", None)?;
        Ok(client)
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn executable(&self) -> &str {
        &self.executable
    }

    fn write(&mut self, message: &Value) -> Result<(), String> {
        let stdin = self.stdin.as_mut().ok_or("app-server stdin closed")?;
        writeln!(stdin, "{message}").map_err(|error| error.to_string())?;
        stdin.flush().map_err(|error| error.to_string())
    }

    fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), String> {
        let mut message = json!({"method":method});
        if let Some(params) = params {
            message["params"] = params;
        }
        self.write(&message)
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
        now_ms: i64,
    ) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({"id":id,"method":method,"params":params}))?;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let message = self
                .messages
                .recv_timeout(remaining)
                .map_err(|error| format!("{method} failed while waiting for response: {error}"))?;
            if is_rate_limits_notification(&message) {
                self.last_notification_ms = Some(now_ms);
                continue;
            }
            if !response_matches_id(&message, id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("app-server request failed")
                    .to_owned());
            }
            return Ok(message);
        }
    }

    pub fn read_rate_limits(&mut self, now_ms: i64) -> Result<QuotaSnapshot, String> {
        let response = self.request(
            "account/rateLimits/read",
            json!({}),
            Duration::from_secs(8),
            now_ms,
        )?;
        self.last_bucket = app_server_bucket_source(&response).map(str::to_owned);
        parse_app_server_response(&response, now_ms)
            .ok_or_else(|| "app-server returned no Codex quota bucket".into())
    }

    pub fn take_update_notification(&mut self, now_ms: i64) -> bool {
        let mut found = false;
        while let Ok(message) = self.messages.try_recv() {
            if is_rate_limits_notification(&message) {
                found = true;
                self.last_notification_ms = Some(now_ms);
            }
        }
        found
    }

    pub fn is_running(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_none()
    }
}

fn official_install_candidates(local_app_data: &Path) -> Vec<PathBuf> {
    let root = local_app_data.join("OpenAI").join("Codex").join("bin");
    let mut candidates = Vec::new();
    let direct = root.join("codex.exe");
    if direct.is_file() {
        candidates.push(direct);
    }
    let mut versioned = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let executable = entry.path().join("codex.exe");
            executable.is_file().then(|| {
                let modified = executable
                    .metadata()
                    .and_then(|value| value.modified())
                    .ok();
                (modified, executable)
            })
        })
        .collect::<Vec<_>>();
    versioned.sort_by_key(|(modified, _)| *modified);
    candidates.extend(versioned.into_iter().rev().map(|(_, path)| path));
    candidates
}

fn codex_candidates() -> Vec<PathBuf> {
    let mut candidates = vec![PathBuf::from("codex")];
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        candidates.extend(official_install_candidates(Path::new(&local_app_data)));
    }
    if let Some(user_profile) = std::env::var_os("USERPROFILE") {
        let standalone = PathBuf::from(user_profile)
            .join(".codex")
            .join("bin")
            .join("codex.exe");
        if standalone.is_file() {
            candidates.push(standalone);
        }
    }
    candidates.dedup();
    candidates
}

fn spawn_app_server(executable: &Path) -> Result<Child, String> {
    let mut command = Command::new(executable);
    command
        .args(["app-server", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.spawn().map_err(|error| error.to_string())
}

impl Drop for AppServerClient {
    fn drop(&mut self) {
        self.stdin.take();
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    #[test]
    fn response_id_must_match() {
        assert!(response_matches_id(&json!({"id":7,"result":{}}), 7));
        assert!(!response_matches_id(&json!({"id":8}), 7));
    }
    #[test]
    fn notification_is_recognized() {
        assert!(is_rate_limits_notification(
            &json!({"method":"account/rateLimits/updated","params":{}})
        ));
    }
    #[test]
    fn unrelated_notification_is_ignored() {
        assert!(!is_rate_limits_notification(
            &json!({"method":"account/updated"})
        ));
    }
    #[test]
    fn official_install_directory_is_discovered_without_path() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "codex-quota-discovery-{}-{nonce}",
            std::process::id()
        ));
        let executable = root
            .join("OpenAI")
            .join("Codex")
            .join("bin")
            .join("build-id")
            .join("codex.exe");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, []).unwrap();
        assert_eq!(official_install_candidates(&root), vec![executable]);
        std::fs::remove_dir_all(root).unwrap();
    }
}
