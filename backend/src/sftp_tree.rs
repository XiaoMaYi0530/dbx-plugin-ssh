//! Pure bookkeeping for recursive SFTP folder downloads (`sftp/download/tree/start`).
//!
//! The remote walk itself lives in `ssh.rs` (it needs the live SFTP session);
//! everything that must be reasonable without a connection — component
//! sanitizing, local path containment, scan caps, symlink skipping, per-file
//! size limits and the failure summary — is collected here so it is unit
//! testable. No shell, no remote temp archives: each regular file streams
//! through the existing chunked download pipeline and lands under a freshly
//! created, collision-free local root directory.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// Hard caps for one folder download. A tree beyond any of these refuses to
/// start instead of silently truncating (the caller sees the error).
pub const MAX_TREE_FILES: usize = 50_000;
pub const MAX_TREE_DIRS: usize = 10_000;
/// Maximum path depth (component count) under the root; deeper subtrees are
/// pruned and recorded as failures, not followed.
pub const MAX_TREE_DEPTH: usize = 64;
/// Per-file byte cap mirrors the single-file transfer limit: files larger than
/// this are recorded as failures and skipped rather than aborting the tree.
pub const MAX_TREE_FILE_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// The finish summary reports at most this many failing paths inline; the full
/// count always travels alongside so nothing is lost.
pub const MAX_REPORTED_FAILURES: usize = 50;

/// One queued regular file of a folder download: `relative` is the sanitized,
/// root-relative local layout, `remote_path` the untouched server-side path it
/// streams from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeFile {
    pub relative: String,
    pub remote_path: String,
    pub size: u64,
}

/// Capacity exhaustion of a scan: the tree cannot be represented inside the
/// documented limits, so `start` must refuse instead of truncating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeScanCapacity(pub &'static str);

impl std::fmt::Display for TreeScanCapacity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// Accumulated scan outcome of one remote walk.
#[derive(Debug, Default, Clone)]
pub struct TreeScan {
    /// Directories to mirror locally (sanitized, root-relative; the root
    /// itself is not listed because the caller creates it separately).
    pub dirs: Vec<String>,
    /// Regular files in download order.
    pub files: Vec<TreeFile>,
    /// Per-path problems recorded while walking (unreadable dirs, unsafe
    /// names, oversized files); the tree still downloads what it can.
    pub failures: Vec<Value>,
    /// Symlinks and special entries skipped (symlinks are never followed, so
    /// cycles cannot loop the walk).
    pub skipped: u64,
    /// Aggregate byte size across accepted files (the progress denominator).
    total_bytes: u64,
}

impl TreeScan {
    pub fn new() -> Self {
        Self::default()
    }

    /// Component count of a sanitized relative path ("a/b" → 2).
    pub fn depth_of(relative: &str) -> usize {
        relative.split('/').count()
    }

    /// Registers a directory. `Ok(true)` means the caller may scan it;
    /// `Ok(false)` means it was rejected for the walk (already recorded as a
    /// failure — its local mirror is still created so the layout survives);
    /// `Err` aborts the whole download on capacity exhaustion.
    pub fn push_dir(&mut self, relative: &str) -> Result<bool, TreeScanCapacity> {
        if Self::depth_of(relative) > MAX_TREE_DEPTH {
            self.record_failure(
                relative,
                format!("path depth exceeds the {MAX_TREE_DEPTH}-level limit"),
            );
            return Ok(false);
        }
        if self.dirs.len() >= MAX_TREE_DIRS {
            return Err(TreeScanCapacity(
                "Folder download is limited to 10000 directories",
            ));
        }
        self.dirs.push(relative.to_string());
        Ok(true)
    }

    /// Registers a regular file. Oversized files are recorded as failures and
    /// skipped (matching what a plain single-file download would refuse).
    pub fn push_file(
        &mut self,
        relative: String,
        remote_path: String,
        size: u64,
    ) -> Result<(), TreeScanCapacity> {
        if self.files.len() >= MAX_TREE_FILES {
            return Err(TreeScanCapacity(
                "Folder download is limited to 50000 files",
            ));
        }
        if size > MAX_TREE_FILE_BYTES {
            self.record_failure(
                &relative,
                format!("file exceeds the {MAX_TREE_FILE_BYTES}-byte per-file transfer limit"),
            );
            return Ok(());
        }
        self.total_bytes = self.total_bytes.saturating_add(size);
        self.files.push(TreeFile {
            relative,
            remote_path,
            size,
        });
        Ok(())
    }

    /// Symlink / special entry: never followed, counted only.
    pub fn skip(&mut self) {
        self.skipped = self.skipped.saturating_add(1);
    }

    /// Records a walk-time problem against a path (unreadable directory,
    /// unusable name, ...). Best-effort: the tree keeps downloading.
    pub fn record_failure(&mut self, path: &str, error: impl Into<String>) {
        self.failures
            .push(json!({ "path": path, "error": error.into() }));
    }

    /// Aggregate byte total across accepted files (the progress denominator).
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub fn file_count(&self) -> u64 {
        self.files.len() as u64
    }

    pub fn dir_count(&self) -> u64 {
        self.dirs.len() as u64
    }
}

/// Sanitizes one root-relative path for local mirroring: every '/'
/// component goes through the single-download file-name sanitizer, so "."
/// / ".." / separators / control characters can never traverse out of the
/// download root. Returns `None` when nothing usable remains.
pub fn sanitize_relative(relative: &str) -> Option<String> {
    if relative.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    for component in relative.split('/') {
        let sanitized = crate::local_downloads::sanitize_file_name(component);
        if sanitized.is_empty() {
            return None;
        }
        parts.push(sanitized);
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// Joins a sanitized relative path onto the download root and verifies the
/// result stays inside it (belt and braces beside `sanitize_relative`).
pub fn safe_tree_path(root: &Path, relative: &str) -> Option<PathBuf> {
    let sanitized = sanitize_relative(relative)?;
    let joined = root.join(sanitized);
    if joined.starts_with(root) {
        Some(joined)
    } else {
        None
    }
}

/// Whether the chunk loop has consumed the whole tree: no queued files left
/// and the in-flight file (if any) is fully transferred. Pure so the
/// aggregate-eof semantics stay unit tested.
pub fn tree_eof(files_remaining: usize, current_remaining: u64) -> bool {
    files_remaining == 0 && current_remaining == 0
}

/// Shapes the finish summary: full failure count plus at most
/// `MAX_REPORTED_FAILURES` sample entries inline.
pub fn failure_report(failures: &[Value]) -> (u64, Vec<Value>) {
    let count = failures.len() as u64;
    let sample = failures
        .iter()
        .take(MAX_REPORTED_FAILURES)
        .cloned()
        .collect();
    (count, sample)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_relative_neutralizes_traversal_and_separators() {
        assert_eq!(sanitize_relative("a/b/c.txt").as_deref(), Some("a/b/c.txt"));
        // "." and ".." collapse to the "download" fallback name like a plain
        // single-file download would; they can never escape the root.
        assert_eq!(sanitize_relative("..").as_deref(), Some("download"));
        assert_eq!(
            sanitize_relative("a/../../x").as_deref(),
            Some("a/download/download/x")
        );
        // Backslashes are separators on Windows targets: stripped per component.
        assert_eq!(sanitize_relative("a\\b/c").as_deref(), Some("ab/c"));
        assert_eq!(sanitize_relative("we\nird").as_deref(), Some("we ird"));
        // An empty relative path has no components at all.
        assert_eq!(sanitize_relative(""), None);
    }

    #[test]
    fn safe_tree_path_stays_under_root() {
        let root = Path::new("/tmp/dl");
        assert_eq!(
            safe_tree_path(root, "a/b.txt"),
            Some(PathBuf::from("/tmp/dl/a/b.txt"))
        );
        // A hostile component that survives sanitizing still cannot jump out.
        assert_eq!(
            safe_tree_path(root, ".."),
            Some(PathBuf::from("/tmp/dl/download"))
        );
        assert_eq!(safe_tree_path(root, ""), None);
        let escaped = safe_tree_path(root, "a/../../etc");
        if let Some(path) = escaped {
            assert!(path.starts_with(root));
        }
    }

    #[test]
    fn scan_records_files_dirs_and_skips() {
        let mut scan = TreeScan::new();
        scan.push_dir("a").expect("push dir");
        scan.push_dir("a/b").expect("push dir");
        scan.push_file("f1".into(), "/r/f1".into(), 3)
            .expect("push file");
        scan.push_file("a/f2".into(), "/r/a/f2".into(), 0)
            .expect("push file");
        scan.skip();
        assert_eq!(scan.file_count(), 2);
        assert_eq!(scan.dir_count(), 2);
        assert_eq!(scan.skipped, 1);
        assert_eq!(scan.total_bytes(), 3);
        assert_eq!(scan.files[0].relative, "f1");
        assert_eq!(scan.files[1].size, 0);
    }

    #[test]
    fn scan_records_failures_but_keeps_walking() {
        let mut scan = TreeScan::new();
        // Oversized file: recorded as a failure, not queued.
        scan.push_file("big".into(), "/r/big".into(), MAX_TREE_FILE_BYTES + 1)
            .expect("not a capacity error");
        assert_eq!(scan.file_count(), 0);
        assert_eq!(scan.failures.len(), 1);
        assert_eq!(scan.failures[0]["path"], "big");
        scan.record_failure("locked/a", "readdir failed");
        assert_eq!(scan.failures.len(), 2);
    }

    #[test]
    fn scan_rejects_deeper_than_limit() {
        let mut scan = TreeScan::new();
        let deep = (0..MAX_TREE_DEPTH + 1)
            .map(|_| "d")
            .collect::<Vec<_>>()
            .join("/");
        assert_eq!(scan.push_dir(&deep), Ok(false));
        assert!(scan.dirs.is_empty());
        assert_eq!(scan.failures.len(), 1);
        // Exactly at the limit is fine.
        let at_limit = (0..MAX_TREE_DEPTH)
            .map(|_| "d")
            .collect::<Vec<_>>()
            .join("/");
        assert_eq!(scan.push_dir(&at_limit), Ok(true));
    }

    #[test]
    fn scan_enforces_capacity() {
        let mut scan = TreeScan::new();
        for index in 0..MAX_TREE_FILES {
            scan.push_file(format!("f{index}"), format!("/r/f{index}"), 1)
                .expect("within capacity");
        }
        let error = scan
            .push_file("overflow".into(), "/r/overflow".into(), 1)
            .unwrap_err();
        assert!(error.to_string().contains("50000"));
        let mut dirs = TreeScan::new();
        for index in 0..MAX_TREE_DIRS {
            dirs.push_dir(&format!("d{index}"))
                .expect("within capacity");
        }
        assert!(dirs.push_dir("overflow").is_err());
    }

    #[test]
    fn eof_requires_queue_and_current_drained() {
        assert!(tree_eof(0, 0));
        assert!(!tree_eof(1, 0));
        assert!(!tree_eof(0, 1));
    }

    #[test]
    fn failure_report_caps_inline_sample() {
        let failures: Vec<Value> = (0..MAX_REPORTED_FAILURES + 10)
            .map(|index| json!({ "path": format!("f{index}"), "error": "denied" }))
            .collect();
        let (count, sample) = failure_report(&failures);
        assert_eq!(count, MAX_REPORTED_FAILURES as u64 + 10);
        assert_eq!(sample.len(), MAX_REPORTED_FAILURES);
        assert_eq!(sample[0]["path"], "f0");
        let (zero, empty) = failure_report(&[]);
        assert_eq!((zero, empty.len()), (0, 0));
    }
}
