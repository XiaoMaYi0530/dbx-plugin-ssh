//! Plain-text Telnet client sessions (RFC 854/855 subset, nyaterm-parity P2-3).
//!
//! Runtime layout mirrors `local_terminal.rs`: one entry per live session in
//! the runtime's own table (a Telnet connection carries a TCP stream and can
//! never live on the SSH `SessionEntry`), keystrokes arrive on the sequenced
//! binary channel `telnet/terminal/in/{sessionId}`, output answers on
//! `telnet/terminal/out/{sessionId}` with the shared 9-byte `TerminalFrame`
//! prefix backed by a [`ReplayBuffer`], and lifecycle changes surface as
//! `telnet/session/state` events (`connecting`/`connected`/`closed`/`error`).
//!
//! IAC handling is a hand-written byte state machine (zero new deps):
//! - `WILL ECHO`/`WILL SGA` → `DO`; `DO NAWS` → `WILL NAWS` (+ NAWS updates on
//!   resize); every other offer/request is politely declined (`DONT`/`WONT`);
//! - `IAC IAC` unescapes to a literal `0xFF` payload byte;
//! - the parser is chunk-crossing safe: an IAC sequence split across reads is
//!   completed from the next chunk (state machine, not per-chunk scans).
//!
//! Auto-login reuses the SSH expect-rule engine (`triggers.rs`) verbatim —
//! `parse_triggers` accepts the same tssh/JSON rule text and the same
//! `sendSecretKey` slots, resolved here from the start request's `secrets`
//! array instead of the connection vault. The decision/segment protocol is
//! byte-identical to the SSH read loop (command answers run locally through
//! `run_credential_command`; `ssh/trigger` becomes `telnet/trigger` and never
//! carries answer content).
//!
//! Safety note: Telnet is cleartext — credentials typed into it travel
//! unencrypted. The plugin surfaces that warning in the UI and never logs
//! keystrokes or auto-login answers.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use dbx_plugin_sdk::PluginEmitter;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};

use crate::model::TerminalStream;
use crate::ssh::ReplayBuffer;
use crate::triggers;

/// Dial timeout for the initial TCP connect (contract P2-3).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Telnet IAC escape byte.
pub const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;
/// Negotiated options: echo, suppress-go-ahead, window-size (RFC 1073).
const OPT_ECHO: u8 = 1;
const OPT_SGA: u8 = 3;
const OPT_NAWS: u8 = 31;

/// Enter-key wire form: CRLF (default, most BBS/Unix line disciplines), bare
/// CR, or LF.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnterMode {
    #[default]
    Crlf,
    Cr,
    Lf,
}

impl EnterMode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "crlf" => Some(Self::Crlf),
            "cr" => Some(Self::Cr),
            "lf" => Some(Self::Lf),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for EnterMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        EnterMode::parse(&value)
            .ok_or_else(|| serde::de::Error::custom("enterMode must be crlf, cr or lf"))
    }
}

/// Backspace wire form: DEL 0x7F (default, what xterm sends) or Ctrl+H 0x08
/// for hosts whose line discipline expects it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackspaceMode {
    #[default]
    Del,
    CtrlH,
}

impl BackspaceMode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "del" => Some(Self::Del),
            "ctrl_h" | "ctrlH" => Some(Self::CtrlH),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for BackspaceMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        BackspaceMode::parse(&value)
            .ok_or_else(|| serde::de::Error::custom("backspaceMode must be del or ctrl_h"))
    }
}

/// Converts one keyboard payload into the wire form the host expects:
/// backspace first (`0x7F` → `0x08` under `CtrlH`), then Enter (`\r` per
/// mode; an already-paired `\r\n` collapses to one Enter so CRLF mode cannot
/// double it). Pure — unit-tested below.
pub fn transform_input(data: &[u8], enter: EnterMode, backspace: BackspaceMode) -> Vec<u8> {
    let mut output = Vec::with_capacity(data.len() + 8);
    let mut previous_cr = false;
    for &byte in data {
        let byte = match (backspace, byte) {
            (BackspaceMode::CtrlH, 0x7f) => 0x08,
            (_, other) => other,
        };
        match byte {
            // One Enter per CR; a CR already followed by LF stays one Enter.
            0x0d => {
                previous_cr = true;
                match enter {
                    EnterMode::Crlf => {
                        output.push(0x0d);
                        output.push(0x0a);
                    }
                    EnterMode::Cr => output.push(0x0d),
                    EnterMode::Lf => output.push(0x0a),
                }
            }
            0x0a if previous_cr => {
                previous_cr = false;
            }
            other => {
                previous_cr = false;
                output.push(other);
            }
        }
    }
    output
}

/// Builds an RFC 1073 NAWS subnegotiation: `IAC SB NAWS <cols hi lo> <rows
/// hi lo> IAC SE`, with `0xFF` value bytes escaped as `IAC IAC`. Pure.
pub fn naws_frame(cols: u16, rows: u16) -> Vec<u8> {
    let mut frame = vec![IAC, SB, OPT_NAWS];
    for half in [cols, rows] {
        let [hi, lo] = half.to_be_bytes();
        for byte in [hi, lo] {
            if byte == IAC {
                frame.push(IAC);
            }
            frame.push(byte);
        }
    }
    frame.extend_from_slice(&[IAC, SE]);
    frame
}

/// Parser states. `Ground` passes payload bytes through; the rest consume an
/// in-flight IAC sequence that may span chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ParseState {
    #[default]
    Ground,
    /// IAC seen; the next byte selects the sequence shape.
    Iac,
    /// WILL/WONT/DO/DONT seen; `verb` holds the request byte.
    Negotiation { verb: u8 },
    /// Inside `SB <option> ... SE` — every byte until the closing `SE` is
    /// swallowed (the plugin never inspects subnegotiation content).
    Subnegotiation,
    /// IAC inside a subnegotiation: `SE` closes, `IAC IAC` is a literal
    /// 0xFF value byte, anything else is malformed and resynchronizes.
    SubnegotiationIac,
}

/// Streaming IAC stripper. Feeds wire chunks in, yields application payload
/// bytes plus negotiation reply bytes. The state machine lives across
/// `feed` calls, so sequences broken across TCP segments parse correctly.
#[derive(Debug, Default)]
pub struct TelnetParser {
    state: ParseState,
}

impl TelnetParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Consumes one wire chunk; returns `(payload, replies)`.
    pub fn feed(&mut self, chunk: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let mut payload = Vec::with_capacity(chunk.len());
        let mut replies = Vec::new();
        for &byte in chunk {
            match self.state {
                ParseState::Ground => match byte {
                    IAC => self.state = ParseState::Iac,
                    other => payload.push(other),
                },
                ParseState::Iac => {
                    self.state = match byte {
                        IAC => {
                            // IAC IAC → literal 0xFF payload byte.
                            payload.push(IAC);
                            ParseState::Ground
                        }
                        WILL | WONT | DO | DONT => ParseState::Negotiation { verb: byte },
                        SB => ParseState::Subnegotiation,
                        // NOP / other one-byte commands: swallowed silently.
                        _ => ParseState::Ground,
                    };
                }
                ParseState::Negotiation { verb } => {
                    self.state = ParseState::Ground;
                    let option = byte;
                    match (verb, option) {
                        (WILL, OPT_ECHO) | (WILL, OPT_SGA) => {
                            replies.extend_from_slice(&[IAC, DO, option]);
                        }
                        (WILL, _) => replies.extend_from_slice(&[IAC, DONT, option]),
                        (DO, OPT_NAWS) => replies.extend_from_slice(&[IAC, WILL, option]),
                        (DO, _) => replies.extend_from_slice(&[IAC, WONT, option]),
                        // Peer refusals (WONT/DONT) need no answer.
                        _ => {}
                    }
                }
                ParseState::Subnegotiation => {
                    if byte == IAC {
                        self.state = ParseState::SubnegotiationIac;
                    }
                }
                ParseState::SubnegotiationIac => {
                    self.state = match byte {
                        SE => ParseState::Ground,
                        IAC => ParseState::Subnegotiation,
                        // Malformed: resynchronize at ground state.
                        _ => ParseState::Ground,
                    };
                }
            }
        }
        (payload, replies)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelnetStartRequest {
    pub workbench_id: String,
    pub host: String,
    /// Defaults to 23.
    pub port: Option<u16>,
    pub enter_mode: Option<EnterMode>,
    pub backspace_mode: Option<BackspaceMode>,
    /// Initial window size for the first NAWS frame; the workbench sends its
    /// real size right after start anyway.
    pub cols: Option<u32>,
    pub rows: Option<u32>,
    /// Optional expect-style auto-login. Rules reuse the SSH trigger grammar;
    /// `secrets` fills the two `sendSecretKey` slots from the request (never
    /// the connection vault).
    pub auto_login: Option<AutoLoginSpec>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoLoginSpec {
    /// tssh text form, JSON string form or raw JSON object form — exactly the
    /// shapes `triggers/validate` accepts.
    pub rules: Value,
    /// Slot values for `trigger_answer_1` / `trigger_answer_2`, in order.
    #[serde(default)]
    pub secrets: Vec<String>,
}

enum TelnetCommand {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Close,
}

struct TelnetSession {
    workbench_id: String,
    host: String,
    port: u16,
    created_at_secs: u64,
    cmd_tx: mpsc::Sender<TelnetCommand>,
    replay: Arc<tokio::sync::Mutex<ReplayBuffer>>,
}

pub struct TelnetSessionRuntime {
    sessions: Arc<RwLock<HashMap<String, Arc<TelnetSession>>>>,
}

/// Resolves the auto-login spec into a validated trigger config. The secrets
/// closure maps the protocol's fixed slot names onto the request-provided
/// values (slot `trigger_answer_N` = `secrets[N-1]`), so the shared
/// `parse_triggers` validation (unknown slot, empty slot) applies unchanged.
pub fn parse_auto_login(spec: &AutoLoginSpec) -> Result<Option<triggers::TriggersConfig>, String> {
    let slots = spec.secrets.clone();
    triggers::parse_triggers(Some(&spec.rules), &move |key| match key {
        "trigger_answer_1" => slots.first().filter(|value| !value.is_empty()).cloned(),
        "trigger_answer_2" => slots.get(1).filter(|value| !value.is_empty()).cloned(),
        _ => None,
    })
}

impl TelnetSessionRuntime {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Opens a Telnet session: registers the table entry immediately (so the
    /// UI can bind its terminal), then the pump dials the host with the 10s
    /// timeout and publishes `connecting` → `connected`/`error` state events.
    pub async fn start(
        &self,
        request: TelnetStartRequest,
        emitter: PluginEmitter,
    ) -> Result<Value, String> {
        let host = request.host.trim().to_string();
        if host.is_empty() {
            return Err("telnet/start: host is required".to_string());
        }
        let port = request.port.unwrap_or(23);
        if port == 0 {
            return Err("telnet/start: port must be between 1 and 65535".to_string());
        }
        let cols = request.cols.unwrap_or(120).clamp(2, u16::MAX as u32) as u16;
        let rows = request.rows.unwrap_or(32).clamp(2, u16::MAX as u32) as u16;
        let enter_mode = request.enter_mode.unwrap_or_default();
        let backspace_mode = request.backspace_mode.unwrap_or_default();
        // Invalid rules fail the start (SSH D7 contract: never degrade
        // silently) — and the error text is configuration feedback, it never
        // includes secret values.
        let triggers_config = match &request.auto_login {
            Some(spec) => parse_auto_login(spec)?,
            None => None,
        };
        let session_id = uuid::Uuid::new_v4().to_string();
        let replay = Arc::new(tokio::sync::Mutex::new(ReplayBuffer::default()));
        let (cmd_tx, cmd_rx) = mpsc::channel(256);
        self.sessions.write().await.insert(
            session_id.clone(),
            Arc::new(TelnetSession {
                workbench_id: request.workbench_id.clone(),
                host: host.clone(),
                port,
                created_at_secs: unix_now_secs(),
                cmd_tx,
                replay: replay.clone(),
            }),
        );
        spawn_pump(
            session_id.clone(),
            request.workbench_id,
            host.clone(),
            port,
            cols,
            rows,
            enter_mode,
            backspace_mode,
            triggers_config,
            cmd_rx,
            replay,
            emitter,
            self.sessions.clone(),
        );
        Ok(json!({
            "sessionId": session_id,
            "host": host,
            "port": port,
        }))
    }

    async fn session(&self, session_id: &str) -> Result<Arc<TelnetSession>, String> {
        self.sessions
            .read()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| "Telnet session was not found".to_string())
    }

    pub async fn resize(&self, session_id: &str, cols: u32, rows: u32) -> Result<(), String> {
        self.session(session_id)
            .await?
            .cmd_tx
            .send(TelnetCommand::Resize {
                cols: cols.clamp(1, u16::MAX as u32) as u16,
                rows: rows.clamp(1, u16::MAX as u32) as u16,
            })
            .await
            .map_err(|_| "Telnet session is closed".to_string())
    }

    pub async fn replay(
        &self,
        session_id: &str,
        after_sequence: u64,
        emitter: &PluginEmitter,
    ) -> Result<Value, String> {
        let session = self.session(session_id).await?;
        let replay = session.replay.lock().await;
        let first_available_sequence = replay.first_sequence();
        let tail_sequence = replay.tail_sequence();
        let frames = replay.after(after_sequence);
        drop(replay);
        for frame in &frames {
            emitter
                .binary(
                    &format!("telnet/terminal/out/{session_id}"),
                    &frame.encode(),
                )
                .map_err(|error| error.message)?;
        }
        Ok(json!({
            "frameCount": frames.len(),
            "firstAvailableSequence": first_available_sequence,
            "tailSequence": tail_sequence,
            "complete": after_sequence.saturating_add(1) >= first_available_sequence
        }))
    }

    pub async fn close(&self, session_id: &str) -> Result<(), String> {
        let session = self
            .sessions
            .write()
            .await
            .remove(session_id)
            .ok_or("Telnet session was not found")?;
        let _ = session.cmd_tx.send(TelnetCommand::Close).await;
        Ok(())
    }

    /// Closing a workbench tears down its Telnet sessions (same contract as
    /// the local shells); a webview reload does NOT pass through here.
    pub async fn close_workbench(&self, workbench_id: &str) {
        let session_ids: Vec<String> = self
            .sessions
            .read()
            .await
            .iter()
            .filter(|(_, session)| session.workbench_id == workbench_id)
            .map(|(session_id, _)| session_id.clone())
            .collect();
        for session_id in session_ids {
            let _ = self.close(&session_id).await;
        }
    }

    /// Read-only inventory of live Telnet sessions (workbench reattach hook).
    pub async fn list(&self) -> Value {
        let sessions = self.sessions.read().await;
        let mut list: Vec<Value> = sessions
            .iter()
            .map(|(session_id, session)| {
                json!({
                    "sessionId": session_id,
                    "workbenchId": session.workbench_id,
                    "host": session.host,
                    "port": session.port,
                    "createdAt": session.created_at_secs,
                })
            })
            .collect();
        list.sort_by(|a, b| {
            a["createdAt"]
                .as_u64()
                .cmp(&b["createdAt"].as_u64())
                .then_with(|| a["sessionId"].as_str().cmp(&b["sessionId"].as_str()))
        });
        json!({ "sessions": list })
    }

    /// Keyboard input from the SDK's blocking binary-handler thread; the
    /// bounded channel applies backpressure instead of dropping keystrokes.
    /// The same entry point serves the `telnet/write` JSON fallback.
    pub fn write_input(&self, session_id: &str, data: Vec<u8>) -> Result<(), String> {
        // Do not hold the session-map read guard while applying backpressure:
        // closing a dead session needs the write lock to drop the receiver.
        let cmd_tx = {
            let sessions = self.sessions.blocking_read();
            sessions
                .get(session_id)
                .map(|session| session.cmd_tx.clone())
                // Same string contract as the SSH/local mirrors — the
                // workbench's dead-session detection matches on this error.
                .ok_or("Telnet session was not found or expired")?
        };
        cmd_tx
            .blocking_send(TelnetCommand::Input(data))
            .map_err(|error| format!("Telnet session input queue is closed: {error}"))
    }
}

async fn publish_telnet_output(
    session_id: &str,
    stream: TerminalStream,
    data: Vec<u8>,
    replay: &Arc<tokio::sync::Mutex<ReplayBuffer>>,
    emitter: &PluginEmitter,
) {
    let frame = replay.lock().await.push(stream, data);
    if let Err(error) = emitter.binary(
        &format!("telnet/terminal/out/{session_id}"),
        &frame.encode(),
    ) {
        eprintln!(
            "[ssh-sftp-plugin] telnet output publish failed: {}",
            error.message
        );
    }
}

fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Applies one trigger decision's segments exactly like the SSH read loop:
/// command answers run locally first (their trimmed stdout becomes the
/// answer), then segments are written with `sleepMs` pacing. Returns `true`
/// when anything was sent (a timeout counts as handled; a failed command
/// sends nothing).
async fn apply_trigger_decision(
    write_half: &mut tokio::net::tcp::OwnedWriteHalf,
    decision: &triggers::TriggerDecision,
    placeholders: &triggers::CommandPlaceholders,
    pacing: (u64, triggers::PassSleep),
) -> std::io::Result<bool> {
    let segments = match decision.kind {
        triggers::TriggerKind::Command => {
            let Some(command) = decision.command.as_deref() else {
                return Ok(false);
            };
            match triggers::run_credential_command(command, placeholders).await {
                Ok(answer) => triggers::pass_sleep_segments(&answer, pacing.0, pacing.1),
                Err(error) => {
                    // Failure reasons only — never the answer content.
                    eprintln!("[telnet] trigger stage {}: {error}", decision.stage);
                    return Ok(false);
                }
            }
        }
        _ => decision.segments.clone(),
    };
    let mut answered = matches!(decision.kind, triggers::TriggerKind::Timeout);
    for (payload, delay_ms) in &segments {
        if *delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(*delay_ms)).await;
        }
        write_half.write_all(payload).await?;
        answered = true;
    }
    Ok(answered)
}

/// Owns one Telnet TCP connection: dials with the 10s timeout, pumps output
/// through the IAC parser (plus the expect engine), serves workbench commands
/// and tears down on peer close/close command/any write failure.
#[allow(clippy::too_many_arguments)]
fn spawn_pump(
    session_id: String,
    workbench_id: String,
    host: String,
    port: u16,
    cols: u16,
    rows: u16,
    enter_mode: EnterMode,
    backspace_mode: BackspaceMode,
    triggers_config: Option<triggers::TriggersConfig>,
    mut cmd_rx: mpsc::Receiver<TelnetCommand>,
    replay: Arc<tokio::sync::Mutex<ReplayBuffer>>,
    emitter: PluginEmitter,
    sessions: Arc<RwLock<HashMap<String, Arc<TelnetSession>>>>,
) {
    tokio::spawn(async move {
        let emit_state = |state: &str, error: Option<String>| {
            let mut payload = json!({
                "sessionId": session_id,
                "workbenchId": workbench_id,
                "state": state,
            });
            if let Some(error) = error {
                payload["error"] = Value::String(error);
            }
            emitter.event("telnet/session/state", payload)
        };
        let _ = emit_state("connecting", None);
        let stream =
            match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host.as_str(), port)))
                .await
            {
                Ok(Ok(stream)) => stream,
                Ok(Err(error)) => {
                    let _ = emit_state("error", Some(format!("connect {host}:{port}: {error}")));
                    let _ = emit_state("closed", None);
                    sessions.write().await.remove(&session_id);
                    return;
                }
                Err(_) => {
                    let _ = emit_state(
                        "error",
                        Some(format!("connect {host}:{port} timed out after 10s")),
                    );
                    let _ = emit_state("closed", None);
                    sessions.write().await.remove(&session_id);
                    return;
                }
            };
        let _ = emit_state("connected", None);

        let (mut read_half, mut write_half) = stream.into_split();
        // 首帧 NAWS：对端声明 DO NAWS 后窗口尺寸就有意义了，尽力而为
        // （未协商时多数实现直接忽略 SB NAWS）。
        let _ = write_half.write_all(&naws_frame(cols, rows)).await;
        // Reader task → unbounded channel, so the select loop never blocks a
        // read against command handling (mirrors the local PTY pump).
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        tokio::spawn(async move {
            let mut buffer = vec![0u8; 8192];
            loop {
                match read_half.read(&mut buffer).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if out_tx.send(buffer[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });

        let mut parser = TelnetParser::new();
        let mut engine = triggers_config.map(|config| {
            triggers::TriggerEngine::new(
                config,
                triggers::CommandPlaceholders::new(&host, "", port, &host),
            )
        });
        let mut closing_reason: Option<String> = None;
        loop {
            tokio::select! {
                chunk = out_rx.recv() => {
                    let Some(chunk) = chunk else {
                        closing_reason.get_or_insert_with(|| "peer closed".to_string());
                        break;
                    };
                    let (payload, replies) = parser.feed(&chunk);
                    if !replies.is_empty() && write_half.write_all(&replies).await.is_err() {
                        closing_reason.get_or_insert_with(|| "write failed".to_string());
                        break;
                    }
                    if payload.is_empty() {
                        continue;
                    }
                    let chunk_text = String::from_utf8_lossy(&payload).into_owned();
                    publish_telnet_output(
                        &session_id,
                        TerminalStream::Stdout,
                        payload,
                        &replay,
                        &emitter,
                    )
                    .await;
                    if let Some(engine) = engine.as_mut() {
                        let hit = engine.observe(&chunk_text, unix_now_ms()).map(|decision| {
                            (decision, engine.placeholders().clone(), engine.pacing())
                        });
                        if let Some((decision, placeholders, pacing)) = hit {
                            match apply_trigger_decision(
                                &mut write_half,
                                &decision,
                                &placeholders,
                                pacing,
                            )
                            .await
                            {
                                // D6: the event never carries answer content.
                                Ok(true) => {
                                    let _ = emitter.event(
                                        "telnet/trigger",
                                        json!({
                                            "sessionId": session_id,
                                            "stage": decision.stage,
                                            "kind": decision.kind.name(),
                                        }),
                                    );
                                }
                                Ok(false) => {}
                                Err(_) => {
                                    closing_reason.get_or_insert_with(|| "write failed".to_string());
                                    break;
                                }
                            }
                        }
                    }
                }
                command = cmd_rx.recv() => match command {
                    Some(TelnetCommand::Input(data)) => {
                        let wire = transform_input(&data, enter_mode, backspace_mode);
                        if !wire.is_empty() && write_half.write_all(&wire).await.is_err() {
                            closing_reason.get_or_insert_with(|| "write failed".to_string());
                            break;
                        }
                    }
                    Some(TelnetCommand::Resize { cols, rows }) => {
                        let frame = naws_frame(cols, rows);
                        if write_half.write_all(&frame).await.is_err() {
                            closing_reason.get_or_insert_with(|| "write failed".to_string());
                            break;
                        }
                    }
                    Some(TelnetCommand::Close) | None => break,
                },
            }
        }
        // Terminal-lifecycle State frame so a drained workbench observes the
        // end of the stream, mirroring `local-terminal-exited`.
        publish_telnet_output(
            &session_id,
            TerminalStream::State,
            b"telnet-session-closed".to_vec(),
            &replay,
            &emitter,
        )
        .await;
        let _ = emit_state("closed", closing_reason);
        sessions.write().await.remove(&session_id);
    });
}

/// Decodes the `telnet/write` JSON fallback payload.
pub fn decode_write_payload(data_base64: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(data_base64.trim())
        .map_err(|_| "telnet/write: dataBase64 is not valid base64".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TerminalFrame;
    use serde_json::json;

    const WILL_ECHO: &[u8] = &[IAC, WILL, OPT_ECHO];
    const DO_NAWS: &[u8] = &[IAC, DO, OPT_NAWS];

    fn feed_all(chunks: &[&[u8]]) -> (Vec<u8>, Vec<u8>) {
        let mut parser = TelnetParser::new();
        let mut payload = Vec::new();
        let mut replies = Vec::new();
        for chunk in chunks {
            let (chunk_payload, chunk_replies) = parser.feed(chunk);
            payload.extend_from_slice(&chunk_payload);
            replies.extend_from_slice(&chunk_replies);
        }
        (payload, replies)
    }

    // —— 协商应答矩阵 ——————————————————————————————————————

    #[test]
    fn negotiation_replies_follow_the_matrix() {
        // WILL ECHO / WILL SGA → DO；WILL 其他 → DONT。
        let (_, replies) = feed_all(&[WILL_ECHO]);
        assert_eq!(replies, vec![IAC, DO, OPT_ECHO]);
        let (_, replies) = feed_all(&[&[IAC, WILL, OPT_SGA]]);
        assert_eq!(replies, vec![IAC, DO, OPT_SGA]);
        let (_, replies) = feed_all(&[&[IAC, WILL, 24]]);
        assert_eq!(replies, vec![IAC, DONT, 24]);
        // DO NAWS → WILL NAWS；DO 其他 → WONT。
        let (_, replies) = feed_all(&[DO_NAWS]);
        assert_eq!(replies, vec![IAC, WILL, OPT_NAWS]);
        let (_, replies) = feed_all(&[&[IAC, DO, OPT_ECHO]]);
        assert_eq!(replies, vec![IAC, WONT, OPT_ECHO]);
        // WONT/DONT 无应答；单字节命令（NOP/AYT）静默吞掉。
        for sequence in [
            vec![IAC, WONT, OPT_ECHO],
            vec![IAC, DONT, OPT_NAWS],
            vec![IAC, 241], // NOP
            vec![IAC, 246], // AYT
        ] {
            let (payload, replies) = feed_all(&[&sequence]);
            assert!(replies.is_empty(), "no reply for {sequence:?}");
            assert!(payload.is_empty());
        }
    }

    #[test]
    fn strip_removes_iac_and_keeps_payload_bytes() {
        let wire = [b'a', b'b', IAC, WILL, OPT_ECHO, b'c', IAC, 241, b'd'];
        let (payload, replies) = feed_all(&[&wire]);
        assert_eq!(payload, b"abcd");
        assert_eq!(replies, vec![IAC, DO, OPT_ECHO]);
    }

    #[test]
    fn iac_iac_unescapes_to_a_literal_ff() {
        let wire = [b'x', IAC, IAC, 0xfe, b'y'];
        let (payload, replies) = feed_all(&[&wire]);
        assert_eq!(payload, vec![b'x', 0xff, 0xfe, b'y']);
        assert!(replies.is_empty());
    }

    #[test]
    fn iac_split_across_chunks_still_parses() {
        // IAC 断在块边界：首块以 IAC 结尾，次块补完 WILL ECHO。
        let (payload, replies) = feed_all(&[&[b'o', b'k', IAC], &[WILL, OPT_ECHO, b'!']]);
        assert_eq!(payload, b"ok!");
        assert_eq!(replies, vec![IAC, DO, OPT_ECHO]);
        // 子协商断块同样成立。
        let (payload, replies) = feed_all(&[&[b'a', IAC, SB, 24, b'x'], &[b'y', IAC], &[SE, b'b']]);
        assert_eq!(payload, b"ab");
        assert!(replies.is_empty());
    }

    #[test]
    fn subnegotiation_content_is_swallowed() {
        // TTYPE (24) 子协商内容（含 0xFF 之外的任意字节）不进 payload。
        let wire = [IAC, SB, 24, 0, b'x', 0xff, b't', IAC, SE, b'z'];
        let (payload, replies) = feed_all(&[&wire]);
        assert_eq!(payload, b"z");
        assert!(replies.is_empty());
        // 子协商里的 IAC IAC 是字面 0xFF 值字节，仍留在 SB 内。
        let wire = [IAC, SB, OPT_NAWS, 0x00, IAC, IAC, 0x78, IAC, SE, b'!'];
        let (payload, _) = feed_all(&[&wire]);
        assert_eq!(payload, b"!");
    }

    #[test]
    fn malformed_subnegotiation_resynchronizes() {
        // 子协商中 IAC 后跟非法字节 → 回到 Ground，后续 payload 恢复透传。
        let wire = [IAC, SB, 24, IAC, b'q', b'!'];
        let (payload, _) = feed_all(&[&wire]);
        assert_eq!(payload, b"!");
    }

    // —— NAWS / 输入转换 ————————————————————————————————————

    #[test]
    fn naws_encodes_big_endian_columns_and_rows() {
        assert_eq!(
            naws_frame(120, 32),
            vec![IAC, SB, OPT_NAWS, 0, 120, 0, 32, IAC, SE]
        );
        // 0xFF 值字节按 Telnet 规则转义为 IAC IAC。
        assert_eq!(
            naws_frame(0x00ff, 0x0102),
            vec![IAC, SB, OPT_NAWS, 0x00, IAC, IAC, 0x01, 0x02, IAC, SE]
        );
    }

    #[test]
    fn enter_mode_converts_carriage_returns() {
        // crlf：\r → \r\n（默认）。
        assert_eq!(
            transform_input(b"a\rb", EnterMode::Crlf, BackspaceMode::Del),
            b"a\r\nb"
        );
        // 已成对的 \r\n 只算一次回车，不翻倍。
        assert_eq!(
            transform_input(b"a\r\nb", EnterMode::Crlf, BackspaceMode::Del),
            b"a\r\nb"
        );
        // cr：原样；lf：\r → \n。
        assert_eq!(
            transform_input(b"a\rb", EnterMode::Cr, BackspaceMode::Del),
            b"a\rb"
        );
        assert_eq!(
            transform_input(b"a\rb", EnterMode::Lf, BackspaceMode::Del),
            b"a\nb"
        );
    }

    #[test]
    fn backspace_mode_maps_del_to_ctrl_h() {
        let input = [b'a', 0x7f, b'b'];
        assert_eq!(
            transform_input(&input, EnterMode::Cr, BackspaceMode::Del),
            input
        );
        assert_eq!(
            transform_input(&input, EnterMode::Cr, BackspaceMode::CtrlH),
            [b'a', 0x08, b'b']
        );
        // 组合生效：ctrl_h 换映射，crlf 换回车。
        assert_eq!(
            transform_input(&input, EnterMode::Crlf, BackspaceMode::CtrlH),
            [b'a', 0x08, b'b']
        );
    }

    // —— 自动登录（复用 triggers 规则引擎）—————————————————

    fn slot_secrets() -> Vec<String> {
        vec!["s3cret-one".to_string(), "s3cret-two".to_string()]
    }

    #[test]
    fn auto_login_parses_rules_and_answers_fake_output() {
        // JSON 对象形态：明文 + 密文槽两阶段（sendSecretKey 槽引用）。
        // JSON 形态按 D7 严格校验（`*assword` 只在 tssh 文本形态有字面量兜底）。
        let spec = AutoLoginSpec {
            rules: json!(
                r#"{"stages":[{"pattern":"ogin:","sendText":"myuser\r"},{"pattern":"assword","sendSecretKey":"trigger_answer_1"}]}"#
            ),
            secrets: slot_secrets(),
        };
        let config = parse_auto_login(&spec)
            .expect("rules parse")
            .expect("enabled");
        assert_eq!(config.stages.len(), 2);

        let mut engine = triggers::TriggerEngine::new(
            config,
            triggers::CommandPlaceholders::new("bbs.example", "", 23, "bbs.example"),
        );
        // 喂假输出流：login 提示命中阶段 1，明文应答。
        let decision = engine
            .observe("Welcome!\r\nlogin: ", 1_000)
            .expect("stage 1 must answer");
        assert_eq!(decision.stage, 1);
        assert_eq!(decision.kind, triggers::TriggerKind::Text);
        assert_eq!(decision.segments, vec![(b"myuser\r".to_vec(), 0)]);

        // 随后 password 提示命中阶段 2，密文槽应答（槽 1 值）。
        let decision = engine
            .observe("Password: ", 2_000)
            .expect("stage 2 must answer");
        assert_eq!(decision.stage, 2);
        assert_eq!(decision.kind, triggers::TriggerKind::Secret);
        assert_eq!(
            decision.segments,
            triggers::pass_sleep_segments("s3cret-one", 100, triggers::PassSleep::None)
        );
    }

    #[test]
    fn auto_login_accepts_the_tssh_text_form() {
        // tssh 文本形态同样全量可用（Expect* 指令）；注意 tssh 语法里没有
        // sendSecretKey——槽引用属于 JSON 形态的 sendSecretKey 字段。
        let spec = AutoLoginSpec {
            rules: json!(
                "#!! ExpectCount 1\n#!! ExpectPattern1 ogin:\n#!! ExpectSendText1 myuser\\r"
            ),
            secrets: slot_secrets(),
        };
        let config = parse_auto_login(&spec)
            .expect("rules parse")
            .expect("enabled");
        let mut engine = triggers::TriggerEngine::new(
            config,
            triggers::CommandPlaceholders::new("bbs", "", 23, "bbs"),
        );
        let decision = engine.observe("login: ", 1_000).expect("stage 1");
        assert_eq!(decision.segments, vec![(b"myuser\r".to_vec(), 0)]);
    }

    #[test]
    fn auto_login_maps_both_secret_slots() {
        let spec = AutoLoginSpec {
            rules: json!(
                r#"{"stages":[{"pattern":"a","sendSecretKey":"trigger_answer_1"},{"pattern":"b","sendSecretKey":"trigger_answer_2"}]}"#
            ),
            secrets: slot_secrets(),
        };
        let config = parse_auto_login(&spec)
            .expect("rules parse")
            .expect("enabled");
        let mut engine = triggers::TriggerEngine::new(
            config,
            triggers::CommandPlaceholders::new("h", "", 23, "h"),
        );
        let decision = engine.observe("aaa", 1_000).expect("stage 1");
        assert_eq!(decision.segments[0].0, b"s3cret-one\r".to_vec());
        let decision = engine.observe("bbb", 2_000).expect("stage 2");
        assert_eq!(decision.segments[0].0, b"s3cret-two\r".to_vec());
    }

    #[test]
    fn auto_login_rejects_invalid_rules_and_empty_slots() {
        // 非法 JSON 且不含 Expect 指令 → 沿用 triggers 的 JSON 报错。
        let spec = AutoLoginSpec {
            rules: json!("{not json"),
            secrets: slot_secrets(),
        };
        let error = parse_auto_login(&spec).expect_err("must fail");
        assert!(error.contains("invalid JSON"), "{error}");
        // 槽位为空 = 配置错误（D7 同款）。
        let spec = AutoLoginSpec {
            rules: json!(r#"{"stages":[{"pattern":"a","sendSecretKey":"trigger_answer_1"}]}"#),
            secrets: Vec::new(),
        };
        let error = parse_auto_login(&spec).expect_err("must fail");
        assert!(error.contains("is empty"), "{error}");
        // ExpectCount 0 = 显式关闭。
        let spec = AutoLoginSpec {
            rules: json!("ExpectCount 0\nExpectPattern1 a\nExpectSendText1 x"),
            secrets: slot_secrets(),
        };
        assert!(parse_auto_login(&spec).expect("rules parse").is_none());
    }

    #[test]
    fn mode_enums_accept_wire_names_only() {
        assert_eq!(EnterMode::parse("crlf"), Some(EnterMode::Crlf));
        assert_eq!(EnterMode::parse("cr"), Some(EnterMode::Cr));
        assert_eq!(EnterMode::parse("lf"), Some(EnterMode::Lf));
        assert_eq!(EnterMode::parse("cr lf"), None);
        assert_eq!(BackspaceMode::parse("del"), Some(BackspaceMode::Del));
        assert_eq!(BackspaceMode::parse("ctrl_h"), Some(BackspaceMode::CtrlH));
        assert_eq!(BackspaceMode::parse("ctrlH"), Some(BackspaceMode::CtrlH));
        assert_eq!(BackspaceMode::parse("backspace"), None);
    }

    #[test]
    fn write_payload_requires_base64() {
        assert_eq!(decode_write_payload("aGk=").unwrap(), b"hi");
        assert!(decode_write_payload("!!").is_err());
    }

    #[test]
    fn telnet_state_frame_roundtrips_through_terminal_frame() {
        let frame = TerminalFrame {
            sequence: 3,
            stream: TerminalStream::State,
            data: b"telnet-session-closed".to_vec(),
        };
        let encoded = frame.encode();
        assert_eq!(encoded[0], 2);
        assert_eq!(&encoded[1..9], &3u64.to_be_bytes());
    }
}
