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
UI 与 MCP 双通道支持 quick sudo / auto sudo。设计与契约见
`docs/IMPL_PLAN_QUICK_SUDO.zh-CN.md`（推翻 FEATURE_PARITY L41 既有「不做」结论）。

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
凭据通道；与 tiny-rdm 未启用 local-vault 档等价），已在 IMPL_PLAN §0/§9 记录权衡，
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

需求：完善 ssh 的 MCP 工具面并接入 ZCode 实测。对标 tiny-rdm 演化版 MCP CTL
工具面后补齐缺口（`SFTPTransfer` / `sftpPwd`），并确认 ZCode 客户端接入路径。

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
无终端会话报错引导；超时返回部分输出命令继续跑。实施计划：
`docs/IMPL_PLAN_AGENT_TERMINAL.zh-CN.md`（后端/前端并行 agent 实施，主会话接线）。

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
   协议/对标/实施文档同步（PROTOCOL §运行时设置/§sudo、FEATURE_PARITY、
   IMPL_PLAN_QUICK_SUDO §11）。
