use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const TERMINAL_REPLAY_LIMIT: usize = 2 * 1024 * 1024;
pub const TRANSFER_CHUNK_SIZE: usize = 256 * 1024;
pub const MAX_TRANSFER_SIZE: u64 = 16 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthenticationMethod {
    Password,
    PrivateKey,
    PrivateKeyPassword,
    Agent,
    None,
}

impl AuthenticationMethod {
    fn from_connection(connection: &serde_json::Map<String, Value>) -> Result<Self, String> {
        let value = connection
            .get("external_config")
            .and_then(Value::as_object)
            .and_then(|config| config.get("authentication"))
            .and_then(Value::as_str)
            .unwrap_or("password");
        match value {
            "password" | "private-key" | "private-key-password" | "agent" | "none" => {
                Ok(Self::from_method_name(value))
            }
            _ => Err(format!("Unsupported SSH authentication method '{value}'")),
        }
    }

    fn from_method_name(value: &str) -> Self {
        match value {
            "private-key" => Self::PrivateKey,
            "private-key-password" => Self::PrivateKeyPassword,
            "agent" => Self::Agent,
            "none" => Self::None,
            _ => Self::Password,
        }
    }

    /// Canonical method name (the `external_config.authentication` spelling)
    /// for display in read-only payloads such as `ssh/sessions/list`.
    /// Deliberately excludes any credential material — names only.
    pub fn method_name(&self) -> &'static str {
        match self {
            Self::PrivateKey => "private-key",
            Self::PrivateKeyPassword => "private-key-password",
            Self::Agent => "agent",
            Self::None => "none",
            Self::Password => "password",
        }
    }
}

/// Where sudo credentials come from (connection form `sudo_source`): the
/// connection's own values ("custom"), a global Quick Sudo profile
/// ("global", resolved from `sudo_profile_ref` or the workbench binding),
/// or disabled ("off"). Legacy 0.4.x connections without the field map from
/// the old `quick_sudo` boolean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SudoSource {
    Off,
    Custom,
    Global,
}

impl SudoSource {
    /// `sudo_source` wins whenever it is present and recognized; absent,
    /// empty, or unknown values fall back to the legacy `quick_sudo` flag so
    /// existing connections keep their behavior.
    pub fn parse(value: Option<&str>, legacy_quick_sudo: bool) -> Self {
        match value.map(str::trim) {
            Some("off") => Self::Off,
            Some("global") => Self::Global,
            Some("custom") => Self::Custom,
            _ => {
                if legacy_quick_sudo {
                    Self::Custom
                } else {
                    Self::Off
                }
            }
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Custom => "custom",
            Self::Global => "global",
        }
    }
}

#[derive(Debug, Clone)]
pub struct StoredConnection {
    pub id: String,
    pub host: String,
    pub port: u16,
    pub runtime_host: String,
    pub runtime_port: u16,
    pub username: String,
    pub password: String,
    pub authentication: AuthenticationMethod,
    pub private_key_path: String,
    pub private_key_passphrase: String,
    #[cfg_attr(windows, allow(dead_code))]
    pub agent_socket: String,
    pub connect_timeout_secs: u64,
    pub keepalive_interval_secs: u64,
    pub read_only: bool,
    /// Quick Sudo orchestration: sudo password override, TOTP secret, and
    /// prompt hints. Secrets come from `connection_secrets`, tuning from
    /// `external_config`. `sudo_source` selects the credential source and
    /// `sudo_profile_ref` names the global profile when the source is global.
    pub sudo_password: String,
    pub totp_secret: String,
    pub sudo_source: SudoSource,
    pub sudo_profile_ref: String,
    pub sudo_use_pty: bool,
    /// sudoers-style per-connection sudo command allowlist
    /// (`external_config.sudo_whitelist`); empty = gate off.
    pub sudo_whitelist: Vec<String>,
    pub password_prompt_hint: String,
    pub totp_prompt_hint: String,
    pub auth_flow_mode: String,
    /// ProxyJump chain: each entry is dialed before the target, the final hop
    /// reaching `host:port` directly (the runtime tunnel endpoint is skipped).
    pub jump_hosts: Vec<JumpHost>,
}

/// One hop of a `external_config.jump_hosts` chain. Credentials live inline
/// because the DBX connection model only provides a single secrets store for
/// the target host.
#[derive(Debug, Clone, Default)]
pub struct JumpHost {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub authentication: String,
    pub private_key_path: String,
    pub private_key_passphrase: String,
    pub agent_socket: String,
    pub totp_secret: String,
    pub password_prompt_hint: String,
    pub totp_prompt_hint: String,
    pub auth_flow_mode: String,
}

impl JumpHost {
    /// Parses one jump entry from a JSON object (also used by the MCP
    /// `jumpHosts` parameter). Field names are snake_case, matching the
    /// `external_config.jump_hosts` form field.
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let Some(config) = value.as_object() else {
            return Err("Jump host must be a JSON object".to_string());
        };
        Self::from_config(config)
    }

    fn from_config(value: &serde_json::Map<String, Value>) -> Result<Self, String> {
        let host = validate_host_field(string_field(value, "host")?)?;
        let port = match value.get("port") {
            None => 22,
            Some(port) => port
                .as_u64()
                .and_then(|value| u16::try_from(value).ok())
                .filter(|value| *value > 0)
                .ok_or("Jump host port must be between 1 and 65535")?,
        };
        let authentication = value
            .get("authentication")
            .and_then(Value::as_str)
            .unwrap_or("password")
            .to_string();
        if !matches!(
            authentication.as_str(),
            "password" | "private-key" | "private-key-password" | "agent"
        ) {
            return Err(format!(
                "Unsupported jump host authentication method '{authentication}'"
            ));
        }
        Ok(Self {
            host,
            port,
            username: string_field(value, "username")?,
            // Credentials must survive verbatim: trimming would silently
            // change the password/passphrase the server sees.
            password: credential_string(value, "password"),
            authentication,
            private_key_path: optional_string(Some(value), "private_key_path"),
            private_key_passphrase: credential_string(value, "private_key_passphrase"),
            agent_socket: optional_string(Some(value), "agent_socket"),
            totp_secret: optional_string(Some(value), "totp_secret"),
            password_prompt_hint: optional_string(Some(value), "password_prompt_hint"),
            totp_prompt_hint: optional_string(Some(value), "totp_prompt_hint"),
            auth_flow_mode: optional_string(Some(value), "auth_flow_mode"),
        })
    }

    pub fn validate(&self, position: usize) -> Result<(), String> {
        match self.authentication.as_str() {
            "password" if self.password.is_empty() => Err(format!(
                "Jump host #{} ({}) requires a password",
                position + 1,
                self.host
            )),
            "private-key" | "private-key-password" if self.private_key_path.is_empty() => Err(
                format!(
                    "Jump host #{} ({}) requires a private key path",
                    position + 1,
                    self.host
                ),
            ),
            _ => Ok(()),
        }
    }

    /// Synthesizes a StoredConnection so the shared connect/auth pipeline
    /// (including keyboard-interactive 2FA) applies to jump hops as well.
    pub fn to_connection(&self, id: &str, timeout_secs: u64, keepalive_secs: u64) -> StoredConnection {
        StoredConnection {
            id: id.to_string(),
            host: self.host.clone(),
            port: self.port,
            runtime_host: self.host.clone(),
            runtime_port: self.port,
            username: self.username.clone(),
            password: self.password.clone(),
            authentication: AuthenticationMethod::from_method_name(&self.authentication),
            private_key_path: self.private_key_path.clone(),
            private_key_passphrase: self.private_key_passphrase.clone(),
            agent_socket: self.agent_socket.clone(),
            connect_timeout_secs: timeout_secs.max(1),
            keepalive_interval_secs: keepalive_secs,
            read_only: false,
            sudo_password: String::new(),
            totp_secret: self.totp_secret.clone(),
            sudo_source: SudoSource::Custom,
            sudo_profile_ref: String::new(),
            sudo_use_pty: false,
            sudo_whitelist: Vec::new(),
            password_prompt_hint: self.password_prompt_hint.clone(),
            totp_prompt_hint: self.totp_prompt_hint.clone(),
            auth_flow_mode: self.auth_flow_mode.clone(),
            jump_hosts: Vec::new(),
        }
    }
}

impl StoredConnection {
    /// Quick Sudo is active unless the source is explicitly off; read-only
    /// connections gate it separately (see `ssh.rs`).
    pub fn sudo_enabled(&self) -> bool {
        self.sudo_source != SudoSource::Off
    }

    pub fn from_lifecycle_params(params: &Value) -> Result<Self, String> {
        let connection = params
            .get("connection")
            .and_then(Value::as_object)
            .ok_or("Missing connection payload")?;
        let id = string_field(connection, "id")?;
        let host = validate_host_field(string_field(connection, "host")?)?;
        let port = connection
            .get("port")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .filter(|value| *value > 0)
            .ok_or("SSH port must be between 1 and 65535")?;
        let runtime = params.get("runtime").and_then(Value::as_object);
        let runtime_host = optional_string(runtime, "host");
        let runtime_port = runtime
            .and_then(|value| value.get("port"))
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .filter(|value| *value > 0)
            .unwrap_or(port);
        let username = string_field(connection, "username")?;
        let password = connection
            .get("password")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let authentication = AuthenticationMethod::from_connection(connection)?;
        let external_config = connection.get("external_config").and_then(Value::as_object);
        let connection_secrets = connection
            .get("connection_secrets")
            .and_then(Value::as_object);
        let private_key_path = optional_string(external_config, "private_key_path");
        let private_key_passphrase = connection_secrets
            .map(|secrets| credential_string(secrets, "private_key_passphrase"))
            .unwrap_or_default();
        let agent_socket = optional_string(external_config, "agent_socket");
        // sudo_password 是凭据：原样读取（首尾空格合法），空白语义由
        // SudoAuth::new 的「空白即回退登录密码」兜底，不在解析层改写。
        let sudo_password = connection_secrets
            .map(|secrets| credential_string(secrets, "sudo_password"))
            .unwrap_or_default();
        let totp_secret = optional_string(connection_secrets, "totp_secret");
        let password_prompt_hint = optional_string(external_config, "password_prompt_hint");
        let totp_prompt_hint = optional_string(external_config, "totp_prompt_hint");
        let auth_flow_mode = optional_string(external_config, "auth_flow_mode");
        // Legacy 0.4.x flag: only consulted when `sudo_source` is absent, so
        // a re-saved connection (stale `quick_sudo` left behind) follows the
        // explicit source chosen on the form.
        let legacy_quick_sudo = external_config
            .and_then(|config| config.get("quick_sudo"))
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let sudo_source = SudoSource::parse(
            external_config
                .and_then(|config| config.get("sudo_source"))
                .and_then(Value::as_str),
            legacy_quick_sudo,
        );
        let sudo_profile_ref = optional_string(external_config, "sudo_profile");
        let sudo_use_pty = external_config
            .and_then(|config| config.get("sudo_use_pty"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let sudo_whitelist = optional_string(external_config, "sudo_whitelist")
            .split('\n')
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(str::to_string)
            .collect();
        let jump_hosts = parse_jump_hosts(external_config)?;
        let runtime_host = if runtime_host.is_empty() {
            host.clone()
        } else {
            runtime_host
        };
        if matches!(
            authentication,
            AuthenticationMethod::Password | AuthenticationMethod::PrivateKeyPassword
        ) && password.is_empty()
        {
            return Err("Password authentication requires a password".to_string());
        }
        if matches!(
            authentication,
            AuthenticationMethod::PrivateKey | AuthenticationMethod::PrivateKeyPassword
        ) && private_key_path.is_empty()
        {
            return Err("Private-key authentication requires a private key path".to_string());
        }
        Ok(Self {
            id,
            host,
            port,
            runtime_host,
            runtime_port,
            username,
            password,
            authentication,
            private_key_path,
            private_key_passphrase,
            agent_socket,
            connect_timeout_secs: connection
                .get("connect_timeout_secs")
                .and_then(Value::as_u64)
                .unwrap_or(15)
                .max(1),
            keepalive_interval_secs: connection
                .get("keepalive_interval_secs")
                .and_then(Value::as_u64)
                .unwrap_or(30),
            // 只读门禁收敛：连接表单 read_only（插件特定配置项）∥ 宿主标准
            // read_only（ConnectionConfig 通用设置）。
            read_only: external_config
                .and_then(|config| config.get("read_only"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || connection
                    .get("read_only")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            sudo_password,
            totp_secret,
            sudo_source,
            sudo_profile_ref,
            sudo_use_pty,
            sudo_whitelist,
            password_prompt_hint,
            totp_prompt_hint,
            auth_flow_mode,
            jump_hosts,
        })
    }
}

fn parse_jump_hosts(
    external_config: Option<&serde_json::Map<String, Value>>,
) -> Result<Vec<JumpHost>, String> {
    let Some(list) = external_config
        .and_then(|config| config.get("jump_hosts"))
        .and_then(Value::as_array)
    else {
        return Ok(Vec::new());
    };
    if list.len() > 3 {
        return Err("At most 3 jump hosts are supported".to_string());
    }
    let mut hosts: Vec<JumpHost> = Vec::with_capacity(list.len());
    for (position, value) in list.iter().enumerate() {
        let Some(config) = value.as_object() else {
            return Err(format!("Jump host #{} must be an object", position + 1));
        };
        let host = JumpHost::from_config(config)?;
        host.validate(position)?;
        // A repeated hop can only be a misconfiguration: the same
        // host:port dialled twice never advances the chain.
        if hosts
            .iter()
            .any(|existing| existing.host == host.host && existing.port == host.port)
        {
            return Err(format!(
                "Jump host #{} ({}) repeats an earlier hop",
                position + 1,
                host.host
            ));
        }
        hosts.push(host);
    }
    Ok(hosts)
}

/// Host fields must be a dialable hostname/IP: no internal whitespace and no
/// URI scheme syntax (`ssh://…`) that would silently become a bogus TCP
/// target instead of failing fast at parse time.
fn validate_host_field(host: String) -> Result<String, String> {
    if host.chars().any(char::is_whitespace) {
        return Err(format!("Invalid SSH host '{host}': must not contain whitespace"));
    }
    if host.contains("://") {
        return Err(format!(
            "Invalid SSH host '{host}': use a bare hostname, not a URI"
        ));
    }
    Ok(host)
}

/// Reads a credential field verbatim (no trimming): passwords and
/// passphrases may legitimately start or end with spaces.
fn credential_string(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> String {
    object
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn optional_string(object: Option<&serde_json::Map<String, Value>>, key: &str) -> String {
    object
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn string_field(object: &serde_json::Map<String, Value>, key: &str) -> Result<String, String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("Missing SSH {key}"))
}

#[derive(Debug, Clone)]
pub struct TerminalFrame {
    pub sequence: u64,
    pub stream: TerminalStream,
    pub data: Vec<u8>,
}

impl TerminalFrame {
    pub fn encode(&self) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(9 + self.data.len());
        encoded.push(self.stream as u8);
        encoded.extend_from_slice(&self.sequence.to_be_bytes());
        encoded.extend_from_slice(&self.data);
        encoded
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TerminalStream {
    Stdout = 0,
    Stderr = 1,
    State = 2,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpEntry {
    pub name: String,
    pub uri: String,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionOpenRequest {
    pub connection_id: String,
    #[serde(default)]
    pub workbench_id: String,
    #[serde(default = "default_cols")]
    pub cols: u32,
    #[serde(default = "default_rows")]
    pub rows: u32,
}

fn default_cols() -> u32 {
    120
}

fn default_rows() -> u32 {
    32
}

pub fn sftp_uri(path: &str) -> String {
    format!(
        "sftp:{}",
        if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        }
    )
}

pub fn path_from_sftp_uri(uri: &str) -> Result<String, String> {
    let path = uri
        .strip_prefix("sftp:")
        .ok_or("SFTP URI must use the sftp: scheme")?;
    normalize_remote_path(path)
}

pub fn normalize_remote_path(path: &str) -> Result<String, String> {
    if path.is_empty() || path.contains('\0') {
        return Err("SFTP path is empty or invalid".to_string());
    }
    let mut components = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            value => components.push(value),
        }
    }
    Ok(format!("/{}", components.join("/")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_remote_paths_without_escaping_root() {
        assert_eq!(
            normalize_remote_path("/home/user/../file").unwrap(),
            "/home/file"
        );
        assert_eq!(normalize_remote_path("../../etc").unwrap(), "/etc");
    }

    #[test]
    fn terminal_frame_carries_stream_and_sequence() {
        let encoded = TerminalFrame {
            sequence: 42,
            stream: TerminalStream::Stderr,
            data: b"x".to_vec(),
        }
        .encode();
        assert_eq!(encoded[0], 1);
        assert_eq!(u64::from_be_bytes(encoded[1..9].try_into().unwrap()), 42);
        assert_eq!(&encoded[9..], b"x");
    }

    #[test]
    fn legacy_password_connections_remain_compatible() {
        let connection = StoredConnection::from_lifecycle_params(&serde_json::json!({
            "connection": {
                "id": "legacy",
                "host": "example.com",
                "port": 22,
                "username": "user",
                "password": "secret"
            }
        }))
        .unwrap();

        assert_eq!(connection.authentication, AuthenticationMethod::Password);
        assert_eq!(connection.password, "secret");
    }

    #[test]
    fn read_only_flags_from_host_and_form_force_read_only() {
        // 插件特定配置项：连接表单 read_only（external_config）→ 只读门禁。
        let form_read_only = StoredConnection::from_lifecycle_params(&serde_json::json!({
            "connection": {
                "id": "form-ro",
                "host": "example.com",
                "port": 22,
                "username": "user",
                "password": "secret",
                "external_config": { "read_only": true }
            }
        }))
        .unwrap();
        assert!(form_read_only.read_only, "form read_only must force the gate");

        // 宿主标准 read_only（ConnectionConfig.read_only，通用连接设置）同样生效。
        let host_read_only = StoredConnection::from_lifecycle_params(&serde_json::json!({
            "connection": {
                "id": "ro",
                "host": "example.com",
                "port": 22,
                "username": "user",
                "password": "secret",
                "read_only": true
            }
        }))
        .unwrap();
        assert!(host_read_only.read_only, "host read_only must force the gate");

        let writable = StoredConnection::from_lifecycle_params(&serde_json::json!({
            "connection": {
                "id": "rw",
                "host": "example.com",
                "port": 22,
                "username": "user",
                "password": "secret"
            }
        }))
        .unwrap();
        assert!(!writable.read_only, "writable connections must stay writable");
    }

    #[test]
    fn manifest_connection_fields_stay_in_sync_with_parsing() {
        // 契约：manifest.json 的 connection-provider 字段与 from_lifecycle_params
        // 的解析覆盖互为镜像——manifest 加字段而解析不消费（或反向）都会漂移，
        // 这里直读 manifest 逐项对账。
        let manifest: Value = serde_json::from_str(include_str!("../../manifest.json")).unwrap();
        let provider = manifest["contributions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["type"] == "connection-provider")
            .expect("connection-provider contribution");
        let fields = provider["fields"].as_array().unwrap();

        let keys: Vec<&str> = fields
            .iter()
            .filter_map(|field| field["key"].as_str())
            .collect();
        let expected = [
            "display_name",
            "host",
            "port",
            "username",
            "authentication",
            "password",
            "private_key_path",
            "private_key_passphrase",
            "agent_socket",
            "connect_timeout_secs",
            "keepalive_interval_secs",
            "sudo_source",
            "sudo_profile",
            "sudo_password",
            "totp_secret",
            "auth_flow_mode",
            "password_prompt_hint",
            "totp_prompt_hint",
            "sudo_use_pty",
            "sudo_whitelist",
            "read_only",
        ];
        assert_eq!(keys, expected, "manifest field list drifted from parsing");

        // secret binding 只允许落在凭据字段；config binding 不得承载凭据语义。
        let secret_keys = ["password", "private_key_passphrase", "sudo_password", "totp_secret"];
        for field in fields {
            let key = field["key"].as_str().unwrap();
            match field["binding"].as_str() {
                Some("secret") => assert!(
                    secret_keys.contains(&key),
                    "unexpected secret binding: {key}"
                ),
                Some("config") => assert!(
                    !key.contains("password") || key == "password_prompt_hint",
                    "config binding must not carry credential material: {key}"
                ),
                _ => {}
            }
        }

        // required_when 链必须与 from_lifecycle_params 的凭据校验一致
        // （model.rs: password/private-key-password 校验密码、private-key* 校验私钥路径）。
        let one_of = |key: &str, constraint: &str| -> Vec<String> {
            fields
                .iter()
                .find(|field| field["key"] == key)
                .unwrap()[constraint]
                .as_object()
                .map(|gate| {
                    gate["one_of"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|value| value.as_str().unwrap().to_string())
                        .collect()
                })
                .unwrap_or_default()
        };
        assert_eq!(one_of("password", "required_when"), ["password", "private-key-password"]);
        assert_eq!(
            one_of("private_key_path", "required_when"),
            ["private-key", "private-key-password"]
        );

        // sudo 覆盖簇跟随表单选定的凭据来源（sudo_source）：自定义模式下才
        // 出现本连接密码/PTY；global 模式出现全局配置引用；2FA 编排字段
        // （totp_secret/auth_flow_mode/hints）服务登录期 keyboard-interactive，
        // global 模式下整体由全局配置接管故隐藏，off/custom 模式仍常显。
        let visible_when = |key: &str| -> Option<(String, Vec<String>)> {
            fields
                .iter()
                .find(|field| field["key"] == key)
                .unwrap()
                .get("visible_when")
                .map(|gate| {
                    (
                        gate["field"].as_str().unwrap().to_string(),
                        gate["one_of"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|value| value.as_str().unwrap().to_string())
                            .collect(),
                    )
                })
        };
        assert_eq!(
            visible_when("sudo_password"),
            Some(("sudo_source".to_string(), vec!["custom".to_string()])),
            "sudo_password must stay gated on sudo_source=custom"
        );
        assert_eq!(
            visible_when("sudo_use_pty"),
            Some(("sudo_source".to_string(), vec!["custom".to_string()])),
            "sudo_use_pty must stay gated on sudo_source=custom"
        );
        assert_eq!(
            visible_when("sudo_profile"),
            Some(("sudo_source".to_string(), vec!["global".to_string()])),
            "sudo_profile must show only for sudo_source=global"
        );
        for key in ["totp_secret", "auth_flow_mode", "password_prompt_hint", "totp_prompt_hint"] {
            assert_eq!(
                visible_when(key),
                Some(("sudo_source".to_string(), vec!["custom".to_string(), "off".to_string()])),
                "{key} must hide under sudo_source=global (the bound profile owns the whole credential source) and stay visible otherwise"
            );
        }
    }

    #[test]
    fn auth_method_names_round_trip_for_display() {
        for name in ["password", "private-key", "private-key-password", "agent", "none"] {
            // Name round-trip is a pure enum mapping; credential validation is
            // exercised separately by the connection parsing tests.
            let method = AuthenticationMethod::from_method_name(name);
            assert_eq!(method.method_name(), name);
        }
    }

    #[test]
    fn sudo_password_is_a_credential_and_keeps_edge_whitespace() {
        // 第 3 轮对抗审查遗留项：跳板/私钥口令已原样读取，sudo_password 同样
        // 不在解析层 trim（首尾空格是合法密码字符）；纯空白语义由
        // SudoAuth::new 的「空白即回退」兜底，不受影响。
        let connection = StoredConnection::from_lifecycle_params(&serde_json::json!({
            "connection": {
                "id": "sudo-ws",
                "host": "example.com",
                "port": 22,
                "username": "user",
                "password": "login",
                "connection_secrets": { "sudo_password": " padded " }
            }
        }))
        .unwrap();
        assert_eq!(connection.sudo_password, " padded ");

        let blank = StoredConnection::from_lifecycle_params(&serde_json::json!({
            "connection": {
                "id": "sudo-blank",
                "host": "example.com",
                "port": 22,
                "username": "user",
                "password": "login",
                "connection_secrets": { "sudo_password": "   " }
            }
        }))
        .unwrap();
        assert_eq!(blank.sudo_password, "   ");
    }

    #[test]
    fn sudo_source_maps_three_modes_with_legacy_fallback() {
        let parse = |external_config: serde_json::Value| {
            StoredConnection::from_lifecycle_params(&serde_json::json!({
                "connection": {
                    "id": "sudo-source",
                    "host": "example.com",
                    "port": 22,
                    "username": "user",
                    "password": "login",
                    "external_config": external_config
                }
            }))
            .unwrap()
        };

        // 表单三选一：off / custom / global（global 附带全局配置引用，trim 后生效）。
        assert_eq!(
            parse(serde_json::json!({ "sudo_source": "off" })).sudo_source,
            SudoSource::Off
        );
        assert_eq!(
            parse(serde_json::json!({ "sudo_source": "custom" })).sudo_source,
            SudoSource::Custom
        );
        let global = parse(serde_json::json!({
            "sudo_source": "global",
            "sudo_profile": " ops "
        }));
        assert_eq!(global.sudo_source, SudoSource::Global);
        assert_eq!(global.sudo_profile_ref, "ops");
        assert!(global.sudo_enabled());
        assert!(!parse(serde_json::json!({ "sudo_source": "off" })).sudo_enabled());

        // 旧连接无 sudo_source：由 quick_sudo 布尔映射，保持原行为。
        assert_eq!(
            parse(serde_json::json!({ "quick_sudo": true })).sudo_source,
            SudoSource::Custom
        );
        assert_eq!(
            parse(serde_json::json!({ "quick_sudo": false })).sudo_source,
            SudoSource::Off
        );
        assert_eq!(parse(serde_json::json!({})).sudo_source, SudoSource::Custom);

        // 显式 sudo_source 优先于遗留 quick_sudo（新表单保存后旧键残留）。
        assert_eq!(
            parse(serde_json::json!({ "sudo_source": "off", "quick_sudo": true })).sudo_source,
            SudoSource::Off
        );
        assert_eq!(
            parse(serde_json::json!({ "sudo_source": "custom", "quick_sudo": false })).sudo_source,
            SudoSource::Custom
        );

        // 空值/未知值回落 legacy。
        assert_eq!(
            parse(serde_json::json!({ "sudo_source": "", "quick_sudo": false })).sudo_source,
            SudoSource::Off
        );
        assert_eq!(
            parse(serde_json::json!({ "sudo_source": "bogus", "quick_sudo": true })).sudo_source,
            SudoSource::Custom
        );
    }

    #[test]
    fn parses_private_key_password_credentials_from_separate_stores() {
        let connection = StoredConnection::from_lifecycle_params(&serde_json::json!({
            "connection": {
                "id": "key-password",
                "host": "example.com",
                "port": 22,
                "username": "user",
                "password": "fallback",
                "external_config": {
                    "authentication": "private-key-password",
                    "private_key_path": "C:/keys/id_ed25519"
                },
                "connection_secrets": {
                    "private_key_passphrase": "key-secret"
                }
            }
        }))
        .unwrap();

        assert_eq!(
            connection.authentication,
            AuthenticationMethod::PrivateKeyPassword
        );
        assert_eq!(connection.private_key_path, "C:/keys/id_ed25519");
        assert_eq!(connection.private_key_passphrase, "key-secret");
    }

    #[test]
    fn uses_the_host_runtime_endpoint_without_changing_host_key_identity() {
        let connection = StoredConnection::from_lifecycle_params(&serde_json::json!({
            "connection": {
                "id": "tunneled",
                "host": "target.internal",
                "port": 22,
                "username": "user",
                "password": "secret"
            },
            "runtime": {
                "host": "127.0.0.1",
                "port": 39122
            }
        }))
        .unwrap();

        assert_eq!(connection.host, "target.internal");
        assert_eq!(connection.port, 22);
        assert_eq!(connection.runtime_host, "127.0.0.1");
        assert_eq!(connection.runtime_port, 39122);
    }

    #[test]
    fn parses_jump_host_chains() {
        let connection = StoredConnection::from_lifecycle_params(&serde_json::json!({
            "connection": {
                "id": "jumped",
                "host": "target.internal",
                "port": 22,
                "username": "user",
                "password": "secret",
                "external_config": {
                    "jump_hosts": [
                        { "host": "bastion.example.com", "port": 2202, "username": "ops", "password": "jump-pw" },
                        { "host": "inner.example.com", "authentication": "private-key", "username": "relay", "private_key_path": "~/.ssh/id_ed25519" }
                    ]
                }
            }
        }))
        .unwrap();

        assert_eq!(connection.jump_hosts.len(), 2);
        assert_eq!(connection.jump_hosts[0].port, 2202);
        assert_eq!(connection.jump_hosts[0].password, "jump-pw");
        assert_eq!(connection.jump_hosts[1].authentication, "private-key");
        let synthesized = connection.jump_hosts[1].to_connection("jump-2", 20, 30);
        assert_eq!(synthesized.host, "inner.example.com");
        assert_eq!(synthesized.runtime_port, 22);
        assert_eq!(synthesized.authentication, AuthenticationMethod::PrivateKey);
    }

    #[test]
    fn rejects_invalid_jump_host_chains() {
        let with_missing_password = serde_json::json!({
            "connection": {
                "id": "x", "host": "t", "port": 22, "username": "u", "password": "p",
                "external_config": { "jump_hosts": [ { "host": "bastion", "port": 22, "username": "ops" } ] }
            }
        });
        assert!(StoredConnection::from_lifecycle_params(&with_missing_password).is_err());

        let too_many = serde_json::json!({
            "connection": {
                "id": "x", "host": "t", "port": 22, "username": "u", "password": "p",
                "external_config": {
                    "jump_hosts": [
                        { "host": "a", "port": 22, "username": "u", "password": "p" },
                        { "host": "b", "port": 22, "username": "u", "password": "p" },
                        { "host": "c", "port": 22, "username": "u", "password": "p" },
                        { "host": "d", "port": 22, "username": "u", "password": "p" }
                    ]
                }
            }
        });
        assert!(StoredConnection::from_lifecycle_params(&too_many).is_err());
    }

    #[test]
    fn rejects_adversarial_jump_host_fields() {
        let build = |jump: Value| {
            StoredConnection::from_lifecycle_params(&serde_json::json!({
                "connection": {
                    "id": "x", "host": "t", "port": 22, "username": "u", "password": "p",
                    "external_config": { "jump_hosts": [jump] }
                }
            }))
        };
        // Port boundaries.
        for port in [0_u64, 65536, 70000] {
            assert!(
                build(serde_json::json!({ "host": "bastion", "port": port, "username": "u", "password": "p" }))
                    .is_err(),
                "port {port} must be rejected"
            );
        }
        // Empty/whitespace username.
        assert!(build(serde_json::json!({ "host": "bastion", "username": "  " })).is_err());
        // Host with internal whitespace or URI syntax must fail fast instead
        // of becoming a bogus TCP target.
        assert!(build(serde_json::json!({ "host": "my bastion", "username": "u", "password": "p" })).is_err());
        assert!(build(serde_json::json!({ "host": "ssh://bastion:22", "username": "u", "password": "p" })).is_err());
        // The target host field is held to the same rules.
        let bad_target = serde_json::json!({
            "connection": {
                "id": "x", "host": "tar get", "port": 22, "username": "u", "password": "p"
            }
        });
        assert!(StoredConnection::from_lifecycle_params(&bad_target).is_err());
    }

    #[test]
    fn rejects_repeated_jump_hops() {
        let repeated = serde_json::json!({
            "connection": {
                "id": "x", "host": "t", "port": 22, "username": "u", "password": "p",
                "external_config": {
                    "jump_hosts": [
                        { "host": "bastion", "port": 2202, "username": "u", "password": "p" },
                        { "host": "bastion", "port": 2202, "username": "u", "password": "p" }
                    ]
                }
            }
        });
        let error = StoredConnection::from_lifecycle_params(&repeated).unwrap_err();
        assert!(error.contains("repeats an earlier hop"), "{error}");
        // Same host on a different port is still a valid chain.
        let distinct = serde_json::json!({
            "connection": {
                "id": "x", "host": "t", "port": 22, "username": "u", "password": "p",
                "external_config": {
                    "jump_hosts": [
                        { "host": "bastion", "port": 2202, "username": "u", "password": "p" },
                        { "host": "bastion", "port": 2203, "username": "u", "password": "p" }
                    ]
                }
            }
        });
        assert!(StoredConnection::from_lifecycle_params(&distinct).is_ok());
    }

    #[test]
    fn jump_credentials_survive_special_characters_verbatim() {
        let connection = StoredConnection::from_lifecycle_params(&serde_json::json!({
            "connection": {
                "id": "x", "host": "t", "port": 22, "username": "u", "password": "p",
                "external_config": {
                    "jump_hosts": [
                        { "host": "bastion", "username": "u",
                          "password": "  p@$$w0rd 'with' \"quotes\" $backslash\\ ",
                          "authentication": "private-key",
                          "private_key_path": "~/.ssh/id_ed25519",
                          "private_key_passphrase": "  phrase with spaces  " }
                    ]
                }
            }
        }))
        .unwrap();
        assert_eq!(
            connection.jump_hosts[0].password,
            "  p@$$w0rd 'with' \"quotes\" $backslash\\ "
        );
        assert_eq!(
            connection.jump_hosts[0].private_key_passphrase,
            "  phrase with spaces  "
        );
    }
}

/// Contract tests binding `../manifest.json` to `from_lifecycle_params`: the
/// fields the host renders must be exactly the fields the parser consumes,
/// with matching required chains, visibility pairing, and defaults.
#[cfg(test)]
mod manifest_contract_tests {
    use super::*;
    use std::path::PathBuf;

    fn manifest() -> Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../manifest.json");
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read manifest at {}: {error}", path.display()));
        serde_json::from_str(&raw).expect("manifest.json must be valid JSON")
    }

    fn connection_fields() -> Vec<Value> {
        manifest()["contributions"]
            .as_array()
            .expect("contributions array")
            .iter()
            .find(|entry| entry["type"] == "connection-provider")
            .expect("connection-provider contribution")["fields"]
            .as_array()
            .expect("fields array")
            .clone()
    }

    fn field(key: &str) -> Value {
        connection_fields()
            .into_iter()
            .find(|entry| entry["key"].as_str() == Some(key))
            .unwrap_or_else(|| panic!("manifest field '{key}' missing"))
    }

    fn condition_one_of(entry: &Value, condition: &str) -> Option<Vec<String>> {
        entry[condition]["one_of"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .map(|value| value.as_str().unwrap_or_default().to_string())
                    .collect()
            })
    }

    fn condition_field<'a>(entry: &'a Value, condition: &str) -> Option<&'a str> {
        entry[condition]["field"].as_str()
    }

    /// Manifest fields required per `authentication` option must match the
    /// credentials `from_lifecycle_params` actually rejects when missing.
    #[test]
    fn required_chain_matches_parser() {
        // Model-required credentials per authentication option, mirroring the
        // parse rejection branches in from_lifecycle_params. Identity fields
        // (id/host/port/username) are always required on both sides.
        let model_required: &[(&str, &[&str])] = &[
            ("password", &["password"]),
            ("private-key", &["private_key_path"]),
            ("private-key-password", &["password", "private_key_path"]),
            ("agent", &[]),
            ("none", &[]),
        ];

        for (authentication, expected) in model_required {
            let mut manifest_required: Vec<String> = Vec::new();
            for entry in connection_fields() {
                let key = entry["key"].as_str().unwrap().to_string();
                let binding = entry["binding"].as_str().unwrap_or("");
                // Only credential/config surfaces take part in the auth chain.
                if !matches!(
                    binding,
                    "password" | "secret" | "config"
                ) || key == "authentication"
                {
                    continue;
                }
                let statically_required = entry["required"].as_bool().unwrap_or(false);
                let conditional = condition_field(&entry, "required_when")
                    == Some("authentication")
                    && condition_one_of(&entry, "required_when")
                        .map(|one_of| one_of.iter().any(|value| value == authentication))
                        .unwrap_or(false);
                if statically_required || conditional {
                    manifest_required.push(key);
                }
            }
            let mut expected: Vec<String> =
                expected.iter().map(|value| value.to_string()).collect();
            expected.sort();
            manifest_required.sort();
            assert_eq!(
                manifest_required, expected,
                "required chain mismatch for authentication='{authentication}'"
            );
        }
    }

    /// Every manifest field must land in the store the model reads it from:
    /// `secret` bindings come from `connection_secrets`, `config` bindings
    /// from `external_config` and are consumed by the parser.
    #[test]
    fn bindings_match_parse_surfaces() {
        let secret_keys = ["private_key_passphrase", "sudo_password", "totp_secret"];
        let config_keys = [
            "authentication",
            "private_key_path",
            "agent_socket",
            "connect_timeout_secs",
            "keepalive_interval_secs",
            "sudo_source",
            "sudo_profile",
            "sudo_use_pty",
            "sudo_whitelist",
            "read_only",
            "auth_flow_mode",
            "password_prompt_hint",
            "totp_prompt_hint",
        ];
        let mut seen_secret: Vec<String> = Vec::new();
        let mut seen_config: Vec<String> = Vec::new();
        for entry in connection_fields() {
            match entry["binding"].as_str().unwrap_or("") {
                "secret" => seen_secret.push(entry["key"].as_str().unwrap().to_string()),
                "config" => seen_config.push(entry["key"].as_str().unwrap().to_string()),
                "name" | "host" | "port" | "username" | "password" => {
                    let key = entry["key"].as_str().unwrap();
                    assert!(
                        matches!(key, "display_name" | "host" | "port" | "username" | "password"),
                        "unexpected binding for field '{key}'"
                    );
                }
                other => panic!("unexpected binding '{other}' in manifest"),
            }
        }
        seen_secret.sort_unstable();
        seen_config.sort_unstable();
        let mut expected_secret: Vec<String> =
            secret_keys.iter().map(|value| value.to_string()).collect();
        expected_secret.sort_unstable();
        let mut expected_config: Vec<String> =
            config_keys.iter().map(|value| value.to_string()).collect();
        expected_config.sort_unstable();
        assert_eq!(
            seen_secret, expected_secret,
            "secret bindings must match connection_secrets keys"
        );
        assert_eq!(
            seen_config, expected_config,
            "config bindings must match external_config keys consumed by the parser"
        );
    }

    /// Visible-when pairing: pure Quick Sudo knobs follow the form's sudo
    /// source selection; the 2FA quartet hides under `global` (the bound
    /// profile owns the whole credential source, login-time
    /// keyboard-interactive included) and stays visible for custom/off where
    /// the connection's own values still serve login-time 2FA.
    #[test]
    fn quick_sudo_visibility_pairing() {
        for key in ["sudo_password", "sudo_use_pty"] {
            let entry = field(key);
            assert_eq!(
                condition_field(&entry, "visible_when"),
                Some("sudo_source"),
                "{key} must be gated on sudo_source"
            );
            assert_eq!(
                condition_one_of(&entry, "visible_when"),
                Some(vec!["custom".to_string()]),
                "{key} must be visible only while sudo_source is custom"
            );
        }
        let profile = field("sudo_profile");
        assert_eq!(
            condition_field(&profile, "visible_when"),
            Some("sudo_source"),
            "sudo_profile must be gated on sudo_source"
        );
        assert_eq!(
            condition_one_of(&profile, "visible_when"),
            Some(vec!["global".to_string()]),
            "sudo_profile must be visible only while sudo_source is global"
        );
        for key in [
            "totp_secret",
            "auth_flow_mode",
            "password_prompt_hint",
            "totp_prompt_hint",
        ] {
            assert_eq!(
                condition_field(&field(key), "visible_when"),
                Some("sudo_source"),
                "{key} must be gated on sudo_source"
            );
            assert_eq!(
                condition_one_of(&field(key), "visible_when"),
                Some(vec!["custom".to_string(), "off".to_string()]),
                "{key} must hide under global (profile owns the source) and stay visible for custom/off"
            );
        }
    }

    /// Manifest defaults must equal the parser's fallback defaults; feeding
    /// them through `from_lifecycle_params` reproduces the same connection.
    #[test]
    fn defaults_match_parser_fallbacks() {
        let expected_defaults: &[(&str, Value)] = &[
            ("display_name", Value::from("SSH server")),
            ("host", Value::from("127.0.0.1")),
            ("port", Value::from(22)),
            ("username", Value::from("root")),
            ("authentication", Value::from("password")),
            ("connect_timeout_secs", Value::from(15)),
            ("keepalive_interval_secs", Value::from(30)),
            ("sudo_source", Value::from("custom")),
            ("sudo_use_pty", Value::from(false)),
            ("auth_flow_mode", Value::from("password_then_otp")),
            ("read_only", Value::from(false)),
        ];
        for (key, expected) in expected_defaults {
            assert_eq!(
                field(key)["default"], *expected,
                "manifest default mismatch for '{key}'"
            );
        }

        let connection = StoredConnection::from_lifecycle_params(&serde_json::json!({
            "connection": {
                "id": "defaults",
                "host": "127.0.0.1",
                "port": 22,
                "username": "root",
                "password": "secret",
                "external_config": {
                    "authentication": "password",
                    "connect_timeout_secs": 15,
                    "keepalive_interval_secs": 30,
                    "sudo_source": "custom",
                    "sudo_use_pty": false,
                    "auth_flow_mode": "password_then_otp",
                    "read_only": false
                }
            }
        }))
        .unwrap();
        assert_eq!(connection.authentication, AuthenticationMethod::Password);
        assert_eq!(connection.connect_timeout_secs, 15);
        assert_eq!(connection.keepalive_interval_secs, 30);
        assert!(connection.sudo_enabled());
        assert!(!connection.sudo_use_pty);
        assert_eq!(connection.auth_flow_mode, "password_then_otp");
        assert!(!connection.read_only);
    }

    /// The private_key_path required chain covers private-key-password (the
    /// parser rejects an empty path there); the passphrase stays optional for
    /// unencrypted keys.
    #[test]
    fn private_key_required_when_covers_password_fallback() {
        let entry = field("private_key_path");
        assert_eq!(
            condition_one_of(&entry, "required_when"),
            Some(vec![
                "private-key".to_string(),
                "private-key-password".to_string()
            ])
        );
        assert!(field("private_key_passphrase")["required_when"].is_null());

        let without_passphrase =
            StoredConnection::from_lifecycle_params(&serde_json::json!({
                "connection": {
                    "id": "plain-key",
                    "host": "example.com",
                    "port": 22,
                    "username": "user",
                    "external_config": {
                        "authentication": "private-key-password",
                        "private_key_path": "/keys/id_ed25519"
                    },
                    "connection_secrets": {}
                }
            }))
            .unwrap_err();
        assert!(
            !without_passphrase.contains("private key path"),
            "unencrypted private-key-password keys must not be rejected for a missing passphrase"
        );
    }
}
