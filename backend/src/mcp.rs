//! MCP stdio server mode (`--mcp`): exposes SSH exec and SFTP tools to MCP
//! clients over newline-delimited JSON-RPC, mirroring tiny-rdm's MCP tool
//! surface (ssh_exec, ssh_exec_sudo, sftp_*) adapted to inline connection
//! parameters with a per-process connection pool.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use dbx_plugin_sdk::PluginEmitter;
use russh::client::Handle;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::FileType;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::sync::{Mutex as AsyncMutex, RwLock as AsyncRwLock};

use crate::agent_terminal::{self, AgentTerminalMode};
use crate::app_bridge;
use crate::exec::{self, AuthFlowMode, Hints, SudoAuth};
use crate::host_key::HostKeyVerifier;
use crate::mcp_safety::{self, CommandRisk};
use crate::model::{AuthenticationMethod, JumpHost, StoredConnection, SudoSource};
use crate::sftp_copy;
use crate::ssh::{SshClient, SshRuntime, NO_TERMINAL_SESSION_MESSAGE};
use crate::sudo_allowlist;
use crate::sudo_profiles;

const PROTOCOL_VERSION: &str = "2024-11-05";

/// Ceilings for the MCP size settings (tiny-rdm's PreferencesMCPSFTP
/// equivalent): `mcp/settings/set` may lower a limit freely or raise it up
/// to these values, never beyond. The same ceilings clamp the values after
/// loading `mcp-settings.json`, so a corrupted or hand-edited file cannot
/// disable the caps.
const READ_LIMIT_CEILING: u64 = 8 * 1024 * 1024;
const UPLOAD_LIMIT_CEILING: u64 = 2 * 1024 * 1024 * 1024;
const DOWNLOAD_LIMIT_CEILING: u64 = 2 * 1024 * 1024 * 1024;

/// Size preferences for the MCP tools, persisted in
/// `<plugin_data_dir>/mcp-settings.json`:
/// - `max_read_bytes`: default size of a single `sftp_read_file` call
///   (previously the `DEFAULT_READ_BYTES` constant);
/// - `max_download_bytes`: ceiling for an explicit `maxBytes` request,
///   i.e. the most a single read/download may return (previously the
///   `MAX_READ_BYTES` constant);
/// - `max_upload_bytes`: ceiling for the content of a single
///   `sftp_write_file` call (new; writes were previously uncapped);
/// - `local_transfer_root`: confinement root for the local side of
///   `sftp_upload` / `sftp_download`; empty = the default roots (OS temp
///   dir + plugin data dir). See
///   [`ensure_local_transfer_allowed_in`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpLimits {
    pub max_read_bytes: u64,
    pub max_upload_bytes: u64,
    pub max_download_bytes: u64,
    pub local_transfer_root: String,
}

impl Default for McpLimits {
    fn default() -> Self {
        Self {
            max_read_bytes: 256 * 1024,
            max_upload_bytes: 16 * 1024 * 1024,
            max_download_bytes: 1024 * 1024,
            local_transfer_root: String::new(),
        }
    }
}

impl McpLimits {
    /// Clamps each field into `1..=ceiling`. Applied after loading the
    /// settings file and after every update so out-of-range values can
    /// never reach the tool implementations.
    fn sanitized(self) -> Self {
        Self {
            max_read_bytes: self.max_read_bytes.clamp(1, READ_LIMIT_CEILING),
            max_upload_bytes: self.max_upload_bytes.clamp(1, UPLOAD_LIMIT_CEILING),
            max_download_bytes: self.max_download_bytes.clamp(1, DOWNLOAD_LIMIT_CEILING),
            local_transfer_root: self.local_transfer_root,
        }
    }

    /// Parses the persisted JSON, falling back per-field to the defaults for
    /// missing or non-numeric entries.
    fn from_json(value: &Value) -> Self {
        let defaults = Self::default();
        let field =
            |key: &str, fallback: u64| value.get(key).and_then(Value::as_u64).unwrap_or(fallback);
        Self {
            max_read_bytes: field("maxReadBytes", defaults.max_read_bytes),
            max_upload_bytes: field("maxUploadBytes", defaults.max_upload_bytes),
            max_download_bytes: field("maxDownloadBytes", defaults.max_download_bytes),
            local_transfer_root: value
                .get("localTransferRoot")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
        }
        .sanitized()
    }

    pub fn to_json(self) -> Value {
        json!({
            "maxReadBytes": self.max_read_bytes,
            "maxUploadBytes": self.max_upload_bytes,
            "maxDownloadBytes": self.max_download_bytes,
            "localTransferRoot": self.local_transfer_root,
        })
    }

    /// Reads the persisted settings; a missing or corrupted file falls back
    /// to the defaults and never fails.
    fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .map(|value| Self::from_json(&value))
            .unwrap_or_default()
    }

    fn save(self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let text = serde_json::to_string_pretty(&self.to_json())
            .map_err(|error| format!("Failed to encode MCP settings: {error}"))?;
        std::fs::write(path, text)
            .map_err(|error| format!("Failed to write MCP settings {}: {error}", path.display()))
    }
}

/// Validates the `localTransferRoot` setting: an absolute path string, or
/// empty to clear the override (falling back to the default transfer
/// roots).
fn validated_transfer_root(value: &Value) -> Result<String, String> {
    let root = value
        .as_str()
        .ok_or_else(|| {
            "localTransferRoot must be a string (absolute path, or empty to reset)".to_string()
        })?
        .trim()
        .to_string();
    if !root.is_empty() && !Path::new(&root).is_absolute() {
        return Err(
            "localTransferRoot must be an absolute path (or empty to reset)".to_string(),
        );
    }
    Ok(root)
}

/// Validates one `mcp/settings/set` field: an unsigned integer within
/// `1..=ceiling`.
fn validated_limit(value: &Value, name: &str, ceiling: u64) -> Result<u64, String> {
    let bytes = value
        .as_u64()
        .ok_or_else(|| format!("{name} must be a positive integer number of bytes"))?;
    if bytes == 0 || bytes > ceiling {
        return Err(format!("{name} must be between 1 and {ceiling} bytes"));
    }
    Ok(bytes)
}

pub fn run_mcp_stdio(data_dir: PathBuf) -> io::Result<()> {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|error| io::Error::other(format!("Failed to create async runtime: {error}")))?;
    let state = Arc::new(McpState::new(data_dir));
    let stdin = io::stdin();
    // Spawned handlers may finish out of order; the mutex keeps each
    // JSON-RPC line intact and id-based correlation makes ordering
    // irrelevant to callers.
    let stdout = Arc::new(std::sync::Mutex::new(io::stdout()));
    // Handles of every spawned request, drained before exit so a task that
    // is mid-connection (or mid-command) isn't cancelled when stdin closes.
    let mut in_flight = Vec::new();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(error) => {
                write_response(
                    &stdout,
                    json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("Parse error: {error}") } }),
                )?;
                continue;
            }
        };
        // Spawn every request instead of block_on: one long tool call (a
        // slow ssh_exec, an sftp transfer) must not stall ping, tools/list,
        // or calls for other connections behind it. The host may already
        // have abandoned THIS call; its handler still runs to completion
        // and replies into the pipe.
        let state = Arc::clone(&state);
        let stdout = Arc::clone(&stdout);
        in_flight.push(runtime.spawn(async move {
            if let Some(response) = state.dispatch(request).await {
                let _ = write_response(&stdout, response);
            }
        }));
    }
    // stdin is closed: drain in-flight handlers (bounded, as a runaway
    // handler must not pin the process forever) before the runtime drops.
    // The timeout future is built INSIDE block_on: tokio timers capture
    // Handle::current() at construction, which needs the runtime context.
    let drain = async {
        for handle in in_flight {
            let _ = handle.await;
        }
    };
    let _ = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(300), drain).await
    });
    Ok(())
}

fn write_response(
    stdout: &std::sync::Mutex<io::Stdout>,
    response: Value,
) -> io::Result<()> {
    let mut guard = stdout
        .lock()
        .map_err(|poisoned| io::Error::other(poisoned.to_string()))?;
    writeln!(guard, "{response}")?;
    guard.flush()
}

struct McpConnection {
    handle: Arc<Handle<SshClient>>,
    #[allow(dead_code)]
    jumps: Vec<Arc<Handle<SshClient>>>,
    sftp: Option<Arc<AsyncMutex<SftpSession>>>,
}

impl McpConnection {
    async fn sftp(&mut self) -> Result<Arc<AsyncMutex<SftpSession>>, String> {
        if let Some(sftp) = self.sftp.as_ref() {
            return Ok(sftp.clone());
        }
        let channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|error| format!("Failed to open SFTP channel: {error}"))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|error| format!("Failed to start SFTP: {error}"))?;
        let sftp = Arc::new(AsyncMutex::new(
            SftpSession::new(channel.into_stream())
                .await
                .map_err(|error| format!("Failed to start SFTP: {error}"))?,
        ));
        self.sftp = Some(sftp.clone());
        Ok(sftp)
    }
}

pub struct McpState {
    runtime: Arc<SshRuntime>,
    connections: AsyncRwLock<HashMap<String, McpConnection>>,
    /// StoredConnection payloads registered through `mcp/call` (DBX
    /// connections), kept so a dropped pooled handle can be reconnected
    /// without asking the caller for credentials again.
    dbx_connections: AsyncRwLock<HashMap<String, StoredConnection>>,
    /// MCP size preferences, mirrored to `limits_path` on every change.
    limits: RwLock<McpLimits>,
    limits_path: PathBuf,
    /// Operator-level kill switch: `DBX_SSH_MCP_READ_ONLY` forces every
    /// tool call (bridge and standalone alike) through the read-only gates.
    global_read_only: bool,
    /// L1 stdio bridge fallback switch: forwards unregistered-`connectionId`
    /// calls to the running DBX app through the local TCP bridge. Always on
    /// in production; tests flip it off to keep decision paths hermetic (no
    /// real app bridge on the box).
    bridge_fallback: bool,
}

/// Truthy values accepted for `DBX_SSH_MCP_READ_ONLY`.
fn env_read_only() -> bool {
    matches!(
        std::env::var("DBX_SSH_MCP_READ_ONLY").ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "on")
    )
}

impl McpState {
    pub fn new(data_dir: PathBuf) -> Self {
        let limits_path = data_dir.join("mcp-settings.json");
        let limits = McpLimits::load(&limits_path);
        Self {
            runtime: Arc::new(SshRuntime::new(data_dir).with_auto_trust_keys()),
            connections: AsyncRwLock::new(HashMap::new()),
            dbx_connections: AsyncRwLock::new(HashMap::new()),
            limits: RwLock::new(limits),
            limits_path,
            global_read_only: env_read_only(),
            bridge_fallback: true,
        }
    }

    /// Wraps the workbench sidecar's shared runtime so `mcp/call` reuses the
    /// plugin's connection registry, known_hosts store, and settings.
    pub fn shared(runtime: Arc<SshRuntime>) -> Self {
        let limits_path = runtime.data_dir().join("mcp-settings.json");
        let limits = McpLimits::load(&limits_path);
        Self {
            runtime,
            connections: AsyncRwLock::new(HashMap::new()),
            dbx_connections: AsyncRwLock::new(HashMap::new()),
            limits: RwLock::new(limits),
            limits_path,
            global_read_only: env_read_only(),
            bridge_fallback: true,
        }
    }

    /// Snapshot of the MCP size preferences (already clamped to ceilings).
    fn size_limits(&self) -> McpLimits {
        self.limits
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    /// `mcp/settings/get`: the effective MCP size preferences.
    pub fn settings_get(&self) -> Value {
        self.size_limits().to_json()
    }

    /// `mcp/settings/set`: partial update with per-field validation
    /// (unsigned integers in `1..=ceiling` only), persisted to
    /// `mcp-settings.json`; returns the complete updated object.
    pub fn settings_set(&self, updates: &Value) -> Result<Value, String> {
        let mut limits = self.size_limits();
        if let Some(value) = updates.get("maxReadBytes") {
            limits.max_read_bytes = validated_limit(value, "maxReadBytes", READ_LIMIT_CEILING)?;
        }
        if let Some(value) = updates.get("maxUploadBytes") {
            limits.max_upload_bytes =
                validated_limit(value, "maxUploadBytes", UPLOAD_LIMIT_CEILING)?;
        }
        if let Some(value) = updates.get("maxDownloadBytes") {
            limits.max_download_bytes =
                validated_limit(value, "maxDownloadBytes", DOWNLOAD_LIMIT_CEILING)?;
        }
        if let Some(value) = updates.get("localTransferRoot") {
            limits.local_transfer_root = validated_transfer_root(value)?;
        }
        limits = limits.sanitized();
        limits.clone().save(&self.limits_path)?;
        *self
            .limits
            .write()
            .unwrap_or_else(|poison| poison.into_inner()) = limits;
        Ok(self.settings_get())
    }

    async fn dispatch(&self, request: Value) -> Option<Value> {
        let id = request.get("id").cloned();
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        if method.starts_with("notifications/") {
            return None;
        }
        let result = match method.as_str() {
            "initialize" => Ok(json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": {
                    "name": "dbx-ssh",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tool_definitions() })),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
                // stdio mode has no event emitter: `runInTerminal` routing
                // goes through the DBX app's local TCP bridge inside
                // `ssh_exec_tool` (the None-emitter arm).
                self.call_tool(&name, &arguments, None).await
            }
            other => Err(format!("Method not found: {other}")),
        };
        Some(match (id, result) {
            (Some(id), Ok(result)) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            (Some(id), Err(message)) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32000, "message": message },
            }),
            // A request without an id is invalid JSON-RPC; reply with a null id.
            (None, result) => json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": { "code": -32600, "message": result.err().unwrap_or_else(|| "Invalid request".to_string()) },
            }),
        })
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: &Value,
        emitter: Option<&PluginEmitter>,
    ) -> Result<Value, String> {
        // Registry reference validation first: an ambiguous connectionName
        // reports its candidates (id + host) here, before any coarser gate
        // can mask the disambiguation error.
        self.registered_connection_by_ref(arguments).await?;
        // Safety gates, ordered cheapest-first and all evaluated before any
        // network I/O:
        // 1. Read-only gate: write-class tools are rejected when the DBX
        //    connection they reference was opened read-only (mirroring the
        //    workbench `ensure_writable` gate) or the operator forced the
        //    whole server read-only via DBX_SSH_MCP_READ_ONLY.
        // 2. Read-only command whitelist: `ssh_exec` stays available on
        //    read-only connections, but only for provably read-only
        //    commands (ls, df, systemctl status, ...).
        // 3. Sensitive-path denylist: on read-only connections even
        //    whitelisted inspection tools may not touch credential/key paths
        //    (~/.ssh, /etc/shadow, .env, *.pem, ...) — the exfiltration
        //    channel the command whitelist cannot see for SFTP tools.
        // 4. Destructive-command confirmation: recognized catastrophic
        //    patterns require an explicit confirmDestructive: true on every
        //    connection (and are refused outright on read-only ones).
        let read_only = self.connection_is_read_only(arguments).await;
        if is_write_tool(name) && read_only {
            return Err(format!(
                "Tool {name} is a write operation and the connection is read-only"
            ));
        }
        if read_only {
            if let Some(path) = sensitive_read_path(name, arguments) {
                return Err(format!(
                    "Tool {name} reads the sensitive path {path}; refused on read-only \
                     connections"
                ));
            }
        }
        if matches!(name, "ssh_exec" | "ssh_exec_sudo" | "ssh_run_bg") {
            let command = required_str(arguments, "command")?;
            // Per-connection sudoers-style allowlist: privileged commands
            // must match a `sudo_whitelist` entry when the connection
            // declares one. Covers `ssh_exec_sudo` plus inline `sudo …` in
            // `ssh_exec`/`ssh_run_bg` (NOPASSWD / cached-stamp bypasses).
            if name == "ssh_exec_sudo" || mcp_safety::runs_under_sudo(command) {
                let allowlist = self.sudo_allowlist_for(arguments).await?;
                if !allowlist.is_empty() && !sudo_allowlist::is_allowed(&allowlist, command) {
                    return Err(format!(
                        "sudo command is not allowed by this connection's whitelist. \
                         Allowed patterns: {}",
                        sudo_allowlist::render_entries(&allowlist)
                    ));
                }
            }
            match mcp_safety::assess_command(command) {
                CommandRisk::Destructive(reason) if read_only => {
                    return Err(format!(
                        "Refused on read-only connection ({reason}): {command}"
                    ));
                }
                CommandRisk::Destructive(reason) => {
                    let confirmed = arguments
                        .get("confirmDestructive")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if !confirmed {
                        return Err(format!(
                            "Command looks destructive ({reason}): {command}. \
                             Retry with confirmDestructive: true if this is intended."
                        ));
                    }
                }
                CommandRisk::Unknown if name == "ssh_exec" && read_only => {
                    return Err(format!(
                        "Connection is read-only and the command is not recognized \
                         as read-only: {command}. Only inspection commands (ls, cat, \
                         df, ps, systemctl status, journalctl, docker ps, ...) pass."
                    ));
                }
                _ => {}
            }
        }
        // L1 stdio bridge fallback: a call referencing a `connectionId` that
        // is NOT in this session's lifecycle registry (the normal state of a
        // standalone `--mcp` session) is forwarded to the running DBX app's
        // own sidecar through the local TCP bridge, so saved connections work
        // without inline credentials and credentials never travel in tool
        // arguments. Gate semantics across the forward: the local gates above
        // stay in front of it (the DBX_SSH_MCP_READ_ONLY kill switch and the
        // command-text checks are arguments-only and must not be dodged),
        // while the registry-metadata gates (per-connection read-only flag,
        // sudo whitelist) cannot resolve here by definition — the app-side
        // sidecar enforces them against its own lifecycle registration.
        if emitter.is_none() {
            if let Some((connection_id, forwarded)) =
                self.bridge_forward_plan(name, arguments).await?
            {
                if let Ok(result) = self
                    .forward_tool_via_bridge(name, &connection_id, &forwarded)
                    .await
                {
                    return Ok(result);
                }
                // Bridge unreachable or refused (app down, unwakeable, older
                // app): fall through to the inline path so the original
                // guidance error — now carrying the self-heal hints — is
                // what the caller sees.
            }
        }
        let text = self.run_tool(name, arguments, emitter).await?;
        // The app-bridge forward returns the app's MCP content envelope
        // verbatim (`app_bridge::call_plugin_tool`); wrapping again would
        // bury the app's answer one JSON level deeper, so an already
        // enveloped result passes through untouched.
        if text.get("content").is_some() && text.get("isError").is_some() {
            return Ok(text);
        }
        Ok(json!({
            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&text).unwrap_or_default() }],
            "isError": false,
        }))
    }

    /// True when the call must pass the read-only gates: either the DBX
    /// connection it references was registered read-only, or the operator
    /// flipped the process-wide `DBX_SSH_MCP_READ_ONLY` kill switch.
    async fn connection_is_read_only(&self, arguments: &Value) -> bool {
        self.global_read_only || self.registered_connection_is_read_only(arguments).await
    }

    /// Unified lifecycle-registry lookup for a call's connection reference:
    /// `connectionId` wins (exact registry id); `connectionName` is the
    /// fallback (exact, trimmed — same convention as the Quick Sudo profile
    /// reference). `Ok(None)` = no reference resolved (the callers keep
    /// their existing inline fallbacks); `Err` = ambiguous name, listing
    /// every candidate id + host so the caller can disambiguate by id.
    async fn registered_connection_by_ref(
        &self,
        arguments: &Value,
    ) -> Result<Option<StoredConnection>, String> {
        let id = arguments
            .get("connectionId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        let name = arguments
            .get("connectionName")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let registry = self.dbx_connections.read().await;
        if let Some(id) = id {
            if let Some(connection) = registry.get(id) {
                return Ok(Some(connection.clone()));
            }
            // A stale id alongside a usable name falls through to the name
            // match; without a name the miss stays a miss (unchanged
            // behavior — the caller raises its own guidance error).
            if name.is_none() {
                return Ok(None);
            }
        }
        let Some(name) = name else {
            return Ok(None);
        };
        let candidates: Vec<&StoredConnection> = registry
            .values()
            .filter(|connection| connection.name.as_deref() == Some(name))
            .collect();
        match candidates.len() {
            1 => Ok(Some(candidates[0].clone())),
            0 => Ok(None),
            _ => Err(format!(
                "Connection name '{name}' is ambiguous: {}; pass connectionId instead",
                render_connection_candidates(&candidates)
            )),
        }
    }

    /// True when the arguments reference a DBX-registered connection that was
    /// registered as read-only through `mcp/call` lifecycle payloads.
    ///
    /// Without a resolvable reference the gate falls back to endpoint
    /// identity (host + port + username): keying read-only by connectionId
    /// alone would let a caller walk around a read-only registration by
    /// re-dialing the same host with inline credentials.
    async fn registered_connection_is_read_only(&self, arguments: &Value) -> bool {
        match self.registered_connection_by_ref(arguments).await {
            Ok(Some(connection)) => connection.read_only,
            // Ambiguous connectionName: conservatively read-only; the
            // disambiguation error with every candidate surfaces from the
            // execution path.
            Err(_) => true,
            Ok(None) => self.inline_dial_is_registered_read_only(arguments).await,
        }
    }

    /// True when the inline dial arguments (host/port/username) identify the
    /// same endpoint as a DBX-registered read-only connection.
    async fn inline_dial_is_registered_read_only(&self, arguments: &Value) -> bool {
        self.registered_connection_matching_inline(arguments)
            .await
            .is_some_and(|connection| connection.read_only)
    }

    /// Finds a lifecycle-registered connection whose endpoint identity
    /// (host + port + username) matches the inline dial arguments. The port
    /// defaults to 22 on both sides; host comparison is ASCII-case-insensitive.
    async fn registered_connection_matching_inline(
        &self,
        arguments: &Value,
    ) -> Option<StoredConnection> {
        let host = arguments
            .get("host")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        let port = arguments
            .get("port")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(22);
        let username = arguments
            .get("username")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        self.dbx_connections
            .read()
            .await
            .values()
            .find(|connection| {
                connection.host.trim().eq_ignore_ascii_case(host)
                    && connection.port == port
                    && connection.username == username
            })
            .cloned()
    }

    /// Resolves the connection's sudoers-style allowlist: by `connectionId`
    /// (or `connectionName`) through the lifecycle registry, else by inline
    /// endpoint identity (the same fallback as the read-only gate, so
    /// re-dialing a whitelisted host with inline credentials cannot skip the
    /// list). Empty = not configured. An ambiguous connectionName is an
    /// error, not a silent gate-off.
    async fn sudo_allowlist_for(&self, arguments: &Value) -> Result<Vec<Vec<String>>, String> {
        Ok(match self.registered_connection_by_ref(arguments).await? {
            Some(connection) => (!connection.sudo_whitelist.is_empty())
                .then(|| sudo_allowlist::entries_from_lines(&connection.sudo_whitelist))
                .unwrap_or_default(),
            None => self
                .registered_connection_matching_inline(arguments)
                .await
                .and_then(|connection| {
                    (!connection.sudo_whitelist.is_empty())
                        .then(|| sudo_allowlist::entries_from_lines(&connection.sudo_whitelist))
                })
                .unwrap_or_default(),
        })
    }

    /// L1 stdio bridge fallback decision: `Ok(Some((connection_id,
    /// arguments)))` when the call must be forwarded through the DBX app
    /// bridge, `Ok(None)` when the local path owns the call (registered
    /// reference, no connection reference, local-only tool, runInTerminal
    /// routing, or a name the bridge list cannot resolve — the inline path
    /// then raises its own guidance error), `Err` for an actionable
    /// disambiguation failure from the bridge list.
    async fn bridge_forward_plan(
        &self,
        name: &str,
        arguments: &Value,
    ) -> Result<Option<(String, Value)>, String> {
        if !self.bridge_fallback || !is_connection_bound_tool(name) {
            return Ok(None);
        }
        // runInTerminal routing already owns its own bridge forward
        // (`ssh_exec_app_bridge`), which never required a registry entry;
        // keep that path exclusive so a down bridge cannot stack two
        // sequential wake-and-wait attempts.
        if arguments.get("runInTerminal").and_then(Value::as_bool) == Some(true) {
            return Ok(None);
        }
        if let Some(id) = arguments
            .get("connectionId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            let registered = self.dbx_connections.read().await.contains_key(id);
            // Registered: the local path runs unchanged (embedded-bridge
            // semantics). Unregistered: forward as-is — the app-side sidecar
            // holds the connection's credentials, read-only flag, and sudo
            // whitelist, and answers with its own MCP envelope.
            return Ok((!registered).then(|| (id.to_string(), arguments.clone())));
        }
        let Some(name_ref) = arguments
            .get("connectionName")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(None);
        };
        // connectionName without connectionId: a registry hit (unique or
        // ambiguous) stays local — `connection()` resolves it through the
        // same registry and reports ambiguity itself. Only a stdio session
        // with no registry entry needs the bridge list to resolve
        // name → id before forwarding.
        match self.registered_connection_by_ref(arguments).await {
            Ok(Some(_)) | Err(_) => Ok(None),
            Ok(None) => match self.resolve_connection_name_via_bridge(name_ref).await {
                Ok(id) => {
                    let mut forwarded = arguments.clone();
                    if let Some(map) = forwarded.as_object_mut() {
                        map.insert("connectionId".to_string(), json!(id));
                    }
                    Ok(Some((id, forwarded)))
                }
                // Bridge list unavailable or the name is not in it: fall
                // through so the inline path reports (for an unavailable
                // bridge) or the resolution error above already did (for a
                // bad name).
                Err(_) => Ok(None),
            },
        }
    }

    /// stdio connectionName resolution: reads the bridge list (no app
    /// wake-up — a down bridge simply fails the resolution and the call
    /// degrades) and matches by exact trimmed name.
    async fn resolve_connection_name_via_bridge(&self, name: &str) -> Result<String, String> {
        let entries = app_bridge::list_plugin_connections().await?;
        resolve_name_in_bridge_list(&entries, name)
    }

    /// Forwards one connection-bound tool call through the DBX app bridge
    /// (L1). Same wake-and-verify contract as `ssh_exec_app_bridge`; the
    /// tool timeout follows the `timeoutSecs` argument with the same 5–300s
    /// clamp ssh_exec applies. The 200 body is the app's MCP content
    /// envelope and is returned verbatim.
    async fn forward_tool_via_bridge(
        &self,
        name: &str,
        connection_id: &str,
        arguments: &Value,
    ) -> Result<Value, String> {
        app_bridge::ensure_app_bridge(app_bridge::DEFAULT_ENSURE_WAIT).await?;
        let timeout_secs = arguments
            .get("timeoutSecs")
            .and_then(Value::as_u64)
            .map(|secs| secs.clamp(5, 300))
            .unwrap_or(300);
        app_bridge::call_plugin_tool(
            connection_id,
            name,
            arguments.clone(),
            Duration::from_secs(timeout_secs),
        )
        .await
    }

    /// `ssh_list_connections`: saved-connection discovery for stdio agents.
    /// The bridge list comes first (metadata only — the app never returns
    /// credentials), merged with this session's lifecycle registry
    /// (deduplicated by id; registry entries may add credential-presence
    /// flags because they carry the secrets). A bridge that answers nothing
    /// (app down, or an older app without the /list-plugin-connections
    /// route) degrades to the registry alone plus an upgrade note instead of
    /// failing.
    async fn ssh_list_connections_tool(&self) -> Result<Value, String> {
        let registry: Vec<StoredConnection> = self
            .dbx_connections
            .read()
            .await
            .values()
            .cloned()
            .collect();
        let bridge = app_bridge::list_plugin_connections().await;
        Ok(connection_list_result(bridge, &registry))
    }

    async fn run_tool(
        &self,
        name: &str,
        arguments: &Value,
        emitter: Option<&PluginEmitter>,
    ) -> Result<Value, String> {
        match name {
            "ssh_close" => self.ssh_close(arguments).await,
            // Local↔remote transfers validate their local side before any
            // connection I/O; validation refusals must keep the pooled
            // connection (they are not transport failures), so these tools
            // own their drop-on-transport-error semantics instead of the
            // blanket cleanup below.
            "sftp_upload" => self.sftp_upload_tool(arguments).await,
            "sftp_download" => self.sftp_download_tool(arguments).await,
            // Global Quick Sudo profile management: local config reads and
            // writes through the shared runtime store (not remote writes, so
            // these stay outside the read-only tool gate).
            "ssh_quick_sudo_profiles_list" => Ok(self.runtime.profiles_list()),
            "ssh_quick_sudo_profiles_save" => self.runtime.profiles_save(arguments).await,
            "ssh_quick_sudo_profiles_delete" => {
                let id = required_str(arguments, "id")?;
                self.runtime.profiles_delete(id).await
            }
            "ssh_test_connection" => {
                let connection = stored_connection_from_arguments(arguments)?;
                let started = std::time::Instant::now();
                let (handle, jumps) = self.runtime.connect_headless(&connection).await?;
                let latency_ms = started.elapsed().as_millis() as u64;
                let _ = handle
                    .disconnect(
                        russh::Disconnect::ByApplication,
                        "MCP connection test complete",
                        "English",
                    )
                    .await;
                for jump in jumps {
                    let _ = jump
                        .disconnect(
                            russh::Disconnect::ByApplication,
                            "MCP jump connection closed",
                            "English",
                        )
                        .await;
                }
                Ok(json!({
                    "ok": true,
                    "host": connection.host,
                    "port": connection.port,
                    "username": connection.username,
                    "latencyMs": latency_ms,
                }))
            }
            "ssh_list_known_hosts" => {
                let verifier = HostKeyVerifier::new(self.runtime.known_hosts_path());
                let entries: Vec<Value> = verifier
                    .list_known_hosts()?
                    .into_iter()
                    .map(|entry| {
                        json!({
                            "hostField": entry.host_field,
                            "keyType": entry.key_type,
                            "fingerprint": entry.fingerprint,
                            "comment": entry.comment,
                        })
                    })
                    .collect();
                Ok(json!({ "knownHosts": entries }))
            }
            "ssh_list_connections" => self.ssh_list_connections_tool().await,
            "ssh_remove_known_host" => {
                let host = required_str(arguments, "host")?;
                let port = arguments
                    .get("port")
                    .and_then(Value::as_u64)
                    .and_then(|value| u16::try_from(value).ok())
                    .filter(|value| *value > 0)
                    .unwrap_or(22);
                let verifier = HostKeyVerifier::new(self.runtime.known_hosts_path());
                let removed = verifier.remove_known_host(host, port)?;
                Ok(json!({ "host": host, "port": port, "removed": removed }))
            }
            _ => {
                let mut result = match name {
                    "ssh_exec" | "ssh_exec_sudo" => {
                        self.ssh_exec_tool(name, arguments, emitter).await
                    }
                    "ssh_run_bg" => self.ssh_run_bg_tool(arguments).await,
                    "ssh_task_status" => self.ssh_task_status_tool(arguments).await,
                    "ssh_metrics" => {
                        let connection = self.connection(arguments).await?;
                        exec::collect_metrics(&connection).await
                    }
                    "sftp_disk_usage" => {
                        let path = required_str(arguments, "path")?;
                        let connection = self.connection(arguments).await?;
                        let command = format!("df -kP {}", exec::shell_quote(&path));
                        // Plugin-internal plumbing: no client setEnv, keeping
                        // the df output parseable regardless of locale overrides.
                        let outcome =
                            exec::exec_plain(&connection, &command, Duration::from_secs(20), &[])
                                .await?;
                        exec::parse_disk_usage(&outcome.output).ok_or_else(|| {
                            format!("Could not parse disk usage: {}", outcome.output)
                        })
                    }
                    other => self.sftp_tool(other, arguments).await,
                };
                if result.is_err() {
                    // Drop the cached connection on failure so the next call
                    // reconnects with fresh credentials instead of reusing a
                    // broken transport.
                    self.drop_connection(arguments).await;
                    // Flapping-network recovery: when the pooled transport
                    // died BEFORE the remote command could start (channel
                    // open/exec refused on a dead connection), retry once on
                    // the fresh connection. Pre-exec failures cannot
                    // double-execute the command; post-exec transport deaths
                    // stay terminal because the command may already have run.
                    if matches!(name, "ssh_exec" | "ssh_exec_sudo" | "ssh_run_bg") {
                        let retryable = result
                            .as_ref()
                            .err()
                            .map(|error| exec::is_pre_exec_transport_error(error))
                            .unwrap_or(false);
                        if retryable {
                            result = match name {
                                "ssh_exec" | "ssh_exec_sudo" => {
                                    self.ssh_exec_tool(name, arguments, emitter).await
                                }
                                _ => self.ssh_run_bg_tool(arguments).await,
                            };
                        }
                    }
                }
                result
            }
        }
    }

    async fn ssh_exec_tool(
        &self,
        name: &str,
        arguments: &Value,
        emitter: Option<&PluginEmitter>,
    ) -> Result<Value, String> {
        let command = required_str(arguments, "command")?;
        let timeout_secs = arguments
            .get("timeoutSecs")
            .and_then(Value::as_u64)
            .map(|secs| Duration::from_secs(secs.clamp(5, 300)));
        // A quickSudoProfile reference is resolved (and validated) before any
        // connection I/O so unknown ids/names fail fast.
        let sudo_profile = if name == "ssh_exec_sudo" {
            let store = sudo_profiles::load_store(&self.runtime.data_dir());
            resolve_profile_reference(&store, arguments)?
        } else {
            None
        };
        // Agent terminal routing (agent terminal mode plan §1): only the DBX
        // embedded bridge carries an emitter plus a lifecycle connectionId;
        // every other caller stays on the hidden exec channel below.
        let run_in_terminal = arguments
            .get("runInTerminal")
            .and_then(Value::as_bool);
        let connection_id = arguments
            .get("connectionId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        match (emitter, connection_id) {
            (Some(emitter), Some(connection_id)) => {
                let mode = self.runtime.agent_terminal_mode(connection_id);
                // An explicit runInTerminal wins; the connection mode decides
                // when it is absent. Off keeps the existing path untouched.
                let route = run_in_terminal.unwrap_or(mode != AgentTerminalMode::Off);
                if route {
                    return self
                        .ssh_exec_terminal_tool(
                            name,
                            command,
                            connection_id,
                            timeout_secs,
                            emitter,
                        )
                        .await;
                }
            }
            (None, _) if run_in_terminal == Some(true) => {
                // stdio mode forwards through the DBX app's local TCP bridge:
                // the app opens the connection's workbench tab and runs the
                // tool on its own sidecar, so the command lands in the app's
                // visible terminal.
                let Some(connection_id) = connection_id else {
                    return Err(
                        "runInTerminal needs a saved DBX connection: pass connectionId (the connection must exist in the DBX app) so the command can run in the app's visible terminal. Saved connection ids can be listed with ssh_list_connections."
                            .to_string(),
                    );
                };
                return self
                    .ssh_exec_app_bridge(name, connection_id, arguments, timeout_secs)
                    .await;
            }
            (Some(_), None) if run_in_terminal == Some(true) => {
                return Err(
                    "runInTerminal requires a lifecycle connectionId from the DBX embedded bridge. Saved connection ids can be listed with ssh_list_connections."
                        .to_string(),
                );
            }
            _ => {}
        }
        // Saved DBX connections declare their Quick Sudo source (form field
        // sudo_source); the hidden exec channel honors it exactly like the
        // workbench instead of requiring inline credentials on every call.
        // The unified lookup also honors connectionName; an ambiguous name
        // degrades to None here (the dial below reports it).
        let stored = self
            .registered_connection_by_ref(arguments)
            .await
            .ok()
            .flatten();
        // The hidden exec channel carries the saved connection's setEnv too,
        // matching the workbench exec path.
        let set_env = stored
            .as_ref()
            .map(|connection| connection.set_env.clone())
            .unwrap_or_default();
        if name == "ssh_exec_sudo" {
            let auth = self
                .resolve_sudo_auth(arguments, sudo_profile, stored)
                .await?;
            // The hidden exec channel stays TTY-less on purpose: with a PTY,
            // each PAM factor read flushes typed-ahead input, so piped
            // credentials race an unknowable per-host timing; without one,
            // stdin is a plain pipe and `exec_with_sudo` queues password and
            // OTP deterministically.
            let connection = self.connection(arguments).await?;
            exec::exec_with_sudo(
                &connection,
                &auth,
                command,
                timeout_secs.unwrap_or(exec::SUDO_EXEC_TIMEOUT),
                false,
                &set_env,
            )
            .await
            .map(|outcome| json!({ "output": outcome.output, "exitCode": outcome.exit_code }))
        } else {
            let connection = self.connection(arguments).await?;
            exec::exec_plain(
                &connection,
                command,
                timeout_secs.unwrap_or(exec::PLAIN_EXEC_TIMEOUT),
                &set_env,
            )
            .await
            .map(|outcome| json!({ "output": outcome.output, "exitCode": outcome.exit_code }))
        }
    }

    /// `ssh_run_bg`: stages a long-running command detached on the remote
    /// host (nohup, output appended to `/tmp/.dbx-ssh-tasks/<taskId>.log`)
    /// and returns immediately. The server-side log file is the task's
    /// durable record: status polling reattaches over a fresh connection, so
    /// flapping networks, disconnects, and MCP-host wait caps never lose
    /// output or kill the job.
    async fn ssh_run_bg_tool(&self, arguments: &Value) -> Result<Value, String> {
        let command = required_str(arguments, "command")?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        let task_id = format!("bg-{stamp}-{}", std::process::id() % 100_000);
        // Single-quote escape for embedding inside the remote `sh -c '...'`.
        let escaped = command.replace('\'', "'\\''");
        let remote = format!(
            "d=/tmp/.dbx-ssh-tasks; mkdir -p \"$d\" || exit 3; f=\"$d/{task_id}.log\"; : > \"$f\"; \
             nohup sh -c '{escaped}; s=$?; echo EXIT_$s' >> \"$f\" 2>&1 & p=$!; \
             echo \"$p\" > \"$f.pid\"; echo \"PID=$p\"; echo \"LOG=$f\""
        );
        let connection = self.connection(arguments).await?;
        // Internal staging wrapper: env-free so the wrapper's own output
        // stays parseable; the user command inherits the server defaults.
        let outcome = exec::exec_plain(&connection, &remote, Duration::from_secs(15), &[]).await?;
        if outcome.exit_code != 0 {
            return Err(format!(
                "Failed to stage background task (exit {}): {}",
                outcome.exit_code, outcome.output
            ));
        }
        let default_log = format!("/tmp/.dbx-ssh-tasks/{task_id}.log");
        let (pid, log_path) = parse_bg_start_output(&outcome.output, default_log);
        Ok(json!({
            "taskId": task_id,
            "pid": pid,
            "logPath": log_path,
            "pollWith": "ssh_task_status",
            "note": "Running detached (nohup). Poll ssh_task_status(logPath); output survives disconnects and session restarts.",
        }))
    }

    /// `ssh_task_status`: polls a task started by `ssh_run_bg` through its
    /// server-side log file. Works across disconnects and from later
    /// sessions because the log lives on the remote host, not in this MCP
    /// session.
    async fn ssh_task_status_tool(&self, arguments: &Value) -> Result<Value, String> {
        let log_path = required_str(arguments, "logPath")?;
        let tail_bytes = arguments
            .get("tailBytes")
            .and_then(Value::as_u64)
            .unwrap_or(4_000)
            .clamp(200, 16_000);
        let quoted = exec::shell_quote(log_path);
        let remote = format!(
            "f={quoted}; if [ ! -f \"$f\" ]; then echo STATE=MISSING; exit 0; fi; \
             if grep -q '^EXIT_[0-9][0-9]*$' \"$f\" 2>/dev/null; then \
               echo STATE=DONE; echo CODE=$(grep -o '^EXIT_[0-9][0-9]*$' \"$f\" | tail -1); \
             else echo STATE=RUNNING; fi; \
             pf=\"$f.pid\"; if [ -f \"$pf\" ]; then p=$(cat \"$pf\"); \
               kill -0 \"$p\" 2>/dev/null && echo PID_ALIVE=yes || echo PID_ALIVE=no; fi; \
             echo ===TAIL===; tail -c {tail_bytes} \"$f\""
        );
        let connection = self.connection(arguments).await?;
        let outcome = exec::exec_plain(&connection, &remote, Duration::from_secs(15), &[]).await?;
        if outcome.exit_code != 0 {
            return Err(format!(
                "Failed to read task status (exit {}): {}",
                outcome.exit_code, outcome.output
            ));
        }
        let (state, exit_code, pid_alive, tail) = parse_task_status_output(&outcome.output);
        Ok(json!({
            "state": state,
            "exitCode": exit_code,
            "pidAlive": pid_alive,
            "output": tail.trim_end(),
            "logPath": log_path,
            "done": state == "done",
        }))
    }

    /// `ssh_exec_sudo` credential resolution for the hidden exec channel:
    /// explicit per-call arguments always win; otherwise a saved DBX
    /// connection's declared sudo source applies (Global: its form profile
    /// reference / workbench binding, Custom: the connection's secret-bound
    /// sudo configuration, Off: refuse unless the caller passed explicit
    /// credentials), mirroring the workbench exec gate; the per-call
    /// `quickSudoProfile` reference replaces the connection's declared
    /// global profile.
    async fn resolve_sudo_auth(
        &self,
        arguments: &Value,
        explicit_profile: Option<sudo_profiles::SudoProfile>,
        stored: Option<StoredConnection>,
    ) -> Result<SudoAuth, String> {
        let has_explicit_credentials = ["sudoPassword", "totpSecret"].iter().any(|key| {
            arguments
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .is_some_and(|value| !value.is_empty())
        });
        if let Some(stored) = &stored {
            if stored.sudo_source == SudoSource::Off && !has_explicit_credentials {
                return Err("Quick Sudo is disabled for this connection".to_string());
            }
        }
        let store = sudo_profiles::load_store(&self.runtime.data_dir());
        let profile = match explicit_profile {
            Some(profile) => Some(profile),
            None => stored
                .as_ref()
                .and_then(|stored| crate::ssh::effective_sudo_profile(stored, &store)),
        };
        let mut auth = match (&stored, &profile) {
            (Some(stored), profile) => crate::ssh::resolved_sudo_auth(stored, profile.as_ref()),
            (None, Some(profile)) => {
                let mut auth = sudo_auth(arguments);
                apply_profile_fallbacks(&mut auth, profile);
                auth
            }
            (None, None) => sudo_auth(arguments),
        };
        apply_explicit_argument_overrides(&mut auth, arguments);
        Ok(auth)
    }

    /// Terminal-routed `ssh_exec` / `ssh_exec_sudo`: resolves the
    /// connection's live workbench PTY, applies the §1 approval matrix
    /// (sudo and catastrophic mcp_safety hits are elevated), then types the
    /// command into the terminal and captures the output.
    /// Polls `session_id_for_connection` until the workbench PTY shows up or
    /// `wait` elapses, so a just-triggered workbench auto-open can win the
    /// race against the first terminal-routed command.
    async fn wait_for_connection_session(
        &self,
        connection_id: &str,
        wait: Duration,
    ) -> Result<String, String> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            match self.runtime.session_id_for_connection(connection_id).await {
                Ok(session_id) => return Ok(session_id),
                Err(error) => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(error);
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn ssh_exec_terminal_tool(
        &self,
        name: &str,
        command: &str,
        connection_id: &str,
        timeout_secs: Option<Duration>,
        emitter: &PluginEmitter,
    ) -> Result<Value, String> {
        let mode = self.runtime.agent_terminal_mode(connection_id);
        let risk = if name == "ssh_exec_sudo" {
            // Sudo runs through the user's terminal where the auto-sudo
            // state machine or a human answers the password prompt.
            agent_terminal::CommandRisk::Elevated
        } else if mcp_safety::runs_under_sudo(command) {
            // An inline `sudo …` is privilege escalation too, even when the
            // inner verb is harmless: teaching mode must approve it.
            agent_terminal::CommandRisk::Elevated
        } else {
            match mcp_safety::assess_command(command) {
                mcp_safety::CommandRisk::Destructive(_) => {
                    agent_terminal::CommandRisk::Elevated
                }
                _ => agent_terminal::CommandRisk::Low,
            }
        };
        let timeout = timeout_secs.map(|duration| duration.as_secs());
        // `ssh_exec_sudo` must type its `sudo` prefix into the terminal or
        // the escalation silently disappears (this path has no separate sudo
        // orchestration — the terminal's auto-sudo state machine answers the
        // prompt the prefixed command raises). Idempotent, so a prefix kept
        // through the approval dialog round-trips unchanged; an inline
        // `sudo …` from `ssh_exec` already carries it and stays untouched.
        // Shadowing here also puts the prefixed text on the approval prompt,
        // so what the user approves is exactly what gets typed.
        let command = if name == "ssh_exec_sudo" {
            agent_terminal::sudo_command_text(command)
        } else {
            command.to_string()
        };
        // Both routing outcomes need the connection's live workbench PTY.
        // The DBX app bridge opens the workbench tab right before forwarding,
        // so the PTY session may take a moment to appear: poll briefly before
        // falling back to the guidance error.
        let session_id = self
            .wait_for_connection_session(connection_id, Duration::from_secs(20))
            .await
            .map_err(|_| NO_TERMINAL_SESSION_MESSAGE.to_string())?;
        // Serialize concurrent agent commands on the same session: the guard
        // is intentionally held across the approval wait and the whole run —
        // that IS the serialization, so a second command queues behind an
        // in-flight approval instead of interleaving keystrokes with it on
        // the shared PTY. The lock is per-session, so different connections
        // still execute in parallel (no global lock here on purpose).
        let _exec_guard = self.runtime.agent_exec_guard(&session_id).await?;
        match agent_terminal::decide(mode, risk) {
            agent_terminal::RoutingDecision::Run => {
                self.runtime
                    .exec_in_terminal(&session_id, name, &command, risk, timeout, emitter)
                    .await
            }
            agent_terminal::RoutingDecision::Prompt => {
                let approved = self
                    .runtime
                    .request_agent_approval(&session_id, name, &command, risk, None, emitter)
                    .await?;
                self.runtime
                    .exec_in_terminal(&session_id, name, &approved, risk, timeout, emitter)
                    .await
            }
            agent_terminal::RoutingDecision::Deny(reason) => Err(reason.to_string()),
        }
    }

    /// stdio-mode terminal forwarding: relays the exec tool call to the DBX
    /// app's local TCP bridge, which opens the connection's workbench tab and
    /// runs the tool on the app's own sidecar — the same process as the
    /// visible terminal. The 200 body is already MCP-content wrapped and is
    /// returned verbatim (`call_tool` passes it through untouched); failures
    /// keep the bridge error and add reconnect guidance.
    async fn ssh_exec_app_bridge(
        &self,
        name: &str,
        connection_id: &str,
        arguments: &Value,
        timeout_secs: Option<Duration>,
    ) -> Result<Value, String> {
        app_bridge::ensure_app_bridge(app_bridge::DEFAULT_ENSURE_WAIT).await?;
        let timeout = timeout_secs.unwrap_or(Duration::from_secs(300));
        app_bridge::call_plugin_tool(connection_id, name, arguments.clone(), timeout)
            .await
            .map_err(|error| format!("{error}. Open the connection in DBX and retry"))
    }

    async fn sftp_tool(&self, name: &str, arguments: &Value) -> Result<Value, String> {
        let mut guard = self.connections.write().await;
        let pool_id = connection_pool_id(arguments);
        let entry = guard
            .get_mut(&pool_id)
            .ok_or("Connection is not established")?;
        match name {
            "sftp_list_dir" => {
                let path = required_str(arguments, "path")?;
                let sftp = entry.sftp().await?;
                let entries = sftp.lock().await.read_dir(path).await.map_err(sftp_error)?;
                let items: Vec<Value> = entries
                    .map(|entry| {
                        let metadata = entry.metadata();
                        json!({
                            "name": entry.file_name(),
                            "path": entry.path(),
                            "kind": match entry.file_type() {
                                FileType::Dir => "directory",
                                FileType::Symlink => "symlink",
                                FileType::File => "file",
                                FileType::Other => "other",
                            },
                            "size": metadata.size,
                            "modifiedAt": metadata.mtime,
                        })
                    })
                    .collect();
                Ok(json!({ "path": path, "entries": items }))
            }
            "sftp_stat" => {
                let path = required_str(arguments, "path")?;
                let sftp = entry.sftp().await?;
                let metadata = sftp.lock().await.metadata(path).await.map_err(sftp_error)?;
                Ok(json!({
                    "path": path,
                    "size": metadata.size,
                    "permissions": metadata.permissions.map(|bits| format!("{:04o}", bits & 0o7777)),
                    "modifiedAt": metadata.mtime,
                    "accessedAt": metadata.atime,
                    "uid": metadata.uid,
                    "gid": metadata.gid,
                }))
            }
            "sftp_exists" => {
                let path = required_str(arguments, "path")?;
                let sftp = entry.sftp().await?;
                let exists = sftp.lock().await.metadata(path).await.is_ok();
                Ok(json!({ "path": path, "exists": exists }))
            }
            "sftp_pwd" => {
                let sftp = entry.sftp().await?;
                let home = sftp
                    .lock()
                    .await
                    .canonicalize(".")
                    .await
                    .map_err(sftp_error)?;
                Ok(json!({ "home": home }))
            }
            "sftp_read_file" => {
                let path = required_str(arguments, "path")?;
                // Settings move the default and the ceiling (down freely, up
                // within the soft caps); the clamp itself stays as the
                // defensive hard limit against oversized requests.
                let limits = self.size_limits();
                let max_bytes = arguments
                    .get("maxBytes")
                    .and_then(Value::as_u64)
                    .unwrap_or(limits.max_read_bytes)
                    .clamp(1, limits.max_download_bytes);
                let as_base64 = arguments
                    .get("base64")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let offset = arguments.get("offset").and_then(Value::as_u64).unwrap_or(0);
                let sftp = entry.sftp().await?;
                let mut file = sftp.lock().await.open(path).await.map_err(sftp_error)?;
                if offset > 0 {
                    use tokio::io::AsyncSeekExt;
                    // SeekFrom::Start only moves the local read cursor; an
                    // offset at/after EOF simply yields no data.
                    file.seek(std::io::SeekFrom::Start(offset))
                        .await
                        .map_err(|error| format!("SFTP seek failed: {error}"))?;
                }
                let mut data = Vec::new();
                file.take(max_bytes.saturating_add(1))
                    .read_to_end(&mut data)
                    .await
                    .map_err(|error| format!("SFTP read failed: {error}"))?;
                let truncated = data.len() as u64 > max_bytes;
                data.truncate(max_bytes as usize);
                if as_base64 {
                    Ok(
                        json!({ "path": path, "dataBase64": BASE64_STANDARD.encode(&data), "truncated": truncated }),
                    )
                } else {
                    Ok(json!({
                        "path": path,
                        "content": String::from_utf8_lossy(&data),
                        "truncated": truncated,
                    }))
                }
            }
            "sftp_write_file" => {
                let path = required_str(arguments, "path")?;
                let content = required_str(arguments, "content")?;
                let upload_limit = self.size_limits().max_upload_bytes;
                if content.len() as u64 > upload_limit {
                    return Err(format!(
                        "Content of {} bytes exceeds the MCP upload limit of {upload_limit} bytes \
                         (adjust maxUploadBytes via mcp/settings/set)",
                        content.len()
                    ));
                }
                let overwrite = arguments
                    .get("overwrite")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let sftp = entry.sftp().await?;
                if !overwrite && sftp.lock().await.metadata(path).await.is_ok() {
                    return Err(format!(
                        "Remote path already exists: {path} (pass overwrite=true to replace)"
                    ));
                }
                let mut file = sftp.lock().await.create(path).await.map_err(sftp_error)?;
                tokio::io::AsyncWriteExt::write_all(&mut file, content.as_bytes())
                    .await
                    .map_err(|error| format!("SFTP write failed: {error}"))?;
                tokio::io::AsyncWriteExt::flush(&mut file)
                    .await
                    .map_err(|error| format!("SFTP write flush failed: {error}"))?;
                Ok(json!({ "path": path, "bytes": content.len() }))
            }
            "sftp_mkdir" => {
                let path = required_str(arguments, "path")?;
                let sftp = entry.sftp().await?;
                sftp.lock()
                    .await
                    .create_dir(path)
                    .await
                    .map_err(sftp_error)?;
                Ok(json!({ "path": path, "created": true }))
            }
            "sftp_remove" => {
                let path = required_str(arguments, "path")?;
                let recursive = arguments
                    .get("recursive")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let sftp = entry.sftp().await?;
                let metadata = sftp
                    .lock()
                    .await
                    .symlink_metadata(path)
                    .await
                    .map_err(sftp_error)?;
                if metadata.is_symlink() || !metadata.is_dir() {
                    sftp.lock()
                        .await
                        .remove_file(path)
                        .await
                        .map_err(sftp_error)?;
                } else if recursive {
                    remove_tree(&sftp, path.to_string()).await?;
                } else {
                    return Err(format!("{path} is a directory; pass recursive=true"));
                }
                Ok(json!({ "path": path, "removed": true }))
            }
            "sftp_rename" => {
                let source = required_str(arguments, "sourcePath")?;
                let target = required_str(arguments, "targetPath")?;
                let sftp = entry.sftp().await?;
                sftp.lock()
                    .await
                    .rename(source, target)
                    .await
                    .map_err(sftp_error)?;
                Ok(json!({ "sourcePath": source, "targetPath": target, "renamed": true }))
            }
            "sftp_chmod" => {
                let path = required_str(arguments, "path")?;
                let mode = arguments
                    .get("mode")
                    .and_then(|value| {
                        value
                            .as_str()
                            .and_then(|text| u32::from_str_radix(text, 8).ok())
                            .or_else(|| value.as_u64().and_then(|v| u32::try_from(v).ok()))
                    })
                    .filter(|value| *value <= 0o7777)
                    .ok_or("mode must be an octal value up to 7777")?;
                let sftp = entry.sftp().await?;
                let metadata = russh_sftp::protocol::FileAttributes {
                    permissions: Some(mode),
                    ..Default::default()
                };
                sftp.lock()
                    .await
                    .set_metadata(path, metadata)
                    .await
                    .map_err(sftp_error)?;
                Ok(json!({ "path": path, "mode": format!("{mode:04o}") }))
            }
            "sftp_copy" | "sftp_move" => {
                let op = if name == "sftp_move" {
                    sftp_copy::CopyOp::Move
                } else {
                    sftp_copy::CopyOp::Copy
                };
                let request = sftp_copy::parse_request(arguments)?;
                // The SFTP channel only enables the rename fast path; its
                // absence falls back to the shell for every item.
                let sftp = match entry.sftp().await {
                    Ok(sftp) => Some(sftp),
                    Err(error) => {
                        eprintln!(
                            "[ssh] MCP {name}: SFTP channel unavailable ({error}); shell fallback only"
                        );
                        None
                    }
                };
                Ok(sftp_copy::execute(&entry.handle, sftp, op, &request)
                    .await
                    .into_json())
            }
            other => Err(format!("Unknown tool: {other}")),
        }
    }

    /// Containment gate for agent-supplied local transfer paths; see
    /// [`ensure_local_transfer_allowed_in`] for the policy.
    fn ensure_local_transfer_allowed(&self, path: &Path) -> Result<(), String> {
        let configured = self.size_limits().local_transfer_root;
        let roots = local_transfer_roots_for(&configured, &self.runtime.data_dir())?;
        ensure_local_transfer_allowed_in(&roots, &configured, path)
    }

    /// `sftp_upload`: transfers one local file to the remote server. The
    /// local side is validated before any connection I/O (readable, inside
    /// the allowed transfer roots, clear of the sensitive-path blocklist,
    /// within the configured `maxUploadBytes`) so bad paths fail fast and
    /// refusals leave the pooled connection untouched. SFTP transport
    /// errors drop the cached connection so the next call reconnects.
    async fn sftp_upload_tool(&self, arguments: &Value) -> Result<Value, String> {
        let local_path = required_str(arguments, "localPath")?;
        let remote_path = required_str(arguments, "remotePath")?;
        let overwrite = arguments
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let local_source = std::fs::canonicalize(&local_path)
            .map_err(|error| format!("Cannot read local file {local_path}: {error}"))?;
        self.ensure_local_transfer_allowed(&local_source)?;
        let data = std::fs::read(&local_source)
            .map_err(|error| format!("Cannot read local file {}: {error}", local_source.display()))?;
        let upload_limit = self.size_limits().max_upload_bytes;
        if data.len() as u64 > upload_limit {
            return Err(format!(
                "Local file {local_path} is {} bytes and exceeds the MCP upload limit of \
                 {upload_limit} bytes (adjust maxUploadBytes via mcp/settings/set)",
                data.len()
            ));
        }
        let outcome = self
            .upload_via_sftp(arguments, local_path, remote_path, &data, overwrite)
            .await;
        if outcome.is_err() {
            self.drop_connection(arguments).await;
        }
        outcome
    }

    async fn upload_via_sftp(
        &self,
        arguments: &Value,
        local_path: &str,
        remote_path: &str,
        data: &[u8],
        overwrite: bool,
    ) -> Result<Value, String> {
        self.connection(arguments).await?;
        let mut guard = self.connections.write().await;
        let entry = guard
            .get_mut(&connection_pool_key(arguments))
            .ok_or("Connection is not established")?;
        let sftp = entry.sftp().await?;
        if !overwrite && sftp.lock().await.metadata(remote_path).await.is_ok() {
            return Err(format!(
                "Remote path already exists: {remote_path} (pass overwrite=true to replace)"
            ));
        }
        let mut file = sftp.lock().await.create(remote_path).await.map_err(sftp_error)?;
        tokio::io::AsyncWriteExt::write_all(&mut file, data)
            .await
            .map_err(|error| format!("SFTP write failed: {error}"))?;
        tokio::io::AsyncWriteExt::flush(&mut file)
            .await
            .map_err(|error| format!("SFTP write flush failed: {error}"))?;
        Ok(json!({ "localPath": local_path, "remotePath": remote_path, "bytes": data.len() }))
    }

    /// `sftp_download`: transfers one remote file to a local path. The local
    /// target is validated before dialing (those refusals keep the pooled
    /// connection); the remote size is checked against the configured
    /// `maxDownloadBytes` and transport errors drop the cached connection.
    async fn sftp_download_tool(&self, arguments: &Value) -> Result<Value, String> {
        let local_path = required_str(arguments, "localPath")?;
        let remote_path = required_str(arguments, "remotePath")?;
        // Local-write hygiene before anything else: remote content must not
        // land on shell bootstrap / scheduled-execution paths.
        if is_sensitive_local_path(local_path) {
            return Err(format!(
                "Refusing to write the local sensitive path {local_path} via sftp_download"
            ));
        }
        let overwrite = arguments
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let local = Path::new(local_path);
        if local.exists() && !overwrite {
            return Err(format!(
                "Local path already exists: {local_path} (pass overwrite=true to replace)"
            ));
        }
        let file_name = local
            .file_name()
            .ok_or_else(|| format!("Invalid local path: {local_path}"))?;
        if let Some(parent) = local
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!("Cannot create local directory {}: {error}", parent.display())
            })?;
        }
        // Write through the canonical parent (symlinks and `..` resolved by
        // the OS) re-joined with the requested file name, so the target is
        // exactly the requested path and never a traversal artifact.
        let local_target = match local.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            Some(parent) => std::fs::canonicalize(parent)
                .map_err(|error| {
                    format!("Cannot resolve local directory {}: {error}", parent.display())
                })?
                .join(file_name),
            None => PathBuf::from(file_name),
        };
        // Containment before dialing: refusals here keep the pooled
        // connection untouched (same contract as the other local checks).
        self.ensure_local_transfer_allowed(&local_target)?;
        let outcome = self
            .download_via_sftp(arguments, remote_path, local_path, &local_target)
            .await;
        if outcome.is_err() {
            self.drop_connection(arguments).await;
        }
        outcome
    }

    async fn download_via_sftp(
        &self,
        arguments: &Value,
        remote_path: &str,
        local_path: &str,
        local_target: &Path,
    ) -> Result<Value, String> {
        self.connection(arguments).await?;
        let download_limit = self.size_limits().max_download_bytes;
        let mut guard = self.connections.write().await;
        let entry = guard
            .get_mut(&connection_pool_key(arguments))
            .ok_or("Connection is not established")?;
        let sftp = entry.sftp().await?;
        let metadata = sftp
            .lock()
            .await
            .metadata(remote_path)
            .await
            .map_err(sftp_error)?;
        if metadata.is_dir() {
            return Err(format!(
                "{remote_path} is a directory; sftp_download transfers a single file"
            ));
        }
        if let Some(size) = metadata.size {
            if size > download_limit {
                return Err(format!(
                    "Remote file {remote_path} is {size} bytes and exceeds the MCP download \
                     limit of {download_limit} bytes (adjust maxDownloadBytes via mcp/settings/set)"
                ));
            }
        }
        let file = sftp.lock().await.open(remote_path).await.map_err(sftp_error)?;
        // `take` is the hard cap for files that reported no size (or grew
        // between stat and open); the stat check above is only the fast path.
        let mut data = Vec::new();
        file.take(download_limit.saturating_add(1))
            .read_to_end(&mut data)
            .await
            .map_err(|error| format!("SFTP read failed: {error}"))?;
        if data.len() as u64 > download_limit {
            return Err(format!(
                "Remote file {remote_path} exceeds the MCP download limit of {download_limit} \
                 bytes (adjust maxDownloadBytes via mcp/settings/set)"
            ));
        }
        std::fs::write(local_target, &data)
            .map_err(|error| format!("Cannot write local file {}: {error}", local_target.display()))?;
        Ok(json!({ "remotePath": remote_path, "localPath": local_path, "bytes": data.len() }))
    }

    async fn ssh_close(&self, arguments: &Value) -> Result<Value, String> {
        let pool_id = connection_pool_key(arguments);
        let entry = self.connections.write().await.remove(&pool_id);
        match entry {
            Some(entry) => {
                let _ = entry
                    .handle
                    .disconnect(
                        russh::Disconnect::ByApplication,
                        "MCP connection closed",
                        "English",
                    )
                    .await;
                for jump in entry.jumps {
                    let _ = jump
                        .disconnect(
                            russh::Disconnect::ByApplication,
                            "MCP jump connection closed",
                            "English",
                        )
                        .await;
                }
                Ok(json!({ "connectionId": pool_id, "closed": true }))
            }
            None => Err(format!("No cached connection for {pool_id}")),
        }
    }

    async fn drop_connection(&self, arguments: &Value) {
        let pool_id = connection_pool_key(arguments);
        self.connections.write().await.remove(&pool_id);
    }

    async fn connection(&self, arguments: &Value) -> Result<Arc<Handle<SshClient>>, String> {
        // DBX bridge calls reference a saved connection by id (or name);
        // inline calls (standalone --mcp mode) derive the pool key from the
        // credentials. The unified lookup resolves connectionId first, then
        // connectionName, and fails on ambiguity with every candidate.
        let explicit_id = arguments
            .get("connectionId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        let explicit_name = arguments
            .get("connectionName")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let registered = self.registered_connection_by_ref(arguments).await?;
        let (pool_id, connection) = match registered {
            // The real registry id is the pool key either way, so pooled
            // handles line up with the app-side and forwarded calls.
            Some(connection) => (connection.id.clone(), Some(connection)),
            None => {
                if let Some(id) = explicit_id {
                    // Unregistered id: a pooled handle may still exist (an
                    // earlier registration left one behind), so the pool hit
                    // below gets its chance before the guidance error fires.
                    (id.to_string(), None)
                } else if let Some(name) = explicit_name {
                    // Unregistered name: no pool key can be derived, so the
                    // self-heal error fires immediately (L0).
                    return Err(format!(
                        "No connection named '{name}' is registered with this plugin session and \
                         the DBX app bridge is unavailable. Start the DBX app, list saved \
                         connections with ssh_list_connections, or provide inline credentials \
                         (host/username plus password or privateKeyPath)."
                    ));
                } else {
                    (
                        connection_pool_id(arguments),
                        Some(stored_connection_from_arguments(arguments)?),
                    )
                }
            }
        };
        {
            let guard = self.connections.read().await;
            if let Some(entry) = guard.get(&pool_id) {
                return Ok(entry.handle.clone());
            }
        }
        // Unregistered connectionId with no pooled handle: with the L1
        // bridge fallback active this only fires when the DBX app bridge is
        // also unavailable, so the message carries the full self-heal path.
        let connection = match connection {
            Some(connection) => connection,
            None => {
                return Err(format!(
                    "Connection {pool_id} is not registered with this plugin session and the DBX \
                     app bridge is unavailable. Start the DBX app, list saved connections with \
                     ssh_list_connections, or provide inline credentials (host/username plus \
                     password or privateKeyPath)."
                ))
            }
        };
        let (handle, jumps) = self.runtime.connect_headless(&connection).await?;
        let mut guard = self.connections.write().await;
        drop(connection);
        let entry = guard.entry(pool_id).or_insert(McpConnection {
            handle: handle.clone(),
            jumps,
            sftp: None,
        });
        Ok(entry.handle.clone())
    }

    /// `mcp/call` entry used by the DBX MCP bridge (`dbx_call_plugin_tool`):
    /// registers the forwarded connection lifecycle payload, then dispatches
    /// the tool with a `connectionId` reference so credentials never travel
    /// inline with tool arguments. The emitter enables the agent terminal
    /// routing (`runInTerminal` / `agentTerminalMode`); stdio mode passes
    /// `None` and only leaves the hidden exec channel for `runInTerminal`
    /// calls, which forward to the DBX app's local TCP bridge.
    pub async fn call_dbx(&self, params: &Value, emitter: PluginEmitter) -> Result<Value, String> {
        self.call_dbx_with(params, Some(emitter)).await
    }

    /// Emitter-less variant; tests (and the stdio path semantics) use it to
    /// exercise lifecycle registration without a bridge emitter.
    pub(crate) async fn call_dbx_with(
        &self,
        params: &Value,
        emitter: Option<PluginEmitter>,
    ) -> Result<Value, String> {
        let tool = required_str(params, "tool")?.to_string();
        let mut arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        if !arguments.is_object() {
            return Err("arguments must be a JSON object".to_string());
        }
        if let Some(lifecycle) = params.get("lifecycle") {
            let connection = StoredConnection::from_lifecycle_params(lifecycle)?;
            self.dbx_connections
                .write()
                .await
                .insert(connection.id.clone(), connection.clone());
            if let Some(map) = arguments.as_object_mut() {
                map.insert("connectionId".to_string(), json!(connection.id));
            }
        }
        self.call_tool(&tool, &arguments, emitter.as_ref()).await
    }
}

async fn remove_tree(sftp: &Arc<AsyncMutex<SftpSession>>, root: String) -> Result<(), String> {
    let mut pending = vec![root];
    let mut directories = Vec::new();
    while let Some(directory) = pending.pop() {
        directories.push(directory.clone());
        let entries = sftp
            .lock()
            .await
            .read_dir(directory)
            .await
            .map_err(sftp_error)?;
        for entry in entries {
            if entry.file_type() == FileType::Dir {
                pending.push(entry.path());
            } else {
                sftp.lock()
                    .await
                    .remove_file(entry.path())
                    .await
                    .map_err(sftp_error)?;
            }
        }
    }
    for directory in directories.into_iter().rev() {
        sftp.lock()
            .await
            .remove_dir(directory)
            .await
            .map_err(sftp_error)?;
    }
    Ok(())
}

fn required_str<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| format!("Missing required parameter: {key}"))
}

fn sftp_error(error: impl std::fmt::Display) -> String {
    format!("SFTP operation failed: {error}")
}

fn connection_pool_id(arguments: &Value) -> String {
    let host = arguments.get("host").and_then(Value::as_str).unwrap_or("");
    let port = arguments.get("port").and_then(Value::as_u64).unwrap_or(22);
    let username = arguments
        .get("username")
        .and_then(Value::as_str)
        .unwrap_or("");
    format!("mcp-{username}@{host}:{port}")
}

/// Pool key for an arguments payload: an explicit `connectionId` (the DBX
/// bridge path) wins over inline credentials, keeping `ssh_close` on the same
/// pool key `connection()` used to register the handle.
fn connection_pool_key(arguments: &Value) -> String {
    arguments
        .get("connectionId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| connection_pool_id(arguments))
}

/// Tools whose execution targets a saved connection: in stdio mode an
/// unregistered `connectionId` is forwarded through the DBX app bridge (L1
/// fallback) instead of failing the local registry lookup. Local-only tools
/// are deliberately absent: `ssh_close` keeps local pool semantics,
/// `ssh_test_connection` is a local dial, known-hosts / quick-sudo /
/// settings tools never target a connection, and `ssh_list_connections` is
/// the discovery surface itself. `sftp_upload` / `sftp_download` forward
/// too: bridge and stdio sidecar run on the same machine, so `localPath`
/// stays valid on the app side (its sidecar re-applies the local transfer
/// gates with the shared plugin settings).
fn is_connection_bound_tool(name: &str) -> bool {
    matches!(
        name,
        "ssh_exec"
            | "ssh_exec_sudo"
            | "ssh_run_bg"
            | "ssh_task_status"
            | "ssh_metrics"
            | "sftp_list_dir"
            | "sftp_stat"
            | "sftp_exists"
            | "sftp_pwd"
            | "sftp_read_file"
            | "sftp_write_file"
            | "sftp_mkdir"
            | "sftp_remove"
            | "sftp_rename"
            | "sftp_chmod"
            | "sftp_copy"
            | "sftp_move"
            | "sftp_disk_usage"
            | "sftp_upload"
            | "sftp_download"
    )
}

/// Registry entry as a list view: metadata plus credential PRESENCE flags
/// only (same discipline as the Quick Sudo profile views — never values).
fn registry_connection_view(connection: &StoredConnection) -> Value {
    json!({
        "id": connection.id,
        "name": connection.name,
        "host": connection.host,
        "port": connection.port,
        "username": connection.username,
        "authentication": connection.authentication.method_name(),
        "readOnly": connection.read_only,
        "passwordSet": !connection.password.is_empty(),
    })
}

/// Merges the bridge connection list (preferred, when the app answered)
/// with the session registry, deduplicated by id (bridge data wins).
/// Returns the tool payload: `source` records the data origin, and the
/// degraded branch carries a note pointing at the app start/upgrade
/// requirement. Bridge entries are credential-free by contract; registry
/// views expose presence flags only.
fn connection_list_result(
    bridge: Result<Vec<Value>, String>,
    registry: &[StoredConnection],
) -> Value {
    match bridge {
        Ok(entries) => {
            let listed: std::collections::HashSet<String> = entries
                .iter()
                .filter_map(|entry| entry.get("id").and_then(Value::as_str).map(str::to_string))
                .collect();
            let mut merged = entries;
            for connection in registry
                .iter()
                .filter(|connection| !listed.contains(&connection.id))
            {
                merged.push(registry_connection_view(connection));
            }
            json!({ "connections": merged, "source": "dbx-app-bridge" })
        }
        Err(_) => json!({
            "connections": registry
                .iter()
                .map(registry_connection_view)
                .collect::<Vec<_>>(),
            "source": "session-registry",
            "note": "The DBX app bridge did not answer the saved-connection list (the app is not running, or this DBX version predates the /list-plugin-connections route). Only connections registered in this session are shown; start or upgrade the DBX app to get the full saved-connection list.",
        }),
    }
}

/// Pure name→id resolution over a bridge connection list: exactly one
/// trimmed-name hit yields its id; zero hits and multiple hits are errors,
/// the latter listing every candidate id + host for disambiguation.
fn resolve_name_in_bridge_list(entries: &[Value], name: &str) -> Result<String, String> {
    let wanted = name.trim();
    let mut hits: Vec<(&str, &str)> = Vec::new();
    for entry in entries {
        let entry_name = entry.get("name").and_then(Value::as_str).map(str::trim);
        if entry_name != Some(wanted) {
            continue;
        }
        if let Some(id) = entry.get("id").and_then(Value::as_str) {
            let host = entry.get("host").and_then(Value::as_str).unwrap_or("");
            hits.push((id, host));
        }
    }
    match hits.len() {
        1 => Ok(hits[0].0.to_string()),
        0 => Err(format!(
            "No connection named '{wanted}' in the DBX app's saved-connection list"
        )),
        _ => Err(format!(
            "Connection name '{wanted}' is ambiguous: {}; pass connectionId instead",
            hits.iter()
                .map(|(id, host)| format!("{id} (host {host})"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Renders registry candidates for the ambiguous-connectionName error.
fn render_connection_candidates(connections: &[&StoredConnection]) -> String {
    connections
        .iter()
        .map(|connection| format!("{} (host {})", connection.id, connection.host))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Tools that mutate remote state and must be rejected on read-only
/// connections (mirrors the workbench `ensure_writable` / sudo-exec gates).
/// Plain `ssh_exec` is not listed: on read-only connections it is gated by
/// the read-only command whitelist in `call_tool` instead, so inspection
/// commands (`df`, `systemctl status`, ...) stay available. Like the
/// workbench read-only terminal, `sudo` execution is always refused.
/// `sftp_download` stays allowed too: it only reads the remote side (its
/// local write target is guarded by `is_sensitive_local_path`, and the
/// remote path by the sensitive-path denylist on read-only connections).
fn is_write_tool(name: &str) -> bool {
    matches!(
        name,
        "ssh_exec_sudo"
            | "ssh_run_bg"
            | "sftp_write_file"
            | "sftp_upload"
            | "sftp_mkdir"
            | "sftp_remove"
            | "sftp_rename"
            | "sftp_chmod"
            | "sftp_copy"
            | "sftp_move"
    )
}

/// Remote-path arguments of read tools that must respect the sensitive-path
/// denylist on read-only connections. These tools return file content or
/// listings straight to the LLM, a channel the exec command whitelist cannot
/// classify; the denylist itself lives in `mcp_safety::is_sensitive_path`.
fn sensitive_read_path(name: &str, arguments: &Value) -> Option<String> {
    if !matches!(
        name,
        "sftp_list_dir"
            | "sftp_read_file"
            | "sftp_stat"
            | "sftp_exists"
            | "sftp_download"
            | "ssh_task_status"
    ) {
        return None;
    }
    ["path", "remotePath", "logPath"]
        .iter()
        .find_map(|key| {
            arguments
                .get(*key)
                .and_then(Value::as_str)
                .filter(|value| mcp_safety::is_sensitive_path(value))
                .map(str::to_string)
        })
}

/// Local write targets that must never receive downloaded content through
/// the MCP channel: shell/SSH bootstrap files plus cron / systemd / launchd
/// drop locations turn a remote file into local code execution. Applied on
/// every connection (read-only or not) — this guard protects the operator
/// machine, not the remote side. Defense in depth, not a sandbox.
fn is_sensitive_local_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    const COMPONENTS: &[&str] = &[".ssh", ".gnupg"];
    if lower
        .split('/')
        .any(|component| COMPONENTS.contains(&component))
    {
        return true;
    }
    const PREFIXES: &[&str] = &[
        "/etc/cron",
        "/var/spool/cron",
        "/etc/sudoers",
        "/etc/ssh",
        "/etc/ld.so",
        "/etc/pam.d",
        "/etc/profile",
        "/etc/bash",
        "/etc/rc",
        "/etc/systemd/system",
        "/library/launchdaemons",
        "/library/launchagents",
    ];
    if PREFIXES.iter().any(|prefix| lower.starts_with(prefix)) {
        return true;
    }
    let file_name = Path::new(&lower)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    matches!(
        file_name,
        ".bashrc" | ".zshrc" | ".profile" | ".bash_profile" | ".zprofile" | ".zshenv"
            | ".zlogin" | ".cshrc" | ".tcshrc" | "authorized_keys" | ".netrc"
            | ".git-credentials" | ".npmrc" | ".htpasswd"
    )
}

/// Resolves the allowed local roots for agent-driven `sftp_upload` /
/// `sftp_download` transfers: the configured `localTransferRoot` when set
/// (it must resolve — an unreachable root is an error, never a silent
/// fallback), otherwise the OS temp dir plus the plugin data dir. Roots
/// are canonicalized so macOS `/var` → `/private/var` style aliasing
/// cannot dodge the `starts_with` containment check.
fn local_transfer_roots_for(configured: &str, data_dir: &Path) -> Result<Vec<PathBuf>, String> {
    if !configured.is_empty() {
        return std::fs::canonicalize(configured)
            .map(|root| vec![root])
            .map_err(|error| {
                format!(
                    "Configured localTransferRoot {configured} cannot be resolved: {error} \
                     (create the directory or reset it via mcp/settings/set)"
                )
            });
    }
    Ok([std::env::temp_dir(), data_dir.to_path_buf()]
        .into_iter()
        .filter_map(|root| std::fs::canonicalize(&root).ok())
        .collect())
}

/// Containment gate for every agent-supplied local path behind
/// `sftp_upload` (read side) and `sftp_download` (write side): the
/// canonical path must stay inside an allowed root and clear the
/// sensitive-path blocklist (credential stores, shell bootstrap files) in
/// every mode, so a configured root cannot be used to reach them either.
/// `path` must already be canonical — symlink and `..` traversal artifacts
/// are resolved by the caller (`std::fs::canonicalize`, or the
/// canonical-parent rejoin on the download side).
fn ensure_local_transfer_allowed_in(
    roots: &[PathBuf],
    configured: &str,
    path: &Path,
) -> Result<(), String> {
    if is_sensitive_local_path(&path.to_string_lossy()) {
        return Err(format!(
            "Refusing to transfer the local sensitive path {} via sftp_upload/sftp_download",
            path.display()
        ));
    }
    if roots.iter().any(|root| path.starts_with(root)) {
        return Ok(());
    }
    Err(if configured.is_empty() {
        format!(
            "Local path {} is outside the allowed transfer roots (OS temp dir and plugin data \
             dir); set localTransferRoot via mcp/settings/set to allow more",
            path.display()
        )
    } else {
        format!(
            "Local path {} is outside the configured localTransferRoot {configured}",
            path.display()
        )
    })
}

/// Resolves the optional `quickSudoProfile` argument (id or exact name)
/// before any connection I/O, so unknown references fail fast instead of
/// dialing first.
fn resolve_profile_reference(
    store: &sudo_profiles::SudoProfileStore,
    arguments: &Value,
) -> Result<Option<sudo_profiles::SudoProfile>, String> {
    let Some(reference) = arguments
        .get("quickSudoProfile")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    sudo_profiles::find_by_ref(store, reference)
        .cloned()
        .map(Some)
        .ok_or_else(|| format!("Quick Sudo profile '{reference}' not found"))
}

/// Fills auth gaps from a resolved global Quick Sudo profile: fields the
/// caller set explicitly always win; unset fields fall back to the profile.
fn apply_profile_fallbacks(auth: &mut SudoAuth, profile: &sudo_profiles::SudoProfile) {
    if auth.password.is_empty() && !profile.sudo_password.trim().is_empty() {
        auth.password = profile.sudo_password.trim().to_string();
    }
    if auth.totp_secrets.is_empty() {
        auth.totp_secrets = exec::parse_totp_secrets(&profile.totp_secret);
    }
    if auth.password_prompt_hint.is_empty() {
        auth.password_prompt_hint = exec::sanitize_prompt_hint(&profile.password_prompt_hint);
    }
    if auth.totp_prompt_hint.is_empty() {
        auth.totp_prompt_hint = exec::sanitize_prompt_hint(&profile.totp_prompt_hint);
    }
    if auth.flow_mode.is_none() {
        auth.flow_mode = Some(AuthFlowMode::parse(&profile.auth_flow_mode));
    }
}

/// Re-applies the caller's explicit per-call arguments on top of the
/// source-resolved base auth: values the caller actually sent always win,
/// whatever the connection's declared sudo source contributed.
fn apply_explicit_argument_overrides(auth: &mut SudoAuth, arguments: &Value) {
    let explicit = |key: &str| {
        arguments
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    if let Some(password) = explicit("sudoPassword") {
        auth.password = password.to_string();
    }
    if let Some(secret) = explicit("totpSecret") {
        auth.totp_secrets = exec::parse_totp_secrets(secret);
    }
    if let Some(hint) = explicit("passwordPromptHint") {
        auth.password_prompt_hint = exec::sanitize_prompt_hint(hint);
    }
    if let Some(hint) = explicit("totpPromptHint") {
        auth.totp_prompt_hint = exec::sanitize_prompt_hint(hint);
    }
    if let Some(mode) = explicit("authFlowMode") {
        auth.flow_mode = Some(AuthFlowMode::parse(mode));
    }
}

fn sudo_auth(arguments: &Value) -> SudoAuth {
    let mut auth = SudoAuth::new(
        arguments
            .get("sudoPassword")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        arguments
            .get("password")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        arguments
            .get("totpSecret")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        Hints {
            password: arguments
                .get("passwordPromptHint")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            totp: arguments
                .get("totpPromptHint")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            flow_mode: arguments
                .get("authFlowMode")
                .and_then(Value::as_str)
                .map(AuthFlowMode::parse),
        },
    );
    auth.otp_ledger_scope = exec::otp_ledger_scope_for(
        arguments
            .get("username")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        arguments.get("host").and_then(Value::as_str).unwrap_or_default(),
        arguments.get("port").and_then(Value::as_u64).unwrap_or(22) as u16,
    );
    auth
}

fn stored_connection_from_arguments(arguments: &Value) -> Result<StoredConnection, String> {
    let jump_hosts = parse_jump_hosts(arguments)?;
    let host = required_str(arguments, "host")?;
    let username = required_str(arguments, "username")?;
    let port = arguments
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .filter(|value| *value > 0)
        .unwrap_or(22);
    let password = arguments
        .get("password")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let private_key_path = arguments
        .get("privateKeyPath")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let authentication = match arguments
        .get("authentication")
        .and_then(Value::as_str)
        .unwrap_or("password")
    {
        "private-key" if !private_key_path.is_empty() => AuthenticationMethod::PrivateKey,
        "private-key-password" if !private_key_path.is_empty() => {
            AuthenticationMethod::PrivateKeyPassword
        }
        "agent" => AuthenticationMethod::Agent,
        _ => {
            if !private_key_path.is_empty() {
                AuthenticationMethod::PrivateKey
            } else {
                AuthenticationMethod::Password
            }
        }
    };
    if matches!(authentication, AuthenticationMethod::Password) && password.is_empty() {
        return Err("Password authentication requires a password".to_string());
    }
    if matches!(
        authentication,
        AuthenticationMethod::PrivateKey | AuthenticationMethod::PrivateKeyPassword
    ) && private_key_path.is_empty()
    {
        return Err("Private-key authentication requires privateKeyPath".to_string());
    }
    Ok(StoredConnection {
        sudo_whitelist: Vec::new(),
        id: connection_pool_id(arguments),
        // Inline MCP dials carry no display name.
        name: None,
        host: host.to_string(),
        port,
        runtime_host: host.to_string(),
        runtime_port: port,
        username: username.to_string(),
        password,
        authentication,
        private_key_path,
        private_key_passphrase: arguments
            .get("privateKeyPassphrase")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        agent_socket: arguments
            .get("agentSocket")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        connect_timeout_secs: arguments
            .get("connectTimeoutSecs")
            .and_then(Value::as_u64)
            .unwrap_or(15)
            .max(1),
        keepalive_interval_secs: 30,
        // MCP drives exec channels, never the user's interactive PTY —
        // activity injection doesn't apply here.
        terminal_keepalive_secs: 0,
        read_only: false,
        sudo_password: arguments
            .get("sudoPassword")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        totp_secret: arguments
            .get("totpSecret")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        // MCP keeps its own quickSudoProfile resolution (explicit arguments
        // win), so the stored connection carries the plain on/off source.
        sudo_source: if arguments
            .get("quickSudo")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        {
            SudoSource::Custom
        } else {
            SudoSource::Off
        },
        sudo_profile_ref: String::new(),
        sudo_use_pty: false,
        password_prompt_hint: arguments
            .get("passwordPromptHint")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        totp_prompt_hint: arguments
            .get("totpPromptHint")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        auth_flow_mode: arguments
            .get("authFlowMode")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        // MCP tool calls carry their own env/command semantics; connection
        // SetEnv / RemoteCommand are workbench connection-form features.
        set_env: Vec::new(),
        remote_command: String::new(),
        jump_hosts,
    })
}

/// Parses the `PID=` / `LOG=` lines emitted by the `ssh_run_bg` staging
/// command. Falls back to the derived default log path when the remote
/// never echoed `LOG=`.
fn parse_bg_start_output(output: &str, default_log: String) -> (String, String) {
    let mut pid = String::new();
    let mut log_path = default_log;
    for line in output.lines() {
        if let Some(value) = line.strip_prefix("PID=") {
            pid = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("LOG=") {
            log_path = value.trim().to_string();
        }
    }
    (pid, log_path)
}

/// Parses the STATE= / CODE= / PID_ALIVE= markers and the post-`===TAIL===`
/// section emitted by the `ssh_task_status` probe command.
type TaskStatusParse = (String, Option<i64>, Option<bool>, String);
fn parse_task_status_output(output: &str) -> TaskStatusParse {
    let mut state = "unknown".to_string();
    let mut exit_code: Option<i64> = None;
    let mut pid_alive: Option<bool> = None;
    let mut tail = String::new();
    for line in output.lines() {
        if let Some(value) = line.strip_prefix("STATE=") {
            state = value.trim().to_lowercase();
        } else if let Some(value) = line.strip_prefix("CODE=EXIT_") {
            exit_code = value.trim().parse().ok();
        } else if let Some(value) = line.strip_prefix("PID_ALIVE=") {
            pid_alive = Some(value.trim() == "yes");
        } else if line == "===TAIL===" {
            tail.clear();
        } else if !line.is_empty() {
            tail.push_str(line);
            tail.push('\n');
        }
    }
    (state, exit_code, pid_alive, tail)
}

/// Parses the optional `jumpHosts` array (snake_case fields, same shape as
/// `external_config.jump_hosts`) into the ProxyJump chain.
fn parse_jump_hosts(arguments: &Value) -> Result<Vec<JumpHost>, String> {
    let Some(list) = arguments.get("jumpHosts") else {
        return Ok(Vec::new());
    };
    let Some(list) = list.as_array() else {
        return Err("jumpHosts must be an array of jump host objects".to_string());
    };
    if list.len() > 3 {
        return Err("At most 3 jump hosts are supported".to_string());
    }
    let mut hosts = Vec::with_capacity(list.len());
    for (position, value) in list.iter().enumerate() {
        let host = JumpHost::from_json(value)
            .map_err(|error| format!("jumpHosts[{position}]: {error}"))?;
        host.validate(position)
            .map_err(|error| format!("jumpHosts[{position}]: {error}"))?;
        hosts.push(host);
    }
    Ok(hosts)
}

/// Tool-specific parameters layered on the shared connection properties.
/// Each entry is `(key, json type, description)`; the type is part of the
/// contract because the dispatcher parses numbers and booleans from these
/// fields (`maxBytes`, `overwrite`, …).
fn connection_properties(extra: &[(&str, &str, &str)]) -> Value {
    let mut properties = json!({
        // Declared here so strict MCP hosts forward it: the dispatcher reads
        // it for terminal routing (runInTerminal) and stored-connection
        // resolution, and an undeclared argument is dropped by schema
        // validation before the sidecar ever sees the call.
        "connectionId": { "type": "string", "description": "Saved DBX connection id (list ids with ssh_list_connections, or in the DBX app). With runInTerminal: true, a stdio-mode call is forwarded through the DBX app bridge to the connection's visible workbench terminal; on the embedded bridge it also resolves the stored connection's Quick Sudo source and read-only flag. In stdio mode an id unknown to this session is forwarded to the running DBX app, so no inline credentials are needed" },
        "connectionName": { "type": "string", "description": "Saved DBX connection name, an alternative to connectionId: resolved against this session's registry first, then through ssh_list_connections in stdio mode. Ambiguous names are refused with their candidate ids" },
        "host": { "type": "string", "description": "Remote SSH host" },
        "port": { "type": "integer", "description": "SSH port (default 22)" },
        "username": { "type": "string", "description": "Login user" },
        "password": { "type": "string", "description": "Login password (password auth, or sudo fallback)" },
        "privateKeyPath": { "type": "string", "description": "Local private key path for key auth" },
        "privateKeyPassphrase": { "type": "string", "description": "Private key passphrase" },
        "agentSocket": { "type": "string", "description": "SSH agent socket for agent auth" },
        "authentication": { "type": "string", "enum": ["password", "private-key", "private-key-password", "agent"] },
        "connectTimeoutSecs": { "type": "integer" },
        "sudoPassword": { "type": "string", "description": "Sudo password override (defaults to password)" },
        "totpSecret": { "type": "string", "description": "TOTP secret (otpauth:// URI, base32 key, or static code) for 2FA auto-answer; multiple secrets (newline/semicolon separated) rotate automatically — unexpired, unused codes first, across calls" },
        "authFlowMode": { "type": "string", "enum": ["password_only", "password_plus_otp", "password_then_otp"] },
        "passwordPromptHint": { "type": "string" },
        "totpPromptHint": { "type": "string" },
        "jumpHosts": { "type": "array", "description": "ProxyJump chain (up to 3): [{\"host\":\"bastion\",\"port\":22,\"username\":\"ops\",\"password\":\"…\"}] with snake_case fields; replaces direct dialing" },
    });
    if let Some(map) = properties.as_object_mut() {
        for (key, kind, description) in extra {
            map.insert(
                key.to_string(),
                json!({ "type": kind, "description": description }),
            );
        }
    }
    properties
}

fn required_connection() -> Vec<String> {
    vec!["host".to_string(), "username".to_string()]
}

pub fn tool_definitions() -> Value {
    json!([
        {
            "name": "ssh_exec",
            "description": "Run a non-interactive remote shell command over SSH. Quick Sudo orchestration is NOT applied; use ssh_exec_sudo for privileged commands. Commands matching catastrophic patterns (disk formatting, recursive system deletes, shutdown, raw device writes, SQL DROP) require confirmDestructive: true; on read-only connections only whitelisted inspection commands (ls, cat, df, ps, systemctl status, journalctl, docker ps, ...) are allowed. Hosts commonly give up waiting after ~15s regardless of timeoutSecs while the command keeps running remotely (and further calls to this server stall until it finishes) - for anything that may exceed ~10s use ssh_run_bg + ssh_task_status instead. After a timeout the command may STILL be running: verify before rerunning.",
            "inputSchema": {
                "type": "object",
                "properties": connection_properties(&[
                    ("command", "string", "Shell command to execute"),
                    ("timeoutSecs", "integer", "Plugin-side wait cap in seconds (5-300, default 60). The MCP host may abandon the wait earlier (~15s); the command keeps running remotely either way"),
                    ("confirmDestructive", "boolean", "Set true to allow a command recognized as destructive (disk formatting, recursive system deletes, shutdown, ...) after human review"),
                    ("runInTerminal", "boolean", "Run inside the user's visible DBX terminal so the command and its output are visible and interruptible. Through the DBX embedded bridge it routes to the open workbench terminal; in stdio mode it is forwarded to the DBX app bridge (requires a saved connectionId that exists in the DBX app). When omitted, the connection's terminal MCP mode — toggled in the DBX terminal toolbar — decides: modes other than off route every exec through the visible terminal, off keeps the silent hidden channel"),
                ]),
                "required": ["command"],
            },
        },
        {
            "name": "ssh_exec_sudo",
            "description": "Run a remote shell command with sudo. The sudo password is piped over stdin and 2FA/TOTP prompts are answered automatically when a TOTP secret is available (inline arguments, or a shared global Quick Sudo profile referenced by quickSudoProfile; explicit arguments win). Commands matching catastrophic patterns require confirmDestructive: true; refused outright on read-only connections. Same wait-cap caveat as ssh_exec: hosts may stop waiting after ~15s; prefer short commands and keep long privileged jobs under ssh_run_bg.",
            "inputSchema": {
                "type": "object",
                "properties": connection_properties(&[
                    ("command", "string", "Shell command to execute with sudo"),
                    ("timeoutSecs", "integer", "Plugin-side wait cap in seconds (5-300, default 90). The MCP host may abandon the wait earlier (~15s); the command keeps running remotely either way"),
                    ("quickSudoProfile", "string", "Global Quick Sudo profile id or exact name supplying sudo password/TOTP/prompt defaults"),
                    ("confirmDestructive", "boolean", "Set true to allow a command recognized as destructive (disk formatting, recursive system deletes, shutdown, ...) after human review"),
                    ("runInTerminal", "boolean", "Run inside the user's visible DBX terminal so the command and its output are visible and interruptible. Through the DBX embedded bridge it routes to the open workbench terminal; in stdio mode it is forwarded to the DBX app bridge (requires a saved connectionId that exists in the DBX app). When omitted, the connection's terminal MCP mode — toggled in the DBX terminal toolbar — decides: modes other than off route every exec through the visible terminal, off keeps the silent hidden channel"),
                ]),
                "required": ["command"],
            },
        },
        {
            "name": "ssh_run_bg",
            "description": "Start a long-running command detached on the remote host (nohup; output appended to /tmp/.dbx-ssh-tasks/<taskId>.log) and return immediately with taskId/pid/logPath. Survives disconnects, MCP-host wait caps, and session restarts because the output lives on the server. Poll progress with ssh_task_status(logPath). Same safety gates as ssh_exec: destructive patterns need confirmDestructive: true, read-only connections refuse it.",
            "inputSchema": {
                "type": "object",
                "properties": connection_properties(&[
                    ("command", "string", "Shell command to run detached"),
                    ("confirmDestructive", "boolean", "Set true to allow a command recognized as destructive (disk formatting, recursive system deletes, shutdown, ...) after human review"),
                ]),
                "required": ["command"],
            },
        },
        {
            "name": "ssh_task_status",
            "description": "Poll a background task started with ssh_run_bg: returns state (running/done/missing), exit code once finished, pid liveness, and the trailing bytes of output from the server-side log file. Reconnects transparently, so it works after disconnects or from a later session.",
            "inputSchema": {
                "type": "object",
                "properties": connection_properties(&[
                    ("logPath", "string", "logPath returned by ssh_run_bg"),
                    ("tailBytes", "integer", "Trailing bytes of output to return (200-16000, default 4000)"),
                ]),
                "required": ["logPath"],
            },
        },
        {
            "name": "ssh_metrics",
            "description": "Collect server metrics (CPU utilization, load, memory, swap, disk mounts with inode usage, uptime, per-interface network rx/tx rates, top 8 processes by CPU and by memory) using read-only commands.",
            "inputSchema": {
                "type": "object",
                "properties": connection_properties(&[]),
                "required": required_connection(),
            },
        },
        {
            "name": "ssh_close",
            "description": "Close the cached SSH/SFTP connection for a host after finishing work.",
            "inputSchema": {
                "type": "object",
                "properties": connection_properties(&[]),
                "required": required_connection(),
            },
        },
        {
            "name": "ssh_test_connection",
            "description": "Verify connectivity and authentication for inline SSH settings (including the jump chain) without running commands.",
            "inputSchema": { "type": "object", "properties": connection_properties(&[]), "required": required_connection() },
        },
        {
            "name": "ssh_list_known_hosts",
            "description": "List entries of the plugin's known_hosts store (the system ~/.ssh/known_hosts is never modified).",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "ssh_list_connections",
            "description": "List the DBX app's saved SSH connections for this plugin (id, name, host, port, username, authentication method, read-only flag) merged with connections registered in this MCP session. Metadata only: credentials are never included. Use an entry's id as connectionId, or its name as connectionName, on the connection-bound tools.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "ssh_remove_known_host",
            "description": "Remove entries for host:port from the plugin's known_hosts store; use after a legitimate server reinstall.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "host": { "type": "string", "description": "Host name as recorded" },
                    "port": { "type": "integer", "description": "Port (default 22)" },
                },
                "required": ["host"],
            },
        },
        {
            "name": "ssh_quick_sudo_profiles_list",
            "description": "List the plugin's global Quick Sudo profiles: named sudo password/TOTP/prompt presets reusable across connections. Secrets are reported as configured flags only, never as values.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "ssh_quick_sudo_profiles_save",
            "description": "Create or update a global Quick Sudo profile (supply id to update). Empty sudoPassword/totpSecret keep the stored values; clearSudoPassword/clearTotpSecret remove them. Returns the profile without secrets.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Profile id to update; omit to create" },
                    "name": { "type": "string", "description": "Unique display name (max 64 characters)" },
                    "sudoPassword": { "type": "string", "description": "Sudo password; empty keeps the stored one" },
                    "totpSecret": { "type": "string", "description": "TOTP secret (otpauth:// URI, base32 key, or static code); multiple secrets (newline/semicolon separated) rotate automatically — unexpired, unused codes first. Empty keeps the stored one" },
                    "clearSudoPassword": { "type": "boolean", "description": "Set true to remove the stored sudo password" },
                    "clearTotpSecret": { "type": "boolean", "description": "Set true to remove the stored TOTP secret" },
                    "authFlowMode": { "type": "string", "enum": ["password_only", "password_plus_otp", "password_then_otp"] },
                    "passwordPromptHint": { "type": "string" },
                    "totpPromptHint": { "type": "string" },
                    "sudoUsePty": { "type": "boolean", "description": "Request a PTY for sudo executions using this profile" },
                },
                "required": ["name"],
            },
        },
        {
            "name": "ssh_quick_sudo_profiles_delete",
            "description": "Delete a global Quick Sudo profile by id. Connections bound to it fall back to their own sudo configuration.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Profile id" },
                },
                "required": ["id"],
            },
        },
        {
            "name": "sftp_list_dir",
            "description": "List a remote directory over SFTP.",
            "inputSchema": { "type": "object", "properties": connection_properties(&[("path", "string", "Remote directory path")]), "required": ["path"] },
        },
        {
            "name": "sftp_stat",
            "description": "Inspect a remote path over SFTP (size, permissions, timestamps).",
            "inputSchema": { "type": "object", "properties": connection_properties(&[("path", "string", "Remote path")]), "required": ["path"] },
        },
        {
            "name": "sftp_exists",
            "description": "Check whether a remote path exists.",
            "inputSchema": { "type": "object", "properties": connection_properties(&[("path", "string", "Remote path")]), "required": ["path"] },
        },
        {
            "name": "sftp_pwd",
            "description": "Return the remote login user's home directory (canonicalized absolute path over SFTP).",
            "inputSchema": { "type": "object", "properties": connection_properties(&[]), "required": required_connection() },
        },
        {
            "name": "sftp_upload",
            "description": "Upload a local file to the remote server over SFTP (single file, no directory recursion). The local file must be readable by the MCP server process; size is capped by the configured maxUploadBytes.",
            "inputSchema": { "type": "object", "properties": connection_properties(&[
                ("localPath", "string", "Local file path to upload"),
                ("remotePath", "string", "Remote file path to create"),
                ("overwrite", "boolean", "Set true to replace an existing remote file"),
            ]), "required": ["localPath", "remotePath"] },
        },
        {
            "name": "sftp_download",
            "description": "Download a remote file to a local path over SFTP (single file). Remote size is capped by the configured maxDownloadBytes; missing local parent directories are created. Shell/cron/systemd bootstrap paths are refused as local targets, and on read-only connections credential paths are refused as remote sources.",
            "inputSchema": { "type": "object", "properties": connection_properties(&[
                ("remotePath", "string", "Remote file path to download"),
                ("localPath", "string", "Local target file path"),
                ("overwrite", "boolean", "Set true to replace an existing local file"),
            ]), "required": ["remotePath", "localPath"] },
        },
        {
            "name": "sftp_read_file",
            "description": "Read a remote file (text by default, or base64). Truncates at maxBytes; default and ceiling come from the plugin's MCP size settings. Pass offset to continue reading from a byte position (empty result at/after EOF).",
            "inputSchema": { "type": "object", "properties": connection_properties(&[
                ("path", "string", "Remote file path"),
                ("maxBytes", "integer", "Maximum bytes to read; clamped to the configured maxDownloadBytes (base64 field ignored)"),
                ("offset", "integer", "Byte offset to start reading from (default 0)"),
                ("base64", "boolean", "Set true to return base64 instead of UTF-8 text"),
            ]), "required": ["path"] },
        },
        {
            "name": "sftp_write_file",
            "description": "Write a remote file with UTF-8 content. Existing files require overwrite=true; content is capped at the configured maxUploadBytes.",
            "inputSchema": { "type": "object", "properties": connection_properties(&[
                ("path", "string", "Remote file path"),
                ("content", "string", "File content (UTF-8 text)"),
                ("overwrite", "boolean", "Set true to replace an existing file"),
            ]), "required": ["path", "content"] },
        },
        {
            "name": "sftp_mkdir",
            "description": "Create a remote directory.",
            "inputSchema": { "type": "object", "properties": connection_properties(&[("path", "string", "Remote directory path")]), "required": ["path"] },
        },
        {
            "name": "sftp_remove",
            "description": "Remove a remote file or directory (directories need recursive=true).",
            "inputSchema": { "type": "object", "properties": connection_properties(&[
                ("path", "string", "Remote path"),
                ("recursive", "boolean", "Set true to remove directories recursively"),
            ]), "required": ["path"] },
        },
        {
            "name": "sftp_rename",
            "description": "Rename or move a remote path.",
            "inputSchema": { "type": "object", "properties": connection_properties(&[
                ("sourcePath", "string", "Existing remote path"),
                ("targetPath", "string", "New remote path"),
            ]), "required": ["sourcePath", "targetPath"] },
        },
        {
            "name": "sftp_chmod",
            "description": "Change permission bits of a remote path (octal, e.g. 0644).",
            "inputSchema": { "type": "object", "properties": connection_properties(&[
                ("path", "string", "Remote path"),
                ("mode", "string", "Octal permission value such as 0644 or 0755"),
            ]), "required": ["path", "mode"] },
        },
        {
            "name": "sftp_copy",
            "description": "Copy remote files or directories into a target directory on the same server (recursive, preserves permissions; write operation). Existing targets require overwrite=true.",
            "inputSchema": { "type": "object", "properties": connection_properties(&[
                ("from", "string", "Remote source path, or an array of source paths"),
                ("toDir", "string", "Existing remote directory that receives the copies"),
                ("overwrite", "boolean", "Set true to replace existing targets"),
            ]), "required": ["from", "toDir"] },
        },
        {
            "name": "sftp_move",
            "description": "Move remote files or directories into a target directory on the same server (the sources are removed; write operation). Existing targets require overwrite=true.",
            "inputSchema": { "type": "object", "properties": connection_properties(&[
                ("from", "string", "Remote source path, or an array of source paths"),
                ("toDir", "string", "Existing remote directory that receives the moved items"),
                ("overwrite", "boolean", "Set true to replace existing targets"),
            ]), "required": ["from", "toDir"] },
        },
        {
            "name": "sftp_disk_usage",
            "description": "Report filesystem usage for the mount containing a remote path.",
            "inputSchema": { "type": "object", "properties": connection_properties(&[("path", "string", "Remote path")]), "required": ["path"] },
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> McpState {
        McpState::new(std::env::temp_dir().join("dbx-mcp-test"))
    }

    #[test]
    fn parses_bg_start_markers_with_default_log_fallback() {
        let (pid, log) = parse_bg_start_output(
            "PID=4242\nLOG=/tmp/.dbx-ssh-tasks/bg-1-2.log\n",
            "/tmp/.dbx-ssh-tasks/default.log".to_string(),
        );
        assert_eq!(pid, "4242");
        assert_eq!(log, "/tmp/.dbx-ssh-tasks/bg-1-2.log");

        // A remote without echo output (busybox edge) still yields the
        // derived default path.
        let (pid, log) = parse_bg_start_output("", "/tmp/.dbx-ssh-tasks/x.log".to_string());
        assert_eq!(pid, "");
        assert_eq!(log, "/tmp/.dbx-ssh-tasks/x.log");
    }

    #[test]
    fn parses_task_status_running_done_and_missing() {
        let (state, code, alive, tail) = parse_task_status_output(
            "STATE=RUNNING\nPID_ALIVE=yes\n===TAIL===\nstep 1 done\nstep 2 running\n",
        );
        assert_eq!(state, "running");
        assert_eq!(code, None);
        assert_eq!(alive, Some(true));
        assert_eq!(tail, "step 1 done\nstep 2 running\n");

        let (state, code, alive, _tail) = parse_task_status_output(
            "STATE=DONE\nCODE=EXIT_3\nPID_ALIVE=no\n===TAIL===\nEXIT_3\n",
        );
        assert_eq!(state, "done");
        assert_eq!(code, Some(3));
        assert_eq!(alive, Some(false));

        let (state, code, alive, tail) =
            parse_task_status_output("STATE=MISSING\n===TAIL===\n");
        assert_eq!(state, "missing");
        assert_eq!(code, None);
        assert_eq!(alive, None);
        assert_eq!(tail, "");
    }

    #[tokio::test]
    async fn run_bg_respects_read_only_gate() {
        let mut state = state();
        state.global_read_only = true;
        let error = state
            .call_tool(
                "ssh_run_bg",
                &json!({
                    "host": "example.test",
                    "username": "op",
                    "command": "sleep 30"
                }),
                None,
            )
            .await
            .unwrap_err();
        assert!(
            error.contains("read-only"),
            "expected read-only refusal, got: {error}"
        );
    }

    #[tokio::test]
    async fn inline_dial_inherits_registered_read_only_gate() {
        let state = state();
        // All credential values in tests are assembled at runtime — never
        // real credentials, never literals in source.
        let stored = StoredConnection::from_lifecycle_params(&json!({
            "connection": {
                "id": "conn-ro",
                "host": "prod.example.test",
                "port": 2222,
                "username": "deploy",
                "password": format!("pw-{}", uuid::Uuid::new_v4()),
                "read_only": true,
            }
        }))
        .unwrap();
        assert!(stored.read_only, "lifecycle payload must carry read_only");
        state
            .dbx_connections
            .write()
            .await
            .insert("conn-ro".to_string(), stored);
        // Same endpoint re-dialed inline (no connectionId, host case-insensitive):
        // the read-only gate still applies to write tools — the error fires
        // before run_tool, so no localPath is needed to distinguish it from
        // a plain validation error.
        let error = state
            .call_tool(
                "sftp_upload",
                &json!({
                    "host": "PROD.example.test",
                    "port": 2222,
                    "username": "deploy",
                    "remotePath": "/tmp/x",
                }),
                None,
            )
            .await
            .unwrap_err();
        assert!(
            error.contains("read-only"),
            "expected read-only refusal for inline dial of a registered host, got: {error}"
        );
        // A different username or host is not covered by that registration.
        for identity in [
            json!({"host": "prod.example.test", "port": 2222, "username": "other"}),
            json!({"host": "other.example.test", "username": "deploy"}),
        ] {
            let error = state
                .call_tool("sftp_upload", &identity, None)
                .await
                .unwrap_err();
            assert!(
                !error.contains("read-only"),
                "expected non-matching identity to pass the gate, got: {error}"
            );
        }
    }

    #[tokio::test]
    async fn sudo_allowlist_gates_privileged_tools() {
        let state = state();
        let stored = StoredConnection::from_lifecycle_params(&json!({
            "connection": {
                "id": "conn-sudo-list",
                "host": "web.example.test",
                "port": 22,
                "username": "deploy",
                "password": format!("pw-{}", uuid::Uuid::new_v4()),
                "external_config": {
                    "sudo_whitelist": "systemctl restart nginx\ndocker restart *"
                },
            }
        }))
        .unwrap();
        assert_eq!(stored.sudo_whitelist.len(), 2, "external_config must parse");
        state
            .dbx_connections
            .write()
            .await
            .insert("conn-sudo-list".to_string(), stored);

        // ssh_exec_sudo with an unlisted command: refused, patterns listed.
        let error = state
            .call_tool(
                "ssh_exec_sudo",
                &json!({
                    "connectionId": "conn-sudo-list",
                    "command": "systemctl restart mysql",
                }),
                None,
            )
            .await
            .unwrap_err();
        assert!(
            error.contains("not allowed by this connection's whitelist")
                && error.contains("docker restart *"),
            "expected allowlist refusal, got: {error}"
        );

        // A matching command passes the gate and fails later (no dial here).
        let error = state
            .call_tool(
                "ssh_exec_sudo",
                &json!({
                    "connectionId": "conn-sudo-list",
                    "command": "docker restart api",
                }),
                None,
            )
            .await
            .unwrap_err();
        assert!(
            !error.contains("whitelist"),
            "expected matching command to pass the gate, got: {error}"
        );

        // Inline `sudo …` inside plain ssh_exec is gated too.
        let error = state
            .call_tool(
                "ssh_exec",
                &json!({
                    "connectionId": "conn-sudo-list",
                    "command": "sudo systemctl restart mysql",
                }),
                None,
            )
            .await
            .unwrap_err();
        assert!(
            error.contains("not allowed by this connection's whitelist"),
            "expected inline-sudo refusal, got: {error}"
        );

        // Inline identity fallback: re-dialing the same host without a
        // connectionId inherits the whitelist; a different host does not.
        let error = state
            .call_tool(
                "ssh_exec_sudo",
                &json!({
                    "host": "web.example.test",
                    "username": "deploy",
                    "command": "reboot",
                }),
                None,
            )
            .await
            .unwrap_err();
        assert!(
            error.contains("not allowed by this connection's whitelist"),
            "expected identity-fallback refusal, got: {error}"
        );
        let error = state
            .call_tool(
                "ssh_exec_sudo",
                &json!({
                    "host": "other.example.test",
                    "username": "deploy",
                    "command": "reboot",
                }),
                None,
            )
            .await
            .unwrap_err();
        assert!(
            !error.contains("whitelist"),
            "expected other host to skip the allowlist, got: {error}"
        );
    }

    #[tokio::test]
    async fn read_only_tools_respect_sensitive_path_denylist() {
        let mut read_only_state = state();
        read_only_state.global_read_only = true;
        for (name, arguments) in [
            (
                "sftp_read_file",
                json!({"host": "h.test", "username": "op", "path": "/root/.ssh/id_rsa"}),
            ),
            (
                "sftp_list_dir",
                json!({"host": "h.test", "username": "op", "path": "/root/.ssh"}),
            ),
            (
                "sftp_download",
                json!({"host": "h.test", "username": "op", "remotePath": "/etc/shadow",
                       "localPath": "/tmp/dbx-denylist-test"}),
            ),
            (
                "ssh_task_status",
                json!({"host": "h.test", "username": "op", "logPath": "/root/.bash_history"}),
            ),
        ] {
            let error = read_only_state
                .call_tool(name, &arguments, None)
                .await
                .unwrap_err();
            assert!(
                error.contains("sensitive"),
                "expected sensitive-path refusal for {name}, got: {error}"
            );
        }
        // Ordinary inspection paths still pass the gate (they fail later, at
        // the dial).
        let error = read_only_state
            .call_tool(
                "sftp_read_file",
                &json!({"host": "h.test", "username": "op", "path": "/var/log/app.log"}),
                None,
            )
            .await
            .unwrap_err();
        assert!(!error.contains("sensitive"), "got: {error}");
        // On normal connections the denylist does not apply — the operator
        // already grants full access.
        let normal_state = state();
        let error = normal_state
            .call_tool(
                "sftp_read_file",
                &json!({"host": "h.test", "username": "op", "path": "/root/.ssh/id_rsa"}),
                None,
            )
            .await
            .unwrap_err();
        assert!(!error.contains("sensitive"), "got: {error}");
    }

    #[tokio::test]
    async fn exec_whitelist_refuses_sensitive_paths_on_read_only() {
        let mut state = state();
        state.global_read_only = true;
        let error = state
            .call_tool(
                "ssh_exec",
                &json!({
                    "host": "h.test",
                    "username": "op",
                    "command": "cat /root/.ssh/id_rsa",
                }),
                None,
            )
            .await
            .unwrap_err();
        assert!(
            error.contains("not recognized"),
            "expected whitelist refusal for a sensitive path, got: {error}"
        );
    }

    #[tokio::test]
    async fn sftp_download_refuses_sensitive_local_targets() {
        let state = state();
        for local in [
            "/home/u/.bashrc",
            "/home/u/.ssh/authorized_keys",
            "/etc/cron.d/payload",
            "/Library/LaunchDaemons/com.example.payload.plist",
        ] {
            let error = state
                .call_tool(
                    "sftp_download",
                    &json!({
                        "host": "h.test",
                        "username": "op",
                        "remotePath": "/tmp/payload.sh",
                        "localPath": local,
                    }),
                    None,
                )
                .await
                .unwrap_err();
            assert!(
                error.contains("Refusing to write the local sensitive path"),
                "expected local denylist refusal for {local}, got: {error}"
            );
        }
        // Ordinary destinations pass the denylist (they fail later, at the
        // dial).
        let error = state
            .call_tool(
                "sftp_download",
                &json!({
                    "host": "h.test",
                    "username": "op",
                    "remotePath": "/tmp/payload.sh",
                    "localPath": "/tmp/dbx-download-ok/payload.sh",
                }),
                None,
            )
            .await
            .unwrap_err();
        assert!(
            !error.contains("Refusing to write"),
            "expected plain target to pass the denylist, got: {error}"
        );
    }

    #[test]
    fn connection_tools_declare_connection_id() {
        // Strict MCP hosts drop arguments the input schema does not declare,
        // so a missing connectionId makes runInTerminal unreachable from
        // stdio mode before the dispatcher ever reads it. connectionName is
        // declared for the same reason (the L3 registry-name lookup reads
        // it, and strict hosts would silently drop it otherwise).
        let tools = tool_definitions();
        let array = tools.as_array().unwrap();
        for name in [
            "ssh_exec",
            "ssh_exec_sudo",
            "ssh_run_bg",
            "ssh_task_status",
            "ssh_metrics",
            "ssh_test_connection",
            "sftp_list_dir",
            "sftp_upload",
        ] {
            let properties = array
                .iter()
                .find(|tool| tool["name"] == name)
                .unwrap_or_else(|| panic!("tool {name} missing from definitions"))
                ["inputSchema"]["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("tool {name} schema has no properties object"));
            for key in ["connectionId", "connectionName"] {
                let schema = properties
                    .get(key)
                    .unwrap_or_else(|| panic!("tool {name} schema does not declare {key}"));
                assert_eq!(schema["type"], "string", "tool {name} {key} type");
            }
        }
    }

    #[tokio::test]
    async fn run_bg_requires_destructive_confirmation() {
        let state = state();
        let error = state
            .call_tool(
                "ssh_run_bg",
                &json!({
                    "host": "example.test",
                    "username": "op",
                    "command": "rm -rf /"
                }),
                None,
            )
            .await
            .unwrap_err();
        assert!(
            error.contains("destructive"),
            "expected destructive refusal, got: {error}"
        );
    }

    #[test]
    fn pre_exec_transport_errors_are_retryable_post_exec_are_not() {
        assert!(exec::is_pre_exec_transport_error(
            "Failed to open exec channel: Disconnected"
        ));
        assert!(exec::is_pre_exec_transport_error(
            "Failed to start command: broken pipe"
        ));
        // Post-exec failures must NOT look retryable: the command may have
        // already run server-side.
        assert!(!exec::is_pre_exec_transport_error(
            "Timed out waiting for the remote command to finish."
        ));
        assert!(!exec::is_pre_exec_transport_error(
            "sudo exited 1: wrong password"
        ));
    }

    #[tokio::test]
    async fn initialize_and_list_tools_follow_mcp_shape() {
        let state = state();
        let init = state
            .dispatch(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }))
            .await
            .unwrap();
        assert_eq!(init["id"], 1);
        assert_eq!(init["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(init["result"]["capabilities"]["tools"].is_object());

        let list = state
            .dispatch(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }))
            .await
            .unwrap();
        let tools = list["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        for expected in [
            "ssh_exec",
            "ssh_exec_sudo",
            "ssh_run_bg",
            "ssh_task_status",
            "ssh_metrics",
            "ssh_close",
            "ssh_test_connection",
            "ssh_list_known_hosts",
            "ssh_list_connections",
            "ssh_remove_known_host",
            "ssh_quick_sudo_profiles_list",
            "ssh_quick_sudo_profiles_save",
            "ssh_quick_sudo_profiles_delete",
            "sftp_list_dir",
            "sftp_stat",
            "sftp_exists",
            "sftp_pwd",
            "sftp_read_file",
            "sftp_write_file",
            "sftp_mkdir",
            "sftp_remove",
            "sftp_rename",
            "sftp_chmod",
            "sftp_copy",
            "sftp_move",
            "sftp_disk_usage",
            "sftp_upload",
            "sftp_download",
        ] {
            assert!(names.contains(&expected), "missing tool {expected}");
        }
        assert!(tools
            .iter()
            .all(|tool| tool["inputSchema"]["type"] == "object"));

        // The discovery tool takes no arguments (same shape as
        // ssh_list_known_hosts) and points at the reference parameters.
        let discovery = tools
            .iter()
            .find(|t| t["name"] == "ssh_list_connections")
            .unwrap();
        assert!(
            discovery["inputSchema"]["properties"]
                .as_object()
                .unwrap()
                .is_empty(),
            "ssh_list_connections must take no arguments"
        );
        assert!(
            discovery["description"]
                .as_str()
                .unwrap()
                .contains("connectionId"),
            "description must explain how to use the entries"
        );

        // The read tool exposes the offset knob so MCP clients can page
        // through files; the metrics tool advertises the extended dimensions.
        let read = tools
            .iter()
            .find(|t| t["name"] == "sftp_read_file")
            .unwrap();
        let props = read["inputSchema"]["properties"].as_object().unwrap();
        assert!(props.contains_key("offset"), "sftp_read_file lacks offset");
        // Transfer tools expose the local/remote path pair and overwrite knob.
        for tool_name in ["sftp_upload", "sftp_download"] {
            let tool = tools.iter().find(|t| t["name"] == tool_name).unwrap();
            let props = tool["inputSchema"]["properties"].as_object().unwrap();
            assert!(props.contains_key("localPath"), "{tool_name} lacks localPath");
            assert!(props.contains_key("remotePath"), "{tool_name} lacks remotePath");
            assert!(props.contains_key("overwrite"), "{tool_name} lacks overwrite");
        }
        let sudo = tools.iter().find(|t| t["name"] == "ssh_exec_sudo").unwrap();
        let sudo_props = sudo["inputSchema"]["properties"].as_object().unwrap();
        assert!(
            sudo_props.contains_key("quickSudoProfile"),
            "ssh_exec_sudo lacks quickSudoProfile"
        );
        // Both exec tools advertise the terminal routing knob.
        for tool_name in ["ssh_exec", "ssh_exec_sudo"] {
            let tool = tools.iter().find(|t| t["name"] == tool_name).unwrap();
            let props = tool["inputSchema"]["properties"].as_object().unwrap();
            assert!(
                props.contains_key("runInTerminal"),
                "{tool_name} lacks runInTerminal"
            );
        }
        let metrics = tools.iter().find(|t| t["name"] == "ssh_metrics").unwrap();
        assert!(metrics["description"]
            .as_str()
            .unwrap()
            .contains("inode usage"));

        // Notifications produce no response; unknown methods return an error.
        assert!(state
            .dispatch(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await
            .is_none());
        let error = state
            .dispatch(json!({ "jsonrpc": "2.0", "id": 3, "method": "no/such" }))
            .await
            .unwrap();
        assert_eq!(error["error"]["code"], -32000);
    }

    #[tokio::test]
    async fn tool_calls_validate_parameters_before_connecting() {
        let state = state();
        let missing = state
            .dispatch(json!({
                "jsonrpc": "2.0", "id": 9, "method": "tools/call",
                "params": { "name": "sftp_upload", "arguments": { "host": "example.com", "username": "u" } },
            }))
            .await
            .unwrap();
        let text = missing["error"]["message"].as_str().unwrap();
        assert!(text.contains("localPath"), "unexpected error: {text}");

        let no_password = state
            .dispatch(json!({
                "jsonrpc": "2.0", "id": 10, "method": "tools/call",
                "params": { "name": "ssh_exec", "arguments": { "host": "example.com", "username": "u", "command": "true" } },
            }))
            .await
            .unwrap();
        let message = no_password["error"]["message"].as_str().unwrap_or_default();
        assert!(message.contains("password"), "unexpected error: {message}");
    }

    #[tokio::test]
    async fn transfer_tools_validate_the_local_side_before_dialing() {
        let directory = tempfile::tempdir().unwrap();
        let state = McpState::new(directory.path().join("data"));
        let remote = json!({ "host": "203.0.113.1", "username": "u", "password": "p" });
        let merge = |mut base: Value, extra: Value| {
            for (key, value) in extra.as_object().unwrap() {
                base[key.as_str()] = value.clone();
            }
            base
        };

        // A missing local file fails before any connection attempt.
        let missing_file = state
            .run_tool(
                "sftp_upload",
                &merge(
                    remote.clone(),
                    json!({ "localPath": "/no/such/file.bin", "remotePath": "/tmp/x" }),
                ),
                None,
            )
            .await
            .err()
            .unwrap();
        assert!(
            missing_file.contains("Cannot read local file"),
            "unexpected error: {missing_file}"
        );

        // The upload cap follows the configured maxUploadBytes.
        let local_file = directory.path().join("payload.bin");
        std::fs::write(&local_file, vec![0u8; 64]).unwrap();
        state
            .settings_set(&json!({ "maxUploadBytes": 8 }))
            .unwrap();
        let too_big = state
            .run_tool(
                "sftp_upload",
                &merge(
                    remote.clone(),
                    json!({ "localPath": local_file.display().to_string(), "remotePath": "/tmp/x" }),
                ),
                None,
            )
            .await
            .err()
            .unwrap();
        assert!(
            too_big.contains("upload limit"),
            "unexpected error: {too_big}"
        );

        // An existing local target refuses to be replaced without overwrite.
        let existing_target = directory.path().join("already-here.txt");
        std::fs::write(&existing_target, "keep").unwrap();
        let refused = state
            .run_tool(
                "sftp_download",
                &merge(
                    remote,
                    json!({
                        "remotePath": "/tmp/remote.txt",
                        "localPath": existing_target.display().to_string(),
                    }),
                ),
                None,
            )
            .await
            .err()
            .unwrap();
        assert!(
            refused.contains("Local path already exists"),
            "unexpected error: {refused}"
        );
    }

    #[tokio::test]
    async fn quick_sudo_profiles_roundtrip_without_echoing_secrets() {
        let dir = std::env::temp_dir().join(format!(
            "dbx-mcp-profiles-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let state = McpState::new(dir.clone());
        // Test-only secret assembled at runtime (never a real credential).
        let secret = format!("test-{}", uuid::Uuid::new_v4());

        let saved = state
            .run_tool(
                "ssh_quick_sudo_profiles_save",
                &json!({
                    "name": "ops",
                    "sudoPassword": secret,
                    "totpSecret": "JBSWY3DPEHPK3PXP",
                    "authFlowMode": "password_plus_otp",
                    "sudoUsePty": true,
                }),
                None,
            )
            .await
            .unwrap();
        assert_eq!(saved["created"], true);
        assert_eq!(saved["profile"]["sudoPasswordSet"], true);
        assert_eq!(saved["profile"]["totpConfigured"], true);

        let listed = state
            .run_tool("ssh_quick_sudo_profiles_list", &json!({}), None)
            .await
            .unwrap();
        let rendered = listed.to_string();
        assert!(!rendered.contains(&secret), "secret leaked: {rendered}");
        assert!(rendered.contains("\"sudoPasswordSet\":true"));

        // Duplicate names are rejected, unknown references report clearly.
        let duplicate = state
            .run_tool("ssh_quick_sudo_profiles_save", &json!({ "name": "OPS" }), None)
            .await;
        assert!(duplicate.unwrap_err().contains("already in use"));
        let missing = resolve_profile_reference(
            &sudo_profiles::load_store(&dir),
            &json!({ "quickSudoProfile": "ghost" }),
        );
        assert!(missing.unwrap_err().contains("not found"));

        let id = saved["profile"]["id"].as_str().unwrap().to_string();
        let removed = state
            .run_tool("ssh_quick_sudo_profiles_delete", &json!({ "id": id }), None)
            .await
            .unwrap();
        assert_eq!(removed["removed"], true);
        let listed = state
            .run_tool("ssh_quick_sudo_profiles_list", &json!({}), None)
            .await
            .unwrap();
        assert_eq!(listed["profiles"].as_array().unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Builds a saved-connection payload with the given declared sudo source;
    /// returns the connection plus its (runtime-composed) login password.
    /// All credential values in tests are assembled at runtime — never real
    /// credentials, never literals in source.
    fn source_connection(source: &str, profile_ref: &str) -> (StoredConnection, String) {
        let login = format!("login-{}", uuid::Uuid::new_v4());
        let stored = StoredConnection::from_lifecycle_params(&json!({
            "connection": {
                "id": "conn-src",
                "host": "example.com",
                "port": 22,
                "username": "user",
                "password": login,
                "external_config": {
                    "sudo_source": source,
                    "sudo_profile": profile_ref,
                },
            }
        }))
        .unwrap();
        (stored, login)
    }

    #[tokio::test]
    async fn sudo_auth_resolution_follows_declared_source() {
        let dir = std::env::temp_dir().join(format!(
            "dbx-mcp-sudo-src-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let state = McpState::new(dir.clone());
        let profile_secret = format!("profile-{}", uuid::Uuid::new_v4());
        let call_secret = format!("call-{}", uuid::Uuid::new_v4());

        let saved = state
            .run_tool(
                "ssh_quick_sudo_profiles_save",
                &json!({
                    "name": "src-ops",
                    "sudoPassword": profile_secret,
                    "totpSecret": "JBSWY3DPEHPK3PXP",
                }),
                None,
            )
            .await
            .unwrap();
        assert_eq!(saved["created"], true);

        // Global source: the referenced global profile owns the credential
        // source (with TOTP), exactly like the workbench.
        let (stored, _) = source_connection("global", "src-ops");
        let auth = state
            .resolve_sudo_auth(&json!({}), None, Some(stored))
            .await
            .unwrap();
        assert_eq!(auth.password, profile_secret);
        assert!(!auth.totp_secrets.is_empty());

        // Explicit per-call arguments win over the declared source.
        let (stored, _) = source_connection("global", "src-ops");
        let auth = state
            .resolve_sudo_auth(
                &json!({ "sudoPassword": call_secret }),
                None,
                Some(stored),
            )
            .await
            .unwrap();
        assert_eq!(auth.password, call_secret);

        // The per-call quickSudoProfile reference replaces the declared one
        // and is applied even without a saved connection (inline MCP calls).
        let explicit_profile = sudo_profiles::SudoProfile {
            id: "inline".to_string(),
            name: "inline".to_string(),
            sudo_password: call_secret.clone(),
            totp_secret: String::new(),
            auth_flow_mode: "password_only".to_string(),
            password_prompt_hint: String::new(),
            totp_prompt_hint: String::new(),
            sudo_use_pty: false,
            created_at: 0,
            updated_at: 0,
        };
        let (stored, _) = source_connection("global", "src-ops");
        let auth = state
            .resolve_sudo_auth(&json!({}), Some(explicit_profile.clone()), Some(stored))
            .await
            .unwrap();
        assert_eq!(auth.password, call_secret);
        assert!(auth.totp_secrets.is_empty());
        let auth = state
            .resolve_sudo_auth(&json!({}), Some(explicit_profile.clone()), None)
            .await
            .unwrap();
        assert_eq!(auth.password, call_secret);

        // Custom source without a binding: the connection's own sudo config
        // (empty here) degrades to the login password fallback.
        let (stored, login) = source_connection("custom", "");
        let auth = state
            .resolve_sudo_auth(&json!({}), None, Some(stored))
            .await
            .unwrap();
        assert_eq!(auth.password, login);

        // Off refuses like the workbench gate, unless the caller passes
        // explicit credentials.
        let (stored, _) = source_connection("off", "");
        let refused = state
            .resolve_sudo_auth(&json!({}), None, Some(stored))
            .await;
        assert!(refused.unwrap_err().contains("Quick Sudo is disabled"));
        let (stored, _) = source_connection("off", "");
        let auth = state
            .resolve_sudo_auth(
                &json!({ "sudoPassword": call_secret }),
                None,
                Some(stored),
            )
            .await
            .unwrap();
        assert_eq!(auth.password, call_secret);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pool_ids_key_by_identity() {
        let id = connection_pool_id(&json!({ "host": "h", "port": 2222, "username": "u" }));
        assert_eq!(id, "mcp-u@h:2222");
        assert_eq!(
            connection_pool_id(&json!({ "host": "h", "username": "u" })),
            "mcp-u@h:22"
        );
    }

    #[test]
    fn jump_hosts_parse_into_the_chain() {
        let connection = stored_connection_from_arguments(&json!({
            "host": "target", "username": "u", "password": "p",
            "jumpHosts": [
                { "host": "bastion", "port": 2222, "username": "ops", "password": "jp" },
                { "host": "inner", "username": "relay", "authentication": "private-key", "private_key_path": "/k" },
            ],
        }))
        .unwrap();
        assert_eq!(connection.jump_hosts.len(), 2);
        assert_eq!(connection.jump_hosts[0].port, 2222);
        assert_eq!(connection.jump_hosts[1].authentication, "private-key");

        let too_many = json!({
            "host": "t", "username": "u", "password": "p",
            "jumpHosts": [
                { "host": "a", "username": "u", "password": "p" },
                { "host": "b", "username": "u", "password": "p" },
                { "host": "c", "username": "u", "password": "p" },
                { "host": "d", "username": "u", "password": "p" },
            ],
        });
        assert!(stored_connection_from_arguments(&too_many)
            .err()
            .unwrap()
            .contains("At most 3"));

        let missing_password = json!({
            "host": "t", "username": "u", "password": "p",
            "jumpHosts": [{ "host": "bastion", "username": "ops" }],
        });
        assert!(stored_connection_from_arguments(&missing_password).is_err());
    }

    #[test]
    fn connections_map_authentication_sensibly() {
        let by_key = stored_connection_from_arguments(&json!({
            "host": "h", "username": "u", "command": "x",
            "privateKeyPath": "/k", "totpSecret": "123456", "authFlowMode": "password_plus_otp"
        }))
        .unwrap();
        assert_eq!(by_key.authentication, AuthenticationMethod::PrivateKey);
        assert_eq!(by_key.totp_secret, "123456");
        assert!(
            stored_connection_from_arguments(&json!({ "host": "h", "username": "u" })).is_err()
        );
    }

    #[test]
    fn mcp_limits_default_and_clamp_into_ceilings() {
        let defaults = McpLimits::default();
        assert_eq!(defaults.max_read_bytes, 256 * 1024);
        assert_eq!(defaults.max_download_bytes, 1024 * 1024);
        assert_eq!(defaults.local_transfer_root, "");
        assert_eq!(
            McpLimits {
                max_read_bytes: 0,
                max_upload_bytes: u64::MAX,
                max_download_bytes: 0,
                local_transfer_root: String::new(),
            }
            .sanitized(),
            McpLimits {
                max_read_bytes: 1,
                max_upload_bytes: UPLOAD_LIMIT_CEILING,
                max_download_bytes: 1,
                local_transfer_root: String::new(),
            }
        );
        // Values inside the ceilings pass through untouched.
        let inside = McpLimits {
            max_read_bytes: 512 * 1024,
            max_upload_bytes: 64 * 1024 * 1024,
            max_download_bytes: 4 * 1024 * 1024,
            local_transfer_root: String::new(),
        };
        assert_eq!(inside.clone().sanitized(), inside);
    }

    #[test]
    fn local_transfer_root_setting_accepts_absolute_and_empty_only() {
        assert_eq!(validated_transfer_root(&json!("")).unwrap(), "");
        assert_eq!(
            validated_transfer_root(&json!("/tmp/transfers")).unwrap(),
            "/tmp/transfers"
        );
        assert!(validated_transfer_root(&json!("relative/path")).is_err());
        assert!(validated_transfer_root(&json!(42)).is_err());
    }

    #[test]
    fn local_transfer_settings_persist_and_roundtrip() {
        let dir = std::env::temp_dir().join(format!("mcp-limits-test-{}", std::process::id()));
        let path = dir.join("mcp-settings.json");
        let _ = std::fs::remove_file(&path);
        let defaults = McpLimits::load(&path);
        assert_eq!(defaults.local_transfer_root, "");
        let mut updated = defaults;
        updated.local_transfer_root = "/tmp/transfers".to_string();
        updated.save(&path).unwrap();
        let reloaded = McpLimits::load(&path);
        assert_eq!(reloaded.local_transfer_root, "/tmp/transfers");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn local_transfer_gate_confines_paths_to_allowed_roots() {
        let scratch = std::env::temp_dir().join(format!("mcp-gate-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();
        let inside_dir = scratch.join("inside");
        std::fs::create_dir_all(&inside_dir).unwrap();
        std::fs::write(inside_dir.join("a.bin"), b"x").unwrap();
        let inside = std::fs::canonicalize(inside_dir.join("a.bin")).unwrap();
        let roots = vec![std::fs::canonicalize(&inside_dir).unwrap()];
        // Inside the only configured root passes…
        assert!(ensure_local_transfer_allowed_in(&roots, "/x", &inside).is_ok());
        // …anything else (even non-sensitive) is refused…
        let outside = std::fs::canonicalize("/usr").unwrap_or_else(|_| PathBuf::from("/usr"));
        let error = ensure_local_transfer_allowed_in(&roots, "/x", &outside).unwrap_err();
        assert!(error.contains("localTransferRoot"), "{error}");
        // …and the default-roots message names the setting to widen.
        let empty_roots: Vec<PathBuf> = vec![];
        let error = ensure_local_transfer_allowed_in(&empty_roots, "", &inside).unwrap_err();
        assert!(error.contains("outside the allowed transfer roots"), "{error}");
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn local_transfer_gate_blocklist_applies_inside_allowed_roots() {
        let scratch = std::env::temp_dir().join(format!("mcp-gate-bl-{}", std::process::id()));
        let secret_dir = scratch.join(".ssh");
        std::fs::create_dir_all(&secret_dir).unwrap();
        std::fs::write(secret_dir.join("id_rsa"), b"k").unwrap();
        let key = std::fs::canonicalize(secret_dir.join("id_rsa")).unwrap();
        let rc = std::fs::canonicalize(&scratch).unwrap().join(".bashrc");
        let roots = vec![std::fs::canonicalize(&scratch).unwrap()];
        assert!(ensure_local_transfer_allowed_in(&roots, "", &key).is_err());
        assert!(ensure_local_transfer_allowed_in(&roots, "", &rc).is_err());
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn local_transfer_roots_default_to_temp_and_data_dir() {
        let data_dir = std::env::temp_dir().join(format!("mcp-data-{}", std::process::id()));
        std::fs::create_dir_all(&data_dir).unwrap();
        let roots = local_transfer_roots_for("", &data_dir).unwrap();
        let temp = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let data = std::fs::canonicalize(&data_dir).unwrap();
        assert!(roots.contains(&temp), "{roots:?}");
        assert!(roots.contains(&data), "{roots:?}");
        // A configured root narrows the policy to exactly that root…
        let roots = local_transfer_roots_for(data.to_str().unwrap(), &data_dir).unwrap();
        assert_eq!(roots, vec![data.clone()]);
        // …and an unresolvable configured root is an error, not a fallback.
        assert!(local_transfer_roots_for("/nonexistent/mcp-root", &data_dir).is_err());
        let _ = std::fs::remove_dir_all(&data_dir);
    }

    #[test]
    fn mcp_limits_load_falls_back_on_missing_or_corrupt_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mcp-settings.json");
        // Missing file and unparseable content both fall back to defaults.
        assert_eq!(McpLimits::load(&path), McpLimits::default());
        std::fs::write(&path, "not json at all {{{").unwrap();
        assert_eq!(McpLimits::load(&path), McpLimits::default());
        // Partial/garbage fields fall back per-field and out-of-range values
        // are clamped on load.
        std::fs::write(
            &path,
            serde_json::to_string(&json!({
                "maxReadBytes": "bogus",
                "maxUploadBytes": 32 * 1024 * 1024,
                "maxDownloadBytes": 999u64 * 1024 * 1024 * 1024,
            }))
            .unwrap(),
        )
        .unwrap();
        let loaded = McpLimits::load(&path);
        assert_eq!(loaded.max_read_bytes, McpLimits::default().max_read_bytes);
        assert_eq!(loaded.max_upload_bytes, 32 * 1024 * 1024);
        assert_eq!(loaded.max_download_bytes, DOWNLOAD_LIMIT_CEILING);
    }

    #[test]
    fn mcp_limits_persist_roundtrip() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mcp-settings.json");
        let limits = McpLimits {
            max_read_bytes: 128 * 1024,
            max_upload_bytes: 8 * 1024 * 1024,
            max_download_bytes: 2 * 1024 * 1024,
            local_transfer_root: String::new(),
        };
        limits.clone().save(&path).unwrap();
        assert_eq!(McpLimits::load(&path), limits);
        // A fresh state over the same data dir picks the persisted values up
        // (this is how the --mcp stdio process shares the settings).
        let state = McpState::new(directory.path().to_path_buf());
        assert_eq!(state.settings_get(), limits.to_json());
    }

    #[test]
    fn mcp_settings_set_validates_and_persists_partial_updates() {
        let directory = tempfile::tempdir().unwrap();
        let state = McpState::new(directory.path().to_path_buf());

        // Partial update: only the named field changes.
        let updated = state
            .settings_set(&json!({ "maxReadBytes": 64 * 1024 }))
            .unwrap();
        assert_eq!(updated["maxReadBytes"], 64 * 1024);
        assert_eq!(
            updated["maxUploadBytes"],
            McpLimits::default().max_upload_bytes
        );
        assert_eq!(state.settings_get(), updated);

        // Invalid values are rejected without touching the stored settings.
        for bad in [
            json!({ "maxReadBytes": 0 }),
            json!({ "maxReadBytes": READ_LIMIT_CEILING + 1 }),
            json!({ "maxUploadBytes": "big" }),
            json!({ "maxDownloadBytes": -1 }),
            json!({ "localTransferRoot": "relative/root" }),
            json!({ "localTransferRoot": 7 }),
        ] {
            assert!(
                state.settings_set(&bad).is_err(),
                "expected error for {bad}"
            );
        }
        assert_eq!(
            state.settings_get()["maxReadBytes"],
            64 * 1024,
            "rejected updates must not change the settings"
        );

        // The transfer root accepts an absolute path and clears on empty.
        let with_root = state
            .settings_set(&json!({ "localTransferRoot": "/tmp/transfers" }))
            .unwrap();
        assert_eq!(with_root["localTransferRoot"], "/tmp/transfers");
        let cleared = state
            .settings_set(&json!({ "localTransferRoot": "" }))
            .unwrap();
        assert_eq!(cleared["localTransferRoot"], "");

        // The soft ceilings themselves are accepted.
        let ceilings = state
            .settings_set(&json!({
                "maxReadBytes": READ_LIMIT_CEILING,
                "maxUploadBytes": UPLOAD_LIMIT_CEILING,
                "maxDownloadBytes": DOWNLOAD_LIMIT_CEILING,
            }))
            .unwrap();
        assert_eq!(ceilings["maxReadBytes"], READ_LIMIT_CEILING);

        // Persistence: a second state over the same directory reloads the
        // last accepted values.
        let reloaded = McpState::new(directory.path().to_path_buf());
        assert_eq!(reloaded.settings_get(), ceilings);
    }

    #[tokio::test]
    async fn destructive_commands_require_explicit_confirmation() {
        let state = state();
        // Refused without the flag; the refusal happens before credential
        // validation (no "password" complaint), proving the gate ordering.
        let refused = state
            .dispatch(json!({
                "jsonrpc": "2.0", "id": 30, "method": "tools/call",
                "params": { "name": "ssh_exec", "arguments": {
                    "host": "203.0.113.1", "username": "u",
                    "command": "mkfs.ext4 /dev/sda1",
                }},
            }))
            .await
            .unwrap();
        let message = refused["error"]["message"].as_str().unwrap();
        assert!(message.contains("confirmDestructive"), "unexpected: {message}");
        assert!(!message.contains("password"), "gate must fire first: {message}");

        // With the flag the gate passes and the call proceeds to parameter
        // validation (missing password), proving it was not blocked.
        let gated_through = state
            .dispatch(json!({
                "jsonrpc": "2.0", "id": 31, "method": "tools/call",
                "params": { "name": "ssh_exec", "arguments": {
                    "host": "203.0.113.1", "username": "u",
                    "command": "mkfs.ext4 /dev/sda1",
                    "confirmDestructive": true,
                }},
            }))
            .await
            .unwrap();
        let message = gated_through["error"]["message"].as_str().unwrap();
        assert!(message.contains("password"), "unexpected: {message}");

        // The same gate covers the sudo tool, and pipelines leak through
        // chain segments.
        for tool in ["ssh_exec_sudo"] {
            let chained = state
                .dispatch(json!({
                    "jsonrpc": "2.0", "id": 32, "method": "tools/call",
                    "params": { "name": tool, "arguments": {
                        "host": "203.0.113.1", "username": "u",
                        "command": "uptime && shutdown -h now",
                    }},
                }))
                .await
                .unwrap();
            let message = chained["error"]["message"].as_str().unwrap();
            assert!(message.contains("confirmDestructive"), "unexpected: {message}");
        }
    }

    /// In stdio mode (no emitter), `runInTerminal: true` needs a saved DBX
    /// connectionId to forward through the app bridge; without one it is
    /// refused with guidance instead of silently running hidden, and the
    /// read-only/destructive gates keep firing before the routing branch.
    #[tokio::test]
    async fn run_in_terminal_stdio_requires_a_connection_id() {
        let state = state();

        let refused = state
            .run_tool(
                "ssh_exec",
                &json!({ "host": "h", "username": "u", "command": "echo hi", "runInTerminal": true }),
                None,
            )
            .await
            .err()
            .unwrap();
        assert!(
            refused.contains("runInTerminal needs a saved DBX connection"),
            "unexpected: {refused}"
        );
        // L0 self-heal hint appended to the guidance error.
        assert!(
            refused.contains("ssh_list_connections"),
            "expected the discovery-tool hint, got: {refused}"
        );

        // The same refusal covers the sudo tool; runInTerminal absent or
        // false keeps the existing hidden-channel behavior (which then fails
        // on the missing password, proving the route was not taken).
        let sudo = state
            .run_tool(
                "ssh_exec_sudo",
                &json!({ "host": "h", "username": "u", "command": "uptime", "runInTerminal": true }),
                None,
            )
            .await
            .err()
            .unwrap();
        assert!(
            sudo.contains("runInTerminal needs a saved DBX connection"),
            "unexpected: {sudo}"
        );
        let hidden = state
            .run_tool(
                "ssh_exec",
                &json!({ "host": "h", "username": "u", "command": "echo hi", "runInTerminal": false }),
                None,
            )
            .await
            .err()
            .unwrap();
        assert!(
            !hidden.contains("runInTerminal"),
            "false must keep the hidden channel: {hidden}"
        );
    }

    /// L0: with the bridge fallback disabled (hermetic stand-in for "the
    /// bridge is also unavailable"), an unregistered connectionId reports
    /// the full self-heal path instead of the old bare message.
    #[tokio::test]
    async fn unregistered_connection_id_error_carries_selfheal_hints() {
        let mut state = state();
        state.bridge_fallback = false;
        let error = state
            .call_tool("ssh_metrics", &json!({ "connectionId": "ghost" }), None)
            .await
            .unwrap_err();
        assert!(
            error.contains(
                "not registered with this plugin session and the DBX app bridge is unavailable"
            ),
            "unexpected: {error}"
        );
        assert!(
            error.contains("ssh_list_connections") && error.contains("inline credentials"),
            "expected the self-heal hints, got: {error}"
        );
    }

    /// L1: the forward decision reads only the registry — a registered id
    /// stays local, an unregistered id forwards with the original
    /// arguments, local-only tools and runInTerminal routing never forward,
    /// and the kill switch disables the fallback entirely.
    #[tokio::test]
    async fn bridge_forward_plan_decides_by_registry() {
        let mut state = state();
        state.bridge_fallback = true;
        let stored = StoredConnection::from_lifecycle_params(&json!({
            "connection": {
                "id": "conn-fwd",
                "name": "Prod",
                "host": "192.0.2.10",
                "port": 22,
                "username": "ops",
                "password": format!("pw-{}", uuid::Uuid::new_v4()),
            }
        }))
        .unwrap();
        state
            .dbx_connections
            .write()
            .await
            .insert("conn-fwd".to_string(), stored);

        // Registered id: local path owns the call.
        let plan = state
            .bridge_forward_plan(
                "ssh_exec",
                &json!({ "connectionId": "conn-fwd", "command": "uptime" }),
            )
            .await
            .unwrap();
        assert!(plan.is_none(), "registered id must stay local");

        // Unregistered id: forward with the arguments untouched.
        let (id, args) = state
            .bridge_forward_plan(
                "ssh_exec",
                &json!({ "connectionId": "ghost", "command": "uptime" }),
            )
            .await
            .unwrap()
            .expect("unregistered id must forward");
        assert_eq!(id, "ghost");
        assert_eq!(args["command"], "uptime");

        // Local-only tools never forward, even with an unregistered id.
        let plan = state
            .bridge_forward_plan("ssh_close", &json!({ "connectionId": "ghost" }))
            .await
            .unwrap();
        assert!(plan.is_none(), "ssh_close keeps local semantics");

        // Registry name hit stays local (connection() resolves it).
        let plan = state
            .bridge_forward_plan(
                "sftp_list_dir",
                &json!({ "connectionName": "Prod", "path": "/tmp" }),
            )
            .await
            .unwrap();
        assert!(plan.is_none(), "registry name hit stays local");

        // No connection reference: local path.
        let plan = state
            .bridge_forward_plan("ssh_metrics", &json!({}))
            .await
            .unwrap();
        assert!(plan.is_none());

        // runInTerminal keeps its exclusive forward path.
        let plan = state
            .bridge_forward_plan(
                "ssh_exec",
                &json!({ "connectionId": "ghost", "command": "uptime", "runInTerminal": true }),
            )
            .await
            .unwrap();
        assert!(plan.is_none(), "runInTerminal owns its bridge forward");

        // Fallback disabled: never forward.
        state.bridge_fallback = false;
        let plan = state
            .bridge_forward_plan(
                "ssh_exec",
                &json!({ "connectionId": "ghost", "command": "uptime" }),
            )
            .await
            .unwrap();
        assert!(plan.is_none(), "disabled fallback must not forward");
    }

    /// L2: list views expose metadata plus credential PRESENCE flags only —
    /// the runtime-composed secret never appears in any branch, and the
    /// degraded branch carries the start/upgrade note.
    #[test]
    fn connection_list_views_never_echo_credentials() {
        let secret = format!("pw-{}", uuid::Uuid::new_v4());
        let stored = StoredConnection::from_lifecycle_params(&json!({
            "connection": {
                "id": "conn-list",
                "name": "Web",
                "host": "web.example.test",
                "port": 2222,
                "username": "deploy",
                "password": secret,
                "read_only": true,
            }
        }))
        .unwrap();
        assert_eq!(stored.name.as_deref(), Some("Web"));

        // Degraded branch: registry alone + note.
        let degraded = connection_list_result(Err("bridge down".to_string()), &[stored.clone()]);
        assert_eq!(degraded["source"], "session-registry");
        assert!(
            degraded["note"]
                .as_str()
                .unwrap()
                .contains("upgrade the DBX app"),
            "expected the upgrade note: {degraded}"
        );
        let entry = &degraded["connections"][0];
        assert_eq!(entry["id"], "conn-list");
        assert_eq!(entry["name"], "Web");
        assert_eq!(entry["host"], "web.example.test");
        assert_eq!(entry["port"], 2222);
        assert_eq!(entry["username"], "deploy");
        assert_eq!(entry["authentication"], "password");
        assert_eq!(entry["readOnly"], true);
        assert_eq!(entry["passwordSet"], true);
        let rendered = degraded.to_string();
        assert!(!rendered.contains(&secret), "secret leaked: {rendered}");
        assert!(
            !rendered.contains("\"password\":"),
            "raw credential field leaked: {rendered}"
        );

        // Bridge branch: bridge data wins by id, registry-only ids append,
        // and no note is attached.
        let bridge_entry = json!({
            "id": "conn-list", "name": "Web", "host": "web.example.test",
            "port": 2222, "username": "deploy", "authentication": "password",
            "readOnly": false,
        });
        let extra = StoredConnection::from_lifecycle_params(&json!({
            "connection": {
                "id": "conn-extra", "host": "db.example.test", "port": 22,
                "username": "ops", "password": format!("pw-{}", uuid::Uuid::new_v4()),
            }
        }))
        .unwrap();
        let merged = connection_list_result(Ok(vec![bridge_entry]), &[stored, extra]);
        assert_eq!(merged["source"], "dbx-app-bridge");
        assert!(merged.get("note").is_none(), "bridge hit must not degrade");
        let entries = merged["connections"].as_array().unwrap();
        assert_eq!(entries.len(), 2, "bridge entry + registry-only append");
        assert_eq!(entries[0]["id"], "conn-list");
        // Bridge entries carry no credential flags (contract: absent means
        // the app did not send one).
        assert!(entries[0].get("passwordSet").is_none());
        assert_eq!(entries[1]["id"], "conn-extra");
    }

    /// L3 stdio name resolution over the bridge list: unique hit, ambiguous
    /// (both candidates listed with host), and missing.
    #[test]
    fn resolve_name_in_bridge_list_disambiguates() {
        let entries = vec![
            json!({ "id": "a", "name": "Web", "host": "h1" }),
            // Trailing space in the payload name still matches after trim.
            json!({ "id": "b", "name": "Web ", "host": "h2" }),
            json!({ "id": "c", "name": "DB", "host": "h3" }),
        ];
        assert_eq!(resolve_name_in_bridge_list(&entries, "DB").unwrap(), "c");
        let error = resolve_name_in_bridge_list(&entries, "Web").unwrap_err();
        assert!(
            error.contains("ambiguous")
                && error.contains("a")
                && error.contains("b")
                && error.contains("h1")
                && error.contains("h2"),
            "expected candidates in the error: {error}"
        );
        assert!(
            resolve_name_in_bridge_list(&entries, "ghost")
                .unwrap_err()
                .contains("No connection named"),
            "missing name must be a clear error"
        );
    }

    /// L3: lifecycle payloads carry the display name through; absent names
    /// stay None (older hosts, inline dials).
    #[test]
    fn lifecycle_name_roundtrip_and_optional_default() {
        let named = StoredConnection::from_lifecycle_params(&json!({
            "connection": {
                "id": "c1", "name": "  Bastion  ", "host": "h", "port": 22,
                "username": "u", "password": "p",
            }
        }))
        .unwrap();
        assert_eq!(named.name.as_deref(), Some("Bastion"));
        let unnamed = StoredConnection::from_lifecycle_params(&json!({
            "connection": { "id": "c2", "host": "h", "port": 22, "username": "u", "password": "p" }
        }))
        .unwrap();
        assert_eq!(unnamed.name, None);
    }

    /// L3: two same-named registry entries make the reference ambiguous —
    /// the error lists every candidate id + host so the caller can
    /// disambiguate by id.
    #[tokio::test]
    async fn ambiguous_connection_name_lists_candidates() {
        let state = state();
        for (id, host) in [("conn-x", "10.0.0.1"), ("conn-y", "10.0.0.2")] {
            let stored = StoredConnection::from_lifecycle_params(&json!({
                "connection": {
                    "id": id, "name": "Twin", "host": host, "port": 22,
                    "username": "ops",
                    "password": format!("pw-{}", uuid::Uuid::new_v4()),
                }
            }))
            .unwrap();
            state.dbx_connections.write().await.insert(id.to_string(), stored);
        }
        let error = state
            .call_tool("ssh_metrics", &json!({ "connectionName": "Twin" }), None)
            .await
            .unwrap_err();
        assert!(
            error.contains("ambiguous")
                && error.contains("conn-x")
                && error.contains("conn-y")
                && error.contains("10.0.0.1")
                && error.contains("10.0.0.2"),
            "expected every candidate in the error: {error}"
        );
    }

    #[tokio::test]
    async fn read_only_connection_gates_writes_and_unknown_commands() {        let state = McpState::shared(Arc::new(SshRuntime::new(
            std::env::temp_dir().join("dbx-mcp-readonly-test"),
        )));
        let lifecycle = json!({
            "connection": {
                "id": "conn-readonly",
                "name": "Prod bastion",
                "host": "192.0.2.10",
                "port": 22,
                "username": "ops",
                "password": "secret",
                "external_config": { "authentication": "password", "read_only": true },
            },
            "runtime": { "host": "192.0.2.10", "port": 22 },
            "operationId": "op-ro",
        });

        // Write-class tools are rejected on sight.
        for tool in ["sftp_write_file", "ssh_exec_sudo"] {
            let error = state
                .call_dbx_with(
                    &json!({
                        "tool": tool,
                        "arguments": { "command": "uptime", "path": "/tmp/x", "content": "y" },
                        "lifecycle": lifecycle,
                    }),
                    None,
                )
                .await
                .err()
                .expect("write tool must be refused on a read-only connection");
            assert!(error.contains("read-only"), "unexpected: {error}");
        }

        // ssh_exec survives only for provably read-only commands: an
        // unrecognized mutating command is refused by the whitelist (before
        // any dialing), while a destructive pattern is refused outright.
        let unknown = state
            .call_dbx_with(
                &json!({
                    "tool": "ssh_exec",
                    "arguments": { "command": "systemctl restart nginx" },
                    "lifecycle": lifecycle,
                }),
                None,
            )
            .await
            .err()
            .unwrap();
        assert!(unknown.contains("not recognized"), "unexpected: {unknown}");

        let destructive = state
            .call_dbx_with(
                &json!({
                    "tool": "ssh_exec",
                    "arguments": { "command": "rm -rf /etc" },
                    "lifecycle": lifecycle,
                }),
                None,
            )
            .await
            .err()
            .unwrap();
        assert!(destructive.contains("Refused on read-only"), "unexpected: {destructive}");

        // confirmDestructive cannot override a read-only connection.
        let confirmed = state
            .call_dbx_with(
                &json!({
                    "tool": "ssh_exec",
                    "arguments": { "command": "rm -rf /etc", "confirmDestructive": true },
                    "lifecycle": lifecycle,
                }),
                None,
            )
            .await
            .err()
            .unwrap();
        assert!(confirmed.contains("Refused on read-only"), "unexpected: {confirmed}");
    }
}

#[cfg(test)]
mod dbx_bridge_tests {
    use super::*;

    #[tokio::test]
    async fn dbx_bridge_registers_lifecycle_and_dispatches_tools() {
        let state = McpState::shared(Arc::new(SshRuntime::new(
            std::env::temp_dir().join("dbx-mcp-bridge-test"),
        )));
        let lifecycle = json!({
            "connection": {
                "id": "conn-ssh-1",
                "name": "Prod bastion",
                "host": "192.0.2.10",
                "port": 22,
                "username": "ops",
                "password": "secret",
                "external_config": { "authentication": "password", "quick_sudo": true },
                "connection_secrets": { "totp_secret": "JBSWY3DPEHPK3PXP" },
            },
            "runtime": { "host": "192.0.2.10", "port": 22 },
            "operationId": "op-1",
        });

        // A tool that needs no connection runs right away.
        let listed = state
            .call_dbx_with(
                &json!({ "tool": "ssh_list_known_hosts", "arguments": {}, "lifecycle": lifecycle }),
                None,
            )
            .await
            .unwrap();
        assert!(listed["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("knownHosts"));

        // The lifecycle connection is registered: a connection-bound tool now
        // gets past credential resolution (it fails on the missing command
        // parameter, proving connectionId routing works end to end).
        let err = state
            .call_dbx_with(&json!({ "tool": "ssh_exec", "arguments": {} }), None)
            .await
            .err()
            .unwrap();
        assert!(err.contains("command"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn dbx_bridge_requires_complete_lifecycle_payloads() {
        let state = McpState::shared(Arc::new(SshRuntime::new(
            std::env::temp_dir().join("dbx-mcp-bridge-test-2"),
        )));
        let err = state
            .call_dbx_with(
                &json!({
                    "tool": "ssh_exec",
                    "arguments": { "command": "true" },
                    "lifecycle": { "connection": { "id": "x", "username": "u", "password": "p" } },
                }),
                None,
            )
            .await
            .err()
            .unwrap();
        assert!(err.contains("host"), "unexpected error: {err}");
    }

    /// L3: a lifecycle-registered connection is resolvable by its display
    /// name — the call gets past the registry lookup and fails on the
    /// missing tool parameter instead, proving name → connection routing.
    #[tokio::test]
    async fn connection_name_resolves_through_registry() {
        let mut state = McpState::shared(Arc::new(SshRuntime::new(
            std::env::temp_dir().join("dbx-mcp-name-test"),
        )));
        // Hermetic: keep the name-miss path off the real app bridge.
        state.bridge_fallback = false;
        let lifecycle = json!({
            "connection": {
                "id": "conn-named",
                "name": "Prod bastion",
                "host": "192.0.2.10",
                "port": 22,
                "username": "ops",
                "password": "secret",
            },
            "runtime": { "host": "192.0.2.10", "port": 22 },
            "operationId": "op-name",
        });
        let listed = state
            .call_dbx_with(
                &json!({ "tool": "ssh_list_known_hosts", "arguments": {}, "lifecycle": lifecycle }),
                None,
            )
            .await
            .unwrap();
        assert!(listed["content"][0]["text"].as_str().unwrap().contains("knownHosts"));

        // connectionName (exact, as stored) reaches the tool's own parameter
        // validation: the registry lookup passed.
        let err = state
            .call_dbx_with(
                &json!({
                    "tool": "ssh_exec",
                    "arguments": { "connectionName": "Prod bastion" },
                }),
                None,
            )
            .await
            .err()
            .unwrap();
        assert!(err.contains("command"), "unexpected error: {err}");

        // An unknown name gets the self-heal guidance (no bridge available
        // in tests is the same degradation as an older app).
        let err = state
            .call_dbx_with(
                &json!({
                    "tool": "ssh_exec",
                    "arguments": { "connectionName": "ghost", "command": "true" },
                }),
                None,
            )
            .await
            .err()
            .unwrap();
        assert!(
            err.contains("No connection named 'ghost'") && err.contains("ssh_list_connections"),
            "unexpected error: {err}"
        );
    }
}
