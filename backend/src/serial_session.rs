//! Serial-port sessions (RS-232 consoles: routers, dev boards, embeddeds).
//!
//! Desktop-only by nature: the sidecar must sit on the machine that owns the
//! port. Mirrors [`crate::telnet_session`] — same 9-byte `TerminalFrame`
//! output channel (`serial/terminal/out/{id}`), same state-event shape
//! (`serial/session/state`), same session-table lifecycle. The blocking
//! `serialport` handle lives on a dedicated OS read thread; writes go through
//! a shared mutex (keystroke-sized, so the brief std lock on the async side
//! is acceptable for an MVP).
//!
//! `BackspaceMode` reuses the telnet mapping: `ctrl_h` rewrites DEL (0x7F)
//! into BS (0x08) for devices that expect a vt100-style erase.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::RwLock;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dbx_plugin_sdk::PluginEmitter;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::model::{TerminalFrame, TerminalStream};

const READ_BUFFER: usize = 4096;
/// The blocking read timeout also bounds how long a close can stall.
const READ_TIMEOUT: Duration = Duration::from_millis(10);

/// Erase-byte mapping shared with the telnet session (`BackspaceMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackspaceMode {
    Del,
    CtrlH,
}

impl BackspaceMode {
    fn parse(value: Option<&String>) -> Self {
        match value.map(String::as_str) {
            Some("ctrl_h") => Self::CtrlH,
            _ => Self::Del,
        }
    }

    fn rewrite(&self, data: &[u8]) -> Vec<u8> {
        match self {
            Self::Del => data.to_vec(),
            Self::CtrlH => data
                .iter()
                .map(|byte| if *byte == 0x7F { 0x08 } else { *byte })
                .collect(),
        }
    }
}

/// Parses the `serialport` enums from the wire strings; invalid values fall
/// back to the 8N1 defaults rather than failing a start that a legacy device
/// might still accept with the defaults.
fn parse_data_bits(value: Option<&String>) -> serialport::DataBits {
    match value.map(String::as_str) {
        Some("7") => serialport::DataBits::Seven,
        _ => serialport::DataBits::Eight,
    }
}

fn parse_parity(value: Option<&String>) -> serialport::Parity {
    match value.map(String::as_str) {
        Some("even") => serialport::Parity::Even,
        Some("odd") => serialport::Parity::Odd,
        _ => serialport::Parity::None,
    }
}

fn parse_stop_bits(value: Option<&String>) -> serialport::StopBits {
    match value.map(String::as_str) {
        Some("2") => serialport::StopBits::Two,
        _ => serialport::StopBits::One,
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct SerialStartRequest {
    pub port_name: String,
    pub baud_rate: u32,
    pub data_bits: Option<String>,
    pub parity: Option<String>,
    pub stop_bits: Option<String>,
    pub backspace_mode: Option<String>,
    pub workbench_id: String,
}

impl Default for SerialStartRequest {
    fn default() -> Self {
        Self {
            port_name: String::new(),
            baud_rate: 115_200,
            data_bits: None,
            parity: None,
            stop_bits: None,
            backspace_mode: None,
            workbench_id: String::new(),
        }
    }
}

pub(crate) struct SerialSession {
    /// 协议往返保留（state 事件与未来多工作台路由使用；当前仅存不计）。
    #[allow(dead_code)]
    workbench_id: String,
    port_name: String,
    baud_rate: u32,
    created_at_secs: u64,
    write: Arc<Mutex<Box<dyn serialport::SerialPort>>>,
    cmd_tx: mpsc::UnboundedSender<SerialCommand>,
    backspace: BackspaceMode,
}

pub(crate) enum SerialCommand {
    Close,
}

pub struct SerialSessionRuntime {
    sessions: Arc<RwLock<HashMap<String, Arc<SerialSession>>>>,
}

impl Default for SerialSessionRuntime {
    fn default() -> Self {
        Self::new()
    }
}

fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn encode_frame(sequence: u64, stream: TerminalStream, data: &[u8]) -> Vec<u8> {
    TerminalFrame {
        sequence,
        stream,
        data: data.to_vec(),
    }
    .encode()
}

impl SerialSessionRuntime {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Lists candidate port names, sorted. Empty on hosts without serial
    /// support — the UI renders its own "no ports" hint from this.
    pub fn list_ports(&self) -> Value {
        let ports = serialport::available_ports()
            .unwrap_or_default()
            .into_iter()
            .map(|entry| match entry.port_type {
                serialport::SerialPortType::UsbPort(info) => {
                    format!("{} (USB)", info.serial_number.clone().unwrap_or_default())
                }
                _ => entry.port_name,
            })
            .collect::<Vec<_>>();
        json!({ "ports": ports })
    }

    /// Opens the port (blocking call moved off the async workers) and spawns
    /// the read thread + output pump.
    pub async fn start(
        &self,
        request: SerialStartRequest,
        emitter: PluginEmitter,
    ) -> Result<Value, String> {
        let port_name = request.port_name.trim().to_string();
        if port_name.is_empty() {
            return Err("serial/start: portName is required".to_string());
        }
        let baud_rate = request.baud_rate.clamp(50, 4_000_000);
        let backspace = BackspaceMode::parse(request.backspace_mode.as_ref());
        let data_bits = parse_data_bits(request.data_bits.as_ref());
        let parity = parse_parity(request.parity.as_ref());
        let stop_bits = parse_stop_bits(request.stop_bits.as_ref());

        let session_id = uuid::Uuid::new_v4().to_string();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();

        // The port open is a blocking syscall; keep it off the async workers.
        let open_name = port_name.clone();
        let port = tokio::task::spawn_blocking(move || {
            serialport::new(&open_name, baud_rate)
                .data_bits(data_bits)
                .parity(parity)
                .stop_bits(stop_bits)
                .timeout(READ_TIMEOUT)
                .open()
                .map_err(|error| format!("serial/start: {error}"))
        })
        .await
        .map_err(|error| format!("serial/start: join error: {error}"))??;

        let port = Arc::new(Mutex::new(port));
        let session = Arc::new(SerialSession {
            workbench_id: request.workbench_id.clone(),
            port_name: port_name.clone(),
            baud_rate,
            created_at_secs: unix_now_secs(),
            write: Arc::clone(&port),
            cmd_tx,
            backspace,
        });
        self.sessions
            .write()
            .expect("serial session registry poisoned")
            .insert(session_id.clone(), Arc::clone(&session));

        spawn_reader(session_id.clone(), Arc::clone(&port), cmd_rx, emitter);
        Ok(json!({
            "sessionId": session_id,
            "port": port_name,
            "baudRate": baud_rate,
        }))
    }

    pub(crate) async fn session(&self, session_id: &str) -> Result<Arc<SerialSession>, String> {
        self.sessions
            .read()
            .expect("serial session registry poisoned")
            .get(session_id)
            .cloned()
            .ok_or_else(|| "Serial session was not found".to_string())
    }

    /// Writes keystroke bytes through the erase mapping. Keystroke-sized
    /// writes on the async side are acceptable for an MVP; the mutex is held
    /// only for the driver call.
    pub fn write_input(&self, session: &SerialSession, data: &[u8]) -> Result<(), String> {
        let payload = session.backspace.rewrite(data);
        let mut port = session
            .write
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        port.write_all(&payload)
            .and_then(|_| port.flush())
            .map_err(|error| format!("serial write failed: {error}"))
    }

    pub async fn close(&self, session_id: &str) -> Result<(), String> {
        let session = self.session(session_id).await?;
        let _ = session.cmd_tx.send(SerialCommand::Close);
        self.sessions
            .write()
            .expect("serial session registry poisoned")
            .remove(session_id);
        Ok(())
    }

    pub async fn list(&self) -> Value {
        let sessions = self
            .sessions
            .read()
            .expect("serial session registry poisoned");
        let rows: Vec<Value> = sessions
            .iter()
            .map(|(id, session)| {
                json!({
                    "sessionId": id,
                    "port": session.port_name,
                    "baudRate": session.baud_rate,
                    "createdAt": session.created_at_secs,
                })
            })
            .collect();
        json!({ "sessions": rows })
    }
}

/// Dedicated blocking read thread: forwards port bytes to the pump channel
/// and honours `Close` by simply exiting (dropping its port clone closes the
/// handle on the writer side too — the OS closes the last reference).
fn spawn_reader(
    session_id: String,
    port: Arc<Mutex<Box<dyn serialport::SerialPort>>>,
    mut cmd_rx: mpsc::UnboundedReceiver<SerialCommand>,
    emitter: PluginEmitter,
) {
    std::thread::spawn(move || {
        let mut buffer = [0u8; READ_BUFFER];
        let mut sequence: u64 = 0;
        loop {
            if matches!(cmd_rx.try_recv(), Ok(SerialCommand::Close)) {
                let _ = emitter.event(
                    "serial/session/state",
                    json!({ "sessionId": session_id, "state": "closed" }),
                );
                return;
            }
            let read = {
                let mut port = port.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                port.read(&mut buffer)
            };
            match read {
                Ok(0) => std::thread::sleep(READ_TIMEOUT),
                Ok(n) => {
                    sequence += 1;
                    let _ = emitter.binary(
                        &format!("serial/terminal/out/{session_id}"),
                        &encode_frame(sequence, TerminalStream::Stdout, &buffer[..n]),
                    );
                }
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                Err(error) => {
                    let _ = emitter.event(
                        "serial/session/state",
                        json!({ "sessionId": session_id, "state": "error", "error": error.to_string() }),
                    );
                    return;
                }
            }
        }
    });
}
/// Base64 解码复用 telnet 的实现（同一负载形状）。
pub fn decode_write_payload(data_base64: &str) -> Result<Vec<u8>, String> {
    crate::telnet_session::decode_write_payload(data_base64)
}
