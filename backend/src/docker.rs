//! Docker management panel backend (`docker/list` / `docker/logs` /
//! `docker/action`): POSIX collector scripts executed through `sh -s`
//! heredocs plus pure parsers, in the same style as [`crate::metrics`].
//! Zero new dependencies by design — the Docker CLI is driven over shell,
//! never the daemon API, and no docker client crate is pulled in.
//!
//! Safety model for actions: the verb comes from a fixed whitelist, the
//! container id must match `^[0-9a-f]{12,64}$`, read-only connections are
//! refused by the caller (`ensure_writable`), the intent is audited before
//! execution, and the sudo password never travels on the command line —
//! the fallback reuses the Quick Sudo pipeline (`exec_with_sudo`) which
//! pipes credentials over stdin.

use std::path::Path;
use std::time::Duration;

use russh::client::Handle;
use serde::Serialize;
use serde_json::{json, Value};

use crate::audit_log;
use crate::exec;
use crate::ssh::SshClient;

/// Wait cap for the container list collector (one `docker ps` round-trip).
pub const LIST_TIMEOUT: Duration = Duration::from_secs(15);
/// Wait cap for `docker logs --tail N`; tailing is bounded by `--tail`.
pub const LOGS_TIMEOUT: Duration = Duration::from_secs(30);
/// Wait cap for lifecycle actions; `stop`/`restart` default to a 10s
/// container shutdown timeout, so 60s covers the worst case with slack.
pub const ACTION_TIMEOUT: Duration = Duration::from_secs(60);

/// Bounds for the `--tail` line count of `docker/logs`.
pub const TAIL_MIN: u64 = 10;
pub const TAIL_MAX: u64 = 2000;
pub const TAIL_DEFAULT: u64 = 200;

// —— Scripts ——————————————————————————————————————————————

/// Container list collector. The probe section distinguishes "docker not
/// installed" (available: false) from "daemon socket denied" (available:
/// false plus needsSudo: true, the docker-group hint) so the panel can
/// explain *why* a host shows an empty list instead of a bare empty state.
pub const LIST_SCRIPT: &str = concat!(
    "sh -s <<'DBXDOCKER'\n",
    "echo DBXDOCKER_PROBE\n",
    "if command -v docker >/dev/null 2>&1; then echo docker-found; else echo docker-missing; fi\n",
    "if docker info >/dev/null 2>&1; then echo daemon-ok; else echo daemon-denied; fi\n",
    "echo DBXDOCKER_PS\n",
    "docker ps -a --no-trunc --format '{{.ID}}\\t{{.Names}}\\t{{.Image}}\\t{{.State}}\\t{{.Status}}\\t{{.Ports}}\\t{{.CreatedAt}}' 2>/dev/null\n",
    "DBXDOCKER\n"
);

/// Builds the logs + inspect collector for one container. Inspect runs
/// first so the (potentially large) log tail cannot bury the marker scan,
/// and the log section is last by construction (everything after its
/// marker is the log text, `2>&1` merged so permission errors surface).
pub fn logs_script(container_id: &str, tail: u64) -> Result<String, String> {
    validate_container_id(container_id)?;
    let tail = clamp_tail(tail);
    Ok(format!(
        "sh -s <<'DBXDOCKER'\n\
         echo DBXDOCKER_INSPECT\n\
         docker inspect {container_id} 2>/dev/null\n\
         echo DBXDOCKER_LOGS\n\
         docker logs --tail {tail} {container_id} 2>&1\n\
         DBXDOCKER\n"
    ))
}

/// Lifecycle verbs the panel may run. Anything outside this whitelist is
/// rejected before any remote I/O (`parse_action`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockerAction {
    Start,
    Stop,
    Restart,
    Kill,
    Remove,
}

impl DockerAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Kill => "kill",
            Self::Remove => "rm",
        }
    }
}

/// Parses the requested action name. Unknown verbs fail with the accepted
/// list in the error (typo-guidance style, like metrics sections).
pub fn parse_action(name: &str) -> Result<DockerAction, String> {
    match name {
        "start" => Ok(DockerAction::Start),
        "stop" => Ok(DockerAction::Stop),
        "restart" => Ok(DockerAction::Restart),
        "kill" => Ok(DockerAction::Kill),
        "rm" | "remove" => Ok(DockerAction::Remove),
        _ => Err(format!(
            "Unsupported docker action '{}'. Supported: start, stop, restart, kill, rm",
            name
        )),
    }
}

/// Renders the remote command for one action. The id is inserted verbatim;
/// callers must have run [`validate_container_id`] (both protocol entry
/// points do, and the format! below only ever receives validated input).
pub fn action_command(action: DockerAction, container_id: &str) -> String {
    format!("docker {} {}", action.as_str(), container_id)
}

/// Container id gate: `^[0-9a-f]{12,64}$`. Hand-rolled (no regex needed):
/// lowercase hex only, 12 (short id) to 64 (full sha256) chars. Anything
/// else — names, ids with shells metacharacters, uppercase — is refused
/// before a command is ever rendered.
pub fn validate_container_id(container_id: &str) -> Result<(), String> {
    let valid_len = (12..=64).contains(&container_id.len());
    let hex_only = container_id
        .bytes()
        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
    if valid_len && hex_only {
        return Ok(());
    }
    Err(format!(
        "Invalid containerId '{container_id}': expected 12-64 lowercase hex characters"
    ))
}

/// Clamps the requested tail line count into [10, 2000].
pub fn clamp_tail(tail: u64) -> u64 {
    tail.clamp(TAIL_MIN, TAIL_MAX)
}

// —— Parsing (pure, fixture-tested) ———————————————————————

/// One row of `docker ps -a --format` (tab-separated, NyaTerm field set).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerRow {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub ports: String,
    pub created_at: String,
}

/// Extracts the text between two exact marker lines (end marker optional —
/// everything after the start marker is returned). Exact whole-line
/// equality on both ends; the sentinel names are unusual enough that a
/// container log echoing them stays a documented corner case.
fn marker_section<'a>(output: &'a str, start: &str, end: Option<&str>) -> &'a str {
    let mut cursor = 0usize;
    let mut begin: Option<usize> = None;
    for line in output.split_inclusive('\n') {
        let trimmed = line.trim();
        if let Some(start_at) = begin {
            if Some(trimmed) == end {
                return &output[start_at..cursor];
            }
        } else if trimmed == start {
            begin = Some(cursor + line.len());
        }
        cursor += line.len();
    }
    match begin {
        Some(start_at) => &output[start_at..],
        None => "",
    }
}

/// Probe verdict for one host: docker CLI present, daemon reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Probe {
    pub docker_found: bool,
    pub daemon_ok: bool,
}

/// Parses the `DBXDOCKER_PROBE` section (two self-describing lines).
pub fn parse_probe(output: &str) -> Probe {
    let mut probe = Probe::default();
    for line in marker_section(output, "DBXDOCKER_PROBE", Some("DBXDOCKER_PS")).lines() {
        match line.trim() {
            "docker-found" => probe.docker_found = true,
            "daemon-ok" => probe.daemon_ok = true,
            _ => {}
        }
    }
    probe
}

/// Parses the tab-separated `docker ps` section. Malformed rows (wrong
/// column count, empty ids) are skipped line by line — one bad row must
/// never hide the rest of the list.
pub fn parse_ps_section(section: &str) -> Vec<ContainerRow> {
    let mut rows = Vec::new();
    for line in section.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 7 || fields[0].trim().is_empty() {
            continue;
        }
        rows.push(ContainerRow {
            id: fields[0].trim().to_string(),
            name: fields[1].trim().to_string(),
            image: fields[2].trim().to_string(),
            state: fields[3].trim().to_string(),
            status: fields[4].trim().to_string(),
            ports: fields[5].trim().to_string(),
            created_at: fields[6].trim().to_string(),
        });
    }
    rows
}

/// Assembles the `docker/list` payload from raw script output:
/// `{available, needsSudo, containers}`. `needsSudo` is the found-but-
/// denied combination (docker CLI present, daemon socket refused) — the
/// actions still have a path via the Quick Sudo fallback.
pub fn list_payload(output: &str) -> Value {
    let probe = parse_probe(output);
    let available = probe.docker_found && probe.daemon_ok;
    let needs_sudo = probe.docker_found && !probe.daemon_ok;
    let containers = parse_ps_section(marker_section(output, "DBXDOCKER_PS", None));
    json!({
        "available": available,
        "needsSudo": needs_sudo,
        "containers": containers,
    })
}

/// Key fields projected out of one `docker inspect` JSON document.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct InspectSummary {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
    pub running: bool,
    pub started_at: String,
    pub health: Option<String>,
    pub restart_policy: String,
}

/// Parses `docker inspect` output (a JSON array; the first element wins)
/// into the summary. Missing/absent fields degrade to defaults so a
/// truncated or exotic inspect document never fails the logs view.
pub fn parse_inspect_output(text: &str) -> Option<InspectSummary> {
    let parsed: Value = serde_json::from_str(text.trim()).ok()?;
    let container = parsed.as_array()?.first()?;
    let state = container.get("State");
    let string_at = |pointer: &str| -> String {
        container
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Some(InspectSummary {
        id: string_at("/Id"),
        name: string_at("/Name").trim_start_matches('/').to_string(),
        image: string_at("/Config/Image"),
        status: state
            .and_then(|state| state.get("Status"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        running: state
            .and_then(|state| state.get("Running"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        started_at: state
            .and_then(|state| state.get("StartedAt"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        health: state
            .and_then(|state| state.get("Health"))
            .and_then(|health| health.get("Status"))
            .and_then(Value::as_str)
            .map(str::to_string),
        restart_policy: container
            .pointer("/HostConfig/RestartPolicy/Name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

/// Assembles the `docker/logs` payload: `{logs, container}` where
/// `container` is the optional inspect summary (null when inspect failed,
/// e.g. the container vanished between list and logs).
pub fn logs_payload(output: &str) -> Value {
    let inspect_text = marker_section(output, "DBXDOCKER_INSPECT", Some("DBXDOCKER_LOGS"));
    let logs = marker_section(output, "DBXDOCKER_LOGS", None);
    json!({
        "logs": logs,
        "container": parse_inspect_output(inspect_text),
    })
}

/// True when a failed plain run is the docker-daemon socket permission
/// error — the only failure shape that justifies the sudo fallback. Every
/// other failure must stay terminal: retrying a half-executed action with
/// sudo could double-apply a state change.
pub fn is_daemon_permission_failure(exit_code: i32, output: &str) -> bool {
    exit_code != 0 && output.to_ascii_lowercase().contains("permission denied")
}

/// Readable error when even the Quick Sudo fallback failed; points at the
/// configuration surface instead of leaking raw sudo noise.
pub fn sudo_fallback_error(error: String) -> String {
    format!(
        "{error}. Docker actions need daemon permission: add the user to the \
         docker group on the host, or configure Quick Sudo for this connection \
         (connection settings > Quick Sudo) so the panel can elevate."
    )
}

// —— Handle-plane collectors (MCP tool path) ——————————————

/// `docker_list`: one `LIST_SCRIPT` round-trip on a fresh exec channel.
pub async fn collect_list(handle: &Handle<SshClient>) -> Result<Value, String> {
    let outcome = exec::exec_plain(handle, LIST_SCRIPT, LIST_TIMEOUT, &[]).await?;
    Ok(list_payload(&outcome.output))
}

/// `docker_action` for the MCP plane: plain run first; on the daemon
/// permission signature only, retries through the Quick Sudo pipeline
/// (`exec_with_sudo` pipes the password over stdin, never the command
/// line, and answers TOTP follow-up prompts). `sudo_auth: None` (no
/// resolvable credentials) skips the fallback and returns the guidance.
pub async fn perform_action(
    handle: &Handle<SshClient>,
    sudo_auth: Option<&exec::SudoAuth>,
    use_pty: bool,
    container_id: &str,
    action: DockerAction,
) -> Result<Value, String> {
    let command = action_command(action, container_id);
    let outcome = exec::exec_plain(handle, &command, ACTION_TIMEOUT, &[]).await?;
    if outcome.exit_code == 0 {
        return Ok(json!({ "success": true, "output": outcome.output }));
    }
    if is_daemon_permission_failure(outcome.exit_code, &outcome.output) {
        let Some(auth) = sudo_auth else {
            return Err(sudo_fallback_error(format!(
                "docker {}: {}",
                action.as_str(),
                outcome.output
            )));
        };
        return match exec::exec_with_sudo(handle, auth, &command, ACTION_TIMEOUT, use_pty, &[])
            .await
        {
            Ok(sudo_outcome) => Ok(json!({ "success": true, "output": sudo_outcome.output })),
            Err(error) => Err(sudo_fallback_error(error)),
        };
    }
    Err(format!(
        "docker {} failed (exit {}): {}",
        action.as_str(),
        outcome.exit_code,
        outcome.output
    ))
}

// —— Audit (workbench docker/action path) ——————————————————

fn audit_row(
    data_dir: &Path,
    command: &str,
    outcome: audit_log::EntryOutcome,
    error: Option<&str>,
    duration_ms: u64,
) {
    let ts_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default();
    let entry = audit_log::AuditEntry {
        ts_ms,
        tool: "docker/action".to_string(),
        // The workbench path keys on sessionId; the connection column stays
        // empty exactly like MCP rows without a connectionId argument.
        connection_id: String::new(),
        gate: audit_log::GateOutcome::Pass,
        approval: audit_log::ApprovalTrail::None,
        outcome,
        exit_code: None,
        duration_ms,
        mode: audit_log::ExecMode::Stdio,
        command: Some(command.to_string()),
        output: None,
        error: error.map(|error| error.chars().take(256).collect()),
    };
    if let Err(error) = audit_log::append(data_dir, &entry) {
        eprintln!("[docker] audit append failed: {error}");
    }
}

/// Mandatory pre-execution audit row: the intent is on the ledger even if
/// the sidecar dies between the audit write and the remote run.
pub fn audit_action_intent(data_dir: &Path, command: &str) {
    audit_row(data_dir, command, audit_log::EntryOutcome::Ok, None, 0);
}

/// Post-execution failure row (success is already covered by the intent
/// row; only failures add a second entry carrying the error text).
pub fn audit_action_failure(data_dir: &Path, command: &str, error: &str, duration_ms: u64) {
    audit_row(
        data_dir,
        command,
        audit_log::EntryOutcome::Error,
        Some(error),
        duration_ms,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `docker ps -a` output as the script emits it: tab-separated, one
    /// running and one exited container.
    const PS_FIXTURE: &str = "\
d4a7c9f1e2b3a1c0d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7\tweb-nginx\tnginx:1.27\trunning\tUp 3 days\t0.0.0.0:8080->80/tcp, :::8080->80/tcp\t2026-09-01 08:15:04 +0000 UTC
9f8e7d6c5b4a\tcache\tredis:7-alpine\texited\tExited (0) 2 hours ago\t\t2026-09-18 21:30:00 +0000 UTC
";

    const LIST_FIXTURE_WITH_ROWS: &str = "\
DBXDOCKER_PROBE
docker-found
daemon-ok
DBXDOCKER_PS
d4a7c9f1e2b3\tweb-nginx\tnginx:1.27\trunning\tUp 3 days\t0.0.0.0:8080->80/tcp\t2026-09-01 08:15:04 +0000 UTC
9f8e7d6c5b4a\tcache\tredis:7-alpine\texited\tExited (0) 2 hours ago\t\t2026-09-18 21:30:00 +0000 UTC
";

    const INSPECT_FIXTURE: &str = r#"[
    {
        "Id": "sha256:d4a7c9f1e2b3a1c0d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7",
        "Created": "2026-09-01T08:15:03.9Z",
        "Path": "nginx",
        "Name": "/web-nginx",
        "State": {
            "Status": "running",
            "Running": true,
            "StartedAt": "2026-09-20T06:00:00.1Z",
            "Health": { "Status": "healthy" }
        },
        "Config": { "Image": "nginx:1.27" },
        "HostConfig": { "RestartPolicy": { "Name": "unless-stopped" } }
    }
]"#;

    #[test]
    fn parses_container_rows_from_tab_output() {
        let rows = parse_ps_section(PS_FIXTURE);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id.len(), 64);
        assert_eq!(rows[0].name, "web-nginx");
        assert_eq!(rows[0].state, "running");
        assert_eq!(rows[0].ports, "0.0.0.0:8080->80/tcp, :::8080->80/tcp");
        assert_eq!(rows[0].created_at, "2026-09-01 08:15:04 +0000 UTC");
        // Empty ports column survives as an empty string (exited container).
        assert_eq!(rows[1].id, "9f8e7d6c5b4a");
        assert_eq!(rows[1].ports, "");
        assert_eq!(rows[1].state, "exited");
    }

    #[test]
    fn malformed_ps_rows_are_skipped_line_by_line() {
        let text = "only-three-columns\there\tyeah\n\
                     \t\n\
                     9f8e7d6c5b4a\tcache\tredis:7\texited\tExited (0)\t\t2026-09-18 21:30:00 +0000 UTC\n";
        let rows = parse_ps_section(text);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "cache");
        assert!(parse_ps_section("").is_empty());
        assert!(parse_ps_section("no tabs at all\n").is_empty());
    }

    #[test]
    fn list_payload_reports_available_and_containers() {
        let payload = list_payload(LIST_FIXTURE_WITH_ROWS);
        assert_eq!(payload["available"], true);
        assert_eq!(payload["needsSudo"], false);
        assert_eq!(payload["containers"].as_array().unwrap().len(), 2);
        assert_eq!(payload["containers"][0]["name"], "web-nginx");
        assert_eq!(payload["containers"][0]["id"], "d4a7c9f1e2b3");
    }

    #[test]
    fn list_payload_marks_missing_docker_unavailable() {
        let output = "DBXDOCKER_PROBE\ndocker-missing\ndaemon-denied\nDBXDOCKER_PS\n";
        let payload = list_payload(output);
        assert_eq!(payload["available"], false);
        assert_eq!(payload["needsSudo"], false);
        assert_eq!(payload["containers"].as_array().unwrap().len(), 0);
        // Garbage or empty output (script failed entirely) degrades to the
        // unavailable state instead of erroring the whole call.
        let empty = list_payload("");
        assert_eq!(empty["available"], false);
    }

    #[test]
    fn list_payload_marks_daemon_denied_as_needs_sudo() {
        let output = "DBXDOCKER_PROBE\ndocker-found\ndaemon-denied\nDBXDOCKER_PS\n";
        let payload = list_payload(output);
        assert_eq!(payload["available"], false);
        assert_eq!(payload["needsSudo"], true);
    }

    #[test]
    fn container_id_gate_enforces_lowercase_hex_length() {
        assert!(validate_container_id("d4a7c9f1e2b3").is_ok()); // 12 hex
        assert!(validate_container_id(&"a".repeat(64)).is_ok()); // full sha
                                                                 // Too short / too long.
        assert!(validate_container_id("d4a7c9f1e2b").is_err());
        assert!(validate_container_id(&"a".repeat(65)).is_err());
        // Uppercase, names, separators, shell metacharacters.
        assert!(validate_container_id("D4A7C9F1E2B3").is_err());
        assert!(validate_container_id("web-nginx").is_err());
        assert!(validate_container_id("d4a7c9f1e2b3; rm -rf /").is_err());
        assert!(validate_container_id("d4a7c9f1 e2b3").is_err());
        assert!(validate_container_id("").is_err());
    }

    #[test]
    fn tail_clamps_into_the_documented_band() {
        assert_eq!(clamp_tail(0), TAIL_MIN);
        assert_eq!(clamp_tail(5), TAIL_MIN);
        assert_eq!(clamp_tail(200), 200);
        assert_eq!(clamp_tail(5000), TAIL_MAX);
        assert_eq!(clamp_tail(u64::MAX), TAIL_MAX);
    }

    #[test]
    fn action_whitelist_and_command_rendering() {
        assert_eq!(parse_action("start").unwrap(), DockerAction::Start);
        assert_eq!(parse_action("rm").unwrap(), DockerAction::Remove);
        assert_eq!(parse_action("remove").unwrap(), DockerAction::Remove);
        let error = parse_action("exec").unwrap_err();
        assert!(
            error.contains("Unsupported docker action 'exec'"),
            "{error}"
        );
        assert!(error.contains("start, stop, restart, kill, rm"), "{error}");
        assert!(parse_action("").is_err());
        assert!(parse_action("RM -RF /").is_err());

        assert_eq!(
            action_command(DockerAction::Start, "d4a7c9f1e2b3"),
            "docker start d4a7c9f1e2b3"
        );
        assert_eq!(
            action_command(DockerAction::Remove, "d4a7c9f1e2b3"),
            "docker rm d4a7c9f1e2b3"
        );
    }

    #[test]
    fn logs_script_validates_and_bakes_parameters() {
        let script = logs_script("d4a7c9f1e2b3", 120).unwrap();
        assert!(script.starts_with("sh -s <<'DBXDOCKER'"), "{script}");
        assert!(script.contains("docker inspect d4a7c9f1e2b3"), "{script}");
        assert!(
            script.contains("docker logs --tail 120 d4a7c9f1e2b3"),
            "{script}"
        );
        // Tail is clamped inside the script too.
        let clamped = logs_script("d4a7c9f1e2b3", 99_999).unwrap();
        assert!(clamped.contains("docker logs --tail 2000"), "{clamped}");
        // Invalid ids are refused before any command exists.
        assert!(logs_script("web-nginx", 100).is_err());
    }

    #[test]
    fn marker_sections_split_inspect_and_logs() {
        let output =
            format!("DBXDOCKER_INSPECT\n{INSPECT_FIXTURE}\nDBXDOCKER_LOGS\nline one\nline two\n");
        let payload = logs_payload(&output);
        assert_eq!(payload["logs"], "line one\nline two\n");
        let container = &payload["container"];
        assert_eq!(container["name"], "web-nginx");
        assert_eq!(container["image"], "nginx:1.27");
        assert_eq!(container["status"], "running");
        assert_eq!(container["running"], true);
        assert_eq!(container["startedAt"], "2026-09-20T06:00:00.1Z");
        assert_eq!(container["health"], "healthy");
        assert_eq!(container["restartPolicy"], "unless-stopped");
        assert!(container["id"]
            .as_str()
            .unwrap()
            .starts_with("sha256:d4a7c9f1"));
    }

    #[test]
    fn logs_payload_tolerates_missing_or_broken_inspect() {
        // No inspect section at all (probe failed before inspect ran).
        let no_inspect = logs_payload("DBXDOCKER_LOGS\nhello\n");
        assert_eq!(no_inspect["logs"], "hello\n");
        assert_eq!(no_inspect["container"], Value::Null);
        // Inspect section present but not JSON (docker wrote an error there).
        let broken = logs_payload("DBXDOCKER_INSPECT\nError: No such object\nDBXDOCKER_LOGS\n");
        assert_eq!(broken["container"], Value::Null);
        assert!(parse_inspect_output("").is_none());
        assert!(parse_inspect_output("[]").is_none());
        // Inspect object without the exotic fields still yields a summary.
        let minimal = parse_inspect_output(r#"[{"Id":"sha256:ab","Name":"/x"}]"#).unwrap();
        assert_eq!(minimal.name, "x");
        assert_eq!(minimal.image, "");
        assert_eq!(minimal.health, None);
    }

    #[test]
    fn daemon_permission_signature_gates_the_sudo_fallback() {
        // Real docker CLI wording on a socket-permission failure.
        assert!(is_daemon_permission_failure(
            1,
            "docker: Got permission denied while trying to connect to the Docker daemon socket"
        ));
        assert!(is_daemon_permission_failure(125, "permission denied"));
        // Case-insensitive match on the merged output.
        assert!(is_daemon_permission_failure(1, "PERMISSION DENIED"));
        // Success or any other failure shape must NOT trigger the fallback
        // (retrying a half-executed action with sudo could double-apply it).
        assert!(!is_daemon_permission_failure(0, "permission denied"));
        assert!(!is_daemon_permission_failure(
            1,
            "Error response from daemon: No such container: d4a7c9f1e2b3"
        ));
        assert!(!is_daemon_permission_failure(1, ""));
    }

    #[test]
    fn sudo_fallback_error_points_at_configuration() {
        let error = sudo_fallback_error("sudo exited 1: boom".to_string());
        assert!(error.contains("Quick Sudo"), "{error}");
        assert!(error.contains("docker group"), "{error}");
    }

    #[test]
    fn audit_rows_land_before_and_after_failures() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path();
        audit_action_intent(data_dir, "docker restart d4a7c9f1e2b3");
        audit_action_failure(
            data_dir,
            "docker restart d4a7c9f1e2b3",
            "docker restart failed (exit 1): oops",
            42,
        );
        let entries = audit_log::tail(data_dir, 10, None).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].tool, "docker/action");
        assert_eq!(entries[0].outcome, audit_log::EntryOutcome::Ok);
        assert_eq!(
            entries[0].command.as_deref(),
            Some("docker restart d4a7c9f1e2b3")
        );
        assert_eq!(entries[1].outcome, audit_log::EntryOutcome::Error);
        assert_eq!(
            entries[1].error.as_deref(),
            Some("docker restart failed (exit 1): oops")
        );
        assert_eq!(entries[1].duration_ms, 42);
    }
}
