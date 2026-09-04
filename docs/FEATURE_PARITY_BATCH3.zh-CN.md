# tiny-rdm 对齐第三批实施文档（batch 3）

基线：`/Users/Jinpy/GolandProjects/tiny-rdm`（tiny-rdm 演化版 MCP CTL，本地源码）。
前置：第一批/第二批见 `FEATURE_PARITY.zh-CN.md`（后端能力面已大体对齐）。
本批目标：补齐差距分析（2026-08）中确认的 **UI 体验层差距** 与 **少量后端缺口**。

## 总原则（所有工作包必须遵守）

1. **UI 风格遵循插件现有规范**：手写 CSS（`wb-*` 类体系）、lucide 图标、现有弹窗/右键菜单/工具栏模式。**禁止引入任何 UI 框架**（无 naive-ui 等）。布局与功能对齐 tiny-rdm，视觉不强行改造。
2. **tab 类标签归 DBX 宿主**：连接级多标签由宿主 workbench 承担，插件不自建连接标签系统。本批"多会话/分屏"不做（见 deferred）。
3. **SFTP 面板布局可紧跟 tiny-rdm**（搜索栏、类型过滤、多选批量条、路径历史等），但组件写法与样式沿用现有 `wb-*` 体系，不破坏既有布局。
4. **i18n 纪律**：所有新增文案进 `frontend/src/lib/i18n.ts` 的 `messages` 主对象 **7 个语言块全部补齐**；每个工作包只在自己域的锚点 key 之后用 **Edit 局部插入**，**禁止 Write 重写整个 i18n.ts**。key 沿用现有扁平驼峰风格，按工作包前缀命名。
5. **文件所有权**（防止并发冲突，见下表）。只允许 Edit 局部修改，禁止 Write 重写 App.vue。
6. 完成后必须跑各自验证命令；**禁止 git commit/push**。

### 并发文件所有权矩阵

| 文件 | A 终端UX | B SFTP面板 | C 后端 | D 指标 |
| --- | --- | --- | --- | --- |
| `frontend/src/App.vue` | 终端函数区(L~490-560、`pasteTerminal` L~1559)+终端右键菜单模板区 | SFTP 函数/模板区 | 禁入 | 指标弹窗模板+其函数 |
| `frontend/src/lib/i18n.ts` | 锚点 `zmodemUploadOnly` 后 | 锚点 `newFolder` 后 | 禁入 | 锚点 `metricsRefreshHint` 后 |
| `frontend/src/components/`、`frontend/src/lib/` 新文件 | ✅ | ✅ | 禁入 | 禁入 |
| `frontend/src/lib/workbench.spec.ts` | ✅ 追加用例 | ✅ 追加用例 | 禁入 | ✅ 追加用例 |
| `backend/src/sftp_copy.rs`(新) | — | — | ✅ | — |
| `backend/src/metrics.rs`(新) | — | — | — | ✅ |
| `backend/src/main.rs` | — | — | 仅 `sftp/extract` 臂后加 `sftp/copy`、`sftp/move` 两臂 | 仅 `ssh/metrics` 臂内改动 |
| `backend/src/ssh.rs` | — | — | 仅 SFTP 区域新增委托 | 仅 metrics 组装处 |
| `backend/src/exec.rs` | — | — | 仅 sudo 保活/OTP 防重放区域（`AuthFlowMode`/`exec_*` 附近） | 仅 `collect_metrics` 改为委托 metrics.rs 的一处小改 |
| `backend/src/mcp.rs` | — | — | ✅ 工具定义 | 禁入 |
| `backend/src/model.rs`、`sudo_fs.rs`、`keys.rs`、`host_key.rs` | — | — | 按需 | 禁入 |
| `frontend/package.json` | 仅 A 可加 `@xterm/addon-search`、`@xterm/addon-web-links`（pnpm） | 禁改 | — | 禁改 |

## 工作包 A：终端体验对齐（终端搜索 / 缩放 / 链接 / 粘贴防护）

对标：tiny-rdm `TerminalPane.vue`、`TerminalSearchPanel.vue`、`dangerous-commands.js`。

1. **终端搜索**：安装 `@xterm/addon-search`；新建 `frontend/src/components/TerminalSearchPanel.vue`（输入框 + 大小写/正则/整词三个开关 + 上一个/下一个 + 关闭；样式对齐现有工具栏 popover）。入口：终端右键菜单"搜索" + `Cmd/Ctrl+F`（经 `attachCustomKeyEventHandler` 或容器 keydown）。Esc 关闭并还焦点给终端。无匹配/有匹配状态提示。
2. **URL 可点击**：安装 `@xterm/addon-web-links` 并加载。
3. **字体缩放**：终端区 `Ctrl+滚轮` 缩放字号（clamp 8–32，步进 1），`Ctrl+0` 复位；缩放后触发 fit。沿用现有字体设置（宿主下发）为基准值。
4. **风险粘贴确认**：改造 `pasteTerminal`（及终端内 `onPaste`/右键粘贴路径）：剪贴板含换行或 ≥200 字符 → 弹确认（复用现有弹窗模式），显示行数/字符数与 400 字预览；内容命中 `dangerousCommands.ts` 的正则（对齐 tiny-rdm：`rm -rf`、`mkfs`、`dd of=`、`:(){`、`shutdown`、`reboot`、`chmod -R 777 /`、`> /dev/sda` 等）→ 危险级确认（红色强调）。记住"本次会话不再提示"选项可不做（保持简单）。
5. **滚动缓冲**：`scrollback: 10_000` → `25_000`（对齐 tiny-rdm 默认）。
6. 新建 `frontend/src/lib/dangerousCommands.ts`（正则数组 + `inspect(text)` 返回风险级别与命中项），并在 `workbench.spec.ts` 追加用例。
7. i18n 前缀：`terminalSearch*`、`terminalPasteConfirm*`、`terminalDanger*`、`terminalZoom*`。锚点：各语言块 `zmodemUploadOnly` 之后。

验收：`pnpm --dir frontend typecheck && pnpm --dir frontend test`（vitest）通过；mock 页面（`frontend/mock.html`）手动可搜可缩放；危险粘贴弹确认。

## 工作包 B：SFTP 面板对齐（搜索/批量/属性/新建文件/路径历史/sudo 编辑/复制粘贴）

对标：tiny-rdm `SshSftpPanel.vue`、`SftpChmodDialog.vue`、`FileCopyMoveDialog.vue`。布局可紧跟 tiny-rdm，样式沿用 `wb-*`。

1. **名称搜索 + 类型过滤**：SFTP 工具栏加搜索输入（前端过滤当前列表）+ 类型下拉（全部/文件夹/文件）。footer 统计显示"命中/总数"。
2. **多选批量**：文件表支持多选（checkbox 或 ctrl/shift 点击，沿用现有交互习惯）；选中 ≥2 项出现批量条：批量删除（确认弹窗）、批量打包（仅所选均为目录/文件可打包，逐个调 `sftp/archive`）。
3. **新建文件**：工具栏"新建文件"：文件名输入 → `sftp/touch`；sudo 模式下走 `sudo/touch`。
4. **属性弹窗**：右键"属性" → `sftp/stat`（sudo 模式 `sudo/stat`）展示：路径/类型/大小/权限（八进制）/属主/修改时间；弹窗内可直接改权限（复用现有 chmod 逻辑）。新建组件 `frontend/src/components/SftpAttributesDialog.vue` 或并入现有弹窗体系（跟随现有弹窗实现模式）。
5. **路径历史 + 快捷路径**：路径输入旁加历史下拉（每连接最近 10 条，存 localStorage `sftp-path-history`）与快捷项（`/`、`/home`、`/tmp`、`/etc`、`/var`、`/root`）。
6. **sudo 模式编辑文件**：sudo 模式下"预览/打开"文本文件改用 `sudo/readFile`；TextPreview 编辑保存走 `sudo/writeFile`（≤4 MiB，与 `sftp/write` 上限一致，超出提示用下载/上传）。非 sudo 路径保持现状。
7. **服务器内复制/剪切/粘贴**：右键菜单加"复制/剪切"（记录选中路径与模式，仅本连接内有效）；目标目录右键/工具栏"粘贴" → 逐项调 `sftp/copy`、`sftp/move`（后端由工作包 C 本批落地；若后端返回未知方法错误，toast 友好提示"需要更新后端"）。目标存在时先 `sftp/exists` 检测并弹覆盖确认。
8. i18n 前缀：`sftpSearch*`、`sftpFilter*`、`sftpBatch*`、`sftpNewFile*`、`sftpAttrs*`、`sftpPathHistory*`、`sftpQuickPath*`、`sftpCopy*`、`sftpPaste*`。锚点：各语言块 `newFolder` 之后。

验收：typecheck + vitest 通过；mock 页面验证搜索/过滤/多选/新建文件/属性弹窗；`sftp/copy`、`sftp/move` 调用路径与 C 的方法名/参数严格一致（见工作包 C 的 API 契约）。

## 工作包 C：后端补齐（sftp copy/move、sudo 保活、OTP 防重放）

对标：tiny-rdm `sftp_service.go`（FsCopyMove 语义）、`sudo_exec_service.go`（sudoKeepaliveLoop/refreshSudoTimestamp）、`ssh_service.go`（OTP 防重放 isOTPUsageMarked）。

1. **`sftp/copy` / `sftp/move`**：新建 `backend/src/sftp_copy.rs`。参数契约（B 依赖此契约实现）：
   `{ connectionId, from: string | string[], toDir: string, overwrite?: bool }` → 逐项执行，返回 `{ success, results: [{ from, to, ok, error? }] }`。
   实现：优先走会话内 shell（复用 `exec_plain` 通道）`cp -a --` / `mv -f --`（`shell_quote` 全部参数；overwrite=false 时目标存在则报错，可先 `test -e` 探测）。move 也可直接用 SFTP rename 失败再回退 shell mv。支持目录递归。sudo 模式不做（tiny-rdm 亦无 sudo copy）。
   `main.rs` 注册两臂（`sftp/extract` 臂之后）；`mcp.rs` 追加 `sftp_copy` 工具定义（只读域外的写操作，按现有 MCP 写操作风格标注）。
2. **sudo 时间戳保活**：`exec.rs` sudo 认证成功后启动保活循环（每 4 分钟 `sudo -v` 校验/续期，复用 `validate_sudo_timestamp`；连续失败 2 次停止循环并打日志），连接断开（`disconnect_connection`/会话清理）时停止。避免重复循环（每连接单例）。
3. **OTP 防重放**：`exec.rs` 认证编排中记录最近使用的 TOTP 码；同一码在其有效窗口内（含 ±1 步长）被再次要求时不再提交（跳过等待用户输入），并在编排日志标注。内存态即可（per connection），重启清零。
4. **Agent forwarding（可选，时间允许才做）**：调研 russh 0.60 的 channel agent forwarding 支持；若非小改（>50 行或需要新依赖），在本文档"deferred"登记并停止。默认不做。

验收：`cargo test --manifest-path backend/Cargo.toml` 通过（为新逻辑补单测：copy/move 参数解析与结果结构、OTP 防重放窗口判断；shell 命令拼装用 mock/纯函数测试）；`scripts/test.sh --skip-host` 后端段通过。

## 工作包 D：指标增强（网络速率 / Top 进程 / 分区展示）

对标：tiny-rdm `ssh_metrics_service.go`（网络接口 rx/tx 速率、Top 进程）+ 前端 SshMetricsFloat 多面板。

1. **后端**：新建 `backend/src/metrics.rs`：在现有采集命令基础上追加（保持 POSIX 单命令风格）：
   - 网络：两次读取 `/proc/net/dev`（Linux）或 `netstat -ibn`（macOS），中间 `sleep 1`，解析出每接口 rx/tx 速率（B/s）与累计量；
   - 进程：`ps` 按 CPU 取前 8（pid/user/cpu%/mem%/command）。
   输出扩展 JSON（`network: [{name, rxRate, txRate, rxTotal, txTotal}]`、`processes: [...]`）；`exec.rs::collect_metrics` 改为薄委托（保持函数签名或返回结构加 serde default 新字段，向后兼容）；`model.rs` 禁改（结构体定义放 metrics.rs）。解析函数全部可单测（喂样例输出文本）。
2. **前端**：指标弹窗改为三个分区（沿用现有弹窗样式，可用简单分节标题，不引入 tab 组件——若确需分节切换，用现有按钮组模式）：概览（现状：CPU/内存/负载/磁盘）、网络（每接口 rx/tx 速率条）、进程（Top 表格）。自动刷新保持 5s（现有 `metricsRefreshHint` 行为不变）。
3. i18n 前缀：`metricsNetwork*`、`metricsProc*`。锚点：各语言块 `metricsRefreshHint` 之后。
4. `workbench.spec.ts` 追加网络/进程解析（前端如有镜像解析则测；纯后端解析则 C/D 各自的 Rust 单测覆盖）。

验收：`cargo test` 通过（喂 Linux/macOS 两种样例输出）；前端 typecheck + vitest 通过；mock 页面打开指标弹窗可见三分区。

## 本批不做（deferred，含原因）

| 项 | 原因 |
| --- | --- |
| 多会话标签 / 分屏 / 最近关闭恢复 | 架构级：插件是单 workbench 单连接上下文，连接级 tab 归 DBX 宿主；需先与宿主确认多会话语义。**批量发送已于 2026-09-04 以跨连接形态落地**（`ssh/terminal/batchInput` + 工作台批量弹窗，目标来自 `ssh/sessions/list` 全部活跃会话，见 IMPL_PLAN_BATCH_QUICK），多会话语义不再是前置 |
| 端口转发 -L/-R | 归属已核实（2026-09-05，见 ssh/docs/REVIEW_FORM_VS_TABBY.zh-CN.md §三）：宿主已有传输层隧道（ssh/proxy/http_tunnel）与本地转发端点机制且插件连接可用，-L/-R 用户面功能自然归宿主；插件不重复（硬性规则 3） |
| SecretRef/vault 化密钥、审计、MCP 写入白名单 | 凭证与审批体系归宿主 |
| 远程 SQL 会话、终端锁定、终端主题配色切换 | 属 tiny-rdm 数据库域/全局外观域，宿主承担 |
| 终端缓冲查看器模态、路径拖拽上传增强 | 低优先，下批评估；**终端侧拖放上传已于 2026-09-02 落地**（terminal 窗格 drop → uploadLocalFiles，只读/ZMODEM 拒绝，见 PROGRESS-P-SSH §8.7），SFTP 面板侧增强仍 deferred |

## 集成与最终验证（主会话执行）

四个工作包合入后统一执行：

```bash
pnpm --dir frontend typecheck
pnpm --dir frontend test
cargo test --manifest-path backend/Cargo.toml
scripts/build.sh   # 产出自包含 ui/index.html，确认无构建报错
```

冲突处理原则：以 `git diff` 审阅四个 agent 的改动；i18n.ts 若出现插入位置相邻导致的重复 key，以语义正确者保留；App.vue 冲突手工合并后必须重跑全部验证。

## 实施状态核对（2026-08-28 基线，S-C 并发期间核对）

四个工作包代码、单测、smoke 均已落地并通过（以读到的当前工作区代码为准）：

| 工作包 | 状态 | 真机/单测证据 |
| --- | --- | --- |
| A 终端体验 | ✅ 已落地 | SearchAddon/WebLinksAddon 装载（App.vue:5-6,573,576）、TerminalSearchPanel.vue、scrollback 25_000（App.vue:569）、危险粘贴确认（buildPasteConfirmation）、Ctrl 滚轮缩放 clamp 8–32；vitest dangerous/paste/zoom/locale 用例全过 |
| B SFTP 面板 | ✅ 已落地 | sftpFileFilters/sftpPathHistory、批量条（删除/打包）、属性弹窗（sftpAttrs）、新建文件、路径历史；`sftp/copy`/`sftp/move` 调用契约与 C 的 wire 参数一致（smoke_batch3 真机验证） |
| C 后端补齐 | ✅ 已落地 | sftp_copy.rs + main.rs:324/333 两臂 + mcp.rs 两工具；sudo 保活（exec.rs:300-313,781-790，连续 2 次失败停止）、OTP 防重放 ±1 窗口（exec.rs:275-296）；smoke_batch3 7/7 真机。C4 Agent forwarding 按约定未做（非小改），未登记 deferred（默认不做） |
| D 指标增强 | ✅ 已落地 | metrics.rs（Linux/macOS 双解析 12 单测）、ssh/metrics 扩展字段、前端网络/进程分区（未上报时降级隐藏）；smoke_batch3 真机 eth0/lo 速率 + Top 进程 |

集成验证四条命令（上文「集成与最终验证」）已于 2026-08-28 全部通过：
`cargo test` 100/100、`pnpm typecheck` 过、`pnpm test` 21/21、`scripts/build.sh` 等价前端构建
产出 `ui/index.html`、`dbx-plugin package .` 出包 0.2.2 darwin-arm64 完整
（明细与并发漂移声明见 `PROGRESS-TESTBASELINE.zh-CN.md`）。
遗留：git 未提交；宿主 `plugin_tools_bridge` 集成段（MCP 桥）待宿主合流后必跑。

## 合流后增强核对（2026-08-28 晚，S-C 全量收口复核）

四工作包之上，S-A 第二轮与 S-B 各自追加了 tiny-rdm 对标增强，全部落地并通过终值基线
（cargo test 107/107、vitest 35/35、smoke_batch3 真机 12/12、出包 0.2.2 sha256
`173985ff…4713`，明细见 `PROGRESS-TESTBASELINE.zh-CN.md`）：

| 项 | 对标 | 落地证据 | 验证 |
| --- | --- | --- | --- |
| ssh/metrics 快照缓存 | `GetLastSnapshot` | ssh.rs:1823（`metrics(session_id, cached)`）、ssh.rs:581（`cached_metrics_payload`）、main.rs:345（参数臂） | cargo test `metrics_snapshot_cache_*` / `cached_metrics_payload_*`；smoke_batch3「serves the cached snapshot」真机 |
| metrics 磁盘 inode + Top 内存进程 | `SSHDiskStat.InodeUsePercent` / `TopMem` | metrics.rs:76/103（`topMemory`/`inodeUsePercent` 组装）；GNU/busybox 双布局解析单测 3 个 | smoke_batch3「reports inode usage and top memory processes」真机 |
| sftp/read 可选 `offset` 分片续读 | `ReadFile(offset, length)` | ssh.rs:1656-1666（seek 语义，超界空读）、main.rs:205（臂）、mcp.rs:516/1102（schema+描述）；宿主 fs/read、sudo/readFile 同步 | cargo test `optional_u64_*` 等；smoke_batch3「sftp/read honors offset paging」真机（head/offset 续读/超界三段） |
| OSC 633 命令标记 | `frontend/src/modules/ssh/osc633-parser.js` | `frontend/src/lib/terminalCommandMarkers.ts`（解析器移植）+ App.vue:56/352/796/2682（接入与状态条） | vitest 8 用例（跨 chunk Cwd、双终止符、A/E/C/D 全周期、ST 半帧等） |
| 会话状态展示 | `frontend/src/modules/ssh/session-status.js` | `frontend/src/lib/sessionStatus.ts` + App.vue:57/440/2612（session-pill）；`reconnecting` 为插件扩展；多会话择优不适用未移植 | vitest 3 用例；i18n `sessionStatus.*` 七语 7 块齐 |
| 命令输出净化 | `frontend/src/modules/ssh/terminal-output.js` | `frontend/src/lib/terminalOutputText.ts` + App.vue:58/441（`commandOutputText` computed） | vitest 3 用例（OSC/CSI 剥离、回显移除、exec 输出纯文本化） |

## X-B 轮核对（2026-08-29 收口补记，TESTBASELINE §5 top5 仓内四项）

| 项 | 对标/来源 | 落地证据 | 验证 |
| --- | --- | --- | --- |
| smoke_test.py `kind` 字段修正 + `sftp/list` 断言 | TESTBASELINE O1 / §5.3 | scripts/smoke_test.py:153-159（`entry.get("kind")` + kind 取值域 `{file,directory,symlink,other}` 与 `uri` 前缀断言） | 真机 PASS 1.1s，`None` 噪音消除 |
| i18n 七语 key/占位符全量对齐断言 | TESTBASELINE §5.2 | i18n.ts `workbenchMessageTable` dev 导出 + workbench.spec.ts:111（七语 key 集双向全等）/:129（`{placeholder}` 集合与 en 一致）；首轮即抓出并补齐 es/it/ja/pt-BR 各缺 9 个 supplemental key | vitest 2 用例（35→38） |
| smoke_batch3 目录级/边界真机用例（12→17） | TESTBASELINE O4 / §5.1 | scripts/smoke_batch3_test.py:511-519 等 5 例：目录递归 copy（`cp -a`）、`from` 单字符串形态、目录 move、目录目标冲突 block、`overwrite` 替换文件目标 | smoke_batch3 17/0/0 真机 |
| 终端标记条运行中时长 tick | TESTBASELINE §5.4 | terminalCommandMarkers.ts `runningCommandElapsedMs`（纯函数）+ App.vue:389-393/879（1s `setInterval` tick，"D"/"A" 帧停表回落终值）+ `marker-elapsed` span | vitest tick 用例 7 组断言 |

## A-SSH 轮核对（2026-08-29，终端工作台专业细节增强）

对标 tiny-rdm SshPage 的命令历史/快速命令/连接信息/字体缩放四项，全部落地（明细见
`PROGRESS-A-SSH.zh-CN.md`）：

| 项 | 对标 | 落地证据 | 验证 |
| --- | --- | --- | --- |
| 命令历史（弹窗历史区/↑↓ 浏览/一键重发） | SshPage 命令历史 | `frontend/src/lib/commandHistory.ts`（`COMMAND_HISTORY_LIMIT=100` 环形，commandHistory.ts:5/12/26/42 `pushCommandHistory`/`browseCommandHistory`/`sanitizeCommandHistory`）+ App.vue 接线（`ssh/exec` 提交即入历史、重发走既有服务端单引号转义路径）；疑似凭据/超长（>200）/多行命令不落 localStorage | vitest 4 用例（环形去重上限/↑↓ 浏览与草稿恢复/持久化过滤/读回消毒） |
| 快速命令栏（CRUD/发送语义） | tiny-rdm QuickCommand | `frontend/src/lib/quickCommands.ts`（`QUICK_COMMANDS_LIMIT=20`，quickCommands.ts:10/23/42/60）+ App.vue:2430 `sendQuickCommand`（PTY 键盘写入原文，保留交互 shell 状态；无 shell 拼接面；zmodem/执行中禁发） | vitest 3 用例（读回规范化/upsert 三语义/按 id 删除） |
| 连接信息面板（含 authMethod 契约增量） | 连接信息只读摘要 | App.vue `connection-info-popover` + `backend/src/model.rs:46` `AuthenticationMethod::method_name()` + `backend/src/ssh.rs:599/609/1236/1249`（`ssh/sessions/list` 行新增只读 `authMethod`，无凭据泄漏）+ `frontend/src/lib/connectionInfo.ts:17` `formatAuthMethodLabel` | cargo 2 用例（五方法名往返 + 无泄漏断言，model.rs:495/ssh.rs:3155）+ vitest 2 用例；smoke_batch3 `sessions/list` 用例扩展 authMethod 断言 |
| 终端字体缩放（绝对字号钳制重构） | 工作包 A 第 3 条细化（0.8x–2.0x → 绝对字号） | `frontend/src/lib/terminalZoom.ts:6` `clampFontSize`（[8,32]）+ App.vue A+/A− 快捷按钮、Ctrl/⌘ 滚轮、Ctrl/⌘+0 复位宿主基准、localStorage 持久化 + 500ms 防抖 toast | vitest clamp 用例；mock 走查截图 docs/screenshots-a-ssh/06 |
