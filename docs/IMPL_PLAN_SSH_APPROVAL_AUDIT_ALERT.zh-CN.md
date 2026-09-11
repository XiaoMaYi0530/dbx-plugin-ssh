# 实施计划：审批记忆与执行审计（#4）+ 告警分诊（#5）

日期：2026-09-11。来源：openocta/openocta 对比评审（评审结论见会话记录；
对标点：`src/pkg/security/approval_queue.go` 持久化审批队列、`deploy/scenarios/
host-inspection/scenario.json` 巡检场景、`docs/webhooks.md` `/hooks/alert` 告警标准化）。

两个特性均为**纯插件侧**改动：无宿主改动（硬性规则 8）、无新依赖、无协议破坏性变更。

## 0. 目标与非目标

### #4 审批记忆与执行审计
- **4a 记住批准**：AI 终端审批弹窗新增「记住此命令」——批准时把命令（用户编辑后的
  最终文本）存入本连接的免审批清单；后续同一命令（含手工泛化的通配形态）命中清单
  即直接执行，不再弹审批。
- **4b 执行审计**：MCP/AI 执行面全量落一份本地 JSONL 审计账（谁、何时、什么工具、
  什么命令、门禁判定、审批结论、退出码、耗时），并提供 `ssh/audit/list` 只读回放。

非目标（本期明确不做）：
- 不自动写回宿主管理的连接 `sudo_whitelist`（external_config 归宿主管，插件侧以
  独立清单叠加，语义见 D3）；
- 工作台人工操作（手敲终端、SFTP、sudo 文件面板）不入审计——与 sudo 白名单的
  信任模型一致（白名单/审计管 AI/MCP 执行面）；
- 审计不做 Web 展示面板（`ssh/audit/list` + 后续可选 UI 延期）。

### #5 告警分诊
- 把异构告警 JSON 标准化为固定结构（对齐 openocta `/hooks/alert` 兼容语义），按
  关键词分类（CPU/内存/磁盘/inode/网络/OOM/服务），并给出**经过安全门验证的只读
  诊断命令清单**；清单可在工作台一键发送到终端，或交由 MCP 调用方经既有 exec 门
  执行。
- 定位刻意收窄：插件不做 LLM 分析（插件是工具面不是 Agent）；分诊 = 结构化 +
  分类 + 白名单级命令建议，推理交给外部调用方（ZCode 等）。

非目标：不接收外部 webhook（超出宿主契约）；不做定时巡检（另有对标项 #1）。

## 1. 决策记录（实施中不得静默偏离）

| # | 决策 | 理由 |
| --- | --- | --- |
| D1 | 记住清单把 `Prompt` 降为 `Run`，**不覆盖 `Deny`**；off 模式未显式 `runInTerminal` 的提权命令仍拒绝 | Deny 是模式级安全语义（用户没选择可见终端），记住是单命令级豁免，不应放大 |
| D2 | `assess_command == Destructive` 的命令**永不入清单、命中清单也无效**；灾难门永远是最后一道 | 与既有四层门序一致，记住不能绕过灾难确认 |
| D3 | 记住清单匹配**完整复用 `sudo_allowlist` 语义**（token 精确 / `*` 一个参数 / 尾 `*` ≥1 参数），存储为原始行，匹配走 `entries_from_lines + is_allowed` | 零新增匹配代码、一套心智模型；记住时存精确形态，用户可在设置里手工泛化为通配 |
| D4 | 清单按 connectionId 键控，独立存储文件 `agent-approved-commands.json`（照 `agent-modes.json`：无凭据、普通 JSON、tmp+rename 原子写、坏文件回退空） | 连接级作用域与 agent 模式同构；命令文本一般不含凭据，不强制 0600 |
| D5 | 审计只记 MCP/AI 执行面 + 审批生命周期；跨进程（embedded sidecar 与 stdio `--mcp`）共享同一数据目录 → **每次写入 open-append 单行-关**，5 MiB 轮转保留一代（`.1`） | O_APPEND 单行写跨进程安全；低频人工量级，性能足够 |
| D6 | 分诊 playbook 的每条命令**必须通过 `mcp_safety` 白名单（单测钉死）**，且 playbook 命令不含重定向/`$(...)`/`sudo` | 保证建议命令在只读连接上可被自家门直接放行，AI 拿到即可执行 |
| D7 | `ssh/alert/triage` 无需活动连接（纯分诊）；`ssh_alert_triage` 是非写工具 | 分诊与执行解耦；执行仍走既有四层门 |

## 2. 协议契约（PROTOCOL.zh-CN.md 同步内容）

全部 camelCase；`<域>/<动作>` 命名；均为**追加**，存量方法不变。

### 2.1 `ssh/agent/resolve`（扩展，向后兼容）
- params 增可选 `remember: boolean`（仅 `decision: "approve"` 时有效）。
- `approve + remember=true`：把**用户编辑后的最终命令文本**写入该连接记住清单
  （D2 拦截：Destructive 命令忽略 remember，行为等同普通 approve）。
- 返回结构不变。

### 2.2 `ssh/settings/get|set`（扩展）
- `get` 增 `rememberedCommands: string[]`（本连接记住清单原始行，配置顺序）。
- `set` 接受 `rememberedCommands`（全量替换）：每行 ≤500 字符、每连接 ≤50 行、
  去重；含 Destructive 命令的行拒绝（错误信息指明行号）。

### 2.3 `ssh/audit/list`（新增）
```jsonc
// 请求
{ "limit": 100, "beforeTs": 1730000000000 }   // 均可选；limit ∈ [1,500] 缺省 100
// 响应
{ "entries": [ { "tsMs": 0, "tool": "ssh_exec", "connectionId": "…",
    "gate": "pass", "approval": "remembered", "outcome": "ok",
    "exitCode": 0, "durationMs": 123, "mode": "terminal", "error": null } ],
  "truncated": false }
```
- `gate ∈ pass | write-denied | whitelist-denied | sensitive-path |
  destructive-unconfirmed | sudo-allowlist-denied | read-only-server`（把现有
  call_tool 门序的各拒绝分支归纳为枚举，**不改门序逻辑**）。
- `approval ∈ none | prompt | approved | denied | timeout | remembered`。
- 只读方法；不涉及凭据字段（凭据从不进入命令文本）。

### 2.4 `ssh/alert/triage`（新增，无需连接）
```jsonc
// 请求
{ "payload": "{\"title\":\"CPU 使用率过高\",\"severity\":\"critical\",…}" }
// payload 为原始文本：合法 JSON 按结构解析；解析失败或 message 为空时
// 整包文本作为 message（对齐 openocta /hooks/alert 兼容语义）
// 响应
{ "normalized": { "alertId": "", "title": "…", "message": "…",
    "severity": "critical", "source": "prometheus", "dataJson": "{…}" },
  "category": "cpu",            // cpu|memory|disk|inode|network|oom|service|generic
  "suggestions": [ { "command": "uptime", "purposeKey": "loadSnapshot" }, … ] }
```
- `purposeKey` 为稳定 key，展示文案走前端 i18n 七语。

### 2.5 MCP 新工具 `ssh_alert_triage`
- inputSchema：`{ payload: string }`；非写工具（`is_write_tool=false`）、无
  `connectionId`。
- 工具描述注明：返回只读诊断清单，执行请走 `ssh_exec`（门禁照常）。

## 3. 实施拆分（工作包与文件所有权）

接线惯例沿用 FEATURE_PARITY 第一批约定：新模块纯函数包可并发实施；
`main.rs` / `mcp.rs` / `ssh.rs` / `App.vue` 等热点文件接线由主会话统一完成。

### WP-A 审批记忆（后端）
- 新建 `backend/src/agent_approvals.rs`：
  - `load_store/save_store`（照 `agent_terminal::load_modes/save_modes` 模式，
    结构 `{version:1, connections:{<connectionId>:{commands:[line,…]}}}`）；
  - `remember / forget / list_lines / matches`——匹配直接复用
    `sudo_allowlist::{entries_from_lines, is_allowed}`（D3）；
  - 上限 50 行/连接、行长 500、去重；单测：remember→matches、去重、上限、
    Destructive 拒绝入库、坏文件回退、roundtrip。
- 接线（主会话）：
  - `main.rs` `ssh/agent/resolve` 臂（~main.rs:394）透传 `remember`；
  - `ssh.rs` `AgentDecision::Approve` 增 `remember: bool`（agent_terminal.rs:180）、
    `resolve_agent_challenge` 批准路径落库（需 session→connectionId，已有）；
  - `agent_terminal.rs` 新增纯函数 `decide_with_memory(mode, risk, explicit,
    remembered) -> RoutingDecision`（Prompt→Run、Deny 不变，D1）+ 矩阵单测；
  - `mcp.rs ssh_exec_terminal_tool`（mcp.rs:1267 前）：`remembered =
    assess!=Destructive && agent_approvals::matches(...)` 后改调
    `decide_with_memory`；
  - `ssh.rs settings_get/settings_set`（ssh.rs:2592/2652）增
    `rememberedCommands`。

### WP-B 执行审计（后端）
- 新建 `backend/src/audit_log.rs`：
  - `AuditEntry` 结构 + `append`（open-append 单行、5 MiB 轮转 `.1`、进程内
    `Mutex` 串行）+ `tail(limit, before_ts)`；
  - 字段照 §2.3；serde 序列化即完成换行转义；
  - 单测：append/tail、轮转、跨实例追加、字段完整性。
- 接线（主会话）：
  - `mcp.rs call_tool`（mcp.rs:412）外层包裹：起点计时 → 既有门序/run_tool →
    单条 append（gate 枚举由现有拒绝分支归纳）；
  - `ssh.rs` `request_agent_approval`（超时=denied/timeout）与
    `resolve_agent_challenge`（approved/denied/remembered）各追加一条审批审计；
  - `main.rs` 注册 `ssh/audit/list`。

### WP-C 告警分诊（后端）
- 新建 `backend/src/alert_triage.rs`（纯函数，无 I/O）：
  - `normalize(payload: &str) -> Normalized`（兼容语义见 §2.4；字符串字段长度
    钳制：title/message ≤2 KiB、dataJson ≤16 KiB）；
  - `classify(&Normalized) -> Category`（title+message+dataJson 双语关键词评分，
    平票落 generic）；
  - `playbook(Category, &Normalized) -> Vec<Suggestion>`：静态命令表初稿——
    cpu: `uptime`、`top -b -n 1 | head -20`、`ps aux --sort=-%cpu | head -15`、
    `vmstat 1 3`；memory: `free -m`、`ps aux --sort=-%mem | head -15`；disk:
    `df -h`、`du -x -d 1 / | sort -rh | head -15`；inode: `df -i`；network:
    `ss -s`、`ip -s link`；oom: `dmesg -T | tail -100`、
    `journalctl -k -n 200 --no-pager`；service: `systemctl status <svc>`、
    `journalctl -u <svc> -n 100 --no-pager`（svc 从 data/message 提取，取不到
    则省略该组）；generic: `uptime`、`df -h`、`free -m`、`ps aux --sort=-%cpu | head -10`。
    **约束（D6）：每条命令单测断言 `mcp_safety::assess_command == ReadOnly`；
    实施时以该测试为准裁剪/替换命令（禁重定向、禁 `sudo`、禁 `$()`）**；
  - 单测：normalize 三态（结构/劣化/超长钳制）、分类正反例、playbook 白名单
    一致性、purposeKey 稳定性。
- 接线（主会话）：`main.rs` 注册 `ssh/alert/triage`；`mcp.rs` 注册工具 + schema
  + 非写归类。

### WP-D 前端
- `frontend/src/lib/agentTerminal.ts` 扩展：`AgentPromptPayload` 不变；新增
  remember 相关纯函数（resolve 请求体构造、设置面板列表归一
  `sanitizeRememberedCommands`）+ spec。
- `frontend/src/lib/alertTriage.ts`（新建）：`severityClass`、payload 非空校验、
  `purposeKey` → i18n key 映射 + spec。
- `App.vue`（主会话接线）：
  - 审批弹窗（复用 host-key 骨架）新增「记住此命令」checkbox（默认不勾），
    resolve 携带 `remember`；
  - 设置弹窗「AI 终端」区块增「已记住命令」列表（展示/删除，走
    `ssh/settings/set rememberedCommands`）；
  - 工具栏新增「告警排查」按钮（Siren 图标）→ 弹窗：粘贴 textarea → 分析 →
    摘要 chips（severity 着色）+ 分类 + 命令列表，每行「发送到终端」（复用
    既有 PTY 键盘写入链路；无活动会话时禁用）+「全部复制」。
- i18n 七语（en/es/it/ja/pt-BR/zh-CN/zh-TW）：
  - `approval.remember`（~1 key）、`settingsRemembered.*`（~4）、
    `alertTriage.*`（~12：title/placeholder/analyze/category.*（8 分类）/
    sendToTerminal/copyAll/emptyResult/noSession）；vitest 七语 key +
    占位符对齐断言自动覆盖。

### WP-E smoke + 文档 + 收口
- `scripts/smoke_fs_test.py`：
  - agent 组扩展：strict/auto 下 approve+remember → 同命令第二次**零弹窗**直跑
    （事件回调驱动断言无 `ssh/agent/prompt`）；Destructive 命令 remember 仍拦截；
    deny+remember 忽略；
  - settings `rememberedCommands` get/set round-trip + 非法行拒绝；
  - `ssh/alert/triage`：结构化 JSON / 劣化文本 / 分类断言 / suggestions 形状；
  - `ssh/audit/list`：执行若干工具后 tail 非空、gate/approval 字段断言、limit。
- `scripts/smoke_mcp.py`：`ssh_alert_triage` schema + 离线调用；审计对 stdio
  路径生效的冒烟断言（调用后数据目录 audit 文件增长）。
- 文档：PROTOCOL（§2 新节「审批记忆与执行审计」「告警分诊」+ RPC 表 3 行 +
  settings 字段）；`MCP.zh-CN.md`（工具一览 27→28 + 告警→排查组合用法段 +
  审计语义）；`FEATURE_PARITY.zh-CN.md`（openocta 对标行 ×2：持久化审批、
  告警标准化分诊）；本任务收尾更新 `PROGRESS-P-SSH.zh-CN.md`。
- 版本：`manifest.json` + `backend/Cargo.toml` 0.4.51 → 0.4.52（如与其他轮合流
  由收口轮统一递增）。

## 4. 验证与完成定义（四件套映射）

| 件 | 内容 |
| --- | --- |
| 单测 | cargo：agent_approvals（6+）、decide_with_memory 矩阵（4）、audit_log（4+）、alert_triage（8+，含 D6 白名单一致性）、settings rememberedCommands（2）；vitest：agentTerminal/alertTriage 新增 + 七语对齐 |
| smoke | smoke_fs 扩展组（≈10 用例）+ smoke_mcp 扩展；按层先跑改动层，交付前 `scripts/test.sh`（历史 smoke_sudo_otp 预存在问题与本轮无关，按 SKIP/预存在语义处理并记录） |
| 对标/清单 | FEATURE_PARITY 两行 + 本文件勾选状态 |
| 七语 | §WP-D key 清单 ×7，vitest 对齐断言 |

前端界面改动按惯例做 visual.html 浏览器验证（审批 checkbox、记住列表、告警
弹窗全流程 + 深浅两态截图，截图入 `docs/screenshots-ui-mock/`，不入库 png 已被
gitignore 覆盖的按仓规处理）。

## 5. 风险与边界

1. **匹配语义偏严**：token 精确匹配对参数变化的命令命中率低（如路径带时间戳），
   属保守方向的刻意取舍——设置面板可手工泛化为通配；不做自动泛化。
2. **审计完整性**：进程崩溃窗口内最后一条可能缺失（append-per-call 已最小化）；
   embedded 与 stdio 两进程并发写靠 O_APPEND 单行原子性，极端长行（>4 KiB）理论
   可撕裂——命令文本已钳制（500 字符），error 文本钳制 1 KiB。
3. **分诊分类是关键词启发式**，不承诺准确率；generic 兜底组保证总有可执行建议。
4. **记住清单作用域**：连接级、明文 JSON（无凭据，D4）；若未来清单里出现敏感
   命令文本，可平移到 sudo-profile 式 0600 存储，模块边界已收敛。
5. stdio `--mcp` 模式与工作台不同进程：记住清单与审计文件经共享数据目录天然
   打通（与 agent-modes.json 同机制），无额外同步逻辑。

## 6. 实施顺序建议

1. WP-A（独立小包，先落 `decide_with_memory` 纯函数与存储模块）；
2. WP-C（纯函数模块，与 WP-A 可并行）；
3. WP-B（依赖对 call_tool 门序的通读，接线量最大，单独一轮）；
4. WP-D 前端（依赖 A/C 契约钉死）；WP-E 收口。
