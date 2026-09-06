# SSH/SFTP 插件前端 UI 体验扫描报告（UI_SCAN_FINDINGS）

> 扫描角色：场景驱动的 UI 体验扫描 agent（只读扫描 + 报告，不实施修复）。
> 扫描对象：`ssh/frontend`（mock.html 可视化夹具 + App.vue/组件源码交叉核对）。本报告只记录发现与建议，不含代码修改。

## 一、扫描环境

| 项 | 值 |
| --- | --- |
| 轮次 | 第 1 轮基线走查（2026-09-06，工作树含 9/5–9/6 的 DirTree/SideNavPanel/TerminalSearchPanel/TextPreview 单测新增改动） |
| Dev server | 新起 `vite --port 5291 --strictPort`（http://localhost:5291/mock.html），扫描结束已 kill，未跑 build |
| 自动化 | playwright-core + 系统 Chrome（channel:"chrome"），独立实例装于 `/tmp/uiscan-ssh`（未进项目依赖），7 个走查脚本按场景驱动真实点击/输入 |
| 视口矩阵 | 1280×900（默认）、720×900（窄）、1440×900（宽） |
| URL 参数矩阵 | 以 mockDbxHost.ts 实际实现为准：默认（dark+只读）/ `?theme=light` / `?rw=1`（可写）/ `?err=disconnect`（4s 后断开）/ `?err=authfail`（认证失败注入）。任务假设的 `?ro=1/?err=1/?noconn=1/?glue=1` 均不存在 |
| 用户旅程 | ① 打开→自动连接→SFTP 浏览→树展开→进目录→文本预览→Esc 关闭 ② 只读 vs rw 写操作 ③ 终端 Ctrl+F 搜索/右键菜单 ④ 命令对话框（历史上下键/运行/取消）⑤ 快速命令增删发 ⑥ 批量发送 ⑦ 指标浮层/连接信息/延迟测量 ⑧ 断开→重连横幅→自动重连→恢复提示 ⑨ 认证失败 ⑩ 多选批量条（Meta/Shift）⑪ 行内重命名/chmod 入口/删除确认/属性 ⑫ 编辑预览（dirty 徽标/保存/放弃确认）⑬ 列选择 popover/传输面板/路径历史 ⑭ 三视口溢出检查 ⑮ light 主题渲染与对比度 |
| 证据 | 截图 30+ 张（走查核对后已全部删除，未入库） |

## 二、发现清单

统计：**P0 × 0，P1 × 2，P2 × 6**。主流程（浏览/预览/编辑/搜索/传输/重连恢复）整体健康，无阻断使用级问题；两条 P1 分别是连接失败路径的错误呈现黑洞与弹层焦点管理缺失。

> 修复跟进（2026-09-06）：P1-1、P1-2 及顺手项 P2-3 已修复并复验通过，详见各条目"修复状态"标注与 `PROGRESS-P-SSH.zh-CN.md` 同日章节。第 2 轮清理（同日）：其余全部 P2（P2-1/P2-2/P2-4/P2-5/P2-6）已修复并浏览器复验通过，详见各条目"修复状态（第 2 轮）"标注与 `PROGRESS-P-SSH.zh-CN.md` 同日第 2 轮章节。

### P1（明显可感知的体验债 / 功能性缺陷）

**P1-1 连接快速失败（认证错误等）陷入无限重试，错误信息永不呈现，用户无出口**
- 位置：`App.vue` `openSession()`（src/App.vue 1381–1449，重试计数在 1388 行入口归零）
- 复现：打开 `http://localhost:5291/mock.html?err=authfail` → 等待。8s/15s/30s 三个观察点均停留在 "Connecting SSH..."（pill=Connecting），无 overlay 按钮；工具栏 Reconnect 按钮在 connecting 态也是 disabled。全程零错误文案。
- 期望 vs 实际：期望认证失败等永久性错误在有限重试后进入 error 态，展示 friendly 错误（connectError.* 已有映射）+ Reconnect 按钮；实际 `openRetryAttempt` 每次重入 `openSession` 都被归零，`OPEN_RETRY_MAX=3` 约束失效，唯一退出条件只剩单次尝试耗时 ≥8s——mock/真实 sidecar 的认证拒绝都是秒级快速失败，于是无限重试。
- 影响面：凭据配置错误（密码被拒、密钥被拒）的用户 100% 命中：工作台永久转圈、无提示、无可点按钮，只能关 tab。且重开即重演。
- 建议：① 重试计数改为跨调用保留（归零移出函数入口，由非重试入口重置）；② 非 inactive 错误累计到上限即进入 error 态并展示 `terminalErrorFriendly`；③ authfail 类永久错误可跳过重试立即失败。**建议真机优先复核**（mock 注入与真实 sidecar 返回节奏存在差异，但错误串按真实风格模拟，风险高）。
- **修复状态（2026-09-06，已修复 + 浏览器复验通过）**：重试计数只在非重试入口归零（`openSession` 增加 `isRetry` 参数，重试定时器重入保留计数，OPEN_RETRY_MAX 重新生效）；重试决策抽为纯函数 `src/lib/connectRetry.ts` `decideConnectRetry()`（backoff 2s·N、8s 快失败窗口、inactive 非 boot 即败、auth/hostKey 判定为永久错误跳过重试直接进 error 态），配套 `connectRetry.spec.ts` 7 用例。浏览器复验（playwright-core + Chrome，`?err=authfail`）：t+0.5s/8s/15s 三个观察点均稳定呈现 friendly 文案 "Authentication failed. Check the username, password or private key…" + Reconnect 出口按钮，无转圈；点击 Reconnect 有响应。真机 sidecar 认证失败节奏复核仍建议在后续真机轮补一次。

**P1-2 弹层打开后焦点不进入弹层：命令/批量对话框、删除确认等全部中招**
- 位置：命令对话框、批量发送、删除确认（destructive-modal）等全部 `.modal-backdrop` 弹层；模板中的 `autofocus` 属性为原生属性，Vue 动态插入 DOM 时不生效
- 复现 A：点击工具栏"Run command"打开命令对话框 → `document.activeElement` 仍是背景的触发按钮（实测 inModal:false，`autofocus` 元素存在但未聚焦）。批量发送弹窗同样。
- 复现 B：右键文件 → Delete 打开删除确认弹窗 → `activeElement` 为 BODY（比 A 更差：Tab 需从文档头重走，Enter/Space 可能误触发背景元素）。
- 期望 vs 实际：期望打开时焦点进入弹层首个交互控件、Tab 在弹层内循环、关闭后归还触发按钮；实际三者皆无。mkdir/newFile/chmod/settings 等写了 `autofocus` 的对话框为同一模式（只读连接下按钮禁用未能逐一实测，源码同构）。
- 影响面：键盘用户与读屏用户打开任何弹层后都要重新定位焦点；焦点滞留背景层还有误触发风险。对标 kafka 插件已收口的 P1-2/P1-3（Esc + 焦点陷阱 + 归还），ssh 插件 Esc 关闭链已完备（onDocumentKeydown 分层退出，走查全部通过），唯焦点管理缺失。
- 建议：弹层 open 时 focus 容器或首个控件 + 简易 focus trap；close 时归还触发按钮。可复用 kafka 的 `decideModalKeydown` 收口经验（shared/frontend 无现成实现，需各插件内落地）。
- **修复状态（2026-09-06，已修复 + 浏览器复验通过）**：ssh 插件内落地 `src/lib/modalFocus.ts`（`focusableElements` / `nextFocusIndex` / `decideModalKeydown` / `pickModalFocusTarget` 纯函数 + `modalFocus.spec.ts` 9 用例，模板保留的 autofocus 属性改作聚焦定位提示）。App.vue 增加弹层开状态计数 watch：打开时 nextTick 聚焦首控件（autofocus 标记优先）、关闭时焦点归还触发元素（触发元素栈与嵌套深度同步 push/pop，逐层弹出跳过已随右键菜单卸载的瞬态控件——`focusin` 跟踪"弹层/右键菜单外最近稳定焦点"作回退目标）；`onDocumentKeydown` 增加 Tab 分支实现焦点陷阱（无弹层不拦截，终端 Tab 穿透不受影响）；Esc 关闭链原样保留。浏览器复验（`?rw=1`）：命令对话框、批量发送、删除确认三类弹层均通过"打开后焦点进入首控件（命令/批量聚焦 input[autofocus]，删除确认聚焦 header 关闭钮、不再落 BODY）/连按 6 次 Tab + Shift+Tab 均不出弹层/Esc 关闭后焦点归还触发按钮"共 16 项检查全绿。

### P2（打磨项 / 夹具缺口）

**P2-1 夹具：`?err=disconnect` 在默认启动路径下完全失效**
- 位置：`mockDbxHost.ts`（disconnect 定时器仅注册在 `ssh/session/open`）
- 复现：直接打开 `mock.html?err=disconnect` 等待任意时长 → 不断开。原因：启动走 reattach（`ssh/sessions/list` 返回 live session → attach），open 不被调用、定时器不注册。必须手动点一次工具栏 Reconnect（触发 open）后 4s 断开才会发生。
- 建议：夹具把断开定时器同时挂到 attach 完成后，或加 `?fresh=1` 之类参数强制走 open 路径；否则该参数注释宣称的"重连横幅/倒计时/立即重连/恢复提示全流程验证"默认不可达。
- 附：手动触发后全流程实际验证通过——断开 → 横幅 "Connection lost, reconnecting automatically / Reconnect now" → 自动重连 → notice "Reconnected, current directory /home/demo"。mock 下重连瞬时完成，横幅一闪而过属夹具特性而非产品问题。
- **修复状态（2026-09-06 第 2 轮，已修复 + 浏览器复验通过）**：断开注入抽为 `scheduleDisconnect()`，open 与 attach 两条启动路径完成会话后都调用（全局单发不复发）。直接打开 `mock.html?err=disconnect` 无需手动 Reconnect，约 4s 后横幅自动出现（"Connection lost, reconnecting automatically / Reconnect now"），自动重连后 pill 恢复 Connected。配套新增 `mockDbxHost.spec.ts`（happy-dom + fake timers）：attach 路径 4s 断开单发、重连（再 attach/open）不复发、open 路径保持可用共 3 用例。

**P2-2 夹具：默认（reattach）首屏终端仅一行 prompt，Welcome/OSC 633/command-marker 视觉不可达**
- 复现：默认打开 mock.html → 终端只有 `user@server:~$ `。Welcome 与 shell-integration 周期只在 open 路径 emit；因此 command-marker 条（"Shell integration active"）首屏永远不可见（重连后才出现，走查已在重连后确认其渲染正常）。
- 建议：attach 路径的 mock replay 回放少量历史帧（含 OSC 633 周期），让 command-marker 的视觉验证不依赖手动重连。
- **修复状态（2026-09-06 第 2 轮，已修复 + 浏览器复验通过）**：open 与 attach 共用同一份 `terminalTranscript`（Welcome + OSC 633 周期 + prompt），attach 完成后作为后续帧推送，默认（reattach）首屏不再只有一行 prompt。浏览器复验：首屏终端含 "Welcome to DBX SSH/SFTP visual fixture" 与 OSC 633 命令回显，command-marker 条（"Shell integration active · /home/demo"）首屏即渲染，且首屏终端文本已含 "systemctl status nginx" 等真实词（首屏搜索不再空手而归）。行为由 `mockDbxHost.spec.ts` 用例锁定。

**P2-3 host-key 验证弹窗文案硬编码英文，未走 i18n（七语缺口）**
- 位置：`App.vue` 模板 4768–4776 行："Verify SSH host key" / "Confirm this fingerprint before DBX sends credentials." / "Remember this key" / "Reject" / "Trust and connect" 均无 `t()`；i18n.ts 中也无对应 key
- 说明：夹具未注入 host-key 事件无法浏览器走查，来源为源码审查；与工作区硬性规则"七语文案"冲突。
- 建议：补 `hostKeyDialog.*` 七语键并接入 `t()`。
- **修复状态（2026-09-06，已修复 + 复验通过）**：i18n.ts 七语块各补 `hostKeyDialog` 8 键（title/desc/server/keyType/fingerprint/remember/reject/trust），App.vue host-key 弹窗模板全部改走 `t()`。复验：workbench 七语键集合一致性单测（`workbench.spec.ts`）通过；浏览器内动态 import i18n 模块逐键解析，7 locale × 8 键全部命中真实翻译（en "Verify SSH host key" / zh-CN "验证 SSH 主机密钥" 等），无键名回退。

**P2-4 light 主题工具栏染色使 muted 文字对比度降到 AA 边缘（≈4.2:1）**
- 复现：`?theme=light` 下采样：identity 文字 5.38:1 ✓、文件页脚 muted 4.74:1 ✓、会话状态 pill 4.22:1 ✗（rgb(115,115,115) 叠在连接色 10% 蓝染工具栏上；dark 下同位 5.65:1 无虞）。
- 建议：对标 kafka P2-11 的收口方式，light 下染色 alpha 10%→5% 左右，或 pill 文字用 foreground 色。视觉整体无拼色/泛色问题（蓝染观感中性，远轻于 kafka 当时的红染）。
- **修复状态（2026-09-06 第 2 轮，已修复 + 浏览器复验通过）**：染色收口为 `src/lib/toolbarTint.ts` 纯函数 `toolbarTintStyle(color, colorScheme)`（App.vue 原 `colorWithAlpha` 内联逻辑迁入），dark 保持 10%/18% 惯例，light 压到 4%/8%（内描边同步减半）。配套 `toolbarTint.spec.ts` 6 用例，含 WCAG 对比度回归口径：muted-foreground 叠染色工具栏 light/dark 均 ≥4.5:1。浏览器实测（`?theme=light`，真实渲染叠底）：toolbar tint alpha=0.04，会话 pill 对比度 **4.53:1**（修复前 ≈4.22:1）；dark 主题 alpha=0.1 未变。

**P2-5 mockDbxHost.ts 注释过时：`?mock=1` 参数不存在**
- 位置：src/mockDbxHost.ts 66 行注释"（?mock=1 走通侧栏树/新建/压缩）"——代码并未读取 mock 参数，fixture 树无条件生效。文档噪音，建议删除参数字样。
- **修复状态（2026-09-06 第 2 轮，已修复）**：注释改为"（无条件生效，无开关参数）"，消除不存在的 `?mock=1` 字样。

**P2-6 dev 环境首载控制台一条 404 资源噪音**
- 复现：mock.html 首载 console 出现一条 "Failed to load resource: 404"（三个脚本各复现一次；response 监听未能定位到具体 URL，推断为 favicon 类请求，vite dev 特有）。低危噪音，对标 kafka P2-14 的收口（mock.html 内联 data-icon）可顺带处理。
- **修复状态（2026-09-06 第 2 轮，已修复 + 浏览器复验通过）**：mock.html `<head>` 补 `<link rel="icon" href="data:," />` 占位声明。浏览器复验：mock.html 首载 0 条 4xx 资源请求、0 条 console error。

### 走查中确认为"设计而非缺陷"的易误解点（不列为发现）

| 现象 | 结论 |
| --- | --- |
| 只读连接下终端右键（无修饰键）不弹菜单而是直接粘贴 | 选中复制模式（默认开）的有意设计；Shift+右键出完整菜单，已验证菜单 7 项齐全、zmodem 上传在只读下正确禁用 |
| 首次打开工作台 SFTP 面板默认收起 | 持久化全局偏好（默认终端优先），工具栏开关可展开；"Open SFTP panel by default" 列选择里有持久化开关 |
| 目录树 Root 初始即展开，点击 caret 是收起 | 懒加载树正常；行单击 = 面板进入目录（实测 Root 行单击后路径栏变 `/`） |
| 终端搜索 "nginx" 报 No matches | 默认 reattach 首屏终端确实没有该词（见 P2-2）；搜真实存在词 "server" 得 1/1 且高亮 decoration 正常 |
| Ctrl+click 多选在 macOS 无效 | macOS 浏览器将 Ctrl+click 转为右键菜单（产品同时支持 ⌘/Ctrl）；Meta+click 3 连选批量条 "3 selected" 正常，Shift 范围选正常 |
| 带未保存修改 Esc 关闭预览"直接关了" | 实为原生 confirm 被自动化接受；手动验证 confirm 拦截链完整（dismiss 保持打开、accept 关闭），文案 "You have unsaved changes. Discard them and continue?" |
| 延迟测量点击后 1 秒仍 "Measuring…" | mock `ssh/exec` 固定 1.5s 延迟；2.6s 后显示 "1.5 s"，功能正常 |

## 三、旅程逐一走查矩阵

| 旅程 | 默认 dark 1280 | light | rw=1 | err=disconnect | err=authfail | 720px | 1440px | 备注 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 连接/会话 pill | ✓ | ✓ | ✓ | ✓(横幅+恢复提示) | ✗ P1-1 | ✓ | ✓ | 状态文案齐全 |
| SFTP 浏览/树/路径历史 | ✓ | ✓ | ✓ | ✓ | — | ✓ | ✓ | 树懒加载、行点击导航、is-current 高亮正常 |
| 文本预览/编辑保存 | ✓(只读无编辑钮) | ✓(无拼色) | ✓(dirty/保存/放弃确认) | — | — | ✓(弹窗 fit) | ✓ | Esc 关闭链完整 |
| 终端搜索 Ctrl+F | ✓(1/1+高亮) | — | — | ✓(重连后 1/4) | — | — | — | Enter/Shift+Enter/Esc、Aa/.* /|w| 开关、No matches 红字 |
| 终端右键菜单 | ✓(Shift+右键) | — | — | — | — | — | — | 只读下 zmodem 正确禁用 |
| 命令对话框 | ✓(运行/历史↑↓/ANSI 清洗) | — | — | — | — | ✓(fit) | ✓ | 焦点不进入 P1-2 |
| 批量发送 | ✓(Sent to 1 session) | — | — | — | — | — | — | 目标徽标（当前/只读）齐全；焦点 P1-2 |
| 快速命令 popover | ✓(增/发/删/空态) | — | — | — | — | — | — | 20 条上限文案、外点关闭正常 |
| 指标浮层 | ✓(磁盘 87% warn 红) | ✓ | — | — | — | — | — | 5s 轮询、关闭正常 |
| 连接信息/延迟测量 | ✓(1.5 s) | — | — | — | — | — | — | 只读徽标、字段齐全 |
| 文件操作（重命名/删除/过滤/多选） | — | — | ✓ | — | — | — | — | 行内重命名自动聚焦 ✓；删除确认焦点 BODY（P1-2） |
| 传输面板/下载 | — | — | ✓(下载链路) | — | — | — | — | 进度卡/速度/取消入口齐 |
| 断开→重连 | — | — | — | ✓(需手动触发 open，见 P2-1) | — | — | — | 横幅文案/立即重连/恢复 notice 全通过 |
| 视口溢出 | — | — | — | — | — | ✓ 无溢出 | ✓ 无溢出 | 720 上下堆叠布局正常、工具栏无截断 |

## 四、验证与遗留

- 走查旅程 15 组 × 参数/视口矩阵，发现条数：P0=0、P1=2、P2=6。
- 遗留未验证：① P1-1 需真机复核真实 sidecar 认证失败的重试节奏（决定其最终定级是否上探 P0）；② host-key/agent 审批/paste 确认等安全弹窗的浏览器走查（夹具未推对应事件，P2-3 为源码审查结论）；③ zmodem 上传全流程、拖拽上传（headless 无原生拖放）、sudo 分支（mock 的 sudo 路径有固定输出但工作台 sudo 模式开关在只读下禁用，未深入）。
- 走查用截图已全部删除，未入工作区；`/tmp/uiscan-ssh` 下的脚本为扫描工具产物，不入库。

## 五、第 3 轮（专家视角深度测试，2026-09-06）

> 视角切换：以重度 SSH/SFTP 运维用户 + 测试专家的苛刻眼光做压力/健壮性/键盘流/i18n 深测；**不重复第 1、2 轮已收口项**（P1-1/P1-2、P2-1~P2-6 全部绕开）。全程 mock 夹具 + 运行时 invoke/binary 事件劫持注入，只记录不改代码。

### 5.1 扫描环境

| 项 | 值 |
| --- | --- |
| 轮次 | 第 3 轮专家视角深度测试（2026-09-06，前 2 轮全部 P0/P1/P2 已修复收口后的回归基线） |
| Dev server | 新起 `pnpm vite --port 5291 --strictPort`（http://localhost:5291/mock.html），扫描结束已 kill |
| 自动化 | playwright-core + 系统 Chrome（channel:"chrome"，headless），独立实例 `/tmp/uiscan-ssh-r3`（不入项目依赖）；8 个场景脚本（perf/perf2/functional/functional2/robust/keyboard/i18n + 定性探针） |
| 注入手段 | addInitScript 劫持 `window.dbxPlugin`：invoke 补丁链（错误/慢响应/坏格式/大数据注入）、binary 监听捕获（终端洪帧注入，9 字节帧头 + 序号）、事件记录（inputAck/progress）、调用台账（逐 method 计数与参数） |
| URL 参数 | `?rw=1`（写操作流）、`?err=disconnect`（断开恢复）；mockDbxHost.ts 实测为准 |
| 视口 | 1440×900 为主 |

### 5.2 新发现清单

统计：**P0 × 0，P1 × 3，P2 × 7**。三条 P1 全部是"键盘/时序/竞态"类深水区问题，常规走查不可达，均以调用台账或 DOM 变异时间线实锤。

#### P1（功能性缺陷）

**R3-P1-1 键盘 Enter 提交 mkdir/newFile 成功后，弹层被"幽灵点击"立即重开（焦点归还 + Enter 激活竞态）**
- 位置：`App.vue` `modalOpenCount` watch 的焦点归还逻辑（src/App.vue 3758–3783）× mkdir/newFile 对话框（4486–4492、newFile 同构）
- 复现：`?rw=1` → 工具栏 New folder → 输入名 → 按 Enter。MutationObserver + capture 级 click 记录时间线：t+237ms 弹层关闭（目录已建、列表已刷新）→ **t+239ms "New folder" 工具栏按钮收到一次无 mousedown 的 click** → t+240ms 弹层重开（草稿被重置为空）。New file 对话框同样复现（sftp/touch 1 次、弹层重开）。
- 影响：纯键盘用户视角 = "按 Enter 后弹窗闪一下又回来"，无法确认成功；最自然的反应是再按一次 Enter → 第二次 createDirectory 撞已存在名 → 错误横幅 "sftp: cannot write /home/demo/ghost-dir" 且弹窗滞留（实测 2 次 invoke）。鼠标用户不受影响（Confirm 按钮路径实测正常关闭）。命令对话框不受影响（运行后不关闭）。
- 根因：弹层关闭时焦点归还到工具栏触发按钮（P1-2 修复引入的归还链），归还发生在同一 Enter 按键序列内，浏览器在刚获得焦点的按钮上产生激活 click。
- 建议：焦点归还推迟一帧（rAF/setTimeout 0）；或归还前给触发按钮挂一次性 capture click 抑制；或归还焦点到弹层容器等不可激活元素。真机建议补验（浏览器内核相关行为）。

> **【R4 修复标注 2026-09-06】已修复**。方式：新增 `src/lib/ghostClickGuard.ts`（纯函数守卫，7 条单测）——焦点归还前 `arm()` 开 400ms 抑制窗，窗内"无 mousedown 前驱"的合成 click 被 document 捕获级监听器 `preventDefault + stopPropagation`（真实鼠标点击因 mousedown 宽限窗而放行）。接线：`App.vue` modalOpenCount watch 归还焦点前 arm + onMounted/onBeforeUnmount 挂卸 capture 监听。R4b 复验：mkdir 键盘 Enter 提交后 backdrop=0、`sftp/createDirectory` 仅 1 次，关闭态后续 click 不再重开弹层。PASS。

**R3-P1-2 Esc 取消行内重命名，实际仍以草稿名提交了重命名**
- 位置：`App.vue` 行内重命名输入框 `@blur="commitRename(entry)"`（src/App.vue 4379–4389）
- 复现：`?rw=1` → server.log 右键 → Rename → 输入 "evil.log" → 按 Escape。invoke 台账记录到 `sftp/rename /home/demo/server.log → /home/demo/evil.log`，刷新后列表确认改名生效。
- 根因：Escape 只清 `renamingPath` 触发输入框卸载，Chromium 对"被移除的聚焦元素"派发 blur → `commitRename` 以当前草稿名提交。
- 影响：**数据变更操作逃逸了取消语义**——用户明确按 Esc 放弃，文件却被改名；配合 R3-P2-1 的双发问题，重命名路径整体不可信。
- 建议：commitRename 入口校验"renamingPath 仍指向本行且非取消中"；Escape 先置 canceling 标志再卸载；或 blur 提交仅在焦点移入同列表其他可交互元素时生效。

> **【R4 修复标注 2026-09-06】已修复**。方式：新增 `src/lib/sftpRename.ts` 的 `shouldCommitRename` 守卫（6 条单测）——`commitRename` 入口校验"editingPath 仍指向本行且非提交中"；Esc 处理器先置 `renamingPath = ''` 再卸载输入框，卸载引发的幽灵 blur 到达时 editingPath 已不指向本行而被短路。R4b 复验：Esc 取消重命名 `sftp/rename`/`sudo/rename` 台账 **0 次**、源文件原样、编辑态收敛。PASS。

**R3-P1-3 目录列表加载竞态：慢响应晚到覆盖后导航结果，无请求序号/取消机制**
- 位置：`App.vue` `loadDirectory()`（src/App.vue 1817–1841）
- 复现：`?rw=1` → 劫持 sftp/list 使 `/etc` 延迟 3s 返回、`/var/log` 立即返回 → 双击 etc 行（触发加载 /etc）→ 150ms 内在路径栏输入 /var/log 回车 → 500ms 时路径栏与列表均为 /var/log → 3.5s 后观察：**路径栏、列表、页脚全部回跳 /etc**（实测 path=/etc、rows=[hosts]、footer 含 /etc）。
- 影响：慢链路（跨公网/高峰 SFTP）下"点了 A 又改去 B，视图却跳回 A"是高频真实场景；路径历史也被污染记录。`loadingFiles` 只挡刷新按钮不挡路径栏/树/行双击导航。
- 建议：loadDirectory 引入单调请求序号，仅最新请求允许写 `entries/currentPath/历史`；或对 in-flight 导航做 AbortSignal。

> **【R4 修复标注 2026-09-06】已修复**。方式：新增 `src/lib/requestEpoch.ts`（4 条单测）；`loadDirectory()` 发起前取单调序号，响应落地、catch、finally 三处均以 `isCurrent(epochId)` 校验，过期响应整体丢弃（列表/路径/选中/历史/loading 收尾只允许最新请求写入）。R4b 复验：/etc 延迟 3s + 改道 /var/log，3.5s 后 footer=/var/log、/etc 的 hosts 行未出现、无回跳。PASS。

#### P2（打磨项）

**R3-P2-1 行内重命名 Enter 提交双发 sftp/rename，每次重命名伴随一条假错误横幅**
- 复现：右键 → Rename → 改名 → Enter。invoke 台账记录 **2 条完全相同的请求**；错误横幅 "sftp: no such file: /home/demo/evil.log"（第二次提交时源已不存在）。
- 根因：Enter 提交 → `renamingPath` 清空 → 输入框卸载 → blur 再次 `commitRename`（无 renameSubmitting 入口守卫）。
- 影响：协议噪音 + 用户每次重命名都看到假错误，掩盖真实故障。
- 建议：commitRename 入口检查 `renameSubmitting`；blur 与 Enter 去重（同 R3-P1-2 一并收口）。

> **【R4 修复标注 2026-09-06】已修复**。方式：与 R3-P1-2 同一守卫（`shouldCommitRename`，submitting 分支 + editingPath 短路）——Enter 提交成功清空 `renamingPath` 后输入框卸载的幽灵 blur 被拒。R4b 复验：Enter 重命名 `sftp/rename` 台账恰 1 条、无错误横幅、编辑态关闭、新名落列。PASS。

**R3-P2-2 重命名冲突无前端预检，失败后 UI 不收敛**
- 复现：deploy.sh 重命名为已存在的 docker-compose.yml → 直接下发、报错横幅 "sftp: cannot write …"；rename 输入框滞留打开态、列表不刷新恢复。
- 对比：粘贴（pasteClipboard）对目标存在性有 exists 预检 + 覆盖确认（src/App.vue 2586–2603），重命名路径无对应处理，交互不一致。OpenSSH 语义下 rename 撞名行为依 posix-rename 扩展而异，前端不做预检会把语义选择完全丢给后端。
- 附：mock 夹具的 `sftp/rename` 先 splice 再写、撞名时源节点丢失，属夹具缺陷（真实 SFTP 原子），建议随本项一并修 mock。
- 建议：与粘贴对齐（exists 预检 + 覆盖确认或直接禁止），失败路径关闭行内编辑态并刷新列表。

> **【R4 修复标注 2026-09-06】已修复**。方式：`commitRename` 对齐粘贴语义——先 `sftp/exists` 预检目标，存在时 `window.confirm`（七语新 key `sftpRename.overwriteConfirm`）确认覆盖；失败路径（catch）关闭行内编辑态并刷新列表，不再滞留。mock 夹具 `sftp/rename` 同步收口为原子语义（先摘目标同名节点再摘源落位，撞名不丢源），配 2 条夹具单测（`mockDbxHost.spec.ts`）。R4b 复验：撞名触发 confirm（接受 → 1 次 exists + 1 次 rename、覆盖落位、编辑态收敛；取消 → 0 次 rename、源文件完好）。PASS。
> 新增用户可见文案已补齐七语（en/zh-CN/zh-TW/es/it/ja/pt-BR）。

**R3-P2-3 后端异常响应防御缺失：entries:null 使列表永久卡 Loading，单条畸形行走不进渲染**
- 复现（invoke 注入坏格式）：① `sftp/list` 返回 `{entries:null}` → pageerror "TypeError: entries.value is not iterable"，文件区停留在 "Loading..."（rows=0），再次导航可恢复；② 返回含 null 行与缺 kind 字段的行 → 排序比较器抛 "Cannot read properties of null (reading 'kind')"，整个列表渲染 0 行。
- 影响：App 不崩（工具栏/导航仍可用），但当前视图僵死 + pageerror 上抛。真实 sidecar 契约虽不应返回 null，但网络/版本错配下防御缺失会放大故障面。
- 建议：`entries.value = Array.isArray(result.entries) ? result.entries : []`；渲染前过滤非对象行；畸形行可考虑占位而非整表丢弃。

> **【R4 修复标注 2026-09-06】已修复**。方式：新增 `src/lib/sftpEntries.ts` 的 `sanitizeSftpEntries`（7 条单测）——非数组 → 空数组；null/非对象/无名/无 uri 行丢弃；缺 kind 降级为 file（渲染占位而非整表丢弃/比较器抛错）；非数值 size/modifiedAt 取中性默认。`loadDirectory` 落地前统一过 sanitize。R4b 复验：注入 `{entries:null}` → 空态视图而非永久 Loading、0 pageerror；注入含 null 行 + 缺 kind 行 → 合法行照常渲染、0 pageerror。PASS。

**R3-P2-4 路径栏不支持 `~` 与 `..` 语义**
- 复现（en 界面路径栏输入）：`~` → 归一成 `/~` 下发 → "sftp: no such directory: /~"；`/home/demo/../etc` → 原样下发 `params.path="/home/demo/../etc"`（invoke 台账证实），mock 报错。尾斜杠/无前导斜杠/双斜杠三种归一均正确。
- 影响：`~` 是 SSH 重度用户肌肉记忆；`..` 即便真实服务器可解析，前端 currentPath 显示与路径历史也会保留 `..` 字样，下游 joinRemote/exists 拼接基于未规范路径。
- 建议：路径栏提交前做 `~` → home 展开（已有 sftp/home）与 `..` 段消解（纯字符串或 realpath）。

> **【R4 修复标注 2026-09-06】已修复**。方式：新增 `src/lib/remotePathInput.ts` 的 `resolveRemotePath`（9 条单测）——`~`/`~/x` 在 home 已探测时展开（探测失败保持原样交由后端报错）、`.`/`..` 段消解（根上多余 `..` 收敛为 `/`）、trim/decodeURIComponent/前导与重复/尾斜杠归一；路径栏 Enter 统一走新入口 `submitPathInput()`。R4b 复验：`~` → /home/demo（server.log 在列）；`/home/demo/../etc` → /home/etc；`/home/demo/../demo` → /home/demo。PASS。

**R3-P2-5 纯键盘无法"打开"目录或文件：预览/进入目录对键盘用户不可达**
- 复现：Tab 到目录行（首个 file-row 需 21 次 Tab，无 roving tabindex/方向键导航）→ Enter 仅选中（路径栏仍为 /，实测）；打开目录/文件只能 dblclick；右键菜单可用 ContextMenu/Shift+F10 唤起（位置正常）作为部分缓解，但文本预览本身无键盘入口。
- 建议：文件行 Enter = 打开（对齐 VS Code/主流文件管理器）、提供方向键导航；至少给选中行加"Enter 打开、F2 重命名、Delete 删除"的键盘语义。

> **【R4 修复标注 2026-09-06】已修复（报告"至少"档语义）**。方式：新增 `src/lib/fileRowKeydown.ts` 的 `decideFileRowAction`（4 条单测）——Enter = 打开（目录进入/文件预览，任意模式）、F2 = 重命名、Delete = 删除（仅可写连接），其余按键不拦截；`App.vue` 文件行 `@keydown` 接线 `onFileRowKeydown`。R4b 复验：键盘 Enter 逐级进入 / → /home → /home/demo、F2 唤起行内重命名、Delete 唤起删除确认弹层。PASS。

**R3-P2-6 目录树行键盘不可达 + 展开 caret 无 accessible name**
- 实测：`.sftp-tree-row` 为 div，无 tabindex/role（Tab 不可达，仅 caret 子按钮可达）；6 个 `sftp-tree-caret` 按钮是全部 43 个 button 中仅有的"icon-only 且无 aria-label/title"集合（读屏只报 "button"）。
- 建议：树行加 `role="treeitem"` + tabindex（或 roving tabindex），caret 补 `:aria-label="t('sftpSide.tree') + node.name"` 之类。

> **【R4 修复标注 2026-09-06】已修复**。方式：`DirTree.vue` 树行加 `role="treeitem"` + `aria-expanded` + roving tabindex（当前目录行 0、其余 -1，无当前行时首行兜底 0），Enter 打开 / Space 展开 / ArrowUp·ArrowDown 跨行移焦（DOM 顺序 roving）；caret 补 `:aria-label`（七语新 key `sftpSide.expandNode/collapseNode`，根目录用 `sftpSide.root`）。组件测试 +5 条（`DirTree.spec.ts`，attachTo body + 真实 focus 断言）。R4b 复验：6 行全部 treeitem、存在 tabindex=0 可达行、6 个 caret 0 个无名、ArrowDown 移焦成功、树行 Enter 联动路径栏。PASS。

**R3-P2-7 zh-TW 用词：「批量傳送命令」应为「批次」**
- 实测 zh-TW 界面：工具栏 title "批量傳送命令"、批量对话框整组文案均用「批量」；繁中社区惯例为「批次」（对话框内其余措辞如「目標會話/僅存活」均道地）。仅术语打磨，不影响理解。

> **【R4 修复标注 2026-09-06】已修复**。方式：`i18n.ts` zh-TW 表 batchSend* 全组「批量」→「批次」（batchSendTitle/batchSendHint/batchSendPlaceholder/batchSendSend 等），zh-CN 表保持「批量」不动。R4b 复验（locale 覆盖 zh-TW）：`[title="批次傳送命令"]` 存在、旧措辞节点为 0。PASS。

### 5.3 已复核无问题的维度（测试内容与方式）

| 维度 | 测了什么 / 怎么测 | 结论 |
| --- | --- | --- |
| 大列表渲染性能 | 劫持 sftp/list 注入 600 条目（500 目录 + 100 文件）到 /big，测导航→DOM 行数就绪耗时；连续点击 5 行测选中响应 | 渲染 122ms；点击响应均值 53ms/最大 61ms（<100ms 达标）；快速滚动主线程 0ms 阻塞 |
| 目录树大数据 | 侧栏注入 500 子目录展开/收起/缓存复展，配合 invoke 台账验证懒加载 | 展开渲染 79ms、收起 35ms、缓存复展 69ms 且**仅 1 次 sftp/list**（缓存生效） |
| 终端洪峰输出 | 二进制通道直注 2000 帧 × 512B（约 1MB），测注入耗时与渲染追平；再注入 30 万行测 scrollback 与内存 | 注入 135ms、渲染追平 7ms（合并写节流有效，顺序保持）；scrollback 稳定 **25050 行**（25000 上限 + 视口，封顶正确）；30 万行后堆 153MB、树点击响应 21ms |
| 洪峰期间交互 | 注入期间点击工具栏/导航 | 35–41ms 响应，无冻结 |
| 终端输入有序性 | 快速连发 30 条命令（200 个输入帧），核对 `ssh/terminal/inputAck` 序号 | 200 帧全部 ack 且严格单调递增，无丢失；洪峰后 UI 35ms（注：mock 不回显 PTY 输入，回显属夹具缺口非产品问题） |
| 特殊文件名全链路 | 注入 emoji/双引号/空格/中文名文件：显示、双击预览、Esc 关闭、过滤搜索 | 全部通过（预览标题/内容正确、Esc 关闭、"🎉"/"space name" 过滤精确命中） |
| 排序语义 | 注入混合命名（大小写/数字/隐藏文件/CJK/符号前缀）测 name/size 升降序 | 目录始终置前；name 排序数字感知（file2<file10）、大小写不敏感；size 排序数值正确；行为符合 localeCompare(numeric, base) 预期 |
| 双击/重复提交守卫 | 删除确认按钮连点 2 次、批量发送连点 2 次、命令对话框开-Esc 循环 5 轮 | 删除/批量各仅 1 次 invoke 且弹层正常关（同步 busy 守卫有效）；命令对话框状态一致（mkdir/newFile 的 Enter 路径问题单列 R3-P1-1） |
| 错误注入与恢复 | sftp/list 抛 "connection reset by peer" | 旧列表保留 + 错误横幅原文呈现 + 下一次导航自动恢复，无状态残留 |
| 断开重连恢复 | `?err=disconnect` 下先导航到 /etc，断开→自动重连 | 重连后终端 Welcome 回放、当前目录 /etc 列表**自动重载**、无残留横幅/错误 |
| 焦点可见性 | 键盘 Tab 至 file-row / 工具栏按钮 / 路径栏，取 focus-visible 与计算样式 | file-row 与工具栏按钮有蓝色 outline；路径栏 :focus-visible 有自定义高亮，均可见 |
| i18n 抽查（en/ja/zh-TW） | 每语言 27 个 title + 11 处可见文本 + placeholder + 右键菜单 10 项 + 命令对话框标题/占位符；检测原始 key 回退与截断 | 三语全部真实翻译、无 key 回退、无截断（唯一瑕疵为 R3-P2-7 用词）；zh-CN 上一轮已抽查不重复 |
| aria 基线 | 全量 button 扫描 + role="switch" 检查 | switch 均有 aria-checked + 可见标签；仅 R3-P2-6 的 6 个树 caret 无名 |

### 5.4 第 3 轮统计与遗留

- 走查场景 8 组 × 运行时注入矩阵，发现条数：**P0=0、P1=3、P2=7**（另记夹具缺陷 1 处：mock sftp/rename 撞名丢源文件，随 R3-P2-2 收口）。
- 本轮价值回顾：三条 P1（幽灵点击重开、Esc 重命名照样提交、目录加载竞态）与两条 P2（Enter 双发、坏格式防御）均属第 1 轮常规走查与第 2 轮清理不可达的时序/键盘/对抗注入类问题，全部有 invoke 台账或 DOM 时间线证据。
- 遗留未验证：① R3-P1-1 的幽灵 click 属浏览器焦点激活行为，建议真机（真实宿主 webview/Chromium 版本）复验；② zmodem/拖拽上传仍无 headless 手段（第 1 轮起持续未覆盖）；③ `?err=disconnect` 与下载传输中途叠加的时序窗口过窄，未构造成功。
- 扫描脚本与 `/tmp/uiscan-ssh-r3` 运行时产物均不入库；截图 0 张留存（调试图已删）。
- 报告定稿后独立抽查复核（同日，dev server 重起 + 新写 verify-p1.js）：三条 P1 全部按记录精确复现——P1-1 时间线实测 close(771.9ms)→幽灵 click(773.5ms，BUTTON)→reopen(774.2ms)；P1-2 Esc 后 invoke 台账出现 `sftp/rename server.log→evil-verify.log`；P1-3 /var/log 导航后慢 /etc 响应晚到、路径栏/列表/行内容全部回跳 /etc。报告结论可信，无需修订。

### 5.5 第 4 轮修复与复验记录（2026-09-06）

> 第 4 轮修复 agent 接手上一中断 agent 的半成品（`src/lib/ghostClickGuard.ts`、`requestEpoch.ts`、`remotePathInput.ts`、`sftpRename.ts` 等 + App.vue/DirTree.vue 改动），逐条审计后确认 10/10 已接线且均有配套 spec（实际完成度高于中断时预期，无需补代码）；随后做全量回归与浏览器复验。

| 项 | 修复方式（新增模块 + 接线点） | 单测 | R4b 复验结论 |
| --- | --- | --- | --- |
| R3-P1-1 幽灵点击重开弹层 | `lib/ghostClickGuard.ts`（400ms 抑制窗 + mousedown 前驱判定）；App.vue 焦点归还前 arm、document capture 监听拦截合成 click | 7 条 | PASS：mkdir Enter 后 backdrop=0、createDirectory 1 次、无重开 |
| R3-P1-2 Esc 取消仍提交 | `lib/sftpRename.ts` `shouldCommitRename`；Esc 先清 renamingPath，卸载 blur 被守卫短路 | 6 条 | PASS：Esc 后 rename 台账 0 次、源文件原样 |
| R3-P1-3 目录加载竞态 | `lib/requestEpoch.ts`；loadDirectory 取号，落地/catch/finally 三处 isCurrent 校验，过期响应整体丢弃 | 4 条 | PASS：慢 /etc 晚到不回跳，footer 稳定 /var/log |
| R3-P2-1 Enter 双发 | 同 R3-P1-2 守卫（submitting 分支） | 同上 | PASS：rename 恰 1 次、无假错误横幅 |
| R3-P2-2 冲突无预检 | commitRename 增加 sftp/exists 预检 + window.confirm（七语 `sftpRename.overwriteConfirm`）；失败路径关编辑态并刷新；mock sftp/rename 改原子语义 + 2 条夹具单测 | 2 条（夹具） | PASS：撞名弹 confirm（接受=覆盖 1 次 invoke；取消=0 invoke 源完好）、编辑态收敛 |
| R3-P2-3 坏响应僵死 | `lib/sftpEntries.ts` `sanitizeSftpEntries`；loadDirectory 落地前统一 sanitize | 7 条 | PASS：entries:null → 空态视图、畸形行不炸比较器、0 pageerror |
| R3-P2-4 路径栏 `~`/`..` | `lib/remotePathInput.ts` `resolveRemotePath`；路径栏 Enter 走 `submitPathInput()` | 9 条 | PASS：`~`→home、`/home/demo/../etc`→/home/etc、消解后进路径历史 |
| R3-P2-5 键盘打开 | `lib/fileRowKeydown.ts` `decideFileRowAction`；文件行 Enter 打开 / F2 重命名 / Delete 删除（只读连接仅 Enter） | 4 条 | PASS：Enter 逐级进目录、F2/Delete 唤起对应 UI |
| R3-P2-6 树键盘 + caret 名 | DirTree.vue：role=treeitem、aria-expanded、roving tabindex、Enter/Space/方向键；caret `:aria-label`（七语 `sftpSide.expandNode/collapseNode`） | 组件 +5 条 | PASS：6/6 treeitem、0 无名 caret、方向键移焦、Enter 联动路径 |
| R3-P2-7 zh-TW「批次」 | i18n.ts zh-TW batchSend* 全组改「批次」（zh-CN 保持「批量」） | — | PASS：新措辞在位、旧措辞 0 节点 |

复验环境：`pnpm vite --port 5291 --strictPort` + playwright-core/系统 Chrome（headless，`/tmp/uiscan-ssh-r4b`，不入库），mock.html?rw=1，invoke 台账 + sftp/list 劫持 + dialog 拦截；**11/11 PASS**（10 条 + P2-2 取消路径变体）。回归：`pnpm typecheck` 0 错；`pnpm test` **237/237**（基线 191 + 新增 46：6 个新 lib spec + DirTree 组件 5 条 + mock 夹具 2 条等）。新增用户可见文案均已补七语。复验截图 0 张留存（失败才截图，本轮无失败；调试图已删）。R3-P1-1 的真机（宿主 webview）复核建议继续保留。
## 六、第 5 轮（复核扫描 / 收敛判定轮，2026-09-06）

> 双重任务：① 按原复现步骤逐条复核第 4 轮标注的 10 条 R4 修复（含"修复是否引入新问题"回归探针）；② 换六个此前未覆盖的角度找新问题（重连循环、深层目录、超长终端行、批量多会话、弹层快速开关、i18n 动态切换）。只记录不改代码。

### 6.1 扫描环境

| 项 | 值 |
| --- | --- |
| 轮次 | 第 5 轮复核扫描（2026-09-06，第 4 轮修复 + R4b 复验全绿后的收敛判定轮） |
| Dev server | `pnpm vite --port 5291 --strictPort`（http://localhost:5291/mock.html），扫描结束已 kill |
| 自动化 | playwright-core + 系统 Chrome（headless，channel:"chrome"），独立实例 `/tmp/uiscan-ssh-r5`（不入库）；脚本 A（修复复核 15 检查）+ 脚本 B/B 补充（六维新角度 6 检查） |
| 注入手段 | dbxPlugin 赋值陷阱：invoke 台账 + 可插拔 hook（sftp/list、ssh/sessions/list 劫持）、onEvent/onBinary 捕获（合成 disconnected 事件、终端帧直注）、clipboard/sendBinary 台账、onLocaleChange 补层（mock 缺该 optional 通道） |
| URL 参数 | `?rw=1`、`?err=disconnect` |

### 6.2 修复复核结论表（10/10 ✅，另 5 项回归探针全绿）

脚本 A 共 15 项检查（10 条修复按原复现步骤复验 + 5 项"修复不误伤"回归探针），**15/15 PASS**：

| 项 | 复验结果 | 证据要点 | 回归探针（修复是否引入新问题） |
| --- | --- | --- | --- |
| R3-P1-1 幽灵点击重开弹层 | ✅ | mkdir 键盘 Enter 提交后 backdrop=0、`sftp/createDirectory` 恰 1 次 | ✅ 守卫窗内带 mousedown 的真实鼠标点击仍正常重开弹层、Esc 可关（守卫不误伤真实点击） |
| R3-P1-2 Esc 取消重命名 | ✅ | Esc 后 `sftp/rename`/`sudo/rename` 台账 0 次、编辑态收敛、源文件原样 | ✅ 合法 blur 提交流（焦点移到路径栏）仍提交恰 1 次（守卫不误伤正常 blur 提交） |
| R3-P1-3 目录加载竞态 | ✅ | /etc 延迟 3s + 改道 /var/log，3.5s 后 footer 稳定 /var/log、hosts 行未出现 | ✅ 无改道时慢响应最终正常渲染 /etc（epoch 序号不过度丢弃） |
| R3-P2-1 Enter 双发 | ✅ | rename 台账恰 1 条、无错误横幅、编辑态关闭、新名落列 | —（与 P1-2 同守卫，已覆盖） |
| R3-P2-2 冲突预检 | ✅ | 撞名触发 confirm（含目标名）+ exists 1 次 + 接受后 rename 1 次覆盖落位 | ✅ 取消路径：dismiss 后 rename 0 次、源文件完好、编辑态收敛 |
| R3-P2-3 坏响应防御 | ✅ | `{entries:null}` → 空态视图（非永久 Loading）；null 行 + 缺 kind 行 → 合法行照常渲染；0 pageerror | — |
| R3-P2-4 `~`/`..` | ✅ | `~`→/home/demo（server.log 在列）；`/home/demo/../etc`→/home/etc；`/home/demo/../demo`→/home/demo | — |
| R3-P2-5 键盘打开 | ✅ | Enter 逐级 / → /home → /home/demo、F2 唤起重命名、Delete 唤起删除确认 | ✅ 只读连接下 F2/Delete 均不触发（不越权） |
| R3-P2-6 树键盘 + caret 名 | ✅ | 6/6 treeitem、tabindex=0 可达行存在、6 个 caret 0 个无名、ArrowDown/ArrowUp 双向移焦、树行 Enter 联动路径栏 | — |
| R3-P2-7 zh-TW「批次」 | ✅ | `[title="批次傳送命令"]` 在位、旧措辞「批量傳送命令」0 节点 | — |

### 6.3 新发现清单

统计：**P0 × 0，P1 × 0，P2 × 2**（其一为边缘场景，可不修）。六维新角度中四个维度零缺陷（见 6.4）。

**R5-P2-1 指标浮层不在 Esc 分层退出链：键盘用户无法用 Esc 关闭（同列 popover 全部支持）**

> R6 修复标注（2026-09-06）：已修复——metricsOpen 并入工具栏弹出层 Esc 分支（closeMetrics 同步清轮询）。typecheck 0 错、237 用例全绿；headless 夹具下浮层不可达（按钮依赖已连接态），留真机例行复核。
- 位置：`App.vue` `onDocumentKeydown` 的 Esc 分支（src/App.vue ~3938–3947：quickMenuOpen/pathHistoryOpen/columnsOpen/transferPanelOpen/connectionInfoOpen 五个 popover 均被关闭，`metricsOpen` 不在列表）
- 复现：点击工具栏 Gauge 图标打开指标浮层（.metrics-float）→ 按 Esc。实测浮层仍开启（residue=1）；快速命令/列选择/路径历史/连接信息 4 个 popover 同场景按 Esc 全部即时关闭。鼠标路径（Gauge 再点 = toggle、浮层 header X）关闭均正常，5 轮快速开关无残留。
- 影响：与同层级弹出物行为不一致；纯键盘用户必须 Tab 回 toggle 按钮或摸鼠标。属打磨项而非阻断。
- 建议：Esc 分支补 `metricsOpen.value = false`（或统一收敛为"任何 popover/浮层 Esc 即关"的判定表）。注意 closeMetrics 需同时清 metricsTimer（现实现已处理）。

**R5-P2-2（边缘，可不修）瞬态 notice 在显示窗口内不随 i18n 切换重译**
- 位置：`App.vue` `showNotice()`（src/App.vue 784–788）：调用点均以 `t()` 现译后**存字符串**入 `notice` ref，3.5s 自动清除；i18n 切换只触发模板重渲染，不会重译已存字符串。
- 复现：en 界面点 A+ 触发 notice "Terminal font size 14px" → 3.5s 窗口内 `onLocaleChange` 切到 zh-CN → notice 文案保持英文直至消失。
- 影响面评估：仅"切语言瞬时命中 3.5s 窗口"这一极窄场景；重新触发任意 notice 即用新语言。实测其余动态内容全部即时换语（见 6.4⑥）。错误横幅显示后端原文（本就不该前端翻译）、原生 confirm 为一次性快照，均属合理设计。
- 建议：可不修；若追求一致可改为存 `{ key, values }` 并在模板处 `t()`。

### 6.4 零发现维度证据（新角度四绿 + 复核无回归）

| 维度 | 测了什么 / 怎么测 | 结论 |
| --- | --- | --- |
| ① 重连循环 ×3 状态一致 | `?err=disconnect` 夹具首断 + onEvent 捕获层合成 disconnected 事件再断 2 次；每周期断言：pill 回 connected、横幅清除、当前目录 /etc 自动重载（hosts 在列）、每周期恰 1 次重连（attach 台账，无风暴/双发）；循环后终端 sendBinary 输入链路 15 帧正常、SFTP 导航正常、0 pageerror | PASS，零新发现 |
| ② 深层目录 12 级 | hook 合成 /d1…/d12 树；路径栏一次直达 12 级（19ms）→ parentFolder 逐级返回 12 步，每步 footer 精确命中且耗时 30–49ms（<300ms 达标）；路径历史含深层条目；heap 20MB 无异常增长；0 pageerror | PASS，零新发现（路径历史为下拉 popover，无面包屑组件；`..` 段消解在 R3-P2-4 已收口，本轮 12 级逐级返回间接复验） |
| ③ 终端 10k 超长行 | 二进制帧直注 10,016 字符单行（含 MARKER-NEEDLE-42）；渲染追平 27ms；折行 48 个 ≥80 列段行；Ctrl+F 搜索 "MARKER-NEEDLE-42" → 状态 "1/1" + 2 个高亮 decoration 定位成功；三击选中 → 选中复制链路 clipboard.writeText 20,076 字符落账；评估往返 1ms 无主线程冻结；0 pageerror | PASS，零新发现（折行/搜索/复制三条链路全通） |
| ④ 批量发送 40 会话 | 劫持 sessions/list 注入 40 会话（21 存活 + 19 断开）；目标列表 40 行且可滚动；勾选 3 行 → 滚到底再回顶 → 3 个 checkbox 全部保持勾选；"Live only" 快捷选择 → 21/40；发送 → 结果列表 20 条失败行、可滚动、滚到中部行不丢；summary "1 sent, 20 failed" 正确；0 pageerror | PASS，零新发现 |
| ⑤ 弹层快速开关 ×5 | 命令对话框/批量弹窗/快速命令/列选择/路径历史/连接信息 6 个弹层各 Esc 开关 5 轮：全部无残留、0 pageerror、文件列表滚动位置保持；指标浮层单独验证（Esc 不关 → R5-P2-1；toggle/X 双路径关闭正常、5 轮快速开关无残留） | 除 R5-P2-1 外零发现 |
| ⑥ i18n 动态切换 zh-CN→en→ja | onLocaleChange 补层（mock 缺该 optional 通道，已记夹具缺口）；title 属性三语即时换（服务器指标/Server metrics/サーバー指標）；指标浮层与批量弹窗打开状态下切换 → 标题/placeholder 即时换语、无 key 回退；瞬态 notice 例外 → R5-P2-2 | 除 R5-P2-2 外零发现 |

夹具缺口备忘（非产品问题）：mockDbxHost.ts 未实现 `onLocaleChange`（宿主 1.1 optional 通道），动态换语验证依赖扫描侧补层；mock 仅 1 个会话，多会话批量场景依赖 sessions/list 劫持。

### 6.5 第 5 轮统计与收敛判定

- 修复复核：**10/10 ✅**（另 5 项回归探针全绿，R4 修复未引入键盘流误伤/CSS 回归/只读越权等新问题）。
- 新发现：**P0=0、P1=0、P2=2**（R5-P2-1 指标浮层 Esc 缺口；R5-P2-2 瞬态 notice 换语，边缘可不修）。
- **未达到"第 5 轮零新发现"收敛判据**：尚余 1 条实质 P2（R5-P2-1，改动面为一行级）。建议：修复 R5-P2-1（R5-P2-2 明确豁免或一并收口）后，第 6 轮按本轮脚本 A/B 直接复跑——若全绿即可宣布扫描收敛。
- 复跑资产：脚本留存于 `/tmp/uiscan-ssh-r5`（verify-a.mjs / verify-b.mjs / verify-b2.mjs，不入库）；本轮截图 2 张调试图已全部删除，0 张留存。

## 7. 第 6 轮（收敛点验，2026-09-06）

最终判定轮：仅点验 R5-P2-1 修复 + 第 3/5 轮已收口项抽查 5 条，不做全面重扫。

### 7.1 R5-P2-1 点验结论：修复有效，真实验证通过

- **代码路径确认**（`src/App.vue`）：`onDocumentKeydown` Esc 链工具栏弹出层分支条件已含 `metricsOpen`（~3939，与 quickMenuOpen/pathHistoryOpen/columnsOpen/transferPanelOpen/connectionInfoOpen 同列），命中时调 `closeMetrics()`（~3945）；`closeMetrics` 同步置 `metricsOpen=false` 并 `clearInterval(metricsTimer)`（~3369–3372），无轮询泄漏。其余 popover 直接置 false、metrics 走 closeMetrics 的差异化处理正确（metrics 有 5s 轮询 timer）。
- **真实交互验证（mock.html?rw=1，headless Chrome + playwright-core）——R5 标注的"headless 夹具下浮层不可达（按钮依赖已连接态）"本轮证实不成立**：连接建立后 Gauge 按钮（`.icon-emerald` + `svg.lucide-gauge`，`:disabled="!connected"`）enabled 且可点，浮层可达，Esc 路径已完成浏览器级验证：
  - 点击打开 `.metrics-float` 成功；打开态 5.6s 内 invoke 台账 `ssh/metrics` +1（5s 轮询在跑）；
  - **按 Esc → 浮层关闭（residue=0）**；Esc 后再等 5.6s，`ssh/metrics` 计数零增量（**closeMetrics 清轮询实证**）；
  - 5 轮 toggle 快速开关无残留；全程 0 pageerror。
  - 原"留真机例行复核"保留项可降级为可选（机制已在真实浏览器事件链上验证）。
- 基线复跑：`pnpm typecheck` 0 错；`pnpm test` **237/237** 全绿，与 R6 修复标注一致。
- 资产备忘：工具栏现有 4 个 `icon-emerald` 按钮（Reconnect / Quick Sudo ×2 / Server metrics），脚本定位 Gauge 必须用 `button:has(svg.lucide-gauge)` 精确匹配（`page.click(".icon-emerald")` 会命中首个 Reconnect 造成假阴/假阳）。

### 7.2 抽查结果表（第 3/5 轮已收口项 5 条复跑）

| 抽查项 | 结论 | 证据 |
| --- | --- | --- |
| R3-P1-1 幽灵点击 | ✅ 无回退 | 键盘 Enter mkdir：backdrop=0、`sftp/createDirectory` 恰 1 次；守卫窗内真实鼠标（带 mousedown）仍可重开、Esc 可关 |
| R3-P1-3 目录竞态 | ✅ 无回退 | /etc 延迟 3s + 改道 /var/log：3.5s 后 footer 稳定 /var/log、hosts 不漏渲染；无改道时慢响应正常渲染 /etc（epoch 不过度丢弃） |
| R3-P2-6 树键盘 | ✅ 无回退 | 6/6 treeitem、6 caret 0 无名、tabindex=0 可达、ArrowDown→idx1 / ArrowUp→idx0 双向移焦、树行 Enter 联动路径栏 |
| R3-P2-7 zh-TW 批次 | ✅ 无回退 | `[title="批次傳送命令"]` 在位、旧措辞「批量傳送命令」0 节点 |
| R5 终端搜索（零发现维度抽查） | ✅ 无回退 | Ctrl+F 面板打开、搜 needle 命中 status "1/1" + 2 个 decoration、Esc 关闭面板 |

### 7.3 第 6 轮收敛判定

- 点验 1/1 ✅（R5-P2-1 修复有效且升级为浏览器级实证）；抽查 5/5 ✅（无回退）；新发现：**P0=0、P1=0、P2=0**。
- R5-P2-2（瞬态 notice 换语）按第 5 轮判定维持"边缘豁免，可不修"。
- **第 6 轮零新发现，扫描收敛。**
- 复跑资产：脚本留存 `/tmp/uiscan-ssh-r6/verify-r6.mjs`（不入库）；本轮截图 0 张留存（全程无失败）。