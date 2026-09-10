# P-SSH 路交付报告（ssh-sftp 成熟化增强）

日期：2026-08-28 深夜。工作目录：`~/btroot/dbx-plugins/ssh-sftp`，在未提交 batch3 + 历轮（S-A/S-B/X/XB）改动之上继续；
仅触碰本路所有权文件：`backend/src/exec.rs`、`backend/src/ssh.rs`、`frontend/src/App.vue`、
`frontend/src/style.css`、`frontend/src/lib/{terminalReconnect,terminalCommandMarkers,sftpBatchProgress,workbench.spec,i18n}.*`、
`scripts/smoke_sudo_otp_test.py`、`scripts/perf_baseline_test.py`（新建）、`docs/PROGRESS-P-SSH.zh-CN.md`（本文件，新建）。
未执行任何 git commit/push；无新 npm/cargo 依赖。

## 0. 总体验证基线（本轮终值）

| 套件 | 结果 |
| --- | --- |
| backend `cargo test` | ✅ **108 passed / 0 failed**（基线 107 → 108，新增 ReplayBuffer 5 MiB 压测单测） |
| frontend `pnpm typecheck` | ✅ vue-tsc --noEmit exit 0 |
| frontend `pnpm test` | ✅ vitest 2 spec **41/41**（基线 38 → 41：重连倒计时 1 + 标记条 tooltip 1 + 批量进度 1） |
| frontend `pnpm build` | ✅ 自包含 `ui/index.html`（2,297,959 B）产出 |
| `scripts/smoke_test.py` | ✅ PASS（1.1s，真机容器） |
| `scripts/smoke_fs_test.py` | ✅ PASS 17 / SKIP 0 / FAIL 0 |
| `scripts/smoke_mcp.py` | ✅ initialize / tools/list 19 tools / call 往返全绿 |
| `scripts/smoke_batch3_test.py` | ✅ 17 passed / 0 skipped / 0 failed |
| `scripts/smoke_sudo_otp_test.py` | ✅ **10 passed / 0 skipped / 0 failed**（XB 遗留项第 5 关单） |
| `scripts/perf_baseline_test.py` | ✅ 3 案例 0 SKIP（真机，见 §3 性能基线） |
| `dbx-plugin package .` | ✅ `dist/io.dbx.ssh-0.2.2-darwin-arm64.dbxp`（4,276,701 B，sha256 `3c2183bf8e57cae4ca1ca48ee4aba3df4aa59f9e46631565001cabb8aed415e2`，5 文件齐） |

测试容器：`dbx-ssh-test`（linuxserver/openssh-server，127.0.0.1:2222），全部 smoke 真机真跑、无环境 SKIP。
出包后对同源码快照二进制复跑 `smoke_test.py` + `smoke_sudo_otp_test.py` 双确认（SKILL.md 流程约定）。

## 1. 任务 1：sudo 保活 / OTP 防重放端到端（XB 遗留第 5 项）

宿主 python3 标准库实现 RFC 6238（HMAC-SHA1，`scripts/smoke_sudo_otp_test.py: totp_now`）；
容器凭据运行时 `chpasswd` 生成（digits+lower+upper+symbol），TOTP secret 运行时 `secrets.token_bytes(20)` 生成、
只写“当窗期望码”入容器、secret 本体永不出宿主；退出时完整恢复（sudoers 备份/密码/shim/OTP 状态文件）。

**验证链（10 用例全绿真机）**：
1. 正确密码 Quick Sudo 执行成功（`sudo -S` 密码注入）；
2. `ssh/sessions/list` 报告 `sudoKeepalive=true`（`sudo -nv` 4 分钟保活循环已注册）；
3. 全局时间戳缓存使后续 sudo 无 prompt（配置错密码仍成功，证明未发生认证交互）；
4. `sudo -k` 后错密码以 `sorry, try again` 报错；恢复正确密码后再执行成功；
5. `ssh/settings/set` 注册多密钥 `totpSecret`（`a;b`），`ssh/settings/get` 只报布尔不回显 secret；
6. 当前窗 OTP 被自动应答并被服务端 ACCEPT（提交日志逐条核对该码）；
7. 同窗二次 sudo：轮换到第二密钥的码（提交日志证明未重放第一码），服务端拒绝（防重放语义成立）；
8. 同窗第三次：硬跳过 OTP 提交（提交日志零新码）；
9. 错误 TOTP secret 提交其派生码并被拒绝。

**容器策略**：Alpine 的 sudo 无 PAM，无法挂真实 TOTP 模块——用 `/usr/local/bin/sudo` POSIX shim
模拟 OTP-prompt 前端：无 flag 文件时纯透传真实 sudo；OTP 相态吃掉 stdin 密码行、打印
`Verification code:`、与宿主写入的期望码比对、逐条记录 `<epoch> <code> ACCEPT|REJECT`，
再委托真实 sudo 完成真实提权。secret 不进容器，仅瞬态期望码。

**顺带修复的两个真实缺陷**：
1. **smoke shim 参数剥离**（脚本缺陷）：shim 转发真实 sudo 前把 `-S`/`-p` 剥掉了，导致真实 sudo
   收不到 stdin 密码、报 "a terminal is required"。修复为携带原始参数转发（`sudo -S -p '' …` 语义保真）。
2. **exec 通道未关闭 → sudo 时间戳锁死锁**（后端真实缺陷，`backend/src/exec.rs`）：russh `Channel`
   drop 不发送 SSH_MSG_CHANNEL_CLOSE（仅 `ChannelCloseOnDrop` 包装才发）。认证失败报错路径直接
   drop 通道后，远端 sudo 进程永远等 stdin 并持有 sudo 全局时间戳锁（`timestamp_type=global`），
   阻塞该连接所有后续 sudo（真机复现：keep-4 请求 45s/120s 超时，容器内可见两个 root sudo 挂起）。
   修复：新增 `abort_exec_channel()`，`exec_with_sudo`（空密码/带密码两分支）与 `exec_plain`
   的错误路径显式 `eof()+close()`，让 sshd 回收远端进程。修复后 10 用例全绿、无残留 sudo 进程。

## 2. 任务 2：UI 成熟化（小步多项，纯函数全部 vitest 覆盖，七语全补）

### 2a) 会话状态 pill 重连倒计时 / 进度感
- `terminalReconnect.ts` 新增纯函数 `describeReconnectCountdown({pending, attempt, nextAt, now, delayMs})`
  → `{seconds, attempt, percent}`（秒数 ceil 收敛到 0、按 backoff 时长算 0-100 进度、非 pending/非法输入返 null）。
- App.vue：调度重试时记录 `reconnectNextAt/reconnectDelayMs`；`watch(reconnectPending)` 驱动 250ms tick
  更新倒计时（退出 reconnecting 即确定性停表；`onBeforeUnmount` 双保险清理）。
- 模板：reconnecting 态 pill 内追加 `session-pill-countdown`（如 “retry in 2s · attempt 1”），dot 加脉冲动画。

### 2b) 终端标记条 hover tooltip（完整命令 + 退出码）
- `terminalCommandMarkers.ts` 新增纯函数 `commandMarkerTooltip(marker, labels)`：多行组装
  完整命令 / 退出码（已知才出现）/ 耗时（终值或运行中 live tick）/ 工作目录，缺失段省略不渲染噪音。
- App.vue：`commandMarkerDetails` computed 喂给标记条 `:title`；标记条启用 hover（`pointer-events: auto`，
  点击回焦终端，不吞交互）。

### 2c) SFTP 批量操作聚合进度条
- 新建 `lib/sftpBatchProgress.ts`：`createBatchProgress / advanceBatchProgress / batchProgressPercent`
  （done+failed 都计入已处理、钳制 0-100、纯不可变）。
- App.vue：`confirmBatchDelete`（含 sudo/remove 分支）与 `batchArchive` 逐项推进进度、失败项计入后原样抛错；
  批量工具条与删除确认对话框渲染 `<progress>` + `{done} / {total}` 文案；批量期间两个入口按钮禁用防并发。

### i18n 七语（en/es/it/ja/pt-BR/zh-CN/zh-TW）
新增 key 全部 ×7：`sessionStatus.reconnectCountdown`、`terminalCommand.tooltipCommand/tooltipExitCode/
tooltipDuration/tooltipDirectory`、`sftpBatch.progress`。既有“七语 key 全对齐 + 占位符对齐”vitest 断言
全量扫过 messages 表，41/41 通过即为自动化证据。

## 3. 任务 3：性能验证与优化

新增 `scripts/perf_baseline_test.py`（真机，SKIP 语义与其余 smoke 一致，SHA-256 双端校验）：

| 案例 | 口径 | 结果（本机，docker loopback） |
| --- | --- | --- |
| 终端 PTY 流灌入 | 5 MiB 连续输出经 PTY→sidecar 环形缓存→binary 帧 | **71.9 MiB/s**（0.07s，不含 2s 静默收尾窗口） |
| 终端 replay 载荷 | `ssh/terminal/replay` afterSequence=0 | **2.00 MiB / 601 帧**，严格 ≤ 2 MiB 上限，`complete=false`、首尾序号连续（旧帧已按序驱逐） |
| SFTP 上传（spool） | 50 MiB / 256 KiB 分块 + 逐块 ack 背压 | **633.9–1078 MB/s**（本地 spool 文件段） |
| SFTP 上传（网络） | `sftp/upload/finish` 真实 SFTP 写 | **220–237 MB/s**（SHA-256 与本地一致） |
| SFTP 下载 | `sftp/download/start+next` 全量 | **113–118 MB/s**（SHA-256 与远端 `sha256sum` 一致） |

**结论与瓶颈记录**：
- 环形缓存内存上限真实生效（Rust 单测 + 真机双重验证：80×64KiB 灌入后 `bytes == 2 MiB`、序号连续、
  `after(0)` 恰好返回保留尾窗；wire 载荷仅多每帧约 1.3B encode 头，属协议开销非泄漏）。
- 上传走“本地 spool → finish 阶段网络写”，两段语义在 perf 报告中分列，避免把 spool 速度误当网络吞吐。
- 本机 loopback 下 220/117 MB/s 远超交互场景需求，**未做分块/并发度参数化改动**（256 KiB 协商块
  `TRANSFER_CHUNK_SIZE` 维持不变）；如未来真实网络成瓶颈，优先候选是下载 `download/next` 流水线化与
  上传 finish 并发写，均已记录在案、本轮不动。

## 4. 任务 4：契约与文档

- **PROTOCOL.zh-CN.md 无需更新**：本轮无新增方法、无参数/返回契约变化；`abort_exec_channel` 为
  行为修复（错误路径通道关闭），sudo/OTP 行为契约此前已在册（Quick Sudo 段）。
- 出包：`io.dbx.ssh-0.2.2-darwin-arm64.dbxp`（sha256 见 §0），打包后对同快照二进制复跑双冒烟全绿。

## 5. 安全红线自查

- sudo/OTP smoke：凭据 `secrets.token_urlsafe` 运行时生成经 stdin `chpasswd`，退出恢复原密码；
  TOTP secret 只在宿主进程内存与容器“当窗期望码”出现，日志输出全部 `redact()` 打码（六位数字 → `******`）；
  sudoers 改动先备份后恢复并 `visudo -c` 校验；shim/flag/log 文件 finally 清理，多次运行幂等。
- 后端改动仅错误路径资源回收，无新增命令构造、无转义面变化。
- 前端：无新依赖；倒计时/tooltip/进度均为纯展示；新增定时器（重连 tick）三条路径确定性清理
  （重连退出、openSession/closeSession、onBeforeUnmount）。

## 6. 遗留与交接

1. **XB 遗留第 5 项已关单**（sudo 保活 / OTP 防重放 e2e）；XB §3 的收口点 2（PARITY/TEST_MATRIX 登记句）
   仍未动（文档所有权约束），主会话收口时请一并刷新：cargo 107→108、vitest 38→41、smoke_sudo_otp 10 用例、
   perf 脚本入库、本文件新基线。
2. 宿主集成段（`scripts/test.sh` 不带 --skip-host）与隧道真机验证仍属宿主依赖，本轮未覆盖。
3. perf 脚本的“上传网络段”时间含远端 `commit_remote_file`（rename 提交）与 flush；严格纯传输带宽可再细分，
   当前口径对基线追踪足够。
4. OTP e2e 依赖 shim 模拟 sudo 前端（Alpine sudo 无 PAM 的替代）；若未来测试镜像换用带 PAM 的发行版，
   可将 shim 相态替换为 `pam_google_authenticator` 真栈，用例结构无需变化。
5. 全部改动未 git 提交（硬性约束），请主会话审阅 diff 后统一收口。

---

## 7. 文件管理器交互轮补录（2026-08-30）

用户反馈 SFTP"丢了预览和编辑"，复核结论：预览/编辑/压缩解压链路均在，但交互门槛
造成功能体感丢失（详见 `FEATURE_PARITY.zh-CN.md`「文件管理器交互轮」节）。纯前端改动：

- `frontend/src/lib/textSniff.ts`（新建）+ `textSniff.spec.ts`（6 用例）：
  `looksBinary` 纯函数——NUL 字节即判二进制；否则按无效 UTF-8（U+FFFD）与
  非文本控制字符（放行 TAB/LF/VT/FF/CR/ESC）占比 > 10% 判定；空块判文本。
- `frontend/src/App.vue`：
  - `openEntry` 重排：图片 MIME 保留 → 已知二进制扩展名直接提示不打开 →
    > 1 MiB confirm 询问（取消即不动）→ 打开前 8 KiB 嗅探兜底 → 预览；
    白名单 `PREVIEWABLE_EXTENSIONS` 与 `isPreviewable` / `containsNullByte` 删除。
  - 截断预览（大文件确认后只读头部）：`previewBinary` ref 换为 `previewTruncated`，
    `previewEditableAllowed` 增加 `!previewTruncated`——修复截断态编辑保存
    整文件被头部覆盖的数据丢失 bug；弹窗标题徽标改显截断提示。
  - `archiveEntry` 放开单文件压缩；右键菜单预览/压缩条件同步放宽。
- `frontend/src/lib/i18n.ts`：七语新增 `binaryFile.notOpen`、
  `previewDialog.tooLargeConfirm` / `truncated`（原 `binaryFile.badge` 弃用删除；
  组名 `previewDialog` 避让既有 `preview` 字符串键）。
- `frontend/src/style.css`：`.preview-binary-badge` → `.preview-truncated-badge`。

验证：`pnpm typecheck` 0 错误；`pnpm test` vitest 3 spec **57/57**（51 → 57）；
`scripts/build.sh` 全绿，出包 `io.dbx.ssh-0.2.6-darwin-arm64.dbxp`。
后端零改动，无新增协议方法：无新 smoke 用例（既有 smoke_fs 的
`sftp/write + sftp/read round-trip` 等继续覆盖底层通道），安装后按惯例对安装副本
复跑双冒烟。遗留：预览解码对 GBK/GB18030 等非 UTF-8 文本仍按 UTF-8 容错显示
（与此前一致，未引入转码）；zip 格式压缩/解压仍不做（协议文档明确仅 tar 系）。

## 8. Quick Sudo 全局配置集中管理（2026-08-30）

需求：多套全局 quick sudo 配置集中管理，单连接可选「全局配置」或「本连接自输入」，
UI 与 MCP 双通道支持 quick sudo / auto sudo。设计契约记录随专项实施计划文档退役删除
（本轮推翻 FEATURE_PARITY L41 既有「不做」结论）。

- `backend/src/sudo_profiles.rs`（新建）：`<plugin_data_dir>/quick-sudo-profiles.json`
  版本化存储（`profiles` + `bindings`，0600，tmp+rename 原子写，损坏按空库），
  CRUD/名称唯一（大小写不敏感）/上限 20/视图永不回显密钥；
  `apply_profile`（字段级覆盖，空配置密码回退登录密码）与 `effective_use_pty`。
- `backend/src/exec.rs`：`AuthFlowMode::name()`（canonical 名），
  `ssh.rs::flow_mode_name` 改为委托。
- `backend/src/ssh.rs`：`resolved_sudo_auth`（绑定 profile 整体覆盖连接来源）；
  `open_session` 编排、`exec` use_pty、`settings_get`（新增 `quickSudoProfileId` /
  `quickSudoProfileName`）、`settings_set`（`quickSudoProfileId` 持久化绑定 +
  存活会话热更新）；`profiles_list` / `profiles_save` / `profiles_delete` +
  `refresh_bound_sessions`（配置改动即时热更新绑定会话，保留 OTP 防重放记账）。
- `backend/src/main.rs`：注册 `sudo/profiles/list|save|delete`。
- `backend/src/mcp.rs`：新增 `ssh_quick_sudo_profiles_list/save/delete` 工具；
  `ssh_exec_sudo` 支持可选 `quickSudoProfile`（id 或精确名称；引用在任何连接 I/O
  前解析，未知名称快速报错；调用内联凭据显式给出时优先）。
- `frontend/src/App.vue`：设置弹窗新增「sudo 凭据来源」select（本连接 / 全局配置）
  + 绑定摘要行 + 隐藏本连接凭据字段（总开关保留）；全局配置管理弹窗
  （列表/新建/编辑/删除，删除走 confirm，密钥仅提交时发送、永不回显）。
- `frontend/src/lib/i18n.ts`：supplemental 七语全补 20 键
  （`settingsCredentialSource`、`profiles*` 等）；`style.css` 三个小样式类。
- `scripts/smoke_fs_test.py`：quick sudo profiles 组 7 用例（list 初始 / save 创建 /
  重复名拒绝 / 空密码更新保持密钥 / list 不回显 / settings 绑定与解除 / delete+幂等）。
- `scripts/smoke_mcp.py`：MCP 通道 save/list/delete 回环 + `ssh_exec_sudo` schema
  断言 `quickSudoProfile` + 未知引用快速报错（默认二进制路径同步改为
  `backend/target/release/dbx-plugin-ssh`，对齐 test.sh）。

验证：`cargo test` **140 passed / 0 failed**（基线 128 → 140：sudo_profiles 12 +
MCP 回环）；`pnpm typecheck` 0 错误；`pnpm test` vitest 3 spec **58/58**（§7 基线 57/57）；`smoke_fs_test.py` **PASS 24 / SKIP 0 / FAIL 0**（真机容器）；
`smoke_mcp.py` **all green（22 tools）**。版本 `0.2.9` → `0.3.0`
（manifest.json + backend/Cargo.toml）。

安全评估：全局配置密钥落盘为插件数据目录明文 JSON（0600）——为「凭据走 secret
binding 不持久化」红线的显式例外（宿主 secret binding 仅支持连接级字段、无全局命名
凭据通道），已在 IMPL_PLAN §0/§9 记录权衡，
存储层收敛于 sudo_profiles 单模块，后续可替换 OS keyring。密钥仅在 list/save/get
以布尔位呈现，不进日志、不参与 shell 拼接。

### §8.1 入口补强：工具栏直达 + 连接表单动作桩（2026-08-30 晚）

反馈：「没有看到 quick sudo 的全局设置」——原入口埋在工作台设置弹窗里。核查宿主
二进制确认贡献点枚举仅 `connection-provider` / `workbench` / `filesystem-provider`，
**不存在插件级独立设置页**；但 `connection-provider` 支持 `actions`（连接表单动作，
宿主点击后调 `connection/action {action, id}`，插件回 `{message, fieldValues}`）。

- `frontend/src/App.vue`：工作台工具栏 Quick Sudo 开关旁新增钥匙按钮
  （KeyRound）直达全局配置管理弹窗（管理操作本就无会话依赖）。
- `ssh/manifest.json`：`connection-provider.actions` 新增 `quick-sudo-profiles`
  （`variant: outline`、`when: always`、`requires_valid_form: false`、
  `timeout_ms: 10000`）+ 六语 label/description；schema 经打包 CLI 宿主同款
  校验通过。
- 后端：`sudo_profiles::action_summary`（纯函数，密钥只报 set/not-set）+
  `SshRuntime::profiles_action_summary` + `main.rs` 注册 `connection/action`
  （未知动作报错）。摘要含全局配置清单与当前连接绑定状态，并提示完整管理入口
  在工作台。
- `scripts/smoke_fs_test.py`：`connection/action` 用例（清单含 profile、绑定行、
  未知动作拒绝；置于 delete 用例之前执行）。

验证：`cargo test` **141 passed**（+action_summary）；前端 typecheck/58 不变；
`smoke_fs_test.py` **PASS 25 / SKIP 0 / FAIL 0**；`smoke_mcp.py` all green；
`dbx-plugin package .` schema 校验通过出包 `io.dbx.ssh-0.3.2-darwin-arm64.dbxp`
（manifest 与 Cargo.toml 同步 0.3.1 → 0.3.2）。

## 9. MCP 完善与 ZCode 接入（2026-08-30 晚）

需求：完善 ssh 的 MCP 工具面并接入 ZCode 实测。补齐 MCP 工具面缺口
（`SFTPTransfer` / `sftpPwd`），并确认 ZCode 客户端接入路径。

- `backend/src/mcp.rs`：新增三工具（22 → **25**）——
  - `sftp_upload`：本地文件 → 远端（单文件）。本地侧校验（可读、≤`maxUploadBytes`）
    **先于拨号**；远端已存在需 `overwrite=true`。
  - `sftp_download`：远端 → 本地路径（单文件）。本地目标已存在需 `overwrite=true`、
    父目录自动创建，均先于拨号；远端先 stat 快速失败，再 `take(limit+1)` 硬上限
    （防无尺寸/边读边涨），≤`maxDownloadBytes`。
  - `sftp_pwd`：`canonicalize(".")` 返回登录家目录（与工作台 `sftp/home` 同源）。
  - **连接池语义修正**：传输工具移出 `run_tool` 兜底 `drop_connection`——本地/远端
    校验类拒绝（"already exists"、超限、是目录）不是传输故障，不清池；仅真正
    SFTP I/O 错误主动 drop 触发下次重连（此前 `sftp_write_file` 拒绝也会清池，
    同族问题留待后续统一，本轮不动存量行为）。
  - `sftp_upload` 加入只读连接写门控（`is_write_tool`）；`sftp_download` 保持只读
    放行（远端只读不写）。
  - 顺带：`model.rs` 去除重复 `#[test]` 属性（历史告警，一用例曾计两次）。
- `scripts/smoke_mcp.py`：
  - EXPECTED_TOOLS 补齐 25（含此前漏断言的 `sftp_copy`/`sftp_move`）；
  - 新增离线组：transfer 工具本地校验先于拨号（缺文件读失败 / 父目录是文件时
    mkdir 失败，均零拨号快速报错）；
  - 新增 `--host` 真机回环段：test_connection → exec（结果断言）→ metrics →
    pwd → list_dir → upload→`sha256sum` 远端比对→download→本地 SHA-256 比对→
    二次 download 拒绝 → 清理 → close；凭据走 `--password` 或
    `DBX_SSH_SMOKE_PASSWORD` 环境变量（新代码不落盘凭据）。
- 文档：`MCP.zh-CN.md`（工具一览 19→25、传输工具语义、**ZCode stdio 接入段**）、
  `FEATURE_PARITY.zh-CN.md`（新增 MCP 传输对标行）。PROTOCOL 无新增方法不动。

验证：`cargo test` **141 passed / 0 failed**（140 去重基线 + 1 新增 transfer
校验用例；去重前的 142 含 model.rs 双计）；`smoke_mcp.py` 离线 all green（25
tools）；真机回环（dbx-ssh-test 容器 127.0.0.1:2222）all green。ZCode 接入：
用户级 `~/.zcode/cli/config.json` `mcp.servers.dbx-ssh`（stdio，`--mcp`），
重启会话后 `mcp__dbx-ssh__*` 工具可用；详见 `MCP.zh-CN.md` 接入段。

## 10. MCP 生产误操作防范（2026-08-31）

需求：MCP 调用方是 LLM，生产环境误操作代价与人工敲错相同。给 exec 工具加
三层安全门，全部在任何网络 I/O 之前（真机验证记录见本节末）。

- `backend/src/mcp_safety.rs`（新增）：命令风险分级器 `assess_command` →
  `ReadOnly`（白名单巡检命令，含管道组合）/ `Destructive(reason)`（灾难模式）/
  `Unknown`（其余，白名单语义：识别不了 = 不放行）。
  - 只读白名单：ls/cat/df/du/ps/journalctl/docker ps/git log 等无条件动词 +
    systemctl/docker/git/kubectl/ip/service 子命令表 + find/crontab/journalctl
    特判（`-delete`/`-exec`、`-r`/`-e`、`--vacuum` 拦）；`timeout/nice/env`
    等包装器与 `FOO=bar` 前缀解包后再评估内层命令；重定向与 `$(...)` 一律
    Unknown；`sudo X` 仅评破坏性、永不计白名单。
  - 灾难模式：递归 rm 深层系统根（≤2 层路径；`/tmp`、`/var/tmp` 例外的常规
    清理不拦）、mkfs/fdisk/wipefs 系、`dd of=/dev/…`、`> /dev/sdX`、
    shutdown/reboot/init 0/6、fork 炸弹、`chmod/chown -R` 系统根、
    /etc/passwd|shadow|sudoers|fstab 与 /boot/ 覆盖删除、docker prune、
    `find -delete`、`kill -9 -1`、SQL DROP DATABASE/TABLE。
  - 刻意保守：引号不做完整解析（引号内 `;` 仍切分，只会降级不会放行）、
    `2>&1` fd 复制中性化后不误伤 `ps aux 2>&1 | grep`。
- `backend/src/mcp.rs`：`call_tool` 三层门——① 只读连接（DBX lifecycle
  read_only 或 `DBX_SSH_MCP_READ_ONLY=1` 全局开关）拒绝写类工具；② 只读连接
  上 `ssh_exec` 走白名单（此前 ssh_exec 完全绕过只读门，本次收紧）；③ 灾难
  命令要求 `confirmDestructive: true`，只读连接直接拒绝且确认位不可覆盖。
  两 exec 工具 schema 增 `confirmDestructive` 参数并在描述中说明门语义。
- `scripts/smoke_mcp.py`：离线组 destructive 无确认拒绝/带确认过门（错误信息
  断言门序先于凭据校验）；真机段 destructive 拒绝；新增
  `read_only_server_section`——第二个进程以 `DBX_SSH_MCP_READ_ONLY=1` 启动，
  断言写工具拒绝、巡检命令过门、未知命令白名单拒绝、确认位不可覆盖。
- 文档：`MCP.zh-CN.md` 新增「生产环境误操作防范」节（三层门 + 全局开关 +
  保守偏差说明）、工具一览补安全语义；`PROTOCOL.zh-CN.md` MCP 通道段补
  安全门一行。manifest/Cargo.toml 0.3.3 → 0.3.4。

验证：`cargo test` **150 passed / 0 failed**（+9 mcp_safety 单测 +2 门禁
集成用例）；`smoke_mcp.py` all green（含真机 dbx-ssh-test 容器 live 段：
destructive gate / read-only server gate 两节新增输出）。另对 DBX 内 vagrant
真实连接（192.168.33.11）完成 25 工具实测 + 磁盘清理演练（根分区 75%→72%），
清理所用命令（`rm -rf /root/.cache/*`、`journalctl --vacuum`、`truncate` 日志
截断）均落在 Unknown 档不误拦，验证白名单分级与真实运维操作兼容。

## 2026-08-31 AI 终端同步执行（agent terminal mode，0.3.4 → 0.4.0）

上游需求：AI/MCP 命令在终端 UI 同步执行——过程完整可见、可中断、可审批、可人工
介入（教学/接管语义）。设计经用户确认四决策：仅 MCP/AI 命令路由；分级审批；
无终端会话报错引导；超时返回部分输出命令继续跑。实施按专项计划执行
（计划文档已随批次退役删除）：后端/前端并行 agent 实施，主会话接线。

- `backend/src/agent_terminal.rs`（新）：`AgentTerminalMode`（off/auto/strict）、
  `CommandRisk`、`decide` 策略矩阵、`sanitize_command`（剥 C0 控制、留 `\n`/`\t`，
  杜绝 AI 命令内嵌 `\x03`/`\x1b`）、`strip_ansi`（CSI/OSC 状态机）、
  `TerminalRecorder`（1 MiB 有界缓冲、提示符回归 + 300ms 静默收尾、回显/尾提示符
  尽力剥离）。9 个单测。
- `backend/src/ssh.rs`：`agent_modes` 连接级内存存储 + settings get/set 新字段
  `agentTerminalMode`；会话 `agent_recorder` 槽挂进 PTY 读循环（auto_sudo.observe
  同位）；`exec_in_terminal`（sanitize → notice 事件 → PTY 键盘写入注入 → 50ms
  轮询收尾 → finish 事件 → `{output, exitCode: null, mode: "terminal", incomplete,
  interrupted}`）；审批挑战管理（`ssh/agent/prompt` 事件 + 120s 默认超时即拒绝 +
  一次性挑战 + `resolve_agent_challenge`）。
- `backend/src/mcp.rs`：`ssh_exec_tool` 路由分支（既有只读/灾难门之后）——
  `runInTerminal` 显式值优先、缺省按连接模式、Off 行为与响应结构不变；stdio
  模式传 `runInTerminal:true` 报错；`call_dbx` 透传 emitter（事件通道）；
  两 exec 工具 schema 增 `runInTerminal`。
- `backend/src/main.rs`：`mod agent_terminal`、`ssh/agent/resolve` 方法臂、
  `mcp/call` 传 emitter。
- 前端：`lib/agentTerminal.ts`（类型/三档/倒计时纯函数 + spec）、App.vue 审批
  弹窗（复用 host-key 骨架：可编辑命令 textarea + 风险徽标 + 倒计时）、执行横幅
  + 中断按钮（`sendTerminalBytes(\x03)` 复用既有 PTY 通道）、设置弹窗三档 select；
  i18n 20 键 × 七语。
- smoke：`smoke_mcp.py` stdio `runInTerminal` 拒绝负例 + schema 断言；
  `smoke_fs_test.py` 新增 agent terminal 组（模式 round-trip / 无会话引导错误 /
  auto 低危路由执行 / strict 审批 approve / deny 拒绝），事件回调驱动审批。
- 文档：PROTOCOL 新增「AI 终端同步执行」节 + RPC 表 `ssh/agent/resolve` 行 +
  settings 字段；MCP.zh-CN.md 新增「AI 终端同步执行」节 + 工具表补参数；
  manifest/Cargo.toml 0.4.0。

已知限制（协议文档已记）：多行命令按行执行；全屏 TUI 无提示符回归走超时路径
（`incomplete: true`，命令留终端人工接管）；回显剥离尽力而为；sudo+终端路径
不注入密码（交给终端 auto-sudo 状态机或人工）。stdio `--mcp` 模式不路由
（与工作台不同进程）。

### 增补：vagrant 真机教学模式验证 + sudo 前缀风险修复（同日）

对真实 vagrant 连接（192.168.33.11，parallels ubuntu，password/private-key 认证）
完成教学模式端到端 9 场景验证：auto 模式低危路由（`uname -sr` → notice 事件 +
输出捕获）、sudo 提权审批（elevated → approve → root）、strict deny/approve、
人工 Ctrl+C 介入（3.3s 返回 vs 30s sleep，命令未跑完、shell 状态保留）、超时
`incomplete:true` 后人工接管（`\x03` 终止残留 sleep 后 shell 可用）、模式恢复。

验证中发现并修复一个风险缺口：路由分级此前只把 `ssh_exec_sudo` **工具**计为
elevated，`ssh_exec` 命令文本内联 `sudo …`（如 `sudo whoami`）被判 Low 直接执行、
绕过审批。修复：`mcp_safety::runs_under_sudo()`（复用既有分段/env 前缀解包逻辑，
任一顶层段首动词为 sudo 即真）接入 `ssh_exec_terminal_tool` 风险计算 → 内联
sudo 一律 elevated 必审，对齐 IMPL_PLAN §1「sudo 一律 elevated」。+1 单测
（162 passed）。容器 smoke 回归 31/31 全绿。

### 增补：自动唤起 DBX app 的完整可视链路（同日晚，宿主配合改动）

用户验收反馈：教学模式应当"自动唤起 DBX.app → 自动打开对应终端 → 命令在终端上
可见执行"。查明宿主三层缺位并补齐（宿主仓 `dbx-plugin-host-worktree` 本地改动，
未提交）：

1. **宿主 `src-tauri/src/commands/mcp_bridge.rs`**：TCP 桥新增 `POST
   /call-plugin-tool`——按 `connection_id` 解析已保存连接 → emit
   `mcp-open-connection-workbench` 事件 → `connection_params_standalone` 构造
   lifecycle → 在 **app 自己的 plugin_host**（与工作台同一 sidecar 进程）上
   `invoke("mcp/call")`，结果原路返回。超时上限 600s（容纳审批）。
2. **宿主前端 `apps/desktop/src/composables/useTauriEvents.ts`**：监听
   `mcp-open-connection-workbench` → `queryStore.openPluginConnection`
   （内部去重、ensureConnected、挂工作台）→ 聚焦窗口。
3. **插件 `backend/src/app_bridge.rs`（新）**：stdio 模式 `runInTerminal:true`
   时转发到 app 桥——端口文件 `<app-data>/mcp-bridge-port`（`DBX_APP_DATA_DIR`
   重定向），缺失则 `open -a DBX.app`（`DBX_APP_LAUNCH_CMD` 可自定义）唤起并
   500ms 轮询 30s；手写最小 HTTP POST（零新依赖）；读超时 = 调用超时 + 150s
   审批余量。`mcp.rs` 内嵌路径另加 `wait_for_connection_session`（20s 轮询，
   解决"标签尚在打开、PTY 未就绪"竞态）。

e2e（隔离 app-data + 宿主 debug bundle + 已保存 vagrant 连接）：stdio
`ssh_exec{runInTerminal, connectionId:"vagrant"}` → app 唤起/工作台自动打开 →
命令在可见终端执行 → AI 拿到 `{mode:"terminal", output:"agent-visible-…\nLinux
5.4.0-216-generic"}`，1.3s 返回，cargo 166 tests 全绿。连接种子需
`db_type:"plugin"` + `plugin_id` + `plugin_connection_provider` +
`plugin_connection_type:"ssh"` 四个绑定字段。宿主 tauri debug 构建末尾的
updater 签名报错（缺 `TAURI_SIGNING_PRIVATE_KEY`）不影响 .app 产物。

### 增补：教学模式并发语义与完整测试覆盖（同日夜）

针对「并发命令 / 连接复用」的系统化补强，三处实现 + 覆盖扩展：

1. **同会话并发串行化**：`SessionEntry.agent_exec_lock`（tokio AsyncMutex，
   OwnedMutexGuard 跨审批+执行全程持有，`mcp.rs::ssh_exec_terminal_tool` 取锁）——
   并发 AI 命令在同会话上确定性排队，recorder 与 PTY 输入零交叉污染；刻意不做
   全局锁，跨连接仍并行。
2. **ssh_exec_sudo 终端注入修复**：`agent_terminal::sudo_command_text`——终端路径
   注入 `sudo <command>`（已带前缀不重复；审批弹窗显示注入原文，所见即所执行，
   用户编辑后的文本不二次套前缀）。修复提权在终端路径被静默丢失的 bug。
3. **前端审批队列**：`agentPromptQueue` + `enqueueAgentPrompt/dropAgentPrompt/
   findAgentPrompt` 纯函数——跨会话并发审批排队逐个处理，队首倒计时基于绝对期限
   自动轮转；同 challengeId 去重。

测试覆盖扩展（smoke_fs agent 组新增 12 用例，44/44 全绿）：
shell 状态复用（export/cd 跨调用持久）、同会话并发 batch（`sidecar_client.
request_batch` 单 pump 多 outstanding id，A/B 输出零交叉）、跨连接并行（4.3s
< 串行 8s，锁非全局）、2MiB 输出有界捕获、ANSI 剥离、多行逐行执行、
runInTerminal:false 回归隐藏通道、ssh_exec_sudo 审批链到 root（终端 auto-sudo
自动应答编排密码）、超时 incomplete → Ctrl+C 恢复、人工 Ctrl+C 介入 2.4s 返回、
smoke_mcp 增桥不可达负例（`DBX_APP_LAUNCH_CMD=:` 快速失败路径）。
cargo 170 tests / vitest 68 tests / 容器 smoke_fs 44·smoke_mcp all green。

### 增补：断线重连死循环 + 侧栏状态一致性修复（同日晚二）

用户报障两则，修复：
1. **断线后无自动重连/转圈不停**：`ssh/session/state disconnected` 分支原样只改
   状态等人手点；现改为有界自动重连梯（500/1000/2000/5000ms 共 4 次，失败落回
   手动重连按钮，不收敛不死循环）。`openSession` 的自愈重试限定"快速失败"
   （<8s，启动竞态特征），重试超时降 30s——真拨号失败不再连续转圈数分钟。
   `attachSession` 退避梯耗尽后回退 `openSession(true)`（原实现永远 attach 死
   sessionId，app 被杀重启后终端永久卡重连）。
2. **侧栏状态与 SSH 实际状态不一致**：`ssh/session/state` 事件补 `connectionId`；
   宿主 `connectionStore` 新增 `markConnectionOffline`（轻量：翻侧栏离线、清
   loading，不关标签不拆池），`useTauriEvents` 监听 `dbx-plugin-event` 转发通道
   驱动它——PTY 掉线即刻反映到左侧树。
另修：批准路径误发 `finish{denied}`（前端横幅闪错）；执行中关闭会话现返回
"SSH terminal session was closed…"而非伪装超时；僵尸会话注入 send 加 5s 上限；
`app_bridge::ensure_app_bridge` 先 TCP 探测端口再返回（杀进程后过期端口文件不再
永久指错），std io 客户端 `McpStdioClient` 入库 sidecar_client.py（select 超时）。
新 e2e 套件：`scripts/e2e_agent_terminal.py`（26 场景，容器 26/26、vagrant
25/25+SKIP 全绿）与 `scripts/e2e_agent_app_bridge.py`（T1-T5）。T5（转发 shell
状态复用偶发空输出）仍在排查，手动探针证明变量持久化正常。

## 2026-08-31 连接表单 sudo 来源三选一 + 宿主保存校验修复（0.4.2）

1. **连接保存报 "Agent socket has an invalid value type"**：根因在宿主——
   `hktkosl1186` 等连接的 `external_config.agent_socket` 等字段存了 JSON
   `null`（旧对话框构建遗留），宿主 `validate_plugin_field_type` 对 text
   字段只认字符串，test/connect 全被拒。修复宿主 worktree
   `crates/dbx-core/src/plugins/host.rs`：校验前把 null 视为未填写
   （`value.filter(|v| !v.is_null())`），附单测
   `tolerates_stored_null_config_values_for_type_check`（cargo +1.97.1 过，
   host 依赖要求 ≥1.91，需 `cargo +1.97.1`）。**宿主 app 需重打包生效**。
2. **连接表单选不到全局 Quick Sudo**：manifest `quick_sudo` 布尔升级为
   `sudo_source` 三选一（off / custom / global）+ `sudo_profile` 引用字段，
   `visible_when` 联动（custom 才显示本连接密码/PTY，global 才显示配置
   引用）；后端 `SudoSource` 解析兼容旧布尔，`effective_sudo_profile` 统一
   会话引导 / exec 门禁 / PTY / settings 的配置解析；`settings/set` 绑定
   变化联动 source，`settings/get` 新增 `sudoSource`。单测 3 个新用例 +
   manifest 一致性用例更新；七语文案补齐（es/it/ja/pt-BR/zh-CN/zh-TW）。
   协议/对标文档同步（PROTOCOL §运行时设置/§sudo、FEATURE_PARITY）。

### §8.2 MCP 直调走声明的 sudo 来源 + 诊断日志（0.4.x 轮增补）

排查 hktkosl1086「终端 sudo -v 不自动输密码 / MCP 直调不走 quick sudo」：

- **MCP 缺口（本轮修复）**：隐藏通道 `ssh_exec_sudo` 此前只从调用参数构建凭据，
  已保存连接声明的 `sudo_source`（global→表单引用/工作台绑定，custom→连接自身
  secret，off→拒绝）完全不参与。新增 `resolve_sudo_auth`：声明来源解析为基底
  凭据 → 调用方显式参数（`sudoPassword`/`totpSecret`/提示词/`authFlowMode`）
  恒优先 → 每调用 `quickSudoProfile` 引用替换连接声明的 global 配置；`off` 且
  无显式凭据时按工作台同款门禁拒绝。`resolved_sudo_auth` /
  `effective_sudo_profile` 放开为 pub(crate) 复用。
- **诊断日志**：终端 watcher 已挂载但凭据为空时，检测到 sudo 密码提示输出
  `[ssh] terminal auto-sudo: sudo password prompt detected but no sudo password
  is configured …`；watcher 解除挂载输出原因（sudoSource/readOnly/凭据是否配置），
  「为什么不自动应答」可直接看 sidecar 日志定位。
- 单测：`sudo_auth_resolution_follows_declared_source`（global 生效 / 显式参数
  优先 / 每调用引用替换声明 / custom 回退登录密码 / off 拒绝与显式放行）。

### §8.3 重启恢复快速失败 + SFTP 面板可收起/默认不打开（纯前端轮）

用户报告两处体验问题，本轮均为前端改动（无新协议方法）：

1. **DBX 重启后恢复的 SSH 工作台长时间转圈**：根因链条——宿主 openTabs
   持久化恢复 plugin-workbench tab 但**不重放 `connection/connect` 生命周期**
   （`openTabsStartup` 无 plugin 处理、`openPluginConnection` 不在恢复路径上），
   sidecar 重启后 `connections` 内存表为空，`ssh/session/open` 立即返回
   `Connection is not active`；而前端 `openSession` 把一切快速失败当
   「启动竞态」盲目重试 3 次（2/4/6s 递增），期间状态 pill 持续「连接中」，
   最终只显示英文原始错误。修复：`terminalReconnect.ts` 新增
   `isConnectionInactiveError`（匹配 sidecar 稳定错误串，大小写不敏感），
   `openSession` catch 里命中即**跳过重试直接 error**，错误显示七语
   `connectionInactive` 文案（指引用户从 DBX 侧边栏重新打开连接再点
   「重新连接」）。真正的启动竞态（sidecar 未激活等）保留原重试逻辑。
   宿主 1.1 的 `restored` 标记分支保留（当前宿主未下发，为死代码无害）。
2. **SFTP 面板增加打开按钮 + 可设置默认不打开**：
   - 工具栏新增 toggle 按钮（`FolderOpen`/`PanelRightClose` 图标，
     `sftpPane.open`/`sftpPane.close` 七语 title），收起时终端
     `flex-basis:100%` 占满（`panes--solo` 类，含窄屏纵向布局覆盖）；
   - 「自定义列」弹出层新增「默认打开 SFTP 面板」checkbox
     （`sftpPane.defaultOpen`/`defaultOpenHint`），写 localStorage
     `ssh-sftp-pane-open`（全局偏好，仅影响新工作台初始态）；
   - 每工作台开关写入 `workbenchState.sftpPaneOpen`（宿主 1.1 可用时
     随 tab 恢复；`restoreUiState` 经 `resolveSftpPaneOpen` 解析，
     损坏值回退全局默认）。纯函数入 `workbenchLayout.ts`
     （`resolveSftpPaneOpen`/`sanitizeSftpPaneDefaultOpen`）。

单测：workbench.spec.ts +3 用例（inactive 错误识别含反例 / 面板可见性
解析 / 偏好解析），i18n 七语 key 对齐检查覆盖新增 key；vitest 71 绿 +
typecheck 过 + build 过 + visual.html 浏览器验证（默认双面板 → 收起 →
默认偏好关闭 → 刷新后仅终端 → 手动重开）。**剩余风险**：重启恢复场景
的端到端行为（真实宿主重启 + tab 恢复）未在本轮实测，依赖单测对错误
分类的覆盖；宿主后续若下发 `restored`，前端已有对应分支。

### §8.4 SFTP 操作归位面板内 + metrics 悬浮卡（纯前端轮）

用户反馈两处工具栏归属/形态问题：

1. **SFTP 专属按钮移入面板 path-toolbar**：Home、刷新、上传、新建文件夹、
   新建文件 5 个按钮从顶部全局工具栏移入 SFTP 面板路径栏（上级/路径输入
   之间与历史/粘贴之后），顶部工具栏只留连接/终端级操作（面板开关、字号、
   重连、Quick Sudo、命令、快速命令、metrics、连接信息、设置、自定义列、
   传输）。面板收起时按钮随 `v-if` 自然消失，不再出现"按钮在但面板不在"
   的悬空禁用态。路径输入框加 `min-width:110px` 防挤压；sudo 开关加
   `.sudo-label` 间距。文案全部复用既有 key，无新增。
2. **metrics 从阻塞弹窗改为悬浮卡**：`openMetrics` 改 `toggleMetrics`
   （再点 Gauge 或 X 关闭，按钮带 `is-active` 高亮），渲染从
   `modal-backdrop` 改为工作台右上角 `.metrics-float`（absolute、z-index 20
   低于 modal、宽 min(400px,100vw-20px)、内部滚动），不遮挡不阻塞终端与
   SFTP 操作——点击终端/收起面板/继续操作时卡片保持打开，可边看指标边
   操作。5s 自动刷新与错误重试逻辑原样保留。

验证：typecheck 过、vitest 71 绿（无新纯函数/文案，无需新用例）、build 过、
visual.html 浏览器验证（顶部按钮清单、path-toolbar 按钮清单、点终端卡片
保持、收起面板卡片保持、Gauge 再点关闭、深浅两态截图）。布局 CSS 仅
`.metrics-float` 系列与 path-toolbar 两处微调。

### §8.5 MCP 长任务/断线恢复三层机制（spawn 并发 + run_bg/status + pre-exec 重试）

真机长任务暴露的问题链：宿主 ~15s 放弃等待（timeoutSecs 形同虚设）→ sidecar
handler 继续跑满 → 远程命令继续执行；stdio 主循环逐请求 `block_on`（mcp.rs
`run_mcp_stdio`）导致一个慢命令阻塞后续全部请求（含 `ssh_close`），表现为整
server 连环 15s 超时直至最长 handler 到期；Agent 误判"超时=没跑"重复下发，
两个 yum/dnf 互等包管理器锁。三层修复：

1. **stdio 请求并发**：`run_mcp_stdio` 逐请求 `tokio::spawn`（响应经
   `Mutex<Stdout>` 保行完整，乱序合法），stdin 关闭后 drain 在途请求至多
   300s 再退出。真机对照：`ssh_exec sleep 15` 运行中 `ping` t+0.0s 即回
   （旧行为需等 15s）。
2. **`ssh_run_bg` / `ssh_task_status`**（工具 25→27）：nohup 脱离会话启动 +
   服务器侧 `/tmp/.dbx-ssh-tasks/<taskId>.log`（含 `EXIT_<code>` 完成标记与
   `.pid` 存活文件），状态轮询跨断线/跨会话。与 `ssh_exec` 同过危险命令确认
   门与只读写门（`is_write_tool` + assess 扩展）。
3. **pre-exec 断线自动重试**：`run_tool` 兜底分支失败丢池照旧；命令启动前的
   传输错误（`exec::is_pre_exec_transport_error`，通道打开/启动失败类）同一次
   调用内换新连接重试一次（不可能双执行）；已启动后的错误保持终态。keepalive
   （30s×3）已有，未改。

配套：`run_to_completion` 超时错误文本加"命令可能仍在远程运行，先查证再重试"；
`ssh_exec`/`ssh_exec_sudo` 描述与 `timeoutSecs` schema 改为如实描述宿主 ~15s
上限；`MCP.zh-CN.md` 工具一览 27 个 + 新增「长任务与断线恢复」章节；用户级
dbx-ssh-sftp-dev skill 增补同名约定章节。

验证：cargo test 181 绿（新增 4：bg/status 输出解析 ×2、只读门/危险门对
ssh_run_bg 生效 ×2、pre-exec 判定正反例）；clippy 无新告警；
`smoke_mcp.py --binary target/debug` all green（27 工具、只读进程级开关、
危险门、app-bridge 拒绝路径均过）；真机 hktkosl1086（RHEL 9.8）三段 smoke：
run_bg 立即返回 taskId/pid/logPath → 新进程轮询 RUNNING(pidAlive=yes) →
完成态 DONE + exitCode 0 + started/finished 时间戳精确（12s 任务实测 12s）。
开发中发现并修复两个自引入问题：`shutdown_timeout` 先取消再等掐死在途任务
（改 drain），`tokio::time::timeout` 构造期取 Handle::current 需在 block_on
context 内构造。

剩余风险：七语不涉及（纯 MCP 工具无 UI 文案）；pre-exec 重试无真机断线注入
（依赖单测正反例）；`ssh_run_bg` 日志不自动清理（刻意保留任务记录，由调用方
清理）；宿主侧 ~15s 等待上限属宿主行为，本插件只能以描述引导 + bg 工具绕开。
安装生效需重新打包发版（本轮未动 manifest 版本）。

### §8.5 metrics 卡与 SFTP 共存 + 选中复制/右键粘贴开关（纯前端轮）

用户反馈两处 UI 继续优化：

1. **metrics 悬浮卡移入终端面板内部**：上轮悬浮卡挂在 workbench 右上、
   宽 400px，会盖住 SFTP 面板（"metrics 和 sftp 不能共存"）。改为挂在
   `terminal-pane` 内部右上（`top/right 8px`、宽 min(360px, 100%-16px)、
   `max-height calc(100%-16px)` 内部滚动、z-index 6 低于搜索面板 7）——
   只遮挡终端一角（随时可关），SFTP 面板完全不被遮挡，收起面板后同样
   可用。浏览器验证：`cardInTerminalPane=true`、与 sftp-pane 包围盒
   零重叠。
2. **新增"选中复制 · 右键粘贴"开关（默认开）**：XShell 风格终端交互。
   - 纯函数 `lib/terminalInteraction.ts`：`sanitizeSelectCopyEnabled`
     （localStorage `ssh-terminal-select-copy`，仅显式 "false" 关闭）+
     `resolveTerminalRightClickAction`（开启且非 Shift → paste，否则 menu）；
   - App.vue：`terminal.onSelectionChange` 选中即静默写剪贴板（无提示刷屏）；
     `showTerminalMenu` 右键分流——开启时普通右键直接走 `pasteTerminal`
     （保留多行/危险命令粘贴确认），**Shift+右键保留完整右键菜单**，关闭
     时恢复纯菜单行为；切换即生效并持久化，notice 提示当前模式；
   - 设置弹窗新增「终端交互」区块 + switch（默认 on），七语文案
     `terminalSelectCopy.{section,label,hint,enabledNotice,disabledNotice}`；
   - 单测 +2（偏好解析 / 右键分流含 Shift 反例），vitest 73 绿。
   - 附带：mockDbxHost 补齐 `sudo/profiles/list`、`ssh/knownHosts/list`、
     `keys/discover`、`mcp/settings/get` 空数据返回（原默认 `{success:true}`
     导致 fixture 打开设置弹窗时 `undefined.length/find` 渲染错误，纯
     测试工具问题，不影响真实 sidecar）。

验证：typecheck 过、vitest 73 绿、build 过、visual.html 浏览器验证
（开关默认 on、切换持久化 localStorage、notice 文案、开启时右键不弹菜单、
Shift+右键弹菜单、关闭后右键恢复菜单）。

### §8.6 终端快捷键 + 搜索面板选区种子/选项持久化（纯前端轮，2026-09-01）

后台 agent 执行轮，最终汇报偏题，改动本体完整有效，由主 agent 补齐验证与
本文档。改动均为纯前端（App.vue / TerminalSearchPanel.vue /
lib/terminalInteraction.{ts,spec.ts}），协议契约与后端零改动：

1. **终端内快捷键路由（iTerm2/XShell 风格）**：新增纯函数
   `resolveTerminalKeyAction`——Ctrl/Cmd+V 与 Ctrl/Cmd+Shift+V 粘贴（沿用
   既有风险确认流程），Ctrl/Cmd+Shift+C 复制当前选区；**普通 Ctrl/Cmd+C 不
   拦截**，保持发给远端 shell（SIGINT 语义）。App.vue `handleTerminalKey`
   接线，复制复用 `copyTerminalSelection`。
2. **搜索面板选区种子**：`terminalSearchSeedFromSelection` 取终端当前选区
   首行（截断 200 字符），打开搜索面板时预填并立即执行一次查找（面板打开
   即出结果）；多行/超长选区不会产生不可用查询。
3. **搜索选项持久化**：`sanitizeSearchOptions` / `persistSearchOptions`——
   localStorage `ssh-terminal-search-options` 保存 caseSensitive/regex/
   wholeWord 三开关（JSON 对象；解析失败或缺失回退全关，localStorage 不可用
   时降级会话级）。面板重开/页面刷新后恢复，切换即时写入。
4. **附带修复**：xterm `allowProposedApi: true`——SearchAddon 的 highlight
   decorations 走 proposed API，缺该项会在 findNext/registerDecoration 时抛
   "allowProposedApi option"。

验证（主 agent 复核）：
- typecheck 过；vitest **79 绿**（基线 73 → 79，terminalInteraction.spec
  8 例：快捷键分流含 Ctrl+C 放行反例、搜索选项 sanitize/persist、选区种子
  首行截断）；build 过（产物写 ui/index.html）。
- 浏览器验证（visual.html @ vite 5180，截图
  `docs/screenshots-ui-mock/search-options-persist-round86.png`）：
  Ctrl+F 打开面板、切换 Aa/.*/|w| 即时写入 localStorage
  （`{"caseSensitive":true,"regex":true,"wholeWord":true}`）、输入 nginx
  命中 2 处（状态 1/2）、页面刷新后重开面板三开关全部恢复。

剩余风险：快捷键真键程未在真机验证（纯函数单测覆盖分流逻辑）；搜索种子
仅首行策略为刻意取舍（多行选区不整段带入）。

### §8.7 终端拖放上传入口 + 大输出渲染节流（纯前端轮，2026-09-02）

后台 agent 持续完善轮，两项聚焦改进，均为纯前端（App.vue / style.css /
lib/terminalWriteThrottle.{ts,spec.ts} / lib/terminalInteraction.{ts,spec.ts} /
mockDbxHost.ts），协议契约与后端零改动：

1. **终端窗格拖放上传**（补齐 batch3 deferred「路径拖拽上传增强」的终端侧）：
   - 此前拖放上传只有 SFTP 面板一个 drop 目标，面板收起（solo 模式）后无入口；
     现在 `.terminal-pane` 自带 dragenter/dragover/dragleave/drop 处理，
     拖入文件显示 `drop-overlay`（虚线框 + 上传图标 + 七语提示
     `terminalDrop.hint`「松开上传到 {path}」，path 为当前 SFTP 目录）；
   - 准入纯函数 `canAcceptTerminalDrop`（terminalInteraction.ts）：已连接 +
     可写 + 非 ZMODEM 占用三者齐才收文件，只读连接/ZMODEM 传输中静默拒绝
     （与 SFTP 面板 drop 同语义）；drop 后复用 `uploadLocalFiles` 全链路
     （传输面板、分块上传、目录刷新、完成 notice），目录跟随开启时即上传到
     shell 当前 cwd；宿主 fileTransfer 拖拽态（dragActive）在面板收起时也
     复用同一 overlay 提示；
   - mockDbxHost 补最小可写 fixture：URL `?rw=1` 切换可写连接（默认只读）+
     `sftp/upload/start|finish`、`sftp/transfer/cancel`、上传 ack 事件，
     拖放上传可在 visual.html 全流程走通。
2. **大输出渲染节流**：新增 `lib/terminalWriteThrottle.ts`——PTY 二进制帧
   不再逐帧直写 xterm，而是排队合并为一帧一次合并 write（rAF 调度，
   setTimeout 兜底），顺序严格保持；排队字节超 1 MiB 上限同步 flush，
   持续突发下内存有界；`dispose()` 于工作台卸载时冲刷残余。sink 惰性引用
   `terminal`，跨终端重建安全。App.vue `writeTerminalOutput` 改走节流通道，
   卸载钩子补 `terminalWriteThrottle.dispose()`。

验证：
- typecheck 过；vitest **86 绿**（基线 79 → 86：terminalWriteThrottle 6 例
  （合帧合并、跨帧顺序、上限同步 flush、flush 取消不双投、dispose 冲刷、
  空队不投递、超限单块整投）+ terminalInteraction 拖放准入 1 例含三反例）；
  build 过（ui/index.html 产出）。
- 浏览器验证（visual.html @ vite 5180，Playwright，截图
  `docs/screenshots-ui-mock/terminal-drop-overlay-round87.png`（分屏）与
  `terminal-drop-solo-round87.png`（solo 全宽））：
  - `?rw=1` 拖入文件 → overlay 出现且提示含 `/home/demo`；dragleave 即消失；
  - drop → 传输面板打开、notice「1 file(s) uploaded」、无错误横幅，分屏与
    solo 两模式均过；
  - 只读模式（默认 fixture）→ overlay 拒绝出现；
  - 节流写入路径回归：欢迎输出/命令标记等 PTY 帧渲染正常。

剩余风险：真实大文件拖放上传未连真机（fixture 全流程 + 单测覆盖逻辑，
真实 SFTP 通道由既有 uploadLocalFiles 链路承担，无新协议面）；xterm 键入
回显 fixture 不模拟（mock 只回 ack），大输出节流在真机突发下的体感收益
未量化（单测保证合并/顺序/上限语义）；`dragleave.self` 沿用 SFTP 面板
同一简易模式，极端嵌套拖拽路径未穷举。

### §8.8 断线重连体验轮 + 重连死锁修复（2026-09-02）

第三轮 agent 因配额超限中断，改动主体完整（重连横幅 / 立即重连按钮 /
恢复提示 / fixture `?err=disconnect`），主 agent 验证时**发现并修复一个被
fixture 首次暴露的既有死锁**。

**新增能力**（纯前端）：
1. **重连横幅**：`reconnectPending` 期间终端内嵌横幅（Loader + 「连接丢失，
   自动重连中」+ 第 N 次重试 + 进度条 + 立即重连按钮），250ms tick 驱动纯函数
   `describeReconnectCountdown`；
2. **恢复提示**：重连成功后按 `describeReconnectRestoredNotice` 显示
   「已重新连接，当前目录 {path}」（有 cwd 时）或「连接已恢复」，首连不弹；
3. **fixture `?err=disconnect`**：会话建立 4s 后注入一次
   `ssh/session/state disconnected`，全流程 UI 验证载体。

**死锁分析**（`?err=disconnect` 首次真实触发，页面 100% CPU 冻结、连
Playwright evaluate 都被饿死、headless virtual-time-budget 永不完成）：
1. 首连消费终端帧 seq=1 后 `lastSequence=1`；
2. 断线自动重连走 `openSession()` 重置 `lastSequence=0`，但 mock 的序号
   计数器是全局的——重连后欢迎帧 seq=2，帧 1 已被消费、**永久缺失**；
3. `drainTerminalFrames` 检测到缺口即调 `ssh/terminal/replay`，mock 返回
   `complete:true` 但不补帧 → `.finally` 再 drain → 缺口依旧 → 再 replay；
4. **promise 微任务级无限自旋**（每轮极快、永不给事件循环让路）。

**修复**（两侧）：
- `mockDbxHost.ts`：`ssh/session/open` 时 `sequence=0`——对齐真实 sidecar
  「每会话重置序号」语义，重连后 `lastSequence=0` 与新帧序号天然对齐；
- `App.vue openSession`：重置游标同时清空 `pendingTerminalFrames` 并复位
  `replayNoProgress`（旧会话残帧不污染新流）；
- `App.vue drainTerminalFrames`：**无进展熔断**——连续 3 次 replay 返回
  complete 但同一缺口未补齐时，`lastSequence = firstPending-1` 越过缺口
  （丢弃缺失前缀的降级路径，优于永久自旋冻结整个工作台）。

**验证**：typecheck 0 错；vitest **87 绿**；build 过。修复前 headless
`--virtual-time-budget` 确定性挂死（exit 124），修复后 10s 虚拟时间完整
跑完（exit 0）终态 Connected；真浏览器 MutationObserver 捕获横幅
「Connection lost, reconnecting automatically · Reconnect now」与恢复提示
「Reconnected, current directory /home/demo」；终端欢迎行 ×2、提示符 ×4
证明第二轮 OSC 633 周期完整重放；截图
`docs/screenshots-ui-mock/reconnect-restored-round88.png`（横幅窗口仅
~500ms，像素截图以 DOM 观察器文本证据为准）。

**剩余风险**：熔断的「丢前缀」是降级路径；真实 sidecar 的序号重启语义与
mock 假设需真机断线注入回归确认；后端未动（协议零变更）。

### §8.9 stdio MCP 声明 connectionId——已打开终端可被 MCP 驱动（2026-09-02）

**问题**：stdio 模式（ZCode 直连 sidecar `--mcp`）下，`ssh_exec{runInTerminal:true,
connectionId}` 三连败——分发器读 `connectionId`（mcp.rs `ssh_exec_tool`）但
`connection_properties()` 从未在 inputSchema 声明该字段，严格校验的 MCP 客户端
先以「未声明参数」拒绝，字段根本到不了 sidecar，表现为「MCP 工具不接受
connectionId」「已打开的终端会话 MCP 调用不了」。

**修复**（纯 schema 声明，零逻辑变更）：
- `mcp.rs connection_properties()`：头部声明 `connectionId`（string，说明终端
  路由与 embedded 存储连接解析两用途）——`ssh_*` + `sftp_*` 共 22 个连接类工具
  一次性覆盖；
- 单测 `connection_tools_declare_connection_id` 防回归（8 个代表工具断言）；
- `smoke_mcp.py` schema 段补 `connectionId` 断言；
- `docs/MCP.zh-CN.md`「AI 终端同步执行」节补声明说明与连接 id 查询口径。

**验证**：cargo test 183 绿；release 重编后对安装同款二进制 spawn stdio 会话：
initialize → tools/list 27 工具、22 个声明 `connectionId` →
`ssh_exec{connectionId:"e60c6b55-…"(hktkosl1103), runInTerminal:true,
command:"echo DBX_BRIDGE_OK_…"}` 经 app bridge（mcp-bridge-port 49568）落到 DBX
可见工作台终端，返回 `{mode:"terminal", output:"\rDBX_BRIDGE_OK_…"}`，marker 命中。

**流程结论**（stdio 客户端视角）：驱动已打开终端 =
`connectionId + runInTerminal:true`；连接 id 查 `~/Library/Application
Support/com.dbx.app/dbx.db` 的 `connections` 表（本机 SSH 连接用户名统一
jinpy.he）。存量 MCP 会话需重连/重启才拿到新 schema。

**剩余风险**：`connectionId` 不带 `runInTerminal` 时在 stdio 模式不解析存储凭据
（仍需内联参数，行为与之前一致）；`ssh_list_connections` 发现工具未做（可选后续，
需定数据源：rusqlite 或宿主桥端点）。——本条欠账已于 §8.16 清账（宿主桥端点方案
+ stdio 桥接兜底，`connectionId` 零凭据可用）。

### §8.10 切 tab 重连/闪屏修复：重挂载 reattach 存活会话（2026-09-02）

**症状**：SSH 工作台切换 tab 触发重连且终端闪屏；已打开的 SSH 从左侧菜单重新
唤起后连接被重置（全新登录、屏幕清空）。

**根因**（宿主侧限制 × 插件侧兜底未命中，三层叠加）：
1. 宿主 `ContentArea.vue` 只渲染 activeTab 且无 KeepAlive——切 tab 即销毁插件
   webview（iframe srcdoc），重开时整体重建（闪屏的物理来源）；
2. 宿主桥**未实现 workbenchState**（pluginHostBridge.ts 全仓 0 处）：插件
   `writeWorkbenchState()` 的 `sessionId/terminalSequence` 持久化被 `?.` + 静默
   catch 吞掉，重挂载后 `initialState().sessionId` 永远为空，§8.8 的 attach
   路径从不命中；
3. 宿主 `openPluginConnection` 每次点击都 `workbenchId: crypto.randomUUID()`，
   且复用 tab 时整体替换 context——即使有 sessionId，sidecar
   `attach_session` 的 `connectionId+workbenchId` 双匹配也必扑空。

三层叠加的净效果：任何 remount 都走 `openSession()` 全新拨号（连接重置），
旧会话在 sidecar 里变僵尸。

**修复**（纯插件侧，自洽不依赖宿主改动）：
- `ssh.rs`：`SessionEntry.workbench_id` 改 `RwLock<String>`（内部可变）；
  `attach_session` 匹配放宽——先精确 `(connectionId, workbenchId)`，否则复用
  该连接的活会话并**重绑**到新 workbench（re-home，后续 close_workbench/list
  归属正确）；匹配逻辑抽纯函数 `pick_attach_target` + 单测；
- 前端 `initialize()`：持久化 sessionId 缺失时先 `ssh/sessions/list` 查该连接
  活会话（`lib/sessionRestore.ts` 纯函数 + spec：同 workbench 优先、createdAt
  最新、死会话忽略），命中则 `attachSession`（replay 恢复终端内容），否则照旧
  `openSession()`；attach 失败仍走既有退避梯子，梯尽 `openSession(true)` 兜底。

**验证**：cargo test **184 绿**（+attach 选择器）；vitest **96 绿**（+5 个
reattach 选择器用例）；typecheck 0 错、build 过；测试容器行为级验证 ALL
GREEN——wb-A 打开的活会话换 wb-B attach 返回同一 sessionId、sessions/list
确认 re-home、二次 attach 稳定、未知连接仍拒绝。

**说明**：七语不涉及（无新 UI 文案，错误全走既有降级路径）；改动生效需重新
打包安装插件（会重启 DBX，等用户窗口期执行）。**宿主侧遗留**（可选后续）：
① 桥补 workbenchState 实现（root fix，插件已兼容两种形态）；② 插件 tab 改
v-show/常驻可消除 iframe 重建闪屏（内存换体验，宿主设计决策）。

### §8.10 增补：SFTP 全局默认关 + 切 tab 闪屏宿主补丁（同日）

**SFTP 全局默认关**：`sanitizeSftpPaneDefaultOpen` 回退翻转（缺失/非法值不再
默认开，仅显式 "true" 开）——新工作台默认纯终端布局；`loadSftpPaneDefaultOpen`
的 catch 回退同步改 false；spec 断言更新。用户历史偏好仍生效（存过 "true" 就
开）。注意 workbenchState 在宿主桥未实现（§8.10 根因 2），工作台内的即时开关
只在本次 webview 存活期内有效。

**闪屏宿主补丁**（连接已由 reattach 保住，闪屏是 iframe 销毁重建的物理现象，
插件侧无解）：宿主 `ContentArea` 被 `:key` 的 KeepAlive 承载，切 tab 整树销毁
重建，iframe 移出 DOM 必然整页重载。按 DriverStorePage/PluginCenterPage 既有
`v-if+v-show` 常驻模式在宿主 App.vue 加常驻插件工作台图层，ContentArea 移除
plugin-workbench 分支（防双挂载）；`openPluginWorkbench` 复用 tab 不再替换
context（左侧菜单每次点击 mint 新 workbenchId，替换会重载 webview 并使会话
绑定失效）。详见 shared/PROGRESS-HOST-SUBREPO §11。验证：宿主 typecheck
0 错 + 相关 vitest 27 例全过（含新增 2 例）。

**生效路径**：插件重新打包安装（`frontend` 三件套已过，`scripts/build.sh` +
`scripts/install.sh --reinstall`）；宿主 `pnpm tauri build --debug` 重建
DBX.app。两者都会重启 DBX，待用户窗口期执行。

### §8.11 终端无输出修复：宿主桥二进制事件契约变更适配（2026-09-04）

**症状**：连接成功后终端零输出（无提示符、按键无回显），SFTP 面板正常。

**根因**（对照 ad76537 的终端改动排查，最终定位在宿主桥契约）：上游宿主
b15281024（随 DBX.app 0.6.2 于 09-04 08:37 生效）把沙箱 binary 事件从
`{ channel, dataBase64 }` 改为零拷贝 `{ channel, data: Uint8Array }`，
`dataBase64` 字段不复存在。插件 `handleBinary` 仍读 `event.dataBase64`（恒
undefined）→ `atob(undefined)` 抛异常 → 每个终端输出帧解码即炸，终端静默；
SFTP 浏览走 invoke（JSON 通道）不受影响，症状精确吻合。输入方向 sendBinary
新桥仍兼容 base64 字符串，故按键能发出、无回显。bug 逃过单测的原因：插件
自带 mockDbxHost 仍按旧形状投递，类型定义（env.d.ts）也是插件本地旧契约，
typecheck/单测全绿但与真实宿主脱节。

**修复**（纯插件侧，兼容新旧两种桥，符合 Host API 1.0 基线 optional 降级）：
- `lib/binaryEvent.ts`（新）：`bridgeBinaryBytes` 归一化两种形状——优先
  `data: Uint8Array`，回退 `decodeBase64(dataBase64)`；+3 spec 用例（新形状/
  旧形状/双缺失）；
- `App.vue`：两处 binary 消费（终端输出帧、SFTP 下载分块 waiter）改走归一化
  函数；
- `env.d.ts`：`dataBase64` 改 optional、新增 `data?`；
- `mockDbxHost.ts`：镜像当前宿主桥形状（`data` 字段），消除 mock 与现实脱节
  （本类 bug 的逃逸口）。

**验证**：typecheck 0 错；vitest **105 绿**（含 3 个新用例）；官方 installer
装 0.4.16（sha256 93719389…，previous 0.4.15）；对安装副本双冒烟 PASS——
smoke_test 全链路（连接→PTY 回显→SFTP→关闭）+ smoke_fs_test **45 PASS /
0 FAIL**。真实终端回显需在 DBX 里重开 SSH 连接人工确认。

**说明**：七语不涉及（无新文案）；版本 0.4.15→0.4.16（Cargo.toml/lock/
manifest）。**installer 重编**：host 子模块同步后上游依赖需 rustc≥1.94，用
本机 1.97.1 工具链 `cargo +1.97.1 build -p dbx-core --example
install_plugin --release` 重编（不动源码树/锁文件）。

**波及面提示**：files 插件前端 `handleBinary`（files/download/ 分块流）同样
消费 `event.dataBase64`，对 0.6.2 宿主有同样的失效风险，需同款适配（归
 files/ 并行会话处理，本轮未动）。

**§8.11 增补（同日收敛）**：`binaryEvent` 已上移 `shared/frontend/` 公共适配层
（与 files 同源单点维护），App.vue 改相对引用、`lib/binaryEvent.spec.ts` 保留
为引用 shared 的薄 spec（3 用例，验证本插件工具链解析/打包/行为）；插件内本地
副本删除。约定见 shared/frontend/README.zh-CN.md 与 AGENTS.md 硬性规则 7。
复验：typecheck 0 错、vitest 112 绿。纯等价重构，已装 0.4.16 行为不变，下次
构建自动带上 shared 源码。

## 2026-09-04 批量发送命令 + 全局快速命令（0.4.17 → 0.4.18）

**需求**：① 支持在多个打开的会话批量发送命令；② 快速命令原存工作台
localStorage，宿主 webview 存储按工作台分区 → 表现为"和连接绑定"，改为插件级
全局存储，沉淀公共脚本。

**契约**（详见 PROTOCOL 新节）：
- 新增 `ssh/quickCommands/list|save|delete`：全局快速命令 CRUD，存储
  `<data_dir>/quick-commands.json`（原子写 + 0600 + 坏文件降级，照抄
  quick-sudo-profiles 模式）；上限 20 条、name ≤60、command ≤500；save 返回
  完整清单供工作台直接采纳权威顺序，delete 对未知 id 回 `removed:false`。
- 新增 `ssh/terminal/batchInput`：`{sessionIds[], command, appendNewline?=true}`，
  把命令写入各会话 PTY（对齐批量发送语义：输出回显在各自终端、不收集
  远端输出），返回逐会话 `{results[{sessionId,success,error?}], sent, failed}`；
  会话不存在/队列满记目标级失败不整体报错；命令归一 `\n`→`\r`、上限 256 KiB。
- `ssh/sessions/list` 行**追加**只读展示字段 `host`/`port`/`username`
  （连接注册表解析，缺失回退空/22/空），供批量目标列表显示 `user@host`。

**实现**：
- 后端：新模块 `quick_commands.rs`（存储 + 校验 + 单测 7 个）；`ssh.rs` 增
  `batch_terminal_input` 及纯函数 `batch_input_payload`/`dedupe_session_ids`/
  `batch_input_row`，`session_info_payload` 加 `ConnectionEndpoint`；
  `main.rs` 注册 4 个方法臂。
- 前端：`lib/batchSend.ts` 纯函数（目标归一/标签/多选/快捷选择/结果汇总）+
  spec 7 用例；App.vue 工具栏批量发送按钮 + 弹窗（目标多选、当前会话预选、
  全选/仅存活、快速命令下拉回填、危险命令复用 `confirmRiskyPaste` 红色确认、
  逐会话发送结果）；快速命令 CRUD 改走后端 RPC，挂载时 `hydrateQuickCommands`
  一次性迁移 localStorage 旧数据后清除本地键，后端不可用回退旧语义。
- i18n：`batchSend*` 15 key + `quickCommandsGlobalHint`，七语全补。

**验证**：cargo test **194 绿**（含 batchInput 纯函数/未知目标聚合、
quick_commands roundtrip/坏文件/上限）；前端 typecheck 0 错、vitest **119 绿**
（含 workbench.spec 七语 key 对齐）、build 通过；smoke
`scripts/smoke_batch_quick_test.py` **10/10**（quickCommands CRUD 全链路 +
batchInput 真机 PTY 回显 marker 验证 + endpoint 字段），回归 smoke_test /
smoke_fs_test（45 PASS）/ smoke_batch3_test（17 PASS）全绿。

**说明**：多会话标签/分屏仍 deferred（宿主职责），但批量发送以跨连接会话为
目标集合已不受"单 workbench"限制（原批次对标文档 deferred 表已注记，该文档已随批次退役删除）。
快速命令旧 localStorage 键仅作迁移种子，删除逻辑保留七语不涉及新键。

**⚠️ 预存在问题（与本次改动无关，待专项排查）**：`scripts/test.sh` 全套验证在
`smoke_sudo_otp_test.py` 的 "same-window replay rotates to the second secret"
用例失败（同一 TOTP 窗口内第二次 sudo exec，shim 未观测到任何 OTP 提交，
报 "no OTP submission observed"）。A/B 定位：2026-08-29 构建的旧二进制
`dbx-plugin-ssh-sftp` 两跑全绿（10/10）；**HEAD 提交源码原样构建的基线二进制
同样失败**——回归介于 8/29 旧二进制与当前 HEAD 之间（0.4.15→0.4.17 的
sudo 时间戳/OTP 编排改动），先于本次批量/快速命令改动存在。本次任务不涉及
exec/sudo/OTP 代码路径（diff hunk 已复核）。test.sh 后续两步按其自身 SKIP
语义处理：mock UI walkthrough（本机无 playwright-core，自门禁 SKIP）、宿主
plugin_tools_bridge（WIP 未集成不编译，文档化 SKIP）。perf baseline 实测通过
（终端回显 57.8 MiB/s、上传 149 MB/s、下载 122 MB/s）。建议下轮专项：
对照 0.4.14→HEAD 的 exec.rs/sudo 编排 diff 定位同一窗口二次提交被跳过的根因。

## 2026-09-05 MCP 只读门禁安全加固（纯后端轮）

**需求**：对只读模式做安全审查（面向 MCP/AI 调用场景），修复发现的缺口：
① 白名单混入"形似只读、实可变更"的命令（`sort -o`、`find -fprint/-fprintf/-fls`
可写文件；`ip route flush`/`ip link set` 等深层变更；`git branch -D`/`tag -d`/
`remote add`/`reflog delete` 变更形态；`dmesg -c/-C/-n`、`history -c` 清理态）；
② 只读门禁按 connectionId 键控，独立 stdio 内联凭据重拨同一主机可绕过；
③ 只读连接上读路径无界，`cat ~/.ssh/id_rsa`、`.env`、`/etc/shadow` 等凭据
位置可直达（LLM 注入后凭只读连接偷凭据的现实威胁）；④ `sftp_download` 在
只读连接放行且本地落点任意 + `overwrite` 可覆盖本机引导文件（落地即代码执行）。

**实现**（`backend/src/mcp_safety.rs`、`backend/src/mcp.rs`）：
- 分类器收紧：`sort -o/--output`（含粘连形式）、`find -f…`（`-fprint/-fprintf/
  -fls` 等）、`dmesg -c/-C/-n/--console-level`、`history -c/-d/-a/-r/-w/-p/-s`
  一律 Unknown；`git` 移出通用子命令表，改为形态敏感的 `git_segment_risk`
  （`branch`/`tag` 仅列表形态放行，`remote` 拒变更子命令，`reflog` 仅
  无参/`show`）；`ip` 增第二层变更子命令检查（`add/del/delete/flush/set/
  change/replace/append`）。
- 新增敏感路径拒绝清单：`is_sensitive_path`（`.ssh/.gnupg/.aws/.kube` 目录、
  `id_*`/`ssh_host_*_key` 私钥、`*.pem/.key/.p12/.pfx`、`/etc/shadow`、
  `/etc/gshadow`、`/etc/sudoers`、`.env*`、`.netrc`、`.git-credentials`、
  `.npmrc`、`.htpasswd`、`.pgpass`、`my.cnf`、shell/mysql/psql history；
  `.pub` 公钥半边放行，尾部 `*` 通配参与 basename 匹配）。命中即把白名单
  命令降级 Unknown——只读连接拒绝、普通连接不受限，复用既有门禁语义。
- `call_tool` 门禁序变为四层（写门 → 白名单 → 敏感路径 → 灾难确认）；只读
  连接上 SFTP 读工具（`sftp_list_dir/read_file/stat/exists/download`）与
  `ssh_task_status` 的 `path/remotePath/logPath` 参数过同一拒绝清单
  （`sensitive_read_path`）。
- 只读判定按连接身份兜底：无 `connectionId` 的内联拨打按
  `host(ASCII case-insensitive) + port(缺省 22) + username` 与已注册只读
  连接比对（`inline_dial_is_registered_read_only`），重拨同一主机不绕过。
- `sftp_download` 本地落点拒绝清单 `is_sensitive_local_path`（任何连接生效，
  保护操作员本机）：`~/.ssh`、`~/.gnupg`、shell 启动文件、`authorized_keys`、
  `/etc/cron*`、`/var/spool/cron`、`/etc/systemd/system`、`/Library/Launch*`
  等；`sftp_download` 工具描述同步。
- 文档：`MCP.zh-CN.md` 门禁章节改为四层并补本地落点防护段；runInTerminal
  小节注记敏感路径清单先于路由生效。

**验证**：cargo test **201 绿**（新增 7 用例：`whitelisted_output_flags_are_
unknown`、`deep_subcommand_mutations_are_unknown`、`sensitive_paths_downgrade_
read_only`、`inline_dial_inherits_registered_read_only_gate`、
`read_only_tools_respect_sensitive_path_denylist`、
`exec_whitelist_refuses_sensitive_paths_on_read_only`、
`sftp_download_refuses_sensitive_local_targets`）；smoke_mcp **全绿**（只读段
扩展：8 个加固形态 + 4 个白名单放行形态 + 敏感路径 exec/SFTP 双路 + 本地
落点拒绝，均离线跑通，无真机段 SKIP 不变）。无 UI 改动，七语不涉及；无新
协议方法，`PROTOCOL.zh-CN.md` 不涉及。

**边界与遗留**：① 按连接的目录白名单（`readPaths` 允许前缀，收窄只读连接
的读范围）需表单/七语/前端配套，本轮先落"敏感路径拒绝清单"这一层，留作
后续可选增强；② 工作台协议 `ssh/exec` 非 sudo 命令仍无白名单（前端信任面，
与交互终端同权级），未纳入 MCP 门禁范围；③ `kubectl get secrets`、
`docker inspect`（容器环境变量）属集群级读取，路径类拒绝清单覆盖不到，
如需管控须在动词层另行处理；④ 模型仍可调用 `ssh_quick_sudo_profiles_save`/
`ssh_remove_known_host`/`mcp/settings/set` 修改操作员本机配置（属本地配置
语义而非远端只读范畴，是否纳入进程级只读开关待定）；⑤ `scripts/test.sh`
全套未跑（历史已知 smoke_sudo_otp 预存在问题与本轮无关，见上节），本轮按
"改哪层跑哪层"以 cargo test + smoke_mcp 验证。

**§UI 功能测试跑通（同日续）**：`scripts/smoke_ui_mock.mjs` 从锚点可见性升级为
真功能走查并全绿。前置：playwright-core 装到仓外 `/tmp/dbx-ui-mock`（项目
package.json 保持零新依赖），系统 Chrome 走 `channel:"chrome"` headless。
- **顺带修掉 mock 夹具一个真 bug**：mock 宿主 `ssh/session/attach` 缺正常分支
  （仅 `failSessionOpen` 抛错，其余落兜底 `{success:true}`），启动时
  `findReattachSession` → attach 的 sessionId 校验必败，工作台一直停在
  Error 态（"The attached SSH session changed unexpectedly"）——此前 walkthrough
  的"绿"只是错误态下锚点也可见。补齐 attach 正常分支（回显 sessionId +
  `replay.complete`，对齐真实 sidecar 重挂语义）后 mock 工作台真正 Connected。
- **功能断言新增**：快速命令全局弹层（全局提示/添加行/删除后清空，1/20 计数）；
  批量发送弹窗（目标行 `user@host`、Current/Read only 徽标、快速命令下拉回填
  草稿、发送后 "Sent to 1 session(s)" summary、mock 终端 PTY 回显命令）；
  截图 01-03 存 `docs/screenshots-ui-mock/`（*.png 已 gitignore）。
- 依赖门控顺手修正：原 `existsSync(...) || existsSync(...)` 不会触发 skip，
  改为显式判断。连跑两轮稳定 all green；typecheck 0 错、vitest 119 绿。

## 2026-09-05 每连接 sudo 命令白名单（sudoers 式，纯后端 + manifest 轮）

**需求**：连接级 sudo 目前"全有或全无"——Quick Sudo 开启后 `ssh_exec_sudo`
可跑任意特权命令（仅灾难门兜底）。参照 Linux sudoers 为连接增加特权命令
白名单：操作员声明允许的 sudo 命令模式，AI/MCP 调用命中才放行。

**契约**：
- 连接表单新字段 `sudo_whitelist`（`external_config.sudo_whitelist`，text，
  `visible_when: sudo_source ∈ {custom, global}`）：每行一条（`;` 分隔兼容
  单行输入，`#` 注释），如 `systemctl restart nginx`、`docker restart *`。
  manifest 七语 label 全补；空 = 门关闭（向后兼容）。
- 匹配语义（`sudo_allowlist.rs`，比真实 sudoers 严）：令牌精确匹配；`*`
  匹配一个参数；结尾 `*` 匹配剩余且须至少一个参数；不写通配 = 仅精确命令
  （反转 sudoers "不写参数=任意参数"的危险默认）；命令前导 `K=V` 赋值与一个
  `sudo` 令牌剥离后匹配，`sudo` 旗标（`-u` 等 run-as）不建模、永不匹配。

**实现**：
- 新模块 `sudo_allowlist.rs`：`parse_entries` / `entries_from_lines` /
  `command_tokens` / `is_allowed` / `render_entries`（拒绝信息回显允许模式，
  `sudo -l` 风格，LLM 可自我纠正），单测 5 个。
- `model.rs`：`StoredConnection.sudo_whitelist: Vec<String>`（原始行），
  `from_lifecycle_params` 解析 external_config；`JumpHost::to_connection`
  与 mcp 内联构造点补空默认。
- 门禁三处：① MCP `call_tool`——`ssh_exec_sudo` 及 `ssh_exec`/`ssh_run_bg`
  内联 `sudo …`（`runs_under_sudo` 检出，防 NOPASSWD/时间戳缓存绕过）在
  灾难门之前过白名单；② 工作台 `ssh/exec` `sudo: true` 经
  `SshRuntime::ensure_sudo_allowed` 过门；③ 内联凭据重拨同一主机按端点
  身份继承白名单（复用上轮 `registered_connection_matching_inline`，只读门
  同步收敛到该共享助手）。结构化 `sudo_fs`（工作台 sudo 文件面板，用户主动
  UI 动作）与只读连接的"全拒 sudo"语义不变。

**验证**：cargo test **207 绿**（新增 sudo_allowlist 5 用例 + mcp
`sudo_allowlist_gates_privileged_tools`：connectionId 命中/未命中、匹配放行、
内联 sudo 门、身份继承、异机不受限，并覆盖 external_config 解析）；release
构建通过；smoke_mcp 回归全绿（stdio 无生命周期注册、白名单门离线不可达，
由单测覆盖）。manifest JSON 结构校验通过（字段序
`sudo_use_pty → sudo_whitelist → read_only`）。前端不改（表单宿主渲染）。

**边界与遗留**：① 白名单按"命令文本"匹配，不解析 shell——包 wrapper
（`timeout 10 systemctl restart nginx`）或引号变形不命中即拒绝（保守方向，
如需放开再议）；② sudoers 的 run-as（`-u`）/NOEXEC 等高级语义未建模；
③ 工作台交互终端手敲 sudo 不受此门（与既定信任模型一致：白名单管 AI/MCP
执行面）；④ `scripts/test.sh` 全套未跑（同前述预存在问题），按层验证。
### §8.12 TOTP 多密钥跨调用轮换：进程级 OTP 台账 + 目标作用域（2026-09-04）

**问题**：OTP 轮换/防重放两本台账（`otp_usage` / `committed_totp`）原挂在
`SudoAuth` 实例字段上。终端与 `ssh/exec` 路径按会话共享实例所以正常；但 MCP
`ssh_exec_sudo` 每次调用经 `resolve_sudo_auth` **全新解析实例**
（`sudo_auth_for` + profile overlay / `sudo_auth(arguments)`），台账随实例
丢弃——同窗第二次调用重复提交第一个密钥已用掉的码（服务端必拒），配了多密钥
也不轮换。这正是 MCP 通道跑 sudo + 2FA 的日常路径。

**修复**（exec.rs，分支 `feat/ssh-totp-rotation`）：
- 两本台账改 sidecar **进程全局**（`OnceLock<Mutex<HashMap>>`），对齐
  服务级标记 OTP 已用的语义；
- 键控升级：`目标作用域(user@host:port) | 密钥 SHA-256 指纹(16hex，只存指纹)
  | 窗口 | 码`；作用域隔离保证共用同一密钥的多个连接互不吞码（A 机烧掉的码
  B 机仍可提交），同机跨调用/跨会话记账连续；
- 静态码 usage 键去掉时间戳分量（原 `now+30` 逐秒漂移，跨调用记账失效）；
- 选择算法对齐轮换验证码优先级排序语义（未用优先 → 剩余有效时长
  最长 → 配置顺序稳定兜底），替换原"首个剩余 ≥5s 未用项"简化循环；防重放
  硬跳过语义不变；
- 作用域由凭据解析点注入：`ssh.rs sudo_auth_for`（连接配置）与
  `mcp.rs sudo_auth`（内联参数，port 缺省 22）。

**测试**：`cargo test` 188 通过（基线 184 + 新增 4：
`otp_rotation_spans_separate_instances`（红→绿，本修复主回归）、
`committed_replay_guard_spans_separate_instances`、
`static_code_usage_keys_do_not_drift_across_calls`、
`otp_ledger_marks_do_not_leak_across_targets`）。台账全局化后并行单测需隔离：
新增 `otp_ledger_test_guard()`（进入清空 + 持锁串行），触碰台账的用例全部
套上；`keyboard_interactive_answers_follow_flow_mode` 的 password+otp 组合
段改用独立静态码——全局防重放正确拦截了同进程内同窗二次注入，属预期新行为。

**真机**：`smoke_sudo_otp_test.py` 对 dbx-ssh-test 容器复跑 10 passed /
0 skipped / 0 failed（同窗轮换第二密钥、第三次硬跳过、错误密钥拒绝等全过）。

**文档**：PROTOCOL.zh-CN.md Quick Sudo 段（台账进程全局 + 作用域/指纹键控 +
排序语义）、MCP.zh-CN.md 工具表、`totpSecret` 工具 schema 描述补多密钥
（换行/分号分隔）轮换语义。

## 2026-09-05 启动恢复插件 tab 自动重连（boot 恢复路径 inactive 重试放开）

宿主侧已为启动恢复的插件 tab 重放连接生命周期（host/queryStore 新增
`reconnectRestoredPluginTabs`，见 shared/PROGRESS-HOST-SUBREPO 第 18 节）。
配套放宽本插件此前「inactive 立即失败」的假设：

- `openSession(forceNew, bootRestore)`：boot 恢复路径（`onMounted` 的
  会话打开分支）传 `bootRestore=true`，`Connection is not active` 与其它
  快失败一起走原有 3 次（2/4/6s）有界重试——宿主重放的 connect 落地后
  下一次 open 即自愈；重试耗尽仍回落七语 `connectionInactive` 指引。
- 非 boot 路径（`reconnect`/`reconnectNow`/掉线重连）保持 inactive 立即
  失败不变：连接确实没被宿主建立时重试不可能成功。
- 无新增用户可见文案（七语无改动）。
- 验证：`vue-tsc` 0 错；`vitest run` 12 文件 119 用例全绿。真机复验随
  host 第 18 节待办一并执行。

## 主题令牌桥：首绘同步与全覆盖（2026-09-05）

- 接入 `shared/frontend/themeSync.ts`（单点实现）：`main.ts` 挂载前
  `installHostThemeBridge()`，把 `--background/--primary/--radius/--*-font-family`
  等插件变量声明为宿主 `--color-*`/`--radius-*`/`--font-*` 令牌引用。首绘即命中
  宿主主题（此前等 init 后 JS 回写，亮色宿主下首绘落在 CSS 暗色默认）；宿主
  切主题时随 SDK 令牌更新自动跟随；primary/radius/字体首次纳入同步面。
- JS `applyAppearance` 保留（终端 ANSI 调色板等非 CSS 场景仍需）。
- 验证：`vue-tsc` 0 错；`vitest run` 13 文件 122 用例全绿（含新增
  `themeSync.spec.ts` 薄 spec）；v0.4.24 发版真机亮色主题下工作台首绘同步。

## 白色主题配色标准化（2026-09-05 第二轮）

四插件联合审查白色主题配色错误，语义令牌与明暗分支在
`shared/frontend/themeSync.ts` 单点收敛，插件只消费变量。

- 桥新增语义状态色 `--success`/`--success-bg`/`--warning`/`--warning-bg`
  （跟随宿主 `--color-success*`/`--color-warning*`；Host API 1.0/mock 缺令牌时
  light 分支回退宿主 tokens.css 规范值 rgb(22 163 74)/rgb(217 119 6)，暗色回退
  rgb(74 222 128)/rgb(251 191 36)）与模态遮罩 `--overlay`（亮色黑 40%，
  暗色 `color-mix(var(--background) 70%, transparent)`）。
- 明暗 CSS 分支统一双属性匹配：`:root[data-theme=…]`（插件 applyAppearance）+
  `:root[data-dbx-theme=…]`（宿主 SDK applyTheme），谁先到都生效，消除
  waitForHostApi 轮询窗口期的错配。
- ssh 本轮替换：会话状态徽章（#22c55e/#eab308/#f97316/#ef4444 → 语义令牌，
  connecting 黄色白底 1.9:1 不可读问题一并解决）、重连横幅（#f97316 →
  `--warning`）、`.folder-icon`（#e6ad52 → `--warning`，白底可读）、
  modal 遮罩（25%/dark 分支 → `--overlay`）、终端空态 SVG 硬编码深色
  （#111827/#1f2937/#60a5fa/#86efac/#e5e7eb → 主题变量内联 style）、
  `.terminal-command-marker.failed` → `--destructive`、图标 dark 变体双属性化。
- 验证：`vue-tsc` 0 错；`vitest run` 13 文件 123 用例全绿（themeSync 薄 spec
  增补语义令牌/遮罩/light 回退断言）。无新增文案，七语不受影响。

## 主题配色第三轮：progress 着色、预览选区、mock 桥接对齐（2026-09-05）

继续收敛残留的非令牌化配色与一处 fixture 漂移，改动均为前端单层：

- **metrics 磁盘警戒红从未生效**：`.disk-row progress.disk-warn::-progress-value`
  是无效伪元素（各家实为 `::-webkit-progress-value` / `::-moz-progress-bar`），
  任何浏览器都不匹配，磁盘 ≥85% 转红的意图从未渲染；同时磁盘/网络行 progress
  无 `accent-color`，走浏览器默认蓝。改为 `accent-color: var(--primary)` +
  `.disk-warn { accent-color: var(--destructive) }`（progress 跨浏览器唯一可靠
  着色点）。重连横幅 progress 补 `accent-color: var(--warning)`。
- **metrics 悬浮卡进程表头色差**：复用的全局 `.file-header` 带 `var(--background)`
  底色，在 popover 底色卡片上显出一块色差；加 `.metrics-float .file-header`
  上下文覆盖为 `var(--popover)`。
- **文本预览选区**：`TextPreview.vue` CodeMirror 选区色由单值 `#5f7aa855` 改为随
  colorScheme 切换（dark `#5f6f8a88` / light `#93b4e088`），与 xterm
  `selectionBackground` 同一观感。
- **mock.html 漏装主题桥**（规约第 7 条"mock 镜像真实桥形状"）：mock.html 自行
  复制了 main.ts 引导却缺 `installHostThemeBridge()`，导致 mock 中
  `--success/--warning/--overlay` 全部未定义——第二轮引入的语义令牌（连接状态点、
  文件夹图标、重连横幅、模态遮罩）在 mock 里根本显示不出来，视觉验证与生产脱节。
  改为 mock.html 直接 `import "./src/main.ts"`（与 visual.html 一致），从根上消除
  这类漂移。`mockDbxHost` 磁盘 fixture 把 `/data` 调到 87%，让 disk-warn 两态
  可被视觉验证覆盖。
- 验证：`vue-tsc` 0 错；`vitest run` 13 文件 123 用例全绿；
  `scripts/smoke_ui_mock.mjs` 全绿。无头 Chromium 对 `mock.html?theme=light|dark`
  逐项读计算样式并截图：磁盘 48%/0% 行 accent 为 `--primary`，87% 行为
  `--destructive`（light rgb(231 0 11) / dark rgb(243 98 95)）；`--warning`
  解析为 light rgb(217 119 6) / dark rgb(251 191 36)，重连横幅 progress accent
  跟随；连接绿点 `--success`（rgb(22 163 74) / rgb(74 222 128)）、文件夹图标
  `--warning` 两主题均正确。无新增文案，七语不受影响。
- 已知 fixture 局限（未改）：mock 启动路径经 `ssh/sessions/list` 附着既有会话、
  不调用 `ssh/session/open`，`?err=disconnect` 掉线计时器不触发，重连横幅只能靠
  计算样式探针验证而非全流程截图。

## 批量发送 "The object can not be cloned."（2026-09-06，根因在宿主桥）

真机批量发送报 WebKit DataCloneError。根因：`sendBatchCommand` 把
`batchSelected.value`（Vue 响应式 Proxy 数组）直接传 `dbxPlugin.invoke`，宿主
注入 SDK 的 `request()` 裸 `parent.postMessage`，Proxy 无法 structured clone。
修复在宿主注入 SDK 单点（params 普通化：structuredClone 优先、JSON 往返兜底），
四插件全部 invoke 调用点一并治愈；详见
`shared/PROGRESS-HOST-SUBREPO.zh-CN.md` §21。宿主重建后生效，本插件无代码
改动、无需重发包；mock 页因无 postMessage 克隆边界而不可复现该问题。

## UI 扫描 P1 修复：连接失败错误呈现 + 弹层焦点管理（2026-09-06）

第 1 轮场景化 UI 扫描（`docs/UI_SCAN_FINDINGS.zh-CN.md`）两条 P1 与顺手项 P2-3 的修复轮。
仅触碰 `frontend/` 内文件：`src/App.vue`、`src/lib/i18n.ts`（七语补键）、
`src/lib/connectRetry.ts` + `connectRetry.spec.ts`（新建）、`src/lib/modalFocus.ts` +
`modalFocus.spec.ts`（新建）。未提交 git、未动依赖；kafka/shared 仅读参考未改动。

- **P1-1 认证失败无限重试**：根因是 `openSession()` 每次重入把 `openRetryAttempt`
  归零，`OPEN_RETRY_MAX=3` 永远打不满，秒级失败的认证错误无限转圈、错误文案永不呈现。
  修复：`openSession` 增加 `isRetry` 参数，重试定时器重入保留计数、仅新入口归零；
  重试决策抽为纯函数 `connectRetry.decideConnectRetry()`（backoff 2s·N、单次 ≥8s 快失败
  窗口、inactive 非 boot 即败、auth/hostKey 永久错误跳过重试立即进 error 态）。
  error 态复用现有 overlay：`connectError.*` friendly 七语文案 + Reconnect 按钮（出口既有）。
- **P1-2 弹层焦点管理**：原生 autofocus 对 Vue 动态插入 DOM 无效，全部弹层焦点不进入、
  Tab 逸出、关闭落 BODY。修复：ssh 内落地 `modalFocus.ts` 纯函数
  （focusableElements / nextFocusIndex / decideModalKeydown / pickModalFocusTarget），
  App.vue 弹层开状态计数 watch 驱动"打开聚焦首控件（autofocus 属性改作定位提示）/
  关闭归还触发元素"，触发元素栈与嵌套深度同步 push/pop；右键菜单项这类打开后即卸载的
  瞬态触发元素不可承接归还，`focusin` 跟踪"弹层/右键菜单外最近稳定焦点"作回退目标；
  `onDocumentKeydown` 增加 Tab 焦点陷阱分支（无弹层不拦截，终端 Tab 穿透不受影响），
  Esc 关闭链原样保留。host-key/agent 审批安全弹窗参与聚焦与陷阱、但不参与 Esc 关闭。
- **P2-3 host-key 弹窗文案 i18n**：`Verify SSH host key` 等 5 处硬编码英文改走
  `hostKeyDialog.*` 七语 8 键（en/es/it/ja/pt-BR/zh-CN/zh-TW）。

验证：`pnpm typecheck` 0 错；`pnpm test` 19 文件 182 用例全绿（新增 connectRetry 7 +
modalFocus 9；既有七语键集合/占位符一致性测试自动看护新键）。浏览器复验
（vite :5291 + playwright-core + 系统 Chrome，走查脚本与截图均未入库）：
`?err=authfail` 于 t+0.5s/8s/15s 三观察点稳定呈现 friendly 文案 + Reconnect 出口、
无转圈、点击有响应；`?rw=1` 下命令对话框/批量发送/删除确认三类弹层 16 项检查全绿
（首控件聚焦、6×Tab + Shift+Tab 不出弹层、Esc 关闭后焦点归还触发按钮，删除确认不再落 BODY）。
遗留：P1-1 的真机 sidecar 认证失败重试节奏复核建议随下次真机轮补做；P2-1/P2-2/P2-4/P2-5/P2-6 夹具与打磨项未在本轮处理。

## UI 扫描第 2 轮清理：全部 P2 收口（2026-09-06）

第 1 轮 UI 扫描剩余 P2（P2-1/P2-2/P2-4/P2-5/P2-6）的清理轮。仅触碰
`frontend/` 内文件：`src/mockDbxHost.ts`、`mock.html`、`src/App.vue`、
`src/lib/toolbarTint.ts` + `toolbarTint.spec.ts`（新建）、
`src/mockDbxHost.spec.ts`（新建）。未提交 git、未动依赖；无新增用户可见
文案，七语不受影响。

- **P2-1 `?err=disconnect` 默认启动失效**：断开注入抽为 `scheduleDisconnect()`，
  open 与 attach（默认 reattach 启动路径）完成会话后都调用，全局单发不复发。
  直接打开 `mock.html?err=disconnect` 约 4s 后横幅自动出现，自动重连后恢复。
- **P2-2 默认首屏终端仅一行 prompt**：open 与 attach 共用同一份
  `terminalTranscript`（Welcome + OSC 633 周期 + prompt），attach 完成后推送，
  默认首屏即含 Welcome、命令回显与 command-marker 条（"Shell integration active"），
  command-marker 视觉验证不再依赖手动重连。
- **P2-4 light 主题工具栏染色对比度**：染色收口为 `lib/toolbarTint.ts`
  `toolbarTintStyle()` 纯函数（App.vue 原 `colorWithAlpha` 内联逻辑迁入），
  dark 保持 10%/18%，light 压到 4%/8%。浏览器实测 light 下会话 pill 对比度
  4.53:1（修复前 ≈4.22:1，AA 达标）。
- **P2-5 `?mock=1` 过时注释**：更正为"无条件生效，无开关参数"。
- **P2-6 dev 首载 404 噪音**：mock.html `<head>` 补 `data:` 占位 favicon，
  首载 0 条 4xx 资源请求、console 干净。

验证：`pnpm typecheck` 0 错；`pnpm test` 21 文件 191 用例全绿（新增
toolbarTint 6（含 WCAG ≥4.5:1 回归口径）+ mockDbxHost 3（happy-dom +
fake timers 锁定 attach 回放与断开单发语义））。浏览器复验（vite :5291 +
playwright-core + 系统 Chrome，10/10 项通过；走查脚本装于
`/tmp/uiscan-ssh-r2` 不入库，截图已删）：默认首屏 Welcome/OSC 633/marker 条、
`?err=disconnect` attach 路径自动断开→横幅→重连恢复、light 染色 alpha 与
pill 对比度 4.53:1、dark 染色不变、404/console 零噪音。本次顺带解除此前
PROGRESS 记录的"fixture 局限：重连横幅只能靠计算样式探针验证"——现在可全流程
浏览器验证。第 1 轮 P1-1 的真机 sidecar 认证失败重试节奏复核遗留项不变。

## UI 扫描第 3 轮修复收口（接手中断半成品）+ 第 4 轮复验记录（2026-09-06）

第 3 轮专家深度测试（`UI_SCAN_FINDINGS.zh-CN.md` 第五章，P1×3 + P2×7）的修复
由前一 agent 发起后中断。本轮接手做逐条审计：`src/lib/` 下
ghostClickGuard / requestEpoch / remotePathInput / sftpRename（另有同批
sftpEntries / fileRowKeydown / 既有 toolbarTint）**均已完成实现、App.vue/
DirTree.vue 接线、配套 spec 与七语文案**，完成度高于中断时预期，10/10 无需
补代码；本轮工作为全量回归验证 + 浏览器复验 + 文档收口。仅触碰
`frontend/` 内文件与两份 docs，未提交 git、未动依赖。

- **R3-P1-1 幽灵点击重开弹层**：`lib/ghostClickGuard.ts`（400ms 抑制窗 +
  mousedown 前驱判定，真实鼠标点击放行）；焦点归还前 arm，document capture
  级监听拦截合成 click。7 条单测。
- **R3-P1-2 / R3-P2-1 Esc 取消仍提交 + Enter 双发**：`lib/sftpRename.ts`
  `shouldCommitRename`（editingPath 指向校验 + submitting 分支）；Esc/Enter
  先清 renamingPath，卸载引发的幽灵 blur 被短路。6 条单测。
- **R3-P1-3 目录加载竞态**：`lib/requestEpoch.ts`；loadDirectory 取单调序号，
  落地/catch/finally 三处 isCurrent 校验，过期响应整体丢弃。4 条单测。
- **R3-P2-2 重命名冲突预检**：commitRename 增加 sftp/exists 预检 +
  window.confirm（七语新 key `sftpRename.overwriteConfirm`），失败路径关编辑
  态并刷新；mock `sftp/rename` 收口为原子语义（撞名不丢源）+ 2 条夹具单测。
- **R3-P2-3 坏响应防御**：`lib/sftpEntries.ts` `sanitizeSftpEntries`（非数组
  → 空数组、坏行丢弃、缺 kind 降级 file），loadDirectory 落地前统一过
  sanitize。7 条单测。
- **R3-P2-4 路径栏 `~`/`..`**：`lib/remotePathInput.ts`
  `resolveRemotePath`（home 展开 + `..` 消解 + 基础归一），路径栏 Enter 走
  `submitPathInput()`。9 条单测。
- **R3-P2-5 键盘打开**：`lib/fileRowKeydown.ts` `decideFileRowAction`，文件行
  Enter 打开 / F2 重命名 / Delete 删除（只读连接仅 Enter）。4 条单测。
- **R3-P2-6 树键盘 + caret 名**：DirTree.vue 加 role=treeitem、aria-expanded、
  roving tabindex、Enter/Space/方向键导航；caret 补 aria-label（七语
  `sftpSide.expandNode/collapseNode`）。组件测试 +5 条。
- **R3-P2-7 zh-TW「批次」**：i18n zh-TW batchSend* 全组「批量」→「批次」，
  zh-CN 不变。

验证：`pnpm typecheck` 0 错；`pnpm test` 27 文件 **237 用例全绿**（第 2 轮
基线 191 + 新增 46）。浏览器复验（vite :5291 + playwright-core + 系统
Chrome headless，`/tmp/uiscan-ssh-r4b` 不入库）：按第 3 轮复现步骤逐条复验
**11/11 PASS**（10 条 + P2-2 取消路径变体）——mkdir 键盘 Enter 一次成功关闭
不重开、Esc 取消重命名 0 invoke、慢 /etc 竞态不回跳、Enter 重命名单发无假
横幅、撞名 confirm 接受/取消两路均收敛、entries:null/畸形行 0 pageerror、
`~` 与 `..` 正确消解、键盘 Enter 逐级进目录、树 6/6 treeitem + 0 无名
caret + 方向键移焦、zh-TW「批次」在位。复验截图 0 张留存（失败才截图，
调试图已删）。新增用户可见文案七语齐（en/zh-CN/zh-TW/es/it/ja/pt-BR）。
遗留：R3-P1-1 的宿主真实 webview 复核建议保留；zmodem/拖拽上传 headless
不可达（沿袭第 1 轮）。

## tssh 对标特性追赶：trz/tsz + SetEnv/RemoteCommand（0.4.34，2026-09-07）

> 分支 `feat/ssh-tssh-parity`（worktree `.worktrees/feat-ssh-tssh-parity`，
> 对标参照 [trzsz-ssh](https://github.com/trzsz/trzsz-ssh)）。并发双工作包
> （前端/后端文件所有权不相交）+ 主会话集中收口。**范围决策（用户，
> 2026-09-07）：转发类特性不做**——端口转发（-L/-R/-D）与 Agent 转发
> （ForwardAgent）均否掉，理由：宿主已有 ssh 隧道实现（连接代拨模型，
> `dbx-core/src/db/ssh_tunnel.rs`）；宿主能力盘点结论（无 -R、无面向用户的
> 转发会话 UI、插件桥无任意 host:port 转发接口）已存档
> `FEATURE_PARITY.zh-CN.md` tssh 补充节。后端工作包原含 Agent 转发，中途
> 按决策裁剪：其 model.rs 半成品被主会话收编，剥离 `forward_agent` 字段，
> 保留 SetEnv/RemoteCommand 两字段解析与契约测试。

**工作包 B（前端，trz/tsz 文件传输）**：引入 `trzsz` 1.1.6（trzsz.js 官方
JS 实现，MIT）——**本插件前端首个依赖豁免**，理由：协议帧收发/转义/tmux
兼容/MD5 校验全在包内，自研等于重写协议；连带 `tsconfig.json` 加
`skipLibCheck`（包内 d.ts 引用了未装 scope 的 `xterm` 类型，本项目用
`@xterm/xterm`，skipLibCheck 是不动第三方文件的最小解法）。集成形态：
`TrzszFilter` 流式挂接（不用绑 WebSocket 的 TrzszAddon）——PTY 下行帧经
`processServerOutput` 空闲透传 + announce 扫描（`::TRZSZ:TRANSFER:`），
传输中输入接管（Ctrl+C 停传）；`sendToServer` 走现有 8 字节序号前缀输入
路径；与 zmodem 共存互斥（`terminalInteraction.ts` 的占用守卫统一为
`transferBusy`）；实例级覆写 `handleTrzszUploadFiles/handleTrzszDownloadFiles`
（浏览器构建硬编码 File System Access API，WKWebView 沙箱没有）——上传走
隐藏 `<input type=file multiple>`，下载缓冲成 Blob 后宿主 `fileTransfer`
优先、`<a download>` 兜底；进度 overlay（单/多文件 i/N、百分比、速度、
取消）；右键菜单「Upload (trz)」向 PTY 发 `trz\r` 触发远端，5s 未响应报错
+15s 看门狗。i18n 七语各 +8 key（trzszUpload/trzszWaiting/trzszUploading/
trzszDownloading/trzszComplete/trzszFailed/trzszNotAvailable/
trzszCancelled）。新增 `lib/terminalTrzsz.ts` + 26 条纯函数单测。

**工作包 A（后端，SetEnv + RemoteCommand）**：
- 连接表单新增 `setEnv`（textarea，多行 `KEY=VALUE`，分号兼容；严格校验：
  非法条目聚合报错连接失败，"宁可连不上也不错配"；重复 key 后者覆盖）与
  `remoteCommand`（text，trim 非空生效），七语 label/description/placeholder
  齐，位于 `keepalive_interval_secs` 与 `sudo_source` 之间；三个
  manifest↔parser 契约测试为此转绿。
- SetEnv 注入点：交互 shell 通道（`open_session` PTY 后、shell/exec 前）+
  `exec_plain`/`exec_with_sudo` 两分支（`exec.rs` 新增纯函数
  `merge_channel_env`：内置默认 best-effort、用户条目 strict 且同名覆盖，
  每变量恰请求一次；与既有 `SUDO_ASKPASS` 清空共存，用户值优先）。russh
  `set_env` 为 fire-and-forget，服务端无 `AcceptEnv` 时静默不生效——与
  ssh(1) 同语义，PROTOCOL 文档已注明需服务端配合。
- RemoteCommand：`open_session` 中非空时 `exec` 替代 `request_shell`
  （PTY 照常）；命令退出即会话终止（与 `ssh host command` 一致）；
  reattach 重放属预期；MCP/sudo/replay 路径零改动（`mcp.rs` 直构连接点补
  空默认）。JumpHost 显式不继承两字段（只作用于最终会话）。
- smoke_test.py 新增两用例：exec 通道 `echo $DBX_SMOKE_ENV` 实测回显、
  remoteCommand 会话回放含标记输出（测试容器 sshd_config 追加
  `AcceptEnv DBX_SMOKE_ENV` 并重启，仅测试容器可逆改动）。
- PROTOCOL.zh-CN.md 同步：RPC 表、连接字段、新章节「会话环境与会话命令
  （SetEnv / RemoteCommand）」。

**验证（0.4.34，`scripts/test.sh --skip-host` + 手动补跑尾两步）**：
cargo test **214/214**（基线 211 + 3 契约转绿 + 3 merge 单测）；前端
typecheck 0 错、vitest **263/263**（基线 237 + 26）、build 过；release 构建
+ .dbxp 打包（0.4.34）+ MCP stdio smoke 过；live smoke：smoke_test PASS
（含新 setEnv/remoteCommand 用例）、smoke_fs **45/45**、smoke_batch3
**17/17**、perf 基线过（upload 网络 160 MB/s 量级）、mock UI walkthrough
全绿。smoke_sudo_otp **8 passed/1 skipped/1 failed**——失败用例
"same-window replay rotates to the second secret" 为**文档在案的存量回归**
（0.4.15→0.4.17 sudo 时间戳/OTP 编排改动引入，见上文 ⚠️ 预存在段落），
本轮 A/B 复核：已安装 0.4.33 副本同样失败、本分支构建同样失败——与本轮
改动无关，专项排查遗留。test.sh 因此在该步中止（set -e），尾两步
（perf/UI mock）已手动补跑通过。

**合并注意**：主工作区存在未提交的 0.4.32→0.4.33 版本号 bump
（manifest.json/Cargo.toml/Cargo.lock）与 UI_SCAN_FINDINGS 文档更新；
本分支已 bump **0.4.34**（越过 0.4.33），合并时版本行以本分支为准，
UI_SCAN 文档改动与本分支无交集可并行保留。worktree 内 `host` 为指向主
工作区子模块的符号链接（path 依赖所需），呈现为 typechange，勿提交。

**遗留**：① trz/tsz 真机实流验证（对装了 trz/tsz 的测试容器跑 `trz`/`tsz`
全流程 + WKWebView 真机 file picker/cancel 行为）——本轮 headless 无法
构造，机制层有 26 条单测 + announce 看门狗兜底；② remoteCommand 命令退出
即断开的产品语义是否保留（备选：退出后回 shell 或提示重连）待用户定；
③ smoke_sudo_otp 存量回归专项排查（归档在案，非本轮引入）；④ setEnv 在
默认 sshd 上需 `AcceptEnv` 配合，文档已注明。
### §8.13 设置弹窗/配置编辑器回显已存原值（2026-09-04）

**问题**：Quick Sudo 设置弹窗与全局配置编辑器的 sudo 密码 / TOTP 密钥输入框
打开时永远空白，仅靠占位符提示"已配置"——用户看不到自己存的原值
（`settings/get` 与 `profile_view` 只回布尔位，设计上从不回显），多密钥
原文（换行/分号串）更是完全无处可查。

**修复**（回显仍是显式、有边界的）：
- `ssh/settings/get` 新增可选 `revealSecrets: true` → 额外回显本连接配置的
  `sudoPassword` / `totpSecret` 原始串；缺省响应与此前完全一致（布尔位），
  MCP 通道不暴露该参数。设置弹窗 `openSettings` 传参预填两个 draft 字段；
  `refreshSettingsMeta` 保持不回显。
- 新增 `sudo/profiles/reveal { id }`（工作台专用，**不进 MCP 工具面**，密钥
  不进 agent 上下文）：返回完整视图含原值；未知 id 报错。配置编辑器
  `startProfileEdit` 在有已存密钥时异步 reveal 预填（带 id/编辑态守卫，
  失败回落占位提示）。保存语义不变：空串/清空字段=保持原值，清除走既有
  清除按钮 / clear 标志。
- `mockDbxHost.ts` 镜像两个方法当前形状（reveal 回空串壳）。

**测试**：`cargo test` 189 通过（新增 `reveal_returns_raw_secrets_for_the_
editor`：原值回显 + 未知 id 报错；`views_never_echo_secrets` 证明 list 视图
仍不回显）。前端 typecheck 0 错、vitest 102 过、build 成功。smoke：
`smoke_sudo_otp_test.py` 新增 `revealSecrets` 用例（默认不回显断言保留 +
reveal 回显多密钥原文）；`smoke_fs_test.py` 新增 `sudo/profiles/reveal`
用例（原值返回 + 未知 id 报错 + list 仍 flag-only）。

**文档**：PROTOCOL.zh-CN.md（`ssh/settings/get` revealSecrets 参数、
`sudo/profiles/reveal` 方法条目及其"仅工作台、不进 MCP"边界）。宿主渲染的
连接表单 `totp_secret` 字段属宿主表单体系，不受本插件控制，不在本轮范围。

### §8.14 修复：OTP 预注入后 watcher 重复应答同一提示，烧掉第二密钥的码（2026-09-05，合并最新 master 后）

**合并 master（kafka、ssh 批量快捷命令 + sudo allowlist、shared/frontend
适配层）后真机 smoke 暴露新问题**：`same-window replay rotates to the
second secret` FAIL——同窗第二次 sudo 无任何 OTP 提交，防重放台账把两个
密钥的码都记为已提交。

**根因**（stderr trace + 提交日志双证）：`exec_with_sudo` Phase 1 把密码与
OTP 码一起预注入 stdin（`totp_answer_logged` → take #1，烧 secret_a 的码），
但 Phase 2 watcher 的 `otp_answered` 标志仍是 false——shim/PAM 打出的**同
一个** "Verification code:" 提示被 watcher 当作新提示再次应答（take #2，
轮换选中 secret_b 的码），写入的码无人消费、直接废弃，但 usage/committed
双台账已标记。同窗第二次 sudo 时 a、b 两码均已 committed，selection fallback
选中已提交码被防重放守卫拦截——轮换语义失效。

**修复**：`PromptContext` 增加 `otp_piped` 标志，Phase 1 成功预注入 OTP 码
时 watcher 的 `otp_answered` 初始为 true（同一提示不再二次应答；预注入被
防重放跳过时保持 false，后续真实提示照常应答）。

**验证**：cargo test 212 通过；前端 typecheck 0 错 / vitest 119 过 /
build 成功（合并 master 后全量复验）；真机 smoke：otp 11 passed / 0 failed
（同窗轮换、第三次硬跳过、错误密钥拒绝、revealSecrets 回显全过），
fs 46 passed / 0 skipped / 0 failed（含 `sudo/profiles/reveal` 新用例）。

**排障基建**：`smoke_sudo_otp_test.py` FAIL 时打印完整提交日志（定位
"码谁烧的"）与 sidecar stderr 过滤尾（进程退出后 drain，避免管道阻塞）。
另注：sidecar 启动依赖可执行文件名 `dbx-plugin-ssh`——非同名副本无法
initialize（sidecar closed），smoke 直连二进制排障时需保持原名。

### §8.15 MCP 本地传输根约束：sftp_upload/download 路径穿越修复（2026-09-07，合并 totp-rotation 轮）

**背景**：合并 `feat/ssh-totp-rotation` 的收尾 commit 被 Mimosa L3 门槛拦截
（6 个 high：mcp.rs 1311/1372 为 sftp 传输工具真实暴露面，945/949/1819/1820
为 sudo_auth 误报，见下）。经用户决策走"先修复再合并"路径。

**修复（真实问题，sftp 两条）**：agent 可指定任意本地路径读（upload 装箱
外送）写（download 落盘），原仅有下载侧敏感路径黑名单。新增本地传输根
约束（`mcp.rs` `local_transfer_roots_for` / `ensure_local_transfer_allowed_in`）：

- `mcp/settings` 新增 `localTransferRoot`（绝对路径或空串；持久化进
  mcp-settings.json）。配置后允许根=该目录；未配置默认=系统临时目录 +
  插件数据目录。canonical 化后 `starts_with` 判定（macOS `/var` 别名不漏）。
- 敏感路径黑名单（`.ssh`/`.gnupg`/shell 启动文件/引导执行路径）升级为
  **双向、任何模式叠加**——upload 侧首次获得防凭据外传校验，配置根内同样
  拦截。
- 配置根不可解析时报错而非静默回落；`mcp/settings/set` 是操作者协议面，
  `mcp/tools`/`mcp/call`（agent 面）不可达——**agent 无法自我扩根**。
- 单测 +5：门槛约束/黑名单叠加/默认根解析/配置校验/持久化 roundtrip
  （220 全过）。

**误报论证（sudo_auth 四条，链 `sudo_auth → new → load(sink:path-traversal)`）**：
`SudoAuth::new` 全链纯字符串处理（`exec.rs` 全文件零 `fs::` 调用），
`sudo_auth()` 的输入是凭据串，可达的 `load`（McpLimits::load /
sudo_profiles::load_store）入参均为 data_dir 固定路径——污点链为扫描器对
泛型名 `new`/`load` 的跨函数混淆，扫描器自身标注 "静态 advisory 需人工确认"。
未为此改代码（改即迎合误报）；如重扫仍报，需在门槛侧按误报处置。

**遗留**：smoke_mcp.py 的门槛拒绝用例（仓库外根双向拒绝 + 根内敏感路径
拒绝）因 Mimosa Edit 钩子对该文件的幻影误报（引证 `../`，实测全文零匹配）
连续拦截写入而暂缓；门槛逻辑已由单测全覆盖，smoke 用例待钩子侧澄清后补。

## 0.4.35 安装被宿主兼容性校验拦截：setEnv/remoteCommand 表单 key 改 snake_case（2026-09-07）

**现象**：`install.sh` 装 0.4.35 连续两次失败（重编 installer 后复现一致，
非构建缓存坑）——宿主 `dbx-core/src/plugins/manifest.rs` 报
`Contribution at index 0 field 11/12 has an invalid or duplicate key` +
六语 localization（es/it/ja/pt-BR/zh-CN/zh-TW）的
`io.dbx.ssh.connection/setEnv|remoteCommand` invalid field entry。

**根因**：0.4.34 tssh 对标轮把两个连接表单字段 key 起成了 camelCase
（`setEnv`/`remoteCommand`），而宿主 `valid_identifier` 只允许小写字母/数字
加 `.`/`-`/`_`——不允许大写。打包期未拦截（CLI 不跑宿主兼容校验），
安装期才爆。en 未被点名是因为 en 文案走字段定义默认 label，本就没有
contribution fields 条目。

**修复**：manifest 字段 key 与六语条目统一改 `set_env`/`remote_command`
（sidecar 解析本就双名兼容 `model.rs` `["setEnv", "set_env"]`，存量 camelCase
连接不受影响；宿主表单此后按新 key 存 config，旧存量在表单中回显为空、
拨号行为不变）。连带同步：`model.rs` 三个防漂移测试数组、
`smoke_test.py` 构造配置改用规范 key、PROTOCOL §「会话环境与会话命令」
与 FEATURE_PARITY tssh 节字段名更正。

## 连接保活盘点 + 终端活动保活（opt-in）+ external_config 解析修复（2026-09-08）

**背景**：用户报告公司策略下 SSH 连接/sudo 状态被空闲超时掐断，要求"添加保活"。
先盘点发现两层保活早已存在，真正缺口有二：服务器侧按键盘活动判空闲的策略
（`TMOUT`、堡垒机审计）协议层探测无效；且 `keepalive_interval_secs` 表单值
从未真正生效（见下）。

**现状盘点（不改即有）**：
- 协议层 keepalive：`bbb9c81`（2026-08-29）起三条拨号路径（正式连接/跳板每跳/
  host-key 探测）均配置 russh `keepalive_interval`（默认 30s）+ `keepalive_max: 3`
  （want_reply 全局请求，等效 OpenSSH `ServerAliveInterval`；任一收到的数据
  重置计数），对齐 keepaliveInterval=30s / keepaliveMaxFail=3。
- Quick Sudo 时间戳保活：sudo 执行成功后注册 `sudo -nv` 循环（4 分钟周期、
  连续 2 次失败自停、断连确定性中止）。

**新能力：终端活动保活 `terminal_keepalive_secs`（默认 0 关闭）**
- manifest 连接表单字段（binding `config`，number，0 关闭；en + 六语
  label/description 全补）；`model.rs` 解析 + `clamp_terminal_keepalive`
  钳制 5–3600s；跳板 `to_connection` 与 MCP `StoredConnection` 固定 0。
- `ssh.rs` `open_session` 按连接配置 spawn 每会话任务，经 `terminal_tx` 注入
  `TERMINAL_KEEPALIVE_INPUT`（`" \x7f"` 空格+退格：空命令行不入 history，
  全屏程序内仅光标往返）；只持有命令 sender，会话读循环退出（关闭/断连）
  即随 `send` 失败终止。`ssh/sessions/list` 新增 `terminalKeepaliveSecs` 上报。
- **开发期自抓回归**：首版任务循环漏写循环内 `tick`（`while send.is_ok(){}`）
  退化 busy-loop，真机 15s 灌 7705 帧——调试脚本抓到后修复为
  `loop { tick; send; }`，复测 15s 恰 3 次注入、回显每帧 <20 字节。

**修复 1：`connect_timeout_secs`/`keepalive_interval_secs` 表单值从未生效**
宿主 `buildPluginConnectionConfig` 把全部 `binding: config` 字段写入
`external_config`（PROGRESS-HOST-SUBREPO §700 实锤），而这两个字段解析只读
顶层 `connection` 对象——表单值被静默丢弃、默认值 15/30 恒生效（keepalive
靠默认 30s 碰巧可用）。按 `read_only` 收敛先例改为
`config_u64(external_config ∥ connection)`，新增单测钉住表单值生效。

**修复 2：基线失败测试 `manifest_connection_fields_stay_in_sync_with_parsing`**
d84787d 连接表单重构重排了 manifest 字段顺序但未同步测试期望数组（基线即
红）。按现 manifest 顺序更新，并纳入新字段 `terminal_keepalive_secs`。

**测试**：cargo test 221 全绿（含新增 `terminal_keepalive_is_opt_in_and_clamped`、
`session_info_payload` 断言扩展）；新增
`scripts/smoke_terminal_keepalive_test.py`（真机：sessions/list 上报 + 空闲窗
观测注入回显 + 会话存活，14s）；`smoke_test.py` PASS、`smoke_fs_test.py`
46/46 对新二进制回归通过。文档：PROTOCOL §「跳板机与连接存活」+ sessions/list
字段表。

## 批量发送交互改版：终端底部命令条（Electerm quick-command bar 风格，2026-09-08）

原"工具栏按钮 + 弹窗"批量发送改为**常驻贴在终端底部的单行命令条**（思路来源
Electerm quick-command bar；批量发送语义不变），纯前端改动，
协议面不变（`ssh/terminal/batchInput`、`ssh/quickCommands/*`、`ssh/sessions/list`
原样复用）。

**命令条组成**（`connected` 时显示；工具栏 ListChecks 按钮改为开关，is-active
态 + localStorage `ssh-batch-bar-open` 持久化，默认开）：

- 目标选择按钮 `目标 {count}/{total}` → 向上展开 popover（复用原目标列表：
  复选框、当前/已断开/只读徽标、全选/仅存活/刷新；顶部保留 batchSendHint 说明）。
- 快速命令 `<select>`：切换即回填输入框（不自动发送，回车/发送键触发）。
- 命令输入框：**回车即发送**；发送后命令保留在输入框（回车即重发的高频路径，
  Electerm 语义），危险/超长命令仍复用 `confirmRiskyPaste` 红色确认。
- 保存按钮（Save 图标）：切换为内联名称输入（默认名 = 命令压平空白截断 30 字），
  走 `ssh/quickCommands/save` 全局共享（≤20 条，超限置灰并提示）；保存成功后
  下拉自动选中该命令。
- 发送按钮（Send 图标 + Loader2 忙态）。

**结果浮条**：发送汇总/错误显示在命令条上方的浮条（复用 zmodem-status 视觉），
失败逐会话列出，手动关闭。

**布局细节**：`.terminal-pane.batch-bar-open` 时 `.terminal-host` inset 底部
让位 37px（ResizeObserver 自动 refit xterm）；command-marker / zmodem-status
底部偏移同步上移避让；目标 popover 改为向上展开（默认向下会被底部裁切）；
命令条根节点 `@contextmenu.stop` 防误触终端右键菜单；Esc 关闭链纳入目标
popover 与保存名称态（原弹窗的 modalOpenStates/Esc 分支移除）。

**目标列表行为**：连接建立 watch 自动刷新；刷新时剔除已关闭会话，选择为空才
兜底预选当前会话（原弹窗每次打开重置，命令条改为粘滞选择）。

**代码**：`App.vue`（状态/函数/模板）；`lib/batchSend.ts` 新增纯函数
`deriveBatchCommandName`/`quickPickCommandById`；`style.css` 弹窗样式段改写为
命令条样式段。i18n 七语新增 `batchBarSave`，其余文案复用 batchSend*/quickCommands*。

**测试**：vitest 268 全绿（新增 5 用例：默认名压平/截断/空串、下拉按 id 取命令
含未知 id）；`vue-tsc` 0 错误；前端 build 通过（UI 自包含产物写入 `ui/`）。
后端与协议零改动，无新增 smoke 用例（batchInput/quickCommands 已有覆盖）。

## 批量命令条首轮反馈修复：清空/历史/跨工作台同步 + 并发同靶 OTP 延迟补答（2026-09-08）

命令条上线后首轮真机反馈四项修复，其中 OTP 一项为后端行为修复（用户明确
诉求："totp 全用过了，应答时等待一下、延迟应答，而不是中断输入"）。

**1. 目标 popover 刷新按钮图标过大**：link-button 内 lucide 图标未约束尺寸，
去掉图标改纯文字（与"全选/仅存活"兄弟链接一致）。

**2. 跨工作台状态同步**（原"各自为政"）：命令条草稿/快速命令下拉选中/开关
状态现在跨工作台同步。新增 sidecar 方法 `ssh/batchBar/state`（notify 语义）：
工作台把 `{ source, draft, quickPickId, open }` 送达 sidecar，sidecar 原样
以同名事件广播给**所有**插件 webview（宿主 `app_handle.emit` 全局广播，各
端 `onEvent` 收到后按 `source` 过滤自己的回声）。输入去抖 150ms，开关/下拉
/发送清空立即发；sidecar 不落存储、纯转发；旧版二进制未注册时前端静默降级
（只影响同步不影响本端）。远端应用不打断本端焦点，不触碰保存态/弹出层。

**3. 发送后清空**：回车发送成功（sent>0）即清空输入框与下拉选中并入命令
历史（对齐原弹窗语义）；全部失败时保留草稿便于重试。

**4. ↑/↓ 历史浏览**：命令条输入框复用与命令弹窗同一份 `commandHistory`
（环形 100、去重、疑似凭据不落盘），↑↓ 语义与弹窗一致（进入浏览态备份
草稿、越过最新一条恢复）。

**5. 并发同靶 OTP 延迟补答（后端）**：批量发送 sudo 命令到同一 host:port
的多个会话时，各会话 OTP 提示几乎同时出现，当前窗口唯一的码被先到会话
提交后，重放保护（committed ledger，±1 step）会拒绝重复注入——原行为直接
跳过应答，后到会话永远晾在提示符上（用户被迫手动干预"中断输入"）。现在：

- `exec.rs`：`SudoAuth::answer_for_with_retry`（终端 watcher 专用变体）在
  OTP 承载型回答撞重放保护时返回 `(None, 下一个窗口边界+1s)`；`TerminalAutoSudo`
  新增 `otp_deferred_kind/until` 推迟态，`take_deferred_otp(now)` 到期重试、
  拿到新窗口码即注入并清推迟态，仍被抢则顺延下一窗口；shell 提示符复位、
  直接应答成功均清推迟态；静态恢复码不变不推迟（重试无意义）。
- `ssh.rs`：终端读循环既有的 250ms directory tick 臂上挂 `take_deferred_
  otp(unix_now_secs())`，到期即向 PTY 注入码并回车，发 `ssh/auto-sudo`
  事件（`kind=otp`）——与其他会话的应答在时间上天然错开 ≥1 个窗口。
- MCP exec 路径应答语义不变（仍走 `answer_for`/`take_totp_answer` 硬跳过）。

**验证**：cargo test 222 全绿（新增
`concurrent_same_target_otp_prompt_defers_then_answers_next_window`：账本
时间回拨模拟窗口滚动，钉住"撞重放→推迟→到期补答→一次性"全链路）；
vue-tsc 0 错、vitest 268 全绿、前端 build 通过；release 二进制上
`ssh/batchBar/state` 冒烟 PASS（`{'broadcast': true}`），smoke_fs_test.py
新增对应用例（未注册旧二进制上 SKIP）。协议文档：RPC 表新增方法行 +
「终端内 Quick Sudo」节补并发排队语义。剩余风险：真机 TOTP 容器下的多会话
并发 sudo 流未端到端演练（单测已钉住核心时序）；跨工作台同步依赖宿主全局
事件广播（当前 `app_handle.emit` 实现为全局，若宿主改为定点投递需跟进）。

### 命令条遮挡终端底行修复（同日第二轮反馈）

**现象**：终端内容滚到底部被命令条遮住；即使不开命令条，底行也略有溢出。

**根因**：FitAddon 计算行数读的是 `terminal.element`（`.xterm`）自身的
computed padding 做扣减，而原布局把 `padding: 5px 0 5px 10px` 挂在宿主
`.terminal-host` 上——fit 比实际可视区多算约 10px，底行渲染到 `.xterm`
框外；命令条打开时这段溢出正好压在条下。

**修复**（`style.css`）：内边距原样移到 `.terminal-host .xterm` 上
（box-sizing 全局 border-box 已有，fit 从 element 读 padding 后行数精确），
底部间距 5px → 8px 作为常驻呼吸间距；命令条让位 37px → 42px（条 ~35px +
7px 间隙），command-marker / zmodem-status 偏移 45px → 50px，结果浮条
41px → 48px。ResizeObserver 观察宿主 div，inset 变化自动 refit，无需前端
逻辑改动。前端 build + vitest 268 全绿复验通过。

### §8.16 MCP 连接发现与凭据免内联：stdio 桥接兜底 + ssh_list_connections + connectionName（2026-09-08）

**痛点**：独立 stdio MCP 会话（ZCode 等 AI 终端直拉 `dbx-plugin-ssh --mcp`）里
`dbx_connections` 注册表恒为空，带 `connectionId` 调用必报 "not registered with
this plugin session"，agent 只能手挖 DBX 应用 SQLite（connections +
connection_secrets 两表）再内联凭据，一轮"找连接"耗七八次工具调用，且密码进工具
参数。

**改动**（插件 `backend/src/` 三文件 + 宿主子仓库本地补丁一处，未 commit）：
1. **L1 桥接兜底**（mcp.rs `bridge_forward_plan` / `forward_tool_via_bridge`）：
   stdio 下连接类工具带未注册 `connectionId` 时，本地安全闸（进程只读总闸、
   destructive 确认）之后整次调用经 `app_bridge` 转发运行中 DBX 应用的
   `/call-plugin-tool`（ensure 探活 + 自动唤起应用，凭据全程不过工具参数）；
   桥不可用回落原内联路径。`bridge_fallback` 字段隔离测试；runInTerminal 路径
   不变（本就直连桥）。
2. **L2 `ssh_list_connections`**（app_bridge.rs `list_plugin_connections` +
   mcp.rs `connection_list_result`）：无参工具，出 id/name/host/port/username/
   authentication/readOnly 元数据（密码位只出 `passwordSet` 布尔，单测断言输出
   不含密钥值与 `"password":` 字段）；数据源 = 宿主桥 ∪ 本会话注册表，桥无路由/
   不可达降级 `source:"session-registry"` + note。
3. **L3 `connectionName`**（model.rs lifecycle 补 name 字段 + `registered_
   connection_by_ref` 统一查找）：注册表按名匹配、重名报错列候选；stdio 下经
   桥列表按名解析出 id 再转发；闸门层重名保守按只读 / sudo 白名单直接报错。
4. **L0 文案**：not registered 与 runInTerminal 两条报错改自愈指引（起 DBX /
   ssh_list_connections / 内联三选一）。
5. **宿主侧**（host/src-tauri/commands/mcp_bridge.rs，子仓库本地补丁）：新增
   `POST /list-plugin-connections` 路由，`PluginConnectionSummary` 封闭白名单
   结构体（仅 7 个 camelCase 元数据字段，单测断言凭据字段零泄漏），按
   plugin_id 过滤、名称排序。

**验证**：插件 cargo test 230 绿（新增 8 + 扩展 2）；scripts/smoke_mcp.py 对
release 二进制 28 工具全绿；宿主 `cargo check -p dbx` + mcp_bridge 单测 16 绿。
真机 E2E（新二进制 spawn stdio 对运行中 DBX 应用）：tools/list 含新工具；
`ssh_list_connections` 旧应用下降级出 note；**`ssh_exec` 仅 `connectionId`
零凭据内联经桥转发真机执行 aliyun-hk 返回 omni-hk**；`connectionName` 旧应用
下返回新自愈文案（宿主路由上线后此路即通）。

**文档同步**：docs/MCP.zh-CN.md（新工具行、stdio 桥接兜底节、连接寻址节、
工具计数 28）、docs/PROTOCOL.zh-CN.md（路由 + connectionName + lifecycle
name）、用户级 skill dbx-ssh-sftp-dev（连接发现四步 playbook 替换查表
workaround + 故障速查行）。

**剩余风险**：宿主路由需随 DBX.app 重构建后真机复验列表与 name→桥解析链；
sftp 池类只读工具桥宕回落文案仍为存量 "Connection is not established"（未统一
新指引）；connectionName-only 传输失败时 `drop_connection`/`ssh_close` 池
key 无法按名清理（id 调用不受影响）。

## 工作台滚动条隐藏：条体不再常驻显示（2026-09-09）

`style.css` 全局滚动条由"6px thin 常驻"改为全部隐藏（`scrollbar-width: none` +
`::-webkit-scrollbar { display: none }`），滚动仍由滚轮/触控板/键盘驱动；xterm
`.xterm-viewport` 的 `scrollbar-width: thin !important` 同步改 none。原先对
webkit 伪元素定制宽高会把滚动条从悬浮态固化为占位常驻态，与宿主观感不符。
改动仅 `ssh/frontend/src/style.css`；验证：`pnpm typecheck` 0 错、`pnpm test`
28 文件 268 用例全绿。

## 终端 MCP 模式开关 + MCP 转发不再抢焦点（2026-09-09）

用户报障两则：① stdio MCP 每次调用都立刻把 DBX 窗口顶到前台，打断其他工作；
② 命令没有出现在终端里（疑似仍走静默会话）。期望：终端上可直接开关"终端 MCP
模式"——开启后 MCP 命令经终端可见执行（审计/学习），关闭走静默隐藏通道。

**根因（两条都坐实）**：
1. 抢焦点：宿主 `mcp_bridge.rs::handle_call_plugin_tool` 对每次 `/call-plugin-tool`
   **无条件** emit `mcp-open-connection-workbench`，宿主前端监听器（useTauriEvents.ts）
   打开工作台后调 `focusCurrentWindow()`——由于 stdio 会话的连接类调用总是经 L1
   桥转发，**连纯静默调用也开标签+抢焦点**。
2. 无终端回显：路由矩阵里 `route = runInTerminal.unwrap_or(mode != Off)`，stdio
   调用不传 `runInTerminal` 且连接模式默认 `off` → 全部走隐藏通道（符合设计但
   不符合用户预期——模式只能在工作台设置弹窗深处设置，无终端就地入口）。

**改动**：
1. **宿主前端 `apps/desktop/src/composables/useTauriEvents.ts`**：
   `mcp-open-connection-workbench` 监听器删除 `focusCurrentWindow()`——标签照常
   打开/切换（命令进真实 PTY、缓冲可回看），但不再抢 OS 焦点。
2. **宿主 Rust `src-tauri/src/commands/mcp_bridge.rs`**：`/call-plugin-tool` 改为
   条件 emit——`is_terminal_routed_exec(tool)`（仅 `ssh_exec`/`ssh_exec_sudo`）且
   （显式 `runInTerminal: true` 或（缺省时探针 `ssh/agent/mode/get` 确认模式非
   `off`））才打开工作台；探针 5s 超时、任何失败（旧版插件无该方法等）回落静默
   路径，绝不因探针失败而开标签。sftp/metrics 等隐藏通道工具转发不再开标签。
3. **插件后端**：新增 RPC `ssh/agent/mode/get`（`{connectionId}` →
   `{agentTerminalMode, hasTerminalSession}`，未知连接降级 `off`/`false` 不报错，
   `ssh.rs::agent_mode_get` + `main.rs` 分发臂）；`ssh_exec`/`ssh_exec_sudo` 工具
   schema 的 `runInTerminal` 描述补"缺省时由连接级终端 MCP 模式决定"。
4. **插件前端**：工作台按钮行新增「终端 MCP 模式」快速开关（`Bot` 图标弹出层，
   三档单选就地生效，复用设置弹窗的三档文案；非 `off` 图标高亮；连接建立时经
   `ssh/settings/get` 同步初值，切换即 `ssh/settings/set`）；文案新增
   `agentTerminalQuickHint` 七语全补。

**路由语义（改后）**：终端模式开关（`agentTerminalMode`）一经在终端打开，stdio
MCP 的 `ssh_exec`/`ssh_exec_sudo`（不传 `runInTerminal`）经宿主桥转发到 embedded
sidecar 后按模式路由——`auto`/`strict` 下命令进可见终端（auto 低危直注、提权/高危
弹审批），`off` 走静默隐藏通道；`runInTerminal` 显式值仍最优先。宿主仅在确认要走
终端时才开工作台标签，且开标签不再抢焦点。

**测试**：backend cargo 231 tests（新增 `agent_mode_get_reports_mode_and_live_
session_presence`）；前端 vitest 28 文件 268 用例 + vue-tsc 0 错；宿主 vue-tsc
0 错、`cargo +1.97.1 check` 过、mcp_bridge 新增 `only_ssh_exec_tools_may_open_
the_workbench_terminal` 单测；smoke_fs agent 组新增 `agent mode get probe`
（模式/会话存在性 + ghost 连接降级）。

**文档**：PROTOCOL（RPC 表新行 + stdio 行为修正——旧"stdio 传 true 报错"已过时
实为转发；工具栏快速开关节）、MCP（开关 + 静默不打扰两节）。

**剩余风险/后续**：
- e2e `e2e_agent_app_bridge.py` 未加 mode-on 自动用例：模式置位需 embedded
  sidecar RPC（GUI 开关），脚本层无法注入；安全 hook 对该文件整体拦截写入后按
  规约还原，mode-on 全链路以手动验证替代——终端开 auto 后 stdio `ssh_exec` 不带
  `runInTerminal` 应在终端可见执行且 DBX 不抢焦点。
- 宿主两处改动（前端监听器 + 桥条件 emit）需重建 DBX.app 生效；正式版用户在
  上游吸收补丁前可临时用 worktree debug 构建验证。

## 插件数据目录 fallback 由 $TMPDIR 改为持久化路径（2026-09-09）

**根因**：DBX 宿主拉起 sidecar 时从未注入 `DBX_PLUGIN_DATA_DIR`（只注入
`DBX_PLUGIN_ID`/`DBX_PLUGIN_VERSION`/`DBX_APP_VERSION`/`DBX_HOST_API_VERSION`/
`DBX_PLUGIN_PROTOCOL_VERSION`），插件一直走
`std::env::temp_dir()/dbx-plugin-data/io.dbx.ssh` 兜底；macOS 的 `$TMPDIR`
（/var/folders/.../T/）在重启时清空，Quick Sudo 配置、快捷命令、mcp-settings、
插件 known_hosts 全部丢失（机器重启后实际发生；kafka 插件同构，已同日修复）。

**修复**：`main.rs::plugin_data_dir()` 拆出纯函数
`resolve_plugin_data_dir(lookup: impl Fn(&str) -> Option<OsString>)`（生产传
`std::env::var_os` 的闭包包装，测试传注入表，不用 `set_var` 避免并行测试竞态），
按序取第一个可用项（"可用"= 存在且 trim 后非空）：① `DBX_PLUGIN_DATA_DIR`
原样使用（宿主显式注入，未来方案 A 接入点）；② `DBX_DATA_DIR` →
`<DBX_DATA_DIR>/plugin-data/io.dbx.ssh`（便携/web 模式，`plugin-data/` 避开
安装器管理的注册树）；③ 平台标准用户数据目录下 `dbx-plugin-data/io.dbx.ssh`
（macOS `$HOME/Library/Application Support`、其他 unix
`${XDG_DATA_HOME:-$HOME/.local/share}`、Windows `%APPDATA%`；平台分支用
`cfg!` 运行时常量，同一二进制内可测）；④ 全缺才回落
`std::env::temp_dir()/dbx-plugin-data/io.dbx.ssh`，函数永不失败。
create_dir_all + canonicalize 边界行为保持不变。backend 中无第二处同语义目录
解析（其余 `temp_dir` 均为测试临时文件或 `local_transfer_roots_for` 的 sftp
本地传输白名单，语义不同不动）。

**测试（TDD）**：先写 6 个用例（DBX_PLUGIN_DATA_DIR 优先 / 空串视为未设 /
`DBX_DATA_DIR` 生效 / macOS HOME 路径 / 全缺回落 temp_dir / unix XDG 与
windows APPDATA 分支按 `#[cfg]` 留对应平台）确认失败
（`cannot find function resolve_plugin_data_dir`），实现后全绿；
`cargo test` 236 用例全过，`cargo build` 干净。本机 darwin 实际解析到
`/Users/Jinpy/Library/Application Support/dbx-plugin-data/io.dbx.ssh`。

**文档**：PROTOCOL「主机密钥」小节首次提及 `DBX_PLUGIN_DATA_DIR` 处补数据
目录解析顺序说明。

**剩余风险/后续**：已迁移历史数据在旧 `$TMPDIR` 路径且机器未重启的窗口期内
不会自动搬家（一次迁移不做，避免与宿主方案 A 冲突）；宿主未来注入
`DBX_PLUGIN_DATA_DIR`（方案 A）后 ① 自动生效，无插件侧改动。

## 终端体验两连：细竖线光标 + 点击定位光标（2026-09-09）

用户反馈两点：① 终端块状光标太粗，希望是细竖线；② 终端不能鼠标点击移动
输入位置，只能键盘方向键，希望像普通输入框一样点击定位。

**① 光标样式**：`App.vue::createTerminal` 的 xterm 配置
`cursorStyle: "block"` → `"bar"`（细竖线，保留闪烁）。纯前端一行改动，
无宿主/协议影响。

**② 点击定位光标**：终端协议里 shell 光标由远端控制，term 无法直接"落点"，
iTerm2/kitty 的通用做法是**同逻辑行内的点击换算成 N 次左右方向键**发给
readline。新增 `frontend/src/lib/terminalClickCursor.ts`（纯计算，无 xterm
依赖）：

- `cellFromMouseEvent`：像素 → 视口 cell，量 `.xterm-screen` 的 rect
  （恰为 cols×rows 格），滚动条宽度不影响列换算。
- `logicalLineSpan`：沿 `isWrapped`（标在续行上）向两侧展开光标所在逻辑行。
- `resolveClickCursorMove`：点击行不在逻辑行内 → 不动作（防止方向键把 shell
  翻进历史命令）；行内则按**字符**（宽字符 2 格记 1，readline 按字符移动）
  差值给方向键次数，上限 `CLICK_CURSOR_MAX_MOVES=1000`；备用屏
  （`buffer.type === "alternate"`）不动作。
- `clickCursorArrows`：展开为 CSI 左/右方向键序列。

`App.vue` 接线：terminalHost 挂 `mousedown`/`mouseup`（左键原地点击，位移
≤2px 且无选区才算点击，不干扰拖拽选择/双击选词）；守卫链：连接存活 →
无选区 → `modes.mouseTrackingMode === "none"`（vim/htop 等鼠标上报应用
点击语义归应用）→ normal buffer → 传输路由 `pty`（trzsz/zmodem 占流时
不代发）→ 发送。卸载时同步 removeEventListener。无新增文案（无 i18n
改动）、无新依赖、无协议/后端改动。

**验证**：新增 `terminalClickCursor.spec.ts` 13 用例（含宽字符、折行跨行、
回滚偏移、备用屏/上报守卫、像素换算），前端 `typecheck` + `vitest` 282
全过，`build` 产出 ui/index.html。真机行为建议装包后在长命令行上点击
回退/前进复核一次（macOS 拼音输入法组合窗口不受影响——点击不触发输入）。

## MCP 连接寻址升级：connectionName/endpoint 唯一匹配免 id（0.4.47 后续轮，2026-09-10）

**痛点**：§8.16 之后连接寻址仍要 agent 先查 `ssh_list_connections` 拿不透明 id
（或名称完全唯一才行），LLM 明明已从上下文知道「主机 + 用户名」却还要多一轮
id 映射；同名连接（多环境双胞胎）只能整体报歧义拒绝。

**改动**（纯后端 mcp.rs，协议面不变）：
1. **统一引用解析**（`registered_connection_by_ref` 重写）：`connectionId` 精确
   命中 > `connectionName` 精确匹配 > 完整 endpoint（host + username，port 默认
   22）唯一匹配；endpoint 字段可收窄同名候选。唯一命中即在 `call_tool` 入口
   归一化为 `connectionId`——池 key、只读门、sudo 白名单、终端路由全部一致。
2. **零猜测原则**：候选 0 个回落内联凭据 / stdio 桥接兜底（行为不变）；>1 个
   报歧义并列全部候选 id + host；`connectionId` 与其余 selector 共存且矛盾时
   直接拒绝（防错连）。
3. **stdio 桥列表同规则**（`resolve_connection_in_bridge_list` 替代 name-only
   辅助）：桥列表支持 connectionName / endpoint 解析出 id 后转发，语义与注册表
   一致。
4. **inputSchema anyOf**：全部 22 个连接类工具 schema 由 `required: host+username`
   改为 `anyOf: [connectionId | connectionName | host+username]`（业务字段
   required 保持独立），严格 MCP 客户端不再因「只传 connectionName」被客户端侧
   schema 校验拒绝。
5. **描述文案**：`connectionName` / `ssh_list_connections` 描述同步 endpoint
   复用语义。

**验证**：新增单测 4（endpoint 唯一复用、name+endpoint 消歧、selector 矛盾拒绝、
桥列表 name/endpoint 解析）+ schema anyOf 断言扩展；cargo test 239 全绿；
smoke_mcp.py 增加 connectionName/anyOf schema 断言；改动文件 mcp.rs +
docs/MCP.zh-CN.md（连接寻址节）。

**边界**：runInTerminal stdio 转发仍要求显式 `connectionId`（桥转发路径不解析
名称/endpoint，与 §8.16 一致）；仅传 host 不传 username 不做匹配（防同机多账户
误选）。
