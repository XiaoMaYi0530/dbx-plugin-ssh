# 实施计划：tssh「自动交互 Expect（终端触发器）」+「外部密码管理器」

日期：2026-09-15。来源：trzsz-ssh（tssh）对标补全——0.4.34 tssh 对标批
（FEATURE_PARITY「tssh 对标补充」节）遗留的最后两个未迁特性：
**Automated Interaction（Expect 系列）** 与 **PasswordCommand/PassphraseCommand**。
分支：`feat/ssh-trigger-auth`（worktree `.worktrees/feat-ssh-trigger-auth`），
双并发工作包实施，主会话收口。本文档是**双包唯一契约**，实施中不得静默偏离；
需变更先回主会话改本文档。

## 0. 目标与非目标

### 特性 A：自动交互 Expect（终端触发器）
- 用户在连接上配置**有序阶段规则**（对齐 tssh `ExpectPattern1..N`）：
  PTY 输出按序匹配正则，命中后自动回发预设输入（expect 式事件驱动脚本）。
- 每阶段应答三选一：明文 `sendText`（对齐 `ExpectSendText`，`\r` 显式）、
  密文引用 `sendSecretKey`（对齐 `ExpectSendPass`，走宿主 secret binding，
  自动补 `\r`）、本地命令 `sendCommand`（对齐 `ExpectSendOtp`，取 stdout 回发）。
- 可选阶段前置匹配 `casePattern` + `caseSendText`/`caseSendSecretKey`
  （对齐 `ExpectCaseSendText?/ExpectCaseSendPass?`，如 yes/no 自动答 y；
  case 命中不推进阶段，答完继续等本阶段 pattern）。
- 阶段超时 `timeoutSecs`（对齐 `ExpectTimeout`，默认 30s）；发送分段延迟
  `sleepMs`（对齐 `ExpectSleepMS`，`\|` 分段）与 `passSleep`
  （对齐 `ExpectPassSleep`，none|each|enter，仅作用于密文/命令类应答）。

非目标：tssh `--enc-secret` 自有加密格式（宿主 secret binding 覆盖）；
`ExpectSendEncTotp`（同前）；per-rule once 标志（tssh 亦无，shell prompt
复位即可重复触发）；exec/命令通道不接入（仅 PTY 终端会话）。

### 特性 B：外部密码管理器
- 连接可配置 `passwordCommand` / `passphraseCommand`（对齐 tssh 同名配置）：
  登录密码 / 私钥口令缺失时在**本地**运行命令取回（gopass、1Password CLI、
  `oathtool` 等），占位符 `%h` host、`%u` username、`%p` port、`%n` 连接名、
  `%%` 字面 `%`。
- 优先级：既有显式凭据（宿主 secret binding / 表单密码）> 命令（对齐
  tssh「加密 > 命令 > 明文」的插件侧映射）。

非目标：不新增任何默认命令；不在 MCP 工具描述里推广命令执行；命令输出
不落日志、用后 zeroize。

## 1. 决策记录（实施中不得静默偏离）

| # | 决策 | 理由 |
| --- | --- | --- |
| D1 | 触发器引擎放 sidecar 读循环观察者链，**先于** `TerminalAutoSudo`；某 chunk 被 trigger 应答则该 chunk 跳过 auto-sudo，反之亦然（互斥防双答） | sidecar 拥有读写双向句柄，覆盖人工/batchInput/MCP 全部输入源；前端实现拿不到旁路写回 |
| D2 | 匹配缓冲：`normalize_auth_prompt_text` 归一化后维护 ≤8 KiB 滚动缓冲，跨 chunk 匹配；命中后消费已匹配文本段 | 提示可能被 TCP 分片截断；复用既有归一化剥 ANSI |
| D3 | 复位语义：`has_shell_prompt`（exec.rs）命中即阶段游标归零（一轮结束后可再次触发）；阶段超时亦归零并发 `timeout` 事件 | 对齐 tssh「登录序列结束后 expect 重新武装」；超时后会话继续不断连 |
| D4 | `sendCommand`/`passwordCommand` 经 shell 执行（unix `sh -c`，windows `cmd /C`），10s 超时，stdout 去掉**单个**结尾换行后为应答；命令源自用户自己的连接配置，文档声明风险 | 对齐 tssh 语义；限定执行面（无默认命令、无通配注入点） |
| D5 | 密文槽位 v1 固定两个：secrets 键 `trigger_answer_1` / `trigger_answer_2`（manifest 表单各一个 password 字段）；`sendSecretKey` 值即该键名 | manifest secret 字段是宿主 secret binding 的入口；两槽覆盖 2FA 主流场景 |
| D6 | `ssh/trigger` 事件 payload `{sessionId, stage(1-based), kind: "text"|"secret"|"command"|"timeout"}`，**永不携带应答内容** | 对齐 `ssh/auto-sudo` 事件口径；密文不出 sidecar |
| D7 | 校验语义：非法 JSON/正则/字段组合 → 连接失败并报明确错误（仿 set_env 严格校验），不静默降级 | 错配静默失效比失败更难排查 |
| D8 | stages 上限 16、pattern/casePattern ≤512 字符、sleepMs ≤5000 | 防配置滥用；512 对齐 `MAX_PROMPT_HINT_LEN` |
| D9 | passwordCommand 解析点：`dial_and_authenticate` orchestration 构建前一次性解析，结果回填供密码链与 sudo 编排共用；passphraseCommand 在私钥口令解析处接入 | 一处解析、登录+sudo 都生效；避免多处执行命令 |

## 2. 契约（双包共享，命名即冻结）

### 2.1 存储配置（`StoredConnection` ← `connection.external_config`，snake_case）

```json
{
  "triggers": {
    "timeoutSecs": 30,
    "sleepMs": 100,
    "passSleep": "none",
    "stages": [
      {
        "pattern": "(?i)verification code",
        "sendText": "654321\\r",
        "sendSecretKey": "trigger_answer_1",
        "sendCommand": "oathtool --totp -b %h",
        "casePattern": "\\(yes/no\\)",
        "caseSendText": "y",
        "caseSendSecretKey": "trigger_answer_2"
      }
    ]
  },
  "password_command": "op read 'op://vault/item/password'",
  "passphrase_command": "security find-generic-password -s %h -w"
}
```

- `triggers` 缺省/stages 空 = 功能关闭。`timeoutSecs` 1..=600、`sleepMs`
  0..=5000、`passSleep` ∈ none|each|enter、`stages` 1..=16。
- 每阶段 `pattern` 必填；应答 `sendText`/`sendSecretKey`/`sendCommand`
  **三选一**；case 组可选，`caseSendText`/`caseSendSecretKey` 二选一。
- `sendText` 转义：两字符序列 `\r` `\n` `\t` 解释为控制字符，`\|` 为分段
  停顿符（段间停 `sleepMs`），其余反斜杠原样；密文/命令应答自动补 `\r`。
- `connection_secrets` 新增键：`trigger_answer_1`、`trigger_answer_2`。
- `pattern`/`casePattern` 为 Rust `regex` crate 语法（支持 `(?i)`），
  编译失败即连接失败（D7）。

### 2.2 manifest 连接表单字段（包 B，key 冻结）

| key | type | binding | 说明 |
| --- | --- | --- | --- |
| `triggers` | textarea | config | placeholder 给 2.1 的 stages 最小示例；description 含恶意服务器伪提示风险提示 |
| `trigger_answer_1` | password | secret | 触发器密文槽 1（`sendSecretKey` 引用） |
| `trigger_answer_2` | password | secret | 触发器密文槽 2 |
| `password_command` | text | config | 登录密码命令；description 含占位符说明与风险提示 |
| `passphrase_command` | text | config | 私钥口令命令；同上 |

只用既有 6 种字段 type，不加 visible_when 链（textarea 内容无法作条件），
字段放 `set_env` / `private_key_passphrase` 邻近；七语 description 必须全补。

### 2.3 MCP 内联拨号参数（包 A，camelCase）

`stored_connection_from_arguments` 新增：`triggers`（JSON 字符串，同 2.1
schema）、`passwordCommand`、`passphraseCommand`（字符串）。解析/校验与存储
路径同一套代码；非法即拨号报错。

### 2.4 事件（包 A 实现，包 B 消费）

`ssh/trigger`：`{ sessionId, stage, kind }`，kind ∈ `text|secret|command|timeout`。
见 D6。前端在 App.vue `handleEvent` 加分支：命中（非 timeout）在对应终端
toast「自动交互已应答（阶段 N）」；timeout 提示「自动交互阶段 N 超时已复位」。
七语文案。

## 3. 工作包划分（文件所有权不相交，禁止越界）

| 文件 | 包 A（后端 Rust） | 包 B（前端/契约/测试） |
| --- | --- | --- |
| `backend/**`（含 Cargo.toml/lock） | ✅ 独占 | ❌ |
| `docs/PROTOCOL.zh-CN.md`、`docs/FEATURE_PARITY.zh-CN.md` | ✅ 独占 | ❌ |
| `manifest.json`、`frontend/**` | ❌ | ✅ 独占 |
| `scripts/**` | ❌ | ✅ 独占 |
| `docs/PROGRESS-P-SSH.zh-CN.md`、版本号 bump | 主会话收口 | 同左 |

### 3.1 包 A 任务清单

1. `backend/Cargo.toml` 加 `regex = "1"`；Cargo.lock 用 CLI 同款 patch 参数
   regenerate（`[patch.crates-io] dbx-plugin-sdk = { path = "../shared/sdk/rust/dbx-plugin-sdk" }`
   必须保留；lockfile `--locked` 打包坑见 skill 常见故障表）。
2. 新模块 `backend/src/triggers.rs`：
   - `TriggersConfig`/`TriggerStage` 解析与严格校验（serde + 手工校验，D7/D8）；
   - `TriggerEngine`：滚动缓冲（D2）、阶段游标、case 预匹配、超时（秒级，
     now 由调用方注入便于测试）、复位（D3）；
   - `observe(&mut self, chunk, now) -> Option<TriggerDecision>`；
     `TriggerDecision` = `{ stage, kind, segments: Vec<(Vec<u8>, delay_ms)> }`
     （分段发送计划，读循环负责 `tokio::sleep` 分段写回 channel）；
   - 命令执行独立函数：占位符替换 → shell 执行 → 超时 → trim → String
     （zeroize 友好）；执行器做成可注入（trait 或函数指针）以便单测。
3. 单测先行（TDD）：匹配/游标推进/case 不推进游标/超时复位/prompt 复位/
   跨 chunk 缓冲/`\|` 分段与 passSleep 语义/转义解析/三选一与上限校验/
   命令执行（echo + sleep 超时用例）。全部纯逻辑不连 SSH。
4. `backend/src/ssh.rs` 读循环 `:1045` 观察者链接入（D1，先于 auto_sudo）；
   挂载/卸载仿 `sync_auto_sudo`（:3368）——连接带 triggers 配置即挂载，
   运行期配置更新同款刷新；命中发 `ssh/trigger` 事件（D6），分段写回用
   `tokio::time::sleep`。
5. `backend/src/ssh.rs` 认证区（D9，`:1390` orchestration 构建前）解析
   `password_command`；私钥口令处接 `passphrase_command`；eprintln 日志只记
   「命令已执行/失败退出码」，不落输出。
6. `backend/src/model.rs`：`StoredConnection` 加 `triggers`/
   `password_command`/`passphrase_command` 字段 + `from_lifecycle_params`
   解析 + 既有「已知 key」清单各登记点（约 :223/:358/:458/:864/:971，以
   实际代码为准）；非法配置返回明确错误。
7. `backend/src/mcp.rs` `stored_connection_from_arguments`（:3693）补
   2.3 三参数。
8. `docs/PROTOCOL.zh-CN.md`：external_config 新键、`connection_secrets` 新键、
   `ssh/trigger` 事件、MCP 参数、**安全声明**（恶意服务器可伪造匹配提示
   骗取回发内容——密文仅用于明确配置的 pattern；本地命令执行面说明）。
   `docs/FEATURE_PARITY.zh-CN.md`「tssh 对标补充」表补两行：
   「自动交互（Expect 系列）✅」「外部密码管理器（PasswordCommand/PassphraseCommand）✅」，
   注明差异（密文走宿主 secret binding 而非 --enc-secret）。
9. 验证：`cargo fmt`、`cargo clippy --all-targets`、`cargo test` 全绿。

### 3.2 包 B 任务清单

1. `manifest.json` 加 2.2 五个字段（式样仿 `set_env` :352 / `private_key`
   :152 / `sudo_password` :216；七语 description；`triggers` placeholder 给
   最小可用 stages 示例 JSON）。
2. `scripts/connection-forms/verify.mjs`：新增字段过 condition evaluator
   的场景用例并转绿（无 visible_when 也要过既有 schema/重复 key 检查）。
3. `frontend/src/App.vue` `handleEvent`（:1801 起）加 `ssh/trigger` 分支 →
   终端 toast（样式仿既有 `ssh/agent/prompt` 提示分支）；toast 需定位到
   对应 sessionId 的会话。
4. `frontend/src/lib/i18n.ts`：新增文案七语全补（zh-CN/zh-TW/en/es/it/ja/pt）：
   toast 两条 + 表单相关（若前端有引用）。`pnpm test`/typecheck 转绿。
5. `scripts/smoke_trigger_test.py`（仿 `smoke_sudo_otp_test.py` 结构）：
   - 容器 `dbx-ssh-test` 不在则整档 SKIP（与 test.sh live 段门控一致）；
   - 远端装 POSIX shim：打印 `Verification code:` → 读一行 → 把收到的行
     append 到日志文件 → 打印 shell 提示；断言 sidecar 按配置自动回发
     （多阶段：阶段 1 密码类提示答 `trigger_answer_1` 密文槽、阶段 2
     `sendText`），并断言 `ssh/trigger` 事件（若 sidecar_client.py 支持
     事件订阅；不支持则以远端日志为准）；
   - **只在包 A 落地后可跑**；开发期不执行，最终由主会话跑。
6. `scripts/test.sh` live smoke 段（:69-70 附近）注册
   `smoke_trigger_test.py`。
7. 验证：`pnpm test`、`pnpm typecheck`、`node scripts/connection-forms/verify.mjs`
   全绿（smoke 脚本语法检查 `python3 -m py_compile`）。

## 4. 验证基线与收口（主会话）

- 基线（已验）：worktree 基于 ab99f31，cargo test 405 通过、pnpm test 402 通过。
- 收口：cargo fmt/clippy/test → `scripts/test.sh`（容器在跑 live smoke，
  无 host worktree 用 `--skip-host` 并记录剩余风险）→ 打包 →
  `scripts/install.sh --reinstall` → 双冒烟（`DBX_PLUGIN_SIDECAR` 指向安装副本）。
- PROGRESS-P-SSH.zh-CN.md 补同日章节；版本 bump 0.4.74
  （manifest.json `version` + backend/Cargo.toml `version`，主会话统一改避免
  双包冲突）。
- 硬性约定三件套：单测 + smoke + 对标清单状态更新，缺一不算完成。

## 5. 合并注意（沿用 0.4.34 教训）

- 主检出区另有未提交的私钥录入改动（keys/model/mcp/ssh/manifest/App.vue/
  i18n 等 15 文件）**不在本分支**；两分支合并时 model.rs、mcp.rs、
  manifest.json、App.vue、i18n.ts 有局部冲突面，合并顺序由用户决定。
- worktree 内如出现 `host` 符号链接（构建产物）勿提交。
- 双包全程**不做 git commit**，改动留在工作区由主会话统一验证后交用户决定提交。
