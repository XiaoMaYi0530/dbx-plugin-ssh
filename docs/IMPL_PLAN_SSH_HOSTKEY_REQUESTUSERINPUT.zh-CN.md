# SSH 主机密钥确认接入 `host/requestUserInput` 实施方案(#11 / #69)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新建连接 → 测试(`connection/test`)遇到首次连接的主机时,主机密钥确认通过宿主规范通道 `host/requestUserInput` 弹窗(宿主暂停 RPC 截止时间,用户可当场点"信任"),而不是发到没人应答的私有事件上等 300 秒后被宿主 10 秒掐掉。

**Architecture:** 在 vendored Rust SDK(`shared/sdk/rust/dbx-plugin-sdk`)移植上游 Host API 1.1 的 `HostClient`(`request_user_input` + string-id 响应路由);backend 的 `PromptBroker` 在主机密钥挑战时优先走宿主弹窗,宿主不支持(-32601)或无 UI(-32001)时降级到现有工作台事件 `connection/challenge`,宿主不应答则快速 fail closed。连接表单、前端工作台 UI、`connection/challenge` 协议均不变,纯插件侧改动。

**Tech Stack:** Rust(russh sidecar、tokio、serde_json)、vendored `dbx-plugin-sdk`、Vue3 前端(本次不改)。

**Spec:** GitHub issue #69(根因与修法)、#11(现象);宿主契约 `dbx/plugins/README.md` "Host API methods a plugin may call" 一节(下文已摘录关键条款)。

## Global Constraints

- 兼容基线:目标 Host API 1.0(`manifest.json` `engines.host_api: ">=1.0.0"` 不动);Host API 1.1 的 `host/requestUserInput` 一律 optional 降级——宿主未广告 `host.requestUserInput` 时**不得发出**该帧,功能可降不可死(dbx-ssh-dev 技能硬性约定 2)。
- 宿主契约要点(权威,来自 `dbx/plugins/README.md:552-586`):
  - 插件主动请求用**字符串 id**(`"plugin-1"`),宿主自有请求/响应保持数字 id;老宿主不认识的帧直接忽略(**不回错误**),所以 sidecar 必须自带等待超时。
  - 参数:`prompt` 必填(≤2000 字符)、`title`(≤200)、`default`(≤1000)、`options`(≤8 项,值唯一,每项 ≤200 字符)、`echo` 默认 false、`timeoutSecs` 宿主钳到 5–600,默认 300。
  - 结果:`{action:"submit",value}` / `{action:"cancel"}` / `{action:"timeout"}`;cancel/timeout 视为"无应答",**fail closed,不得猜**。
  - 错误:`-32001` 无 UI(headless/MCP 或桌面弹窗未挂载)、`-32602` 参数非法、`-32601` 宿主未实现;三种都必须优雅降级,不允许永久阻塞。
  - 弹窗走宿主自己的阻塞式对话框,对 `connection/test` / `connection/connect` 同样生效,弹窗打开期间宿主**暂停该请求的截止时间**。
  - 能力门控:`plugin/initialize` 的 `host.hostApiVersion` ≥ 1.1.0 且 `host.features` 含 `host.requestUserInput`(**注意:宿主下发的是点分形式 `host.requestUserInput`,见 `dbx/crates/dbx-plugin-runtime/src/plugins/manifest.rs:18`;上游 README 示例里的斜杠形式 `host/requestUserInput` 与实际下发不一致,门控需两种形式都接受**)。
  - 每插件会话最多 4 个并发弹窗(跳板链最多 2 个,不会触顶)。
- 协议命名:不新增 sidecar 方法,`connection/challenge` 事件及其 resolve RPC 原样保留(工作台降级路径与 `scripts/smoke_test.py` 依赖它)。
- 安全红线:cancel/timeout 一律拒绝握手;除 MCP 模式既有 `auto_trust` 外不得自动信任;私钥/指纹不出内容;`docs/PROTOCOL.zh-CN.md` 需同步。
- 后端新增用户可见文案为英文硬编码(与既有 `test_timeout_message` 先例一致);本次**无前端改动,无七语需求**。
- 分支:`codex/ssh/issue-11-69-hostkey-userinput`,base `main`,一个分支完成;认证/协议相关改动 = `human_review_required`,agent 不合并、不安装、不重启 DBX。
- 验证以 `agent-flow.yml` `validation.local` 全清单为准(见 Task 5)。

---

### Task 1: vendored SDK 移植 `HostClient` + `UserInputPrompt` + string-id 响应路由

**Files:**
- Modify: `shared/sdk/rust/dbx-plugin-sdk/src/lib.rs`(参照上游实现 `dbx/plugins/sdk/rust/dbx-plugin-sdk/src/lib.rs`,只读参考,不引入 host 仓库依赖;保留本仓库特有的 `ChannelExecutor` 二进制通道有序化改动)

**Interfaces:**
- Produces(后续任务依赖的导出):
  - `pub const HOST_REQUEST_USER_INPUT_METHOD: &str = "host/requestUserInput";`
  - `pub const HOST_REQUEST_USER_INPUT_FEATURE: &str = "host.requestUserInput";`
  - `pub struct HostClient` + `host_client() -> Option<HostClient>` + `install_host_client(HostClient)`
  - `HostClient::{host_api_version, supports, request, request_with_timeout, request_user_input, note_host_description, deliver_response}`(其中 `note_host_description`/`deliver_response`/`new` 为 crate 内可见)
  - `pub struct UserInputPrompt`(`::secret/::text/::choice`、`with_title/with_default/with_timeout_secs`)、`pub struct UserInputOption { value, label }`、`pub struct UserInputAnswer { action, value }`(`submitted()/is_cancelled()/is_timeout()`)
  - `#[doc(hidden)] pub fn PluginEmitter::for_tests(sink, transport)`(backend 测试构造事件发射器用)
- 内部不变量:`PluginServer::serve` 先 `install_host_client` 再进入读循环;`dispatch_json` 先尝试 `deliver_response` 再按 `ProtocolRequest` 解析;`plugin/initialize` 分支调用 `note_host_description`。

- [ ] **Step 1: 写失败测试(SDK 侧)**

在 `shared/sdk/rust/dbx-plugin-sdk/src/lib.rs` 的 `#[cfg(test)] mod tests` 中追加(沿用上游 `RecordingOutput` 测试模式):

```rust
/// Output sink that records every frame the plugin writes.
#[derive(Clone, Default)]
struct RecordingOutput(Arc<Mutex<Vec<u8>>>);

impl RecordingOutput {
    fn lines(&self) -> Vec<Value> {
        let bytes = self.0.lock().unwrap().clone();
        String::from_utf8(bytes)
            .unwrap()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

impl std::io::Write for RecordingOutput {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buffer);
        Ok(())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn recording_client() -> (HostClient, RecordingOutput) {
    let output = RecordingOutput::default();
    let sink: Arc<Mutex<Box<dyn std::io::Write + Send>>> = Arc::new(Mutex::new(Box::new(output.clone())));
    let client = HostClient::new(sink, PluginTransport::JsonLines);
    install_host_client(client.clone());
    (client, output)
}

#[test]
fn host_client_gates_on_advertised_features() {
    let (client, _output) = recording_client();
    assert!(!client.supports(HOST_REQUEST_USER_INPUT_FEATURE));
    assert_eq!(client.host_api_version(), None);

    // The real host advertises the dot form (dbx-plugin-runtime manifest.rs).
    client.note_host_description(&serde_json::json!({
        "host": { "hostApiVersion": "1.1.0", "features": [HOST_REQUEST_USER_INPUT_FEATURE] }
    }));
    assert_eq!(client.host_api_version().as_deref(), Some("1.1.0"));
    assert!(client.supports(HOST_REQUEST_USER_INPUT_FEATURE));
    // supports() is an exact match; callers must pass the advertised form.
    assert!(!client.supports(HOST_REQUEST_USER_INPUT_METHOD));
}

#[test]
fn user_input_prompt_serializes_host_api_params() {
    let prompt = UserInputPrompt::choice(
        "Trust this host?",
        vec![
            UserInputOption { value: "accept".into(), label: "Trust once".into() },
            UserInputOption { value: "remember".into(), label: "Trust and remember".into() },
        ],
    )
    .with_title("SSH host key — h:22")
    .with_timeout_secs(300);
    let value = serde_json::to_value(&prompt).unwrap();
    assert_eq!(value["prompt"], "Trust this host?");
    assert_eq!(value["title"], "SSH host key — h:22");
    assert_eq!(value["echo"], true);
    assert_eq!(value["options"][1]["value"], "remember");
    assert_eq!(value["timeoutSecs"], 300);
    assert_eq!(value["options"].as_array().unwrap().len(), 2);

    let secret = serde_json::to_value(UserInputPrompt::secret("code")).unwrap();
    assert_eq!(secret["echo"], false);
    assert!(secret.get("title").is_none());
    assert!(secret.get("options").is_none());
    assert!(secret.get("timeoutSecs").is_none());
}

#[test]
fn host_client_routes_string_id_answers() {
    let (client, output) = recording_client();
    client.note_host_description(&serde_json::json!({
        "host": { "hostApiVersion": "1.1.0", "features": [HOST_REQUEST_USER_INPUT_FEATURE] }
    }));

    let caller = {
        let client = client.clone();
        thread::spawn(move || client.request_user_input(&UserInputPrompt::secret("code")))
    };
    let request = loop {
        if let Some(request) = output.lines().into_iter().find(|line| line["method"] == HOST_REQUEST_USER_INPUT_METHOD) {
            break request;
        }
        thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(request["id"].as_str().unwrap().starts_with("plugin-"), true);
    assert_eq!(request["params"]["prompt"], "code");

    client.deliver_response(&serde_json::json!({
        "jsonrpc": "2.0", "id": request["id"], "result": { "action": "submit", "value": "000000" }
    }));
    let answer = caller.join().unwrap().unwrap();
    assert_eq!(answer.submitted(), Some("000000"));
}

#[test]
fn host_client_surfaces_host_errors_and_local_timeouts() {
    let (client, output) = recording_client();
    let caller = {
        let client = client.clone();
        thread::spawn(move || client.request_user_input(&UserInputPrompt::secret("code")))
    };
    let request = loop {
        if let Some(request) = output.lines().into_iter().find(|line| line["method"] == HOST_REQUEST_USER_INPUT_METHOD) {
            break request;
        }
        thread::sleep(Duration::from_millis(5));
    };
    client.deliver_response(&serde_json::json!({
        "jsonrpc": "2.0", "id": request["id"], "error": { "code": -32001, "message": "no user interface" }
    }));
    let error = caller.join().unwrap().unwrap_err();
    assert_eq!(error.code, -32001);

    // No answer at all: the local wait must expire (older hosts drop the frame).
    let started = Instant::now();
    let error = client
        .request_with_timeout(HOST_REQUEST_USER_INPUT_METHOD, serde_json::json!({"prompt": "x"}), Duration::from_millis(50))
        .unwrap_err();
    assert_eq!(error.code, -32001);
    assert!(error.message.contains("did not answer"));
    assert!(started.elapsed() >= Duration::from_millis(40));
}

#[test]
fn deliver_response_ignores_numeric_id_requests() {
    let (client, _output) = recording_client();
    // Host -> plugin requests keep numeric ids and must fall through to the
    // request handler, never be swallowed as answers.
    assert!(!client.deliver_response(&serde_json::json!({
        "jsonrpc": "2.0", "id": 7, "method": "connection/test", "params": {}
    })));
}
```

需要的测试导入补充:`use std::time::{Duration, Instant};`(现有测试块已有 `thread`/`Arc`/`Mutex`)。

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test --manifest-path backend/Cargo.toml -p dbx-plugin-sdk 2>&1 | tail -5`
Expected: 编译失败(`HostClient`、`UserInputPrompt` 等不存在)。

- [ ] **Step 3: 移植实现(保留本仓库 ChannelExecutor)**

对 `shared/sdk/rust/dbx-plugin-sdk/src/lib.rs` 做以下修改(顺序即文件顺序):

3a. 顶部导入与常量:

```rust
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub const HOST_REQUEST_USER_INPUT_METHOD: &str = "host/requestUserInput";
/// The dot form is what the host actually advertises in `plugin/initialize`
/// (`host.features`); the slash form appears in docs. `supports()` is an
/// exact match, so callers gate on the form they expect.
pub const HOST_REQUEST_USER_INPUT_FEATURE: &str = "host.requestUserInput";
const DEFAULT_HOST_REQUEST_TIMEOUT: Duration = Duration::from_secs(330);
```

3b. 把 `PluginEmitter::write_json` 的落盘逻辑抽成自由函数,并给 `PluginEmitter` 加测试构造器(放在 `impl PluginEmitter` 内):

```rust
fn write_json_frame(
    output: &Arc<Mutex<Box<dyn Write + Send>>>,
    transport: PluginTransport,
    payload: &[u8],
) -> Result<(), PluginError> {
    let mut output = output.lock().map_err(|_| PluginError::new(-32000, "Plugin output lock is poisoned"))?;
    match transport {
        PluginTransport::JsonLines => {
            output.write_all(payload).map_err(io_error)?;
            output.write_all(b"\n").map_err(io_error)?;
        }
        PluginTransport::Framed => {
            output.write_all(&[FRAME_KIND_JSON]).map_err(io_error)?;
            output.write_all(&(payload.len() as u32).to_be_bytes()).map_err(io_error)?;
            output.write_all(payload).map_err(io_error)?;
        }
    }
    output.flush().map_err(io_error)
}
```

`PluginEmitter::write_json` 改为调用 `write_json_frame(&self.output, self.transport, &payload)`;另加:

```rust
impl PluginEmitter {
    /// Test-only constructor so downstream crates can script events and
    /// assert what a handler emitted. Hidden from the documented API.
    #[doc(hidden)]
    pub fn for_tests(
        sink: Arc<Mutex<Box<dyn Write + Send>>>,
        transport: PluginTransport,
    ) -> Self {
        Self { output: sink, transport }
    }
}
```

3c. 追加 Host API 1.1 类型与 `HostClient`(放在 `PluginEmitter` 实现之后、`PluginServer` 之前;实现照上游移植,含 `HostDescription`、`note_host_description`、`deliver_response`、`request`/`request_with_timeout`/`request_user_input`、`HOST_CLIENT` 静态、`host_client()`/`install_host_client()`/`request_user_input()` 便捷函数;代码全文见上游 `plugins/sdk/rust/dbx-plugin-sdk/src/lib.rs:170-460`,此处不逐行重抄,唯一调整:`UserInputAnswer` 增加 `#[derive(Clone)]` 以便 backend 测试脚本复用):

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputPrompt {
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub echo: bool,
    #[serde(rename = "default", skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<UserInputOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}
// UserInputOption / UserInputPrompt 构造器 / UserInputAnswer 与上游逐字一致。
```

3d. `PluginServer::serve` 改造(共享输出 + 先装 client):

```rust
pub fn serve(self) -> io::Result<()> {
    let output: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(Box::new(io::stdout())));
    let emitter = PluginEmitter { output: output.clone(), transport: self.transport };
    // Publish before serving so even the first request can call back.
    install_host_client(HostClient::new(output, self.transport));
    let workers = WorkerPool::new(self.worker_threads, self.work_queue_capacity)?;
    let binary = ChannelExecutor::new(workers.clone());
    match self.transport {
        PluginTransport::JsonLines => self.serve_json_lines(BufReader::new(io::stdin()), emitter, &workers),
        PluginTransport::Framed => self.serve_framed(io::stdin(), emitter, &workers, &binary),
    }
}
```

3e. `dispatch_json` 开头插入响应路由,`plugin/initialize` 分支记录宿主描述(其余分支不动):

```rust
fn dispatch_json(&self, payload: &[u8], emitter: PluginEmitter, workers: &WorkerPool) -> Result<(), String> {
    let value: Value = serde_json::from_slice(payload).map_err(|error| error.to_string())?;
    // Answers to plugin-initiated Host API calls carry string ids; host ->
    // plugin requests keep numeric ids. Route answers first: ProtocolRequest
    // has no "method" field for them and would fail to parse.
    if let Some(host) = host_client() {
        if host.deliver_response(&value) {
            return Ok(());
        }
    }
    let request: ProtocolRequest = serde_json::from_value(value).map_err(|error| error.to_string())?;
    // ...现有 jsonrpc 校验、validate_protocol_name 不变...
    if request.method == "plugin/initialize" {
        let id = request.id.ok_or("plugin/initialize must be a request")?;
        if let Some(host) = host_client() {
            host.note_host_description(&request.params);
        }
        // ...现有 protocolVersions 支持判断与应答不变...
    }
    // ...worker 提交不变...
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test --manifest-path backend/Cargo.toml -p dbx-plugin-sdk`
Expected: 新增 5 个测试 PASS,SDK 原有测试(worker pool、channel lanes、limited line reader)全部 PASS。

- [ ] **Step 5: 格式与静态检查**

Run: `cargo fmt --manifest-path backend/Cargo.toml --check && cargo clippy --locked --manifest-path backend/Cargo.toml --all-targets -- -D warnings`
Expected: 无输出/无警告。

- [ ] **Step 6: Commit**

```bash
git add shared/sdk/rust/dbx-plugin-sdk/src/lib.rs
git commit -m "feat(sdk): port HostClient request_user_input and string-id answer routing from Host API 1.1"
```

---

### Task 2: backend 主机密钥挑战优先走宿主弹窗,降级/fail-closed

**Files:**
- Modify: `backend/src/ssh.rs`(`PromptBroker` 及其 `request`、`SshClient::check_server_key` 调用处不变)

**Interfaces:**
- Consumes: Task 1 的 `dbx_plugin_sdk::{host_client, UserInputPrompt, UserInputOption, UserInputAnswer, HOST_REQUEST_USER_INPUT_FEATURE, HOST_REQUEST_USER_INPUT_METHOD}`、`PluginEmitter::for_tests`。
- Produces:
  - `trait HostPromptGateway: Send + Sync { fn supports_request_user_input(&self) -> bool; fn request_user_input(&self, prompt: &UserInputPrompt) -> Result<UserInputAnswer, PluginError>; }`
  - `struct SdkHostPromptGateway;`(实现读 `dbx_plugin_sdk::host_client()`)
  - `PromptBroker`:新增字段 `gateway: Arc<dyn HostPromptGateway>`、`challenge_raised: Arc<AtomicBool>`;新增 `fn with_gateway(gateway: Arc<dyn HostPromptGateway>) -> Self`(测试用)与 `fn clear_challenge_raised(&self)` / `fn challenge_was_raised(&self) -> bool`(Task 3 用)。
  - `PromptBroker::request(host, port, key_type, fingerprint, connection_id, operation_id, emitter) -> Option<PromptDecision>` 签名不变;内部:宿主弹窗 → 失败分类(可降级→走原事件路径,重命名为 `request_via_workbench`;不可降级→`None`)。
  - 提示取值映射:`submit`+`"accept"` → `{accept:true, remember:false}`;`submit`+`"remember"` → `{accept:true, remember:true}`;`cancel`/`timeout`/未知 → `{accept:false, remember:false}`(拒绝,fail closed)。

- [ ] **Step 1: 写失败测试(backend 侧)**

在 `backend/src/ssh.rs` 的 `#[cfg(test)] mod tests` 中追加:

```rust
mod host_key_prompt {
    use super::*;
    use dbx_plugin_sdk::{PluginTransport, UserInputAnswer, UserInputPrompt};
    use std::sync::atomic::Ordering;

    struct SharedSink(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for SharedSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    fn events(sink: &Arc<Mutex<Vec<u8>>>) -> Vec<serde_json::Value> {
        String::from_utf8(sink.lock().unwrap().clone())
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    fn emitter_for_test() -> (PluginEmitter, Arc<Mutex<Vec<u8>>>) {
        let sink = Arc::new(Mutex::new(Vec::new()));
        (
            dbx_plugin_sdk::PluginEmitter::for_tests(
                Arc::new(Mutex::new(Box::new(SharedSink(sink.clone())))),
                PluginTransport::JsonLines,
            ),
            sink,
        )
    }

    struct ScriptedGateway {
        supports: bool,
        answer: Mutex<Result<UserInputAnswer, PluginError>>,
        prompts_seen: Mutex<Vec<serde_json::Value>>,
    }

    impl ScriptedGateway {
        fn supports() -> Self {
            Self { supports: true, answer: Mutex::new(Ok(UserInputAnswer { action: "submit".into(), value: Some("remember".into()) })), prompts_seen: Mutex::new(Vec::new()) }
        }
        fn without_feature() -> Self {
            Self { supports: false, answer: Mutex::new(Ok(UserInputAnswer { action: "submit".into(), value: None })), prompts_seen: Mutex::new(Vec::new()) }
        }
        fn answering(answer: Result<UserInputAnswer, PluginError>) -> Self {
            Self { supports: true, answer: Mutex::new(answer), prompts_seen: Mutex::new(Vec::new()) }
        }
    }

    impl HostPromptGateway for ScriptedGateway {
        fn supports_request_user_input(&self) -> bool { self.supports }
        fn request_user_input(&self, prompt: &UserInputPrompt) -> Result<UserInputAnswer, PluginError> {
            self.prompts_seen.lock().unwrap().push(serde_json::to_value(prompt).unwrap());
            self.answer.lock().unwrap().clone()
        }
    }

    const HOST: &str = "server.example.com";
    const FINGERPRINT: &str = "SHA256:abcdefgh";

    async fn challenge_via(broker: &PromptBroker, emitter: &PluginEmitter) -> Option<PromptDecision> {
        broker
            .request(HOST, 22, "ssh-ed25519".into(), FINGERPRINT.into(), "conn-1", "op-1", emitter)
            .await
    }

    #[tokio::test]
    async fn host_dialog_submit_remember_maps_to_accept_and_remember() {
        let gateway = Arc::new(ScriptedGateway::supports());
        let broker = PromptBroker::with_gateway(gateway.clone());
        let (emitter, sink) = emitter_for_test();

        let decision = challenge_via(&broker, &emitter).await;

        assert_eq!(decision, Some(PromptDecision { accept: true, remember: true }));
        assert!(events(&sink).iter().all(|event| event["method"] != "connection/challenge"));
        let prompt = gateway.prompts_seen.lock().unwrap()[0].clone();
        assert_eq!(prompt["options"][0]["value"], "accept");
        assert_eq!(prompt["options"][1]["value"], "remember");
        assert!(prompt["prompt"].as_str().unwrap().contains(FINGERPRINT));
        assert!(broker.challenge_was_raised());
    }

    #[tokio::test]
    async fn host_dialog_accept_maps_to_accept_without_remember() {
        let gateway = Arc::new(ScriptedGateway::answering(Ok(UserInputAnswer { action: "submit".into(), value: Some("accept".into()) })));
        let broker = PromptBroker::with_gateway(gateway);
        let (emitter, _sink) = emitter_for_test();
        assert_eq!(challenge_via(&broker, &emitter).await, Some(PromptDecision { accept: true, remember: false }));
    }

    #[tokio::test]
    async fn host_dialog_cancel_and_timeout_reject_fail_closed() {
        for action in ["cancel", "timeout"] {
            let gateway = Arc::new(ScriptedGateway::answering(Ok(UserInputAnswer { action: action.into(), value: None })));
            let broker = PromptBroker::with_gateway(gateway);
            let (emitter, sink) = emitter_for_test();
            assert_eq!(challenge_via(&broker, &emitter).await, Some(PromptDecision { accept: false, remember: false }));
            assert!(events(&sink).iter().all(|event| event["method"] != "connection/challenge"));
        }
    }

    #[tokio::test]
    async fn host_without_feature_falls_back_to_workbench_event() {
        let gateway = Arc::new(ScriptedGateway::without_feature());
        let broker = PromptBroker::with_gateway(gateway);
        let (emitter, sink) = emitter_for_test();

        let pending = tokio::spawn({
            let broker = broker.clone();
            let emitter = emitter.clone();
            async move { broker.request(HOST, 22, "ssh-ed25519".into(), FINGERPRINT.into(), "conn-1", "op-1", &emitter).await }
        });
        // 降级路径必须发出既有事件载荷(workbench 依赖 challengeId/kind 字段)。
        let mut challenge_id = None;
        for _ in 0..200 {
            if let Some(event) = events(&sink).into_iter().find(|event| event["method"] == "connection/challenge") {
                challenge_id = Some(event["params"]["challengeId"].as_str().unwrap().to_string());
                assert_eq!(event["params"]["kind"], "host-key");
                assert_eq!(event["params"]["fingerprint"], FINGERPRINT);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let challenge_id = challenge_id.expect("legacy challenge event must be emitted");
        broker.resolve(&challenge_id, "op-1", PromptDecision { accept: true, remember: true }).unwrap();
        assert_eq!(pending.await.unwrap(), Some(PromptDecision { accept: true, remember: true }));
    }

    #[tokio::test]
    async fn no_ui_error_falls_back_to_workbench_event() {
        let gateway = Arc::new(ScriptedGateway::answering(Err(PluginError::new(-32001, "no user interface is attached"))));
        let broker = PromptBroker::with_gateway(gateway);
        let (emitter, sink) = emitter_for_test();
        let pending = tokio::spawn({
            let broker = broker.clone();
            let emitter = emitter.clone();
            async move { broker.request(HOST, 22, "ssh-ed25519".into(), FINGERPRINT.into(), "conn-1", "op-1", &emitter).await }
        });
        let mut challenge_id = None;
        for _ in 0..200 {
            if let Some(event) = events(&sink).into_iter().find(|event| event["method"] == "connection/challenge") {
                challenge_id = Some(event["params"]["challengeId"].as_str().unwrap().to_string());
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        broker.resolve(&challenge_id.expect("fallback event"), "op-1", PromptDecision { accept: false, remember: false }).unwrap();
        assert_eq!(pending.await.unwrap(), Some(PromptDecision { accept: false, remember: false }));
    }

    #[tokio::test]
    async fn host_silence_fails_closed_without_fallback() {
        // SDK 本地超时的错误以 -32001 + "did not answer" 表达;此时再降级会
        // 把总等待拖到 630s,必须直接拒绝。
        let gateway = Arc::new(ScriptedGateway::answering(Err(PluginError::new(-32001, "Host did not answer 'host/requestUserInput' in time"))));
        let broker = PromptBroker::with_gateway(gateway);
        let (emitter, sink) = emitter_for_test();
        assert_eq!(challenge_via(&broker, &emitter).await, None);
        assert!(events(&sink).iter().all(|event| event["method"] != "connection/challenge"));
    }

    #[tokio::test]
    async fn challenge_raised_flag_clears_between_probes() {
        let gateway = Arc::new(ScriptedGateway::supports());
        let broker = PromptBroker::with_gateway(gateway);
        let (emitter, _sink) = emitter_for_test();
        broker.challenge_raised_flag.store(true, Ordering::Relaxed);
        broker.clear_challenge_raised();
        let _ = challenge_via(&broker, &emitter).await;
        assert!(broker.challenge_was_raised());
        broker.clear_challenge_raised();
        assert!(!broker.challenge_was_raised());
    }
}
```

注:`challenge_raised` 字段对 tests 模块可见(crate 内 `pub(crate)` 或 `pub(super)`),测试最后一例直接触碰字段以验证语义;若 clippy 不允许,可改为经 `clear_challenge_raised` + 一次成功挑战来断言。

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test --locked --manifest-path backend/Cargo.toml host_key_prompt 2>&1 | tail -5`
Expected: 编译失败(`HostPromptGateway`、`PromptBroker::with_gateway` 不存在)。

- [ ] **Step 3: 实现**

3a. 导入(ssh.rs 顶部):

```rust
use dbx_plugin_sdk::{
    host_client, PluginEmitter, PluginError, UserInputAnswer, UserInputOption, UserInputPrompt,
    HOST_REQUEST_USER_INPUT_FEATURE, HOST_REQUEST_USER_INPUT_METHOD,
};
use std::sync::atomic::{AtomicBool, Ordering};
```

3b. 网关与降级判定(放在 `PromptBroker` 定义旁):

```rust
/// Whether the attached DBX host advertised the Host API 1.1 user-input
/// dialog. The host sends the dot form; the slash form appears in docs, so
/// accept both when gating.
fn host_supports_user_input() -> bool {
    host_client()
        .map(|client| client.supports(HOST_REQUEST_USER_INPUT_FEATURE) || client.supports(HOST_REQUEST_USER_INPUT_METHOD))
        .unwrap_or(false)
}

/// Seam between the broker and Host API 1.1; tests script it instead of a
/// live host process.
trait HostPromptGateway: Send + Sync {
    fn supports_request_user_input(&self) -> bool;
    fn request_user_input(&self, prompt: &UserInputPrompt) -> Result<UserInputAnswer, PluginError>;
}

struct SdkHostPromptGateway;

impl HostPromptGateway for SdkHostPromptGateway {
    fn supports_request_user_input(&self) -> bool {
        host_supports_user_input()
    }

    fn request_user_input(&self, prompt: &UserInputPrompt) -> Result<UserInputAnswer, PluginError> {
        match host_client() {
            Some(client) => client.request_user_input(prompt),
            None => Err(PluginError::new(-32000, "Host API is unavailable: the plugin server is not running")),
        }
    }
}
```

3c. `PromptBroker` 改造(`Default` 手写;`request` 拆成编排 + `request_via_workbench`,原 300s 事件等待逻辑整体移入 `request_via_workbench` 不改语义):

```rust
#[derive(Clone)]
pub struct PromptBroker {
    pending: Arc<AsyncMutex<HashMap<String, PendingPrompt>>>,
    gateway: Arc<dyn HostPromptGateway>,
    /// Set whenever a confirmation was raised, whichever channel served it;
    /// `connection/test` reads it to make its timeout guidance truthful.
    pub(super) challenge_raised: Arc<AtomicBool>,
}

impl Default for PromptBroker {
    fn default() -> Self {
        Self {
            pending: Arc::new(AsyncMutex::new(HashMap::new())),
            gateway: Arc::new(SdkHostPromptGateway),
            challenge_raised: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl PromptBroker {
    pub(crate) fn with_gateway(gateway: Arc<dyn HostPromptGateway>) -> Self {
        Self { gateway, ..Self::default() }
    }

    pub(crate) fn clear_challenge_raised(&self) {
        self.challenge_raised.store(false, Ordering::Relaxed);
    }

    pub(crate) fn challenge_was_raised(&self) -> bool {
        self.challenge_raised.load(Ordering::Relaxed)
    }

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
        if self.gateway.supports_request_user_input() {
            match self.request_via_host(host, port, &key_type, &fingerprint) {
                Ok(decision) => return decision,
                // Host predates the dialog (-32601) or no dialog surface is
                // mounted (-32001 without the SDK's own timeout wording): the
                // workbench event may still reach a UI.
                Err(error) if Self::host_prompt_unavailable(&error) => {}
                // Anything else — including the SDK's own -32001 "did not
                // answer" timeout — must fail closed instead of stacking
                // another 300s wait on top.
                Err(_) => return None,
            }
        }
        self.request_via_workbench(host, port, key_type, fingerprint, connection_id, operation_id, emitter)
            .await
    }

    /// Blocking on purpose: runs on the SDK worker thread already driving
    /// this handshake, and the host pauses the RPC deadline while its dialog
    /// is open, so the wait cannot be mistaken for a connect timeout.
    fn request_via_host(
        &self,
        host: &str,
        port: u16,
        key_type: &str,
        fingerprint: &str,
    ) -> Result<Option<PromptDecision>, PluginError> {
        self.challenge_raised.store(true, Ordering::Relaxed);
        let prompt = UserInputPrompt::choice(
            format!(
                "The authenticity of host {host}:{port} can't be established.\n\
                 Key type: {key_type}\n\
                 SHA-256 fingerprint: {fingerprint}\n\
                 Trust this host and continue connecting?"
            ),
            vec![
                UserInputOption { value: "accept".to_string(), label: "Trust once".to_string() },
                UserInputOption { value: "remember".to_string(), label: "Trust and remember".to_string() },
            ],
        )
        .with_title(format!("SSH host key — {host}:{port}"))
        .with_timeout_secs(HOST_KEY_CHALLENGE_WAIT.as_secs());
        let answer = self.gateway.request_user_input(&prompt)?;
        Ok(Some(match answer.action.as_str() {
            // Unknown values are treated as a rejection: never guess.
            "submit" => PromptDecision { accept: true, remember: answer.value.as_deref() == Some("remember") },
            _ => PromptDecision { accept: false, remember: false },
        }))
    }

    fn host_prompt_unavailable(error: &PluginError) -> bool {
        error.code == -32601
            || (error.code == -32001 && !error.message.contains("did not answer"))
    }

    async fn request_via_workbench(/* 原 request 的参数与函数体,原样保留 */) -> Option<PromptDecision> {
        self.challenge_raised.store(true, Ordering::Relaxed);
        // ...原函数体:challenge_id、pending 注册、connection/challenge 事件、
        //    300s tokio timeout、resolve 处理,全部不动...
    }
    // resolve() 不变
}
```

3d. `SshClient::check_server_key` 不改调用方式(`prompts.request(...)` 签名未变)。

- [ ] **Step 4: 运行测试确认通过 + 既有测试无回归**

Run: `cargo test --locked --manifest-path backend/Cargo.toml`
Expected: `host_key_prompt` 模块 7 个测试 PASS;原有 `mcp_confirm_challenge_payload_shape`、host-key verdict 等测试 PASS。

- [ ] **Step 5: fmt + clippy**

Run: `cargo fmt --manifest-path backend/Cargo.toml --check && cargo clippy --locked --manifest-path backend/Cargo.toml --all-targets -- -D warnings`
Expected: 通过。

- [ ] **Step 6: Commit**

```bash
git add backend/src/ssh.rs
git commit -m "fix(ssh): confirm host keys through the host dialog on connection/test (#11, #69)"
```

---

### Task 3: `connection/test` 超时提示补充"主机密钥确认待决"上下文

**Files:**
- Modify: `backend/src/ssh.rs`(`test_timeout_message`、`test_connection`;宿主为 Host API 1.0 且无工作台时,超时文案要说明真实原因,而非误导用户去调大超时)

**Interfaces:**
- Consumes: Task 2 的 `PromptBroker::{clear_challenge_raised, challenge_was_raised}`。
- Produces: `fn test_timeout_message(host: &str, port: u16, budget_secs: u64, host_default: bool, challenge_raised: bool) -> String`(新增第 5 参数)。

- [ ] **Step 1: 写失败测试**

在 `mod tests` 中找到/新增对 `test_timeout_message` 的断言:

```rust
#[test]
fn test_timeout_message_names_pending_host_key_confirmation() {
    let base = test_timeout_message("h", 22, 9, true, false);
    assert!(base.contains("timed out after 9 seconds (host default)"));
    assert!(base.contains("Increase 'SSH timeout'"));

    let with_challenge = test_timeout_message("h", 22, 9, false, true);
    assert!(with_challenge.contains("host-key confirmation"));
    assert!(!with_challenge.contains("(host default)"));
}
```

第二个测试的现实约束:`test_connection` 的超时路径需要真实 SSH 拨号(挑战要握手才触发),纯单测不连 SSH,所以"标志位 → 消息"的接线由源码审查 + Task 4 冒烟兜底,单测只锁两件可测的事:消息函数本身(上面,放 `mod tests`)和标志位在两次探针之间不串扰(下面,放进 Task 2 的 `mod host_key_prompt`,复用其 `ScriptedGateway`):

```rust
#[test]
fn challenge_flag_clears_between_probes() {
    let gateway = Arc::new(ScriptedGateway::without_feature());
    let broker = PromptBroker::with_gateway(gateway);
    broker.challenge_raised.store(true, std::sync::atomic::Ordering::Relaxed);
    broker.clear_challenge_raised();
    assert!(!broker.challenge_was_raised());
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test --locked --manifest-path backend/Cargo.toml test_timeout_message 2>&1 | tail -5`
Expected: 编译失败(参数个数不符)。

- [ ] **Step 3: 实现**

```rust
fn test_timeout_message(host: &str, port: u16, budget_secs: u64, host_default: bool, challenge_raised: bool) -> String {
    let source = if host_default { " (host default)" } else { "" };
    let mut message = format!(
        "SSH connection to {host}:{port} timed out after {budget_secs} seconds{source}. Increase 'SSH timeout' under Advanced options and retry."
    );
    if challenge_raised {
        message.push_str(
            " A host-key confirmation was raised but went unanswered; connect once from the SSH workbench to trust this host, or update DBX to 0.6.17+ so the confirmation can appear here.",
        );
    }
    message
}
```

`test_connection` 改动(ssh.rs:1679 起):

```rust
pub async fn test_connection(&self, connection: &StoredConnection, operation_id: &str, emitter: PluginEmitter) -> Result<(), String> {
    self.prompts.clear_challenge_raised();
    // ...budget 计算不变...
    match tokio::time::timeout(Duration::from_secs(budget_secs), probe).await {
        Ok(result) => result?,
        Err(_elapsed) => {
            return Err(test_timeout_message(
                &connection.runtime_host,
                connection.runtime_port,
                budget_secs,
                !connection.connect_timeout_explicit,
                self.prompts.challenge_was_raised(),
            ));
        }
    }
    // ...disconnect 不变...
}
```

更新 `test_timeout_message` 的全部调用点(仅 `test_connection` 一处 + 测试)。

- [ ] **Step 4: 运行测试**

Run: `cargo test --locked --manifest-path backend/Cargo.toml`
Expected: 全部 PASS。

- [ ] **Step 5: fmt + clippy + Commit**

```bash
cargo fmt --manifest-path backend/Cargo.toml --check && cargo clippy --locked --manifest-path backend/Cargo.toml --all-targets -- -D warnings
git add backend/src/ssh.rs
git commit -m "fix(ssh): name pending host-key confirmation in connection/test timeout guidance (#11)"
```

---

### Task 4: 文档同步 + 冒烟回归(legacy 路径不回归)

**Files:**
- Modify: `docs/PROTOCOL.zh-CN.md`(新增小节)、`docs/FEATURE_PARITY.zh-CN.md`(状态行)

**Interfaces:** 无代码接口;文档必须覆盖降级矩阵与 fail-closed 语义。

- [ ] **Step 1: `docs/PROTOCOL.zh-CN.md` 新增小节(放在"生命周期与状态隔离"之后或 RPC 章节合适位置)**

```markdown
## 主机密钥确认通道(requestUserInput)

首次连接(或主机密钥变更)时,sidecar 的确认请求按以下顺序选通道:

1. **宿主弹窗(优先)**:宿主在 `plugin/initialize` 广告 `host.hostApiVersion >= 1.1.0`
   且 `host.features` 含 `host.requestUserInput`(点分形式)时,sidecar 直接调用
   `host/requestUserInput`(字符串 id `plugin-N`,`echo: true`,`options:
   accept/remember`,`timeoutSecs: 300`)。弹窗期间宿主暂停 `connection/test` /
   `connection/connect` 的请求截止时间,连接表单里即可完成信任。
2. **工作台事件(降级)**:宿主不支持(-32601)或无可用弹窗面(-32001 且非 SDK
   本地超时)时,仍发既有事件 `connection/challenge`,由工作台 UI 应答
   (`connection/challenge/resolve`),语义与字段不变。
3. **fail closed**:用户 cancel/timeout、宿主对请求不应答(SDK 本地 330s 超时,
   `-32001` + "did not answer")、或其他错误——一律拒绝握手,不降级、不猜测。
   MCP 模式的 `auto_trust`(TOFU)行为不变。

已知限制:Host API 1.0 宿主 + 工作台未打开(连接表单路径)仍无应答者,`connection/test`
约 9s 后返回可读超时文案(0.4.78+ 缓解),文案在挑战已发出时附指引。
```

- [ ] **Step 2: `docs/FEATURE_PARITY.zh-CN.md` 更新主机密钥确认行**

状态改为:连接表单内确认 → 支持(需宿主 ≥0.6.17 / Host API 1.1);旧宿主降级说明指向 PROTOCOL 小节。

- [ ] **Step 3: 冒烟回归(直接驱动 sidecar,initialize 无 features → 必须走 legacy 路径)**

```bash
cargo build --manifest-path backend/Cargo.toml
DBX_PLUGIN_DATA_DIR=$(mktemp -d) DBX_PLUGIN_SIDECAR=target/debug/dbx-plugin-ssh \
  python3 scripts/smoke_test.py
```

Expected: 与改动前一致——`auto_accept_challenge` 能看到 `connection/challenge` 事件并自动应答,全链路(连接→挑战→PTY→SFTP→关闭)通过。smoke 的 `plugin/initialize` 不带 `host.features`,`supports()=false`,因此必然走降级路径,正好验证回归安全。

- [ ] **Step 4: Commit**

```bash
git add docs/PROTOCOL.zh-CN.md docs/FEATURE_PARITY.zh-CN.md
git commit -m "docs: host-key confirmation channel (requestUserInput) and parity status"
```

---

### Task 5: 全量本地验证 + PR 交接记录

**Files:** 无新文件;产出 PR 描述。

- [ ] **Step 1: 按 `agent-flow.yml` `validation.local` 全清单验证**

```bash
python3 scripts/validate_repo.py
node scripts/connection-forms/verify.mjs
pnpm --dir frontend typecheck && pnpm --dir frontend test && pnpm --dir frontend build
cargo fmt --manifest-path backend/Cargo.toml --check
cargo clippy --locked --manifest-path backend/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path backend/Cargo.toml
```

Expected: 全绿(前端虽无改动也必须跑,证明 vendored SDK 变更未破坏前端构建管线)。

- [ ] **Step 2: 推分支、开 PR,记录交接要素**

- 分支:`codex/ssh/issue-11-69-hostkey-userinput`;PR 描述记录 base SHA、变更文件清单、测试结果、`Fixes #11`/`Closes #69` 关联(合并后生效)。
- 高风险声明:主机密钥确认属于认证路径 + sidecar↔宿主协议帧路由改动,按仓库约定 `human_review_required`。
- 剩余风险(如实写):
  1. `-32001` 错误码同时表示"无 UI"与 SDK 本地超时,降级判定靠 message 文案区分(`"did not answer"`),上游若改文案需同步;
  2. `challenge_raised` 标志为 SshService 级共享,并发测试连接时可能互相看到对方的挑战标志,只影响提示文案,不影响安全语义;
  3. 宿主 1.0 + 无工作台路径本次不修(结构性不可修),文档已注明;
  4. 上游 README 的 `supports("host/requestUserInput")` 示例与宿主实际下发的点分形式不一致,门控两种形式都接受(防御)。
- Follow-up(不在本 PR):`ssh/agent/prompt`、keyboard-interactive MFA 若也要接 `requestUserInput`,另开 issue。

---

## 自审记录(Self-Review)

- **Spec 覆盖**:#69 三条修法 → ①requestUserInput = Task 2;②SDK 同步 = Task 1(带能力门控);③快速降级 = Task 2(fail-closed 分类)+ Task 3(可读超时文案增强,0.4.78 已有基础上补挑战上下文)。#11 现象(表单测试超时)在宿主 ≥0.6.17 下根除,≤0.6.16 降级不变。
- **占位符扫描**:`request_via_workbench` 注明"原函数体原样保留"是有意的移动而非待办;其余步骤均含实际代码。
- **类型一致性**:`PromptDecision{accept,remember}`、`UserInputAnswer{action,value}`、`HostPromptGateway` 两个方法签名在 Task 1/2/3 间已互相对齐;`test_timeout_message` 新签名在 Task 3 内自洽。
