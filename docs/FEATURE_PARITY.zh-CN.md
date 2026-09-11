# SSH/SFTP 特性能力清单

插件现状：`backend/src/main.rs` 方法表（**67 个分发方法臂、68 个方法名**——`ssh/host-key/resolve`
与 `connection/challenge/resolve` 共用一臂（main.rs:174），含 sftp/copy、sftp/move、ssh/host-key/check；
2026-08-29 收口复核，修正如下的「68 臂」口径）。
原则：补齐完整 SSH/SFTP 能力面；DBX 已由宿主承担的能力（连接管理、
profile 分组、全局外观）不重复实现。

## 能力对照总表

| 能力 | 参照位置 | 插件状态 | 优先级 |
| --- | --- | --- | --- |
| SFTP 基础（list/read/mkdir/rename/chmod/delete/upload/download/传输槽） | sftp_service.go | ✅ 已有（`sftp/read` 另支持可选 `offset` 分片续读，对齐 ReadFile(offset,length)） | — |
| 目录磁盘占用 diskUsage | sftp_service.go | ✅ 已有 | — |
| 终端 PTY/回放/resize/目录跟随 | ssh_service.go | ✅ 已有 | — |
| ssh/exec + sudo（含 PTY/MFA/TOTP 编排） | sudo_exec_service.go | ✅ 已有（exec.rs）；2026-09-04 OTP 多密钥轮换升级：使用/防重放台账进程全局（跨 exec 调用、跨会话；MCP `ssh_exec_sudo` 每调用独立实例也连续轮换），密钥 SHA-256 指纹+窗口+码键控，静态码 usage 键防时间戳漂移，选择排序对齐 resolveRotatingOTP（未用优先→剩余时长最长→配置顺序） | — |
| ZMODEM rz/sz | ssh_service.go | ✅ 已有 | — |
| 终端 Shell Integration 命令标记（OSC 633：命令/退出码/时长/cwd） | frontend/src/modules/ssh/osc633-parser.js | ✅ 已有（`terminalCommandMarkers.ts` 解析器移植 + 状态条，S-B；运行中时长 1s tick `runningCommandElapsedMs` + `marker-elapsed` span，X-B） | — |
| 会话状态规范化展示（连接中/已连接/重连中/已断开/错误） | frontend/src/modules/ssh/session-status.js | ✅ 已有（`sessionStatus.ts` 移植 + reconnecting 扩展，S-B；多会话择优不适用未移植） | — |
| 命令输出净化（控制序列剥离/回显移除） | frontend/src/modules/ssh/terminal-output.js | ✅ 已有（`terminalOutputText.ts` 通用部分移植；`.mcp_ctl_*` hook 特判不适用） | — |
| 命令历史（弹窗历史区/↑↓ 浏览/一键重发） | SshPage 命令历史 | ✅ 已有（`frontend/src/lib/commandHistory.ts` 环形 100 条 + App.vue 接线；疑似凭据/超长/多行命令不落 localStorage，A-SSH） | — |
| 快速命令栏（CRUD/发送语义） | QuickCommand | ✅ 已有（`frontend/src/lib/quickCommands.ts` 上限 20 + 工具栏 Zap 下拉；发送走 PTY 键盘写入原文保留交互 shell 状态，A-SSH。2026-09-04 起存储升级为**全局**：`ssh/quickCommands/*` + sidecar `quick-commands.json`，所有连接/工作台共享，localStorage 旧数据一次性迁移） | — |
| 批量发送（多会话发送命令） | useBatchSend/SshBatchSendPanel | ✅ 已有（`ssh/terminal/batchInput` 写入各会话 PTY；交互 2026-09-08 改版为终端底部常驻命令条（思路 Electerm quick-command bar）：回车即发送、目标选择 popover（全选/仅存活/刷新）、快速命令下拉切换回填、内联保存为快速命令（`ssh/quickCommands/*` 全局共享 ≤20）、结果浮条逐会话汇总；发送成功清空入历史、↑↓ 回选；跨工作台草稿/开关经 `ssh/batchBar/state` 广播同步；危险命令复用粘贴确认；输出不收集回显在各自终端；并发同靶 OTP 提示撞重放保护时延迟到下一窗口自动补答（2026-09-08） | — |
| 连接信息面板（Host/Port/User/认证方式/只读/延迟） | 连接信息摘要 | ✅ 已有（App.vue `connection-info-popover`；`ssh/sessions/list` 行新增只读 `authMethod`（model.rs:46 `method_name()`、ssh.rs:599/609/1236/1249），延迟走既有 ssh/exec echo 探测，A-SSH） | — |
| 终端字体缩放（Ctrl/⌘ 滚轮、复位、持久化） | batch3 工作包 A 第 3 条细化 | ✅ 已有（`frontend/src/lib/terminalZoom.ts` `clampFontSize` 绝对字号 [8,32] + App.vue A+/A− 按钮与 localStorage 持久化，A-SSH） | — |
| 点击定位光标（iTerm2/kitty 风格）+ 细竖线光标 | iTerm2 Option+Click / kitty click-to-move | ✅ 已有（2026-09-09：`frontend/src/lib/terminalClickCursor.ts` 纯几何计算——同逻辑行内原地点击按字符差值代发左右方向键，宽字符 2 格记 1、折行跨行展开、备用屏/鼠标上报应用/trzsz·zmodem 占流一律不动作；光标 `cursorStyle: "bar"`。终端协议无直接落点能力，readline 只认按键，行外点击不动作防翻历史） | — |
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
| Profile 级快速 sudo / 执行模式 | UpdateProfileQuickSudo / UpdateProfileSSHExecution | ✅ 已有（2026-08-30，**推翻 2026-08-29「不做」结论**）：全局多套 Quick Sudo 配置集中管理（`sudo/profiles/list|save|delete`，`<plugin_data_dir>/quick-sudo-profiles.json` 持久化，密钥永不回显）+ 连接级绑定选择（`ssh/settings/set quickSudoProfileId`，选全局或本连接，插件侧持久化、重连保留）+ 终端 auto sudo / exec / MCP（`ssh_quick_sudo_profiles_*`、`ssh_exec_sudo quickSudoProfile`）全通道生效；2026-08-31（0.4.2）连接表单升级 `sudo_source` 三选一：不开 / 本连接自定义 / 全局配置（`sudo_profile` 引用，visible_when 联动，存量连接按 `quick_sudo` 映射兼容）；2026-08-31（0.4.5）表单 `sudo_profile` 升级动态下拉（`sudo_profile` 字段声明 `options_action: sudo/profiles/options`，宿主拉取配置列表渲染 select，无该扩展能力的宿主文本回退），`global` 模式隐藏 2FA 四件套（`totp_secret`/`auth_flow_mode`/hints——凭据来源整体由全局配置接管），终端监视器改为随设置/配置更新**重新挂载**（修复连接时无凭据、后在工作台配置 quick sudo 不生效的问题，对齐每次输出动态 resolve 的语义） | P1 完成 |
| Profile 分组/排序 | SaveProfileOrganization | DBX 连接管理已承担 | 不做 |
| **SSH 隧道 / 跳板 / 代理（ProxyJump）** | 自建 jump 链 | **整合 DBX 已有能力，不重复实现**：隧道/代理在 DBX 连接编辑"隧道/代理"标签配置（tunnel_profiles）；DBX 先解析传输层，把实际入口以 `runtime.host/port` 传给插件，插件 dial 使用 runtime 端点、主机密钥校验仍以原始 `connection.host/port` 为身份。插件表单已移除 `jump_hosts` 字段避免双轨配置；sidecar 对历史数据保持兼容 | 整合 |

## 第一批任务（本轮指派，纯后端新模块，不改 main.rs 之外的现有文件）

1. `backend/src/sudo_fs.rs` — Sudo 文件操作族（P0）
   - `sudo_fs::{stat, exists, touch, list_dir, read_file, write_file, mkdir, remove, remove_all, chmod, rename}`
   - 复用 `exec.rs` 的 sudo 编排（AuthFlowMode/SudoAuth/Hints）与 ssh.rs 的会话池
   - 语义：stat 走 `stat -c`，list 走 `ls -la --time-style=+%s` 解析，
     read 走 `dd`/`base64`，write 走 `dd of=`，remove_all 防符号链接跟随
2. `backend/src/sftp_ext.rs` — 基础补齐（P0）
   - `sftp_ext::{stat, exists, touch, write_file}`（russh-sftp 原生实现，非 sudo）
   - `sftp_ext::{archive, extract}`：tar.gz 打包/解压（远端 `tar` 命令实现）
3. `backend/src/keys.rs` — 密钥发现（P0）
   - `keys::discover()`：扫描 `~/.ssh`（id_rsa/ed25519/ecdsa/…、config 内 IdentityFile），
     返回路径 + 算法 + 指纹（SHA256），不返回私钥内容
   - `host_key` 管理 API：`list_known_hosts()/remove_known_host(host, port)`（host_key.rs 已有，补薄封装）

接线（main.rs match 臂注册）与 smoke 扩展由主会话统一完成，避免多 agent 编辑冲突。

## 第三批任务（已落地，2026-08-28 基线）

2026-08 全量差距复审后立项，四个并发工作包：
A 终端体验（搜索/字体缩放/WebLinks/风险粘贴防护/滚动缓冲 25k）、
B SFTP 面板（搜索过滤/多选批量/新建文件/属性弹窗/路径历史/sudo 编辑/服务器内复制粘贴；
0.4.x 增补：面板可收起/打开按钮 + 默认不打开偏好、DBX 重启恢复的
「Connection is not active」快速失败提示，见 PROGRESS-P-SSH §8.3）、
C 后端补齐（sftp/copy+move、sudo 时间戳保活、OTP 防重放）、
D 指标增强（网络接口速率/Top 进程/分区展示）。
实施细节、文件所有权与验收标准记录在批次对标文档中（已随批次退役删除，见 git 历史）。
四包代码、单测与 smoke 均已通过（并发期间 cargo test 100/100、vitest 21/21、smoke_batch3 7/7）；
合流后 S-A 第二轮（metrics inode/topMemory、sftp/read offset、快照缓存）与 S-B
（OSC 633 命令标记、会话状态展示、输出净化）继续追加；X-B 轮落地 top5 建议仓内四项
（smoke kind 修正 + sftp/list 断言、i18n 七语 key/占位符全对齐断言、smoke_batch3 目录级用例
12→17、终端标记条运行中时长 tick）；A-SSH 轮落地命令历史/快速命令/连接信息（含
`ssh/sessions/list` 增量 `authMethod` 契约）/字体缩放绝对字号钳制。收口终值基线
（2026-08-29 全量复测）：cargo test 109/109、vitest 51/51、五份 smoke 全绿
（smoke_test PASS、smoke_fs 17、smoke_mcp 19 tools、smoke_batch3 17、smoke_sudo_otp 10）、
出包 0.2.2 sha256 `a67bb683…f347e`（明细记录文档已随批次退役删除，见 git 历史）。
A-SSH 各项归属见原批次「A-SSH 轮核对」记录（该文档已删除）。
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

## tssh 对标补充（2026-09-07，0.4.34，feat/ssh-tssh-parity 分支）

以 [trzsz-ssh（tssh）](https://github.com/trzsz/trzsz-ssh) 为参照做的特性追赶
（worktree `.worktrees/feat-ssh-tssh-parity`，并发双工作包，详见
`PROGRESS-P-SSH.zh-CN.md` 同日章节）：

| tssh 能力 | 插件状态 | 说明 |
| --- | --- | --- |
| trz/tsz 文件传输 | ✅ 已有 | 前端集成 trzsz.js 1.1.6（`TrzszFilter` 流式挂接，与 zmodem 共存互斥；announce 自动接管 + 右键 Upload (trz) 触发；上传走 File API、下载宿主 fileTransfer 优先/浏览器 `<a download>` 兜底；WKWebView 无 File System Access API 的接缝已覆写处理）。唯一依赖豁免，理由见 PROGRESS |
| SetEnv | ✅ 已有 | 连接表单 `set_env`（textarea，多行 `KEY=VALUE`，分号兼容，严格校验非法条目即连接失败；0.4.35 前表单 key 为 `setEnv`，sidecar 兼容读取）；交互 shell 通道 + exec/sudo 命令通道统一注入（`exec.rs merge_channel_env`，用户条目覆盖内置默认）；fire-and-forget 语义与 ssh(1) 一致（服务端无 AcceptEnv 时静默不生效，文档已注明） |
| RemoteCommand | ✅ 已有 | 连接表单 `remote_command`（非空生效；0.4.35 前表单 key 为 `remoteCommand`，sidecar 兼容读取）：交互会话 PTY 照常、exec 替代 shell request；命令退出即会话终止（与 `ssh host command` 同语义）；reattach 重放属预期 |
| 端口转发（-L/-R/-D） | ❌ 不做 | **2026-09-07 用户决策**：宿主已有 ssh 隧道实现（连接代拨模型，`dbx-core/src/db/ssh_tunnel.rs` 支持 -L/-D 内部转发），插件不重复。宿主盘点结论：宿主无 -R、无面向用户的转发会话 UI、插件桥无任意 host:port 转发接口；若未来需要，需宿主侧立项（通用转发 API + 工作台面板） |
| Agent 转发（ForwardAgent/-A） | ❌ 不做 | **2026-09-07 用户决策**：转发类特性不做（认证侧 ssh-agent 已支持：SSH_AUTH_SOCK/自定义 socket/Pageant/agent 内证书身份，见 `ssh.rs authenticate_agent`） |
| mosh/UDP 漫游、X11、GSSAPI、ControlMaster、SSH console | ❌ 不做 | 需自研服务端组件/大额自研、或宿主已承担（连接管理/凭据）、或 GUI 客户端不适用；理由见 review 结论 |
| 批量登录、登录选择器/分组、记住密码、自动重连 | ✅ 已有（等价） | 分别对应批量发送（跨连接活跃会话）、DBX 连接管理、宿主 secret binding、断线自动重连 |

## openocta 对标补充（2026-09-11，0.4.52）

以 [openocta/openocta](https://github.com/openocta/openocta)（AIOps 运维智能体平台，
Go Gateway + Agent Runtime + Skills + MCP 客户端）为参照的运维闭环能力借鉴
（实施计划 `docs/IMPL_PLAN_SSH_APPROVAL_AUDIT_ALERT.zh-CN.md`；SSH 传输层不对标
——对方依赖系统 openssh-clients，本插件自研 russh 深度领先）：

| openocta 能力 | 插件状态 | 说明 |
| --- | --- | --- |
| 审批队列持久化（`src/pkg/security/approval_queue.go` 按 storePath 单例 + 持久化批准存储） | ✅ 已有（等价增强） | 审批弹窗「记住此命令」（`ssh/agent/resolve remember`）→ 连接级免审批清单 `agent-approved-commands.json`，匹配复用 sudoers 式 token 语义、可手工泛化通配；破坏性命令拒绝入库且命中重检无效（灾难门永不绕过） |
| 执行可追溯（事件总线 + 审批中间件） | ✅ 已有（SSH 域内） | 执行审计 JSONL（`audit-log.jsonl`，5 MiB 轮转）：MCP/AI 执行面每调用一条（gate/outcome/exitCode/耗时）+ 审批生命周期一条（approved/denied/timeout/remembered）；`ssh/audit/list` 只读回放；工作台人工操作按既定信任模型不记 |
| 告警标准化 + 固定分析 Prompt（`/hooks/alert`） | ✅ 已有（插件侧形态） | `ssh_alert_triage` 工具 + `ssh/alert/triage` RPC：异构告警 JSON/纯文本 → 结构化（相同兼容语义：解析失败整包当 message）+ 双语关键词分类 + 白名单级只读诊断命令清单；分析与执行分离（分诊在插件、推理在外部 Agent），不接收 webhook（宿主契约之外） |
| 主机巡检场景（`deploy/scenarios/host-inspection`） | ⏸ 未做（另有对标项） | 一键巡检报告（复用 `ssh/metrics` + `ssh/exec` 的配方化组织）列为后续候选，见对比评审结论 #1 |
| 定时调度（`src/pkg/cron`）、IM 渠道指挥（channels）、数字员工（employees） | ❌ 不做 | 宿主/生态层职责（sidecar 生命周期受宿主管理，长期定时任务不合适；IM 渠道超出插件契约）；角色预设包（快速命令组 + sudo 白名单模板组合）列为后续候选 |

## sshbool 对标补充（2026-09-11，0.4.52）

以 [omarsenusi/sshbool](https://github.com/omarsenusi/sshbool)（Tauri v2 + russh +
React 19 独立桌面 SSH 工作台）为参照的能力借鉴（实施计划
`docs/IMPL_PLAN_SSH_VAULT_TRANSFER_HISTORY.zh-CN.md`；SSH 传输层双方同级单连接
多路复用，其每次操作新开 SFTP channel 的做法劣于本插件会话级复用）：

| sshbool 能力 | 插件状态 | 说明 |
| --- | --- | --- |
| 本地凭据静态加密（SQLCipher 库 + Argon2id KEK + AES-256-GCM DEK 信封） | ✅ 已有（keyfile 默认档） | `quick-sudo-profiles.json` 升 v2：密钥字段 AES-256-GCM 信封（AAD 绑定字段+档案 id）、DEK 默认存同目录 0600 keyfile，OS keychain 仅显式选入（`DBX_SSH_VAULT_STORAGE=keychain`；macOS 每次更新都会重弹授权框，见 IMPL_PLAN D2a）、遗留 keychain 档自动迁移；v1 自动迁移、解密失败按空降级；实施计划特性 A |
| 传输任务持久化（`transfer_jobs`/`transfer_items` 表） | ✅ 已有（轻量形态） | `transfer-history.json` 环形 200 条 + `sftp/transfer/history` 持久化/live 合并查询，仅状态跃迁落盘；断点续传/逐文件 resume 不做（登记后续候选）；实施计划特性 B1 |
| SFTP 书签（`sftp_bookmarks` 表 + 双栏书签） | ✅ 已有 | `sftp-bookmarks.json` + `sftp/bookmarks/list` / `save` / `delete` + 路径栏星标收藏/下拉跳转（七语）；实施计划特性 B2 |
| 主密码 Vault（解锁屏 / 自动锁定 / 生物识别 / FIDO2） | ❌ 不做 | 宿主插件形态下无人值守 sudo 自动应答要求重启免解锁；keychain 托管已覆盖“防拷贝/备份外泄”目标，主密码模型收益不成立 |
| 端口转发（`channel_open_direct_tcpip` ProxyJump/本地转发，数据库面板经隧道连 DB） | ❌ 不做 | 沿 2026-09-07 用户决策（宿主已有 ssh 隧道实现，见 tssh 节）；sshbool 的 russh direct-tcpip 用法留作未来宿主侧通用转发 API 的参考 |
| 监控历史趋势（`host_snapshots` + 分桶 `metric_series` 落盘 + 趋势图） | ⏸ 未做（候选） | 现有 `ssh/metrics` 单快照维度更全（inode/Top 进程/每接口速率）；快照环形落盘 + sparkline 趋势列为后续候选 |
| 终端 BiDi/阿拉伯语变形（`arabic-xterm.ts` 词级 reshape 保词序 + shell UTF-8 locale） | ⏸ 未做（候选） | xterm.js 原生无 BiDi/shaping；本插件 UI 七语无阿拉伯语，但终端输出内容可能含 RTL 文本，shaping 管线可放 `shared/frontend/` 公共层单点实现 |
| 审计账 + 审计面板（`audit_log` 表 + audit-panel） | ✅ 已有（SSH 域内） | MCP/AI 执行面 JSONL 审计 + `ssh/audit/list`，见上节 openocta 对标（本批前已落地） |
