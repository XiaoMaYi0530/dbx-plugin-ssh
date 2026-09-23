//! Extended SFTP operations (stat/touch/direct-write/archive), mirroring tiny-rdm.
#![allow(dead_code)] // entry points are wired in main.rs; see the snippet at the bottom

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, FileType, OpenFlags};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use crate::model::normalize_remote_path;
use crate::ssh::{apply_preserved_permissions, lookup_owner_group_names, SshRuntime};

/// Largest payload [`write_file`] accepts in one call; bigger files must go
/// through the streaming upload slot (`sftp/upload/start`).
pub const MAX_DIRECT_WRITE_SIZE: usize = 4 * 1024 * 1024;

/// Remote `tar` commands (archive, listing, extract) run with this budget.
const REMOTE_TAR_TIMEOUT_SECS: u64 = 120;

/// `sftp/stat` — metadata for one remote path. Uses lstat semantics so
/// symlinks are reported as `symlink`, matching the `sftp/list` entries.
pub async fn stat(runtime: &SshRuntime, session_id: &str, path: &str) -> Result<Value, String> {
    let sftp = runtime.sftp(session_id).await?;
    let path = normalize_remote_path(path)?;
    let metadata = sftp
        .lock()
        .await
        .symlink_metadata(path.clone())
        .await
        .map_err(|error| format!("SFTP stat failed: {error}"))?;
    let kind = match metadata.file_type() {
        FileType::File => "file",
        FileType::Dir => "directory",
        FileType::Symlink => "symlink",
        FileType::Other => "other",
    };
    // SFTP 协议默认只返回 uid/gid 数字，user/group 字段为 None。
    // 如果 russh-sftp 没拿到名字，远程跑 `stat -c '%U %G'` 查。
    let (owner_name, group_name) = match (metadata.user.clone(), metadata.group.clone()) {
        (Some(u), Some(g)) if !u.is_empty() && !g.is_empty() => (Some(u), Some(g)),
        _ => lookup_owner_group_names(runtime, session_id, &path).await,
    };
    let owner_display = owner_name
        .clone()
        .or_else(|| metadata.uid.map(|u| u.to_string()));
    let group_display = group_name
        .clone()
        .or_else(|| metadata.gid.map(|g| g.to_string()));
    Ok(json!({
        "path": path,
        "kind": kind,
        "size": metadata.size,
        "modifiedAt": metadata.mtime.map(u64::from),
        "mode": metadata.permissions.map(format_mode),
        "owner": owner_display,
        "group": group_display,
        "ownerName": owner_name,
        "ownerUid": metadata.uid,
        "groupName": group_name,
        "groupGid": metadata.gid,
    }))
}

/// `sftp/exists` — `true` when the path is statable (dangling symlinks count).
pub async fn exists(runtime: &SshRuntime, session_id: &str, path: &str) -> Result<bool, String> {
    let sftp = runtime.sftp(session_id).await?;
    let path = normalize_remote_path(path)?;
    let exists = sftp.lock().await.symlink_metadata(path).await.is_ok();
    Ok(exists)
}

/// Upper bound for `name(1)..name(999)` collision probing in
/// [`rename_unique`]; also bounds how long the SFTP mutex is held.
pub const UNIQUE_NAME_PROBE_LIMIT: u32 = 999;

/// Longest accepted file name (chars) for the unique-name probe.
const MAX_UNIQUE_NAME_CHARS: usize = 255;

/// `sftp/rename-unique` — suggests a non-conflicting file name inside `dir`
/// for an upload about to land there. `name` itself wins when free, otherwise
/// `name(1)` .. `name(999)` are probed (the `(n)` is inserted before the last
/// extension: `report.pdf` -> `report(1).pdf`). Returns `{name, conflict}`.
pub async fn rename_unique(
    runtime: &SshRuntime,
    session_id: &str,
    dir: &str,
    name: &str,
) -> Result<Value, String> {
    let sftp = runtime.sftp(session_id).await?;
    let dir = normalize_remote_path(dir)?;
    let clean = clean_unique_name(name)?;
    let session = sftp.lock().await;
    let probe = |candidate: &str| {
        let full = join_remote_name(&dir, candidate);
        session.symlink_metadata(full)
    };
    for (index, candidate) in unique_name_candidates(&clean).into_iter().enumerate() {
        if probe(&candidate).await.is_err() {
            return Ok(json!({ "name": candidate, "conflict": index > 0 }));
        }
    }
    Err(format!(
        "No unique name derived from '{clean}' within {UNIQUE_NAME_PROBE_LIMIT} attempts"
    ))
}

/// `sftp/touch` — creates an empty file when missing, otherwise refreshes
/// mtime/atime. Servers that reject SETSTAT times make the refresh a no-op,
/// mirroring tiny-rdm's create-only `Touch`.
pub async fn touch(runtime: &SshRuntime, session_id: &str, path: &str) -> Result<(), String> {
    runtime.ensure_writable(session_id).await?;
    let sftp = runtime.sftp(session_id).await?;
    let path = normalize_remote_path(path)?;
    let session = sftp.lock().await;
    if session.symlink_metadata(path.clone()).await.is_ok() {
        let now = current_unix_secs();
        let times = FileAttributes {
            atime: Some(now),
            mtime: Some(now),
            ..FileAttributes::default()
        };
        // Unsupported (or unpermitted) utime is not an error for touch.
        let _ = session.set_metadata(path, times).await;
        return Ok(());
    }
    let file = session
        .open_with_flags(path, OpenFlags::CREATE | OpenFlags::WRITE)
        .await
        .map_err(sftp_error)?;
    drop(file);
    Ok(())
}

/// `sftp/write` — direct write for small files: base64 payload in, temporary
/// `.dbx-part-<uuid>` file out, atomic rename onto the target.
pub async fn write_file(
    runtime: &SshRuntime,
    session_id: &str,
    path: &str,
    data_base64: &str,
) -> Result<(), String> {
    runtime.ensure_writable(session_id).await?;
    let data = decode_direct_write_payload(data_base64)?;
    let sftp = runtime.sftp(session_id).await?;
    let path = normalize_remote_path(path)?;
    let task_id = Uuid::new_v4().to_string();
    let (temporary, backup) = direct_write_paths(&path, &task_id);
    {
        let session = sftp.lock().await;
        let mut file = session
            .create(temporary.clone())
            .await
            .map_err(sftp_error)?;
        if let Err(error) = file.write_all(&data).await {
            drop(file);
            let _ = session.remove_file(temporary.clone()).await;
            return Err(format!("SFTP write failed: {error}"));
        }
        if let Err(error) = file.flush().await {
            drop(file);
            let _ = session.remove_file(temporary.clone()).await;
            return Err(format!("SFTP write flush failed: {error}"));
        }
    }
    commit_temporary_file(&sftp, &temporary, &path, &backup).await
}

/// `sftp/archive` — packs multiple remote paths into `archive_path` with a
/// remote `tar -czf`. The archive is built at `<archive_path>.tmp` and renamed
/// into place only after tar succeeds. Returns `{path, size}`.
pub async fn archive(
    runtime: &SshRuntime,
    session_id: &str,
    source_paths: &[String],
    archive_path: &str,
) -> Result<Value, String> {
    runtime.ensure_writable(session_id).await?;
    let sources = clean_source_paths(source_paths)?;
    let target = normalize_remote_path(archive_path)?;
    let parent = common_parent_dir(&sources);
    let relatives = sources
        .iter()
        .map(|source| relative_to_parent(source, &parent))
        .collect::<Vec<_>>();
    let temporary = format!("{target}.tmp");
    let command = build_archive_command(&temporary, &parent, &relatives);
    let outcome = runtime
        .exec(
            session_id,
            None,
            &command,
            false,
            Some(REMOTE_TAR_TIMEOUT_SECS),
        )
        .await?;
    let sftp = runtime.sftp(session_id).await?;
    if let Err(error) = check_exec_success(&outcome, "archive") {
        let _ = sftp.lock().await.remove_file(temporary).await;
        return Err(error);
    }
    // tar -czf would have replaced an existing archive; mirror that by
    // dropping a stale target before the rename (plain SFTP rename does not
    // overwrite).
    let session = sftp.lock().await;
    if session.metadata(target.clone()).await.is_ok() {
        session.remove_file(target.clone()).await.map_err(|error| {
            format!("SFTP archive target already exists and could not be replaced: {error}")
        })?;
    }
    if let Err(error) = session.rename(temporary.clone(), target.clone()).await {
        let _ = session.remove_file(temporary).await;
        return Err(sftp_error(error));
    }
    let size = session
        .metadata(target.clone())
        .await
        .ok()
        .and_then(|metadata| metadata.size)
        .unwrap_or(0);
    Ok(json!({ "path": target, "size": size }))
}

/// `sftp/extract` — unpacks a remote `.tar.gz`/`.tgz`/`.tar` archive into
/// `destination_path` after `mkdir -p`. With `overwrite` disabled the member
/// listing is compared against the destination first and any name collision
/// aborts before anything is written. `.zip` is unsupported.
pub async fn extract(
    runtime: &SshRuntime,
    session_id: &str,
    archive_path: &str,
    destination_path: &str,
    overwrite: bool,
) -> Result<(), String> {
    runtime.ensure_writable(session_id).await?;
    let archive = normalize_remote_path(archive_path)?;
    let destination = normalize_remote_path(destination_path)?;
    let compressed = tar_z_flag(&archive)?;
    // Listing first validates the archive and yields its top-level entries.
    let listing = runtime
        .exec(
            session_id,
            None,
            &build_tar_list_command(&archive, compressed),
            false,
            Some(REMOTE_TAR_TIMEOUT_SECS),
        )
        .await?;
    check_exec_success(&listing, "archive listing")?;
    let members = listing
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let top_entries = unique_top_level_entries(members);
    if top_entries.is_empty() {
        return Err("Archive is empty or could not be listed".to_string());
    }
    if !overwrite {
        let sftp = runtime.sftp(session_id).await?;
        let session = sftp.lock().await;
        for entry in &top_entries {
            let candidate = format!("{}/{}", destination.trim_end_matches('/'), entry);
            if session.symlink_metadata(candidate).await.is_ok() {
                return Err(format!(
                    "Destination already contains '{entry}'; pass overwrite to replace it"
                ));
            }
        }
    }
    let outcome = runtime
        .exec(
            session_id,
            None,
            &build_extract_command(&archive, &destination, compressed),
            false,
            Some(REMOTE_TAR_TIMEOUT_SECS),
        )
        .await?;
    check_exec_success(&outcome, "extract")
}

// ---------------------------------------------------------------------------
// Symbolic links (IMPL_PLAN v2 P2-6)
// ---------------------------------------------------------------------------
// Spike conclusion: russh-sftp 3.0.0 exposes the SYMLINK/READLINK packets
// natively (`SftpSession::symlink(linkpath, targetpath)` and
// `SftpSession::read_link(path)`), so no exec-channel `ln -s` fallback and no
// shell quoting are needed — arguments travel as SFTP string fields.

/// `sftp/symlink-create {sessionId, target, linkPath}` — creates `linkPath`
/// pointing at `target` (relative targets are kept verbatim, like `ln -s`).
/// Refuses to clobber an existing entry; write-gated like `sftp/chmod`.
pub async fn symlink_create(
    runtime: &SshRuntime,
    session_id: &str,
    target: &str,
    link_path: &str,
) -> Result<(), String> {
    runtime.ensure_writable(session_id).await?;
    let target = clean_symlink_arg(target, "Link target")?;
    let link_path = normalize_remote_path(&clean_symlink_arg(link_path, "Link path")?)?;
    let sftp = runtime.sftp(session_id).await?;
    let session = sftp.lock().await;
    if session.symlink_metadata(link_path.clone()).await.is_ok() {
        return Err(format!("'{link_path}' already exists; remove it first"));
    }
    session.symlink(link_path, target).await.map_err(sftp_error)
}

/// `sftp/symlink-read {sessionId, linkPath} -> {target}` — read-only.
pub async fn symlink_read(
    runtime: &SshRuntime,
    session_id: &str,
    link_path: &str,
) -> Result<Value, String> {
    let sftp = runtime.sftp(session_id).await?;
    let link_path = normalize_remote_path(&clean_symlink_arg(link_path, "Link path")?)?;
    let target = sftp
        .lock()
        .await
        .read_link(link_path)
        .await
        .map_err(sftp_error)?;
    Ok(json!({ "target": target }))
}

/// `sftp/symlink-update {sessionId, linkPath, target}` — re-points an existing
/// symlink: readlink first (refuses non-symlinks), then delete + recreate
/// because SFTP has no in-place retarget. A failure between the two steps can
/// leave a dangling link — acceptable, the old target is already gone.
pub async fn symlink_update(
    runtime: &SshRuntime,
    session_id: &str,
    link_path: &str,
    target: &str,
) -> Result<(), String> {
    runtime.ensure_writable(session_id).await?;
    let target = clean_symlink_arg(target, "Link target")?;
    let link_path = normalize_remote_path(&clean_symlink_arg(link_path, "Link path")?)?;
    let sftp = runtime.sftp(session_id).await?;
    let session = sftp.lock().await;
    session
        .read_link(link_path.clone())
        .await
        .map_err(|_| format!("'{link_path}' is not a symbolic link"))?;
    session
        .remove_file(link_path.clone())
        .await
        .map_err(sftp_error)?;
    session.symlink(link_path, target).await.map_err(sftp_error)
}

/// Validates a symlink argument (link path or target): non-empty after trim,
/// free of NUL and line-break bytes. Those bytes are never legal in SFTP path
/// strings and rejecting them up front keeps logs/dialogs well-formed.
fn clean_symlink_arg(value: &str, label: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} is required"));
    }
    if trimmed.contains('\0') {
        return Err(format!("{label} must not contain NUL bytes"));
    }
    if trimmed.contains('\n') || trimmed.contains('\r') {
        return Err(format!("{label} must not contain line breaks"));
    }
    Ok(trimmed.to_string())
}

// ---------------------------------------------------------------------------
// Remote-edit round-trip upload (`sftp/upload-local`)
// ---------------------------------------------------------------------------
// The `watch/file-modified` flow hands a local copy out under
// `<downloads>/remote-edit/`; this call pushes it back. Without a gate this
// would be an arbitrary local-file read primitive, so uploads are accepted
// only for files that canonicalize beneath the watcher's remote-edit root.

/// Cap for one `sftp/upload-local` round-trip. Deliberately well below the
/// streaming-transfer limit (16 GiB): an accidental watcher upload should fail
/// fast instead of inching along for hours.
pub const MAX_UPLOAD_LOCAL_SIZE: u64 = 256 * 1024 * 1024;

/// Copy buffer for the streaming local→SFTP push.
const UPLOAD_LOCAL_CHUNK: usize = 128 * 1024;

/// `sftp/upload-local {sessionId, localPath, remotePath} -> {path, size}` —
/// pushes a watcher-delivered local file back to the remote path, staged via
/// `.dbx-part-<uuid>` + atomic rename like `sftp/write`.
pub async fn upload_watched_file(
    runtime: &SshRuntime,
    session_id: &str,
    local_path: &str,
    remote_path: &str,
) -> Result<Value, String> {
    runtime.ensure_writable(session_id).await?;
    let local =
        validate_remote_edit_path(Path::new(local_path.trim()), &runtime.data_dir(), |key| {
            std::env::var_os(key)
        })?;
    let size = tokio::fs::metadata(&local)
        .await
        .map_err(|error| format!("Local file '{}' is unreadable: {error}", local.display()))?
        .len();
    if size > MAX_UPLOAD_LOCAL_SIZE {
        return Err(format!(
            "Remote-edit uploads are limited to {MAX_UPLOAD_LOCAL_SIZE} bytes"
        ));
    }
    let remote_path = normalize_remote_path(remote_path)?;
    let sftp = runtime.sftp(session_id).await?;
    let task_id = Uuid::new_v4().to_string();
    let (temporary, backup) = direct_write_paths(&remote_path, &task_id);
    {
        let session = sftp.lock().await;
        let mut file = session
            .create(temporary.clone())
            .await
            .map_err(sftp_error)?;
        let mut reader = match tokio::fs::File::open(&local).await {
            Ok(reader) => reader,
            Err(error) => {
                let _ = session.remove_file(temporary.clone()).await;
                return Err(format!(
                    "Local file '{}' is unreadable: {error}",
                    local.display()
                ));
            }
        };
        let mut buffer = vec![0u8; UPLOAD_LOCAL_CHUNK];
        loop {
            let read = match reader.read(&mut buffer).await {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) => {
                    drop(file);
                    let _ = session.remove_file(temporary.clone()).await;
                    return Err(format!(
                        "Local file '{}' read failed: {error}",
                        local.display()
                    ));
                }
            };
            if let Err(error) = file.write_all(&buffer[..read]).await {
                drop(file);
                let _ = session.remove_file(temporary.clone()).await;
                return Err(format!("SFTP write failed: {error}"));
            }
        }
        if let Err(error) = file.flush().await {
            drop(file);
            let _ = session.remove_file(temporary.clone()).await;
            return Err(format!("SFTP write flush failed: {error}"));
        }
    }
    commit_temporary_file(&sftp, &temporary, &remote_path, &backup).await?;
    Ok(json!({ "path": remote_path, "size": size }))
}

/// Security gate: the local file must exist and canonicalize beneath
/// `<downloads>/remote-edit/`. Both sides are canonicalized (macOS hands out
/// `/var` vs `/private/var` through `$HOME` vs `canonicalize`), and a missing
/// remote-edit root means nothing the watcher produced can live there. The
/// env lookup is injected so tests can pin the downloads root to a tempdir.
fn validate_remote_edit_path(
    local_path: &Path,
    data_dir: &Path,
    lookup: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Result<PathBuf, String> {
    if !local_path.is_absolute() {
        return Err("localPath must be an absolute path".to_string());
    }
    let canonical = local_path.canonicalize().map_err(|error| {
        format!(
            "Local file '{}' does not exist: {error}",
            local_path.display()
        )
    })?;
    let downloads = crate::local_downloads::downloads_base_dir(lookup, data_dir);
    let root = downloads.join("remote-edit");
    let root = root
        .canonicalize()
        .map_err(|_| "Local file is not inside the remote-edit directory".to_string())?;
    if !canonical.starts_with(&root) {
        return Err("Only files inside the remote-edit directory can be uploaded".to_string());
    }
    Ok(canonical)
}

// ---------------------------------------------------------------------------
// Helpers (pure, unit-testable)
// ---------------------------------------------------------------------------

/// Single-quote shell escaping for embedding a path in a remote command;
/// byte-for-byte compatible with `exec::shell_quote`.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Octal permission string in `0755` style (type bits masked away).
fn format_mode(value: u32) -> String {
    format!("{:04o}", value & 0o7777)
}

fn current_unix_secs() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as u32)
        .unwrap_or(0)
}

fn sftp_error(error: impl std::fmt::Display) -> String {
    format!("SFTP operation failed: {error}")
}

/// Decodes a base64 payload and enforces the direct-write size cap.
fn decode_direct_write_payload(data_base64: &str) -> Result<Vec<u8>, String> {
    let data = BASE64_STANDARD
        .decode(data_base64)
        .map_err(|error| format!("Invalid base64 file data: {error}"))?;
    ensure_direct_write_size(data.len())?;
    Ok(data)
}

fn ensure_direct_write_size(size: usize) -> Result<(), String> {
    if size > MAX_DIRECT_WRITE_SIZE {
        Err(format!(
            "Direct SFTP writes are limited to {MAX_DIRECT_WRITE_SIZE} bytes; use the streaming upload slot (sftp/upload/start) instead"
        ))
    } else {
        Ok(())
    }
}

/// Temporary and backup names placed next to the write target, mirroring the
/// `.dbx-upload-<task>.part` pattern of the streaming upload path.
fn direct_write_paths(target: &str, task_id: &str) -> (String, String) {
    let (parent, _) = target.rsplit_once('/').unwrap_or(("/", ""));
    let parent = if parent.is_empty() { "/" } else { parent };
    let base = format!("{}/.dbx-part-{task_id}", parent.trim_end_matches('/'));
    (base.clone(), format!("{base}.backup"))
}

/// Renames a finished temporary file onto its target; an existing target is
/// moved aside first and restored if the rename fails. The target's
/// permission bits ride along onto the staged file, so an overwritten script
/// keeps its executable bit (issue #37).
async fn commit_temporary_file(
    sftp: &Arc<AsyncMutex<SftpSession>>,
    temporary: &str,
    target: &str,
    backup: &str,
) -> Result<(), String> {
    let target_attributes = sftp.lock().await.metadata(target.to_string()).await.ok();
    apply_preserved_permissions(sftp, temporary, target_attributes.as_ref()).await?;
    let target_exists = target_attributes.is_some();
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

/// Validates and bounds a caller-provided file name for the unique-name
/// probe: no path separators, no `.`/`..`, capped length.
fn clean_unique_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("File name is required".to_string());
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err("File name must not contain path separators".to_string());
    }
    if trimmed == "." || trimmed == ".." {
        return Err("File name must not be a relative path component".to_string());
    }
    Ok(trimmed.chars().take(MAX_UNIQUE_NAME_CHARS).collect())
}

/// The n-th collision candidate: `(n)` inserted before the last extension
/// (`report.pdf` -> `report(1).pdf`); names without a usable extension get a
/// suffix instead (`report` -> `report(1)`, `.bashrc` -> `.bashrc(1)`).
fn unique_name_candidate(name: &str, n: u32) -> String {
    match name.rfind('.') {
        Some(index) if index > 0 => format!("{}({n}){}", &name[..index], &name[index..]),
        _ => format!("{name}({n})"),
    }
}

/// Full probe order for a name: the plain name first, then the bounded
/// `name(1)..name(999)` collision candidates.
fn unique_name_candidates(name: &str) -> Vec<String> {
    let mut candidates = Vec::with_capacity(UNIQUE_NAME_PROBE_LIMIT as usize + 1);
    candidates.push(name.to_string());
    for n in 1..=UNIQUE_NAME_PROBE_LIMIT {
        candidates.push(unique_name_candidate(name, n));
    }
    candidates
}

/// Pure decision core of [`rename_unique`]: the first candidate the `exists`
/// probe reports as free, paired with whether a `(n)` rename was needed.
fn next_unique_name<F>(name: &str, mut exists: F) -> Option<(String, bool)>
where
    F: FnMut(&str) -> bool,
{
    for (index, candidate) in unique_name_candidates(name).into_iter().enumerate() {
        if !exists(&candidate) {
            return Some((candidate, index > 0));
        }
    }
    None
}

/// Joins a normalized directory with a bare file name (`/` root included).
fn join_remote_name(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// Trims, drops blanks and normalizes every source path; errors when nothing
/// usable remains.
fn clean_source_paths(source_paths: &[String]) -> Result<Vec<String>, String> {
    let mut cleaned = Vec::new();
    for source in source_paths {
        let trimmed = source.trim();
        if trimmed.is_empty() {
            continue;
        }
        cleaned.push(normalize_remote_path(trimmed)?);
    }
    if cleaned.is_empty() {
        return Err("At least one source path is required".to_string());
    }
    Ok(cleaned)
}

/// Longest common ancestor directory of the given absolute paths, ported from
/// tiny-rdm's `commonParentDir` with one fix: the last component of a sole
/// path is dropped, so archiving a single file still yields a usable `-C`
/// directory instead of the file itself.
fn common_parent_dir(paths: &[String]) -> String {
    let ancestors = paths
        .iter()
        .map(|path| {
            let mut components: Vec<&str> = path.trim_end_matches('/').split('/').collect();
            components.pop();
            components
        })
        .collect::<Vec<_>>();
    let mut common = ancestors.first().cloned().unwrap_or_default();
    for ancestor in &ancestors[1..] {
        let limit = common.len().min(ancestor.len());
        let mut shared = 0;
        while shared < limit && common[shared] == ancestor[shared] {
            shared += 1;
        }
        common.truncate(shared);
    }
    if common.len() <= 1 {
        return "/".to_string();
    }
    format!("/{}", common[1..].join("/"))
}

/// Path of `path` relative to its ancestor `parent` (result of
/// [`common_parent_dir`); `"."` as a defensive fallback, like tiny-rdm.
fn relative_to_parent(path: &str, parent: &str) -> String {
    let prefix = if parent == "/" { "" } else { parent };
    let relative = path.strip_prefix(&format!("{prefix}/")).unwrap_or(path);
    if relative.is_empty() {
        ".".to_string()
    } else {
        relative.to_string()
    }
}

/// `tar -czf <tmp> -C <parent> <rel...>` with every argument shell-quoted.
fn build_archive_command(temporary: &str, parent: &str, relatives: &[String]) -> String {
    let members = relatives
        .iter()
        .map(|relative| shell_quote(relative))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "tar -czf {} -C {} {}",
        shell_quote(temporary),
        shell_quote(parent),
        members
    )
}

/// `tar -t[z]f <archive>` — member listing used by the overwrite pre-check.
fn build_tar_list_command(archive: &str, compressed: bool) -> String {
    let z_flag = if compressed { "z" } else { "" };
    format!("tar -t{z_flag}f {}", shell_quote(archive))
}

/// `mkdir -p <dest> && tar -x[z]f <archive> -C <dest>`.
fn build_extract_command(archive: &str, destination: &str, compressed: bool) -> String {
    let z_flag = if compressed { "z" } else { "" };
    format!(
        "mkdir -p {} && tar -x{z_flag}f {} -C {}",
        shell_quote(destination),
        shell_quote(archive),
        shell_quote(destination)
    )
}

/// Whether the archive name calls for the gzip `z` flag; `.zip` and anything
/// else are rejected, matching tiny-rdm's tar-only support.
fn tar_z_flag(archive: &str) -> Result<bool, String> {
    let name = archive.to_ascii_lowercase();
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Ok(true)
    } else if name.ends_with(".tar") {
        Ok(false)
    } else if name.ends_with(".zip") {
        Err(
            "unsupported archive type: .zip archives are not supported, re-pack as .tar.gz"
                .to_string(),
        )
    } else {
        Err(format!(
            "unsupported archive type '{archive}': only .tar.gz, .tgz and .tar are supported"
        ))
    }
}

/// Distinct top-level entry names from a `tar -t` listing. `./dir/file`,
/// `dir/` and `name` all reduce to their first component; `..` members and
/// blank lines are ignored.
fn unique_top_level_entries(listing: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for line in listing.lines() {
        let trimmed = line.trim().trim_start_matches("./");
        let name = trimmed.split('/').next().unwrap_or_default();
        if name.is_empty() || name == ".." {
            continue;
        }
        if seen.insert(name.to_string()) {
            entries.push(name.to_string());
        }
    }
    entries
}

/// Turns an exec outcome (`{success, output, exitCode}`) into an error when
/// the remote command failed.
fn check_exec_success(outcome: &Value, operation: &str) -> Result<(), String> {
    let exit_code = outcome
        .get("exitCode")
        .and_then(Value::as_i64)
        .unwrap_or(-1);
    if exit_code == 0 {
        Ok(())
    } else {
        let output = outcome
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        Err(format!(
            "{operation} exited with status {exit_code}: {output}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_wraps_and_escapes_single_quotes() {
        assert_eq!(shell_quote("/var/log/app log"), "'/var/log/app log'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote("a'b'c"), r"'a'\''b'\''c'");
        assert_eq!(shell_quote("'; rm -rf /"), r"''\''; rm -rf /'");
    }

    #[test]
    fn format_mode_masks_type_bits() {
        assert_eq!(format_mode(0o100644), "0644");
        assert_eq!(format_mode(0o040755), "0755");
        assert_eq!(format_mode(0o120777), "0777");
    }

    #[test]
    fn common_parent_dir_handles_single_and_nested_paths() {
        // Single file: the file name is dropped, unlike tiny-rdm.
        assert_eq!(
            common_parent_dir(&["/var/log/app.log".to_string()]),
            "/var/log"
        );
        // Siblings share their directory.
        assert_eq!(
            common_parent_dir(&["/a/b.txt".to_string(), "/a/c.txt".to_string()]),
            "/a"
        );
        // Nested sources keep the deeper shared directory.
        assert_eq!(
            common_parent_dir(&["/a/b/c.txt".to_string(), "/a/b/d/e.txt".to_string()]),
            "/a/b"
        );
        // Disjoint paths fall back to the root.
        assert_eq!(
            common_parent_dir(&["/x/1".to_string(), "/y/2".to_string()]),
            "/"
        );
        // A source that is itself an ancestor archives relative to its parent.
        assert_eq!(
            common_parent_dir(&["/tmp".to_string(), "/tmp/a.txt".to_string()]),
            "/"
        );
    }

    #[test]
    fn relative_to_parent_strips_the_common_directory() {
        assert_eq!(
            relative_to_parent("/var/log/app.log", "/var/log"),
            "app.log"
        );
        assert_eq!(relative_to_parent("/a/b/c", "/a"), "b/c");
        assert_eq!(relative_to_parent("/etc", "/"), "etc");
    }

    #[test]
    fn archive_command_quotes_every_argument() {
        let command = build_archive_command(
            "/tmp/arc.tar.gz.tmp",
            "/var/log",
            &["my app.log".to_string(), "it's".to_string()],
        );
        assert_eq!(
            command,
            "tar -czf '/tmp/arc.tar.gz.tmp' -C '/var/log' 'my app.log' 'it'\\''s'"
        );
    }

    #[test]
    fn tar_z_flag_dispatches_by_extension() {
        assert!(tar_z_flag("/tmp/a.tar.gz").unwrap());
        assert!(tar_z_flag("/tmp/a.TGZ").unwrap());
        assert!(!tar_z_flag("/tmp/a.tar").unwrap());
        let zip = tar_z_flag("/tmp/a.zip").unwrap_err();
        assert!(zip.contains("unsupported"), "{zip}");
        assert!(tar_z_flag("/tmp/a.rar").is_err());
    }

    #[test]
    fn tar_commands_carry_the_z_flag_and_destination() {
        assert_eq!(
            build_tar_list_command("/tmp/a.tgz", true),
            "tar -tzf '/tmp/a.tgz'"
        );
        assert_eq!(
            build_extract_command("/tmp/a.tar", "/opt/app", false),
            "mkdir -p '/opt/app' && tar -xf '/tmp/a.tar' -C '/opt/app'"
        );
        assert_eq!(
            build_extract_command("/tmp/a.tgz", "/opt/app", true),
            "mkdir -p '/opt/app' && tar -xzf '/tmp/a.tgz' -C '/opt/app'"
        );
    }

    #[test]
    fn top_level_entries_reduce_tar_listings() {
        assert_eq!(
            unique_top_level_entries("./dir1/\n./dir1/a.txt\ndir2/b/c\nroot.txt\n"),
            vec![
                "dir1".to_string(),
                "dir2".to_string(),
                "root.txt".to_string()
            ]
        );
        // Path traversal members and noise never produce entries.
        assert!(unique_top_level_entries("../evil\n..\n\n").is_empty());
    }

    #[test]
    fn direct_write_size_limit_rejects_oversized_payloads() {
        assert_eq!(ensure_direct_write_size(0).unwrap(), ());
        assert_eq!(ensure_direct_write_size(MAX_DIRECT_WRITE_SIZE).unwrap(), ());
        let error = ensure_direct_write_size(MAX_DIRECT_WRITE_SIZE + 1).unwrap_err();
        assert!(error.contains("streaming upload"), "{error}");
    }

    #[test]
    fn decode_direct_write_payload_handles_base64_edges() {
        assert_eq!(decode_direct_write_payload("").unwrap(), Vec::<u8>::new());
        assert_eq!(
            decode_direct_write_payload("aGVsbG8=").unwrap(),
            b"hello".to_vec()
        );
        // Exactly 4 MiB of decoded zeros stays within the cap (boundary).
        let boundary = format!("{}AA==", "A".repeat(4 * (MAX_DIRECT_WRITE_SIZE - 1) / 3));
        assert!(decode_direct_write_payload(&boundary).is_ok());
        assert!(decode_direct_write_payload("not*base64").is_err());
    }

    #[test]
    fn direct_write_paths_live_next_to_the_target() {
        let (temporary, backup) = direct_write_paths("/home/user/file.txt", "abc");
        assert_eq!(temporary, "/home/user/.dbx-part-abc");
        assert_eq!(backup, "/home/user/.dbx-part-abc.backup");
        let (root_temp, _) = direct_write_paths("/file.txt", "abc");
        assert_eq!(root_temp, "/.dbx-part-abc");
    }

    #[test]
    fn exec_failures_report_status_and_output() {
        assert!(check_exec_success(&json!({ "exitCode": 0 }), "extract").is_ok());
        let error =
            check_exec_success(&json!({ "exitCode": 2, "output": "tar: eof\n" }), "archive")
                .unwrap_err();
        assert!(error.starts_with("archive exited with status 2: tar: eof"));
    }

    #[test]
    fn clean_unique_name_rejects_paths_and_bounds_length() {
        assert_eq!(clean_unique_name("  report.pdf ").unwrap(), "report.pdf");
        assert!(clean_unique_name("").is_err());
        assert!(clean_unique_name("a/b.pdf").is_err());
        assert!(clean_unique_name("a\\b.pdf").is_err());
        assert!(clean_unique_name(".").is_err());
        assert!(clean_unique_name("..").is_err());
        let long = "x".repeat(300);
        assert_eq!(clean_unique_name(&long).unwrap().chars().count(), 255);
    }

    #[test]
    fn unique_name_candidate_inserts_before_the_last_extension() {
        assert_eq!(unique_name_candidate("report.pdf", 1), "report(1).pdf");
        assert_eq!(
            unique_name_candidate("archive.tar.gz", 12),
            "archive.tar(12).gz"
        );
        assert_eq!(unique_name_candidate("report", 3), "report(3)");
        // Hidden files keep their leading dot untouched.
        assert_eq!(unique_name_candidate(".bashrc", 2), ".bashrc(2)");
    }

    #[test]
    fn next_unique_name_picks_the_first_free_candidate() {
        let free = |_: &str| false;
        assert_eq!(
            next_unique_name("report.pdf", free),
            Some(("report.pdf".to_string(), false))
        );
        let taken = |candidate: &str| candidate == "report.pdf" || candidate == "report(1).pdf";
        assert_eq!(
            next_unique_name("report.pdf", taken),
            Some(("report(2).pdf".to_string(), true))
        );
        // Exhausting the probe budget yields None (handler turns it into an error).
        let everything = |_: &str| true;
        assert_eq!(next_unique_name("report.pdf", everything), None);
    }

    #[test]
    fn unique_name_candidates_start_with_the_plain_name() {
        let candidates = unique_name_candidates("a.txt");
        assert_eq!(candidates.first().unwrap(), "a.txt");
        assert_eq!(candidates.get(1).unwrap(), "a(1).txt");
        assert_eq!(candidates.len(), (UNIQUE_NAME_PROBE_LIMIT + 1) as usize);
        assert_eq!(candidates.last().unwrap(), "a(999).txt");
    }

    #[test]
    fn join_remote_name_handles_the_root() {
        assert_eq!(join_remote_name("/", "a.txt"), "/a.txt");
        assert_eq!(join_remote_name("/tmp/up", "a.txt"), "/tmp/up/a.txt");
    }

    // ---- symlink argument validation ----

    #[test]
    fn clean_symlink_arg_trims_and_accepts_relative_targets() {
        assert_eq!(
            clean_symlink_arg("  ../lib/libssl.so  ", "Link target").unwrap(),
            "../lib/libssl.so"
        );
        assert_eq!(
            clean_symlink_arg("/etc/alternatives/java", "Link path").unwrap(),
            "/etc/alternatives/java"
        );
    }

    #[test]
    fn clean_symlink_arg_rejects_empty_nul_and_line_breaks() {
        assert!(clean_symlink_arg("", "Link target").is_err());
        assert!(clean_symlink_arg("   ", "Link path").is_err());
        assert!(clean_symlink_arg("a\0b", "Link target").is_err());
        assert!(clean_symlink_arg("a\nb", "Link path").is_err());
        assert!(clean_symlink_arg("a\r\nb", "Link target").is_err());
        let error = clean_symlink_arg("", "Link target").unwrap_err();
        assert!(error.contains("Link target"), "{error}");
    }

    // ---- remote-edit upload gate ----

    /// Pins the downloads base dir to `base` via the injected lookup, then
    /// lays out `<base>/remote-edit/<ts>/notes.txt` plus an outside file —
    /// the same shape the watcher hands out in production.
    fn remote_edit_gate(base: &Path, path: &Path) -> Result<PathBuf, String> {
        validate_remote_edit_path(path, base, |key| {
            if key == crate::local_downloads::DOWNLOAD_DIR_ENV {
                Some(std::ffi::OsString::from(base.to_path_buf()))
            } else {
                std::env::var_os(key)
            }
        })
    }

    fn write_remote_edit_fixture(dir: &tempfile::TempDir) -> PathBuf {
        let root = dir.path().join("remote-edit");
        let session = root.join("1700000000000");
        std::fs::create_dir_all(&session).expect("mkdir remote-edit/<ts>");
        std::fs::write(session.join("notes.txt"), b"edited").expect("write file");
        std::fs::write(dir.path().join("outside.txt"), b"nope").expect("write outside");
        root
    }

    #[test]
    fn remote_edit_gate_accepts_files_under_the_watch_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = write_remote_edit_fixture(&dir);
        let watched = root.join("1700000000000").join("notes.txt");
        let resolved = remote_edit_gate(dir.path(), &watched).expect("watched file accepted");
        assert!(resolved.ends_with("notes.txt"));
        // A stale-but-similar path still resolves through the canonical form.
        let nested = root
            .join("1700000000000")
            .join("..")
            .join("1700000000000")
            .join("notes.txt");
        assert!(remote_edit_gate(dir.path(), &nested).is_ok());
    }

    #[test]
    fn remote_edit_gate_rejects_outside_relative_and_missing_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = write_remote_edit_fixture(&dir);
        // Sibling of the remote-edit root.
        let error = remote_edit_gate(dir.path(), &dir.path().join("outside.txt")).unwrap_err();
        assert!(error.contains("remote-edit"), "{error}");
        // A file whose root component merely shares the name.
        let decoy = dir.path().join("remote-edit-evil").join("notes.txt");
        std::fs::create_dir_all(decoy.parent().unwrap()).expect("mkdir decoy");
        std::fs::write(&decoy, b"nope").expect("write decoy");
        assert!(remote_edit_gate(dir.path(), &decoy).is_err());
        // Relative paths and missing files are refused outright.
        assert!(remote_edit_gate(dir.path(), Path::new("relative/notes.txt")).is_err());
        assert!(remote_edit_gate(dir.path(), &root.join("gone.txt")).is_err());
        // A missing remote-edit root can never host a legitimate file.
        let empty = tempfile::tempdir().expect("tempdir");
        let orphan = empty.path().join("remote-edit").join("1").join("f.txt");
        assert!(remote_edit_gate(empty.path(), &orphan).is_err());
    }

    #[test]
    fn upload_local_size_cap_is_bounded() {
        assert_eq!(MAX_UPLOAD_LOCAL_SIZE, 256 * 1024 * 1024);
    }
}

// ---------------------------------------------------------------------------
// main.rs registration
// ---------------------------------------------------------------------------
// `mod sftp_ext;` is declared next to the other module declarations. The
// match arms below drop into `Plugin::handle_request` (parameters follow the
// existing camelCase convention):
//
//     "sftp/stat" => {
//         let session_id = required_string(&params, "sessionId")?;
//         let path = required_string(&params, "path")?;
//         self.runtime
//             .block_on(sftp_ext::stat(&self.ssh, session_id, path))
//     }
//     "sftp/exists" => {
//         let session_id = required_string(&params, "sessionId")?;
//         let path = required_string(&params, "path")?;
//         let exists = self
//             .runtime
//             .block_on(sftp_ext::exists(&self.ssh, session_id, path))?;
//         Ok(json!({ "exists": exists }))
//     }
//     "sftp/touch" => {
//         let session_id = required_string(&params, "sessionId")?;
//         let path = required_string(&params, "path")?;
//         self.runtime
//             .block_on(sftp_ext::touch(&self.ssh, session_id, path))?;
//         Ok(json!({ "success": true }))
//     }
//     "sftp/write" => {
//         let session_id = required_string(&params, "sessionId")?;
//         let remote_path = required_string(&params, "remotePath")?;
//         let data_base64 = required_string(&params, "dataBase64")?;
//         self.runtime.block_on(sftp_ext::write_file(
//             &self.ssh,
//             session_id,
//             remote_path,
//             data_base64,
//         ))?;
//         Ok(json!({ "success": true }))
//     }
//     "sftp/archive" => {
//         let session_id = required_string(&params, "sessionId")?;
//         let source_paths = params
//             .get("sourcePaths")
//             .and_then(Value::as_array)
//             .ok_or("Missing sourcePaths")?
//             .iter()
//             .map(|value| {
//                 value
//                     .as_str()
//                     .map(str::to_string)
//                     .ok_or_else(|| "sourcePaths must be strings".to_string())
//             })
//             .collect::<Result<Vec<String>, String>>()?;
//         let archive_path = required_string(&params, "archivePath")?;
//         self.runtime.block_on(sftp_ext::archive(
//             &self.ssh,
//             session_id,
//             &source_paths,
//             archive_path,
//         ))
//     }
//     "sftp/extract" => {
//         let session_id = required_string(&params, "sessionId")?;
//         let archive_path = required_string(&params, "archivePath")?;
//         let destination_path = required_string(&params, "destinationPath")?;
//         let overwrite = params
//             .get("overwrite")
//             .and_then(Value::as_bool)
//             .unwrap_or(false);
//         self.runtime.block_on(sftp_ext::extract(
//             &self.ssh,
//             session_id,
//             archive_path,
//             destination_path,
//             overwrite,
//         ))?;
//         Ok(json!({ "success": true }))
//     }
