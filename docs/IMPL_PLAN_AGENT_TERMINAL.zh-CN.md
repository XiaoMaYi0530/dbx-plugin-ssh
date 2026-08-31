# IMPL PLAN：AI 终端同步执行（agent terminal mode，io.dbx.ssh）

> 上游需求：AI/MCP 命令在终端 UI 同步执行——过程完整可见、可中断、可审批、可人工介入。
> 基线：PROTOCOL.zh-CN.md（v0.3.4）、PROGRESS-A-SSH（快速命令 PTY 写入语义先例）、
> mcp_safety 风险分级、host-key 挑战骨架（事件→RPC resolve）。
> 修订：2026-08-31 初版（设计已经用户确认：仅 MCP/AI 命令路由；分级审批；
> 无终端报错引导；超时返回部分输出命令继续跑）。

## 0. 目标与非目标

**目标**

1. DBX 内嵌 AI 通道（`dbx_call_plugin_tool` → `mcp/call`，带 lifecycle connectionId）
   的 `ssh_exec` / `ssh_exec_sudo` 可路由到**用户当前交互 shell**（PTY）执行：
   命令回显在终端、输出实时可见、AI 拿到捕获的输出文本。
2. 分级审批：连接级模式 `agentTerminalMode`（`off` 默认 / `auto` 分级 / `strict`
   全审）。`auto` 下低危直接执行（发 notice 事件），elevated（sudo 或灾难模式命中）
   弹审批；`strict` 全部审批。审批复用 host-key 挑战骨架（事件阻塞 → RPC resolve），
   超时默认拒绝。
3. 人工介入天然成立：执行走用户 shell——人可随时打字、Ctrl+C 打断（录制器见新
   提示符自然收尾）；sudo 场景终端内 auto-sudo 自动应答（既有状态机），无凭据时
   密码提示留给人工输入。
4. 超时语义：命令超时返回**已捕获输出** + `incomplete: true`，命令留在终端继续跑。

**非目标**

- stdio `--mcp` 独立进程模式不路由（与工作台不同进程，无 UI）——`runInTerminal`
  在该模式报错说明；内联凭据调用（无 lifecycle connectionId）同理不路由。
- 不做多会话择优：连接有多个存活终端时取 `session_id_for_connection` 返回的第一个。
- 不做精确 exitCode（无 shell 集成时为 `null`，靠输出文本判断）；不做回放审计存储。
- 不动快速命令栏/命令历史（人工路径现状保留）。

## 1. 模式与策略矩阵

`agentTerminalMode`：连接级，内存态（`SshRuntime` 按 connection_id 存
HashMap，sidecar 重启回默认 off，与 Quick Sudo 字段级覆盖同信任语义）。

| 模式 | low 风险 | elevated（sudo / mcp_safety 灾难命中） |
| --- | --- | --- |
| `off`（默认） | 走既有隐藏 exec 通道 | 同左 |
| `auto` | 直接注入终端（notice 事件） | 审批后注入 |
| `strict` | 审批后注入 | 审批后注入 |

调用级覆盖：`runInTerminal: true` 强制终端路径（仅内嵌通道；无终端会话报错）、
`false` 强制隐藏通道、缺省按连接模式。既有只读门禁/灾难确认（`confirmDestructive`）
在路由判定**之前**生效，模式不绕过任何安全门。

## 2. 一次路由的完整流程

```
ssh_exec{command, runInTerminal?}
  → 只读白名单/灾难确认门（现状不变）
  → 路由判定（§1 矩阵；Off 且 runInTerminal!=true → 原 exec_plain 路径，行为不变）
  → session_id_for_connection(connectionId)；Err → 报错
    "No open terminal session for this connection; open the SSH workbench terminal first"
  → Run：发 ssh/agent/notice → 注入
  → Prompt：发 ssh/agent/prompt（含 challengeId/timeoutSecs）→ 等待 resolve
    超时(120s，钳 10–300)/拒绝 → 返回错误给 AI（"user denied / approval timed out"），
    发 ssh/agent/finish{status:"denied"}
  → 注入：sanitize_command（剥 C0 控制字符，保留 \n \t）→ write_terminal(命令 + "\r")
    （与快速命令同语义：无 shell 拼接面；不用 bracketed-paste，多行按行执行为已知限制）
  → 录制：安装 session 级 TerminalRecorder → 读循环 observe 捕获 →
    终点 = 提示符回归（行尾 $/#，复用 exec.rs 判定）且静默 300ms；或超时
  → 发 ssh/agent/finish{status:"done"|"timeout"} → 返回 AI：
    { output(ANSI 剥离文本，尽力去回显行/尾提示符行), exitCode: null,
      mode: "terminal", incomplete: bool, interrupted: bool }
```

sudo + 终端路径：`ssh_exec_sudo{runInTerminal}` 不走 exec.rs 的 sudo 编排注入，
而是把 `sudo …` 原文注入用户 shell——密码/TOTP 应答交给终端 auto-sudo 状态机
（`quick_sudo` 开且有用时自动应答并出 `ssh/auto-sudo` 事件；否则密码提示显示在
终端，由人工输入，即人工介入）。elevated 风险在 `auto` 下必审。

## 3. 协议契约（钉死，双端共同遵守）

### 3.1 RPC / 设置

| 项 | 形态 |
| --- | --- |
| `ssh/settings/get` | 新增返回 `"agentTerminalMode": "off"\|"auto"\|"strict"` |
| `ssh/settings/set` | 新增可选 `agentTerminalMode`（非法值报错；缺省不改变） |
| `ssh/agent/resolve` | `{challengeId, decision: "approve"\|"deny", command?}` → `{success: true}`；challenge 未知/已过期报错。`command` 为审批弹窗编辑后的命令（approve 时生效） |

### 3.2 事件（camelCase，宿主 events 通道）

| 事件 | payload |
| --- | --- |
| `ssh/agent/prompt` | `{challengeId, sessionId, tool, command, risk: "low"\|"elevated", requestedAt: unixSecs, timeoutSecs}` |
| `ssh/agent/notice` | `{sessionId, tool, command, risk}` |
| `ssh/agent/finish` | `{sessionId, status: "done"\|"timeout"\|"denied"}` |

### 3.3 MCP 工具

`ssh_exec` / `ssh_exec_sudo` 新增可选 `runInTerminal: boolean`（schema 描述注明
仅 DBX 内嵌桥生效；stdio 模式传 true 报错）。内嵌桥响应在终端路径下附
`mode:"terminal"` / `incomplete` / `interrupted`；隐藏通道路径响应结构不变。

## 4. 后端组件

### 4.1 新模块 `agent_terminal.rs`（纯逻辑 + 单测，不连 SSH）

```rust
pub enum AgentTerminalMode { Off, Auto, Strict }   // parse()未知→Off；name()→canonical
pub enum CommandRisk { Low, Elevated }
pub enum RoutingDecision { Run, Prompt, Deny(&'static str) }
pub fn decide(mode: AgentTerminalMode, risk: CommandRisk) -> RoutingDecision  // §1 矩阵

pub fn sanitize_command(command: &str) -> Result<String, String>
// 剥 C0 控制字符（保留 \n \t； notably \x03 \x1b），trim 后空则报错

pub fn strip_ansi(text: &str) -> String            // CSI/OSC 序列剥离

pub enum RecorderState { Idle, Capturing, Settled }
pub struct TerminalRecorder { /* 1 MiB 有界缓冲；SILENCE_DEBOUNCE = 300ms */ }
impl TerminalRecorder {
    pub fn arm(&mut self) -> ();                   // 进入 Capturing
    pub fn observe(&mut self, chunk: &str) -> RecorderState;  // 读循环喂
    pub fn is_settled(&self) -> bool;              // 提示符见过 && 距末块 ≥300ms
    pub fn finish(&mut self) -> ();                // 强制收尾（deny/中断路径）
    pub fn take_output(&mut self, command: &str) -> String;
    // strip_ansi + 尽力剥离：首行若含命令回显片段则去、末行若为提示符行（行尾 $/#）则去
}
```

提示符判定复用 `exec.rs` 既有行尾 `$`/`#` 逻辑（现为私有则本地复制 4 行并注明）。
单测：decide 全矩阵、sanitize（\x03/\x1b 剥离、空拒绝）、strip_ansi、
recorder 状态机（echo→输出→提示符→静默 settle；超缓冲截断；finish 强制）、
take_output 回显/尾提示符剥离。

### 4.2 `ssh.rs`（SshRuntime 扩展）

- `agent_modes: Mutex<HashMap<String, AgentTerminalMode>>`（key=connection_id）+
  `agent_terminal_mode(&self, connection_id)`；`settings_get/set` 挂接新字段。
- 会话结构加 `agent_recorder: Arc<Mutex<Option<TerminalRecorder>>>`；PTY 读循环
  stdout 分支（auto_sudo.observe 同一位置）追加：
  `if let Some(rec) = &mut *session.agent_recorder.lock() { rec.observe(&chunk); }`
- `pub async fn exec_in_terminal(&self, session_id, command, timeout_secs,
  emitter) -> Result<Value, String>`：装 recorder → notice → write_terminal 注入 →
  50ms 轮询 is_settled/超时 → 卸 recorder → finish → 组响应（interrupted 恒 false，
  预留）。
- 审批挑战：`agent_challenges: Mutex<HashMap<String, oneshot::Sender<Decision>>>` +
  `request_agent_approval(...emitter) -> Result<String, String>`（返回批准后的命令；
  Err=denied/timeout）+ `pub fn resolve_agent_challenge(challenge_id, decision,
  command)`（供 main.rs `ssh/agent/resolve` 臂调用）。

### 4.3 `mcp.rs`

- `call_dbx` 增加 `emitter: PluginEmitter` 参数（main.rs 传入）；`call_tool` 透传
  Option；stdio `run_mcp_stdio` 路径传 None。
- `ssh_exec_tool`：既有只读/灾难门之后加路由分支（§1 矩阵 + runInTerminal 覆盖）。
- `tool_definitions()` 两工具 schema 加 `runInTerminal`。

### 4.4 `main.rs`（主会话接线）

- `mod agent_terminal;`
- `ssh/agent/resolve` 方法臂 → `resolve_agent_challenge`。
- `"mcp/call"` 臂把 `emitter.clone()` 传给 `call_dbx`。

## 5. 前端（App.vue + lib + i18n）

- `lib/agentTerminal.ts`：类型（AgentPrompt/AgentNotice/AgentFinish）、
  `AGENT_MODES`（off/auto/strict 顺序与 value）、`approvalRemainingSecs(payload)`
  纯函数 + `agentTerminal.spec.ts`。
- `handleEvent` 三分支：prompt→审批弹窗数据；notice→执行横幅；finish→清横幅。
- 审批弹窗（复用 host-key 弹窗骨架）：来源标签（AI 助手 · 工具名）、风险徽标
  （低危/提权）、**可编辑**命令 textarea（所见即所执行）、倒计时（payload
  timeoutSecs 起，到 0 自动收起弹窗）、批准/拒绝按钮；批准时提交编辑后命令。
- 执行横幅（terminal 工具栏区）：「AI 正在执行命令…」+ [中断] 按钮
  → `sendTerminalBytes(new Uint8Array([3]))`（复用既有 PTY 通道）。
- 设置弹窗 Quick Sudo 区块下新增「AI 终端同步」select 三档，随 `ssh/settings/set`
  保存；`SshSettings` 类型补 `agentTerminalMode`。
- i18n 七语新增键（对齐键集 parity 测试）：`agentTerminalSection, agentTerminalMode,
  agentTerminalOff, agentTerminalOffHint, agentTerminalAuto, agentTerminalAutoHint,
  agentTerminalStrict, agentTerminalStrictHint, agentPromptTitle, agentPromptSource,
  agentPromptRiskLow, agentPromptRiskElevated, agentPromptCommandLabel,
  agentPromptApprove, agentPromptDeny, agentPromptTimeoutHint, agentRunningBanner,
  agentInterrupt, agentFinished, agentDenied`。

## 6. 安全

- 注入信任域与快速命令一致（键盘写入原文，无 shell 拼接）；C0 剥离杜绝 AI 命令
  内嵌 `\x03`/`\x1b` 干扰终端与状态机。
- 审批默认拒绝（超时=拒绝）；弹窗展示完整命令原文。
- 只读白名单、灾难 `confirmDestructive`、`quick_sudo` 总开关、只读拒 sudo——
  全部先于路由判定生效，模式不放宽任何门禁。
- 挑战一次性：resolve 后即从 map 移除；重复 resolve 报错。

## 7. 测试计划（完成定义四件套）

1. **单测**（cargo test）：§4.1 全列；settings 字段 round-trip。
2. **前端**（pnpm typecheck + vitest）：agentTerminal.spec + i18n 七语键集 parity。
3. **smoke**（主会话补，未注册 SKIP 不 FAIL）：
   - smoke_mcp.py：stdio 模式 `ssh_exec{runInTerminal:true}` → 明确报错（负例）。
   - smoke_fs_test.py 增（sidecar_client 驱动内嵌 `mcp/call`）：settings_set auto →
     无终端会话报错引导 → 开会话 → low 危 echo 路由成功（响应 mode:"terminal"、
     输出含标记）→ strict 触发 prompt 事件后经 RPC `ssh/agent/resolve` approve →
     执行；deny → 报错含 denied。
4. **文档**：PROTOCOL（§3 契约）、MCP.zh-CN.md（runInTerminal/模式）、
   PROGRESS-P-SSH 收尾记录；manifest 0.3.4 → **0.4.0**。

## 8. 里程碑与分工

| # | 任务 | 负责 | 验收 |
| --- | --- | --- | --- |
| M1 | agent_terminal.rs（§4.1 全量 + 单测）+ main.rs 仅加 mod 行 | 后端 agent | cargo test 通过 |
| M2 | ssh.rs/mcp.rs 集成（§4.2/4.3） | 后端 agent | cargo test 通过 |
| M3 | 前端（§5 全量） | 前端 agent | typecheck + vitest 通过 |
| M4 | main.rs 接线（§4.4）+ smoke + 文档 + manifest | 主会话 | scripts/test.sh 全绿 |

并行划界：后端 agent 独占 backend/（main.rs 仅 mod 行）；前端 agent 独占 frontend/；
main.rs 方法注册与 smoke/文档归主会话，避免编辑冲突。**全程不做 git 提交**。

## 9. 风险与备注

- **全屏 TUI**（vim/top）：无提示符回归 → 走超时路径返回部分输出（incomplete:true），
  命令留终端人工接管——符合既定语义，文档记为已知限制。
- **多行命令**：按行依次执行（无 bracketed-paste），文档记为已知限制。
- **echo 剥离是尽力而为**：极端 shell 配置（无 echo/自定义 PS1）下输出可能含
  命令回显或提示符行，不影响正确性。
- **用户正在输入时注入**：与快速命令栏现状一致（接受）。
- approval 挑战与 host-key 挑战互相独立，不复用 `prompts` 结构（语义不同：
  host-key 是连接生命周期，agent 是命令级），仅复用「事件+resolve」交互骨架。
