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
