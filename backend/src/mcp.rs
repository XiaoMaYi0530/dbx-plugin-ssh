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
use russh::client::Handle;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::FileType;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::sync::{Mutex as AsyncMutex, RwLock as AsyncRwLock};

use crate::exec::{self, AuthFlowMode, Hints, SudoAuth};
use crate::host_key::HostKeyVerifier;
use crate::model::{AuthenticationMethod, JumpHost, StoredConnection};
use crate::sftp_copy;
use crate::ssh::SshClient;
use crate::ssh::SshRuntime;

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
///   `sftp_write_file` call (new; writes were previously uncapped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpLimits {
    pub max_read_bytes: u64,
    pub max_upload_bytes: u64,
    pub max_download_bytes: u64,
}

impl Default for McpLimits {
    fn default() -> Self {
        Self {
            max_read_bytes: 256 * 1024,
            max_upload_bytes: 16 * 1024 * 1024,
            max_download_bytes: 1024 * 1024,
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
        }
        .sanitized()
    }

    pub fn to_json(self) -> Value {
        json!({
            "maxReadBytes": self.max_read_bytes,
            "maxUploadBytes": self.max_upload_bytes,
            "maxDownloadBytes": self.max_download_bytes,
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
    let state = McpState::new(data_dir);
    let stdin = io::stdin();
    let stdout = io::stdout();
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
        let response = runtime.block_on(state.dispatch(request));
        if let Some(response) = response {
            write_response(&stdout, response)?;
        }
    }
    Ok(())
}

fn write_response(mut stdout: &io::Stdout, response: Value) -> io::Result<()> {
    writeln!(stdout, "{response}")?;
    stdout.flush()
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
        limits = limits.sanitized();
        limits.save(&self.limits_path)?;
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
                self.call_tool(&name, &arguments).await
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

    async fn call_tool(&self, name: &str, arguments: &Value) -> Result<Value, String> {
        // Read-only gate: write-class tools must be rejected when the DBX
        // connection they reference was opened read-only, mirroring the
        // workbench `ensure_writable` gate. Inline (standalone --mcp) calls
        // carry no read-only flag, so only registered DBX connections gate.
        if is_write_tool(name) && self.registered_connection_is_read_only(arguments).await {
            return Err(format!(
                "Tool {name} is a write operation and the connection is read-only"
            ));
        }
        let text = self.run_tool(name, arguments).await?;
        Ok(json!({
            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&text).unwrap_or_default() }],
            "isError": false,
        }))
    }

    /// True when the arguments reference a DBX-registered connection that was
    /// registered as read-only through `mcp/call` lifecycle payloads.
    async fn registered_connection_is_read_only(&self, arguments: &Value) -> bool {
        let Some(id) = arguments
            .get("connectionId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            return false;
        };
        self.dbx_connections
            .read()
            .await
            .get(id)
            .map(|connection| connection.read_only)
            .unwrap_or(false)
    }

    async fn run_tool(&self, name: &str, arguments: &Value) -> Result<Value, String> {
        match name {
            "ssh_close" => self.ssh_close(arguments).await,
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
                let result = match name {
                    "ssh_exec" | "ssh_exec_sudo" => self.ssh_exec_tool(name, arguments).await,
                    "ssh_metrics" => {
                        let connection = self.connection(arguments).await?;
                        exec::collect_metrics(&connection).await
                    }
                    "sftp_disk_usage" => {
                        let path = required_str(arguments, "path")?;
                        let connection = self.connection(arguments).await?;
                        let command = format!("df -kP {}", exec::shell_quote(&path));
                        let outcome =
                            exec::exec_plain(&connection, &command, Duration::from_secs(20))
                                .await?;
                        exec::parse_disk_usage(&outcome.output).ok_or_else(|| {
                            format!("Could not parse disk usage: {}", outcome.output)
                        })
                    }
                    other => self.sftp_tool(other, arguments).await,
                };
                // Drop the cached connection on failure so the next call
                // reconnects with fresh credentials instead of reusing a
                // broken transport.
                if result.is_err() {
                    self.drop_connection(arguments).await;
                }
                result
            }
        }
    }

    async fn ssh_exec_tool(&self, name: &str, arguments: &Value) -> Result<Value, String> {
        let command = required_str(arguments, "command")?;
        let timeout_secs = arguments
            .get("timeoutSecs")
            .and_then(Value::as_u64)
            .map(|secs| Duration::from_secs(secs.clamp(5, 300)));
        let connection = self.connection(arguments).await?;
        if name == "ssh_exec_sudo" {
            let auth = sudo_auth(arguments);
            exec::exec_with_sudo(
                &connection,
                &auth,
                command,
                timeout_secs.unwrap_or(exec::SUDO_EXEC_TIMEOUT),
                false,
            )
            .await
            .map(|outcome| json!({ "output": outcome.output, "exitCode": outcome.exit_code }))
        } else {
            exec::exec_plain(
                &connection,
                command,
                timeout_secs.unwrap_or(exec::PLAIN_EXEC_TIMEOUT),
            )
            .await
            .map(|outcome| json!({ "output": outcome.output, "exitCode": outcome.exit_code }))
        }
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
        // DBX bridge calls reference a saved connection by id; inline calls
        // (standalone --mcp mode) derive the pool key from the credentials.
        let explicit_id = arguments
            .get("connectionId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        let pool_id = match explicit_id {
            Some(id) => id.to_string(),
            None => connection_pool_id(arguments),
        };
        {
            let guard = self.connections.read().await;
            if let Some(entry) = guard.get(&pool_id) {
                return Ok(entry.handle.clone());
            }
        }
        let connection = match explicit_id {
            Some(id) => {
                let registered = {
                    let dbx = self.dbx_connections.read().await;
                    dbx.get(id).cloned()
                };
                registered.ok_or_else(|| {
                    format!("Connection {id} is not registered with this plugin session")
                })?
            }
            None => stored_connection_from_arguments(arguments)?,
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
    /// inline with tool arguments.
    pub async fn call_dbx(&self, params: &Value) -> Result<Value, String> {
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
        self.call_tool(&tool, &arguments).await
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

/// Tools that mutate remote state and must be rejected on read-only
/// connections (mirrors the workbench `ensure_writable` / sudo-exec gates).
/// Plain `ssh_exec` stays allowed: it is the non-interactive shell already
/// available to read-only sessions, like the workbench terminal.
fn is_write_tool(name: &str) -> bool {
    matches!(
        name,
        "ssh_exec_sudo"
            | "sftp_write_file"
            | "sftp_mkdir"
            | "sftp_remove"
            | "sftp_rename"
            | "sftp_chmod"
            | "sftp_copy"
            | "sftp_move"
    )
}

fn sudo_auth(arguments: &Value) -> SudoAuth {
    SudoAuth::new(
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
    )
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
        id: connection_pool_id(arguments),
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
        quick_sudo: arguments
            .get("quickSudo")
            .and_then(Value::as_bool)
            .unwrap_or(true),
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
        jump_hosts,
    })
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
        "totpSecret": { "type": "string", "description": "TOTP secret (otpauth:// URI, base32 key, or static code) for 2FA auto-answer" },
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
            "description": "Run a non-interactive remote shell command over SSH. Quick Sudo orchestration is NOT applied; use ssh_exec_sudo for privileged commands.",
            "inputSchema": {
                "type": "object",
                "properties": connection_properties(&[("command", "string", "Shell command to execute"), ("timeoutSecs", "integer", "Execution timeout in seconds (5-300)")]),
                "required": ["command"],
            },
        },
        {
            "name": "ssh_exec_sudo",
            "description": "Run a remote shell command with sudo. The sudo password is piped over stdin and 2FA/TOTP prompts are answered automatically when totpSecret is configured.",
            "inputSchema": {
                "type": "object",
                "properties": connection_properties(&[("command", "string", "Shell command to execute with sudo"), ("timeoutSecs", "integer", "Execution timeout in seconds (5-300)")]),
                "required": ["command"],
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
            "ssh_metrics",
            "ssh_close",
            "ssh_test_connection",
            "ssh_list_known_hosts",
            "ssh_remove_known_host",
            "sftp_list_dir",
            "sftp_stat",
            "sftp_exists",
            "sftp_read_file",
            "sftp_write_file",
            "sftp_mkdir",
            "sftp_remove",
            "sftp_rename",
            "sftp_chmod",
            "sftp_copy",
            "sftp_move",
            "sftp_disk_usage",
        ] {
            assert!(names.contains(&expected), "missing tool {expected}");
        }
        assert!(tools
            .iter()
            .all(|tool| tool["inputSchema"]["type"] == "object"));

        // The read tool exposes the offset knob so MCP clients can page
        // through files; the metrics tool advertises the extended dimensions.
        let read = tools
            .iter()
            .find(|t| t["name"] == "sftp_read_file")
            .unwrap();
        let props = read["inputSchema"]["properties"].as_object().unwrap();
        assert!(props.contains_key("offset"), "sftp_read_file lacks offset");
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
                "params": { "name": "ssh_exec", "arguments": { "host": "example.com", "username": "u" } },
            }))
            .await
            .unwrap();
        let text = missing["error"]["message"].as_str().unwrap();
        assert!(text.contains("command"), "unexpected error: {text}");

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
        assert_eq!(
            McpLimits {
                max_read_bytes: 0,
                max_upload_bytes: u64::MAX,
                max_download_bytes: 0,
            }
            .sanitized(),
            McpLimits {
                max_read_bytes: 1,
                max_upload_bytes: UPLOAD_LIMIT_CEILING,
                max_download_bytes: 1,
            }
        );
        // Values inside the ceilings pass through untouched.
        let inside = McpLimits {
            max_read_bytes: 512 * 1024,
            max_upload_bytes: 64 * 1024 * 1024,
            max_download_bytes: 4 * 1024 * 1024,
        };
        assert_eq!(inside.sanitized(), inside);
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
        };
        limits.save(&path).unwrap();
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
            .call_dbx(
                &json!({ "tool": "ssh_list_known_hosts", "arguments": {}, "lifecycle": lifecycle }),
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
            .call_dbx(&json!({ "tool": "ssh_exec", "arguments": {} }))
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
            .call_dbx(&json!({
                "tool": "ssh_exec",
                "arguments": { "command": "true" },
                "lifecycle": { "connection": { "id": "x", "username": "u", "password": "p" } },
            }))
            .await
            .err()
            .unwrap();
        assert!(err.contains("host"), "unexpected error: {err}");
    }
}
