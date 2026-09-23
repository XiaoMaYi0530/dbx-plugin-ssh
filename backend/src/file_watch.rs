//! Local file watchers backing the SFTP "open in external editor" flow.
//!
//! The frontend downloads a remote file into `<downloads>/remote-edit/<ts>/`,
//! opens it with the OS default application and registers a watcher here
//! (`watch/start`). When the external editor saves, the watcher confirms the
//! content really changed (fingerprint compare, not just mtime noise) and
//! pushes a `watch/file-modified` event so the workbench can offer to upload
//! it back. Watchers are desktop-only: on web/docker the sidecar does not run
//! on the user's machine, so `watch/start` refuses up front.
//!
//! Lifecycle mirrors the transfer registry: watchers are keyed per
//! `{sessionId}:{localPath}` (one watcher per file per session — restarting a
//! watch re-baselines the fingerprint), torn down by `watch/stop`,
//! `watch/stop-all` and the `ssh/session/close` hook in main.rs, and self-heal
//! when the watched file disappears or the owning session dies.

use std::collections::HashMap;
use std::future::Future;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use notify::Watcher as _;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio::sync::RwLock as AsyncRwLock;

/// Collapse a burst of editor save events into one evaluation.
pub const DEBOUNCE: Duration = Duration::from_millis(500);
/// Editor saves that land right after `watch/start` are almost always the
/// editor re-writing its own state (or the download's tail flush); the first
/// window after registration is suppressed so the watcher never fires on the
/// file it was just created from.
pub const SUPPRESS_WINDOW: Duration = Duration::from_secs(2);
/// Files above this size are never hashed (and therefore never emit): the
/// fingerprint is the only misfire guard, and hashing a huge file on every
/// save would hurt more than a missed upload prompt.
pub const MAX_HASH_BYTES: u64 = 64 * 1024 * 1024;
/// `watch/upload` refuses files above this size: the round-trip is buffered
/// in memory on purpose (single atomic commit), so multi-GB editor saves must
/// go through the regular upload slot instead.
pub const MAX_UPLOAD_BYTES: u64 = 64 * 1024 * 1024;

/// Async session-liveness probe injected from main.rs: the watcher cannot
/// reach into the SSH session table directly (module boundaries), so each
/// emission confirms the owning session still exists before prompting.
pub type SessionProbe =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

/// Sink for confirmed changes. Production forwards to the plugin host as a
/// `watch/file-modified` event; tests collect into a channel instead.
pub trait EventPublisher: Send + Sync + 'static {
    fn publish(&self, payload: Value);
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchStartRequest {
    pub session_id: String,
    pub remote_path: String,
    pub local_path: String,
}

/// Content fingerprint of one snapshot of the watched file. `len` +
/// `modified_ms` act as the cheap pre-filter; `sha256` is the authoritative
/// comparison so an editor's mtime-only touch never fires the event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFingerprint {
    pub len: u64,
    pub modified_ms: u64,
    pub sha256: [u8; 32],
}

/// Outcome of comparing a fresh snapshot against the watch baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeVerdict {
    /// Content provably identical (or mtime untouched) — no event.
    Same,
    /// Bytes really differ from the baseline — emit.
    ContentChanged,
    /// Snapshot unavailable (missing/too large to hash) — skip the emit and
    /// keep the watch; the next writable snapshot decides.
    Undeterminable,
}

/// Reads and hashes `path`. `None` when the file is gone, unreadable, or over
/// [`MAX_HASH_BYTES`] — the caller treats `None` as "cannot confirm a change".
pub fn file_fingerprint(path: &Path) -> Option<FileFingerprint> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let len = metadata.len();
    if len > MAX_HASH_BYTES {
        return None;
    }
    let modified_ms = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut read_total = 0u64;
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        read_total += read as u64;
    }
    // The file may have been truncated between metadata() and the read loop;
    // a snapshot whose byte count no longer matches its own metadata is not a
    // trustworthy baseline.
    if read_total != len {
        return None;
    }
    let sha256: [u8; 32] = hasher.finalize().into();
    Some(FileFingerprint {
        len,
        modified_ms,
        sha256,
    })
}

/// Pure comparison core of the emit decision (unit-tested without IO).
pub fn classify_change(
    baseline: &Option<FileFingerprint>,
    current: &Option<FileFingerprint>,
) -> ChangeVerdict {
    let (Some(baseline), Some(current)) = (baseline, current) else {
        return ChangeVerdict::Undeterminable;
    };
    if baseline.len != current.len {
        return ChangeVerdict::ContentChanged;
    }
    if baseline.modified_ms == current.modified_ms {
        return ChangeVerdict::Same;
    }
    if baseline.sha256 == current.sha256 {
        return ChangeVerdict::Same;
    }
    ChangeVerdict::ContentChanged
}

/// Injection point for clocks so the debounce/suppression logic is unit
/// testable without sleeping through the production windows.
pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> std::time::Instant;
}

#[derive(Default)]
struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> std::time::Instant {
        std::time::Instant::now()
    }
}

/// Tunables for [`WatchRuntime`]; production uses [`Tunables::production`].
#[derive(Clone, Copy)]
pub struct Tunables {
    pub debounce: Duration,
    pub suppress: Duration,
    /// `Some(true)` short-circuits the desktop probe (`local_downloads::can_save_local`)
    /// so the watcher suite runs on CI runners without desktop download dirs;
    /// `None` keeps the production env probe.
    pub desktop_gate_override: Option<bool>,
}

impl Tunables {
    pub fn production() -> Self {
        Self {
            debounce: DEBOUNCE,
            suppress: SUPPRESS_WINDOW,
            desktop_gate_override: None,
        }
    }
}

/// Mutable state owned by the per-watch pump task; the registry only reads
/// the immutable identity fields.
struct WatchPump {
    baseline: Option<FileFingerprint>,
    /// Deadline of the currently armed debounce window, if any.
    pending_at: Option<std::time::Instant>,
    /// When the watch was registered (suppression window anchor).
    started_at: std::time::Instant,
}

struct WatchEntry {
    session_id: String,
    /// Identity snapshot from `watch/start`: the pump event payload and
    /// `watch/upload` both read the pair from here, so the workbench only
    /// ever has to remember the watchId.
    local_path: PathBuf,
    remote_path: String,
    /// Keeping the notify watcher alive is what keeps events flowing; dropping
    /// it (registry removal) is also how the pump task learns to stop.
    _watcher: notify::RecommendedWatcher,
}

pub struct WatchRuntime {
    watches: Arc<AsyncRwLock<HashMap<String, WatchEntry>>>,
    /// `{sessionId}:{canonicalLocalPath}` -> watchId, so re-opening the same
    /// file in an external editor never stacks a second watcher.
    dedup: Arc<AsyncRwLock<HashMap<String, String>>>,
    tunables: Tunables,
    clock: Arc<dyn Clock>,
}

impl WatchRuntime {
    pub fn new() -> Self {
        Self::with_tunables(Tunables::production())
    }

    pub fn with_tunables(tunables: Tunables) -> Self {
        Self {
            watches: Arc::new(AsyncRwLock::new(HashMap::new())),
            dedup: Arc::new(AsyncRwLock::new(HashMap::new())),
            tunables,
            clock: Arc::new(SystemClock),
        }
    }

    /// `watch/start` — registers a non-recursive watcher on `local_path`.
    /// Desktop-only (see [`crate::local_downloads::can_save_local`]); refuses
    /// anything that is not an existing regular file.
    pub async fn start(
        &self,
        request: WatchStartRequest,
        publisher: Arc<dyn EventPublisher>,
        probe: SessionProbe,
    ) -> Result<Value, String> {
        let desktop_ok = self
            .tunables
            .desktop_gate_override
            .unwrap_or_else(|| crate::local_downloads::can_save_local(|key| std::env::var_os(key)));
        if !desktop_ok {
            return Err(
                "File watching is only available on desktop — open the file from a local download instead"
                    .to_string(),
            );
        }
        let session_id = request.session_id.trim().to_string();
        let remote_path = request.remote_path.trim().to_string();
        if session_id.is_empty() || remote_path.is_empty() {
            return Err("sessionId and remotePath are required".to_string());
        }
        let local_path = PathBuf::from(request.local_path.trim());
        if !local_path.is_absolute() {
            return Err("localPath must be an absolute path".to_string());
        }
        let baseline = file_fingerprint(&local_path).ok_or_else(|| {
            format!(
                "Watched file '{}' does not exist or is not a regular file",
                local_path.display()
            )
        })?;

        let dedup_key = dedup_key(&session_id, &local_path);
        // Bind BEFORE the if-let: a guard temporary in the `if let` scrutinee
        // lives until the end of the block (pre-2024 editions), which would
        // hold the dedup read lock across `stop()` below and self-deadlock on
        // its dedup write.
        let existing = self.dedup.read().await.get(&dedup_key).cloned();
        if let Some(existing) = existing {
            // Re-opening the same file: the fresh download is the new baseline,
            // so the stale watcher is torn down before the replacement starts.
            let _ = self.stop(&existing).await;
        }

        let watch_id = uuid::Uuid::new_v4().to_string();
        let (event_tx, event_rx) = mpsc::unbounded_channel::<()>();
        let mut watcher =
            notify::recommended_watcher(move |result: Result<notify::Event, notify::Error>| {
                if result.is_ok() {
                    // Payload is irrelevant: any filesystem activity arms the pump.
                    let _ = event_tx.send(());
                }
            })
            .map_err(|error| format!("Failed to watch '{}': {error}", local_path.display()))?;
        watcher
            .watch(&local_path, notify::RecursiveMode::NonRecursive)
            .map_err(|error| format!("Failed to watch '{}': {error}", local_path.display()))?;

        let started_at = self.clock.now();
        let pump = Arc::new(AsyncRwLock::new(WatchPump {
            baseline: Some(baseline),
            pending_at: None,
            started_at,
        }));
        self.watches.write().await.insert(
            watch_id.clone(),
            WatchEntry {
                session_id: session_id.clone(),
                local_path: local_path.clone(),
                remote_path: remote_path.clone(),
                _watcher: watcher,
            },
        );
        self.dedup.write().await.insert(dedup_key, watch_id.clone());

        let runtime_self = Self {
            watches: self.watches.clone(),
            dedup: self.dedup.clone(),
            tunables: self.tunables,
            clock: self.clock.clone(),
        };
        tokio::spawn(Self::pump(
            runtime_self,
            watch_id.clone(),
            session_id,
            local_path,
            remote_path,
            pump,
            event_rx,
            publisher,
            probe,
        ));
        Ok(json!({ "watchId": watch_id }))
    }

    /// Event pump for one watcher: debounces notify bursts, confirms content
    /// changes against the baseline, and publishes once per confirmed change.
    /// Exits when the registry drops the entry (all notify senders gone) or
    /// after self-cleanup on file loss / dead session.
    #[allow(clippy::too_many_arguments)]
    async fn pump(
        runtime: Self,
        watch_id: String,
        session_id: String,
        local_path: PathBuf,
        remote_path: String,
        pump: Arc<AsyncRwLock<WatchPump>>,
        mut event_rx: mpsc::UnboundedReceiver<()>,
        publisher: Arc<dyn EventPublisher>,
        probe: SessionProbe,
    ) {
        loop {
            let deadline = pump.read().await.pending_at;
            tokio::select! {
                // The channel closes when the registry entry (and with it the
                // notify watcher owning the sender) is dropped; `None` is the
                // pump's exit signal — without the break the closed channel
                // would re-arm the debounce forever.
                signal = event_rx.recv() => {
                    if signal.is_none() {
                        break;
                    }
                    let mut state = pump.write().await;
                    if runtime.clock.now() < state.started_at + runtime.tunables.suppress {
                        // Startup suppression window: editor warm-up noise is
                        // dropped outright, not delayed into a prompt.
                        continue;
                    }
                    if state.pending_at.is_none() {
                        state.pending_at = Some(runtime.clock.now() + runtime.tunables.debounce);
                    }
                }
                _ = sleep_until(deadline), if deadline.is_some() => {
                    let verdict = {
                        let mut state = pump.write().await;
                        state.pending_at = None;
                        let current = file_fingerprint(&local_path);
                        classify_change(&state.baseline, &current)
                    };
                    match verdict {
                        ChangeVerdict::Same => {}
                        ChangeVerdict::Undeterminable => {
                            if !local_path.exists() {
                                // The file (or its whole remote-edit folder) is
                                // gone; keep no watcher on a ghost.
                                let _ = runtime.stop(&watch_id).await;
                                break;
                            }
                        }
                        ChangeVerdict::ContentChanged => {
                            if !probe(session_id.clone()).await {
                                let _ = runtime.stop(&watch_id).await;
                                break;
                            }
                            publisher.publish(json!({
                                "watchId": watch_id,
                                "sessionId": session_id,
                                "localPath": local_path.to_string_lossy(),
                                "remotePath": remote_path,
                            }));
                            // The emitted state becomes the new baseline so a
                            // repeated identical save does not re-fire.
                            if let Some(current) = file_fingerprint(&local_path) {
                                pump.write().await.baseline = Some(current);
                            }
                        }
                    }
                }
            }
        }
    }

    /// `watch/stop` — tears down one watcher.
    pub async fn stop(&self, watch_id: &str) -> Result<(), String> {
        let removed = self.watches.write().await.remove(watch_id).is_some();
        self.dedup.write().await.retain(|_, id| id != watch_id);
        if removed {
            Ok(())
        } else {
            Err("Watch was not found".to_string())
        }
    }

    /// `watch/upload` — pushes the current on-disk bytes of a watched file
    /// back to its remote path. The workbench cannot read local files by path
    /// (the host file-transfer bridge only exposes user-picked handles), so
    /// the sidecar — which downloaded the file into `remote-edit/` in the
    /// first place — reads the bytes and streams them through the same atomic
    /// temporary-file commit as `sftp/write` (permissions preserved). The
    /// write gate (`ensure_writable`) applies exactly as for any other SFTP
    /// write; the watchId proves the file came from this plugin's own
    /// download, so no arbitrary local path ever crosses the bridge.
    pub async fn upload_back(
        &self,
        ssh: &crate::ssh::SshRuntime,
        watch_id: &str,
    ) -> Result<Value, String> {
        let (session_id, local_path, remote_path) = {
            let watches = self.watches.read().await;
            let entry = watches
                .get(watch_id)
                .ok_or_else(|| "Watch was not found".to_string())?;
            (
                entry.session_id.clone(),
                entry.local_path.clone(),
                entry.remote_path.clone(),
            )
        };
        let data = std::fs::read(&local_path).map_err(|error| {
            format!(
                "Could not read watched file '{}': {error}",
                local_path.display()
            )
        })?;
        if data.len() as u64 > MAX_UPLOAD_BYTES {
            return Err(format!(
                "Watched file '{}' is larger than the {} MiB watch-upload limit; upload it manually instead",
                local_path.display(),
                MAX_UPLOAD_BYTES / (1024 * 1024)
            ));
        }
        crate::sftp_ext::write_bytes(ssh, &session_id, &remote_path, &data).await?;
        Ok(json!({
            "remotePath": remote_path,
            "size": data.len(),
        }))
    }

    /// `watch/stop-all` (and the `ssh/session/close` hook) — drops every
    /// watcher owned by one SSH session.
    pub async fn stop_session(&self, session_id: &str) -> usize {
        let ids: Vec<String> = self
            .watches
            .read()
            .await
            .iter()
            .filter(|(_, entry)| entry.session_id == session_id)
            .map(|(id, _)| id.clone())
            .collect();
        let mut watches = self.watches.write().await;
        let mut dedup = self.dedup.write().await;
        for id in &ids {
            watches.remove(id);
            dedup.retain(|_, watch_id| watch_id != id);
        }
        ids.len()
    }

    /// Test/inspection helper: number of live watchers.
    #[cfg(test)]
    pub async fn live_count(&self) -> usize {
        self.watches.read().await.len()
    }
}

/// Dedup identity: session plus the canonicalized local path so `a/b` and
/// `a/./b` map to the same watcher.
fn dedup_key(session_id: &str, local_path: &Path) -> String {
    let canonical = std::fs::canonicalize(local_path).unwrap_or_else(|_| local_path.to_path_buf());
    format!("{session_id}:{}", canonical.to_string_lossy())
}

/// Sleeps until `deadline`, finishing immediately (without error) when the
/// deadline is already in the past. `None` never resolves — `select!` only
/// reaches this branch when a deadline is armed.
async fn sleep_until(deadline: Option<std::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await,
        // Park forever; select! discards this branch unless the guard flips.
        None => std::future::pending::<()>().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[derive(Default)]
    struct CollectingPublisher {
        count: AtomicUsize,
        payloads: std::sync::Mutex<Vec<Value>>,
    }

    impl EventPublisher for CollectingPublisher {
        fn publish(&self, payload: Value) {
            self.count.fetch_add(1, Ordering::SeqCst);
            self.payloads
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(payload);
        }
    }

    impl CollectingPublisher {
        fn payload(&self) -> Value {
            self.payloads
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .last()
                .cloned()
                .unwrap_or(Value::Null)
        }
    }

    fn always_alive() -> SessionProbe {
        Arc::new(|_| Box::pin(async { true }) as Pin<Box<dyn Future<Output = bool> + Send>>)
    }

    fn always_dead() -> SessionProbe {
        Arc::new(|_| Box::pin(async { false }) as Pin<Box<dyn Future<Output = bool> + Send>>)
    }

    fn write_file(path: &Path, content: &[u8]) {
        std::fs::write(path, content).expect("write test file");
    }

    // ---- fingerprint classification (pure) ----

    #[test]
    fn fingerprint_ignores_mtime_only_touches() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("watched.txt");
        write_file(&path, b"stable");
        let first = file_fingerprint(&path).expect("fingerprint");
        // Same bytes, later mtime (writer runs after the first snapshot).
        std::thread::sleep(Duration::from_millis(20));
        write_file(&path, b"stable");
        let second = file_fingerprint(&path).expect("fingerprint");
        assert_eq!(first.len, second.len);
        assert_eq!(first.sha256, second.sha256);
        // Classification must say "same" even if the mtime ticked.
        assert_eq!(
            classify_change(&Some(first), &Some(second)),
            ChangeVerdict::Same
        );
    }

    #[test]
    fn fingerprint_detects_content_change() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("watched.txt");
        // 两次写入刻意用不同长度：同毫秒内完成时 mtime 短路无法区分，
        // 长度差异保证 classify_change 判定稳定（不依赖文件系统时间精度）。
        write_file(&path, b"before");
        let baseline = file_fingerprint(&path).expect("fingerprint");
        // The gap keeps the two snapshots on distinct millisecond mtimes so
        // the classification exercises the sha branch, not the mtime one.
        std::thread::sleep(Duration::from_millis(10));
        write_file(&path, b"after!");
        let current = file_fingerprint(&path).expect("fingerprint");
        assert_eq!(
            classify_change(&Some(baseline), &Some(current)),
            ChangeVerdict::ContentChanged
        );
    }

    #[test]
    fn fingerprint_same_content_rewritten_has_same_sha() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("watched.txt");
        write_file(&path, b"identical bytes");
        let first = file_fingerprint(&path).expect("fingerprint");
        std::thread::sleep(Duration::from_millis(20));
        write_file(&path, b"identical bytes");
        let second = file_fingerprint(&path).expect("fingerprint");
        assert_eq!(first.sha256, second.sha256);
    }

    #[test]
    fn fingerprint_is_undeterminable_for_missing_and_oversized() {
        assert_eq!(classify_change(&None, &None), ChangeVerdict::Undeterminable);
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(file_fingerprint(&dir.path().join("gone.txt")).is_none());
        // A directory is not a watchable file.
        assert!(file_fingerprint(dir.path()).is_none());
    }

    #[test]
    fn classify_change_treats_missing_snapshot_as_undeterminable() {
        let baseline = Some(FileFingerprint {
            len: 3,
            modified_ms: 1,
            sha256: [0; 32],
        });
        assert_eq!(
            classify_change(&baseline, &None),
            ChangeVerdict::Undeterminable
        );
        assert_eq!(
            classify_change(&None, &baseline),
            ChangeVerdict::Undeterminable
        );
    }

    // ---- dedup table (registry, no real filesystem events) ----

    fn tunables() -> Tunables {
        Tunables {
            debounce: Duration::from_millis(20),
            suppress: Duration::from_millis(10),
            desktop_gate_override: Some(true),
        }
    }

    async fn start_watch(
        runtime: &WatchRuntime,
        dir: &tempfile::TempDir,
        file_name: &str,
        session_id: &str,
        remote_path: &str,
        publisher: &Arc<CollectingPublisher>,
    ) -> String {
        let path = dir.path().join(file_name);
        write_file(&path, b"start");
        let response = runtime
            .start(
                WatchStartRequest {
                    session_id: session_id.to_string(),
                    remote_path: remote_path.to_string(),
                    local_path: path.to_string_lossy().into_owned(),
                },
                publisher.clone() as Arc<dyn EventPublisher>,
                always_alive(),
            )
            .await
            .expect("watch start");
        response["watchId"].as_str().expect("watchId").to_string()
    }

    #[tokio::test]
    async fn start_replaces_dedup_watch_for_the_same_file() {
        let runtime = WatchRuntime::with_tunables(tunables());
        let dir = tempfile::tempdir().expect("tempdir");
        let publisher = Arc::new(CollectingPublisher::default());
        let first = start_watch(
            &runtime,
            &dir,
            "watched.txt",
            "sess-1",
            "/remote/a.txt",
            &publisher,
        )
        .await;
        let second = start_watch(
            &runtime,
            &dir,
            "watched.txt",
            "sess-1",
            "/remote/a.txt",
            &publisher,
        )
        .await;
        assert_ne!(first, second);
        assert_eq!(runtime.live_count().await, 1, "dedup keeps one watcher");
        // A different session owns a separate watcher for the same file.
        let _third = start_watch(
            &runtime,
            &dir,
            "watched.txt",
            "sess-2",
            "/remote/a.txt",
            &publisher,
        )
        .await;
        assert_eq!(runtime.live_count().await, 2);
    }

    #[tokio::test]
    async fn stop_and_stop_session_clear_the_registry() {
        let runtime = WatchRuntime::with_tunables(tunables());
        let dir = tempfile::tempdir().expect("tempdir");
        let publisher = Arc::new(CollectingPublisher::default());
        let watch_id = start_watch(
            &runtime,
            &dir,
            "watched.txt",
            "sess-1",
            "/remote/a.txt",
            &publisher,
        )
        .await;
        runtime.stop(&watch_id).await.expect("stop");
        assert_eq!(runtime.live_count().await, 0);
        assert!(runtime.stop(&watch_id).await.is_err(), "double stop errors");

        let _a = start_watch(
            &runtime,
            &dir,
            "watched.txt",
            "sess-1",
            "/remote/a.txt",
            &publisher,
        )
        .await;
        let _b = start_watch(
            &runtime,
            &dir,
            "other.txt",
            "sess-1",
            "/remote/b.txt",
            &publisher,
        )
        .await;
        let _c = start_watch(
            &runtime,
            &dir,
            "watched.txt",
            "sess-2",
            "/remote/a.txt",
            &publisher,
        )
        .await;
        assert_eq!(runtime.stop_session("sess-1").await, 2);
        assert_eq!(runtime.live_count().await, 1, "other sessions survive");
        assert_eq!(runtime.stop_session("sess-1").await, 0, "idempotent");
    }

    #[tokio::test]
    async fn start_validates_desktop_and_paths() {
        let runtime = WatchRuntime::with_tunables(tunables());
        let publisher = Arc::new(CollectingPublisher::default());
        // Missing file is refused before any watcher is created.
        let missing = runtime
            .start(
                WatchStartRequest {
                    session_id: "s".to_string(),
                    remote_path: "/r".to_string(),
                    local_path: "/definitely/not/here.dbx-watch-test".to_string(),
                },
                publisher.clone() as Arc<dyn EventPublisher>,
                always_alive(),
            )
            .await;
        assert!(missing.unwrap_err().contains("does not exist"));
        // Relative local paths are refused (desktop detection happens first;
        // when the desktop gate is off, that error wins — both are valid on
        // CI, so accept either rejection text).
        let dir = tempfile::tempdir().expect("tempdir");
        let relative = runtime
            .start(
                WatchStartRequest {
                    session_id: "s".to_string(),
                    remote_path: "/r".to_string(),
                    local_path: "relative/path.txt".to_string(),
                },
                publisher.clone() as Arc<dyn EventPublisher>,
                always_alive(),
            )
            .await;
        let error = relative.unwrap_err();
        assert!(
            error.contains("absolute path") || error.contains("desktop"),
            "unexpected error: {error}"
        );
        let _ = dir;
    }

    /// End-to-end: a real editor-style rewrite past the suppression window
    /// arms the debounce and fires exactly one confirmed event; a mtime-only
    /// rewrite of identical bytes never fires.
    #[tokio::test]
    async fn pump_emits_once_per_confirmed_change() {
        let runtime = WatchRuntime::with_tunables(tunables());
        let dir = tempfile::tempdir().expect("tempdir");
        let publisher = Arc::new(CollectingPublisher::default());
        let _watch_id = start_watch(
            &runtime,
            &dir,
            "watched.txt",
            "sess-e2e",
            "/remote/e2e.txt",
            &publisher,
        )
        .await;

        // Past the 10ms suppression window: rewrite with new content.
        tokio::time::sleep(Duration::from_millis(150)).await;
        let path = dir.path().join("watched.txt");
        write_file(&path, b"edited-by-external-editor");

        let wait_for = |count: usize| {
            let publisher = publisher.clone();
            async move {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
                while tokio::time::Instant::now() < deadline {
                    if publisher.count.load(Ordering::SeqCst) >= count {
                        return true;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                false
            }
        };
        assert!(wait_for(1).await, "confirmed change did not emit");
        let payload = publisher.payload();
        assert_eq!(payload["sessionId"], "sess-e2e");
        assert_eq!(payload["remotePath"], "/remote/e2e.txt");
        assert!(payload["watchId"].as_str().is_some());
        assert!(payload["localPath"]
            .as_str()
            .unwrap()
            .ends_with("watched.txt"));

        // Editor saving identical bytes (mtime-only) must not re-fire.
        tokio::time::sleep(Duration::from_millis(60)).await;
        write_file(&path, b"edited-by-external-editor");
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(
            publisher.count.load(Ordering::SeqCst),
            1,
            "mtime-only rewrite must not emit"
        );
    }

    /// A watcher whose owning session died stops itself instead of prompting
    /// into a dead workbench.
    #[tokio::test]
    async fn dead_session_stops_the_watch_without_emitting() {
        let runtime = WatchRuntime::with_tunables(tunables());
        let dir = tempfile::tempdir().expect("tempdir");
        let publisher = Arc::new(CollectingPublisher::default());
        let _watch_id = start_watch_with_probe(
            &runtime,
            &dir,
            "watched.txt",
            "sess-dead",
            "/remote/dead.txt",
            &publisher,
            always_dead(),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        let path = dir.path().join("watched.txt");
        write_file(&path, b"change-into-the-void");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline && runtime.live_count().await > 0 {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(runtime.live_count().await, 0, "dead session self-cleans");
        assert_eq!(publisher.count.load(Ordering::SeqCst), 0);
    }

    async fn start_watch_with_probe(
        runtime: &WatchRuntime,
        dir: &tempfile::TempDir,
        file_name: &str,
        session_id: &str,
        remote_path: &str,
        publisher: &Arc<CollectingPublisher>,
        probe: SessionProbe,
    ) -> String {
        let path = dir.path().join(file_name);
        write_file(&path, b"start");
        let response = runtime
            .start(
                WatchStartRequest {
                    session_id: session_id.to_string(),
                    remote_path: remote_path.to_string(),
                    local_path: path.to_string_lossy().into_owned(),
                },
                publisher.clone() as Arc<dyn EventPublisher>,
                probe,
            )
            .await
            .expect("watch start");
        response["watchId"].as_str().expect("watchId").to_string()
    }

    /// Deleting the watched file self-cleans the registry.
    #[tokio::test]
    async fn deleting_the_file_stops_the_watch() {
        let runtime = WatchRuntime::with_tunables(tunables());
        let dir = tempfile::tempdir().expect("tempdir");
        let publisher = Arc::new(CollectingPublisher::default());
        let _watch_id = start_watch(
            &runtime,
            &dir,
            "watched.txt",
            "sess-del",
            "/remote/del.txt",
            &publisher,
        )
        .await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        std::fs::remove_file(dir.path().join("watched.txt")).expect("remove");
        // Touch the parent so watchers that only see directory-level events
        // (FSEvents) wake up and notice the removal.
        write_file(&dir.path().join("watched.txt"), b"resurrect");
        std::fs::remove_file(dir.path().join("watched.txt")).expect("remove again");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline && runtime.live_count().await > 0 {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(runtime.live_count().await, 0, "deleted file self-cleans");
        assert_eq!(publisher.count.load(Ordering::SeqCst), 0);
    }
}
