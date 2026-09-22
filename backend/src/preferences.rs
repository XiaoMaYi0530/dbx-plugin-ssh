//! Plugin-level UI preferences persisted in `<plugin_data_dir>/preferences.json`.
//! The workbench iframe is sandboxed (`sandbox="allow-scripts"`, opaque origin),
//! so `window.localStorage` throws and this sidecar file is the only durable
//! store for workbench preferences such as the download directory. The schema
//! is a fixed allowlist — arbitrary keys from the renderer are dropped.

use std::path::Path;

use serde_json::{json, Map, Value};

const STORAGE_VERSION: u64 = 1;
const FILE_NAME: &str = "preferences.json";
/// Bounded so a broken renderer cannot grow the file without limit.
const MAX_DOWNLOAD_DIR_LEN: usize = 512;

const MAX_TIMESTAMP_FORMAT_LEN: usize = 64;

pub fn store_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join(FILE_NAME)
}

fn sanitize_download_dir(value: &Value) -> Option<String> {
    let dir = value.as_str()?.trim();
    Some(dir.chars().take(MAX_DOWNLOAD_DIR_LEN).collect())
}

/// 本地终端 shell 偏好：程序路径或可解析名，空串=跟随自动探测。
fn sanitize_local_shell(value: &Value) -> Option<String> {
    let shell = value.as_str()?.trim();
    Some(shell.chars().take(200).collect())
}

/// 冲突策略白名单：自动重命名（默认）/ 询问我 / 覆盖已有文件。
fn sanitize_conflict_policy(value: &Value) -> Option<&'static str> {
    match value.as_str()? {
        "rename" => Some("rename"),
        "ask" => Some("ask"),
        "overwrite" => Some("overwrite"),
        _ => None,
    }
}

/// 数值偏好钳制：非负整数夹进 [min, max]，超界取边界、非法取 fallback。
fn sanitize_u64_clamped(value: &Value, min: u64, max: u64, fallback: u64) -> u64 {
    let raw = match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    };
    match raw {
        Some(value) => value.clamp(min, max),
        None => fallback,
    }
}

/// 动作链接三类匹配器开关（ipv4/host_port/archive）。逐字段收紧、缺省 true；
/// 整键缺失或形状非法由调用方跳过（前端按缺省处理）。
fn sanitize_action_links_matchers(value: &Value) -> Option<Value> {
    let object = value.as_object()?;
    let flag = |key: &str| object.get(key).and_then(Value::as_bool).unwrap_or(true);
    Some(json!({
        "ipv4": flag("ipv4"),
        "host_port": flag("host_port"),
        "archive": flag("archive"),
    }))
}

/// 终端时间戳格式：白名单字符（字母数字与 []:-./, 空格）+ 64 字符截断；
/// 清洗后为空回退默认 "[HH:mm:ss]"，与前端 sanitizeGutterSettings 同向。
fn sanitize_timestamp_format(value: &Value) -> Option<String> {
    let raw = value.as_str()?;
    let cleaned: String = raw
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '[' | ']' | ':' | '-' | '.' | '/' | ',' | ' ')
        })
        .take(MAX_TIMESTAMP_FORMAT_LEN)
        .collect();
    let trimmed = cleaned.trim();
    Some(if trimmed.is_empty() {
        "[HH:mm:ss]".to_string()
    } else {
        cleaned
    })
}

/// Reads the raw preferences map; a missing or corrupted file yields an empty
/// map so a bad file can never break the workbench (same policy as
/// quick-commands).
fn load_map(data_dir: &Path) -> Map<String, Value> {
    let text = std::fs::read_to_string(store_path(data_dir)).unwrap_or_default();
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|value| value.get("prefs")?.as_object().cloned())
        .unwrap_or_default()
}

/// Public view: only allowlisted keys, normalized.
pub fn load_preferences(data_dir: &Path) -> Value {
    let map = load_map(data_dir);
    let mut prefs = Map::new();
    if let Some(dir) = map.get("downloadDir").and_then(sanitize_download_dir) {
        prefs.insert("downloadDir".to_string(), Value::String(dir));
    }
    if let Some(use_default) = map.get("downloadUseDefaultDir").and_then(Value::as_bool) {
        prefs.insert(
            "downloadUseDefaultDir".to_string(),
            Value::Bool(use_default),
        );
    }
    if let Some(policy) = map
        .get("downloadConflictPolicy")
        .and_then(sanitize_conflict_policy)
    {
        prefs.insert(
            "downloadConflictPolicy".to_string(),
            Value::String(policy.to_string()),
        );
    }
    if let Some(shell) = map.get("localShell").and_then(sanitize_local_shell) {
        prefs.insert("localShell".to_string(), Value::String(shell));
    }
    if let Some(integration) = map.get("localShellIntegration").and_then(Value::as_bool) {
        prefs.insert(
            "localShellIntegration".to_string(),
            Value::Bool(integration),
        );
    }
    // 上传并发（1..=10，默认 3）与重复目标策略（P1-5）。
    if map.contains_key("transfer_concurrency") {
        let concurrency = sanitize_u64_clamped(&map["transfer_concurrency"], 1, 10, 3);
        prefs.insert("transfer_concurrency".to_string(), Value::from(concurrency));
    }
    if let Some(policy) = map
        .get("transfer_duplicate_policy")
        .and_then(sanitize_conflict_policy)
    {
        prefs.insert(
            "transfer_duplicate_policy".to_string(),
            Value::String(policy.to_string()),
        );
    }
    // 命令输入建议（P1-1）：开关（默认开）与查询长度上下限。
    if let Some(enabled) = map
        .get("history_suggestions_enabled")
        .and_then(Value::as_bool)
    {
        prefs.insert(
            "history_suggestions_enabled".to_string(),
            Value::Bool(enabled),
        );
    }
    if map.contains_key("history_suggestion_min_chars") {
        let min_chars = sanitize_u64_clamped(&map["history_suggestion_min_chars"], 1, 16, 2);
        prefs.insert(
            "history_suggestion_min_chars".to_string(),
            Value::from(min_chars),
        );
    }
    if map.contains_key("history_suggestion_max_chars") {
        let max_chars = sanitize_u64_clamped(&map["history_suggestion_max_chars"], 8, 512, 64);
        prefs.insert(
            "history_suggestion_max_chars".to_string(),
            Value::from(max_chars),
        );
    }
    if let Some(enabled) = map.get("action_links_enabled").and_then(Value::as_bool) {
        prefs.insert("action_links_enabled".to_string(), Value::Bool(enabled));
    }
    if let Some(matchers) = map
        .get("action_links_matchers")
        .and_then(sanitize_action_links_matchers)
    {
        prefs.insert("action_links_matchers".to_string(), matchers);
    }
    if let Some(line_numbers) = map
        .get("terminal_show_line_numbers")
        .and_then(Value::as_bool)
    {
        prefs.insert(
            "terminal_show_line_numbers".to_string(),
            Value::Bool(line_numbers),
        );
    }
    if let Some(timestamps) = map.get("terminal_show_timestamps").and_then(Value::as_bool) {
        prefs.insert(
            "terminal_show_timestamps".to_string(),
            Value::Bool(timestamps),
        );
    }
    if let Some(format) = map
        .get("terminal_timestamp_format")
        .and_then(sanitize_timestamp_format)
    {
        prefs.insert(
            "terminal_timestamp_format".to_string(),
            Value::String(format),
        );
    }

    Value::Object(prefs)
}

/// Merges the allowlisted keys present in `params` into the store and persists
/// atomically (tmp + rename). Returns the stored preferences.
pub fn save_preferences(data_dir: &Path, params: &Value) -> Result<Value, String> {
    let mut map = load_map(data_dir);
    if let Some(value) = params.get("downloadDir") {
        let dir = sanitize_download_dir(value)
            .ok_or_else(|| "downloadDir must be a string".to_string())?;
        map.insert("downloadDir".to_string(), Value::String(dir));
    }
    if let Some(value) = params.get("downloadUseDefaultDir") {
        let use_default = value
            .as_bool()
            .ok_or_else(|| "downloadUseDefaultDir must be a boolean".to_string())?;
        map.insert(
            "downloadUseDefaultDir".to_string(),
            Value::Bool(use_default),
        );
    }
    if let Some(value) = params.get("downloadConflictPolicy") {
        let policy = sanitize_conflict_policy(value)
            .ok_or_else(|| "downloadConflictPolicy must be rename, ask or overwrite".to_string())?;
        map.insert(
            "downloadConflictPolicy".to_string(),
            Value::String(policy.to_string()),
        );
    }
    if let Some(value) = params.get("localShell") {
        let shell =
            sanitize_local_shell(value).ok_or_else(|| "localShell must be a string".to_string())?;
        map.insert("localShell".to_string(), Value::String(shell));
    }
    if let Some(value) = params.get("localShellIntegration") {
        let integration = value
            .as_bool()
            .ok_or_else(|| "localShellIntegration must be a boolean".to_string())?;
        map.insert(
            "localShellIntegration".to_string(),
            Value::Bool(integration),
        );
    }
    // 上传并发/重复策略与命令建议键：数值一律钳制到合法区间（非法回落默认），
    // 不报错，保证旧前端/手改文件不会把偏好写入卡死。
    if params.get("transfer_concurrency").is_some() {
        map.insert(
            "transfer_concurrency".to_string(),
            Value::from(sanitize_u64_clamped(
                &params["transfer_concurrency"],
                1,
                10,
                3,
            )),
        );
    }
    if let Some(value) = params.get("transfer_duplicate_policy") {
        let policy = sanitize_conflict_policy(value).ok_or_else(|| {
            "transfer_duplicate_policy must be rename, ask or overwrite".to_string()
        })?;
        map.insert(
            "transfer_duplicate_policy".to_string(),
            Value::String(policy.to_string()),
        );
    }
    if let Some(value) = params.get("history_suggestions_enabled") {
        let enabled = value
            .as_bool()
            .ok_or_else(|| "history_suggestions_enabled must be a boolean".to_string())?;
        map.insert(
            "history_suggestions_enabled".to_string(),
            Value::Bool(enabled),
        );
    }
    if params.get("history_suggestion_min_chars").is_some() {
        map.insert(
            "history_suggestion_min_chars".to_string(),
            Value::from(sanitize_u64_clamped(
                &params["history_suggestion_min_chars"],
                1,
                16,
                2,
            )),
        );
    }
    if params.get("history_suggestion_max_chars").is_some() {
        map.insert(
            "history_suggestion_max_chars".to_string(),
            Value::from(sanitize_u64_clamped(
                &params["history_suggestion_max_chars"],
                8,
                512,
                64,
            )),
        );
    }
    if let Some(value) = params.get("action_links_enabled") {
        let enabled = value
            .as_bool()
            .ok_or_else(|| "action_links_enabled must be a boolean".to_string())?;
        map.insert("action_links_enabled".to_string(), Value::Bool(enabled));
    }
    if let Some(value) = params.get("action_links_matchers") {
        let matchers = sanitize_action_links_matchers(value)
            .ok_or_else(|| "action_links_matchers must be an object".to_string())?;
        map.insert("action_links_matchers".to_string(), matchers);
    }
    if let Some(value) = params.get("terminal_show_line_numbers") {
        let line_numbers = value
            .as_bool()
            .ok_or_else(|| "terminal_show_line_numbers must be a boolean".to_string())?;
        map.insert(
            "terminal_show_line_numbers".to_string(),
            Value::Bool(line_numbers),
        );
    }
    if let Some(value) = params.get("terminal_show_timestamps") {
        let timestamps = value
            .as_bool()
            .ok_or_else(|| "terminal_show_timestamps must be a boolean".to_string())?;
        map.insert(
            "terminal_show_timestamps".to_string(),
            Value::Bool(timestamps),
        );
    }
    if let Some(value) = params.get("terminal_timestamp_format") {
        let format = sanitize_timestamp_format(value)
            .ok_or_else(|| "terminal_timestamp_format must be a string".to_string())?;
        map.insert(
            "terminal_timestamp_format".to_string(),
            Value::String(format),
        );
    }

    let path = store_path(data_dir);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let text = serde_json::to_string_pretty(&json!({
        "version": STORAGE_VERSION,
        "prefs": Value::Object(map),
    }))
    .map_err(|error| format!("Failed to encode preferences: {error}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)
        .map_err(|error| format!("Failed to write {}: {error}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .map_err(|error| format!("Failed to write {}: {error}", path.display()))?;
    Ok(load_preferences(data_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_terminal_prefs_roundtrip_with_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        // 空偏好：两键都不出现（前端按缺省处理）。
        let prefs = load_preferences(dir.path());
        assert!(prefs.get("localShell").is_none());
        assert!(prefs.get("localShellIntegration").is_none());
        // 写入 + 读回；空串 shell 归一为空串（=自动探测）。
        save_preferences(
            dir.path(),
            &serde_json::json!({ "localShell": "  /opt/homebrew/bin/fish  ", "localShellIntegration": false }),
        )
        .expect("save");
        let prefs = load_preferences(dir.path());
        assert_eq!(prefs["localShell"], "/opt/homebrew/bin/fish");
        assert_eq!(prefs["localShellIntegration"], false);
        // 非法类型报错且不落盘污染。
        let error = save_preferences(dir.path(), &serde_json::json!({ "localShell": 42 }))
            .expect_err("must reject");
        assert!(error.contains("localShell"));
    }

    #[test]
    fn roundtrip_merges_and_normalizes() {
        let data_dir = tempfile::tempdir().expect("tempdir");
        let stored = save_preferences(
            data_dir.path(),
            &json!({ "downloadDir": "  /tmp/dl  ", "downloadUseDefaultDir": true, "evil": "dropped" }),
        )
        .expect("save");
        assert_eq!(stored["downloadDir"].as_str().unwrap(), "/tmp/dl");
        assert_eq!(stored["downloadUseDefaultDir"].as_bool(), Some(true));
        assert!(stored.get("evil").is_none());
        // 部分更新只改出现的键。
        let again = save_preferences(data_dir.path(), &json!({ "downloadUseDefaultDir": false }))
            .expect("save again");
        assert_eq!(again["downloadDir"].as_str().unwrap(), "/tmp/dl");
        assert_eq!(again["downloadUseDefaultDir"].as_bool(), Some(false));
    }

    #[test]
    fn missing_or_corrupted_file_yields_empty() {
        let data_dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(load_preferences(data_dir.path()), json!({}));
        std::fs::write(store_path(data_dir.path()), "{not json").expect("write junk");
        assert_eq!(load_preferences(data_dir.path()), json!({}));
    }

    #[test]
    fn rejects_wrong_types() {
        let data_dir = tempfile::tempdir().expect("tempdir");
        assert!(save_preferences(data_dir.path(), &json!({ "downloadDir": 7 })).is_err());
        assert!(
            save_preferences(data_dir.path(), &json!({ "downloadUseDefaultDir": "yes" })).is_err()
        );
    }

    #[test]
    fn transfer_and_suggestion_prefs_clamp_and_roundtrip() {
        let data_dir = tempfile::tempdir().expect("tempdir");
        save_preferences(
            data_dir.path(),
            &json!({
                "transfer_concurrency": 99,
                "transfer_duplicate_policy": "ask",
                "history_suggestions_enabled": false,
                "history_suggestion_min_chars": 0,
                "history_suggestion_max_chars": 999,
            }),
        )
        .expect("save");
        let prefs = load_preferences(data_dir.path());
        assert_eq!(prefs["transfer_concurrency"], 10);
        assert_eq!(prefs["transfer_duplicate_policy"], "ask");
        assert_eq!(prefs["history_suggestions_enabled"], false);
        assert_eq!(prefs["history_suggestion_min_chars"], 1);
        assert_eq!(prefs["history_suggestion_max_chars"], 512);
        // 非法策略名报错；缺省键不出现（前端按默认处理）。
        let error = save_preferences(
            data_dir.path(),
            &json!({ "transfer_duplicate_policy": "clobber" }),
        )
        .expect_err("must reject");
        assert!(error.contains("transfer_duplicate_policy"));
        let fresh = tempfile::tempdir().expect("tempdir");
        assert_eq!(load_preferences(fresh.path()), json!({}));
    }

    #[test]
    fn terminal_feature_prefs_roundtrip_with_defaults() {
        let data_dir = tempfile::tempdir().expect("tempdir");
        // 空偏好：五键都不出现（前端按缺省 = 功能全关处理）。
        let prefs = load_preferences(data_dir.path());
        assert!(prefs.get("action_links_enabled").is_none());
        assert!(prefs.get("action_links_matchers").is_none());
        assert!(prefs.get("terminal_show_line_numbers").is_none());
        assert!(prefs.get("terminal_show_timestamps").is_none());
        assert!(prefs.get("terminal_timestamp_format").is_none());
        // 写入 + 读回：matchers 部分字段缺省为 true；格式串原样保留。
        save_preferences(
            data_dir.path(),
            &serde_json::json!({
                "action_links_enabled": true,
                "action_links_matchers": { "ipv4": false },
                "terminal_show_line_numbers": true,
                "terminal_show_timestamps": true,
                "terminal_timestamp_format": "[HH:mm]",
            }),
        )
        .expect("save");
        let prefs = load_preferences(data_dir.path());
        assert_eq!(prefs["action_links_enabled"], true);
        assert_eq!(prefs["action_links_matchers"]["ipv4"], false);
        assert_eq!(prefs["action_links_matchers"]["host_port"], true);
        assert_eq!(prefs["action_links_matchers"]["archive"], true);
        assert_eq!(prefs["terminal_show_line_numbers"], true);
        assert_eq!(prefs["terminal_show_timestamps"], true);
        assert_eq!(prefs["terminal_timestamp_format"], "[HH:mm]");
        // 非法形状报错且不落盘污染；matchers 非对象拒绝。
        assert!(save_preferences(
            data_dir.path(),
            &serde_json::json!({ "action_links_enabled": 1 })
        )
        .is_err());
        assert!(save_preferences(
            data_dir.path(),
            &serde_json::json!({ "action_links_matchers": "all" })
        )
        .is_err());
        assert!(save_preferences(
            data_dir.path(),
            &serde_json::json!({ "terminal_show_timestamps": "yes" })
        )
        .is_err());
        // 格式串清洗：危险字符剔除、超长截断、清洗后为空回退默认。
        save_preferences(
            data_dir.path(),
            &serde_json::json!({ "terminal_timestamp_format": "  <>  " }),
        )
        .expect("save fallback");
        let prefs = load_preferences(data_dir.path());
        assert_eq!(prefs["terminal_timestamp_format"], "[HH:mm:ss]");
        let long = "Y".repeat(100);
        save_preferences(
            data_dir.path(),
            &serde_json::json!({ "terminal_timestamp_format": long }),
        )
        .expect("save long");
        let prefs = load_preferences(data_dir.path());
        assert_eq!(
            prefs["terminal_timestamp_format"].as_str().unwrap().len(),
            64
        );
    }
}
