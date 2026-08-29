use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use dbx_plugin_sdk::{PluginEmitter, PluginError};
use russh::client::{self, AuthResult, Handle};
use russh::keys::agent::{client::AgentClient, AgentIdentity};
use russh::keys::ssh_key::HashAlg;
use russh::keys::{decode_secret_key, key::PrivateKeyWithHashAlg};
use russh::{ChannelMsg, Disconnect, MethodKind};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::FileType;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex, RwLock as AsyncRwLock};
use tokio::time::Instant;
use uuid::Uuid;

use crate::exec::{
    self, AuthFlowMode, ExecOutcome, Hints, SudoAuth, PLAIN_EXEC_TIMEOUT, SUDO_EXEC_TIMEOUT,
};
use crate::host_key::{HostKeyState, HostKeyVerifier};
use crate::model::{
    normalize_remote_path, path_from_sftp_uri, sftp_uri, AuthenticationMethod, SftpEntry,
    StoredConnection, TerminalFrame, TerminalStream, MAX_TRANSFER_SIZE, TERMINAL_REPLAY_LIMIT,
    TRANSFER_CHUNK_SIZE,
};

/// Resolves the Quick Sudo / 2FA orchestration settings for a connection.
fn sudo_auth_for(connection: &StoredConnection) -> SudoAuth {
    SudoAuth::new(
        &connection.sudo_password,
        &connection.password,
        &connection.totp_secret,
        Hints {
            password: exec::sanitize_prompt_hint(&connection.password_prompt_hint),
            totp: exec::sanitize_prompt_hint(&connection.totp_prompt_hint),
            flow_mode: (!connection.auth_flow_mode.is_empty())
                .then(|| AuthFlowMode::parse(&connection.auth_flow_mode)),
        },
    )
}

/// How the next SSH hop is reached: a fresh TCP connection, or a
/// direct-tcpip channel tunneled through an established jump-host handle.
enum DialTarget {
    Tcp((String, u16)),
    JumpStream(russh::ChannelStream<russh::client::Msg>),
}

impl DialTarget {
    async fn through_jump(jump: &Handle<SshClient>, host: &str, port: u16) -> Result<Self, String> {
        let channel = jump
            .channel_open_direct_tcpip(host, u32::from(port), "127.0.0.1", 0)
            .await
            .map_err(|error| format!("Failed to open tunnel to {host}:{port}: {error}"))?;
        Ok(Self::JumpStream(channel.into_stream()))
    }
}

const DIRECTORY_HANDSHAKE_LIMIT: usize = 64 * 1024;
const DIRECTORY_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);
const REMOTE_SHELL_DETECTION_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy)]
pub struct PromptDecision {
    pub accept: bool,
    pub remember: bool,
}

#[derive(Clone, Default)]
pub struct PromptBroker {
    pending: Arc<AsyncMutex<HashMap<String, PendingPrompt>>>,
}

struct PendingPrompt {
    operation_id: String,
    sender: oneshot::Sender<PromptDecision>,
}

impl PromptBroker {
    async fn request(
        &self,
        host: &str,
        port: u16,
        key_type: String,
        fingerprint: String,
        connection_id: &str,
        operation_id: &str,
        emitter: &PluginEmitter,
    ) -> Option<PromptDecision> {
        let challenge_id = Uuid::new_v4().to_string();
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(
            challenge_id.clone(),
            PendingPrompt {
                operation_id: operation_id.to_string(),
                sender,
            },
        );
        if emitter
            .event(
                "connection/challenge",
                json!({
                    "challengeId": challenge_id,
                    "operationId": operation_id,
                    "connectionId": connection_id,
                    "kind": "host-key",
                    "host": host,
                    "port": port,
                    "keyType": key_type,
                    "fingerprint": fingerprint
                }),
            )
            .is_err()
        {
            self.pending.lock().await.remove(&challenge_id);
            return None;
        }
        let result = tokio::time::timeout(Duration::from_secs(300), receiver)
            .await
            .ok()?
            .ok();
        self.pending.lock().await.remove(&challenge_id);
        result
    }

    pub async fn resolve(
        &self,
        challenge_id: &str,
        operation_id: &str,
        decision: PromptDecision,
    ) -> Result<(), String> {
        let pending = self
            .pending
            .lock()
            .await
            .remove(challenge_id)
            .ok_or("Host-key challenge was not found or already resolved")?;
        if pending.operation_id != operation_id {
            return Err("Host-key challenge operation does not match".to_string());
        }
        pending
            .sender
            .send(decision)
            .map_err(|_| "Host-key challenge is no longer waiting".to_string())
    }
}

/// Shared dial deadline. While a host-key challenge waits for the user, the
/// dial timeout budget is suspended so the SSH connect future is not killed
/// underneath the confirmation dialog.
#[derive(Default)]
struct DialDeadline {
    deadline_ms: AtomicU64,
    challenge_pending: AtomicBool,
}

impl DialDeadline {
    fn start(timeout: Duration) -> Arc<Self> {
        let state = Arc::new(Self::default());
        state.deadline_ms.store(
            Self::epoch_ms() + timeout.as_millis() as u64,
            Ordering::SeqCst,
        );
        state
    }

    fn epoch_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_millis() as u64)
            .unwrap_or(0)
    }

    fn enter_challenge(&self, extra: Duration) {
        self.challenge_pending.store(true, Ordering::SeqCst);
        self.deadline_ms.store(
            Self::epoch_ms() + extra.as_millis() as u64,
            Ordering::SeqCst,
        );
    }

    fn exit_challenge(&self, timeout: Duration) {
        self.challenge_pending.store(false, Ordering::SeqCst);
        self.deadline_ms.store(
            Self::epoch_ms() + timeout.as_millis() as u64,
            Ordering::SeqCst,
        );
    }

    fn remaining(&self) -> Option<Duration> {
        let now = Self::epoch_ms();
        let deadline = self.deadline_ms.load(Ordering::SeqCst);
        deadline.checked_sub(now).map(Duration::from_millis)
    }
}

/// Host-key challenges wait up to this long for a decision; the value also
/// caps how far the dial deadline may be pushed out while a challenge runs.
const HOST_KEY_CHALLENGE_WAIT: Duration = Duration::from_secs(300);

pub struct SshClient {
    verifier: Arc<HostKeyVerifier>,
    prompts: PromptBroker,
    /// Event emitter; `None` in MCP stdio mode where stdout belongs to the
    /// JSON-RPC protocol and unknown host keys are trusted on first use.
    emitter: Option<PluginEmitter>,
    auto_trust: bool,
    host: String,
    port: u16,
    connection_id: String,
    operation_id: String,
    dial_deadline: Arc<DialDeadline>,
    connect_timeout: Duration,
}

impl client::Handler for SshClient {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        match self
            .verifier
            .check(&self.host, self.port, server_public_key)
        {
            Ok(HostKeyState::Trusted) => {
                eprintln!(
                    "[ssh-trace] host-key trusted {}:{}, fingerprint ok",
                    self.host, self.port
                );
                Ok(true)
            }
            Ok(HostKeyState::Unknown) => {
                eprintln!(
                    "[ssh-trace] host-key unknown {}:{} (auto_trust={})",
                    self.host, self.port, self.auto_trust
                );
                if self.auto_trust {
                    // Trust-on-first-use for MCP mode: keys land in the same
                    // known_hosts store and changes are still rejected.
                    if let Err(error) =
                        self.verifier
                            .learn(&self.host, self.port, server_public_key)
                    {
                        eprintln!("[ssh-sftp-plugin] failed to record host key: {error}");
                    }
                    return Ok(true);
                }
                let Some(emitter) = self.emitter.as_ref() else {
                    eprintln!("[ssh-trace] host-key unknown and no emitter -> reject");
                    return Ok(false);
                };
                // The user may take arbitrarily long to confirm the
                // fingerprint; that wait must not consume the dial timeout.
                self.dial_deadline
                    .enter_challenge(HOST_KEY_CHALLENGE_WAIT + self.connect_timeout);
                eprintln!("[ssh-trace] host-key challenge raised, waiting for user decision");
                let decision = self
                    .prompts
                    .request(
                        &self.host,
                        self.port,
                        server_public_key.algorithm().to_string(),
                        server_public_key.fingerprint(HashAlg::Sha256).to_string(),
                        &self.connection_id,
                        &self.operation_id,
                        emitter,
                    )
                    .await;
                self.dial_deadline.exit_challenge(self.connect_timeout);
                eprintln!(
                    "[ssh-trace] host-key challenge resolved: present={} accept={:?} remember={:?}",
                    decision.is_some(),
                    decision.as_ref().map(|d| d.accept),
                    decision.as_ref().map(|d| d.remember)
                );
                let Some(decision) = decision else {
                    return Ok(false);
                };
                if !decision.accept {
                    return Ok(false);
                }
                if decision.remember {
                    if let Err(error) =
                        self.verifier
                            .learn(&self.host, self.port, server_public_key)
                    {
                        let _ = emitter.event(
                            "ssh/host-key/notice",
                            json!({ "kind": "learn-failed", "message": error.to_string() }),
                        );
                    }
                }
                Ok(true)
            }
            Err(error) => {
                eprintln!(
                    "[ssh-trace] host-key CHANGED for {}:{}: {error}",
                    self.host, self.port
                );
                if let Some(emitter) = self.emitter.as_ref() {
                    let _ = emitter.event(
                        "ssh/host-key/notice",
                        json!({ "kind": "changed", "message": error.to_string() }),
                    );
                }
                Err(russh::Error::from(error))
            }
        }
    }
}

/// Verdict of a key-exchange-only host-key probe against the known_hosts
/// stores.
#[derive(Debug, Clone, PartialEq)]
enum HostKeyVerdict {
    Trusted,
    Unknown,
    /// A recorded key differs from the presented one; carries the
    /// verifier's explanation (possible man-in-the-middle).
    Changed(String),
}

fn host_key_verdict(check: Result<HostKeyState, std::io::Error>) -> HostKeyVerdict {
    match check {
        Ok(HostKeyState::Trusted) => HostKeyVerdict::Trusted,
        Ok(HostKeyState::Unknown) => HostKeyVerdict::Unknown,
        Err(error) => HostKeyVerdict::Changed(error.to_string()),
    }
}

/// Response payload for `ssh/host-key/check`. Split from the probe path so
/// the state mapping is unit-testable without a server.
fn host_key_check_response(verdict: &HostKeyVerdict, key_type: &str, fingerprint: &str) -> Value {
    let state = match verdict {
        HostKeyVerdict::Trusted => "trusted",
        HostKeyVerdict::Unknown => "unknown",
        HostKeyVerdict::Changed(_) => "changed",
    };
    json!({
        "state": state,
        "keyType": key_type,
        "fingerprint": fingerprint,
    })
}

fn host_key_unreachable_response(error: String) -> Value {
    json!({ "state": "unreachable", "error": error })
}

/// Key-exchange-only probe handler (tiny-rdm's CheckHostKey equivalent):
/// the server key is fingerprinted and compared against the known_hosts
/// stores, then the handshake is aborted with `Ok(false)` so no
/// authentication, challenge, or session ever happens. Deliberately not
/// `SshClient`, whose check_server_key would raise a host-key challenge.
struct HostKeyProbe {
    verifier: Arc<HostKeyVerifier>,
    host: String,
    port: u16,
    seen: Arc<Mutex<Option<(HostKeyVerdict, String, String)>>>,
}

impl client::Handler for HostKeyProbe {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        let verdict = host_key_verdict(self.verifier.check(
            &self.host,
            self.port,
            server_public_key,
        ));
        if let Ok(mut slot) = self.seen.lock() {
            *slot = Some((
                verdict,
                server_public_key.algorithm().to_string(),
                server_public_key.fingerprint(HashAlg::Sha256).to_string(),
            ));
        }
        // Abort before authentication; russh surfaces this as
        // Error::UnknownKey and tears the transport down on drop.
        Ok(false)
    }
}

enum TerminalCommand {
    Input(Vec<u8>),
    Resize { cols: u32, rows: u32 },
    DirectoryTracking { enabled: bool },
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteShell {
    Bash,
    Zsh,
    Other,
}

#[derive(Default)]
struct DirectoryHandshakeFilter {
    marker: Option<Vec<u8>>,
    buffered: Vec<u8>,
    started_at: Option<Instant>,
    failed: bool,
}

impl DirectoryHandshakeFilter {
    fn begin(&mut self, marker: Vec<u8>) {
        self.marker = Some(marker);
        self.buffered.clear();
        self.started_at = Some(Instant::now());
        self.failed = false;
    }

    fn filter(&mut self, data: &[u8]) -> Option<Vec<u8>> {
        let Some(marker) = self.marker.as_ref() else {
            return Some(data.to_vec());
        };
        self.buffered.extend_from_slice(data);
        if self.buffered.len() > DIRECTORY_HANDSHAKE_LIMIT {
            return self.fail_open();
        }
        let Some(index) = find_bytes(&self.buffered, marker) else {
            return None;
        };
        let result = self.buffered[index + marker.len()..].to_vec();
        self.marker = None;
        self.buffered.clear();
        self.started_at = None;
        (!result.is_empty()).then_some(result)
    }

    fn flush_if_timed_out(&mut self) -> Option<Vec<u8>> {
        if self
            .started_at
            .is_some_and(|started| started.elapsed() >= DIRECTORY_HANDSHAKE_TIMEOUT)
        {
            return self.fail_open();
        }
        None
    }

    fn take_failed(&mut self) -> bool {
        std::mem::take(&mut self.failed)
    }

    fn fail_open(&mut self) -> Option<Vec<u8>> {
        self.marker = None;
        self.started_at = None;
        self.failed = true;
        let buffered = std::mem::take(&mut self.buffered);
        (!buffered.is_empty()).then_some(buffered)
    }
}

#[derive(Default)]
struct ReplayBuffer {
    frames: VecDeque<TerminalFrame>,
    bytes: usize,
    sequence: u64,
}

impl ReplayBuffer {
    fn push(&mut self, stream: TerminalStream, data: Vec<u8>) -> TerminalFrame {
        self.sequence += 1;
        let frame = TerminalFrame {
            sequence: self.sequence,
            stream,
            data,
        };
        self.bytes += frame.data.len();
        self.frames.push_back(frame.clone());
        while self.bytes > TERMINAL_REPLAY_LIMIT {
            let Some(removed) = self.frames.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(removed.data.len());
        }
        frame
    }

    fn after(&self, sequence: u64) -> Vec<TerminalFrame> {
        self.frames
            .iter()
            .filter(|frame| frame.sequence > sequence)
            .cloned()
            .collect()
    }

    fn first_sequence(&self) -> u64 {
        self.frames
            .front()
            .map(|frame| frame.sequence)
            .unwrap_or(self.sequence.saturating_add(1))
    }
}

struct SessionEntry {
    connection_id: String,
    workbench_id: String,
    read_only: bool,
    keepalive_interval_secs: u64,
    connected: AtomicBool,
    /// Unix seconds when the session was opened (for `ssh/sessions/list`).
    created_at_secs: u64,
    handle: Arc<Handle<SshClient>>,
    /// Open jump-host connections that carry this session's target tunnel;
    /// kept alive alongside the target handle.
    jump_chain: Vec<Arc<Handle<SshClient>>>,
    /// Live Quick Sudo / 2FA orchestration, updatable at runtime through
    /// `ssh/settings/set` and shared by the terminal and exec paths.
    orchestration: Arc<RwLock<SudoAuth>>,
    terminal_tx: mpsc::Sender<TerminalCommand>,
    replay: Arc<AsyncMutex<ReplayBuffer>>,
    sftp: AsyncMutex<Option<Arc<AsyncMutex<SftpSession>>>>,
}

struct UploadState {
    session_id: String,
    remote_path: String,
    expected_size: u64,
    received: u64,
    local_path: PathBuf,
    file: std::fs::File,
}

#[derive(Clone)]
struct DownloadState {
    session_id: String,
    remote_path: String,
    file_name: String,
    size: u64,
    next_offset: u64,
}

struct FinishingUpload {
    session_id: String,
    remote_path: String,
    size: u64,
    transferred: Arc<AtomicU64>,
    cancelled: Arc<AtomicBool>,
}

/// Background `sudo -nv` refresh loop keeping a connection's sudo timestamp
/// alive, ported from tiny-rdm's sudoExecService keepalive.
struct SudoKeepalive {
    #[allow(dead_code)]
    handle: Arc<Handle<SshClient>>,
    task: tokio::task::JoinHandle<()>,
}

/// Last collected metrics snapshot for one session, timestamped with the
/// collection moment. Mirrors tiny-rdm's `GetLastSnapshot`: the workbench can
/// render instantly from the previous sample before a fresh poll lands.
struct CachedMetrics {
    value: Value,
    collected_at: u64,
}

/// Unix seconds for the metrics cache timestamps.
fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Builds the response served from the snapshot cache: the stored fields plus
/// a `cachedAt` marker (Unix seconds) so callers can show the age of the data.
/// The stored snapshot itself is never mutated.
fn cached_metrics_payload(snapshot: &Value, collected_at: u64) -> Value {
    let mut payload = snapshot.clone();
    if let Some(object) = payload.as_object_mut() {
        object.insert("cachedAt".to_string(), json!(collected_at));
    }
    payload
}

/// One row of `ssh/sessions/list`. Pure so tests can exercise the payload
/// shape without a live SSH connection.
fn session_info_payload(
    session_id: &str,
    connection_id: &str,
    workbench_id: &str,
    read_only: bool,
    connected: bool,
    sudo_keepalive: bool,
    created_at_secs: u64,
    auth_method: &str,
) -> Value {
    json!({
        "sessionId": session_id,
        "connectionId": connection_id,
        "workbenchId": workbench_id,
        "readOnly": read_only,
        "connected": connected,
        "sudoKeepalive": sudo_keepalive,
        "createdAt": created_at_secs,
        "authMethod": auth_method,
    })
}

pub struct SshRuntime {
    connections: RwLock<HashMap<String, StoredConnection>>,
    sessions: Arc<AsyncRwLock<HashMap<String, Arc<SessionEntry>>>>,
    uploads: Mutex<HashMap<String, UploadState>>,
    finishing_uploads: Mutex<HashMap<String, FinishingUpload>>,
    downloads: Mutex<HashMap<String, DownloadState>>,
    transfer_history: Mutex<VecDeque<Value>>,
    sudo_keepalive: Arc<Mutex<HashMap<String, SudoKeepalive>>>,
    /// Last metrics snapshot per session, in-memory only (see `CachedMetrics`).
    metrics_cache: Mutex<HashMap<String, CachedMetrics>>,
    /// In-flight remote command executions, cancellable by exec id.
    exec_tasks: Mutex<HashMap<String, tokio::task::AbortHandle>>,
    /// Trust-on-first-use for unknown host keys (MCP stdio mode).
    auto_trust: bool,
    pub prompts: PromptBroker,
    data_dir: PathBuf,
    known_hosts_path: PathBuf,
    transfer_dir: PathBuf,
}

impl SshRuntime {
    pub fn new(data_dir: PathBuf) -> Self {
        let transfer_dir = data_dir.join("transfers");
        let known_hosts_path = data_dir.join("known_hosts");
        let _ = std::fs::create_dir_all(&transfer_dir);
        Self {
            connections: RwLock::new(HashMap::new()),
            sessions: Arc::new(AsyncRwLock::new(HashMap::new())),
            uploads: Mutex::new(HashMap::new()),
            finishing_uploads: Mutex::new(HashMap::new()),
            downloads: Mutex::new(HashMap::new()),
            transfer_history: Mutex::new(VecDeque::new()),
            sudo_keepalive: Arc::new(Mutex::new(HashMap::new())),
            metrics_cache: Mutex::new(HashMap::new()),
            exec_tasks: Mutex::new(HashMap::new()),
            auto_trust: false,
            prompts: PromptBroker::default(),
            data_dir,
            known_hosts_path,
            transfer_dir,
        }
    }

    /// Root directory holding plugin-owned state (known_hosts, transfers,
    /// mcp-settings.json).
    pub fn data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }

    pub fn store_connection(&self, connection: StoredConnection) -> Result<(), String> {
        self.connections
            .write()
            .map_err(|_| "Connection registry is poisoned".to_string())?
            .insert(connection.id.clone(), connection);
        Ok(())
    }

    pub async fn disconnect_connection(&self, connection_id: &str) -> Result<(), String> {
        self.connections
            .write()
            .map_err(|_| "Connection registry is poisoned".to_string())?
            .remove(connection_id);
        let session_ids = self
            .sessions
            .read()
            .await
            .iter()
            .filter(|(_, session)| session.connection_id == connection_id)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for session_id in session_ids {
            let _ = self.close_session(&session_id).await;
        }
        self.stop_sudo_keepalive(connection_id).await;
        Ok(())
    }

    pub async fn open_session(
        &self,
        connection_id: &str,
        workbench_id: &str,
        cols: u32,
        rows: u32,
        operation_id: &str,
        emitter: PluginEmitter,
    ) -> Result<Value, String> {
        let connection = self
            .connections
            .read()
            .map_err(|_| "Connection registry is poisoned".to_string())?
            .get(connection_id)
            .cloned()
            .ok_or("Connection is not active; reopen it from DBX")?;
        eprintln!(
            "[ssh-trace] open_session connection_id={connection_id} workbench_id={workbench_id} -> {}:{} auth={:?}",
            connection.host, connection.port, connection.authentication
        );
        let (handle, jump_chain) = self
            .connect_authenticated(&connection, operation_id, Some(emitter.clone()))
            .await?;
        eprintln!("[ssh-trace] open_session dial+auth ok");
        let handle = Arc::new(handle);
        let jump_chain = jump_chain.into_iter().map(Arc::new).collect::<Vec<_>>();
        let remote_shell = detect_remote_shell(&handle).await;
        let directory_tracking_supported = remote_shell.supports_directory_tracking();
        let mut channel = handle
            .channel_open_session()
            .await
            .map_err(|error| format!("Failed to open SSH terminal channel: {error}"))?;
        channel
            .request_pty(true, "xterm-256color", cols.max(1), rows.max(1), 0, 0, &[])
            .await
            .map_err(|error| format!("Failed to request SSH PTY: {error}"))?;
        channel
            .request_shell(true)
            .await
            .map_err(|error| format!("Failed to start SSH shell: {error}"))?;

        let session_id = Uuid::new_v4().to_string();
        let (terminal_tx, mut terminal_rx) = mpsc::channel(256);
        let replay = Arc::new(AsyncMutex::new(ReplayBuffer::default()));
        let orchestration = Arc::new(RwLock::new(sudo_auth_for(&connection)));
        // In-terminal Quick Sudo: answers sudo password / 2FA prompts while
        // the user keeps typing normal commands (ported from tiny-rdm).
        let mut auto_sudo = (connection.quick_sudo && !connection.read_only)
            .then(|| exec::TerminalAutoSudo::new(orchestration.clone()))
            .filter(exec::TerminalAutoSudo::is_useful);
        let entry = Arc::new(SessionEntry {
            connection_id: connection.id.clone(),
            workbench_id: workbench_id.to_string(),
            read_only: connection.read_only,
            keepalive_interval_secs: connection.keepalive_interval_secs,
            connected: AtomicBool::new(true),
            created_at_secs: unix_now_secs(),
            handle,
            jump_chain,
            orchestration: orchestration.clone(),
            terminal_tx,
            replay: replay.clone(),
            sftp: AsyncMutex::new(None),
        });
        self.sessions
            .write()
            .await
            .insert(session_id.clone(), entry.clone());

        let task_id = session_id.clone();
        let directory_marker_id = session_id.clone();
        let sessions = self.sessions.clone();
        tokio::spawn(async move {
            let mut directory_filter = DirectoryHandshakeFilter::default();
            let mut directory_tracking_enabled = false;
            let mut directory_timeout = tokio::time::interval(Duration::from_millis(250));
            directory_timeout.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = directory_timeout.tick() => {
                        if let Some(data) = directory_filter.flush_if_timed_out() {
                            publish_terminal(&task_id, TerminalStream::Stdout, data, &replay, &emitter).await;
                        }
                        if directory_filter.take_failed() {
                            directory_tracking_enabled = false;
                            publish_terminal(
                                &task_id,
                                TerminalStream::State,
                                b"directory-tracking-unavailable".to_vec(),
                                &replay,
                                &emitter,
                            ).await;
                        }
                    }
                    command = terminal_rx.recv() => match command {
                        Some(TerminalCommand::Input(data)) => {
                            if channel.data(&data[..]).await.is_err() { break; }
                        }
                        Some(TerminalCommand::Resize { cols, rows }) => {
                            let _ = channel.window_change(cols.max(1), rows.max(1), 0, 0).await;
                        }
                        Some(TerminalCommand::DirectoryTracking { enabled }) => {
                            if remote_shell == RemoteShell::Other || directory_tracking_enabled == enabled {
                                continue;
                            }
                            directory_tracking_enabled = enabled;
                            directory_filter.begin(directory_tracking_marker(&directory_marker_id));
                            publish_terminal(
                                &task_id,
                                TerminalStream::Stdout,
                                b"\r\x1b[2K".to_vec(),
                                &replay,
                                &emitter,
                            ).await;
                            let script = directory_tracking_script(enabled, &directory_marker_id, remote_shell);
                            if channel.data(script.as_bytes()).await.is_err() { break; }
                        }
                        Some(TerminalCommand::Close) | None => {
                            let _ = channel.close().await;
                            break;
                        }
                    },
                    message = channel.wait() => {
                        let (data, stream) = match message {
                            Some(ChannelMsg::Data { data }) => (data.to_vec(), TerminalStream::Stdout),
                            Some(ChannelMsg::ExtendedData { data, .. }) => (data.to_vec(), TerminalStream::Stderr),
                            Some(ChannelMsg::Eof | ChannelMsg::Close) | None => break,
                            _ => continue,
                        };
                        if stream == TerminalStream::Stdout {
                            if let Some((kind, answer)) = auto_sudo
                                .as_mut()
                                .and_then(|auto| auto.observe(&String::from_utf8_lossy(&data)))
                            {
                                let mut payload = answer.into_bytes();
                                payload.push(b'\r');
                                if channel.data(&payload[..]).await.is_err() { break; }
                                let _ = emitter.event("ssh/auto-sudo", json!({
                                    "sessionId": task_id,
                                    "kind": if kind == exec::AutoSudoKind::Totp { "otp" } else { "password" },
                                }));
                            }
                        }
                        let Some(data) = directory_filter.filter(&data) else { continue; };
                        publish_terminal(&task_id, stream, data, &replay, &emitter).await;
                        if directory_filter.take_failed() {
                            directory_tracking_enabled = false;
                            publish_terminal(
                                &task_id,
                                TerminalStream::State,
                                b"directory-tracking-unavailable".to_vec(),
                                &replay,
                                &emitter,
                            ).await;
                        }
                    }
                }
            }
            entry.connected.store(false, Ordering::Release);
            publish_terminal(
                &task_id,
                TerminalStream::State,
                b"ssh-transport-disconnected".to_vec(),
                &replay,
                &emitter,
            )
            .await;
            let _ = emitter.event(
                "ssh/session/state",
                json!({ "sessionId": task_id, "state": "disconnected" }),
            );
            sessions.write().await.remove(&task_id);
        });

        Ok(json!({
            "sessionId": session_id,
            "connectionId": connection.id,
            "connected": true,
            "sequence": 0,
            "chunkSize": TRANSFER_CHUNK_SIZE,
            "directoryTrackingSupported": directory_tracking_supported
        }))
    }

    pub async fn test_connection(
        &self,
        connection: &StoredConnection,
        operation_id: &str,
        emitter: PluginEmitter,
    ) -> Result<(), String> {
        let (handle, jumps) = self
            .connect_authenticated(connection, operation_id, Some(emitter))
            .await?;
        handle
            .disconnect(
                Disconnect::ByApplication,
                "DBX SSH connection test complete",
                "English",
            )
            .await
            .map_err(|error| format!("SSH test disconnect failed: {error}"))?;
        for jump in jumps {
            let _ = jump
                .disconnect(
                    Disconnect::ByApplication,
                    "DBX SSH jump connection closed",
                    "English",
                )
                .await;
        }
        Ok(())
    }

    /// Preflight host-key check for a saved connection (tiny-rdm's
    /// CheckHostKey): dials only to key exchange, compares the presented
    /// server key against the known_hosts stores, and aborts the handshake
    /// before any authentication or challenge. Jump chains are not
    /// tunneled through (that would require full jump authentication), so
    /// jump-only targets report `unreachable`. TCP failures and timeouts
    /// are returned as `{state: "unreachable", error}` inside `Ok` so the
    /// frontend can render them; only internal errors (unknown connection,
    /// poisoned registry) are `Err`.
    pub async fn check_host_key(&self, connection_id: &str) -> Result<Value, String> {
        let connection = self
            .connections
            .read()
            .map_err(|_| "Connection registry is poisoned".to_string())?
            .get(connection_id)
            .cloned()
            .ok_or_else(|| {
                format!("Connection {connection_id} is not active; reopen it from DBX")
            })?;
        let config = Arc::new(client::Config {
            nodelay: true,
            keepalive_interval: (connection.keepalive_interval_secs > 0)
                .then(|| Duration::from_secs(connection.keepalive_interval_secs)),
            keepalive_max: 3,
            ..Default::default()
        });
        let probe = HostKeyProbe {
            verifier: Arc::new(HostKeyVerifier::new(self.known_hosts_path.clone())),
            host: connection.host.clone(),
            port: connection.port,
            seen: Arc::new(Mutex::new(None)),
        };
        let seen = probe.seen.clone();
        let timeout = Duration::from_secs(connection.connect_timeout_secs.max(1));
        let connect = client::connect(
            config,
            (connection.runtime_host.as_str(), connection.runtime_port),
            probe,
        );
        let failure = match tokio::time::timeout(timeout, connect).await {
            // The probe always rejects the key, so a handle here would be
            // unexpected; disconnect defensively anyway.
            Ok(Ok(handle)) => {
                let _ = handle
                    .disconnect(
                        Disconnect::ByApplication,
                        "DBX host-key check complete",
                        "English",
                    )
                    .await;
                None
            }
            // Key exchange ran and the probe aborted it: the stashed verdict
            // decides the response regardless of the resulting russh error.
            Ok(Err(error)) => {
                let probed = seen.lock().map(|slot| slot.is_some()).unwrap_or(false);
                (!probed).then(|| {
                    format!(
                        "SSH connection to {}:{} failed: {error}",
                        connection.runtime_host, connection.runtime_port
                    )
                })
            }
            Err(_elapsed) => Some(format!(
                "SSH connection to {}:{} timed out after {} seconds",
                connection.runtime_host,
                connection.runtime_port,
                timeout.as_secs()
            )),
        };
        let outcome = seen.lock().ok().and_then(|mut slot| slot.take());
        Ok(match (outcome, failure) {
            (Some((verdict, key_type, fingerprint)), _) => {
                host_key_check_response(&verdict, &key_type, &fingerprint)
            }
            (None, Some(error)) => host_key_unreachable_response(error),
            (None, None) => host_key_unreachable_response(
                "SSH handshake completed without presenting a host key".to_string(),
            ),
        })
    }

    /// Dials the ProxyJump chain (if any) and returns the authenticated
    /// target handle plus the jump-host handles that must stay alive for the
    /// target tunnel to keep working. Ported from tiny-rdm's dialThroughJump.
    async fn connect_authenticated(
        &self,
        connection: &StoredConnection,
        operation_id: &str,
        emitter: Option<PluginEmitter>,
    ) -> Result<(Handle<SshClient>, Vec<Handle<SshClient>>), String> {
        let mut jump_handles = Vec::new();
        let mut dial = DialTarget::Tcp((connection.runtime_host.clone(), connection.runtime_port));

        for (position, jump) in connection.jump_hosts.iter().enumerate() {
            let jump_connection = jump.to_connection(
                &format!("jump-{}-{}", position + 1, connection.id),
                connection.connect_timeout_secs,
                connection.keepalive_interval_secs,
            );
            let handle = self
                .dial_and_authenticate(&jump_connection, dial, operation_id, emitter.clone())
                .await
                .map_err(|error| {
                    format!(
                        "Jump host #{} ({}:{}) failed: {error}",
                        position + 1,
                        jump.host,
                        jump.port
                    )
                })?;
            // The next hop dials through a direct-tcpip channel on this jump.
            let next = if position + 1 < connection.jump_hosts.len() {
                let target = &connection.jump_hosts[position + 1];
                (target.host.clone(), target.port)
            } else {
                (connection.host.clone(), connection.port)
            };
            dial = DialTarget::through_jump(&handle, &next.0, next.1).await?;
            jump_handles.push(handle);
        }

        let target = self
            .dial_and_authenticate(connection, dial, operation_id, emitter)
            .await?;
        Ok((target, jump_handles))
    }

    async fn dial_and_authenticate(
        &self,
        connection: &StoredConnection,
        dial: DialTarget,
        operation_id: &str,
        emitter: Option<PluginEmitter>,
    ) -> Result<Handle<SshClient>, String> {
        let config = Arc::new(client::Config {
            nodelay: true,
            keepalive_interval: (connection.keepalive_interval_secs > 0)
                .then(|| Duration::from_secs(connection.keepalive_interval_secs)),
            // Match tiny-rdm: after 3 unanswered keepalive probes the
            // connection is declared dead so the workbench can reconnect.
            keepalive_max: 3,
            ..Default::default()
        });
        let verifier = Arc::new(HostKeyVerifier::new(self.known_hosts_path.clone()));
        let timeout = Duration::from_secs(connection.connect_timeout_secs);
        let dial_deadline = DialDeadline::start(timeout);
        let handler = SshClient {
            verifier,
            prompts: self.prompts.clone(),
            emitter,
            auto_trust: self.auto_trust,
            host: connection.host.clone(),
            port: connection.port,
            connection_id: connection.id.clone(),
            operation_id: operation_id.to_string(),
            dial_deadline: dial_deadline.clone(),
            connect_timeout: timeout,
        };
        let timeout_message = || {
            format!(
                "SSH connection timed out after {} seconds",
                connection.connect_timeout_secs
            )
        };
        // The deadline is dynamic: a pending host-key challenge suspends the
        // dial timeout while the user studies the fingerprint dialog.
        let mut session = match dial {
            DialTarget::Tcp(address) => {
                let connect = client::connect(config, (address.0.as_str(), address.1), handler);
                tokio::pin!(connect);
                loop {
                    let remaining = dial_deadline.remaining().ok_or_else(timeout_message)?;
                    match tokio::time::timeout_at(
                        tokio::time::Instant::now() + remaining,
                        &mut connect,
                    )
                    .await
                    {
                        Ok(result) => {
                            break result
                                .map_err(|error| format!("SSH connection failed: {error}"))?
                        }
                        Err(_elapsed) => continue,
                    }
                }
            }
            DialTarget::JumpStream(stream) => {
                let connect = client::connect_stream(config, stream, handler);
                tokio::pin!(connect);
                loop {
                    let remaining = dial_deadline.remaining().ok_or_else(timeout_message)?;
                    match tokio::time::timeout_at(
                        tokio::time::Instant::now() + remaining,
                        &mut connect,
                    )
                    .await
                    {
                        Ok(result) => {
                            break result
                                .map_err(|error| format!("SSH connection failed: {error}"))?
                        }
                        Err(_elapsed) => continue,
                    }
                }
            }
        };

        let none = session
            .authenticate_none(&connection.username)
            .await
            .map_err(|error| format!("SSH auth probe failed: {error}"))?;
        eprintln!(
            "[ssh-trace] tcp+banner ok, auth none success={}",
            none.success()
        );
        if none.success() {
            return Ok(session);
        }
        if connection.authentication == AuthenticationMethod::None {
            return Err("SSH server rejected unauthenticated access".to_string());
        }

        let orchestration = sudo_auth_for(connection);

        match connection.authentication {
            AuthenticationMethod::Password => {
                authenticate_password_or_interactive(
                    &mut session,
                    connection,
                    &orchestration,
                    &none,
                )
                .await?;
            }
            AuthenticationMethod::PrivateKey => {
                authenticate_private_key(&mut session, connection).await?;
            }
            AuthenticationMethod::PrivateKeyPassword => {
                let key_result = authenticate_private_key_result(&mut session, connection).await?;
                if !key_result.success() {
                    authenticate_password_or_interactive(
                        &mut session,
                        connection,
                        &orchestration,
                        &key_result,
                    )
                    .await?;
                }
            }
            AuthenticationMethod::Agent => {
                authenticate_agent(&mut session, connection).await?;
            }
            AuthenticationMethod::None => unreachable!(),
        }

        Ok(session)
    }

    /// Path of the plugin's own known_hosts store.
    pub fn known_hosts_path(&self) -> std::path::PathBuf {
        self.known_hosts_path.clone()
    }

    /// Enables trust-on-first-use for unknown host keys (MCP stdio mode).
    pub fn with_auto_trust_keys(mut self) -> Self {
        self.auto_trust = true;
        self
    }

    /// Connects and authenticates without a plugin emitter; used by the MCP
    /// stdio mode. Returns the target handle plus the jump chain that must be
    /// kept alive for the tunnel.
    pub async fn connect_headless(
        &self,
        connection: &StoredConnection,
    ) -> Result<(Arc<Handle<SshClient>>, Vec<Arc<Handle<SshClient>>>), String> {
        let (handle, jumps) = self.connect_authenticated(connection, "mcp", None).await?;
        Ok((Arc::new(handle), jumps.into_iter().map(Arc::new).collect()))
    }

    pub async fn close_session(&self, session_id: &str) -> Result<(), String> {
        let session = self
            .sessions
            .write()
            .await
            .remove(session_id)
            .ok_or("SSH session was not found")?;
        let _ = session.terminal_tx.send(TerminalCommand::Close).await;
        for jump in &session.jump_chain {
            let _ = jump
                .disconnect(
                    Disconnect::ByApplication,
                    "DBX SSH session closed",
                    "English",
                )
                .await;
        }
        self.cleanup_session_transfers(session_id)?;
        if let Ok(mut cache) = self.metrics_cache.lock() {
            cache.remove(session_id);
        }
        let connection_id = session.connection_id.clone();
        let connection_has_sessions = self
            .sessions
            .read()
            .await
            .values()
            .any(|session| session.connection_id == connection_id);
        if !connection_has_sessions {
            self.stop_sudo_keepalive(&connection_id).await;
        }
        Ok(())
    }

    /// Read-only inventory of the sessions this sidecar currently tracks
    /// (tiny-rdm's `ListSessions`): connection/workbench identity, liveness,
    /// sudo-keepalive state and creation time. No SSH traffic is involved.
    pub async fn list_sessions(&self) -> Value {
        let sessions = self.sessions.read().await;
        let keepalives: std::collections::HashSet<String> = match self.sudo_keepalive.lock() {
            Ok(guard) => guard.keys().cloned().collect(),
            Err(_) => std::collections::HashSet::new(),
        };
        // Read-only auth method name per connection id for the info panel;
        // a poisoned store just means the panel shows the default method.
        let connections = match self.connections.read() {
            Ok(guard) => Some(guard),
            Err(_) => None,
        };
        let mut list: Vec<Value> = sessions
            .iter()
            .map(|(session_id, entry)| {
                let auth_method = connections
                    .as_deref()
                    .and_then(|store| store.get(&entry.connection_id))
                    .map(|connection| connection.authentication.method_name())
                    .unwrap_or("password");
                session_info_payload(
                    session_id,
                    &entry.connection_id,
                    &entry.workbench_id,
                    entry.read_only,
                    entry.connected.load(Ordering::Acquire),
                    keepalives.contains(&entry.connection_id),
                    entry.created_at_secs,
                    auth_method,
                )
            })
            .collect();
        drop(connections);
        drop(sessions);
        // Oldest first, stable by id — mirrors tiny-rdm's deterministic order.
        list.sort_by(|a, b| {
            a["createdAt"]
                .as_u64()
                .cmp(&b["createdAt"].as_u64())
                .then_with(|| a["sessionId"].as_str().cmp(&b["sessionId"].as_str()))
        });
        json!({ "sessions": list })
    }

    pub async fn resize_terminal(
        &self,
        session_id: &str,
        cols: u32,
        rows: u32,
    ) -> Result<(), String> {
        self.session(session_id)
            .await?
            .terminal_tx
            .send(TerminalCommand::Resize { cols, rows })
            .await
            .map_err(|_| "SSH terminal is closed".to_string())
    }

    pub async fn set_directory_tracking(
        &self,
        session_id: &str,
        enabled: bool,
    ) -> Result<(), String> {
        self.session(session_id)
            .await?
            .terminal_tx
            .send(TerminalCommand::DirectoryTracking { enabled })
            .await
            .map_err(|_| "SSH terminal session is closed".to_string())
    }

    pub fn write_terminal(&self, session_id: &str, data: Vec<u8>) -> Result<(), String> {
        let sessions = self.sessions.blocking_read();
        let session = sessions
            .get(session_id)
            .ok_or("SSH session was not found")?;
        session
            .terminal_tx
            .try_send(TerminalCommand::Input(data))
            .map_err(|error| format!("SSH input queue is full or closed: {error}"))
    }

    pub async fn replay_terminal(
        &self,
        session_id: &str,
        after_sequence: u64,
        emitter: &PluginEmitter,
    ) -> Result<Value, String> {
        let session = self.session(session_id).await?;
        let replay = session.replay.lock().await;
        let first_available_sequence = replay.first_sequence();
        let tail_sequence = replay.sequence;
        let frames = replay.after(after_sequence);
        drop(replay);
        for frame in &frames {
            emitter
                .binary(&format!("ssh/terminal/out/{session_id}"), &frame.encode())
                .map_err(plugin_error)?;
        }
        Ok(json!({
            "frameCount": frames.len(),
            "firstAvailableSequence": first_available_sequence,
            "tailSequence": tail_sequence,
            "complete": after_sequence.saturating_add(1) >= first_available_sequence
        }))
    }

    pub async fn attach_session(
        &self,
        connection_id: &str,
        workbench_id: &str,
        after_sequence: u64,
        emitter: &PluginEmitter,
    ) -> Result<Value, String> {
        let session = self
            .sessions
            .read()
            .await
            .iter()
            .find(|(_, session)| {
                session.connection_id == connection_id
                    && session.workbench_id == workbench_id
                    && session.connected.load(Ordering::Acquire)
            })
            .map(|(session_id, session)| (session_id.clone(), session.clone()))
            .ok_or("No live SSH session is attached to this workbench")?;
        let replay = self
            .replay_terminal(&session.0, after_sequence, emitter)
            .await?;
        Ok(json!({
            "sessionId": session.0,
            "connectionId": connection_id,
            "workbenchId": workbench_id,
            "connected": true,
            "sequence": replay.get("tailSequence").and_then(Value::as_u64).unwrap_or(0),
            "replay": replay,
            "chunkSize": TRANSFER_CHUNK_SIZE
        }))
    }

    pub async fn close_workbench(&self, workbench_id: &str) -> Result<(), String> {
        let session_ids = self
            .sessions
            .read()
            .await
            .iter()
            .filter(|(_, session)| session.workbench_id == workbench_id)
            .map(|(session_id, _)| session_id.clone())
            .collect::<Vec<_>>();
        for session_id in session_ids {
            let _ = self.close_session(&session_id).await;
        }
        Ok(())
    }

    async fn session(&self, session_id: &str) -> Result<Arc<SessionEntry>, String> {
        self.sessions
            .read()
            .await
            .get(session_id)
            .cloned()
            .ok_or("SSH session was not found or expired".to_string())
    }

    pub(crate) async fn ensure_writable(&self, session_id: &str) -> Result<(), String> {
        if self.session(session_id).await?.read_only {
            Err("SFTP write operation is disabled by the read-only connection setting".to_string())
        } else {
            Ok(())
        }
    }

    pub(crate) async fn sftp(
        &self,
        session_id: &str,
    ) -> Result<Arc<AsyncMutex<SftpSession>>, String> {
        let session = self.session(session_id).await?;
        let mut current = session.sftp.lock().await;
        if let Some(sftp) = current.as_ref() {
            return Ok(sftp.clone());
        }
        let channel = session
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
                .map_err(sftp_error)?,
        ));
        *current = Some(sftp.clone());
        Ok(sftp)
    }

    pub async fn sftp_home(&self, session_id: &str) -> Result<String, String> {
        let sftp = self.sftp(session_id).await?;
        let home = sftp
            .lock()
            .await
            .canonicalize(".")
            .await
            .map_err(sftp_error)?;
        Ok(home)
    }

    pub async fn session_id_for_connection(&self, connection_id: &str) -> Result<String, String> {
        self.sessions
            .read()
            .await
            .iter()
            .find(|(_, session)| {
                session.connection_id == connection_id && session.connected.load(Ordering::Acquire)
            })
            .map(|(id, _)| id.clone())
            .ok_or("No active SSH session exists for this connection".to_string())
    }

    /// Runs a command on the session's connection, optionally with Quick Sudo
    /// orchestration (password injection plus automatic 2FA/TOTP answers).
    pub async fn exec(
        &self,
        session_id: &str,
        exec_id: Option<&str>,
        command: &str,
        sudo: bool,
        timeout_secs: Option<u64>,
    ) -> Result<Value, String> {
        let session = self.session(session_id).await?;
        if sudo && session.read_only {
            return Err(
                "Sudo execution is disabled by the read-only connection setting".to_string(),
            );
        }
        let timeout = Duration::from_secs(
            timeout_secs
                .unwrap_or(if sudo {
                    SUDO_EXEC_TIMEOUT.as_secs()
                } else {
                    PLAIN_EXEC_TIMEOUT.as_secs()
                })
                .clamp(5, 300),
        );
        let connection = if sudo {
            let connection = self
                .connections
                .read()
                .map_err(|_| "Connection registry is poisoned".to_string())?
                .get(&session.connection_id)
                .cloned()
                .ok_or("Connection is not active; reopen it from DBX".to_string())?;
            if !connection.quick_sudo {
                return Err("Quick Sudo is disabled for this connection".to_string());
            }
            Some(connection)
        } else {
            None
        };
        let orchestration = session
            .orchestration
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        let keepalive_handle = session.handle.clone();
        let connection_id = session.connection_id.clone();
        let keepalive_interval = session.keepalive_interval_secs;
        let handle = session.handle.clone();
        let command = command.to_string();

        let use_pty = connection
            .as_ref()
            .map(|connection| connection.sudo_use_pty)
            .unwrap_or(false);
        let run = async move {
            let outcome = if sudo {
                exec::exec_with_sudo(&handle, &orchestration, &command, timeout, use_pty).await?
            } else {
                exec::exec_plain(&handle, &command, timeout).await?
            };
            Ok(outcome)
        };

        let outcome = match exec_id.filter(|id| !id.is_empty()) {
            Some(exec_id) => {
                let exec_id = exec_id.to_string();
                let task = tokio::spawn(run);
                if let Ok(mut tasks) = self.exec_tasks.lock() {
                    tasks.insert(exec_id.clone(), task.abort_handle());
                }
                let result = task
                    .await
                    .map_err(|join_error| {
                        if join_error.is_cancelled() {
                            "Remote command was cancelled".to_string()
                        } else {
                            "Remote command task failed".to_string()
                        }
                    })
                    .and_then(|inner| inner);
                if let Ok(mut tasks) = self.exec_tasks.lock() {
                    tasks.remove(&exec_id);
                }
                result?
            }
            None => run.await?,
        };
        if sudo {
            self.register_sudo_keepalive(&connection_id, keepalive_handle, keepalive_interval);
        }
        Ok(self.exec_response(&outcome))
    }

    /// Aborts an in-flight `ssh/exec`; ported from tiny-rdm's AbortCommand.
    pub fn cancel_exec(&self, exec_id: &str) -> Result<(), String> {
        let task = self
            .exec_tasks
            .lock()
            .map_err(|_| "Exec registry is poisoned".to_string())?
            .remove(exec_id);
        match task {
            Some(task) => {
                task.abort();
                Ok(())
            }
            None => Err("Remote command was not found or already finished".to_string()),
        }
    }

    fn exec_response(&self, outcome: &ExecOutcome) -> Value {
        json!({
            "success": true,
            "output": outcome.output,
            "exitCode": outcome.exit_code,
        })
    }

    /// Starts a per-connection `sudo -nv` refresh loop after a successful
    /// sudo execution. The loop runs every [`exec::SUDO_KEEPALIVE_INTERVAL`]
    /// (4 minutes), stops itself after
    /// [`exec::SUDO_KEEPALIVE_MAX_FAILURES`] consecutive validation failures,
    /// and is aborted deterministically on disconnect / session cleanup via
    /// [`Self::stop_sudo_keepalive`]. Per connection single-instance: a
    /// repeat registration while one is live is a no-op.
    fn register_sudo_keepalive(
        &self,
        connection_id: &str,
        handle: Arc<Handle<SshClient>>,
        _keepalive_interval_secs: u64,
    ) {
        let mut keepalive = match self.sudo_keepalive.lock() {
            Ok(keepalive) => keepalive,
            Err(_) => return,
        };
        if keepalive.contains_key(connection_id) {
            return;
        }
        let interval = exec::SUDO_KEEPALIVE_INTERVAL;
        let registry = self.sudo_keepalive.clone();
        let id = connection_id.to_string();
        let task_handle = handle.clone();
        let task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut failures = 0_u32;
            loop {
                ticker.tick().await;
                let succeeded = exec::validate_sudo_timestamp(&task_handle).await.is_ok();
                match exec::keepalive_failure_step(failures, succeeded) {
                    Some(next) => {
                        if next != 0 {
                            eprintln!(
                                "[ssh] sudo keepalive for {id}: timestamp validation failed ({next}/{})",
                                exec::SUDO_KEEPALIVE_MAX_FAILURES
                            );
                        }
                        failures = next;
                    }
                    None => {
                        eprintln!(
                            "[ssh] sudo keepalive for {id}: {} consecutive validation failures, stopping (sudo timestamp expired)",
                            exec::SUDO_KEEPALIVE_MAX_FAILURES
                        );
                        if let Ok(mut keepalive) = registry.lock() {
                            keepalive.remove(&id);
                        }
                        break;
                    }
                }
            }
        });
        keepalive.insert(connection_id.to_string(), SudoKeepalive { handle, task });
    }

    async fn stop_sudo_keepalive(&self, connection_id: &str) {
        let entry = self
            .sudo_keepalive
            .lock()
            .ok()
            .and_then(|mut keepalive| keepalive.remove(connection_id));
        if let Some(entry) = entry {
            entry.task.abort();
        }
    }

    pub async fn sftp_list_path(
        &self,
        session_id: &str,
        path: &str,
    ) -> Result<Vec<SftpEntry>, String> {
        let sftp = self.sftp(session_id).await?;
        let path = normalize_remote_path(path)?;
        let entries = sftp.lock().await.read_dir(path).await.map_err(sftp_error)?;
        let mut result = entries
            .map(|entry| {
                let metadata = entry.metadata();
                let kind = match entry.file_type() {
                    FileType::File => "file",
                    FileType::Dir => "directory",
                    FileType::Symlink => "symlink",
                    FileType::Other => "other",
                };
                SftpEntry {
                    name: entry.file_name(),
                    uri: sftp_uri(&entry.path()),
                    kind,
                    size: metadata.size,
                    modified_at: metadata.mtime.map(u64::from),
                    permissions: metadata.permissions.map(format_permissions),
                    content_type: content_type_for_path(&entry.path()),
                }
            })
            .collect::<Vec<_>>();
        result.sort_by(|left, right| {
            let left_dir = left.kind == "directory";
            let right_dir = right.kind == "directory";
            right_dir
                .cmp(&left_dir)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        Ok(result)
    }

    pub async fn sftp_read_path(
        &self,
        session_id: &str,
        path: &str,
        offset: u64,
        max_bytes: usize,
    ) -> Result<(Vec<u8>, bool), String> {
        let sftp = self.sftp(session_id).await?;
        let path = normalize_remote_path(path)?;
        let mut file = sftp.lock().await.open(path).await.map_err(sftp_error)?;
        if offset > 0 {
            use tokio::io::AsyncSeekExt;
            // `SeekFrom::Start` only moves the local read cursor (no fstat
            // round trip); an offset at/after EOF simply yields no data.
            file.seek(std::io::SeekFrom::Start(offset))
                .await
                .map_err(|error| format!("SFTP seek failed: {error}"))?;
        }
        let mut data = Vec::new();
        file.take(max_bytes.saturating_add(1) as u64)
            .read_to_end(&mut data)
            .await
            .map_err(|error| format!("SFTP read failed: {error}"))?;
        let truncated = data.len() > max_bytes;
        data.truncate(max_bytes);
        Ok((data, truncated))
    }

    pub async fn sftp_create_directory(&self, session_id: &str, path: &str) -> Result<(), String> {
        self.ensure_writable(session_id).await?;
        let sftp = self.sftp(session_id).await?;
        let result = sftp
            .lock()
            .await
            .create_dir(normalize_remote_path(path)?)
            .await
            .map_err(sftp_error);
        result
    }

    pub async fn sftp_write_path(
        &self,
        session_id: &str,
        path: &str,
        data: &[u8],
        create: bool,
        overwrite: bool,
    ) -> Result<(), String> {
        self.ensure_writable(session_id).await?;
        if data.len() > 1024 * 1024 {
            return Err(
                "Direct filesystem writes are limited to 1 MiB; use the streaming transfer API"
                    .to_string(),
            );
        }
        let sftp = self.sftp(session_id).await?;
        let path = normalize_remote_path(path)?;
        let exists = sftp.lock().await.metadata(path.clone()).await.is_ok();
        if exists && !overwrite {
            return Err("SFTP target already exists and overwrite is disabled".to_string());
        }
        if !exists && !create {
            return Err("SFTP target does not exist and create is disabled".to_string());
        }
        let task_id = Uuid::new_v4().to_string();
        let (temporary, backup) = remote_transfer_paths(&path, &task_id)?;
        let mut file = sftp
            .lock()
            .await
            .create(temporary.clone())
            .await
            .map_err(sftp_error)?;
        if let Err(error) = file.write_all(data).await {
            drop(file);
            let _ = sftp.lock().await.remove_file(temporary).await;
            return Err(format!("SFTP write failed: {error}"));
        }
        file.flush()
            .await
            .map_err(|error| format!("SFTP write flush failed: {error}"))?;
        drop(file);
        commit_remote_file(&sftp, &temporary, &path, &backup).await
    }

    pub async fn sftp_rename(
        &self,
        session_id: &str,
        source: &str,
        target: &str,
    ) -> Result<(), String> {
        self.ensure_writable(session_id).await?;
        let sftp = self.sftp(session_id).await?;
        let result = sftp
            .lock()
            .await
            .rename(
                normalize_remote_path(source)?,
                normalize_remote_path(target)?,
            )
            .await
            .map_err(sftp_error);
        result
    }

    pub async fn sftp_delete(
        &self,
        session_id: &str,
        path: &str,
        recursive: bool,
    ) -> Result<(), String> {
        self.ensure_writable(session_id).await?;
        let sftp = self.sftp(session_id).await?;
        let path = normalize_remote_path(path)?;
        let metadata = sftp
            .lock()
            .await
            .symlink_metadata(path.clone())
            .await
            .map_err(sftp_error)?;
        if metadata.is_symlink() {
            sftp.lock()
                .await
                .remove_file(path)
                .await
                .map_err(sftp_error)
        } else if metadata.is_dir() {
            if !recursive {
                return sftp.lock().await.remove_dir(path).await.map_err(sftp_error);
            }
            delete_directory_tree(&sftp, path).await
        } else {
            sftp.lock()
                .await
                .remove_file(path)
                .await
                .map_err(sftp_error)
        }
    }

    /// Changes the permission bits of a remote path (`sftp/chmod`).
    pub async fn sftp_chmod(&self, session_id: &str, path: &str, mode: u32) -> Result<(), String> {
        self.ensure_writable(session_id).await?;
        let sftp = self.sftp(session_id).await?;
        let metadata = russh_sftp::protocol::FileAttributes {
            permissions: Some(mode),
            ..Default::default()
        };
        let result = sftp
            .lock()
            .await
            .set_metadata(normalize_remote_path(path)?, metadata)
            .await;
        result.map_err(sftp_error)
    }

    /// Reports filesystem usage for the mount containing `path` via `df -kP`.
    /// Read-only, so it stays available on read-only connections.
    pub async fn sftp_disk_usage(&self, session_id: &str, path: &str) -> Result<Value, String> {
        let session = self.session(session_id).await?;
        let path = normalize_remote_path(path)?;
        let command = format!("df -kP {}", exec::shell_quote(&path));
        let outcome = exec::exec_plain(&session.handle, &command, Duration::from_secs(20)).await?;
        exec::parse_disk_usage(&outcome.output)
            .ok_or_else(|| format!("Could not parse disk usage output: {}", outcome.output))
    }

    /// Collects server metrics (CPU, memory, load, disks, network rates, top
    /// processes) with read-only commands; available even on read-only
    /// connections. With `cached: true`, serves the session's last snapshot
    /// (marked with `cachedAt`) when one exists — fresh collection only runs
    /// on a cache miss; every fresh result backfills the cache.
    pub async fn metrics(&self, session_id: &str, cached: bool) -> Result<Value, String> {
        if cached {
            if let Some(payload) = self.cached_metrics_snapshot(session_id) {
                return Ok(payload);
            }
        }
        let session = self.session(session_id).await?;
        let snapshot = exec::collect_metrics(&session.handle).await?;
        self.store_metrics_snapshot(session_id, &snapshot);
        Ok(snapshot)
    }

    /// Returns the cached snapshot for a session with its `cachedAt` marker,
    /// or `None` when nothing was collected for it yet.
    fn cached_metrics_snapshot(&self, session_id: &str) -> Option<Value> {
        let cache = self
            .metrics_cache
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        cache
            .get(session_id)
            .map(|entry| cached_metrics_payload(&entry.value, entry.collected_at))
    }

    /// Backfills the snapshot cache after a fresh collection.
    fn store_metrics_snapshot(&self, session_id: &str, snapshot: &Value) {
        let mut cache = self
            .metrics_cache
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        cache.insert(
            session_id.to_string(),
            CachedMetrics {
                value: snapshot.clone(),
                collected_at: unix_now_secs(),
            },
        );
    }

    /// Reads the live Quick Sudo / 2FA settings for a session's connection.
    /// Secrets are reported as boolean flags only, never as values.
    pub async fn settings_get(&self, session_id: &str) -> Result<Value, String> {
        let session = self.session(session_id).await?;
        let connection = self
            .connections
            .read()
            .map_err(|_| "Connection registry is poisoned".to_string())?
            .get(&session.connection_id)
            .cloned();
        let auth = session
            .orchestration
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        Ok(json!({
            "quickSudo": connection.as_ref().map(|c| c.quick_sudo).unwrap_or(true),
            "sudoUsePty": connection.as_ref().map(|c| c.sudo_use_pty).unwrap_or(false),
            "sudoPasswordSet": !auth.password.is_empty(),
            "totpConfigured": auth.totp_configured(),
            "authFlowMode": auth.flow_mode.map(flow_mode_name).unwrap_or("password_then_otp"),
            "passwordPromptHint": auth.password_prompt_hint,
            "totpPromptHint": auth.totp_prompt_hint,
        }))
    }

    /// Updates Quick Sudo / 2FA settings at runtime: applies to the stored
    /// connection and every live session of that connection immediately
    /// (terminal auto-answer and exec re-read the shared orchestration).
    /// Values are sidecar-local; reopening the connection from DBX restores
    /// the host-provided configuration.
    pub async fn settings_set(&self, session_id: &str, updates: &Value) -> Result<Value, String> {
        let session = self.session(session_id).await?;
        let connection_id = session.connection_id.clone();
        let login_password = {
            let connections = self
                .connections
                .read()
                .map_err(|_| "Connection registry is poisoned".to_string())?;
            connections
                .get(&connection_id)
                .map(|connection| connection.password.clone())
        };

        let optional_string = |key: &str| {
            updates
                .get(key)
                .and_then(Value::as_str)
                .map(|value| value.trim().to_string())
        };
        // Prompt hints are sanitized (control characters/ANSI stripped,
        // length capped) before they touch the stored connection or any
        // live session; malformed input degrades instead of poisoning the
        // prompt classifier. Flow modes and TOTP secrets keep the
        // connect-time parse fallbacks (AuthFlowMode::parse / parse_totp_secrets).
        let hint_string = |key: &str| {
            updates
                .get(key)
                .and_then(Value::as_str)
                .map(exec::sanitize_prompt_hint)
        };

        // Persist flags and secrets on the stored connection.
        {
            let mut connections = self
                .connections
                .write()
                .map_err(|_| "Connection registry is poisoned".to_string())?;
            if let Some(connection) = connections.get_mut(&connection_id) {
                if let Some(value) = updates.get("quickSudo").and_then(Value::as_bool) {
                    connection.quick_sudo = value;
                }
                if let Some(value) = updates.get("sudoUsePty").and_then(Value::as_bool) {
                    connection.sudo_use_pty = value;
                }
                if let Some(value) = optional_string("sudoPassword") {
                    connection.sudo_password = value;
                }
                if let Some(value) = optional_string("totpSecret") {
                    connection.totp_secret = value;
                }
                if let Some(value) = hint_string("passwordPromptHint") {
                    connection.password_prompt_hint = value;
                }
                if let Some(value) = hint_string("totpPromptHint") {
                    connection.totp_prompt_hint = value;
                }
                if let Some(value) = optional_string("authFlowMode") {
                    connection.auth_flow_mode = value;
                }
            }
        }

        // Apply to every live session of the connection, field by field so
        // in-flight OTP usage bookkeeping survives unrelated updates.
        let sessions = self
            .sessions
            .read()
            .await
            .iter()
            .filter(|(_, entry)| entry.connection_id == connection_id)
            .map(|(_, entry)| entry.orchestration.clone())
            .collect::<Vec<_>>();
        for orchestration in sessions {
            let mut auth = orchestration
                .write()
                .unwrap_or_else(|poison| poison.into_inner());
            if let Some(value) = optional_string("sudoPassword") {
                auth.password = if value.is_empty() {
                    login_password.clone().unwrap_or_default()
                } else {
                    value
                };
            }
            if let Some(value) = optional_string("totpSecret") {
                auth.totp_secrets = exec::parse_totp_secrets(&value);
            }
            if let Some(value) = hint_string("passwordPromptHint") {
                auth.password_prompt_hint = value;
            }
            if let Some(value) = hint_string("totpPromptHint") {
                auth.totp_prompt_hint = value;
            }
            if let Some(value) = optional_string("authFlowMode") {
                auth.flow_mode = (!value.is_empty()).then(|| AuthFlowMode::parse(&value));
            }
        }
        self.settings_get(session_id).await
    }

    pub async fn start_upload(
        &self,
        session_id: String,
        remote_path: String,
        size: u64,
        emitter: &PluginEmitter,
    ) -> Result<Value, String> {
        self.ensure_writable(&session_id).await?;
        if size > MAX_TRANSFER_SIZE {
            return Err(format!(
                "Transfers are limited to {MAX_TRANSFER_SIZE} bytes"
            ));
        }
        if self.active_transfer_count(&session_id)? >= 3 {
            return Err("This SSH session already has three active transfers".to_string());
        }
        let task_id = Uuid::new_v4().to_string();
        let local_path = self.transfer_dir.join(format!("upload-{task_id}.part"));
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&local_path)
            .map_err(|error| format!("Failed to create upload spool file: {error}"))?;
        self.uploads
            .lock()
            .map_err(|_| "Upload registry is poisoned".to_string())?
            .insert(
                task_id.clone(),
                UploadState {
                    session_id: session_id.clone(),
                    remote_path: normalize_remote_path(&remote_path)?,
                    expected_size: size,
                    received: 0,
                    local_path,
                    file,
                },
            );
        let file_name = remote_path.rsplit('/').next().unwrap_or("upload");
        emitter
            .event(
                "sftp/transfer/progress",
                json!({ "taskId": task_id, "sessionId": session_id, "direction": "upload", "fileName": file_name, "transferred": 0, "size": size, "status": "queued" }),
            )
            .map_err(plugin_error)?;
        Ok(
            json!({ "taskId": task_id, "chunkSize": TRANSFER_CHUNK_SIZE, "maxBytes": MAX_TRANSFER_SIZE }),
        )
    }

    pub fn append_upload(
        &self,
        task_id: &str,
        payload: &[u8],
        emitter: &PluginEmitter,
    ) -> Result<(), String> {
        if payload.len() < 8 {
            return Err("Upload chunk is missing its offset".to_string());
        }
        let offset = u64::from_be_bytes(
            payload[..8]
                .try_into()
                .map_err(|_| "Invalid upload offset")?,
        );
        let chunk = &payload[8..];
        if chunk.len() > TRANSFER_CHUNK_SIZE {
            return Err("Upload chunk exceeds the negotiated chunk size".to_string());
        }
        let mut uploads = self
            .uploads
            .lock()
            .map_err(|_| "Upload registry is poisoned".to_string())?;
        let upload = uploads
            .get_mut(task_id)
            .ok_or("Upload task was not found")?;
        if upload.received != offset {
            return Err(format!(
                "Upload offset mismatch: expected {}, received {offset}",
                upload.received
            ));
        }
        if upload.received.saturating_add(chunk.len() as u64) > upload.expected_size {
            return Err("Upload exceeds the declared file size".to_string());
        }
        upload
            .file
            .write_all(chunk)
            .map_err(|error| format!("Failed to spool upload chunk: {error}"))?;
        upload.received = upload.received.saturating_add(chunk.len() as u64);
        emitter
            .event(
                "sftp/transfer/progress",
                json!({ "taskId": task_id, "sessionId": upload.session_id, "direction": "upload", "transferred": upload.received, "size": upload.expected_size, "status": "running" }),
            )
            .map_err(plugin_error)?;
        emitter
            .event(
                "sftp/upload/ack",
                json!({ "taskId": task_id, "offset": offset, "length": chunk.len(), "nextOffset": upload.received }),
            )
            .map_err(plugin_error)
    }

    pub async fn finish_upload(
        &self,
        task_id: &str,
        emitter: &PluginEmitter,
    ) -> Result<Value, String> {
        let upload = self
            .uploads
            .lock()
            .map_err(|_| "Upload registry is poisoned".to_string())?
            .remove(task_id)
            .ok_or("Upload task was not found")?;
        if upload.received != upload.expected_size {
            let _ = std::fs::remove_file(&upload.local_path);
            return Err(format!(
                "Upload is incomplete: expected {}, received {}",
                upload.expected_size, upload.received
            ));
        }
        let UploadState {
            session_id,
            remote_path,
            expected_size,
            received: _,
            local_path,
            file,
        } = upload;
        drop(file);
        let transferred_bytes = Arc::new(AtomicU64::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        self.finishing_uploads
            .lock()
            .map_err(|_| "Finishing upload registry is poisoned".to_string())?
            .insert(
                task_id.to_string(),
                FinishingUpload {
                    session_id: session_id.clone(),
                    remote_path: remote_path.clone(),
                    size: expected_size,
                    transferred: transferred_bytes.clone(),
                    cancelled: cancelled.clone(),
                },
            );
        let result: Result<(), String> = async {
            let sftp = self.sftp(&session_id).await?;
            let (temporary, backup) = remote_transfer_paths(&remote_path, task_id)?;
            let mut source = tokio::fs::File::open(&local_path)
                .await
                .map_err(|error| format!("Failed to open upload spool file: {error}"))?;
            let mut target = sftp
                .lock()
                .await
                .create(temporary.clone())
                .await
                .map_err(sftp_error)?;
            let mut transferred = 0_u64;
            let mut buffer = vec![0_u8; TRANSFER_CHUNK_SIZE];
            loop {
                if cancelled.load(Ordering::Acquire) {
                    drop(target);
                    let _ = sftp.lock().await.remove_file(temporary.clone()).await;
                    return Err("Upload cancelled".to_string());
                }
                let read = source
                    .read(&mut buffer)
                    .await
                    .map_err(|error| format!("Failed to read upload spool file: {error}"))?;
                if read == 0 {
                    break;
                }
                if let Err(error) = target.write_all(&buffer[..read]).await {
                    drop(target);
                    let _ = sftp.lock().await.remove_file(temporary.clone()).await;
                    return Err(format!("SFTP upload failed: {error}"));
                }
                transferred = transferred.saturating_add(read as u64);
                transferred_bytes.store(transferred, Ordering::Release);
                emitter
                    .event(
                        "sftp/transfer/progress",
                        json!({ "taskId": task_id, "sessionId": session_id, "direction": "upload", "transferred": transferred, "size": expected_size, "status": "running" }),
                    )
                    .map_err(plugin_error)?;
            }
            target
                .flush()
                .await
                .map_err(|error| format!("SFTP upload flush failed: {error}"))?;
            drop(target);
            commit_remote_file(&sftp, &temporary, &remote_path, &backup).await
        }
        .await;
        self.finishing_uploads
            .lock()
            .map_err(|_| "Finishing upload registry is poisoned".to_string())?
            .remove(task_id);
        let _ = tokio::fs::remove_file(&local_path).await;
        match result {
            Ok(()) => {
                let task = json!({ "taskId": task_id, "sessionId": session_id, "direction": "upload", "fileName": remote_path.rsplit('/').next().unwrap_or("upload"), "transferred": expected_size, "size": expected_size, "status": "completed" });
                self.record_transfer(task.clone());
                emitter
                    .event("sftp/transfer/progress", task)
                    .map_err(plugin_error)?;
                Ok(json!({ "success": true, "taskId": task_id, "transferred": expected_size }))
            }
            Err(error) => {
                let status = if cancelled.load(Ordering::Acquire) {
                    "cancelled"
                } else {
                    "failed"
                };
                let task = json!({ "taskId": task_id, "sessionId": session_id, "direction": "upload", "fileName": remote_path.rsplit('/').next().unwrap_or("upload"), "transferred": transferred_bytes.load(Ordering::Acquire), "size": expected_size, "status": status, "error": error });
                self.record_transfer(task.clone());
                let _ = emitter.event("sftp/transfer/progress", task);
                Err(error)
            }
        }
    }

    pub async fn start_download(
        &self,
        session_id: &str,
        remote_path: &str,
        emitter: &PluginEmitter,
    ) -> Result<Value, String> {
        let remote_path = normalize_remote_path(remote_path)?;
        if self.active_transfer_count(session_id)? >= 3 {
            return Err("This SSH session already has three active transfers".to_string());
        }
        let sftp = self.sftp(session_id).await?;
        let size = sftp
            .lock()
            .await
            .metadata(remote_path.clone())
            .await
            .map_err(sftp_error)?
            .size
            .unwrap_or(0);
        if size > MAX_TRANSFER_SIZE {
            return Err(format!(
                "Transfers are limited to {MAX_TRANSFER_SIZE} bytes"
            ));
        }
        let file_name = remote_path
            .rsplit('/')
            .next()
            .filter(|value| !value.is_empty())
            .unwrap_or("download")
            .to_string();
        let task_id = Uuid::new_v4().to_string();
        self.downloads
            .lock()
            .map_err(|_| "Download registry is poisoned".to_string())?
            .insert(
                task_id.clone(),
                DownloadState {
                    session_id: session_id.to_string(),
                    remote_path,
                    file_name: file_name.clone(),
                    size,
                    next_offset: 0,
                },
            );
        emitter
            .event(
                "sftp/transfer/progress",
                json!({ "taskId": task_id, "sessionId": session_id, "direction": "download", "transferred": 0, "size": size, "status": "queued" }),
            )
            .map_err(plugin_error)?;
        Ok(
            json!({ "taskId": task_id, "fileName": file_name, "size": size, "chunkSize": TRANSFER_CHUNK_SIZE }),
        )
    }

    pub async fn download_chunk(
        &self,
        task_id: &str,
        offset: u64,
        emitter: &PluginEmitter,
    ) -> Result<Value, String> {
        let download = {
            let downloads = self
                .downloads
                .lock()
                .map_err(|_| "Download registry is poisoned".to_string())?;
            downloads
                .get(task_id)
                .cloned()
                .ok_or("Download task was not found")?
        };
        if offset != download.next_offset {
            return Err(format!(
                "Download offset mismatch: expected {}, received {offset}",
                download.next_offset
            ));
        }
        let sftp = self.sftp(&download.session_id).await?;
        let mut source = sftp
            .lock()
            .await
            .open(download.remote_path.clone())
            .await
            .map_err(sftp_error)?;
        source
            .seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|error| format!("SFTP download seek failed: {error}"))?;
        let remaining = download.size.saturating_sub(offset);
        let requested = remaining.min(TRANSFER_CHUNK_SIZE as u64) as usize;
        let mut chunk = vec![0_u8; requested];
        let length = source
            .read(&mut chunk)
            .await
            .map_err(|error| format!("SFTP download failed: {error}"))?;
        chunk.truncate(length);
        let next_offset = offset.saturating_add(length as u64);
        let mut payload = Vec::with_capacity(8 + length);
        payload.extend_from_slice(&offset.to_be_bytes());
        payload.extend_from_slice(&chunk);
        emitter
            .binary(&format!("sftp/download/{task_id}"), &payload)
            .map_err(plugin_error)?;
        if let Some(current) = self
            .downloads
            .lock()
            .map_err(|_| "Download registry is poisoned".to_string())?
            .get_mut(task_id)
        {
            if current.next_offset != offset {
                return Err("Download task changed while a chunk was in flight".to_string());
            }
            current.next_offset = next_offset;
        }
        emitter
            .event(
                "sftp/transfer/progress",
                json!({ "taskId": task_id, "sessionId": download.session_id, "direction": "download", "transferred": next_offset, "size": download.size, "status": "running" }),
            )
            .map_err(plugin_error)?;
        Ok(
            json!({ "taskId": task_id, "offset": offset, "length": length, "eof": next_offset >= download.size, "fileName": download.file_name }),
        )
    }

    pub fn cancel_transfer(&self, task_id: &str, emitter: &PluginEmitter) -> Result<(), String> {
        let upload = self
            .uploads
            .lock()
            .map_err(|_| "Upload registry is poisoned".to_string())?
            .remove(task_id);
        let download = self
            .downloads
            .lock()
            .map_err(|_| "Download registry is poisoned".to_string())?
            .remove(task_id);
        let finishing = self
            .finishing_uploads
            .lock()
            .map_err(|_| "Finishing upload registry is poisoned".to_string())?
            .get(task_id)
            .map(|upload| {
                upload.cancelled.store(true, Ordering::Release);
                json!({ "taskId": task_id, "sessionId": upload.session_id, "direction": "upload", "fileName": upload.remote_path.rsplit('/').next().unwrap_or("upload"), "size": upload.size, "transferred": upload.transferred.load(Ordering::Acquire), "status": "cancelled" })
            });
        if let Some(upload) = upload.as_ref() {
            let _ = std::fs::remove_file(&upload.local_path);
        }
        if upload.is_none() && download.is_none() && finishing.is_none() {
            return Err("Transfer task was not found".to_string());
        }
        let task = upload
            .as_ref()
            .map(|upload| json!({ "taskId": task_id, "sessionId": upload.session_id, "direction": "upload", "fileName": upload.remote_path.rsplit('/').next().unwrap_or("upload"), "size": upload.expected_size, "transferred": upload.received, "status": "cancelled" }))
            .or_else(|| download.as_ref().map(|download| json!({ "taskId": task_id, "sessionId": download.session_id, "direction": "download", "fileName": download.file_name, "size": download.size, "transferred": download.next_offset, "status": "cancelled" })))
            .or(finishing)
            .expect("a transfer was present");
        self.record_transfer(task.clone());
        emitter
            .event("sftp/transfer/progress", task)
            .map_err(plugin_error)
    }

    pub fn complete_download(&self, task_id: &str, emitter: &PluginEmitter) -> Result<(), String> {
        let download = self
            .downloads
            .lock()
            .map_err(|_| "Download registry is poisoned".to_string())?
            .remove(task_id)
            .ok_or("Download task was not found".to_string())?;
        if download.next_offset < download.size {
            return Err(format!(
                "Download is incomplete: received {} of {} bytes",
                download.next_offset, download.size
            ));
        }
        let task = json!({ "taskId": task_id, "sessionId": download.session_id, "direction": "download", "fileName": download.file_name, "size": download.size, "transferred": download.size, "status": "completed" });
        self.record_transfer(task.clone());
        emitter
            .event("sftp/transfer/progress", task)
            .map_err(plugin_error)?;
        Ok(())
    }

    pub fn transfer_list(&self, session_id: &str) -> Result<Value, String> {
        let uploads = self
            .uploads
            .lock()
            .map_err(|_| "Upload registry is poisoned".to_string())?;
        let downloads = self
            .downloads
            .lock()
            .map_err(|_| "Download registry is poisoned".to_string())?;
        let finishing_uploads = self
            .finishing_uploads
            .lock()
            .map_err(|_| "Finishing upload registry is poisoned".to_string())?;
        let history = self
            .transfer_history
            .lock()
            .map_err(|_| "Transfer history is poisoned".to_string())?;
        let mut tasks = history
            .iter()
            .filter(|task| task.get("sessionId").and_then(Value::as_str) == Some(session_id))
            .cloned()
            .collect::<Vec<_>>();
        tasks.extend(uploads
            .iter()
            .filter(|(_, upload)| upload.session_id == session_id)
            .map(|(task_id, upload)| {
                json!({ "taskId": task_id, "sessionId": session_id, "direction": "upload", "fileName": upload.remote_path.rsplit('/').next().unwrap_or("upload"), "size": upload.expected_size, "transferred": upload.received, "status": "running" })
            })
            .collect::<Vec<_>>());
        tasks.extend(
            finishing_uploads
                .iter()
                .filter(|(_, upload)| upload.session_id == session_id)
                .map(|(task_id, upload)| {
                    json!({ "taskId": task_id, "sessionId": session_id, "direction": "upload", "fileName": upload.remote_path.rsplit('/').next().unwrap_or("upload"), "size": upload.size, "transferred": upload.transferred.load(Ordering::Acquire), "status": if upload.cancelled.load(Ordering::Acquire) { "cancelled" } else { "running" } })
                }),
        );
        tasks.extend(
            downloads
                .iter()
                .filter(|(_, download)| download.session_id == session_id)
                .map(|(task_id, download)| {
                    json!({ "taskId": task_id, "sessionId": session_id, "direction": "download", "fileName": download.file_name, "size": download.size, "transferred": download.next_offset, "status": "running" })
                }),
        );
        Ok(json!({ "tasks": tasks }))
    }

    pub fn transfer_status(&self, task_id: &str) -> Result<Value, String> {
        if let Some(upload) = self
            .uploads
            .lock()
            .map_err(|_| "Upload registry is poisoned".to_string())?
            .get(task_id)
        {
            return Ok(
                json!({ "taskId": task_id, "sessionId": upload.session_id, "direction": "upload", "size": upload.expected_size, "transferred": upload.received, "status": "running" }),
            );
        }
        if let Some(upload) = self
            .finishing_uploads
            .lock()
            .map_err(|_| "Finishing upload registry is poisoned".to_string())?
            .get(task_id)
        {
            return Ok(
                json!({ "taskId": task_id, "sessionId": upload.session_id, "direction": "upload", "size": upload.size, "transferred": upload.transferred.load(Ordering::Acquire), "status": if upload.cancelled.load(Ordering::Acquire) { "cancelled" } else { "running" } }),
            );
        }
        if let Some(download) = self
            .downloads
            .lock()
            .map_err(|_| "Download registry is poisoned".to_string())?
            .get(task_id)
        {
            return Ok(
                json!({ "taskId": task_id, "sessionId": download.session_id, "direction": "download", "size": download.size, "transferred": download.next_offset, "status": "running" }),
            );
        }
        if let Some(task) = self
            .transfer_history
            .lock()
            .map_err(|_| "Transfer history is poisoned".to_string())?
            .iter()
            .find(|task| task.get("taskId").and_then(Value::as_str) == Some(task_id))
        {
            return Ok(task.clone());
        }
        Err("Transfer task was not found".to_string())
    }

    fn record_transfer(&self, task: Value) {
        let Some(task_id) = task.get("taskId").and_then(Value::as_str) else {
            return;
        };
        if let Ok(mut history) = self.transfer_history.lock() {
            history.retain(|entry| entry.get("taskId").and_then(Value::as_str) != Some(task_id));
            history.push_back(task);
            while history.len() > 64 {
                history.pop_front();
            }
        }
    }

    fn active_transfer_count(&self, session_id: &str) -> Result<usize, String> {
        let uploads = self
            .uploads
            .lock()
            .map_err(|_| "Upload registry is poisoned".to_string())?
            .values()
            .filter(|upload| upload.session_id == session_id)
            .count();
        let downloads = self
            .downloads
            .lock()
            .map_err(|_| "Download registry is poisoned".to_string())?
            .values()
            .filter(|download| download.session_id == session_id)
            .count();
        let finishing_uploads = self
            .finishing_uploads
            .lock()
            .map_err(|_| "Finishing upload registry is poisoned".to_string())?
            .values()
            .filter(|upload| upload.session_id == session_id)
            .count();
        Ok(uploads + finishing_uploads + downloads)
    }

    fn cleanup_session_transfers(&self, session_id: &str) -> Result<(), String> {
        let removed_uploads = {
            let mut uploads = self
                .uploads
                .lock()
                .map_err(|_| "Upload registry is poisoned".to_string())?;
            let task_ids = uploads
                .iter()
                .filter(|(_, upload)| upload.session_id == session_id)
                .map(|(task_id, _)| task_id.clone())
                .collect::<Vec<_>>();
            task_ids
                .into_iter()
                .filter_map(|task_id| uploads.remove(&task_id))
                .collect::<Vec<_>>()
        };
        for upload in removed_uploads {
            let _ = std::fs::remove_file(upload.local_path);
        }
        for upload in self
            .finishing_uploads
            .lock()
            .map_err(|_| "Finishing upload registry is poisoned".to_string())?
            .values()
            .filter(|upload| upload.session_id == session_id)
        {
            upload.cancelled.store(true, Ordering::Release);
        }
        self.downloads
            .lock()
            .map_err(|_| "Download registry is poisoned".to_string())?
            .retain(|_, download| download.session_id != session_id);
        if let Ok(mut history) = self.transfer_history.lock() {
            history
                .retain(|task| task.get("sessionId").and_then(Value::as_str) != Some(session_id));
        }
        Ok(())
    }
}

fn flow_mode_name(mode: AuthFlowMode) -> &'static str {
    match mode {
        AuthFlowMode::PasswordOnly => "password_only",
        AuthFlowMode::PasswordPlusOtp => "password_plus_otp",
        AuthFlowMode::PasswordThenOtp => "password_then_otp",
    }
}

fn method_offered(result: &AuthResult, kind: MethodKind) -> bool {
    match result {
        AuthResult::Failure {
            remaining_methods, ..
        } => remaining_methods.contains(&kind),
        _ => false,
    }
}

/// Tries password authentication first, then keyboard-interactive. The
/// `offered` result of the preceding auth attempt tells which methods the
/// server still accepts. Keyboard-interactive rounds are auto-answered from
/// the Quick Sudo orchestration config, covering PAM 2FA/TOTP logins.
async fn authenticate_password_or_interactive(
    session: &mut Handle<SshClient>,
    connection: &StoredConnection,
    orchestration: &SudoAuth,
    offered: &AuthResult,
) -> Result<(), String> {
    if method_offered(offered, MethodKind::Password) {
        eprintln!("[ssh-trace] auth: password method offered, trying password");
        let result = try_password(session, connection).await?;
        eprintln!(
            "[ssh-trace] auth: password result success={}",
            result.success()
        );
        if result.success() {
            return Ok(());
        }
        if method_offered(&result, MethodKind::KeyboardInteractive) {
            eprintln!("[ssh-trace] auth: falling back to keyboard-interactive");
            return authenticate_keyboard_interactive(session, connection, orchestration).await;
        }
        return Err("SSH password authentication was rejected".to_string());
    }
    if method_offered(offered, MethodKind::KeyboardInteractive) {
        eprintln!("[ssh-trace] auth: keyboard-interactive only, starting");
        // Servers with PasswordAuthentication disabled still accept the
        // password through keyboard-interactive (PAM), including hosts that
        // ask a 2FA/TOTP follow-up question.
        return authenticate_keyboard_interactive(session, connection, orchestration).await;
    }
    eprintln!("[ssh-trace] auth: neither password nor keyboard-interactive offered");
    Err("SSH server did not advertise password or keyboard-interactive authentication; refusing to send the password".to_string())
}

async fn try_password(
    session: &mut Handle<SshClient>,
    connection: &StoredConnection,
) -> Result<AuthResult, String> {
    tokio::time::timeout(
        Duration::from_secs(connection.connect_timeout_secs),
        session.authenticate_password(&connection.username, &connection.password),
    )
    .await
    .map_err(|_| "SSH password authentication timed out".to_string())?
    .map_err(|error| format!("SSH password authentication failed: {error}"))
}

/// Drives a keyboard-interactive handshake, answering each round from the
/// orchestration config (password plus optional TOTP follow-ups).
async fn authenticate_keyboard_interactive(
    session: &mut Handle<SshClient>,
    connection: &StoredConnection,
    orchestration: &SudoAuth,
) -> Result<(), String> {
    let timeout = Duration::from_secs(connection.connect_timeout_secs);
    let mut state = exec::KeyboardInteractiveState::default();
    let mut response = tokio::time::timeout(
        timeout,
        session.authenticate_keyboard_interactive_start(&connection.username, None),
    )
    .await
    .map_err(|_| "SSH keyboard-interactive authentication timed out".to_string())?
    .map_err(|error| format!("SSH keyboard-interactive authentication failed: {error}"))?;
    for _ in 0..4 {
        match response {
            client::KeyboardInteractiveAuthResponse::Success => return Ok(()),
            client::KeyboardInteractiveAuthResponse::Failure { .. } => {
                return Err("SSH keyboard-interactive authentication was rejected".to_string());
            }
            client::KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let answers =
                    exec::keyboard_interactive_answers(orchestration, &mut state, &prompts);
                response = tokio::time::timeout(
                    timeout,
                    session.authenticate_keyboard_interactive_respond(answers),
                )
                .await
                .map_err(|_| "SSH keyboard-interactive authentication timed out".to_string())?
                .map_err(|error| {
                    format!("SSH keyboard-interactive authentication failed: {error}")
                })?;
            }
        }
    }
    Err("SSH keyboard-interactive authentication did not finish".to_string())
}

async fn authenticate_private_key_result(
    session: &mut Handle<SshClient>,
    connection: &StoredConnection,
) -> Result<AuthResult, String> {
    let key_path = expand_private_key_path(&connection.private_key_path);
    let key_text = tokio::fs::read_to_string(&key_path)
        .await
        .map_err(|error| {
            format!(
                "Failed to read SSH private key '{}': {error}",
                key_path.display()
            )
        })?;
    let passphrase = (!connection.private_key_passphrase.is_empty())
        .then_some(connection.private_key_passphrase.as_str());
    let private_key = decode_secret_key(&key_text, passphrase)
        .map_err(|error| format!("Failed to decode SSH private key: {error}"))?;
    let hash = session
        .best_supported_rsa_hash()
        .await
        .ok()
        .flatten()
        .flatten();
    tokio::time::timeout(
        Duration::from_secs(connection.connect_timeout_secs),
        session.authenticate_publickey(
            &connection.username,
            PrivateKeyWithHashAlg::new(Arc::new(private_key), hash),
        ),
    )
    .await
    .map_err(|_| "SSH private-key authentication timed out".to_string())?
    .map_err(|error| format!("SSH private-key authentication failed: {error}"))
}

async fn authenticate_private_key(
    session: &mut Handle<SshClient>,
    connection: &StoredConnection,
) -> Result<(), String> {
    if authenticate_private_key_result(session, connection)
        .await?
        .success()
    {
        Ok(())
    } else {
        Err("SSH private-key authentication was rejected".to_string())
    }
}

fn expand_private_key_path(path: &str) -> PathBuf {
    let Some(remainder) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) else {
        return PathBuf::from(path);
    };
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(remainder)
}

async fn authenticate_agent(
    session: &mut Handle<SshClient>,
    connection: &StoredConnection,
) -> Result<(), String> {
    #[cfg(unix)]
    let mut agent = if connection.agent_socket.is_empty() {
        AgentClient::connect_env()
            .await
            .map_err(|error| format!("SSH Agent is unavailable: {error}"))?
    } else {
        AgentClient::connect_uds(&connection.agent_socket)
            .await
            .map_err(|error| {
                format!(
                    "SSH Agent at '{}' is unavailable: {error}",
                    connection.agent_socket
                )
            })?
    };

    #[cfg(windows)]
    let mut agent = {
        let stream = pageant::PageantStream::new()
            .await
            .map_err(|error| format!("Windows Pageant is unavailable: {error}"))?;
        AgentClient::connect(stream)
    };

    let identities = agent
        .request_identities()
        .await
        .map_err(|error| format!("Failed to list SSH Agent identities: {error}"))?;
    if identities.is_empty() {
        return Err("SSH Agent has no identities".to_string());
    }
    let hash = session
        .best_supported_rsa_hash()
        .await
        .ok()
        .flatten()
        .flatten();
    let authenticated = tokio::time::timeout(
        Duration::from_secs(connection.connect_timeout_secs),
        async {
            for identity in identities {
                let result = match &identity {
                    AgentIdentity::PublicKey { key, .. } => {
                        session
                            .authenticate_publickey_with(
                                &connection.username,
                                key.clone(),
                                hash,
                                &mut agent,
                            )
                            .await
                    }
                    AgentIdentity::Certificate { certificate, .. } => {
                        session
                            .authenticate_certificate_with(
                                &connection.username,
                                certificate.clone(),
                                hash,
                                &mut agent,
                            )
                            .await
                    }
                };
                if result.is_ok_and(|result| result.success()) {
                    return true;
                }
            }
            false
        },
    )
    .await
    .map_err(|_| "SSH Agent authentication timed out".to_string())?;
    authenticated
        .then_some(())
        .ok_or_else(|| "No SSH Agent identity was accepted".to_string())
}

async fn publish_terminal(
    session_id: &str,
    stream: TerminalStream,
    data: Vec<u8>,
    replay: &Arc<AsyncMutex<ReplayBuffer>>,
    emitter: &PluginEmitter,
) {
    let frame = replay.lock().await.push(stream, data);
    if let Err(error) = emitter.binary(&format!("ssh/terminal/out/{session_id}"), &frame.encode()) {
        eprintln!(
            "[ssh-sftp-plugin] terminal output failed: {}",
            error.message
        );
    }
}

async fn delete_directory_tree(
    sftp: &Arc<AsyncMutex<SftpSession>>,
    root: String,
) -> Result<(), String> {
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

fn remote_transfer_paths(target: &str, task_id: &str) -> Result<(String, String), String> {
    let (parent, _) = target
        .rsplit_once('/')
        .ok_or("Remote upload path has no parent")?;
    let parent = if parent.is_empty() { "/" } else { parent };
    Ok((
        format!(
            "{}/.dbx-upload-{task_id}.part",
            parent.trim_end_matches('/')
        ),
        format!(
            "{}/.dbx-upload-{task_id}.backup",
            parent.trim_end_matches('/')
        ),
    ))
}

async fn commit_remote_file(
    sftp: &Arc<AsyncMutex<SftpSession>>,
    temporary: &str,
    target: &str,
    backup: &str,
) -> Result<(), String> {
    let target_exists = sftp.lock().await.metadata(target.to_string()).await.is_ok();
    if target_exists {
        sftp.lock()
            .await
            .rename(target.to_string(), backup.to_string())
            .await
            .map_err(sftp_error)?;
    }
    if let Err(error) = sftp
        .lock()
        .await
        .rename(temporary.to_string(), target.to_string())
        .await
    {
        if target_exists {
            let _ = sftp
                .lock()
                .await
                .rename(backup.to_string(), target.to_string())
                .await;
        }
        let _ = sftp.lock().await.remove_file(temporary.to_string()).await;
        return Err(sftp_error(error));
    }
    if target_exists {
        let _ = sftp.lock().await.remove_file(backup.to_string()).await;
    }
    Ok(())
}

fn content_type_for_path(path: &str) -> Option<String> {
    let extension = path.rsplit('.').next()?.to_ascii_lowercase();
    let content_type = match extension.as_str() {
        "txt" | "log" | "md" | "rs" | "ts" | "js" | "json" | "yaml" | "yml" | "toml" | "conf" => {
            "text/plain"
        }
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        _ => return None,
    };
    Some(content_type.to_string())
}

fn format_permissions(value: u32) -> String {
    format!("{:04o}", value & 0o7777)
}

fn directory_tracking_marker(session_id: &str) -> Vec<u8> {
    format!("\x1b]777;dbx-directory-ready-{session_id}\x07").into_bytes()
}

fn directory_tracking_script(enabled: bool, session_id: &str, shell: RemoteShell) -> String {
    let marker = format!("\\033]777;dbx-directory-ready-{session_id}\\007");
    let (body, history_flush) = match (shell, enabled) {
        (RemoteShell::Bash, true) => (
            r#"if [ -z "${__DBX_CWD_ACTIVE+x}" ]; then __DBX_CWD_ACTIVE=1; __DBX_OLD_HISTCONTROL_SET=${HISTCONTROL+x}; __DBX_OLD_HISTCONTROL=${HISTCONTROL-}; case ":${HISTCONTROL-}:" in *:ignorespace:*|*:ignoreboth:*) __DBX_HISTORY_NEEDS_DELETE=0 ;; *) __DBX_HISTORY_NEEDS_DELETE=1; HISTCONTROL="${HISTCONTROL:+$HISTCONTROL:}ignorespace" ;; esac; __DBX_OLD_PROMPT_COMMAND=${PROMPT_COMMAND-}; __dbx_emit_cwd(){ printf '\033]7;file://%s%s\007' "${HOSTNAME:-localhost}" "$PWD"; }; PROMPT_COMMAND='__dbx_emit_cwd;'"$__DBX_OLD_PROMPT_COMMAND"; if [ "$__DBX_HISTORY_NEEDS_DELETE" = 1 ]; then history -d $((HISTCMD-1)) 2>/dev/null || true; fi; unset __DBX_HISTORY_NEEDS_DELETE; fi"#,
            "",
        ),
        (RemoteShell::Bash, false) => (
            r#"if [ -n "${__DBX_CWD_ACTIVE+x}" ]; then PROMPT_COMMAND=${__DBX_OLD_PROMPT_COMMAND-}; unset -f __dbx_emit_cwd 2>/dev/null || true; if [ "${__DBX_OLD_HISTCONTROL_SET-}" = x ]; then HISTCONTROL=${__DBX_OLD_HISTCONTROL-}; else unset HISTCONTROL; fi; unset __DBX_CWD_ACTIVE __DBX_OLD_PROMPT_COMMAND __DBX_OLD_HISTCONTROL_SET __DBX_OLD_HISTCONTROL; fi"#,
            "",
        ),
        (RemoteShell::Zsh, true) => (
            r#"if [ -z "${__DBX_CWD_ACTIVE+x}" ]; then __DBX_CWD_ACTIVE=1; if [[ -o HIST_IGNORE_SPACE ]]; then __DBX_OLD_HIST_IGNORE_SPACE=1; else __DBX_OLD_HIST_IGNORE_SPACE=0; setopt HIST_IGNORE_SPACE; fi; __dbx_emit_cwd(){ printf '\033]7;file://%s%s\007' "${HOST:-localhost}" "$PWD"; }; typeset -ga precmd_functions; precmd_functions=(__dbx_emit_cwd ${precmd_functions:#__dbx_emit_cwd}); fi"#,
            " \r",
        ),
        (RemoteShell::Zsh, false) => (
            r#"if [ -n "${__DBX_CWD_ACTIVE+x}" ]; then precmd_functions=(${precmd_functions:#__dbx_emit_cwd}); unfunction __dbx_emit_cwd 2>/dev/null || true; if [[ "${__DBX_OLD_HIST_IGNORE_SPACE-1}" = 0 ]]; then unsetopt HIST_IGNORE_SPACE; fi; unset __DBX_CWD_ACTIVE __DBX_OLD_HIST_IGNORE_SPACE; fi"#,
            " \r",
        ),
        (RemoteShell::Other, _) => ("", ""),
    };
    format!(" {body}; printf '{marker}'\r{history_flush}")
}

impl RemoteShell {
    fn supports_directory_tracking(self) -> bool {
        matches!(self, Self::Bash | Self::Zsh)
    }
}

async fn detect_remote_shell(handle: &Handle<SshClient>) -> RemoteShell {
    remote_shell_or_timeout(
        detect_remote_shell_inner(handle),
        REMOTE_SHELL_DETECTION_TIMEOUT,
    )
    .await
}

async fn remote_shell_or_timeout<F>(future: F, timeout: Duration) -> RemoteShell
where
    F: Future<Output = RemoteShell>,
{
    tokio::time::timeout(timeout, future)
        .await
        .unwrap_or(RemoteShell::Other)
}

async fn detect_remote_shell_inner(handle: &Handle<SshClient>) -> RemoteShell {
    let Ok(mut channel) = handle.channel_open_session().await else {
        return RemoteShell::Other;
    };
    if channel.exec(true, "printf '%s' \"$SHELL\"").await.is_err() {
        return RemoteShell::Other;
    }
    let mut output = Vec::new();
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } => output.extend_from_slice(&data),
            ChannelMsg::Eof | ChannelMsg::Close => break,
            _ => {}
        }
    }
    classify_remote_shell(&String::from_utf8_lossy(&output))
}

fn classify_remote_shell(value: &str) -> RemoteShell {
    let shell = value.trim().replace('\\', "/");
    match shell.rsplit('/').next().unwrap_or_default() {
        "bash" => RemoteShell::Bash,
        "zsh" => RemoteShell::Zsh,
        _ => RemoteShell::Other,
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

pub fn filesystem_path(params: &Value) -> Result<String, String> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| "Missing filesystem URI".to_string())?;
    path_from_sftp_uri(uri)
}

pub fn connection_id_param(params: &Value) -> Result<&str, String> {
    params
        .get("connectionId")
        .and_then(Value::as_str)
        .ok_or("Missing connectionId".to_string())
}

fn sftp_error(error: impl std::fmt::Display) -> String {
    format!("SFTP operation failed: {error}")
}

fn plugin_error(error: PluginError) -> String {
    error.message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_buffer_is_sequence_addressable() {
        let mut replay = ReplayBuffer::default();
        replay.push(TerminalStream::Stdout, b"one".to_vec());
        replay.push(TerminalStream::Stderr, b"two".to_vec());
        let frames = replay.after(1);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].sequence, 2);
    }

    /// Stress test: 5 MiB of continuous terminal output through the 2 MiB
    /// ring buffer. The buffer must stay pinned at the cap, keep sequence
    /// addressing contiguous (so replay `complete` stays exact), and hand
    /// back only the newest 2 MiB.
    #[test]
    fn replay_buffer_stress_keeps_five_megabytes_within_the_cap() {
        const CHUNK: usize = 64 * 1024;
        const TOTAL: usize = 5 * 1024 * 1024;
        let mut replay = ReplayBuffer::default();
        let pushes = TOTAL / CHUNK;
        for index in 0..pushes {
            let mut chunk = vec![b'x'; CHUNK];
            chunk[0] = b'a' + (index % 26) as u8;
            replay.push(TerminalStream::Stdout, chunk);
            assert!(replay.bytes <= TERMINAL_REPLAY_LIMIT, "buffer exceeded the cap at push {index}");
        }
        // The buffer holds exactly the newest 2 MiB, not the full 5 MiB.
        assert_eq!(replay.bytes, TERMINAL_REPLAY_LIMIT);
        let first = replay.first_sequence();
        assert_eq!(first as usize, pushes - TERMINAL_REPLAY_LIMIT / CHUNK + 1);

        let frames = replay.after(0);
        let bytes: usize = frames.iter().map(|frame| frame.data.len()).sum();
        assert_eq!(bytes, TERMINAL_REPLAY_LIMIT);
        assert_eq!(frames.first().unwrap().sequence, first);
        // Sequence addressing stays contiguous after the eviction ramp.
        for pair in frames.windows(2) {
            assert_eq!(pair[1].sequence, pair[0].sequence + 1);
        }
        assert_eq!(frames.last().unwrap().sequence, replay.sequence);
        // after(first-1) returns the whole retained tail; older queries stay
        // empty, which is what drives replay `complete == false`.
        assert_eq!(replay.after(first - 1).len(), frames.len());
        assert!(replay.after(replay.sequence).is_empty());
    }

    #[test]
    fn transfer_paths_stay_next_to_target() {
        let (temporary, backup) = remote_transfer_paths("/home/user/file.txt", "task").unwrap();
        assert_eq!(temporary, "/home/user/.dbx-upload-task.part");
        assert_eq!(backup, "/home/user/.dbx-upload-task.backup");
    }

    #[test]
    fn cached_metrics_payload_marks_age_without_mutating_the_snapshot() {
        let snapshot = json!({ "loadAverage": 0.4, "network": [] });
        let payload = cached_metrics_payload(&snapshot, 1_700_000_123);
        assert_eq!(payload["loadAverage"], json!(0.4));
        assert_eq!(payload["cachedAt"], json!(1_700_000_123));
        // The stored snapshot stays clean of the marker.
        assert!(snapshot.get("cachedAt").is_none());
    }

    #[test]
    fn metrics_snapshot_cache_serves_and_cleans_per_session() {
        let data_dir = tempfile::tempdir().expect("tempdir");
        let runtime = SshRuntime::new(data_dir.path().to_path_buf());
        let snapshot = json!({ "loadAverage": 1.25 });

        assert!(runtime.cached_metrics_snapshot("sess-1").is_none());

        runtime.store_metrics_snapshot("sess-1", &snapshot);
        runtime.store_metrics_snapshot("sess-2", &json!({ "loadAverage": 2.5 }));

        let served = runtime.cached_metrics_snapshot("sess-1").expect("cached");
        assert_eq!(served["loadAverage"], json!(1.25));
        assert!(served["cachedAt"].is_u64(), "cachedAt must be unix seconds");
        // Sessions are independent: the second session serves its own data.
        let other = runtime.cached_metrics_snapshot("sess-2").expect("cached");
        assert_eq!(other["loadAverage"], json!(2.5));

        // Session close semantics drop only that session's entry.
        if let Ok(mut cache) = runtime.metrics_cache.lock() {
            cache.remove("sess-1");
        }
        assert!(runtime.cached_metrics_snapshot("sess-1").is_none());
        assert!(runtime.cached_metrics_snapshot("sess-2").is_some());
    }

    #[test]
    fn session_info_payload_carries_identity_and_liveness() {
        let row = session_info_payload("sess-1", "conn-1", "wb-1", true, true, true, 1_700_000_123, "private-key");
        assert_eq!(row["sessionId"], json!("sess-1"));
        assert_eq!(row["connectionId"], json!("conn-1"));
        assert_eq!(row["workbenchId"], json!("wb-1"));
        assert_eq!(row["readOnly"], json!(true));
        assert_eq!(row["connected"], json!(true));
        assert_eq!(row["sudoKeepalive"], json!(true));
        assert_eq!(row["createdAt"], json!(1_700_000_123));
        assert_eq!(row["authMethod"], json!("private-key"));
        // No secrets leak through the inventory payload.
        let serialized = row.to_string();
        assert!(!serialized.contains("password"));
        assert!(!serialized.contains("passphrase"));
        assert!(!serialized.contains("privateKeyMaterial"));
    }

    #[test]
    fn sessions_list_reports_an_empty_inventory_without_sessions() {
        let data_dir = tempfile::tempdir().expect("tempdir");
        let runtime = SshRuntime::new(data_dir.path().to_path_buf());
        let payload = tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(runtime.list_sessions());
        assert_eq!(payload["sessions"].as_array().expect("array").len(), 0);
    }

    #[test]
    fn directory_tracking_scripts_are_session_local() {
        let bash = directory_tracking_script(true, "session-1", RemoteShell::Bash);
        let zsh = directory_tracking_script(true, "session-1", RemoteShell::Zsh);
        assert!(bash.contains("PROMPT_COMMAND"));
        assert!(bash.contains("dbx-directory-ready-session-1"));
        assert!(zsh.contains("precmd_functions"));
    }

    #[test]
    fn directory_handshake_filters_split_marker() {
        let marker = directory_tracking_marker("session-1");
        let mut filter = DirectoryHandshakeFilter::default();
        filter.begin(marker.clone());
        assert_eq!(filter.filter(b"echoed script"), None);
        let split = marker.len() / 2;
        assert_eq!(filter.filter(&marker[..split]), None);
        let mut final_frame = marker[split..].to_vec();
        final_frame.extend_from_slice(b"prompt$ ");
        assert_eq!(filter.filter(&final_frame), Some(b"prompt$ ".to_vec()));
    }

    #[test]
    fn host_key_verdicts_map_check_results() {
        assert_eq!(
            host_key_verdict(Ok(HostKeyState::Trusted)),
            HostKeyVerdict::Trusted
        );
        assert_eq!(
            host_key_verdict(Ok(HostKeyState::Unknown)),
            HostKeyVerdict::Unknown
        );
        let changed = host_key_verdict(Err(std::io::Error::other(
            "Host key for h:22 changed (recorded at known_hosts, line 3)",
        )));
        assert!(matches!(&changed, HostKeyVerdict::Changed(message) if message.contains("line 3")));
    }

    #[test]
    fn host_key_check_responses_carry_state_and_key_identity() {
        let trusted =
            host_key_check_response(&HostKeyVerdict::Trusted, "ssh-ed25519", "SHA256:abcdef");
        assert_eq!(trusted["state"], "trusted");
        assert_eq!(trusted["keyType"], "ssh-ed25519");
        assert_eq!(trusted["fingerprint"], "SHA256:abcdef");

        assert_eq!(
            host_key_check_response(&HostKeyVerdict::Unknown, "rsa", "f")["state"],
            "unknown"
        );
        // A changed key keeps the identity of the presented key so the UI
        // can show the new fingerprint next to the mismatch reason.
        let changed = host_key_check_response(
            &HostKeyVerdict::Changed("key mismatch".to_string()),
            "rsa",
            "f",
        );
        assert_eq!(changed["state"], "changed");
        assert!(
            changed.get("error").is_none(),
            "reason travels in the notice event, not the check response"
        );

        let unreachable =
            host_key_unreachable_response("SSH connection to h:22 timed out".to_string());
        assert_eq!(unreachable["state"], "unreachable");
        assert_eq!(unreachable["error"], "SSH connection to h:22 timed out");
    }
}
