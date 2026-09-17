//! Local filesystem browsing for the in-app folder picker (`local/fs/browse`,
//! `local/fs/drives`). The sandboxed workbench iframe (opaque origin) has no
//! directory-picker API, so the desktop sidecar lists directories on the
//! user's machine — directories only, never file contents, and each listing
//! is capped. Same trust boundary as the other `local/*` methods: the caller
//! is our own plugin UI, and the paths stay on the user's machine.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// Cap per listing so a huge directory cannot stall the picker.
const MAX_ENTRIES: usize = 500;

/// Windows drive letters for the picker's "This PC" page (A/B are floppy-era
/// and probing them can hang on legacy hardware). Empty on other platforms —
/// the picker then starts at the default directory instead.
pub fn list_local_drives() -> Vec<String> {
    let mut out = Vec::new();
    if cfg!(windows) {
        for letter in b'C'..=b'Z' {
            let drive = format!("{}:\\", letter as char);
            if Path::new(&drive).exists() {
                out.push(drive);
            }
        }
    }
    out
}

/// Lists the subdirectories of `path` (default download dir when absent or
/// empty), name-sorted case-insensitively. Returns `{ path, parent, entries }`
/// with directories only; symlinks to directories are followed, entries that
/// cannot be stat'ed are skipped. Errors when the target is not an existing,
/// readable absolute directory.
pub fn browse_local_dir(path: Option<&str>, data_dir: &Path) -> Result<Value, String> {
    let dir = path
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            crate::local_downloads::downloads_base_dir(|key| std::env::var_os(key), data_dir)
        });
    if !dir.is_absolute() {
        return Err("Path must be absolute".to_string());
    }
    let metadata = std::fs::metadata(&dir)
        .map_err(|error| format!("Cannot open '{}': {error}", dir.display()))?;
    if !metadata.is_dir() {
        return Err(format!("'{}' is not a directory", dir.display()));
    }
    let read = std::fs::read_dir(&dir)
        .map_err(|error| format!("Cannot read '{}': {error}", dir.display()))?;
    let mut entries: Vec<(String, String)> = Vec::new();
    for entry in read.flatten() {
        // metadata() follows symlinks so linked folders stay navigable.
        let is_dir = entry.metadata().map(|meta| meta.is_dir()).unwrap_or(false);
        if !is_dir {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        entries.push((name, entry.path().to_string_lossy().into_owned()));
        if entries.len() >= MAX_ENTRIES {
            break;
        }
    }
    entries.sort_by_key(|entry| entry.0.to_lowercase());
    Ok(json!({
        "path": dir.to_string_lossy(),
        "parent": dir.parent().map(|parent| parent.to_string_lossy()),
        "entries": entries
            .iter()
            .map(|(name, path)| json!({ "name": name, "path": path, "is_dir": true }))
            .collect::<Vec<_>>(),
    }))
}

/// Pre-download conflict probe for the "ask me" policy: reports whether
/// `<dir>/<sanitized name>` already exists, plus the exact candidate path so
/// the prompt can show it verbatim.
pub fn target_exists(dir: &str, name: &str) -> Result<Value, String> {
    let dir = dir.trim();
    if dir.is_empty() {
        return Err("Directory is required".to_string());
    }
    let dir_path = PathBuf::from(dir);
    if !dir_path.is_absolute() {
        return Err("Directory must be an absolute path".to_string());
    }
    let candidate = dir_path.join(crate::local_downloads::sanitize_file_name(name));
    Ok(json!({
        "exists": candidate.exists(),
        "path": candidate.to_string_lossy(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browse_lists_dirs_sorted_and_skips_files() {
        let data_dir = tempfile::tempdir().expect("tempdir");
        let root = tempfile::tempdir().expect("root");
        std::fs::create_dir_all(root.path().join("beta")).unwrap();
        std::fs::create_dir_all(root.path().join("Alpha")).unwrap();
        std::fs::write(root.path().join("file.txt"), "x").unwrap();
        let result =
            browse_local_dir(Some(root.path().to_str().unwrap()), data_dir.path()).expect("browse");
        let names: Vec<&str> = result["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["Alpha", "beta"]);
        assert_eq!(
            result["parent"].as_str().unwrap(),
            root.path().parent().unwrap().to_string_lossy()
        );
    }

    #[test]
    fn browse_rejects_relative_missing_and_file_paths() {
        let data_dir = tempfile::tempdir().expect("tempdir");
        assert!(browse_local_dir(Some("relative/dir"), data_dir.path()).is_err());
        assert!(browse_local_dir(Some("/no/such/path/at/all"), data_dir.path()).is_err());
        let file = data_dir.path().join("f.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(browse_local_dir(Some(file.to_str().unwrap()), data_dir.path()).is_err());
    }

    #[test]
    fn browse_default_falls_back_to_download_dir() {
        let data_dir = tempfile::tempdir().expect("tempdir");
        let result = browse_local_dir(None, data_dir.path()).expect("browse default");
        assert!(result["path"].as_str().unwrap().len() > 1);
    }

    #[test]
    fn target_exists_reports_collision_candidate() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("report.pdf"), "x").unwrap();
        let hit = target_exists(dir.path().to_str().unwrap(), "report.pdf").expect("probe");
        assert_eq!(hit["exists"].as_bool(), Some(true));
        let miss = target_exists(dir.path().to_str().unwrap(), "other.pdf").expect("probe");
        assert_eq!(miss["exists"].as_bool(), Some(false));
        assert!(target_exists("relative", "a.txt").is_err());
        assert!(target_exists("", "a.txt").is_err());
    }

    #[test]
    fn drives_empty_off_windows() {
        if cfg!(windows) {
            assert!(list_local_drives().iter().any(|drive| drive == "C:\\"));
        } else {
            assert!(list_local_drives().is_empty());
        }
    }
}
