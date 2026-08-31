//! Global Quick Sudo profiles: named credential/policy presets persisted in
//! `<plugin_data_dir>/quick-sudo-profiles.json` and selectable per connection
//! (tiny-rdm's global Manual Sudo + per-profile QuickSudo override, adapted
//! to the DBX plugin shape). Secrets live in the store only; every view
//! returned to callers reports `sudoPasswordSet` / `totpConfigured` booleans
//! and never echoes the values.

use std::collections::HashMap;
use std::path::Path;

use serde_json::{json, Value};

use crate::exec::{self, AuthFlowMode, SudoAuth};

const STORAGE_VERSION: u64 = 1;
const FILE_NAME: &str = "quick-sudo-profiles.json";
/// Cap aligned with the workbench quick-commands limit: enough for fleet
/// Segmentation (per environment / per team) without unbounded growth.
pub const MAX_PROFILES: usize = 20;
const MAX_NAME_LEN: usize = 64;

/// One named Quick Sudo preset. Secrets are stored verbatim (trimming happens
/// at use time in `SudoAuth::new`, mirroring the connection secret pipeline).
#[derive(Debug, Clone, PartialEq)]
pub struct SudoProfile {
    pub id: String,
    pub name: String,
    pub sudo_password: String,
    pub totp_secret: String,
    /// Canonical `AuthFlowMode` name (`password_then_otp` / …).
    pub auth_flow_mode: String,
    pub password_prompt_hint: String,
    pub totp_prompt_hint: String,
    pub sudo_use_pty: bool,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Default)]
pub struct SudoProfileStore {
    pub profiles: Vec<SudoProfile>,
    /// connectionId -> profileId. Dangling entries (profile deleted by a
    /// hand-edited file) resolve to "no binding" at lookup time instead of
    /// failing the connection.
    pub bindings: HashMap<String, String>,
}

fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub fn store_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join(FILE_NAME)
}

/// Loads the store; a missing or corrupted file yields an empty store so a
/// bad file can never break connecting (same policy as mcp-settings.json).
pub fn load_store(data_dir: &Path) -> SudoProfileStore {
    let text = std::fs::read_to_string(store_path(data_dir)).unwrap_or_default();
    let Some(value) = serde_json::from_str::<Value>(&text).ok() else {
        return SudoProfileStore::default();
    };
    let profiles = value
        .get("profiles")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(profile_from_json).collect())
        .unwrap_or_default();
    let bindings = value
        .get("bindings")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(connection, profile)| {
                    let profile = profile.as_str()?;
                    (!connection.is_empty() && !profile.is_empty())
                        .then(|| (connection.clone(), profile.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    SudoProfileStore { profiles, bindings }
}

/// Persists atomically (tmp + rename); the file carries 0600 permissions on
/// Unix because it holds sudo credentials.
pub fn save_store(data_dir: &Path, store: &SudoProfileStore) -> Result<(), String> {
    let path = store_path(data_dir);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let value = json!({
        "version": STORAGE_VERSION,
        "profiles": store.profiles.iter().map(profile_json).collect::<Vec<_>>(),
        "bindings": store.bindings,
    });
    let text = serde_json::to_string_pretty(&value)
        .map_err(|error| format!("Failed to encode Quick Sudo profiles: {error}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)
        .map_err(|error| format!("Failed to write {}: {error}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, &path)
        .map_err(|error| format!("Failed to write {}: {error}", path.display()))
}

fn profile_from_json(value: &Value) -> Option<SudoProfile> {
    let object = value.as_object()?;
    let string = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Some(SudoProfile {
        id: string("id"),
        name: string("name"),
        sudo_password: string("sudoPassword"),
        totp_secret: string("totpSecret"),
        auth_flow_mode: string("authFlowMode"),
        password_prompt_hint: string("passwordPromptHint"),
        totp_prompt_hint: string("totpPromptHint"),
        sudo_use_pty: object.get("sudoUsePty").and_then(Value::as_bool).unwrap_or(false),
        created_at: object.get("createdAt").and_then(Value::as_u64).unwrap_or(0),
        updated_at: object.get("updatedAt").and_then(Value::as_u64).unwrap_or(0),
    })
}

fn profile_json(profile: &SudoProfile) -> Value {
    json!({
        "id": profile.id,
        "name": profile.name,
        "sudoPassword": profile.sudo_password,
        "totpSecret": profile.totp_secret,
        "authFlowMode": profile.auth_flow_mode,
        "passwordPromptHint": profile.password_prompt_hint,
        "totpPromptHint": profile.totp_prompt_hint,
        "sudoUsePty": profile.sudo_use_pty,
        "createdAt": profile.created_at,
        "updatedAt": profile.updated_at,
    })
}

/// Caller-facing view: secret presence flags only, never the values.
pub fn profile_view(profile: &SudoProfile) -> Value {
    json!({
        "id": profile.id,
        "name": profile.name,
        "sudoPasswordSet": !profile.sudo_password.trim().is_empty(),
        "totpConfigured": !profile.totp_secret.trim().is_empty(),
        "authFlowMode": profile.auth_flow_mode,
        "passwordPromptHint": profile.password_prompt_hint,
        "totpPromptHint": profile.totp_prompt_hint,
        "sudoUsePty": profile.sudo_use_pty,
        "createdAt": profile.created_at,
        "updatedAt": profile.updated_at,
    })
}

/// Sorted by name (case-insensitive) so UI and MCP listings are stable.
pub fn list_views(store: &SudoProfileStore) -> Vec<Value> {
    let mut profiles = store.profiles.clone();
    profiles.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    profiles.iter().map(profile_view).collect()
}

fn validate_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Quick Sudo profile name is required".to_string());
    }
    if name.len() > MAX_NAME_LEN {
        return Err(format!(
            "Quick Sudo profile name is limited to {MAX_NAME_LEN} characters"
        ));
    }
    Ok(name.to_string())
}

/// Creates or updates one profile from the shared camelCase parameter shape
/// (protocol `sudo/profiles/save` and the MCP save tool). Empty
/// `sudoPassword`/`totpSecret` keeps the stored value; explicit
/// `clearSudoPassword`/`clearTotpSecret` booleans wipe them. Returns the
/// saved profile plus whether it was newly created.
pub fn save_profile(
    store: &mut SudoProfileStore,
    params: &Value,
) -> Result<(SudoProfile, bool), String> {
    let name = validate_name(
        params
            .get("name")
            .and_then(Value::as_str)
            .ok_or("Missing name")?,
    )?;
    let existing_id = params
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let existing = existing_id.and_then(|id| {
        store
            .profiles
            .iter()
            .position(|profile| profile.id == id)
    });
    // Case-insensitive uniqueness so the name stays an unambiguous MCP reference.
    if let Some(duplicate) = store.profiles.iter().find(|profile| {
        profile
            .name
            .eq_ignore_ascii_case(&name)
            && existing
                .map(|index| store.profiles[index].id != profile.id)
                .unwrap_or(true)
    }) {
        return Err(format!(
            "Quick Sudo profile name '{}' is already in use",
            duplicate.name
        ));
    }

    let string_input = |key: &str| -> Option<String> {
        params
            .get(key)
            .and_then(Value::as_str)
            .map(|value| value.to_string())
    };
    let flow_mode = match string_input("authFlowMode") {
        None => None,
        Some(value) if value.trim().is_empty() => None,
        Some(value) => {
            let trimmed = value.trim();
            if !matches!(
                trimmed,
                "password_only" | "password_plus_otp" | "password_then_otp"
            ) {
                return Err(format!("Unsupported authFlowMode '{trimmed}'"));
            }
            Some(trimmed.to_string())
        }
    };

    let created = existing.is_none();
    if created && store.profiles.len() >= MAX_PROFILES {
        return Err(format!(
            "At most {MAX_PROFILES} Quick Sudo profiles are supported"
        ));
    }
    let now = unix_now_secs();
    let mut profile = match existing {
        Some(index) => store.profiles[index].clone(),
        None => SudoProfile {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.clone(),
            sudo_password: String::new(),
            totp_secret: String::new(),
            auth_flow_mode: AuthFlowMode::PasswordThenOtp.name().to_string(),
            password_prompt_hint: String::new(),
            totp_prompt_hint: String::new(),
            sudo_use_pty: false,
            created_at: now,
            updated_at: now,
        },
    };
    profile.name = name;
    if let Some(value) = string_input("sudoPassword") {
        if !value.is_empty() {
            profile.sudo_password = value;
        }
    }
    if params
        .get("clearSudoPassword")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        profile.sudo_password = String::new();
    }
    if let Some(value) = string_input("totpSecret") {
        if !value.trim().is_empty() {
            profile.totp_secret = value;
        }
    }
    if params
        .get("clearTotpSecret")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        profile.totp_secret = String::new();
    }
    if let Some(value) = flow_mode {
        profile.auth_flow_mode = value;
    }
    if let Some(value) = string_input("passwordPromptHint") {
        profile.password_prompt_hint = exec::sanitize_prompt_hint(&value);
    }
    if let Some(value) = string_input("totpPromptHint") {
        profile.totp_prompt_hint = exec::sanitize_prompt_hint(&value);
    }
    if let Some(value) = params.get("sudoUsePty").and_then(Value::as_bool) {
        profile.sudo_use_pty = value;
    }
    profile.updated_at = now;

    match existing {
        Some(index) => store.profiles[index] = profile.clone(),
        None => store.profiles.push(profile.clone()),
    }
    Ok((profile, created))
}

/// Removes a profile and every binding pointing at it. Returns false when
/// the id is unknown (mirrors `ssh/knownHosts/remove` semantics).
pub fn delete_profile(store: &mut SudoProfileStore, id: &str) -> bool {
    let before = store.profiles.len();
    store.profiles.retain(|profile| profile.id != id);
    let removed = store.profiles.len() != before;
    if removed {
        store.bindings.retain(|_, profile_id| profile_id != id);
    }
    removed
}

/// Resolves an id-or-exact-name reference (MCP ergonomics).
pub fn find_by_ref<'a>(store: &'a SudoProfileStore, reference: &str) -> Option<&'a SudoProfile> {
    store
        .profiles
        .iter()
        .find(|profile| profile.id == reference)
        .or_else(|| {
            store
                .profiles
                .iter()
                .find(|profile| profile.name == reference)
        })
}

/// The profile bound to a connection, if any. Dangling bindings fall back to
/// "no binding" instead of breaking the connection.
pub fn bound_profile<'a>(
    store: &'a SudoProfileStore,
    connection_id: &str,
) -> Option<&'a SudoProfile> {
    let profile_id = store.bindings.get(connection_id)?;
    store
        .profiles
        .iter()
        .find(|profile| profile.id.as_str() == profile_id)
}

/// Persists a connection binding. `None` clears it; `Some` must reference an
/// existing profile.
pub fn set_binding(
    store: &mut SudoProfileStore,
    connection_id: &str,
    profile_id: Option<&str>,
) -> Result<(), String> {
    match profile_id {
        None => {
            store.bindings.remove(connection_id);
        }
        Some(profile_id) => {
            if !store.profiles.iter().any(|profile| profile.id == profile_id) {
                return Err(format!("Quick Sudo profile '{profile_id}' not found"));
            }
            store
                .bindings
                .insert(connection_id.to_string(), profile_id.to_string());
        }
    }
    Ok(())
}

/// Overlays a bound profile onto resolved connection auth: the profile owns
/// the whole credential source, so an empty profile password falls back to
/// the login password (never to the connection's own sudo secret). Field
/// assignment keeps the auth's OTP usage bookkeeping intact.
pub fn apply_profile(auth: &mut SudoAuth, profile: &SudoProfile, login_password: &str) {
    let profile_password = profile.sudo_password.trim();
    auth.password = if profile_password.is_empty() {
        login_password.to_string()
    } else {
        profile_password.to_string()
    };
    auth.totp_secrets = exec::parse_totp_secrets(&profile.totp_secret);
    auth.password_prompt_hint = exec::sanitize_prompt_hint(&profile.password_prompt_hint);
    auth.totp_prompt_hint = exec::sanitize_prompt_hint(&profile.totp_prompt_hint);
    auth.flow_mode = Some(AuthFlowMode::parse(&profile.auth_flow_mode));
}

/// A bound profile's PTY preference replaces the connection's.
pub fn effective_use_pty(connection_pty: bool, profile: Option<&SudoProfile>) -> bool {
    profile.map(|profile| profile.sudo_use_pty).unwrap_or(connection_pty)
}

/// Plain-text summary returned by the connection-form action
/// (`connection/action` with action `quick-sudo-profiles`): the global
/// profile list plus the connection's current binding. The host can only
/// render a message on the connection panel (no interactive plugin pages),
/// so this doubles as the discoverable entry point hinting where the full
/// management UI lives. Secrets are reported as set/not-set only.
pub fn action_summary(store: &SudoProfileStore, connection_id: Option<&str>) -> String {
    let mut lines = Vec::new();
    if store.profiles.is_empty() {
        lines.push("No global Quick Sudo profiles yet.".to_string());
        lines.push(
            "Create one in the SSH workbench: toolbar key button, or Settings > Sudo credential source > Manage profiles."
                .to_string(),
        );
    } else {
        lines.push(format!(
            "Global Quick Sudo profiles ({}):",
            store.profiles.len()
        ));
        let mut profiles = store.profiles.clone();
        profiles.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
        for profile in profiles {
            lines.push(format!(
                "- {}: password {}, TOTP {}, flow {}{}",
                profile.name,
                if profile.sudo_password.trim().is_empty() { "not set" } else { "set" },
                if profile.totp_secret.trim().is_empty() { "not set" } else { "set" },
                profile.auth_flow_mode,
                if profile.sudo_use_pty { ", PTY" } else { "" },
            ));
        }
    }
    if let Some(connection_id) = connection_id.filter(|id| !id.is_empty()) {
        match bound_profile(store, connection_id) {
            Some(profile) => lines.push(format!("Bound to this connection: {}", profile.name)),
            None => lines.push(
                "This connection uses its own sudo configuration (no global profile bound)."
                    .to_string(),
            ),
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-only secret assembled at runtime: obviously fake, and never a
    /// literal credential in source.
    fn test_secret(tag: &str) -> String {
        format!("{tag}-{}", uuid::Uuid::new_v4())
    }

    fn store_with(name: &str, password: &str) -> SudoProfileStore {
        let mut store = SudoProfileStore::default();
        let (profile, _) = save_profile(
            &mut store,
            &json!({ "name": name, "sudoPassword": password, "totpSecret": "JBSWY3DPEHPK3PXP" }),
        )
        .unwrap();
        store.bindings.insert("conn-1".to_string(), profile.id.clone());
        store
    }

    #[test]
    fn create_update_keeps_secret_when_blank() {
        let secret = test_secret("keep");
        let mut store = store_with("ops", &secret);
        let id = store.profiles[0].id.clone();
        let (updated, created) = save_profile(
            &mut store,
            &json!({ "id": id, "name": "ops", "sudoPassword": "", "sudoUsePty": true }),
        )
        .unwrap();
        assert!(!created);
        assert_eq!(updated.sudo_password, secret);
        assert!(updated.sudo_use_pty);
        // Rewriting the secret works too.
        let replacement = test_secret("new");
        let (changed, _) = save_profile(
            &mut store,
            &json!({ "id": id, "name": "ops", "sudoPassword": replacement }),
        )
        .unwrap();
        assert_eq!(changed.sudo_password, replacement);
    }

    #[test]
    fn explicit_clear_flags_wipe_secrets() {
        let secret = test_secret("clear");
        let mut store = store_with("ops", &secret);
        let id = store.profiles[0].id.clone();
        let (updated, _) = save_profile(
            &mut store,
            &json!({ "id": id, "name": "ops", "clearTotpSecret": true }),
        )
        .unwrap();
        assert!(updated.totp_secret.is_empty());
        assert_eq!(updated.sudo_password, secret);
    }

    #[test]
    fn name_rules_are_enforced() {
        let mut store = store_with("ops", "x");
        assert!(save_profile(&mut store, &json!({ "name": "  " })).is_err());
        assert!(save_profile(&mut store, &json!({ "name": "a".repeat(65) })).is_err());
        // Case-insensitive uniqueness, trimmed compare.
        let error = save_profile(&mut store, &json!({ "name": " OPS " })).unwrap_err();
        assert!(error.contains("already in use"), "{error}");
        // Updating the same profile keeps its own name.
        let id = store.profiles[0].id.clone();
        assert!(save_profile(&mut store, &json!({ "id": id, "name": "OPS" })).is_ok());
    }

    #[test]
    fn profile_cap_is_enforced() {
        let mut store = SudoProfileStore::default();
        for index in 0..MAX_PROFILES {
            save_profile(&mut store, &json!({ "name": format!("p{index}") })).unwrap();
        }
        assert!(save_profile(&mut store, &json!({ "name": "overflow" })).is_err());
    }

    #[test]
    fn delete_cascades_bindings() {
        let mut store = store_with("ops", "x");
        let id = store.profiles[0].id.clone();
        assert!(delete_profile(&mut store, &id));
        assert!(store.bindings.get("conn-1").is_none());
        assert!(!delete_profile(&mut store, &id));
    }

    #[test]
    fn corrupted_file_falls_back_to_empty_store() {
        let dir = std::env::temp_dir().join(format!("dbx-sudo-profiles-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(store_path(&dir), "{not json").unwrap();
        let store = load_store(&dir);
        assert!(store.profiles.is_empty());
        assert!(store.bindings.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_roundtrip_preserves_profiles_and_bindings() {
        let dir = std::env::temp_dir().join(format!("dbx-sudo-profiles-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = store_with("ops", "s3cret");
        save_store(&dir, &store).unwrap();
        let loaded = load_store(&dir);
        assert_eq!(loaded.profiles, store.profiles);
        assert_eq!(loaded.bindings, store.bindings);
        // Credentials land only in the 0600 store file, never in a view.
        assert!(save_store(&dir, &store).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn views_never_echo_secrets() {
        let secret = test_secret("view");
        let store = store_with("ops", &secret);
        let views = list_views(&store);
        let rendered = serde_json::to_string(&views).unwrap();
        assert!(!rendered.contains(&secret));
        assert!(!rendered.contains("sudoPassword\":"));
        assert!(rendered.contains("\"sudoPasswordSet\":true"));
        let view = profile_view(&store.profiles[0].clone());
        assert_eq!(view["totpConfigured"], true);
    }

    #[test]
    fn refs_resolve_by_id_then_name() {
        let store = store_with("ops", "x");
        let profile = &store.profiles[0];
        assert_eq!(find_by_ref(&store, &profile.id).unwrap().name, "ops");
        assert_eq!(find_by_ref(&store, "ops").unwrap().id, profile.id);
        assert!(find_by_ref(&store, "missing").is_none());
    }

    #[test]
    fn bindings_validate_and_resolve() {
        let mut store = store_with("ops", "x");
        assert!(set_binding(&mut store, "conn-2", Some("nope")).is_err());
        let profile_id = store.profiles[0].id.clone();
        set_binding(&mut store, "conn-2", Some(&profile_id)).unwrap();
        assert!(bound_profile(&store, "conn-2").is_some());
        set_binding(&mut store, "conn-2", None).unwrap();
        assert!(bound_profile(&store, "conn-2").is_none());
        // Dangling binding resolves to none instead of failing.
        store
            .bindings
            .insert("conn-3".to_string(), "ghost".to_string());
        assert!(bound_profile(&store, "conn-3").is_none());
    }

    #[test]
    fn apply_profile_overrides_source_and_falls_back_to_login() {
        let conn_secret = test_secret("conn");
        let store = store_with("ops", &test_secret("profile"));
        let profile = store.profiles[0].clone();
        let mut auth = SudoAuth::new(&conn_secret, "login-pass", "", Default::default());
        apply_profile(&mut auth, &profile, "login-pass");
        assert_eq!(auth.password, profile.sudo_password);
        assert_ne!(auth.password, conn_secret);
        assert!(!auth.totp_secrets.is_empty());

        let mut blank = profile.clone();
        blank.sudo_password = String::new();
        let mut auth = SudoAuth::new(&conn_secret, "login-pass", "", Default::default());
        apply_profile(&mut auth, &blank, "login-pass");
        // A bound profile without a password falls back to the login
        // password, not to the connection's own sudo secret.
        assert_eq!(auth.password, "login-pass");
    }

    #[test]
    fn use_pty_follows_bound_profile() {
        let mut store = store_with("ops", "x");
        let id = store.profiles[0].id.clone();
        let (_, _) = save_profile(
            &mut store,
            &json!({ "id": id, "name": "ops", "sudoUsePty": true }),
        )
        .unwrap();
        let profile = &store.profiles[0];
        assert!(effective_use_pty(false, Some(profile)));
        assert!(effective_use_pty(true, None));
        assert!(!effective_use_pty(false, None));
    }

    #[test]
    fn action_summary_lists_profiles_and_binding() {
        let secret = test_secret("sum");
        let store = store_with("ops", &secret);
        let summary = action_summary(&store, Some("conn-1"));
        assert!(summary.contains("Global Quick Sudo profiles (1)"), "{summary}");
        assert!(summary.contains("- ops: password set, TOTP set, flow password_then_otp"), "{summary}");
        assert!(summary.contains("Bound to this connection: ops"), "{summary}");
        assert!(!summary.contains(&secret), "summary leaked a secret");

        let unbound = action_summary(&store, Some("other-conn"));
        assert!(unbound.contains("uses its own sudo configuration"), "{unbound}");
        let no_id = action_summary(&store, None);
        assert!(!no_id.contains("Bound to this connection"), "{no_id}");

        let empty = action_summary(&SudoProfileStore::default(), None);
        assert!(empty.contains("No global Quick Sudo profiles yet"), "{empty}");
    }
}
