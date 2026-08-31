# 与 tiny-rdm 的 SSH/SFTP 特性对标清单

基线：`/Users/Jinpy/GolandProjects/tiny-rdm`（本地源码）。
插件现状：`backend/src/main.rs` 方法表（**67 个分发方法臂、68 个方法名**——`ssh/host-key/resolve`
与 `connection/challenge/resolve` 共用一臂（main.rs:174），含 sftp/copy、sftp/move、ssh/host-key/check；
2026-08-29 收口复核，修正如下的「68 臂」口径）。
原则：一比一补齐 tiny-rdm 的 SSH/SFTP 能力面；DBX 已由宿主承担的能力（连接管理、
profile 分组、全局外观）不重复实现。

## 能力对照总表

| tiny-rdm 能力 | tiny-rdm 位置 | 插件状态 | 优先级 |
| --- | --- | --- | --- |
| SFTP 基础（list/read/mkdir/rename/chmod/delete/upload/download/传输槽） | sftp_service.go | ✅ 已有（`sftp/read` 另支持可选 `offset` 分片续读，对齐 ReadFile(offset,length)） | — |
| 目录磁盘占用 diskUsage | sftp_service.go | ✅ 已有 | — |
| 终端 PTY/回放/resize/目录跟随 | ssh_service.go | ✅ 已有 | — |
| ssh/exec + sudo（含 PTY/MFA/TOTP 编排） | sudo_exec_service.go | ✅ 已有（exec.rs） | — |
| ZMODEM rz/sz | ssh_service.go | ✅ 已有 | — |
| 终端 Shell Integration 命令标记（OSC 633：命令/退出码/时长/cwd） | frontend/src/modules/ssh/osc633-parser.js | ✅ 已有（`terminalCommandMarkers.ts` 解析器移植 + 状态条，S-B；运行中时长 1s tick `runningCommandElapsedMs` + `marker-elapsed` span，X-B） | — |
| 会话状态规范化展示（连接中/已连接/重连中/已断开/错误） | frontend/src/modules/ssh/session-status.js | ✅ 已有（`sessionStatus.ts` 移植 + reconnecting 扩展，S-B；多会话择优不适用未移植） | — |
| 命令输出净化（控制序列剥离/回显移除） | frontend/src/modules/ssh/terminal-output.js | ✅ 已有（`terminalOutputText.ts` 通用部分移植；`.mcp_ctl_*` hook 特判不适用） | — |
| 命令历史（弹窗历史区/↑↓ 浏览/一键重发） | SshPage 命令历史 | ✅ 已有（`frontend/src/lib/commandHistory.ts` 环形 100 条 + App.vue 接线；疑似凭据/超长/多行命令不落 localStorage，A-SSH） | — |
| 快速命令栏（CRUD/发送语义） | tiny-rdm QuickCommand | ✅ 已有（`frontend/src/lib/quickCommands.ts` 上限 20 + 工具栏 Zap 下拉；发送走 PTY 键盘写入原文保留交互 shell 状态，A-SSH） | — |
| 连接信息面板（Host/Port/User/认证方式/只读/延迟） | 连接信息摘要 | ✅ 已有（App.vue `connection-info-popover`；`ssh/sessions/list` 行新增只读 `authMethod`（model.rs:46 `method_name()`、ssh.rs:599/609/1236/1249），延迟走既有 ssh/exec echo 探测，A-SSH） | — |
| 终端字体缩放（Ctrl/⌘ 滚轮、复位、持久化） | batch3 工作包 A 第 3 条细化 | ✅ 已有（`frontend/src/lib/terminalZoom.ts` `clampFontSize` 绝对字号 [8,32] + App.vue A+/A− 按钮与 localStorage 持久化，A-SSH） | — |
| known_hosts 管理（list/remove，宿主侧文件） | ssh_service.go ListKnownHosts/RemoveKnownHost | ✅ 已有（第一批，实测通过） | — |
| 主机密钥预检/接受/拒绝（profile 维度） | CheckHostKey/Accept/RejectHostKey | ✅ ssh/host-key/check（探针预检三态，真机验证）+ 挑战流程 | P1 完成 |
| 本地 SSH 私钥发现（~/.ssh 扫描 + 指纹） | DiscoverKeys | ✅ 已有（第一批，实测通过）（第一批） | P0 |
| Stat（文件元信息单查） | sftp_service.go Stat | ✅ 已有（第一批，实测通过）（第一批） | P0 |
| Exists / Touch | Exists/Touch | ✅ 已有（第一批，实测通过）（第一批） | P0 |
| 小文件直写 WriteFile（非传输槽） | WriteFile | ✅ 已有（第一批，实测通过）（第一批） | P0 |
| 归档打包（多路径→tar/zip 远端打包） | Archive | ✅ 已有（第一批，实测通过）（第一批）；2026-08-30 起右键对单文件同样提供压缩 | P1 |
| 解压（tar/zip→目录，可覆盖） | Extract | ✅ 已有（第一批，实测通过）（第一批） | P1 |
| 文件管理器交互（双击预览、二进制不打开、大文件确认、预览内编辑保存） | Sftp 界面（文本预览/编辑/压缩） | ✅ 已有（2026-08-30 交互轮，见下文专节） | P1 |
| **Sudo 文件操作族**（无 root 登录下管理 root 文件） | ListDirSudo/ReadFileSudo/WriteFileSudo/MkdirSudo/RemoveSudo/RemoveAllSudo/ChmodSudo/RenameSudo/StatSudo | ✅ 已有（sudo/stat…sudo/rename 共 11 方法，实测通过）；**DownloadSudo 未实现**——大体积 root 文件二进制下载暂退化为 `sudo/readFile`（exec+base64，受包尺寸限制），2026-08-29 对标复核修正口径 | **P0 核心** |
| 终端缓冲区查询（增量 seq） | GetTerminalBuffer | ✅ ssh/terminal/replay | — |
| 命令中止 | AbortCommand | ✅ ssh/exec/cancel | — |
| SSH 指标（延迟/吞吐采样） | ssh_metrics_service.go | ✅ ssh/metrics：CPU/内存/负载/磁盘 + 网络接口速率、Top CPU/内存进程（`topMemory`）、磁盘 inode 使用率（`inodeUsePercent`）、快照缓存（`cached: true` → `cachedAt`，对齐 GetLastSnapshot） | — |
| MCP 尺寸限制策略（max read/upload/download） | PreferencesMCPSFTP | ✅ mcp/settings/get|set（持久化，重启重载，--mcp 同源） | P2 完成 |
| MCP 本地↔远端传输 + 家目录（sftp_upload / sftp_download / sftp_pwd） | SFTPTransfer / sftpPwd | ✅ 已有（2026-08-30）：25 工具齐；单文件传输受 maxUpload/maxDownload 限制，本地路径校验先于拨号、校验拒绝不清连接池；`smoke_mcp.py --host` 真机回环（SHA-256 双端比对） | P2 完成 |
| Profile MCP 策略开关 | UpdateProfileMCPPolicy | ⚠️ 由 DBX 侧承担，插件不重复 | 不做 |
| Profile 级快速 sudo / 执行模式 | UpdateProfileQuickSudo / UpdateProfileSSHExecution | ✅ 已有（2026-08-30，**推翻 2026-08-29「不做」结论**）：全局多套 Quick Sudo 配置集中管理（`sudo/profiles/list|save|delete`，`<plugin_data_dir>/quick-sudo-profiles.json` 持久化，密钥永不回显）+ 连接级绑定选择（`ssh/settings/set quickSudoProfileId`，选全局或本连接，插件侧持久化、重连保留）+ 终端 auto sudo / exec / MCP（`ssh_quick_sudo_profiles_*`、`ssh_exec_sudo quickSudoProfile`）全通道生效；详见 `IMPL_PLAN_QUICK_SUDO.zh-CN.md`；2026-08-31（0.4.2）连接表单升级 `sudo_source` 三选一：不开 / 本连接自定义 / 全局配置（`sudo_profile` 引用，visible_when 联动，存量连接按 `quick_sudo` 映射兼容）；2026-08-31（0.4.5）表单 `sudo_profile` 升级动态下拉（`sudo_profile` 字段声明 `options_action: sudo/profiles/options`，宿主拉取配置列表渲染 select，无该扩展能力的宿主文本回退），`global` 模式隐藏 2FA 四件套（`totp_secret`/`auth_flow_mode`/hints——凭据来源整体由全局配置接管），终端监视器改为随设置/配置更新**重新挂载**（修复连接时无凭据、后在工作台配置 quick sudo 不生效的问题，对齐 tiny-rdm 每次输出动态 resolve） | P1 完成 |
| Profile 分组/排序 | SaveProfileOrganization | DBX 连接管理已承担 | 不做 |
| **SSH 隧道 / 跳板 / 代理（ProxyJump）** | 自建 jump 链 | **整合 DBX 已有能力，不重复实现**：隧道/代理在 DBX 连接编辑"隧道/代理"标签配置（tunnel_profiles）；DBX 先解析传输层，把实际入口以 `runtime.host/port` 传给插件，插件 dial 使用 runtime 端点、主机密钥校验仍以原始 `connection.host/port` 为身份。插件表单已移除 `jump_hosts` 字段避免双轨配置；sidecar 对历史数据保持兼容 | 整合 |

## 第一批任务（本轮指派，纯后端新模块，不改 main.rs 之外的现有文件）

1. `backend/src/sudo_fs.rs` — Sudo 文件操作族（P0）
   - `sudo_fs::{stat, exists, touch, list_dir, read_file, write_file, mkdir, remove, remove_all, chmod, rename}`
   - 复用 `exec.rs` 的 sudo 编排（AuthFlowMode/SudoAuth/Hints）与 ssh.rs 的会话池
   - 语义对齐 tiny-rdm：stat 走 `stat -c`，list 走 `ls -la --time-style=+%s` 解析，
     read 走 `dd`/`base64`，write 走 `dd of=`，remove_all 防符号链接跟随
2. `backend/src/sftp_ext.rs` — 基础补齐（P0）
   - `sftp_ext::{stat, exists, touch, write_file}`（russh-sftp 原生实现，非 sudo）
   - `sftp_ext::{archive, extract}`：tar.gz 打包/解压（远端 `tar` 命令实现，对齐 tiny-rdm）
3. `backend/src/keys.rs` — 密钥发现（P0）
   - `keys::discover()`：扫描 `~/.ssh`（id_rsa/ed25519/ecdsa/…、config 内 IdentityFile），
     返回路径 + 算法 + 指纹（SHA256），不返回私钥内容
   - `host_key` 管理 API：`list_known_hosts()/remove_known_host(host, port)`（host_key.rs 已有，补薄封装）

接线（main.rs match 臂注册）与 smoke 扩展由主会话统一完成，避免多 agent 编辑冲突。

## 第三批任务（已落地，2026-08-28 基线）

2026-08 全量差距复审（tiny-rdm 演化版 MCP CTL 基线）后立项，四个并发工作包：
A 终端体验（搜索/字体缩放/WebLinks/风险粘贴防护/滚动缓冲 25k）、
B SFTP 面板（搜索过滤/多选批量/新建文件/属性弹窗/路径历史/sudo 编辑/服务器内复制粘贴）、
C 后端补齐（sftp/copy+move、sudo 时间戳保活、OTP 防重放）、
D 指标增强（网络接口速率/Top 进程/分区展示）。
实施细节、文件所有权与验收标准见 `FEATURE_PARITY_BATCH3.zh-CN.md`。
四包代码、单测与 smoke 均已通过（并发期间 cargo test 100/100、vitest 21/21、smoke_batch3 7/7）；
合流后 S-A 第二轮（metrics inode/topMemory、sftp/read offset、快照缓存）与 S-B
（OSC 633 命令标记、会话状态展示、输出净化）继续追加；X-B 轮落地 top5 建议仓内四项
（smoke kind 修正 + sftp/list 断言、i18n 七语 key/占位符全对齐断言、smoke_batch3 目录级用例
12→17、终端标记条运行中时长 tick）；A-SSH 轮落地命令历史/快速命令/连接信息（含
`ssh/sessions/list` 增量 `authMethod` 契约）/字体缩放绝对字号钳制。收口终值基线
（2026-08-29 全量复测）：cargo test 109/109、vitest 51/51、五份 smoke 全绿
（smoke_test PASS、smoke_fs 17、smoke_mcp 19 tools、smoke_batch3 17、smoke_sudo_otp 10）、
出包 0.2.2 sha256 `a67bb683…f347e`（明细见 `PROGRESS-COLLECT-FINAL.zh-CN.md` 与
`PROGRESS-TESTBASELINE.zh-CN.md`）。A-SSH 各项归属见 BATCH3「A-SSH 轮核对」节。
git 提交与宿主管线集成验收留待主会话合流后执行。
deferred（本批不做）：多会话分屏、端口转发、SecretRef/审计、远程 SQL——原因见该文档。

## 文件管理器交互轮（2026-08-30）

双击文件是文件管理器的主路径，此前实现存在三处体感缺陷：扩展名白名单外的文本文件
（`Makefile` / `.env` / 无后缀等）与超过 1 MiB 的文本文件双击后**静默转为下载**，预览
形同丢失；二进制文件会先打开预览弹窗再显示徽标，而非"不打开并提示"；大文件无任何
询问。本轮纯前端修复（`frontend/src/App.vue` + `frontend/src/lib/textSniff.ts` 新建）：

- **双击默认预览**：取消扩展名白名单门槛，非图片文件一律走预览（图片 MIME 判定保留）。
- **二进制不打开并提示**：已知二进制扩展名直接提示不打开；无后缀/改名文件在打开前读
  头部 8 KiB 嗅探（`looksBinary`：NUL 字节 / 无效 UTF-8 与控制字符占比 > 10%），命中
  同样只提示不开弹窗（七语 `binaryFile.notOpen`）。
- **大文件先询问**：> 1 MiB 先 confirm（七语 `previewDialog.tooLargeConfirm`），确认后
  仅加载头部 1 MiB 且**只读**，弹窗标题显示截断徽标（`previewDialog.truncated`）；
  并修复原截断分支可编辑保存导致整文件被头部覆盖的数据丢失 bug
  （`previewEditableAllowed` 增加"未截断"约束）。
- **压缩入口放宽**：右键压缩从"仅目录"放开到单文件（归档文件本身除外）；批量压缩、
  解压逻辑不变。
- 后端零改动（`sftp/read` / `sftp/write` / `sftp/archive` / `sftp/extract` 契约不变），
  无新增协议方法与 smoke 用例；新增 `textSniff.spec.ts` 6 用例（vitest 51 → 57）。

## 第二批任务（已完成）

- 前端工作台接入：sudo 模式开关（文件面板工具栏）、归档/解压右键菜单、
  小文件快速编辑保存（WriteFile）、known_hosts 管理入口
- 主机密钥预检 API（连接表单"检查密钥"按钮）
- MCP 设置 API（尺寸限制）
- i18n 七语文案补齐
