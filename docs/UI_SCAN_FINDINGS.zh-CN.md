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
