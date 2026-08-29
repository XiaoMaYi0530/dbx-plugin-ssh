# S-B 路交付报告（ssh-sftp 前端增强）

日期：2026-08-28。工作目录：`~/btroot/dbx-plugins/ssh-sftp`，仅触碰 `frontend/src/**` 与本文件（S-B 所有权范围）。未执行任何 git commit/push；未新增 npm 依赖；未改后端。

## 1. 基线状态

在未提交 batch3 改动之上（`frontend/src/{App.vue,lib/i18n.ts,lib/workbench.spec.ts,mockDbxHost.ts,style.css}`、`components/TerminalSearchPanel.vue` 及 batch3 已抽出的 lib 模块）：

| 检查 | 基线结果 |
| --- | --- |
| `pnpm install --frozen-lockfile` | 通过 |
| `pnpm typecheck`（vue-tsc） | 通过（exit 0） |
| `pnpm test`（vitest） | 2 文件 21 用例全绿 |
| `pnpm build`（自包含 ui/index.html） | 通过（仅 chunk>500kB 容量告警，非错误） |

逐项核对 batch3 前端未提交改动（终端搜索面板、危险粘贴确认、字体缩放、SFTP 搜索/过滤/多选、路径历史、新建文件、属性弹窗、复制粘贴等）：均为**完成态，无半成品**，本轮在其上继续而非返工。

## 2. 本轮完成项（三项，对标 tiny-rdm ssh 前端模块）

### 2.1 OSC 633 命令标记解析（对标 `modules/ssh/osc633-parser.js`）

- **新文件** `frontend/src/lib/terminalCommandMarkers.ts`：tiny-rdm 解析器的 TS 移植（纯函数 `parseOsc633StreamChunk`/`getOsc633ParserState` + 有状态封装 `Osc633CommandParser` + `formatCommandDuration`）。远端 shell 装有 VS Code 风格 shell integration 时，从终端字节流解析 `\u001b]633;…` 帧（A/B/C/E/D/P；BEL 与 ST 双终止符、跨 chunk carry、ST 半帧分割）；无标记流原样透传，零后端依赖。
- **接入** `App.vue`：`writeTerminalOutput` 喂解析器 → 工具栏级 `commandMarker` 状态（命令执行中/最近一次退出码/时长/cwd）。终端左下角新增 `wb-*` 状态条 `terminal-command-marker`（仅检测到 633 帧后出现；执行中 spinner+命令、结束后 `exit N · 时长`，非零红色；右侧附 cwd）。显示策略：`D` 帧结果保留显示直到新 `E` 帧开始（`A` 帧的 `lastExitCode=null` 重置不冲掉上次结果）。
- **目录跟随兜底**：`followDirectory` 开启且后端报告 `directoryTrackingSupported=false`（无 OSC 7）时，`P;Cwd=` 更新驱动 `loadDirectory`（纯前端读操作，无后端改动）。
- **会话生命周期**：`openSession`/`closeSession` 重置解析器与标记条。
- **测试证据**：`workbench.spec.ts` 新增 describe「OSC 633 command markers (ported from tiny-rdm)」8 用例（跨 chunk Cwd、未知 payload 保留可见、双终止符、A/E/C/D 全周期、ST 半帧、普通 ANSI 不进 carry、无 D 时 A 关闭命令、时长格式化）。
- **mock 验证**：`mockDbxHost.ts` 欢迎输出注入一段完整 633 周期；Playwright 截图确认标记条渲染（`Shell Integration active`/结果态）、命令回显正常、布局无破坏。

### 2.2 会话状态展示（对标 `modules/ssh/session-status.js`，扩展"重连中"）

- **新文件** `frontend/src/lib/sessionStatus.ts`：移植 `normalizeSshSessionStatus`（closed/timeout/pending/dialing/… → 五类规范态）与 `isUsableSshSession`；新增 `describeWorkbenchSessionStatus(state, { reattaching })`，把工作台四态 + attachSession 退避重试中的 `reconnectPending` 映射为 连接中/已连接/重连中/已断开/错误 五种用户可见状态。tiny-rdm 的 `findPreferredSshSession`（多会话择优）无对应场景——插件为单 workbench 单连接，未移植（见 §5）。
- **接入** `App.vue`：`reconnectPending` ref 在 attach 重试调度时置位、成功/放弃/断开事件时复位；computed `sessionStatus` 驱动两处 UI——工具栏 identity 区 `session-pill`（色点+文案：绿=已连接/黄=连接中/橙=重连中/红=错误/灰=已断开）与终端覆盖层文案（重连中显示 `reattachingTerminal` 详情，而非笼统"连接中"）。
- **测试证据**：describe「session status semantics (ported from tiny-rdm)」3 用例（断连/错误类规范化、usable 判定、reconnecting 相位与异常字符串鲁棒性）。
- **七语**：`sessionStatus.connecting/connected/reconnecting/disconnected/error` 全 7 语言块补齐。

### 2.3 命令弹窗输出净化（对标 `modules/ssh/terminal-output.js` 可移植部分）

- **新文件** `frontend/src/lib/terminalOutputText.ts`：移植通用部分 `stripTerminalControlSequences`（OSC+CSI）、`stripCommandEcho`/`stripHiddenCommandEchoes`（回显行移除+未命中排队）、新增 `sanitizeCommandOutput`（控制序列剥离+尾部空白/NUL 收尾）。tiny-rdm 的 `.mcp_ctl_shell_*` hook 特判**未移植**——那是 tiny-rdm 自有 hook 注入的回显清理，本插件从不注入该类命令（见 §5）。
- **接入** `App.vue`：运行命令弹窗 `commandResult.output` 渲染前经 `commandOutputText` computed 净化——`ssh/exec` 输出常带 ANSI 颜色码，改前在 `<pre>` 里显示为乱码转义符。`mockDbxHost.ts` 的 exec 输出已加 ANSI 码供视觉复核。
- **测试证据**：describe「terminal output text sanitization (ported from tiny-rdm)」3 用例（标题 OSC/CSI 剥离、回显行移除与 remainingCommands 排队、exec 输出纯文本化）。

## 3. 硬性规范自查

- **i18n 七语**：新增 `terminalCommand.{running,finished,hint}` 与 `sessionStatus.{connecting,connected,reconnecting,disconnected,error}` 共 8 key × 7 语言块（en/es/it/ja/pt-BR/zh-CN/zh-TW），全部以 `terminalZoom` 组之后 Edit 局部插入（未重写 i18n.ts）；`workbench.spec.ts` 的 `requiredKeys` 七语循环已覆盖全部新 key，**35 用例全绿即七语完备的自动化证据**。
- **样式**：`session-pill`/`terminal-command-marker` 均为手写 CSS，沿用 `--border`/`--background`/`--primary`/`color-mix` 变量与 999px 圆角胶囊风格，对齐既有 `read-only-badge`/`zmodem-status`；无 UI 框架、无新依赖。
- **错误处理**：本轮三特性均为展示层纯前端逻辑，无新错误路径；既有 `showError(cause, "terminal"|"sftp")` 未被绕过。
- **fileTransfer 兜底**：未触及 fileTransfer 相关路径。
- **浏览器验证**：Playwright 打开 `mock.html` 截图确认 session pill、命令标记条、OSC 633 回显渲染与整体布局（截图证据在会话记录中）。后端二次微调（D/A 帧显示策略）后 Playwright 后端持续超时（连重试 3 次不可恢复），该细节由单测+三件套覆盖，建议主会话 mock 页终验。

## 4. 最终验证（收尾）

```
pnpm typecheck   通过
pnpm test        2 文件 35 用例全绿（基线 21 → 35）
pnpm build       通过，产出自包含 ui/index.html
```

复核（同日二次运行，S-B 收尾波）：`pnpm install --frozen-lockfile` / typecheck / test（35/35）/ build 再次全绿；七语 key 逐块核对（en/es/it/ja/pt-BR/zh-CN/zh-TW 七组 `terminalCommand`+`sessionStatus` 均在，spec `requiredKeys` 循环覆盖）；浏览器复核：vite dev 打开 `mock.html` 截图确认 session-pill「Connected」与终端左下角「Shell Integration active /home/demo」标记条渲染正常、SFTP 面板无破坏（mock 页需 vite dev server，静态 http 服务器会因裸模块说明符白屏——fixture 属性，非缺陷）。

## 5. 遗留与不做

| 项 | 状态/原因 |
| --- | --- |
| `findPreferredSshSession`（多会话择优） | 未移植：插件单 workbench 单连接，无会话列表语义；若宿主未来提供多会话 API 再评估 |
| `terminal-output.js` 的 mcp_ctl hook 特判 | 不适用：本插件无该 hook 注入路径 |
| batch-send / useBatchSend | 按任务指示不做（架构级阻塞：需宿主多会话语义） |
| 命令标记条"运行中"时长实时跳动 | 未做：需 UI tick 定时器，收益低；完成态时长已显示 |
| metrics-i18n.js / quick-sudo.js / profile-display.js 对标 | quick-sudo 插件已有等价实现（exec 编排+设置面板）；metrics 前端分区属 S-D 范围；profile-display 属宿主连接管理域 |

## 6. 交接（给 S-A / 主会话）

- **无需后端改动**：本轮三项全部纯前端（消费既有终端字节流与既有事件）。OSC 633 解析完全旁路，`directoryTrackingSupported=false` 时的 633 Cwd 兜底只调既有 `sftp/list`。
- **可选后端增强（非阻塞，登记备查）**：若希望目录跟随在"远端装有 shell integration 但后端 OSC 7 注入失败"场景更实时，后端可在 `ssh/terminal/directoryTracking` 响应中区分"注入失败/不支持 shell"两态（当前统一为 supported=false，前端已有 633 兜底，不影响功能）。
- **后端如注入 OSC 633**：sidecar 未来若主动安装 shell integration 脚本（对标 tiny-rdm 的 prompt 注入），前端解析器零改动即可点亮命令时长/退出码条；当前仅在远端已装 VS Code shell integration 时生效。
- **宿主验证建议**：`mock.html` 打开后即见 session pill 与（注入的）OSC 633 标记条；真实连接需远端 `~/.bashrc` 载入 shell integration 脚本才能看到标记条。
- **冲突提示**：本轮对 `App.vue` 的改动集中在 script 头部 imports/refs/computed、`writeTerminalOutput`、`openSession`/`attachSession`/`closeSession`、模板 identity 区与 terminal-pane 底部；i18n 插入点在各语言块 `terminalZoom` 之后——与其他路锚点（newFolder/metricsRefreshHint）不相交。
