# 对标 NyaTerm 差距项实施方案（2/4/5/7/8/9/10）

> **执行方式**：按任务逐条执行，步骤用 `- [ ]` 勾选跟踪。每个任务先写失败测试再实现。
> 对标源码已克隆到 `/Users/Jinpy/btroot/nyaterm`（浅克隆，master），下文 `nyaterm:` 前缀的路径均相对该目录。

**目标**：在 DBX 插件形态内对标实现 NyaTerm 的 7 项差距：会话类型广度、会话导入迁移、OTP 中心化管理、X11 转发、终端体验细节、主机监控深度、SFTP 工作流尾部。

**架构**：本插件 = DBX 宿主 webview 内的 Vue 3 workbench（`frontend/src`）+ 本机 Rust sidecar（`backend/src`，russh 0.62 + tokio，stdio-framed JSON/二进制协议）。NyaTerm 是 Tauri 同进程桌面应用，因此每一项都要先回答"宿主边界内怎么落地"。

**全局约束**（对所有任务生效）：

- 插件不管理连接库：DBX 连接由宿主存储，sidecar 只有 `list_plugin_connections` 读取桥（`backend/src/app_bridge.rs:205`）。任何"新连接类型"默认落**插件级存储**，不动 manifest。
- 插件级持久化先例：`<plugin_data_dir>/<name>.json`，0600，损坏即视为空（`backend/src/quick_commands.rs:39-59`、`backend/src/preferences.rs`）。
- 桌面端/ Web·Docker 双形态：凡依赖本机文件系统、PTY、串口、notify watcher 的能力，必须沿现有先例做"桌面可用、Web/Docker 降级或禁用"（参考 `App.vue` 下载目录探测）。
- 只读连接、sudo 白名单、审计日志、MCP 工具安全门必须跟随新能力（这是本插件相对 NyaTerm 的既有优势，不能倒退）。
- manifest 只允许收紧性变更；新增 UI 能力优先走 workbench 内面板，不改 connection-provider 字段（宿主 `deny_unknown_fields` 契约，见 CHANGELOG 0.4.79）。
- 许可证：NyaTerm 为 MIT，可参考/移植其代码与脚本，需保留版权声明；涉及算法（WindTerm 解密、TOTP）以规范为准独立实现亦可。

---

## 0. 分期路线图

| 期 | 差距项 | 内容 | 依赖 | 工作量级 |
| --- | --- | --- | --- | --- |
| P1 | 9a/9b | GPU（nvidia-smi）+ Ascend NPU（npu-smi）监控 | 无新依赖 | 小 |
| P1 | 8a | 命令历史 + 模糊建议 | 无 | 中 |
| P1 | 8b | 动作链接（IPv4/host:port/压缩包） | 无 | 中 |
| P1 | 8c | 行号/时间戳 gutter | 无 | 中 |
| P1 | 10b | 传输并发配置 + 重复目标策略 | 无 | 中 |
| P2 | 5 | OTP 库（TOTP/HOTP/二维码导入/集中面板/防重放） | `rqrr`、`image` | 中 |
| P2 | 4 | 会话导入（Xshell/MobaXterm/WindTerm） | `zip`、`encoding_rs`、`sha3`、`cbc`、`pbkdf2` | 中 |
| P2 | 10a | 远程文件外部编辑 + watcher 自动回传 | `notify` | 中 |
| P2 | 10c | SFTP 新建符号链接（`symlink@openssh.com`） | 无 | 小 |
| P2 | 9c | Docker 管理面板（复用 Quick Sudo 回退） | 无 | 大 |
| P2 | 8e | 背景图 | 无 | 小 |
| P2 | 8d | 选中文本在线搜索/翻译 | 无 | 中 |
| P2 | 8f | 大输出保护（背压限流） | 无 | 中 |
| P3 | 2a | 本地 Shell 会话 | `portable-pty` | 中 |
| P3 | 2b | Telnet 会话 | 无（自写 IAC） | 中 |
| P3 | 2c | 串口会话 | `serialport` | 中 |
| P3 | 2d | VNC 会话 | vnc-rs（或 vendored fork） | 大 |
| P3 | 2e | RDP 会话 | ironrdp 全家桶 + vendored forks | 特大，建议独立评审 |
| P3 | 7 | X11 转发 | russh x11 API 验证 | 中，先 spike |

P1 全部无宿主依赖、无 manifest 变更；P2 引入新依赖或新协议面，逐项过安全评审；P3 是协议级扩展，涉及 AGENTS.md 定义的高风险区（认证/协议变更），每项独立分支 + 人工评审。

---

## 1. 差距项 9a/9b：GPU 与 Ascend NPU 监控（P1）

### NyaTerm 1:1 分析

- 采集全部走 SSH exec 单发 POSIX 脚本，**无远端 agent**（`nyaterm:src-tauri/src/core/monitoring/gpu.rs`、`ascend_npu.rs`）。
- NVIDIA：`command -v nvidia-smi` 探测 → `GPU_AVAILABLE 0/1`；查询字段 `index,uuid,name,driver_version,temperature.gpu,utilization.gpu,utilization.memory,memory.total,memory.used,memory.free,power.draw,power.limit,fan.speed,pstate`（`--format=csv,noheader,nounits`）；进程用 `--query-compute-apps=gpu_uuid,pid,used_gpu_memory,process_name`；解析按 `GPU_CSV_BEGIN/END`、`GPU_PROCESS_CSV_BEGIN/END` 分节，自带引号感知 CSV 解析与 `N/A`/`[Not Supported]` 容错。
- Ascend：只依赖 `npu-smi info`（PATH → `/usr/local/bin/npu-smi` → `/usr/local/Ascend/driver/tools/*/npu-smi` 依次探测）；CANN 版本从安装元数据 `ascend_toolkit_install.info` 提取而非 npu-smi；按 `|` 切表格，HBM 优先于 Memory，`device_key = "ascend:{npu_index}:{chip_id}"`，支持一卡多芯片与 Phy-ID 新布局。
- 前端纪律：轮询最短 3s；探测不可用（GPU_AVAILABLE=0）即停轮询，手动刷新成功才恢复；连续 3 次失败清空数据。

### 本仓库落点与设计

本插件已有同风格采集器（`backend/src/metrics.rs`，POSIX 脚本 + 标记行解析 + 纯函数单测），直接扩展：

- 新建 `backend/src/metrics_gpu.rs`：
  - `pub fn gpu_overview_script() -> String`（含 NVIDIA 探测 + CSV 分节，逐字参考 NyaTerm 字段集）
  - `pub fn parse_gpu_overview_output(output: &str) -> serde_json::Value`
  - `pub fn npu_overview_script() -> String`、`pub fn parse_npu_overview_output(output: &str) -> serde_json::Value`
- 接入 `metrics.rs` 的 section 机制：`ssh_metrics` 的 `sections` 白名单增加 `gpu`、`npu`（`backend/src/metrics.rs:124` `validate_section_names` 同步更新），复用现有 exec 通道与超时。
- MCP 零成本受益：`ssh_metrics` 自动获得 gpu/npu sections。
- workbench 监控面板（现有 metrics UI）增加 GPU/NPU 卡片：利用率、显存/HBM、温度、功耗、进程列表（uuid 关联）；轮询间隔 ≥3s，`GPU_AVAILABLE=0` 或 3 次失败停轮询。
- 审计/只读：纯只读采集，无写操作；read_only 连接照常可用。

### 任务

- [ ] Task 1.1 `metrics_gpu.rs`：脚本生成 + Linux/macOS 采样 fixture 驱动的解析单测（GPU CSV 引号、N/A 容错、HBM 优先、多芯片）。验收：`cargo test -p dbx-plugin-ssh metrics_gpu`
- [ ] Task 1.2 sections 白名单 + 真机冒烟（有 NVIDIA/Ascend 主机各一；无则跳过并在 TEST_MATRIX 记录）
- [ ] Task 1.3 前端 GPU/NPU 卡片 + 轮询节流 + 停轮询状态；七语文案

---

## 2. 差距项 8a：命令历史 + 模糊建议（P1）

### NyaTerm 1:1 分析

- 采集是**纯前端按键跟踪**，不做 prompt 检测：`nyaterm:src/lib/terminalInputTracker.ts` 在 `onData` 里逐键维护 `{value, cursor, desynced, multiline, pasteMode}`，处理 `\r`、Ctrl+C/A/E/U/W/K、退格、bracketed paste；Tab 补全后置 `desynced`，下次输入从 xterm buffer 读回真实行重同步。
- 回车时提交：仅当未 desync、非多行时取出命令 → 后端注册；后端 `sanitize_history_command` 再剥已知 prompt 前缀兜底（`nyaterm:src-tauri/src/core/history/sanitize.rs`）。
- 存储：sidecar 内存 LRU 5000 条 + 异步持久化（`core/history/store.rs`），重复命令上浮并计数。
- 模糊匹配：Rust 端 nucleo-matcher，`fuzzy_search_history` + quick commands 并行搜索、按分合并取前 12，输入 80ms debounce（`hooks/useCommandHistory.ts:221`）。
- 噪音过滤：`command_suggestion_min_chars`（默认 2）/ `max_chars`（默认 64）。
- 交互式抑制 5 门：凭据输入窗口、alternate buffer（vim/top）、shell integration `commandRunning`、命中抑制程序集（btop/htop/less/man/nano/vim/top… + journalctl 无 --no-pager + tail -f）后以 Ctrl+C 或 q 解除、pager 输入启发式（`/^[/?:]/` 与单键翻页）。
- UI：光标下方浮层（380px，匹配高亮、来源图标、单条删除、Tab 填充 / Enter 执行），定位用 xterm 私有 `_core._renderService.dimensions.css.cell`。

### 本仓库落点与设计

- 新建 `frontend/src/lib/terminalInputTracker.ts`（框架无关纯 TS，可参照 NyaTerm 逻辑改写）：`createInputTracker()` 返回 `{ onData(data): TrackerState, resync(bufferLine): void }`。
- 新建 `frontend/src/lib/commandHistory.ts`：模糊评分（自写 subsequence + 连续命中加分，避免新 npm 依赖；数据量 ≤5000 前端匹配足够）；`search(query, {history, quickCommands}, limit=12)`。
- 存储：sidecar 新建 `backend/src/command_history.rs`，`<plugin_data_dir>/command-history.json`（0600，上限 5000，重复上浮 + use_count），协议方法 `history/list`、`history/record`、`history/delete`。Web/Docker 形态 sidecar 不在本机——沿 quick-commands 同一模式即可（该文件本就随 sidecar 宿主）。
- **安全裁剪（本插件特有）**：以下输入永不入库——认证/凭据窗口期的输入、Expect 自动应答注入的文本、OTP 手输、被标记 `sensitivity: secret` 的会话输入；命令以 `containsSecretHint`（命中 `password`/`token` 等可配置词表）时仍入库但 UI 建议默认折叠（保持与 NyaTerm "长度过滤"一致的可配置性：`history_min_chars`/`history_max_chars`）。审计策略在 TEST_MATRIX 记录。
- UI：`frontend/src/components/CommandSuggestions.vue` 浮层 + `App.vue` 终端 onData 管线接入（现有输入队列按序回放逻辑不动，历史记录在提交后异步 `history/record`，不阻塞输入）。
- 抑制门（对齐 NyaTerm 5 门）：alternate buffer、认证窗口、抑制程序集、pager 启发式；shell-integration 门本插件无对应信号，用"会话未就绪/命令执行中标志"替代。

### 任务

- [ ] Task 2.1 `terminalInputTracker.ts` + 单测（键序模型、desync 重同步、bracketed paste、多行）——纯逻辑测，仿 `terminalWebkitInput.spec.ts`
- [ ] Task 2.2 `commandHistory.ts` 评分/过滤/合并 + 单测
- [ ] Task 2.3 sidecar `command_history.rs`（record/list/delete + 上限 + 0600）+ 协议注册 + 单测
- [ ] Task 2.4 `CommandSuggestions.vue` + App.vue 接入 + 抑制门 + 七语文案；验收：headless Chrome 注入按键流，输入 `doc` 出建议、Tab 填充、Enter 执行，密码提示窗口期输入不出现在 `history/list`

---

## 3. 差距项 8b：动作链接（P1）

### NyaTerm 1:1 分析

- 自研 `ITerminalAddon + ILinkProvider`（`nyaterm:src/lib/actionLinksAddon.ts`，985 行），叠加在内建 WebLinks 之上；命中区域用 `registerDecoration` 画虚线下划线。
- 匹配器（`src/lib/actionLinksMatcher.ts`）：IPv4 严格正则 + `isValidIPv4`；host:port 排除源码/日志后缀误报（`SOURCE_LOCATION_EXTENSIONS`）；压缩包 `zip|rar|7z|tar.gz|tgz|tar.bz2|tbz2|tar.xz|txz`。
- **点击不直接执行**：默认动作是把构造好的命令（`ping X`、`curl http://ip`、`nc -vz h p`、`telnet h p`、`unzip f`…）准备进输入行；Alt+Click 出全部动作菜单；Ctrl/Cmd+Click 执行默认动作。逻辑行窗口化匹配 + 缓存 + 滚动 eviction。
- 默认关闭（`action_links_enabled` 默认 false）。

### 本仓库落点与设计

- 新建 `frontend/src/lib/actionLinksAddon.ts`（xterm addon + link provider，TS 框架无关）+ `actionLinksMatcher.ts`（正则与校验函数，独立单测）。
- 复用现有 decoration 基建：本插件已有 IP/关键词高亮管线（`frontend/src/lib/keywordHighlight.ts`，含"仅重绘且文本变化行重建"的防抖策略），动作链接与关键词高亮共用同一条 decoration 生命周期，避免重蹈 0.4.79 修复过的自激重绘回路（风险点必须遵守：新 decoration 同样只在文本变化行重建）。
- 交互：hover 指针 + 虚线下划线；点击 = 填充命令到输入行（不回车、不直接执行——比 NyaTerm 更保守）；设置项 `action_links_enabled`（默认关）、三类 matcher 开关。入 `SettingsDialog.vue`。
- 与 IP 关键词高亮共存：同文本命中时动作链接让位（跳过已有关键词装饰的行段）。

### 任务

- [ ] Task 3.1 `actionLinksMatcher.ts` + 正则/误报单测（源码位置误报、压缩包大小写、host:port 端口范围）
- [ ] Task 3.2 addon + decoration 接入（复用高亮重建策略）+ 回归：headless 验证空闲 12s 零额外重绘（同 0.4.79 验收口径）
- [ ] Task 3.3 点击填充命令 + 设置项 + 七语文案

---

## 4. 差距项 8c：行号/时间戳 gutter（P1）

### NyaTerm 1:1 分析

- 独立 DOM 列（非 xterm decoration）：`nyaterm:src/components/terminal/TerminalGutter.tsx`，每可视行一个 div；行高对齐 `_core._renderService.dimensions.css.cell.height`，`paddingTop` 取 `.xterm-screen.offsetTop`；`onRender/onWriteParsed/onScroll/onResize` + rAF 节流；wrapped 行与 alternate buffer 不显示。
- 时间戳 = 行**首写时刻**（`lineTimestampsRef: Map<绝对行号, ms>`，每批输出写完盖时间，map 裁剪至 start-3000），回车时把光标所在逻辑行（含 wrapped）重盖为回车时刻。
- 设置：`show_line_numbers`、`show_timestamps`、`timestamp_format`（默认 `[HH:mm:ss]`，token 化，≤64 字符）。

### 本仓库落点与设计

- 新建 `frontend/src/lib/terminalGutter.ts`（几何计算纯函数：`computeGutterRows({renderDims, screenOffsetTop, scrollTop, rows, timestamps}) -> GutterRow[]`，独立单测）+ `frontend/src/components/TerminalGutter.vue`。
- 时间戳采集挂在现有输出写入完成点（App.vue xterm write 队列完成后批量 `stampWrittenLines(from,to,ts)`；回车重盖逻辑同 NyaTerm）。
- 设置入 `SettingsDialog.vue`：`terminal.show_line_numbers`、`terminal.show_timestamps`、`terminal.timestamp_format`。
- 约束：xterm 私有 `_core._renderService` 升级风险——集中封装到 `terminalGutter.ts` 单点，附降级（读不到 dimensions 则隐藏 gutter，不报错）。大输出 strained 模式（见 8f）下挂起。

### 任务

- [ ] Task 4.1 `terminalGutter.ts` 几何/裁剪单测（wrapped 跳过、buffer 绝对行号、map 裁剪）
- [ ] Task 4.2 `TerminalGutter.vue` + 时间戳采集接线 + 设置项 + 七语文案
- [ ] Task 4.3 headless 验收：滚动/回滚/resize/`\r` 原地改写四场景行号对齐、无内存增长（map 裁剪生效）

---

## 5. 差距项 10b：传输并发配置 + 重复目标策略（P1）

### NyaTerm 1:1 分析

- 调度在前端（`nyaterm:src/context/TransferContext.tsx`）：按方向并发（`transfer.download_threads/upload_threads`，默认 3，clamp 1–10），queued/parked 状态纯前端迁移，in-flight 暂停走后端 `pause_transfer` 状态机（`TransferControlState = Running|Paused|Cancelled` + `Notify`）。
- 重复目标：入队前按策略 skip/overwrite/rename/ask 预筛（`lib/transferDuplicateResolution.ts`，ask 逐个弹窗 + 应用到全部）；传输中真冲突由后端 emit 请求、前端 `respond_transfer_duplicate` 应答；rename 生成 `name(1)..name(999)`。
- 速度：3s 滑窗采样；失败重试：后端 `max_transfer_retries`（默认 2）单文件循环。

### 本仓库落点与设计

本插件已有断点续传/暂停（`frontend/src/lib/transferResume.ts`）、速度（`transferSpeed.ts`）、传输历史（`backend/src/transfer_history.rs`）与原子替换。补齐：

- 设置：`transfer.concurrent_uploads`/`concurrent_downloads`（默认 3，clamp 1–10）与 `transfer.duplicate_policy`（`rename`(默认)/`overwrite`/`ask`），入 `SettingsDialog.vue` + `preferences.rs` 白名单。
- 前端队列：`frontend/src/lib/transferQueue.ts` 纯函数调度器 `nextRunnable(queue, runningByDirection, limits)`（单测驱动）；App.vue 传输入口接入。
- 重复检测：上传前对目标路径做 `sftp/exists` 预检（本插件已有该能力，`backend/src/sftp_ext.rs:70`），按策略：rename → `sftp/rename-unique`（后端新增，`name(1)..name(999)` 生成）；overwrite → 直接覆盖（沿现有原子替换）；ask → 复用现有确认对话框组件。
- 失败重试：沿用现有断点续传语义，不做 NyaTerm 的后端循环重试（避免半成品重复写，保持本插件"原子替换"安全边界——设计取舍写入 CHANGELOG）。

### 任务

- [ ] Task 5.1 `transferQueue.ts` 调度器 + 单测（按方向计数、暂停占位、取消释放）
- [ ] Task 5.2 sidecar `sftp/rename-unique` + 单测（999 上限、跨目录）
- [ ] Task 5.3 上传入口接重复策略（rename/overwrite/ask）+ 设置项 + 七语文案；验收：并发=2 时连续上传 5 文件最多 2 个 in-flight；ask 模式弹确认且"应用到全部"生效

---

## 6. 差距项 5：OTP 中心化管理（P2）

### NyaTerm 1:1 分析

- 独立零依赖 crate（`nyaterm:src-tauri/crates/otp`）：`Hotp{alg,issuer,label,digits,counter,secret}` / `Totp{period,hotp}`，RFC 4226/6238 标准截断，`generate_at/verify/to_uri/from_uri`；`otpauth://` 要求 `issuer:label` 前缀与 issuer 参数一致。
- 存储：`OtpEntry{otp_type,issuer,username,secret(磁盘 AES-256-GCM 加密),algorithm,digits,period,counter}` 存 redb；命令层生成时 HOTP 持久化 counter+1，TOTP 返回 `{code, remaining_seconds}`。
- 二维码导入：后端 `image`(png/jpeg/bmp/gif/webp) → `rqrr` 解码 → `from_uri` 预填编辑框（`cmd/otp.rs:81`）。
- SSH 自动填充：认证前 `resolve_otp_info` 读取连接绑定的 `otp_id`；keyboard-interactive 阶段恰好 1 个 prompt 且启发式判 OTP → 自动生成填充；否则 emit `otp-request` 弹窗，用户提交；TOTP **防重放**：进程内 `{otp_id: {code, time_step}}` 缓存，命中则睡到下一周期重新生成，认证成功后记录。
- UI：Security/Auth 面板（列表 + 显隐码 + 编辑/删除 + 发送到终端 + 扫码导入 + 线性进度条）。

### 本仓库落点与设计

本插件已有连接级 TOTP 自动应答（`auth_flow_mode` + `totp_secret`，0.4.79 打磨的登录 MFA/sudo 编排），**保留不动**；新增的是"库"这一层：

- 新建 `backend/src/otp.rs`（零新依赖：`hmac`+`sha1`/`sha2` 已有，base32 用已有 `data-encoding`）：
  - `pub fn hotp(alg: Algorithm, secret: &[u8], counter: u64, digits: u8) -> u32`
  - `pub fn totp_at(alg, secret, period: u64, ts: u64, digits: u8) -> (String, u64 /*remaining*/)`
  - `pub fn parse_otpauth_uri(uri: &str) -> Result<OtpParams, String>`（totp/hotp、secret/issuer/algorithm/digits/period/counter）
- 存储：`<plugin_data_dir>/otp-entries.json`（0600），secret 字段经 `backend/src/vault.rs` 加密落盘（与私钥内容同策略）。
- 协议方法：`otp/list`（secret 永不返回）、`otp/save`、`otp/delete`、`otp/generate`（TOTP 带 remaining；HOTP 生成即 counter+1 持久化）、`otp/import-qr`（参数：图片字节或桌面端路径；实现 `image`+`rqrr`，**仅此任务引入这两个 crate**）。
- 连接关联：不改 manifest。workbench 设置里维护映射 `connection_id -> otp_entry_id`（存 `otp-entries.json` 的 `bindings` 段）；sidecar 在现有 OTP 自动应答取码时**优先级**：连接 `totp_secret` 字段（现状）→ 绑定的 OTP 库条目 → 全局 Quick Sudo（现状）。防重放：进程内 `{entry_id: (code, time_step)}` 缓存 + 命中等待下一周期。
- UI：`SideNavPanel.vue` 新增 OTP 面板：列表（issuer/username/TOTP|HOTP 标签）、显隐验证码 + 线性进度条、复制、扫码导入（桌面选图 / Web·Docker 上传图片字节）、编辑/删除；发送到终端按钮走现有输入通道（标 `sensitivity: secret`，不进命令历史、不进录制——与第 8a/录制任务联动）。

### 任务

- [ ] Task 6.1 `otp.rs` 算法 + RFC 4226/6238 官方向量单测 + `otpauth://` 解析/生成往返
- [ ] Task 6.2 存储 + 协议方法（secret 加密、不回显）+ 单测
- [ ] Task 6.3 `rqrr`/`image` 二维码导入 + fixture 图片单测（totp/hotp 各一）
- [ ] Task 6.4 自动应答接入绑定映射 + 防重放缓存 + 单测（同一 time_step 不重复用码）
- [ ] Task 6.5 OTP 面板 + 七语文案 + headless 验收
- [ ] 风险标注：`rqrr`/`image` 为新增编译面（image 特性裁到 png/jpeg/bmp/webp），过依赖评审

---

## 7. 差距项 4：会话导入迁移（P2）

### NyaTerm 1:1 分析

- 单模块多解析器（`nyaterm:src-tauri/src/core/importer/`），按扩展名分发：`.xts`→Xshell、`.mxtsessions`→MobaXterm、`.sessions`→WindTerm（另支持 SecureCRT/FinalShell/JSON 类，本期不做）。
- 输入一律是"用户选文件"，跨平台；编码 BOM→UTF-8→GBK 兜底；INI 自解析（`;`/`#` 注释）。
- Xshell：`.xts` 是 ZIP，取内含 `.xsh`（INI）：`[CONNECTION] Protocol/Host/Port`、`[CONNECTION:AUTHENTICATION] UserName/UserKey`；分组=ZIP 目录路径；密码不导入。
- MobaXterm：`.mxtsessions` INI，只取 `Bookmarks*` 段，`SubRep` 按 `\` 切分组，value 为 `#109#...%host%port%user%...` 管道格式，仅 SSH；密码材料不存在于该文件。
- WindTerm（最强，485 行）：`.sessions` JSON；解密链 = 同级 `user.config` 的 `application.fingerprint` 作盐，PBKDF2-HMAC-SHA3-512 100k 迭代派生 48B → AES-256 key+IV，`autoLogin` 为 base64(AES-256-CBC/PKCS7)；解析 `protocol/target(user@host)/label/port/group(> 分隔)/description`；autoLogin `PasswordEnabled+Password` 导入密码（应用层再加密落盘）；`Public Key.windows.path/pass` 读私钥文件+passphrase，按（路径,passphrase）去重；主密码开启而未提供 → 明确报错弹密码框。
- UI：来源九宫格 → 选文件 → 导入即追加（无预览），分组按路径去重复用 id，提示"合并不清空"。

### 本仓库落点与设计

宿主桥无连接写入口（`app_bridge.rs` 仅 list），导入产物落**插件级连接库**：

- 新建 `backend/src/connection_import.rs`：
  - `pub struct ImportedSession { name, host, port, username, auth: ImportedAuth, group_path: Vec<String>, description }`，`ImportedAuth = Password{value:Option<secret>} | PrivateKey{path, content:Option, passphrase:Option} | None`
  - `pub fn parse_xshell(zip_bytes: &[u8]) -> Result<Vec<ImportedSession>, String>`（`zip` crate，条目名 GBK 解码）
  - `pub fn parse_moba_ini(text: &str) -> Result<Vec<ImportedSession>, String>`
  - `pub fn parse_windterm(sessions_json: &[u8], user_config: Option<&[u8]>, master_password: Option<&str>) -> Result<Vec<ImportedSession>, String>`（新增 `sha3`、`cbc`、`pbkdf2` 依赖；解密失败仅 warn 跳过凭据，不中断）
  - 入库：`<plugin_data_dir>/imported-connections.json`（0600；密码/口令/私钥内容经 vault 加密；去重：`name+host+port` 重复时生成 `name (2)` 并保留，与 NyaTerm "只并不清"一致但在列表标记疑似重复）
- 协议方法：`import/parse`（返回预览列表，不入库）、`import/commit`（入库返回计数）——比 NyaTerm 多一步**预览**，用户可勾选。
- 输入通道：桌面端 sidecar 直接读路径（文件选择经宿主 picker 或路径输入）；Web/Docker 前端 File API 读字节上传（WindTerm 需同时选 `user.config`）。
- UI：`SideNavPanel.vue` 新增"导入的会话"面板：导入向导（选来源→选文件→WindTerm 主密码输入→预览勾选→导入）；列表分组展示，条目动作 = 在当前 workbench 打开（复用现有连接流程，凭据取自插件库）、删除。明确文案：这些会话保存在插件本机，不进 DBX 连接库（待宿主提供连接写入口后可迁移——CHANGELOG 记录该边界）。
- 安全评审点：解析器处理不可信文件——windterm 解密用内存内 AES，禁止 panic（全部 Result）；上传字节设上限（20MB）；ZIP 解压防 zip-bomb（条目数/解压后总大小上限，如 10000 条 / 256MB，超限报错）。

### 任务

- [ ] Task 7.1 `parse_moba_ini` + fixture 单测（最先做，无依赖）
- [ ] Task 7.2 `parse_xshell` + 构造最小 .xts fixture（zip 两目录三会话）单测
- [ ] Task 7.3 `parse_windterm` + 用已知向量自造 `user.config`/`autoLogin` 密文 fixture（PBKDF2/AES-CBC 解密往返）单测；主密码缺失报错路径
- [ ] Task 7.4 存储入库 + 预览/提交协议 + vault 加密 + 单测
- [ ] Task 7.5 导入向导 UI + 七语文案；验收：三个格式 fixture 文件各导入 ≥3 会话，重启后仍在，"打开"可建立连接
- [ ] 风险标注：`sha3`/`cbc`/`pbkdf2`/`zip`/`encoding_rs` 新增依赖过评审；WindTerm 解密仅用于读取用户自有文件

---

## 8. 差距项 10a：远程文件外部编辑 + watcher 自动回传（P2）

### NyaTerm 1:1 分析

- 外部编辑链路：下载到 `tempDir()/nyaterm/{sessionId}/{ts}/safeName` → `start_file_watch`（notify `recommended_watcher`，非递归，`{session}:{path}` 去重）→ 系统编辑器打开。
- 防误报（`core/watcher/fingerprint.rs`）：`FileFingerprint{len,modified,SHA256(≤64MB)}` 与基线比对，处理 temp+rename 原子保存；启动 2s 抑制窗；500ms 去抖；确认内容真变才 emit `file-modified`。
- 回传确认：加入 `alwaysUpload` 名单则静默上传，否则弹"上传一次 / 总是上传 / 取消"子窗口。
- 冲突防护：内置编辑器保存带 `expectedMtime/Size/Hash` 基线，后端比对不符返回 conflict → 强制覆盖或丢弃重载。

### 本仓库落点与设计

- 新建 `backend/src/file_watch.rs`：`notify` crate（新依赖）+ `fingerprint(len, mtime, sha256)`；`watch/start`、`watch/stop` 协议方法；去重表、500ms 去抖、2s 启动抑制；变化时通过现有 sidecar→前端事件通道推送 `file-modified {watchId, localPath}`（实现前先确认该通道：传输进度/终端输出已有一条 sidecar→UI 推送路径，复用之；若仅轮询则新增一条轻量事件帧——属于协议面新增，过评审）。
- UI（`App.vue` SFTP 面板右键菜单"在外部编辑器中打开"）：桌面端 → sidecar `local/downloads` 已有落盘目录先例，下载到 `<downloads>/remote-edit/{ts}/{name}` → `watch/start` → 经宿主 open 能力打开（无 open 桥则提示路径让用户手动打开，COPY 路径到剪贴板兜底）→ `file-modified` 到达 → 弹"上传一次 / 总是上传（记住该路径）/ 取消" → 上传走现有 SFTP 通道 + 只读/写入门禁照常。
- Web/Docker：sidecar 不在本机，菜单项禁用并七语提示（沿下载目录探测先例）。
- 会话关闭时清理：`watch/stop-all`（该会话的 watcher 表清空）。

### 任务

- [ ] Task 8.1 `file_watch.rs` fingerprint + 去抖/抑制 + 单测（临时写脚本模拟 temp+rename；用真实临时目录集成测）
- [ ] Task 8.2 watch 协议 + 事件推送 + 会话关闭清理 + 单测
- [ ] Task 8.3 UI 链路（菜单→下载→watch→确认→上传）+ 七语文案；验收：本地编辑保存 → 3s 内收到确认弹窗 → 上传后远端 mtime/内容更新；取消不误传
- [ ] 风险标注：`notify` 新依赖；sidecar→UI 事件通道若为新增协议面需评审

---

## 9. 差距项 10c：SFTP 新建符号链接（P2）

### NyaTerm 要点

OpenSSH EXT_EXTENDED `symlink@openssh.com`（linkpath/target 参数顺序陷阱已处理：`nyaterm:src-tauri/src/core/sftp/sftp_backend/fs.rs:421`）；UI 支持新建/改指向；移动/复制有 `ensure_no_symlink_ancestors` 防护。

### 本仓库落点与设计

- 现状：symlink 只识别跳过（`backend/src/sftp_tree.rs:61,128`）。新增：
  - `backend/src/sftp_ext.rs`：`sftp/symlink-create`（russh-sftp 的 symlink 扩展 API；若 russh-sftp 3 未暴露 `symlink@openssh.com`，退化为 `exec` 通道执行 `ln -s`（引号转义 + 白名单校验目标/链接路径不含 shell 元字符，或经 `sh_quote`）——实现时先 spike 确认）。
  - `sftp/symlink-read`（readlink）与 `sftp/symlink-update`（重建指向）。
- UI：SFTP 目录树右键"新建符号链接…"/"编辑链接指向…"；列表对 symlink 条目显示 `→ target`。
- 安全：只读连接拒绝；创建动作写审计日志；目标路径规范化，允许绝对/相对（相对相对 CWD 显示）。

### 任务

- [ ] Task 9.1 spike：russh-sftp 3 symlink API 确认（半天），定扩展包或 exec 兜底路线
- [ ] Task 9.2 协议方法 + 转义/校验单测 + 审计接入
- [ ] Task 9.3 UI 右键 + 七语文案；验收：Linux 主机创建/读取/改指向成功，dangling link 可创建

---

## 10. 差距项 9c：Docker 管理（P2）

### NyaTerm 1:1 分析

- 纯 shell over SSH（`nyaterm:src-tauri/src/cmd/docker.rs` 1638 行 + `core/monitoring/docker/scripts.rs`）：`docker ps -a/images/volume ls/network ls` 用 `--format` tab 输出；详情 `inspect` + `stats --no-stream`；日志 `logs --tail N`（clamp 10–2000）；compose 探测 `docker compose version` 回退 `docker-compose`。
- sudo 四级回退：PATH 注入直跑 → `sudo -n` → 缓存密码 `sudo -S -p ""`（stdin 传密码，绝不拼命令行）→ 前端弹窗（最多 2 次）；错误分类（permission denied / 需要密码 / 认证失败）。
- 动作白名单 `normalize_container_action`：start/stop/restart/kill/rm；前端 rm/kill/prune 一律确认框；exec 进入容器 = 生成 `docker exec -it <id> sh -lc 'bash→zsh→fish→ash→sh 兜底'` 写入现有终端会话。

### 本仓库落点与设计

- 新建 `backend/src/docker.rs`：脚本生成（`--format` tab + BEGIN/END 分节，逐字对齐 NyaTerm 字段集）+ 解析纯函数；动作执行统一走 `docker_action(container_id, action)`，白名单 `start|stop|restart|kill|rm`，容器 id 严格 hex 校验。
- **sudo 回退直接复用 Quick Sudo**：plain → Quick Sudo profile（本插件已有的 sudo 编排含 TOTP）→ 报错指路配置。比 NyaTerm 的四级回退更简单且复用现有安全面（白名单、审计）。
- 协议方法：`docker/list`、`docker/inspect`、`docker/logs`（tail clamp 10–2000）、`docker/action`、`docker/compose-ls`（第一期不做 compose 动作，只读展示）。全部受 read_only 门约束（action 类拒绝）、写审计日志、进 MCP 工具清单（`docker_list` 只读工具 + `docker_action` 需 confirm）。
- UI：`SideNavPanel.vue` 新增 Docker 面板：容器列表（状态/端口/镜像）、详情抽屉（inspect/stats）、日志抽屉、"在终端打开"（生成 `docker exec -it` 命令写入当前终端，兼容会话未连接时禁用）；rm/kill 强制确认对话框；轮询 ≥10s、3 失败停轮询。
- 工作量提示：这是 P2 里最大单项；面板可先做"列表 + 启停 + 日志"三件事，inspect/stats/compose 后置。

### 任务

- [ ] Task 10.1 脚本 + 解析纯函数 + fixture 单测（含 stats 空、compose 缺失）
- [ ] Task 10.2 协议方法 + Quick Sudo 回退 + 白名单/审计/read_only 门 + 单测
- [ ] Task 10.3 MCP 工具注册 + 文档（MCP.zh-CN.md 表格）
- [ ] Task 10.4 Docker 面板（列表/启停/日志/进终端）+ 确认框 + 七语文案；验收：docker 主机上列表刷新、start/stop 生效且审计留痕、只读连接 action 被拒

---

## 11. 差距项 8e：背景图（P2）

### NyaTerm 要点

纯 CSS 层（data URL + opacity + cover/contain/stretch/tile，`nyaterm:src/lib/backgroundImage.ts`）；三重可读性保障：表面变量 color-mix 半透明、xterm theme.background 置透明、**开背景图时挂起 WebGL 渲染器**回退 DOM（`shouldSuspendTerminalWebglForBackground`）。

### 本仓库落点与设计

- 设置入 `SettingsDialog.vue`：`appearance.background_image`（桌面：sidecar `local/fs` 读图返回 data URL，上限 8MB，仅 png/jpg/webp；Web/Docker：支持粘贴 data URL）、`background_fit`、`background_opacity`（默认 0.45）、`terminal_content_opacity`（默认 0.78）。
- 前端：workbench 根部加 `pointer-events:none` 背景层；表面/终端底色按 opacity 用 color-mix 透明化；**开背景图时禁用 WebGL 渲染器**（复用 0.6.0 的 WebGL 重建逻辑开关点，`App.vue` 渲染器选择处），关闭后恢复。
- 偏好存 `preferences.json` 白名单（新键入 `preferences.rs` allowlist）。

### 任务

- [ ] Task 11.1 preferences 新键 + sidecar 读图（类型/大小校验）+ 单测
- [ ] Task 11.2 背景层 + 透明化 + WebGL 挂起联动 + headless 验收（开/关各一次，确认无黑屏、文字可读、重绘正常）

---

## 12. 差距项 8d：选中文本在线搜索 / 翻译（P2）

### NyaTerm 要点

入口在终端右键菜单：搜索 = `url_template.replace("%s", encodeURIComponent(text))` 开系统浏览器（可配自定义引擎列表）；翻译 = 子菜单选 provider → Rust 端 `translate_text`（google/microsoft 免费端点 + deepl/baidu/ali 需 key）→ 对话框展示可复制。

### 本仓库落点与设计

- 终端右键菜单组件 `frontend/src/components/TerminalContextMenu.vue`（本插件当前无终端自定义右键菜单，需新建；浏览器 webview 的 contextmenu 阻止默认后自绘；注意 macOS 与 WKWebView 现有输入直写路径不冲突——菜单打开时暂停输入捕获）。
- 在线搜索：引擎设置 `search_engines`（默认 Google；支持自定义 name+url_template）。打开方式：优先宿主 open-url 能力（实施时探测 `window.dbxPlugin` 桥是否有 openExternal；**当前 SDK 无此接口**，兜底 = 复制构造好的 URL 到剪贴板 + toast 提示，并在 docs 记录"待宿主 Host API 提供 openExternal"）。
- 翻译：sidecar 新建 `backend/src/translate.rs`，`translate/execute` 协议方法（provider: `google`/`microsoft` 免费端点 + `deepl`（API key 经 vault 存 `preferences` 引用））；前端翻译结果对话框可复制。免责：免费端点随时可能失效，实现成可插拔 provider，失败给出可读错误。
- 裁剪建议：第一期只做"选中→复制 + 打开搜索引擎（URL 兜底剪贴板）"；翻译后置到宿主 open-url 可用之后（避免半残体验）。

### 任务

- [ ] Task 12.1 `TerminalContextMenu.vue`（复制/粘贴/搜索/翻译骨架 + 选中态检测）+ 组件测试
- [ ] Task 12.2 搜索引擎设置 + URL 构造 + 打开/兜底复制 + 七语文案
- [ ] Task 12.3（后置）`translate.rs` + deepl key 管理 + 翻译对话框

---

## 13. 差距项 8f：大输出保护（P2）

### NyaTerm 要点

前端背压限流（不丢弃、不清屏）：写入管线 pending bytes ≥128KB 进入 `strained`，分帧 32KB 限速写入 + ack 协调（后端等前端确认再推）；alternate screen 16KB/20fps 节流；恢复阈值 visible 200K/hidden 50K；strained 期间 gutter/搜索等副组件挂起；写失败进入 overloaded 模式。提示文案存在但未接线（NyaTerm 自身 TODO）。

### 本仓库落点与设计

- 前置审计（Task 第一步）：梳理 `App.vue` 现有 xterm 输出写入路径（队列、ack、按序回放——0.6.0 输入侧已有序保证，输出侧确认是否已有背压）。
- 新建 `frontend/src/lib/terminalBackpressure.ts` 纯逻辑：`OutputGate` 状态机（normal/strained/recovering），阈值常量对齐 NyaTerm（128KB/64KB/32KB/200K/50K），输入 = 写入完成/积压字节采样，输出 = 是否放行/分帧大小。单测驱动。
- 接入：输出写入前过 gate；strained 时 `TerminalGutter`（8c）与关键词高亮扫描挂起；恢复后自动重建（复用现有重建预算逻辑）。
- 提示接线（比 NyaTerm 做完整）：状态条/toast "大输出保护已生效/已恢复"七语文案。
- read_only/审计无关；不影响输入路径。

### 任务

- [ ] Task 13.1 输出管线审计报告（现有队列/ack 行为，写进本文档附录）
- [ ] Task 13.2 `terminalBackpressure.ts` 状态机 + 单测（阈值进出、恢复、挂起信号）
- [ ] Task 13.3 App.vue 接入 + 副组件挂起 + 提示 + headless 验收：`yes | head -c 50M` 灌入，UI 不卡死、内存有界、恢复后高亮/gutter 正常

---

## 14. 差距项 2：会话类型广度（P3，高风险区——逐项独立评审）

### 14.0 共同架构决策（先做，P3 的前置任务）

NyaTerm 的统一模型：Local/Telnet/Serial 共用 `SessionCommand`（Write/Resize/Close/PauseOutput…）+ `SessionOutputCoalescer` + 编解码层，差异只在传输层；RDP/VNC 独立 Manager + 44 字节二进制帧协议（sequence+尺寸+patch 头，`core/remote_desktop/frame.rs`），前端共享 Canvas2D/WebGL2 渲染器。

本插件映射：

- **会话容器**：workbench 终端区改为轻量会话容器（当前 SSH 会话为默认 tab；Local/Telnet/Serial/VNC 以"临时会话 tab"加入，不入 DBX 连接库）。这是 P3 第一个任务（纯前端重构，不动协议）。
- **sidecar 会话注册表**：`backend/src/local_session.rs` 引入 `EphemeralSession`（id、kind、句柄、输入/输出帧复用现有二进制帧 channel 机制——SSH 终端已有的同通道 FIFO 顺序保证直接继承）；`session/close`、`session/write`、`session/resize` 按 kind 分派。
- **形态门**：Local/Telnet/Serial 仅桌面端（Web/Docker 无本机 PTY/设备，菜单禁用 + 七语提示）。
- **安全**：临时会话同样过审计日志（命令输入）、只读门（read_only 的 DBX 连接不提供写入型临时会话入口？——不：临时会话与 DBX 连接无关，默认不启用只读门，但 UI 显式标注"本地/直连会话不受连接级只读约束"，评审时确认）。
- **manifest 不变**：这四类是 workbench 内会话，不是 connection-provider。

### 14.1 Local Shell（`portable-pty`）

- NyaTerm：`portable-pty 0.8`（`openpty`→`spawn_command`→master 读写线程）；shell 探测 Windows=powershell、Unix=$SHELL 兜底 bash（bash/zsh/fish 加 `--login -i`）；cwd 跟踪 = OSC 7 解析（`core/terminal_session/local/cwd.rs`）；resize 走 master.resize。
- 本插件：`backend/src/local_session.rs`（新依赖 `portable-pty`）——`local/open`（shell/args/cwd 可配，默认探测）、write/resize/close 复用会话协议；前端 tab + 终端复用。OSC 7 cwd 解析可复用本插件已有的"远程目录跟随"基建。
- 风险：PTY 权限（沙箱/打包后 spawn shell）、Windows ConPTY；Windows 行为单独验收。

### 14.2 Telnet（自写 IAC，无 crate）

- NyaTerm：tokio TcpStream + 手工 IAC（WILL/DO 协商表、NAWS resize 子协商、IAC IAC 转义剥离）；Backspace Mode（`ctrl_h` 把 0x7F→0x08，`core/input.rs:remap_del_to_bs`）；回车模式 CRLF/Cr/Lf；本地行编辑/回显；自动登录（正则应答用户名/密码提示）。
- 本插件：`backend/src/telnet_session.rs`（~800 行，逐块对齐 `core/terminal_session/telnet/`：negotiation/session/auto_login/types 四文件移植改写）；UI 新会话表单（host/port/backspace/enter 模式/自动登录开关）。SSH 触发器（triggers.rs）的 Expect 引擎可复用为自动登录实现基础。

### 14.3 Serial（`serialport` crate）

- NyaTerm：`serialport 4` 阻塞读线程（4096 缓冲 + 10ms 超时轮询）；参数枚举 `available_ports()`；`SerialConfig{port,baud,data_bits,parity,stop_bits,backspace,encoding}`；resize 忽略；X/Y/ZModem 复用。
- 本插件：`backend/src/serial_session.rs`（新依赖 `serialport`）；UI 表单（波特率预设 9600–921600 可手输）。Zmodem 本插件终端已有（zmodem-js 前端），串口会话复用。

### 14.4 VNC（vnd fork vnc-rs）

- NyaTerm：vendored 加固版 vnc-rs（None/VNC-Auth DES、Raw/ZRLE/Tight、44 字节帧 patch、前端 fit/stretch/actual 纯 CSS 缩放、断线 generation 重连、剪贴板 Latin-1、Tight JPEG 显式报错、64MB/2160p 有界分配）。
- 本插件：**建议第二梯队**。sidecar 引入 vnc-rs（评估上游 HsuJv/vnc-rs 0.5.3 直接依赖 vs 复制 NyaTerm 加固 fork——倾向先上游 + 我们补有界分配审查）；前端共享渲染层可整体移植（`lib/remoteDesktopFrame.ts` + `renderer.ts` + viewport，React 无关）。协议：帧 patch 走现有二进制帧通道（大 payload 分帧）。密码 ≤8 字节限制提示。
- 风险：二进制带宽（全屏 patch 经 stdio-framed 桥 + 宿主桥两层，需压测帧率/延迟；失败则降级"低帧率查看"定位）。

### 14.5 RDP（ironrdp + vendored forks）

- NyaTerm：ironrdp 0.17 + **5 个 vendored fork**（ironrdp-client 注入证书验证器/cliprdr 工厂、connector 0.10 补丁、picky、sspi 0.21），后端 2409 行编排（CredSSP/NLA、证书校验事件、输入 scancode 映射、文本剪贴板桥、重连 generation、Windows 键盘全局捕获）。
- 本插件：**建议单独立项评审，不并入本期**。理由：vendored fork 链的维护责任、CredSSP 凭据处理（AGENTS.md 高风险区）、带宽压力同 VNC。若立项：按 NyaTerm 结构移植（engine crate + 编排层 + 共享渲染层），范围裁剪为"密码/NLA + TLS + 文本剪贴板 + 重连"，不做音频/驱动器重定向/键盘捕获。

### 14.6 差距项 7：X11 转发（P3，先 spike）

- NyaTerm 有 `core/ssh/x11_forwarding` 模块（分析确认存在，细节未逐行核对）。
- 本插件路径：russh 0.62 客户端 **先 spike 验证**两件事：(a) 能否发 `x11-req`（`request_x11` 类 API）；(b) `channel_open_x11`（或以 `channel_open_direct_tcpip` 到本机 `DISPLAY` TCP 端口兜底）。前提是用户本机装有 X server（macOS: XQuartz / Windows: VcXsrv），sidecar 在本机转发 `localhost:6000+n`。
- 若 russh 0.62 缺 API：评估成本（fork russh / 换传输层）后再决定；spike 报告先出，不直接排实现任务。
- 安全评审点：X11 转发本质是本机开放 X 协议面，文档必须警示"仅在可信网络使用"；read_only 连接默认禁用。

### 任务（P3 按序）

- [ ] Task 14.0 会话容器重构（前端 tab 容器 + 临时会话协议骨架 `session/open|write|resize|close`）+ 单测
- [ ] Task 14.1 Local Shell（portable-pty）+ 桌面验收（macOS/Windows 各一）
- [ ] Task 14.2 Telnet（IAC/NAWS/Backspace/自动登录）+ 真机冒烟（路由器/交换机类设备）
- [ ] Task 14.3 Serial + 参数表单 + 冒烟（USB-串口）
- [ ] Task 14.4 VNC（依赖评估 → 移植 → 帧通道压测 → 面板）
- [ ] Task 14.5 X11 spike 报告（russh API 结论 + 兜底路线 + 是否立项建议）
- [ ] Task 14.6 RDP 立项评审材料（范围裁剪 + vendored 维护计划 + 安全评审清单），另行排期

---

## 15. 全局验证与交付纪律

- 每任务提交前：`cargo test`（backend 相关模块）+ 前端 `vitest` + 类型检查 + 生产构建（沿 `dbx-ssh-dev` 技能的最小验证集）；UI 改动过 headless Chrome 走查（`mock.html` 流程）。
- 每期交付：CHANGELOG 双语条目、TEST_MATRIX 增行、`docs/COMPARISON.zh-CN.md` 能力矩阵更新对应行。
- 新依赖逐个过评审（记录用途/许可证/维护状态）：P2 合计 `rqrr`、`image`、`zip`、`encoding_rs`、`sha3`、`cbc`、`pbkdf2`、`notify`；P3 合计 `portable-pty`、`serialport`、vnc 引擎。
- 高风险区（协议新增：watcher 事件、VNC 帧通道、X11、RDP、临时会话协议）逐项人工评审后才进实现分支。
- 分支策略：每差距项一个 `codex/ssh/nyaterm-parity-<item>` 分支，不与当前 hostkey 分支混行； agents 不自合 PR。

## 16. 边界与未知

- NyaTerm 侧结论以本轮 1:1 源码分析为据（`/Users/Jinpy/btroot/nyaterm`），其实现质量未逐行验证；移植时以行为对齐为准，不必复刻内部命名。
- 宿主侧三个待确认点（落地前各半天确认）：openExternal Host API 有无（影响 8d）、连接写入接口有无（影响 4 的长期归属）、sidecar→UI 事件通道形态（影响 10a）。
- 大输出保护与输出管线的耦合程度需 Task 13.1 审计后才能定实现细节。
