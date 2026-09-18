use std::collections::{HashMap, VecDeque};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u32 = 1;
const MAX_JSON_BYTES: usize = 8 * 1024 * 1024;
const MAX_BINARY_BYTES: usize = 64 * 1024 * 1024;
const FRAME_KIND_JSON: u8 = 0;
const FRAME_KIND_BINARY: u8 = 1;
const DEFAULT_WORK_QUEUE_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginTransport {
    JsonLines,
    Framed,
}

#[derive(Debug, Clone)]
pub struct PluginMetadata {
    pub id: String,
    pub version: String,
    pub capabilities: Vec<String>,
}

impl PluginMetadata {
    pub fn new(id: impl Into<String>, version: impl Into<String>) -> Self {
        Self { id: id.into(), version: version.into(), capabilities: Vec::new() }
    }

    pub fn with_capability(mut self, capability: impl Into<String>) -> Self {
        self.capabilities.push(capability.into());
        self
    }
}

#[derive(Debug, Clone)]
pub struct RequestContext {
    pub request_id: Option<u64>,
    pub driver: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl PluginError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::new(-32601, format!("Method not found: {method}"))
    }
}

pub trait PluginHandler: Send + Sync + 'static {
    fn handle(
        &self,
        context: RequestContext,
        method: &str,
        params: Value,
        emitter: &PluginEmitter,
    ) -> Result<Value, PluginError>;

    fn handle_binary(&self, _channel: &str, _data: Vec<u8>, _emitter: &PluginEmitter) -> Result<(), PluginError> {
        Err(PluginError::new(-32601, "Binary input is not supported"))
    }
}

#[derive(Clone)]
pub struct PluginEmitter {
    output: Arc<Mutex<Box<dyn Write + Send>>>,
    transport: PluginTransport,
}

impl PluginEmitter {
    pub fn event(&self, method: &str, params: Value) -> Result<(), PluginError> {
        validate_protocol_name(method)?;
        self.write_json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        }))
    }

    pub fn binary(&self, channel: &str, data: &[u8]) -> Result<(), PluginError> {
        if self.transport != PluginTransport::Framed {
            return Err(PluginError::new(-32000, "Binary messages require framed transport"));
        }
        validate_protocol_name(channel)?;
        if data.len() > MAX_BINARY_BYTES {
            return Err(PluginError::new(-32600, "Binary message is too large"));
        }
        let channel = channel.as_bytes();
        if channel.len() > u16::MAX as usize {
            return Err(PluginError::new(-32600, "Binary channel is too long"));
        }
        let payload_len = 2 + channel.len() + data.len();
        let mut output = self.output.lock().map_err(|_| PluginError::new(-32000, "Plugin output lock is poisoned"))?;
        output.write_all(&[FRAME_KIND_BINARY]).map_err(io_error)?;
        output.write_all(&(payload_len as u32).to_be_bytes()).map_err(io_error)?;
        output.write_all(&(channel.len() as u16).to_be_bytes()).map_err(io_error)?;
        output.write_all(channel).map_err(io_error)?;
        output.write_all(data).map_err(io_error)?;
        output.flush().map_err(io_error)
    }

    fn respond(&self, id: u64, result: Result<Value, PluginError>) -> Result<(), PluginError> {
        match result {
            Ok(result) => self.write_json(&serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })),
            Err(error) => self.write_json(&serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": error })),
        }
    }

    fn write_json(&self, value: &Value) -> Result<(), PluginError> {
        let payload = serde_json::to_vec(value).map_err(|error| PluginError::new(-32603, error.to_string()))?;
        if payload.len() > MAX_JSON_BYTES {
            return Err(PluginError::new(-32600, "JSON message is too large"));
        }
        let mut output = self.output.lock().map_err(|_| PluginError::new(-32000, "Plugin output lock is poisoned"))?;
        match self.transport {
            PluginTransport::JsonLines => {
                output.write_all(&payload).map_err(io_error)?;
                output.write_all(b"\n").map_err(io_error)?;
            }
            PluginTransport::Framed => {
                output.write_all(&[FRAME_KIND_JSON]).map_err(io_error)?;
                output.write_all(&(payload.len() as u32).to_be_bytes()).map_err(io_error)?;
                output.write_all(&payload).map_err(io_error)?;
            }
        }
        output.flush().map_err(io_error)
    }
}

pub struct PluginServer<H> {
    metadata: PluginMetadata,
    handler: Arc<H>,
    transport: PluginTransport,
    worker_threads: usize,
    work_queue_capacity: usize,
}

impl<H: PluginHandler> PluginServer<H> {
    pub fn new(metadata: PluginMetadata, handler: H) -> Self {
        Self {
            metadata,
            handler: Arc::new(handler),
            transport: PluginTransport::JsonLines,
            worker_threads: default_worker_threads(),
            work_queue_capacity: DEFAULT_WORK_QUEUE_CAPACITY,
        }
    }

    pub fn transport(mut self, transport: PluginTransport) -> Self {
        self.transport = transport;
        self
    }

    pub fn worker_threads(mut self, worker_threads: usize) -> Self {
        self.worker_threads = worker_threads.max(1);
        self
    }

    pub fn work_queue_capacity(mut self, work_queue_capacity: usize) -> Self {
        self.work_queue_capacity = work_queue_capacity.max(1);
        self
    }

    pub fn serve(self) -> io::Result<()> {
        let emitter = PluginEmitter { output: Arc::new(Mutex::new(Box::new(io::stdout()))), transport: self.transport };
        let workers = WorkerPool::new(self.worker_threads, self.work_queue_capacity)?;
        let binary = ChannelExecutor::new(workers.clone());
        match self.transport {
            PluginTransport::JsonLines => self.serve_json_lines(BufReader::new(io::stdin()), emitter, &workers),
            PluginTransport::Framed => self.serve_framed(io::stdin(), emitter, &workers, &binary),
        }
    }

    fn serve_json_lines<R: BufRead>(
        &self,
        mut input: R,
        emitter: PluginEmitter,
        workers: &WorkerPool,
    ) -> io::Result<()> {
        loop {
            let Some(line) = read_limited_line(&mut input, MAX_JSON_BYTES)? else {
                return Ok(());
            };
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            if let Err(error) = self.dispatch_json(&line, emitter.clone(), workers) {
                eprintln!("[dbx-plugin-sdk] {error}");
            }
        }
    }

    fn serve_framed<R: Read>(
        &self,
        mut input: R,
        emitter: PluginEmitter,
        workers: &WorkerPool,
        binary: &ChannelExecutor,
    ) -> io::Result<()> {
        loop {
            let mut header = [0u8; 5];
            match input.read_exact(&mut header) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
                Err(error) => return Err(error),
            }
            let kind = header[0];
            let length = u32::from_be_bytes(header[1..5].try_into().unwrap()) as usize;
            let maximum = if kind == FRAME_KIND_JSON { MAX_JSON_BYTES } else { MAX_BINARY_BYTES + 1024 };
            if length > maximum {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "plugin frame is too large"));
            }
            let mut payload = vec![0; length];
            input.read_exact(&mut payload)?;
            match kind {
                FRAME_KIND_JSON => {
                    if let Err(error) = self.dispatch_json(&payload, emitter.clone(), workers) {
                        eprintln!("[dbx-plugin-sdk] {error}");
                    }
                }
                FRAME_KIND_BINARY => {
                    if let Err(error) = self.dispatch_binary(payload, emitter.clone(), binary) {
                        eprintln!("[dbx-plugin-sdk] {error}");
                    }
                }
                _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "unknown plugin frame kind")),
            }
        }
    }

    fn dispatch_json(&self, payload: &[u8], emitter: PluginEmitter, workers: &WorkerPool) -> Result<(), String> {
        let request: ProtocolRequest = serde_json::from_slice(payload).map_err(|error| error.to_string())?;
        if request.jsonrpc.as_deref() != Some("2.0") {
            return Err("request does not declare jsonrpc 2.0".to_string());
        }
        validate_protocol_name(&request.method).map_err(|error| error.message)?;
        if request.method == "plugin/initialize" {
            let id = request.id.ok_or("plugin/initialize must be a request")?;
            let supported = request
                .params
                .get("host")
                .and_then(|host| host.get("protocolVersions"))
                .and_then(Value::as_array)
                .is_some_and(|versions| {
                    versions.iter().any(|version| version.as_u64() == Some(PROTOCOL_VERSION as u64))
                });
            let result = if supported {
                Ok(serde_json::json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": self.metadata.capabilities,
                    "plugin": { "id": self.metadata.id, "version": self.metadata.version }
                }))
            } else {
                Err(PluginError::new(-32001, "DBX and plugin do not share a protocol version"))
            };
            return emitter.respond(id, result).map_err(|error| error.message);
        }

        let handler = self.handler.clone();
        workers.submit(move || {
            let context = RequestContext { request_id: request.id, driver: request.driver };
            let result = handler.handle(context, &request.method, request.params, &emitter);
            if let Some(id) = request.id {
                if let Err(error) = emitter.respond(id, result) {
                    eprintln!("[dbx-plugin-sdk] failed to write response: {}", error.message);
                }
            }
        })
    }

    fn dispatch_binary(&self, payload: Vec<u8>, emitter: PluginEmitter, binary: &ChannelExecutor) -> Result<(), String> {
        if payload.len() < 2 {
            return Err("invalid binary frame".to_string());
        }
        let channel_len = u16::from_be_bytes([payload[0], payload[1]]) as usize;
        if channel_len == 0 || payload.len() < 2 + channel_len {
            return Err("invalid binary channel".to_string());
        }
        let channel = std::str::from_utf8(&payload[2..2 + channel_len])
            .map_err(|_| "binary channel is not UTF-8".to_string())?
            .to_string();
        validate_protocol_name(&channel).map_err(|error| error.message)?;
        let data = payload[2 + channel_len..].to_vec();
        let handler = self.handler.clone();
        let job_channel = channel.clone();
        binary.submit(&channel, Box::new(move || {
            if let Err(error) = handler.handle_binary(&job_channel, data, &emitter) {
                eprintln!("[dbx-plugin-sdk] binary handler failed: {}", error.message);
            }
        }))
    }
}

/// Binary frames on one channel form an ordered stream (terminal keystrokes,
/// upload chunks), so their handler runs must preserve wire order: a terminal
/// that applies "l" and "s" out of order scrambles what the remote shell sees.
/// The executor keeps one lane per channel; the lane's head job runs on the
/// shared worker pool and the next job starts only after the previous returns,
/// while different lanes stay fully concurrent. JSON requests are independent
/// and bypass lanes entirely.
#[derive(Clone)]
struct ChannelExecutor {
    lanes: Arc<Mutex<HashMap<String, ChannelLane>>>,
    pool: WorkerPool,
}

#[derive(Default)]
struct ChannelLane {
    running: bool,
    pending: VecDeque<PluginJob>,
}

impl ChannelExecutor {
    fn new(pool: WorkerPool) -> Self {
        Self { lanes: Arc::new(Mutex::new(HashMap::new())), pool }
    }

    fn submit(&self, channel: &str, job: PluginJob) -> Result<(), String> {
        let head = {
            let mut lanes = self
                .lanes
                .lock()
                .map_err(|_| "plugin channel lanes are poisoned".to_string())?;
            let lane = lanes.entry(channel.to_string()).or_default();
            if lane.running {
                lane.pending.push_back(job);
                None
            } else {
                lane.running = true;
                Some(job)
            }
        };
        match head {
            Some(job) => self.start(channel.to_string(), job),
            None => Ok(()),
        }
    }

    fn start(&self, channel: String, job: PluginJob) -> Result<(), String> {
        let executor = self.clone();
        let finish_channel = channel.clone();
        let chained: PluginJob = Box::new(move || {
            job();
            executor.finish(finish_channel);
        });
        if let Err(error) = self.pool.submit(chained) {
            // The pool is gone; free the lane so a restart is not blocked by a
            // head job that will never run. Pending jobs die with the process.
            self.release(&channel);
            return Err(error);
        }
        Ok(())
    }

    fn finish(&self, channel: String) {
        let next = {
            let mut lanes = match self.lanes.lock() {
                Ok(lanes) => lanes,
                Err(_) => return,
            };
            let Some(lane) = lanes.get_mut(&channel) else { return };
            match lane.pending.pop_front() {
                Some(job) => job,
                None => {
                    lane.running = false;
                    lanes.remove(&channel);
                    return;
                }
            }
        };
        let _ = self.start(channel, next);
    }

    fn release(&self, channel: &str) {
        if let Ok(mut lanes) = self.lanes.lock() {
            if let Some(lane) = lanes.get_mut(channel) {
                lane.running = false;
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct ProtocolRequest {
    jsonrpc: Option<String>,
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    driver: Option<String>,
    method: String,
    #[serde(default)]
    params: Value,
}

type PluginJob = Box<dyn FnOnce() + Send + 'static>;

#[derive(Clone)]
struct WorkerPool {
    sender: mpsc::SyncSender<PluginJob>,
}

impl WorkerPool {
    fn new(worker_threads: usize, queue_capacity: usize) -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<PluginJob>(queue_capacity);
        let receiver = Arc::new(Mutex::new(receiver));
        for index in 0..worker_threads {
            let receiver = receiver.clone();
            thread::Builder::new().name(format!("dbx-plugin-worker-{index}")).spawn(move || loop {
                let job = match receiver.lock() {
                    Ok(receiver) => receiver.recv(),
                    Err(_) => return,
                };
                match job {
                    Ok(job) => job(),
                    Err(_) => return,
                }
            })?;
        }
        Ok(Self { sender })
    }

    fn submit(&self, job: impl FnOnce() + Send + 'static) -> Result<(), String> {
        self.sender.send(Box::new(job)).map_err(|_| "plugin worker pool is unavailable".to_string())
    }
}

fn default_worker_threads() -> usize {
    thread::available_parallelism().map(usize::from).unwrap_or(4).clamp(2, 16)
}

fn read_limited_line<R: BufRead>(reader: &mut R, maximum: usize) -> io::Result<Option<Vec<u8>>> {
    let mut output = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if output.is_empty() { Ok(None) } else { Ok(Some(output)) };
        }
        let take = available.iter().position(|byte| *byte == b'\n').map(|index| index + 1).unwrap_or(available.len());
        if output.len().saturating_add(take) > maximum {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "plugin JSON line is too large"));
        }
        output.extend_from_slice(&available[..take]);
        reader.consume(take);
        if output.last() == Some(&b'\n') {
            return Ok(Some(output));
        }
    }
}

fn validate_protocol_name(value: &str) -> Result<(), PluginError> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_whitespace) {
        return Err(PluginError::new(-32600, "Protocol name is invalid"));
    }
    Ok(())
}

fn io_error(error: io::Error) -> PluginError {
    PluginError::new(-32000, error.to_string())
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{
        read_limited_line, ChannelExecutor, PluginEmitter, PluginError, PluginHandler, PluginMetadata, PluginServer,
        PluginTransport, RequestContext, WorkerPool,
    };
    use serde_json::Value;

    struct NoopHandler;

    impl PluginHandler for NoopHandler {
        fn handle(
            &self,
            _context: RequestContext,
            method: &str,
            _params: Value,
            _emitter: &PluginEmitter,
        ) -> Result<Value, PluginError> {
            Err(PluginError::method_not_found(method))
        }
    }

    #[test]
    fn worker_pool_executes_queued_jobs() {
        let pool = WorkerPool::new(2, 4).unwrap();
        let (sender, receiver) = mpsc::channel();
        for value in 0..4 {
            let sender = sender.clone();
            pool.submit(move || sender.send(value).unwrap()).unwrap();
        }
        drop(sender);

        let mut values = (0..4).map(|_| receiver.recv_timeout(Duration::from_secs(1)).unwrap()).collect::<Vec<_>>();
        values.sort_unstable();
        assert_eq!(values, vec![0, 1, 2, 3]);
    }

    /// Records the order in which binary frames were applied to the handler.
    /// Sequence N sleeps `delays_ms[N]` before recording, with later sequences
    /// sleeping less: on a concurrent pool the later frames finish first,
    /// which is the terminal-input reordering seen as scrambled keystrokes.
    struct ReorderingProbe {
        order: Arc<Mutex<Vec<u64>>>,
        delays_ms: [u64; 4],
    }

    impl PluginHandler for ReorderingProbe {
        fn handle(
            &self,
            _context: RequestContext,
            method: &str,
            _params: Value,
            _emitter: &PluginEmitter,
        ) -> Result<Value, PluginError> {
            Err(PluginError::method_not_found(method))
        }

        fn handle_binary(&self, _channel: &str, data: Vec<u8>, _emitter: &PluginEmitter) -> Result<(), PluginError> {
            let sequence = u64::from_be_bytes(data[..8].try_into().unwrap());
            thread::sleep(Duration::from_millis(self.delays_ms[sequence as usize]));
            self.order.lock().unwrap().push(sequence);
            Ok(())
        }
    }

    fn binary_frame_payload(channel: &str, sequence: u64) -> Vec<u8> {
        let channel = channel.as_bytes();
        let mut payload = Vec::with_capacity(2 + channel.len() + 8);
        payload.extend_from_slice(&(channel.len() as u16).to_be_bytes());
        payload.extend_from_slice(channel);
        payload.extend_from_slice(&sequence.to_be_bytes());
        payload
    }

    fn test_emitter() -> PluginEmitter {
        PluginEmitter {
            output: Arc::new(Mutex::new(Box::new(Cursor::new(Vec::new())) as Box<dyn Write + Send>)),
            transport: PluginTransport::Framed,
        }
    }

    #[test]
    fn binary_frames_on_one_channel_apply_in_submission_order() {
        let handler = ReorderingProbe { order: Arc::new(Mutex::new(Vec::new())), delays_ms: [120, 90, 60, 30] };
        let order = handler.order.clone();
        let server = PluginServer::new(PluginMetadata::new("sample", "1.0.0"), handler);
        let emitter = test_emitter();
        let binary = ChannelExecutor::new(WorkerPool::new(4, 16).unwrap());

        for sequence in 0..4u64 {
            server
                .dispatch_binary(binary_frame_payload("ssh/terminal/in/s1", sequence), emitter.clone(), &binary)
                .unwrap();
        }

        let deadline = Instant::now() + Duration::from_secs(5);
        while order.lock().unwrap().len() < 4 {
            assert!(Instant::now() < deadline, "frames never completed");
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(*order.lock().unwrap(), vec![0, 1, 2, 3]);
    }

    fn wait_for_length(order: &Mutex<Vec<&'static str>>, length: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while order.lock().unwrap().len() < length {
            assert!(Instant::now() < deadline, "jobs never completed");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn channel_executor_keeps_lanes_independent_and_fifo() {
        let executor = ChannelExecutor::new(WorkerPool::new(2, 8).unwrap());
        let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let gate = Arc::new(Mutex::new(false));

        // Lane "a": the head job blocks on the gate, the follow-up is queued
        // behind it. Lane "b" must complete even while "a" is stuck.
        let blocked_order = order.clone();
        let blocked_gate = gate.clone();
        executor
            .submit("a", Box::new(move || {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !*blocked_gate.lock().unwrap() {
                    assert!(Instant::now() < deadline, "gate never opened");
                    thread::sleep(Duration::from_millis(5));
                }
                blocked_order.lock().unwrap().push("a-head");
            }))
            .unwrap();
        executor
            .submit("a", {
                let order = order.clone();
                Box::new(move || order.lock().unwrap().push("a-queued"))
            })
            .unwrap();
        executor
            .submit("b", {
                let order = order.clone();
                Box::new(move || order.lock().unwrap().push("b-head"))
            })
            .unwrap();

        wait_for_length(&order, 1);
        assert_eq!(*order.lock().unwrap(), vec!["b-head"]);

        *gate.lock().unwrap() = true;
        wait_for_length(&order, 3);
        assert_eq!(*order.lock().unwrap(), vec!["b-head", "a-head", "a-queued"]);
    }

    #[test]
    fn server_configuration_clamps_zero_worker_values() {
        let server = PluginServer::new(PluginMetadata::new("sample", "1.0.0"), NoopHandler)
            .worker_threads(0)
            .work_queue_capacity(0);

        assert_eq!(server.worker_threads, 1);
        assert_eq!(server.work_queue_capacity, 1);
    }

    #[test]
    fn limited_line_reader_rejects_oversized_messages() {
        let mut reader = Cursor::new(b"12345\n".to_vec());
        assert!(read_limited_line(&mut reader, 4).unwrap_err().to_string().contains("too large"));
    }
}
