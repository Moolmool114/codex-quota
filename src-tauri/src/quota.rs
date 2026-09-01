use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct QuotaWindow {
    pub kind: String,
    pub used_percent: Option<f64>,
    pub remaining_percent: Option<f64>,
    pub resets_at_ms: Option<i64>,
    pub window_duration_mins: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct QuotaSnapshot {
    pub five_hour: Option<QuotaWindow>,
    pub long_window: Option<QuotaWindow>,
    pub source: String,
    pub observed_at_ms: Option<i64>,
    pub received_at_ms: Option<i64>,
    pub source_file: Option<String>,
}

fn number(value: &Value, names: &[&str]) -> Option<f64> {
    names
        .iter()
        .find_map(|name| value.get(name)?.as_f64())
        .filter(|v| v.is_finite())
}

pub fn normalize_window(value: &Value) -> QuotaWindow {
    let used = number(value, &["usedPercent", "used_percent"]);
    let remaining = number(value, &["remainingPercent", "remaining_percent"])
        .or_else(|| used.map(|v| (100.0 - v).clamp(0.0, 100.0)));
    let duration = number(
        value,
        &[
            "windowDurationMins",
            "window_duration_mins",
            "windowMinutes",
            "window_minutes",
        ],
    )
    .map(|v| v.max(0.0) as u64);
    let reset = number(value, &["resetsAt", "resets_at", "resetAt", "reset_at"]).and_then(|v| {
        (v > 0.0).then_some(if v < 1e12 {
            (v * 1000.0) as i64
        } else {
            v as i64
        })
    });
    QuotaWindow {
        used_percent: used,
        remaining_percent: remaining,
        resets_at_ms: reset,
        window_duration_mins: duration,
        ..Default::default()
    }
}

pub fn find_windows(value: &Value, output: &mut Vec<QuotaWindow>) {
    match value {
        Value::Object(map) => {
            for key in ["rate_limits", "rateLimits"] {
                if let Some(limits) = map.get(key).filter(|v| !v.is_null()) {
                    for bucket in ["primary", "secondary"] {
                        if let Some(window) = limits.get(bucket).filter(|v| v.is_object()) {
                            output.push(normalize_window(window));
                        }
                    }
                }
            }
            if map.contains_key("usedPercent") || map.contains_key("used_percent") {
                output.push(normalize_window(value));
            }
            for child in map.values() {
                find_windows(child, output);
            }
        }
        Value::Array(items) => {
            for child in items {
                find_windows(child, output);
            }
        }
        _ => {}
    }
}

pub fn classify_windows(
    windows: impl IntoIterator<Item = QuotaWindow>,
    source: &str,
    now_ms: i64,
) -> Option<QuotaSnapshot> {
    let mut snapshot = QuotaSnapshot {
        source: source.into(),
        observed_at_ms: Some(now_ms),
        received_at_ms: Some(now_ms),
        ..Default::default()
    };
    for mut window in windows {
        match window.window_duration_mins {
            Some(295..=305) => {
                window.kind = "five-hour".into();
                snapshot.five_hour = Some(window);
            }
            Some(10070..=10090) => {
                window.kind = "weekly".into();
                snapshot.long_window = Some(window);
            }
            Some(306..) => {
                window.kind = "long-window".into();
                if snapshot.long_window.is_none() {
                    snapshot.long_window = Some(window);
                }
            }
            _ => {}
        }
    }
    (snapshot.five_hour.is_some() || snapshot.long_window.is_some()).then_some(snapshot)
}

pub fn parse_app_server_response(response: &Value, now_ms: i64) -> Option<QuotaSnapshot> {
    let result = response.get("result")?;
    let bucket = result
        .get("rateLimitsByLimitId")
        .and_then(|all| all.get("codex"))
        .or_else(|| {
            result.get("rateLimits").filter(|v| {
                v.get("limitId")
                    .and_then(Value::as_str)
                    .is_none_or(|id| id == "codex")
            })
        })?;
    let mut windows = Vec::new();
    find_windows(bucket, &mut windows);
    classify_windows(windows, "app-server", now_ms)
}

pub fn app_server_bucket_source(response: &Value) -> Option<&'static str> {
    let result = response.get("result")?;
    if result
        .get("rateLimitsByLimitId")
        .and_then(|all| all.get("codex"))
        .is_some()
    {
        return Some("rateLimitsByLimitId.codex");
    }
    result
        .get("rateLimits")
        .filter(|value| {
            value
                .get("limitId")
                .and_then(Value::as_str)
                .is_none_or(|id| id == "codex")
        })
        .map(|_| "rateLimits fallback")
}

pub fn source_rank(source: &str) -> u8 {
    match source {
        "app-server" => 3,
        "session" => 2,
        "cache" => 1,
        _ => 0,
    }
}

pub fn should_replace(current: Option<&QuotaSnapshot>, candidate: &QuotaSnapshot) -> bool {
    current.is_none_or(|old| source_rank(&candidate.source) >= source_rank(&old.source))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn camel_case_normalizes() {
        let value = normalize_window(&json!({"usedPercent":32,"windowDurationMins":300}));
        assert_eq!(value.remaining_percent, Some(68.0));
        assert_eq!(value.window_duration_mins, Some(300));
    }
    #[test]
    fn snake_case_normalizes() {
        let value = normalize_window(&json!({"used_percent":20,"window_minutes":10080}));
        assert_eq!(value.remaining_percent, Some(80.0));
        assert_eq!(value.window_duration_mins, Some(10080));
    }
    #[test]
    fn explicit_remaining_wins() {
        assert_eq!(
            normalize_window(&json!({"usedPercent":90,"remainingPercent":44})).remaining_percent,
            Some(44.0)
        );
    }
    #[test]
    fn percentages_are_clamped_when_derived() {
        assert_eq!(
            normalize_window(&json!({"usedPercent":140})).remaining_percent,
            Some(0.0)
        );
        assert_eq!(
            normalize_window(&json!({"usedPercent":-20})).remaining_percent,
            Some(100.0)
        );
    }
    #[test]
    fn seconds_become_milliseconds() {
        assert_eq!(
            normalize_window(&json!({"resetsAt":1_788_000_000})).resets_at_ms,
            Some(1_788_000_000_000)
        );
    }
    #[test]
    fn milliseconds_are_preserved() {
        assert_eq!(
            normalize_window(&json!({"resets_at":1_788_000_000_000_i64})).resets_at_ms,
            Some(1_788_000_000_000)
        );
    }
    #[test]
    fn classify_standard_windows() {
        let result = classify_windows(
            vec![
                normalize_window(&json!({"usedPercent":10,"windowDurationMins":300})),
                normalize_window(&json!({"usedPercent":20,"windowDurationMins":10080})),
            ],
            "session",
            7,
        )
        .unwrap();
        assert_eq!(result.five_hour.unwrap().kind, "five-hour");
        assert_eq!(result.long_window.unwrap().kind, "weekly");
    }
    #[test]
    fn classify_generic_long_window() {
        let result = classify_windows(
            vec![normalize_window(
                &json!({"usedPercent":10,"windowDurationMins":1440}),
            )],
            "session",
            7,
        )
        .unwrap();
        assert_eq!(result.long_window.unwrap().kind, "long-window");
    }
    #[test]
    fn unknown_duration_is_ignored() {
        assert!(classify_windows(
            vec![normalize_window(
                &json!({"usedPercent":10,"windowDurationMins":60})
            )],
            "session",
            7
        )
        .is_none());
    }
    #[test]
    fn app_server_codex_bucket_parses() {
        let response = json!({"result":{"rateLimitsByLimitId":{"codex":{"primary":{"usedPercent":25,"windowDurationMins":300},"secondary":{"usedPercent":50,"windowDurationMins":10080}}}}});
        let result = parse_app_server_response(&response, 99).unwrap();
        assert_eq!(result.source, "app-server");
        assert_eq!(result.five_hour.unwrap().remaining_percent, Some(75.0));
    }
    #[test]
    fn codex_bucket_wins_over_other_buckets() {
        let response = json!({"result":{"rateLimitsByLimitId":{"other":{"primary":{"usedPercent":1,"windowDurationMins":300}},"codex":{"primary":{"usedPercent":92,"windowDurationMins":300}}}}});
        assert_eq!(
            parse_app_server_response(&response, 99)
                .unwrap()
                .five_hour
                .unwrap()
                .remaining_percent,
            Some(8.0)
        );
    }
    #[test]
    fn null_secondary_is_safe() {
        let response = json!({"result":{"rateLimits":{"primary":{"usedPercent":25,"windowDurationMins":300},"secondary":null}}});
        let value = parse_app_server_response(&response, 99).unwrap();
        assert!(value.five_hour.is_some());
        assert!(value.long_window.is_none());
    }
    #[test]
    fn missing_reset_is_preserved_as_unknown() {
        let response = json!({"result":{"rateLimits":{"primary":{"usedPercent":25,"windowDurationMins":300}}}});
        assert!(parse_app_server_response(&response, 99)
            .unwrap()
            .five_hour
            .unwrap()
            .resets_at_ms
            .is_none());
    }
    #[test]
    fn bucket_source_is_reported() {
        let codex = json!({"result":{"rateLimitsByLimitId":{"codex":{}}}});
        let fallback = json!({"result":{"rateLimits":{"limitId":"codex"}}});
        assert_eq!(
            app_server_bucket_source(&codex),
            Some("rateLimitsByLimitId.codex")
        );
        assert_eq!(
            app_server_bucket_source(&fallback),
            Some("rateLimits fallback")
        );
    }
    #[test]
    fn app_server_root_fallback_parses() {
        let response = json!({"result":{"rateLimits":{"limitId":"codex","primary":{"usedPercent":25,"windowDurationMins":300}}}});
        assert!(parse_app_server_response(&response, 99)
            .unwrap()
            .five_hour
            .is_some());
    }
    #[test]
    fn non_codex_fallback_is_rejected() {
        let response = json!({"result":{"rateLimits":{"limitId":"other","primary":{"usedPercent":25,"windowDurationMins":300}}}});
        assert!(parse_app_server_response(&response, 99).is_none());
    }
    #[test]
    fn source_ranking_is_explicit() {
        assert!(source_rank("app-server") > source_rank("session"));
        assert!(source_rank("session") > source_rank("cache"));
    }
    #[test]
    fn higher_priority_replaces() {
        let old = QuotaSnapshot {
            source: "cache".into(),
            ..Default::default()
        };
        let new = QuotaSnapshot {
            source: "session".into(),
            ..Default::default()
        };
        assert!(should_replace(Some(&old), &new));
    }
    #[test]
    fn newer_session_cannot_override_live() {
        let old = QuotaSnapshot {
            source: "app-server".into(),
            received_at_ms: Some(100),
            ..Default::default()
        };
        let new = QuotaSnapshot {
            source: "session".into(),
            observed_at_ms: Some(101),
            ..Default::default()
        };
        assert!(!should_replace(Some(&old), &new));
    }
    #[test]
    fn older_cache_cannot_replace_live() {
        let old = QuotaSnapshot {
            source: "app-server".into(),
            received_at_ms: Some(100),
            ..Default::default()
        };
        let new = QuotaSnapshot {
            source: "cache".into(),
            observed_at_ms: Some(99),
            ..Default::default()
        };
        assert!(!should_replace(Some(&old), &new));
    }
}
