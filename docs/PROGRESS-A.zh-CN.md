# S-A 路交付报告（ssh-sftp 后端增强）

日期：2026-08-28。工作目录：`~/btroot/dbx-plugins/ssh-sftp`，仅触碰 `backend/src/**`、`scripts/smoke_batch3_test.py`、`docs/PROTOCOL.zh-CN.md`（S-A 所有权范围）。未执行任何 git commit/push。

## 1. 基线状态

`cargo test`（backend/）在未提交 batch3 改动之上：**98 passed / 0 failed，全绿**。

逐项核对了 batch3 未提交改动（exec.rs / main.rs / mcp.rs / ssh.rs / metrics.rs / sftp_copy.rs）——全部为**完成态，无半成品**：

| 工作包 | 状态 | 证据 |
| --- | --- | --- |
| C：sftp/copy、sftp/move（sftp_copy.rs） | 完成 | main.rs 两臂已注册；mcp.rs `sftp_copy`/`sftp_move` 工具已定义；7 个单测（参数解析、shell 转义、目标探测、契约 JSON） |
| C：sudo 时间戳保活（exec.rs + ssh.rs） | 完成 | `SUDO_KEEPALIVE_INTERVAL=240s`、`SUDO_KEEPALIVE_MAX_FAILURES=2`、`keepalive_failure_step` 单测；`register_sudo_keepalive` 单实例 + `stop_sudo_keepalive` 断连确定性中止 |
| C：OTP 防重放（exec.rs） | 完成 | `committed_totp` + `take_totp_answer`（±1 步长硬跳过）+ 3 个单测（窗口计算、重复提交跳过、clone 共享状态） |
| D：metrics 增强（metrics.rs） | 完成 | 网络 rx/tx 速率（Linux `/proc/net/dev` + macOS `netstat -ibn`）、Top 8 进程；Linux/macOS 样例解析单测齐备；`exec::collect_metrics` 已薄委托 |
| smoke | 已有 | `scripts/smoke_batch3_test.py`（metrics/copy/move，未注册方法 SKIP） |

## 2. 本轮完成项

### 2.1 ssh/metrics 快照缓存（对齐 tiny-rdm `GetLastSnapshot`）

- **能力**：`SshRuntime` 按会话缓存最近一次指标快照（内存态，`close_session` 清除）。`ssh/metrics` 新增可选参数 `cached: bool`（默认 `false`）：`true` 且有缓存 → 返回上次快照并附 `cachedAt`（Unix 秒）；未命中 → 现采并回填。现采成功路径一律回填。用途：工作台指标弹窗可先用上次数据即时渲染再刷新（采集本身含两次网络采样约 1.4s）。
- **测试证据**：`cargo test` 100 passed / 0 failed（新增 `cached_metrics_payload_marks_age_without_mutating_the_snapshot`、`metrics_snapshot_cache_serves_and_cleans_per_session`，后者用 tempdir 构建真实 `SshRuntime` 验证 serve/隔离/清理语义）。`cargo build --release` 通过。
- **smoke**：`scripts/smoke_batch3_test.py` 新增 `ssh/metrics serves the cached snapshot` 用例（校验现采无 `cachedAt`、缓存命中带正 `cachedAt`、与首次快照同源；`ssh/metrics` 未注册时随既有 SKIP 链跳过）；`python3 -m py_compile` 通过。
- **PROTOCOL 更新**：新增「## 服务器指标」章节（返回字段全表 + `cached`/`cachedAt` 语义 + 缓存生命周期）。

### 2.2 PROTOCOL.zh-CN.md 全面同步（补齐既有缺口）

- **RPC 表**补列已实现但未入册的方法：`ssh/host-key/check`、`mcp/settings/get|set`、`sftp/transfer/list|status`、`sftp/copy|move`；`ssh/metrics` 行说明扩展维度与 `cached`。
- **新章节**：`### sftp/copy`、`### sftp/move`（参数表、`from` 单值/数组、`toDir`、`overwrite` 探测语义、逐项 `results` 契约、同目录 move 优先 SFTP rename、300s 远端执行预算、无 sudo 变体）。
- **新章节**：`## 服务器指标`（见 2.1）。
- **「二进制通道」**补 `sftp/transfer/list`（`{ tasks: [...] }`，会话维度过滤含历史）与 `sftp/transfer/status`（单任务同构状态，任务不存在查历史）说明。
- **「Quick Sudo 远程执行」**两处行为更新：保活循环细化（4 分钟周期、连续 2 次失败自停、单实例、断连确定性中止）；OTP 防重放语义（提交后窗口 = 自身有效窗 + ±1 步长内不再注入、编排日志标注 `otp auto-answer skipped`、连接级内存态重启清零）。

### 2.3 mcp.rs `ssh_metrics` 工具描述更新

描述补齐 network/processes 维度，与实际返回一致（MCP 工具发现面准确性）。

## 3. 安全红线自查（新增代码逐条对照）

| 红线 | 结论 |
| --- | --- |
| 远端命令 shell 单引号转义 | 本轮**未新增**任何远端命令拼装；既有覆盖：`shell_quotes_paths`、`copy_and_move_commands_quote_every_argument`、`shell_quotes_embedded_single_quotes`、`shell_quote_wraps_and_escapes_single_quotes` |
| 写操作 `ensure_writable` | 未新增写操作；抽查确认 batch3 `sftp_copy.rs:300` 已过守卫，`sudo_fs`/`sftp_ext` 各写入口均在 |
| sudo 写分块 ≤256KiB | 未触碰；`sudo_fs.rs` `WRITE_CHUNK_BYTES = 96 KiB` |
| `removeAll` 拒 `/` | 未触碰；`sudo_fs.rs:506` 守卫在位 |
| 私钥只出指纹 | 未触碰；`keys.rs` 仅 SHA256 指纹 |

## 4. 遗留与说明

- `cached` 用例对**支持 `ssh/metrics` 但不识别 `cached` 参数的旧 sidecar** 会 FAIL 而非 SKIP（方法存在、参数被忽略、返回无 `cachedAt`）。这是有意为之：对当前开发构建它是回归信号；对外兼容语义不受影响（旧 sidecar 忽略未知参数仍正常现采）。
- FEATURE_PARITY / TEST_MATRIX 状态由 S-C 统一收口，本轮未改（所有权约束）。
- tiny-rdm `StartMetrics/StopMetrics/Pause/Resume` 的持续采集 runtime + 事件推送模型与本仓「按需现采 + 快照缓存」是架构级差异，未照搬（插件单连接上下文 + 宿主事件模型不同）；快照缓存已覆盖 `GetLastSnapshot` 的实际消费场景。
- smoke_batch3 全量跑（真实 SSH 容器）本轮未执行（容器未起）；脚本编译与单测验证通过，`DBX_PLUGIN_SIDECAR=backend/target/release/dbx-plugin-ssh python3 scripts/smoke_batch3_test.py` 可直接复验。

## 5. 交接（给 S-B 前端）

- **指标弹窗**：打开时可先调 `ssh/metrics { sessionId, cached: true }` 即时渲染（返回含 `cachedAt`，可显示数据时点），随后常规 `ssh/metrics` 刷新（5s 轮询语义不变）。首次打开（无缓存）现采约 1.4–2s，注意 loading 态。
- **服务器内复制/粘贴**：`sftp/copy`、`sftp/move` 已注册，参数 `{ sessionId | connectionId, from: string|string[], toDir, overwrite? }`，返回 `{ success, results: [{ from, to, ok, error? }] }`（逐项成败）。目标存在需先 `sftp/exists` + 覆盖确认，或直接 `overwrite: true`。与 FEATURE_PARITY_BATCH3 工作包 B 契约一致。
- **传输面板**：可用 `sftp/transfer/list`（按 `sessionId` 过滤）恢复/展示历史传输，`sftp/transfer/status` 单查。
- **PROTOCOL.zh-CN.md** 为本轮后为准（copy/move、服务器指标、传输查询均已成文）。

## 6. 阻塞

无。

---

# S-A 路第二轮交付报告（2026-08-28 续）

在第一轮（batch3 收口 + ssh/metrics 快照缓存）工作区状态之上继续，仅触碰
`backend/src/{metrics,ssh,main,mcp}.rs`、`scripts/smoke_batch3_test.py`、
`docs/PROTOCOL.zh-CN.md`。未执行任何 git commit/push。

## 1. 基线状态

`cargo test`（backend/，未提交 batch3 改动之上）：**107 passed / 0 failed，全绿**
（第一轮交付时 100，其间 workspace 又合入 7 个并发用例，本轮起点即 107 全绿）。
batch3 各工作包（sftp_copy、sudo 保活、OTP 防重放、metrics 增强）复核均为完成态，无半成品。

对照 `FEATURE_PARITY.zh-CN.md` / `FEATURE_PARITY_BATCH3.zh-CN.md` 复审：主清单已全部
✅（known_hosts 管理、sftp/transfer 查询、tiny-rdm 只读能力面均已覆盖），剩余后端侧
可做缺口为 tiny-rdm `ssh_metrics_service.go` / `sftp_service.go` 中的**增强维度**，本轮选做 2 项：

## 2. 本轮完成项

### 2.1 ssh/metrics 增强：磁盘 inode 使用率 + Top 内存进程（对齐 tiny-rdm SSHDiskStat.InodeUsePercent / SSHMetricsProcessGroup.TopMem）

- **能力**（均 additive，旧调用方零破坏）：
  - 采集脚本追加 `--dfi--` 节（`df -iP`，仍单条 POSIX 只读命令）：按挂载点合并进
    `disks[]`，新增 `inodeUsePercent` 字段；`df -iP` 采不到的挂载（如部分虚拟挂载）不出现该字段。
    解析端按「mount 前最后一个 `%` 后缀列」取百分比，GNU/busybox 列布局差异由解析吸收。
  - 采集脚本追加 `--psm--` 节（同一条 `ps` 管道改按 `%MEM` 排序取前 8）：新增顶层
    `topMemory` 数组，与 `processes`（按 CPU 排序）字段同构。
- **测试证据**：`cargo test` 107 → 全绿（新增
  `parses_inode_usage_across_df_dialects`（GNU/busybox 双布局 + 表头跳过）、
  `inode_usage_merges_into_matching_mounts_only`（合并按 mount 匹配、无数据字段缺席而非置空）、
  `top_memory_processes_are_parsed_independently`；Linux/macOS fixture 已扩充）。
  真机 smoke：`topMemory=8 rows; disks with inodeUsePercent: 5/5`。
- **smoke**：`scripts/smoke_batch3_test.py` 新增
  `ssh/metrics reports inode usage and top memory processes`（`ssh/metrics` 未注册时随 SKIP 链跳过）。

### 2.2 sftp/read 支持可选 `offset` 分片续读（对齐 tiny-rdm `ReadFile(offset, length)`，与 `sudo/readFile` 家族语义对齐）

- **能力**：`sftp/read` 新增可选参数 `offset`（字节偏移，默认 0；省略/非法按 0）。
  实现为 russh-sftp 文件句柄 `seek(SeekFrom::Start)`（仅移动本地读游标，无 fstat 往返），
  `offset` 在/超 EOF 时返回空内容且 `truncated: false`（SFTP 侧以空读表示 EOF，与
  `sudo/readFile` 的超界报错语义不同，PROTOCOL 已注明）。调用方以
  `offset += 返回字节数` 续读，配合既有 `maxBytes`（默认 256 KiB、上限 1 MiB）实现大文件预览分页。
  同步生效面：
  - 宿主 filesystem 桥 `filesystem_read`（`fs/read`）同样接受 `offset`；
  - MCP `sftp_read_file` 工具新增 `offset` 属性（schema + 处理器 + 描述）；
  - `sudo/readFile` 的 offset/length 参数解析收敛到共享 `optional_u64` 助手（行为不变）。
- **测试证据**：`cargo test` 全绿（新增 `optional_u64_falls_back_on_missing_or_invalid`、
  MCP tools/list 形状测试扩展断言 `sftp_read_file` schema 含 `offset`）；
  真机 smoke `sftp/read honors offset paging`：head(maxBytes) 截断 / offset 16 续读 / 超界空读三段全过。
- **smoke**：`scripts/smoke_batch3_test.py` 新增上述用例（`sftp/read` 未注册时 SKIP）。

### 2.3 PROTOCOL.zh-CN.md 同步

- **RPC 表**：`ssh/metrics` 行补「磁盘（含 inode 使用率）+ Top CPU/内存进程」；
  `sftp/read` 行注明支持可选 `offset` 分片续读。
- **新章节 `### sftp/read`**：参数表（`path`/`offset`/`maxBytes`）、返回契约
  （`dataBase64`/`truncated`）、EOF 空读语义、与 sudo/readFile 的差异、错误面。
- **`## 服务器指标`**：返回 JSON 增加 `disks[].inodeUsePercent` 与 `topMemory`；
  采集命令描述补 `df -iP` 与按内存排序的进程表；注明两扩展字段旧 sidecar 可能缺失（调用方按可选处理）。

## 3. 安全红线自查（新增代码逐条对照）

| 红线 | 结论 |
| --- | --- |
| 远端命令 shell 单引号转义 | 本轮采集脚本仅追加**固定字面量**只读命令（`ps`/`df -iP`），无任何用户输入拼接；`sftp/read offset` 走 SFTP 协议 seek，不经过 shell |
| 写操作 `ensure_writable` | 未新增写操作（sftp/read、metrics 均为只读；只读连接可用性不变） |
| sudo 写分块 ≤256KiB | 未触碰（`sudo_fs.rs` 本轮零改动） |
| `removeAll` 拒 `/` | 未触碰 |
| 私钥只出指纹 | 本轮未新增涉及私钥的代码（`keys.rs` 的 diff 为既有 batch3 改动 + fmt 格式化，见 §5 说明） |

## 4. 遗留与说明

- FEATURE_PARITY / TEST_MATRIX 状态由 S-C 统一收口，本轮未改。
- tiny-rdm metrics 的接口 IP 地址/错误丢包计数（需 Linux 加采 `ip -o addr`）与
  StartMetrics/Pause/Resume 持续采集模型仍未做——前者收益/复杂度比低，后者是架构级差异（同第一轮结论）。
- **工作区提示**：本轮为保持风格一致在改动文件上执行了 `cargo fmt`，波及
  exec.rs/host_key.rs/keys.rs/ssh.rs 等既有 batch3 改动文件的**格式**（语义零变化，
  cargo test 先后全绿）；无 batch3 改动的 model.rs/sudo_fs.rs/sftp_ext.rs 已还原为 HEAD，
  避免无关噪音。review 时这些文件 diff 会混入纯格式行。
- smoke_batch3 全量真机已跑：**12 passed / 0 skipped / 0 failed**（release 二进制，
  linuxserver/openssh-server 容器），两轮新增用例均在其中。

## 5. 交接（给 S-B 前端）

- **指标弹窗**：磁盘分区可展示 inode 使用率（`disks[].inodeUsePercent`，字段可能缺席，
  建议缺省隐藏该列）；进程分区可加「按内存」切换（`topMemory` 与 `processes` 同构，直接换数据源）。
  两者均为可选字段——对旧 sidecar 返回需容忍缺失。
- **文本预览分页**：大文件预览可用 `sftp/read { path, offset, maxBytes }` 续读
  （`truncated: true` 时 `offset += dataBase64 解码后字节数` 继续）；`offset` 超 EOF 返回空串不报错。
  sudo 路径继续走 `sudo/readFile`（其 offset 超界会报错，两族语义不同，前端错误处理要分开）。
- **PROTOCOL.zh-CN.md** 以本轮后为准。

## 6. 阻塞

无。
