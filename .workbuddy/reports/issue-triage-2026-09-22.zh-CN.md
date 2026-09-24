# DBX SSH 插件 Issue 状态盘点

**仓库**：`jinpy666/dbx-plugin-ssh`（本地 `main` @ `acddf777`，工作区干净，仅有一份未跟踪方案文档）
**基线版本**：`ssh-v0.6.0`（2026-09-22 09:30 UTC 发布，manifest 版本 `0.6.0`）
**盘点时间**：2026-09-22
**样本**：全部 69 条 issue（29 open / 40 closed）

## 判据说明

每条结论都基于三类可核验证据，不使用推测：

1. **代码事实**：`main` 分支上的实现（文件路径 + 行号）。
2. **提交/PR 事实**：`git log`、PR 合并状态、release tag。
3. **工单讨论**：维护者在 issue 评论中的定性（会与代码事实交叉验证，冲突时以代码为准）。

"已解决"的口径 = **功能已在 `main` 落地且随 0.6.0 发布**；"可解决"的口径 = **不依赖上游、插件侧存在明确可执行的修法**。

---

## 一、结论摘要

| 分类 | 数量 | 含义 |
| --- | --- | --- |
| **A. 已在 main 实现，建议关闭** | 8 | 功能已落地，issue 只是滞后未关，补齐回归证据即可闭环 |
| **B. 未解决，插件侧可解决** | 12 | 无上游阻塞，修法明确，可直接排期 |
| **C. 受上游/宿主限制，或信息不足** | 9 | 依赖 DBX 能力开放，或需提问者补料才能定位 |

29 条 open issue 中，**约 28% 实际上已经修好了**（A 类）；另有 **1 条（#91）的实现已存在于未合并分支上**，只差评审合并。

---

## 二、分类总表

| # | 标题 | 分类 | 关键证据 | 建议动作 |
| --- | --- | --- | --- | --- |
| 31 | SSH 终端字体可单独设置 | **A** | `SettingsDialog.vue:141` 字体预设 + 自定义 + 字号；`b7bece1a` | 关闭 |
| 62 | 支持端口转发 | **A** | `PortForwardDialog.vue`、`backend/src/forward.rs`；0.6.0 CHANGELOG | 关闭 |
| 63 | SFTP 右键上传/下载 + 拖拽 | **A** | `8a3570aa`（右键上传）+ 0.6.0 三条拖拽链路 | 关闭 |
| 77 | 非 HTTPS 下 `crypto.randomUUID` 不存在 | **A** | `frontend/src/lib/uuid.ts`（getRandomValues 降级 + 单测） | 关闭 |
| 79 | 文件上传报错 | **A** | `e3b89fc6` / PR #92；宿主桥失败回退原生选择器 | 关闭（需回归） |
| 83 | 上传失败 `unknown plugin file handle` | **A** | 同 #79；截图错误与实际实现一致 | 关闭（需回归） |
| 71 | mac 输入过快丢字 | **A** | `58f6ef3a` macOS 直写提交绕开 xterm 延迟 diff；`terminalInputQueue.ts` | 请在 0.6.0 复测后关闭 |
| 44* | 本地终端功能 | **C** | 分支 `codex/ssh/local-terminal` 已实现，PR #85–#88 打开，阻塞于宿主 A1 贡献点 | 提供 PR-A4 进度说明 |
| 91 | 字体/行距/字距无法设置 | **A′** | **未合并分支** `codex/ssh/terminal-themes`：`fontWeight/lineHeight/letterSpacing/padding/cursor` + Tabby 配色 + 多主题（+6905 行） | 评审合并即可关闭 |
| 96 | JSON 预览格式化 + 复制字段 | **B** | `TextPreview.vue` 仅 CodeMirror 高亮，无格式化/复制动作 | 小改动，可排期 |
| 73 | 宿主背景图片导致终端纯黑 | **B** | 无 `allowTransparency`；底色取宿主 `--color-background`，透明场景 xterm 回退 `#000` | 可修，见 §四 |
| 90 | `sz` 下载无任何反馈 | **B** | `App.vue:1762` 仅接受 `role==="send"`，`sz` 走 `detection.deny()` 静默丢弃；trzsz 已支持下载 | 实现 zmodem receive |
| 66 | SFTP 下载限速 | **B** | 无任何限速实现（`downloadPrefs.ts` 无 rate 字段） | 令牌桶，可排期 |
| 78 | 文件夹上传 | **B** | 无 `webkitdirectory`；后端已有 `sftp_tree.rs`（仅下载）可对称实现 | 中改动 |
| 95 | 终端执行 git 看不到内容 | **B** | 截图显示输出被**部分吞掉**（只剩红色碎片），属渲染/解析缺陷 | 需复现定位，见 §四 |
| 94 | macOS 打字慢 + 模式键后首击丢失 | **B** | `terminalWebkitInput.ts` 已覆盖 keyCode 229；Caps/Shift 后首击丢失未覆盖 | 需复现确认 |
| 11 | 新建连接「测试」超时 | **B** | `ssh.rs:325` 走私有事件 `connection/challenge`，无应答者 | 方案已就绪，见 §四 |
| 69 | 同上（根因记录） | **B** | 同上；`docs/IMPL_PLAN_SSH_HOSTKEY_REQUESTUSERINPUT.zh-CN.md`（914 行，未跟踪） | 直接实施 |
| 72 | Docker 环境「测试」超时 | **B** | #69 的 Docker 变体 | 随 #69 一并验收 |
| 75 | 阿里云短信 MFA 无法输入验证码 | **B** | 前端无交互式提示 UI，`auth_flow_mode` 只能自动应答 TOTP | 复用同一 `requestUserInput` 通道 |
| 80 | 选中即复制/右键粘贴无效 | **B/C** | 已实现（`onSelectionChange` + `resolveTerminalRightClickAction`），宿主拦截事件 | 捕获阶段兜底 or 上游 |
| 12 | 小标签无 tooltip + 录制无法删除 | **B** | 录制部分已修复并验证（`96606d61`）；tooltip 部分无实现 | 拆分为独立 issue |
| 23 | SSH 无法连接（P0，Finalshell 正常） | **C** | 截图为 `os error 10054` 握手期被服务端断开；无算法协商日志 | 需 `ssh -vvv` 对比日志 |
| 25 | 堡垒机反复重连 | **C** | 维护者已索要堡垒机型号与细节，未回复 | 需补料 |
| 93 | SFTP 下载未落到本机 | **C** | `local_downloads.rs` 明确记录 WebView 无法落盘；Docker/Web 端字节落在服务器侧 | 需确认部署形态 |
| 74 | 同一连接多开窗口 | **C** | 工作台内已支持「新建会话 tab」（PR #50）；侧边栏独立窗口依赖宿主 | 说明现状并关闭或转需求 |
| 76 | 密码/密钥同步 | **C** | `vault.rs` 已实现本机密钥库；跨设备同步依赖宿主同步能力 | 转上游 |
| 57 | 对接 AI 助手 | **C** | MCP 已可用（31 工具，`--mcp` + DBX MCP 桥，`docs/MCP.zh-CN.md`）；应用内助手依赖宿主 | 文档引导 + 转上游 |

\* #44 归类 C，但其代码实现已存在于未合并分支，性质更接近 A′。

---

## 三、A 类：已在 `main` 落地，建议直接收口

### #31 终端字体单独设置
`frontend/src/components/SettingsDialog.vue:134-175` 提供「跟随宿主 / 8 个预设等宽字体 / 自定义」+ 字号，代码注释直接标注 `issue #31`；引入提交 `b7bece1a`，随 0.6.0 发布。原 issue 诉求（终端字体与 DBX 全局解耦）已满足。

### #62 端口转发
`backend/src/forward.rs` + `frontend/src/components/PortForwardDialog.vue`，支持用户级 `-L/-R`、录入校验、冲突预检、监听地址与网卡选择合并为单字段。这是 0.6.0 的头条特性。

### #63 SFTP 右键上传 / 拖拽
右键上传由 `8a3570aa` 落地，issue 内已有维护者的回归记录（v0.4.79 实测通过）；拖拽上传在 0.6.0 收敛为**三条链路统一门禁**（终端 / SFTP 面板 / 宿主级拖入），其中宿主级拖入此前无只读保护，属顺带修掉的缺陷。

### #77 `crypto.randomUUID` 在非 HTTPS 下不可用
`frontend/src/lib/uuid.ts` 做安全上下文降级：原生 API 存在时透传，不存在时用 `getRandomValues` 组装 v4；配套 `uuid.spec.ts` 覆盖两种路径。分支 `codex/ssh/issue-77-uuid-fallback` 已合并。

### #79 / #83 上传报错
两张截图的错误分别是 `unknown plugin file handle`（宿主文件桥返回）与 `Upload cancelled (local-read-error)`，指向同一根因：宿主文件桥 pick/读盘失败。修复 `e3b89fc6`（PR #92 已合并）在宿主桥失败时改走 WebView 原生文件选择器（File API）重挑继续上传。**注意**：维护者在 #79/#83 的评论「要等 DBX 上游修复」已过期，实际已用降级方案绕过。

### #71 macOS 快速输入丢字
`58f6ef3a`（macOS WKWebView 绕过 xterm 6.1 延迟 textarea diff）+ `terminalInputQueue.ts`（按传输顺序回放）随 0.6.0 发布。issue 中最后一次反馈停留在 `v0.4.80`，需在 0.6.0 上复测后关闭。

### #91 字体 / 行距 / 字距设置（A′：实现已在分支上）
未合并分支 `codex/ssh/terminal-themes` 的 `frontend/src/lib/terminalAppearance.ts` 已定义完整外观契约：

```
fontWeight / fontWeightBold / lineHeight (1-3) / letterSpacing (-5-10px)
paddingX / paddingY / cursorStyle / cursorBlink / cursorInactiveStyle
schemeSource: host|custom + 明暗双槽配色 + 自定义主题快照（上限 60/30）
```

并附 `terminalSchemeCatalog.ts`（Tabby 配色迁移脚本 `gen-terminal-schemes.py`）与 `TerminalSchemePicker.vue` / `TerminalHotkeyEditor.vue`。这基本覆盖了 #91 的全部诉求，**只差评审合并**。

---

## 四、B 类：未解决，插件侧可解决

### #69 / #11 / #72 新建连接「测试」超时 —— 优先级最高
**根因已由 issue 提出者完整定位，且与代码一致**：`backend/src/ssh.rs:325` 在主机密钥未知时发出私有事件 `connection/challenge` 并等待最长 300 秒（`ssh.rs:421` `HOST_KEY_CHALLENGE_WAIT`），而宿主只在 `host/requestUserInput` 提示期间暂停 RPC 截止时间——私有事件不是宿主 prompt，倒计时照跑，10 秒后请求被掐。

**为什么值得优先做**：这是**首次连接必现**的路径，报错文案还会把用户引向排查网络与密钥；#72 证明它在 Docker 部署下同样存在（且用户侧绕过无效，实测白屏）。

**现状**：方案文档 `docs/IMPL_PLAN_SSH_HOSTKEY_REQUESTUSERINPUT.zh-CN.md`（914 行，**未跟踪**）已写成可执行的任务分解——vendored SDK 移植上游 Host API 1.1 的 `HostClient` + string-id 响应路由，`PromptBroker` 优先走宿主弹窗、`-32601/-32001` 降级回 `connection/challenge`、宿主不应答则 fail closed。纯插件侧改动，不动前端协议。

**动作**：直接实施。属认证类改动（`human_review_required`），按仓库契约需人工评审、不得自动安装或重启 DBX。

### #75 阿里云短信 MFA 无法输入验证码
前端当前**没有交互式提示 UI**：`auth_flow_mode` 只能自动应答 TOTP（`password_prompt_hint` / OTP 提示词是预置文本匹配，无法承载"短信码每次不同"的场景）。这与 #69 需要的是同一条能力——宿主 `host/requestUserInput`。建议与 #69 合并为同一工作流：先打通通道，再复用到登录期 MFA。

### #90 `sz` 下载无反馈
`frontend/src/App.vue:1762` 的 `handleZmodemDetection` 仅接受 `detection.get_session_role() === "send"`，其余一律 `detection.deny()`；无待传文件时**静默拒绝**，与 issue 描述的"无任何反馈"完全吻合。`terminalZmodem.ts` 只有 `sendZmodemFiles`（发送）一条路径。

**补充事实**：trzsz 链路已支持下载（`terminalTrzsz.ts` 的 `saveDownloadedFiles` / `recvFiles`，含目录），所以"从远端取文件"并非完全无解——但 `sz` 用户不会主动改工具。修法：为 zmodem 补 receive 分支（落盘复用 `fileTransfer` / `<a download>`），或至少在 `deny()` 时给出明确提示与替代方案。注意 #65（同一诉求）已被标为 #90 的重复项，本条是唯一追踪点。

### #96 JSON 预览格式化与复制
`TextPreview.vue` 用 CodeMirror + 按文件名匹配语言（JSON 有语法高亮），但**没有任何格式化 / 复制动作**。诉求可拆为三项小改动：JSON 美化按钮、复制单个字段值、复制全文。改动局限在预览弹窗，风险低。

### #73 宿主背景图片导致终端纯黑
`frontend/src` 中**不存在 `allowTransparency`**；终端底色来自宿主 token `--color-background`（`hostTheme.ts:7`），缺失或不可解析时 `resolveAppearance` 回退规范色板。当宿主设了背景图片时，若下发的是透明/非颜色值，xterm 会保留其默认 `#000` 视口底色。`21c66bf2` 修的是 xterm 6.1 viewport `#000` 优先级问题（同一现象的另一个来源）。修法：把背景色解析结果做净化，非法/透明时退回色板底色；如需图片透出则开启 `allowTransparency` 并把容器底色置透明。

### #95 终端执行 git 看不到内容
截图（`611x210`）显示 `git status` 的 untracked 文件名**整体消失，只余红色碎片**（`!`、`1`、`60,`、`[`、`true,`，缩进逐级递增）——这是**输出被部分吞掉**的典型形态，而非单纯配色问题。可疑组件按可能性排序：

1. `terminalCommandMarkers.ts`（`Osc633CommandParser`，370 行）——基于 `\u001b]633;` 前缀 + BEL/ST 终止符做流式消费，若终止符匹配逻辑在流边界失配可能吞字节。
2. `terminalTrzsz.ts` 的 `processServerOutput` 过滤器——常驻在每条下行流上。
3. `registerOscColorQueryHandlers` / OSC 52 处理器——若误判 `ESC ]` 序列会一直吞到 BEL。
4. `terminalWriteThrottle` 合并写入的边界处理。

**动作**：需要提问者给出确切命令 + DBX 版本 + 原始字节（sidecar trace），再做二分定位。这是本次盘点中**最有价值的一条未定位缺陷**。

### #94 macOS 打字慢 / 模式键后首击丢失
0.6.0 的 `terminalWebkitInput.ts`（191 行）已专门处理 WKWebView 下 `keyCode=229` 的 IME 直写路径；但 issue 第 2/3 点描述的是 **CapsLock/Shift 之后的第一击无响应**（修饰键单独按下 → 下一次可打印键丢失）。这是不同的输入序列，现有控制器未必覆盖。提问者环境为 `v0.5.0`，建议先在 0.6.0 复测，未改善再补 `terminalWebkitInput` 的修饰键状态机用例。

### #66 SFTP 下载限速 / #78 文件夹上传
两条都是**明确未实现**的中等改动，且后端已有可对称复用的骨架：`sftp_tree.rs`（递归下载的状态机、配额、符号链接处理、失败汇总）可直接改造为上传方向；限速则加在传输分块循环上。

### #80 选中即复制 / 右键粘贴
实现是完整的：`App.vue:1449` 的 `onSelectionChange` 写剪贴板（受 `termSelectCopy` 开关控制）、`App.vue:6602` 的 `showTerminalMenu` 在选中复制模式下把右键映射为粘贴。维护者已确认「网页版好使，DBX 里事件被拦截」。插件侧仍有一条兜底可试：在 document **捕获阶段**监听 `contextmenu` 并提前 `preventDefault`，压过宿主的事件拦截。

### #12 录制删除（已修）+ 标签 tooltip（未修）
录制列表/删除已由 `96606d61` 钉住并在 0.4.79 实测通过（含仅头信息录制）。tooltip 部分无对应实现。**建议拆分为独立 issue 再关本条**，避免把已修复内容一直挂在 open 状态。

---

## 五、C 类：受上游限制或需补充信息

| # | 阻塞性质 | 说明 |
| --- | --- | --- |
| 23 | 信息不足 | 截图仅给到 `os error 10054`（握手期被服务端 FIN）。同为「Finalshell 能连」，最大嫌疑是算法协商或服务端策略（如仅允许特定 KEX/MAC、或拒绝对话标识）。需要 `ssh -vvv <host>` 与插件侧 trace 对比。 |
| 25 | 信息不足 | 维护者已索要堡垒机型号/细节，提问者未回复；与登录期 MFA 相关，`codex/ssh/jumpserver-ki-mfa` 分支已有相关实现。 |
| 93 | 需确认部署形态 | `local_downloads.rs` 文档明确：WebView 自身无法落盘，插件优先用宿主 `fileTransfer`，其次 sidecar 落盘（桌面端 = 本机 Downloads，Docker/Web = **服务器侧** 目录），最后才 `<a download>`——而 `<a download>` 在 Tauri/WKWebView 无下载处理器时会被静默取消。提问者现象（提示成功但本机没有）指向 Docker/Web 形态。修法：侧车与客户端不同机时强制浏览器下载通道，或把"落在服务器侧"显式告知用户。 |
| 74 | 宿主限制 | 工作台内同连接多开已支持（`openNewSessionTab`，PR #50）。侧边栏连接/标签由宿主管理，协作者已回复需 DBX 开放能力。 |
| 76 | 宿主限制 | `vault.rs` 已实现 keyfile / keychain 双后端密钥库，本机加密没问题；"换一台机器恢复"需要宿主同步通道。 |
| 57 | 半可解决 | MCP 侧已可用：31 个 SSH/SFTP 工具，支持 DBX MCP 桥（凭据由宿主解析，参数不带密码）与 `--mcp` 独立 stdio 模式，`docs/MCP.zh-CN.md` 有完整配置示例。issue 想的是"应用内 AI 助手"，那需要宿主。**建议：先补一段"当前如何用外部 Agent 操作服务器"的答复并保留 open 跟踪应用内助手。** |
| 44 | 宿主限制（代码已就绪） | 分支 `codex/ssh/local-terminal` 已实现 connectionless 本地终端 tab，PR #85–#88 打开中，合并门控是宿主 A1 贡献点。维护者评论「在做了」属实。 |

---

## 六、已关闭 issue 提供的基线（40 条）

- **COMPLETED**：38 条，集中在连接可靠性（#1 VPN 超时、#11 同族、#21 私钥解码、#22 Windows 启动、#55 重连、#58 初始化失败）、认证与算法（#9 hmac-sha1、#13 老设备算法、#15 私钥内容、#17 JumpServer、#30 堡垒机 OTP）、SFTP 体验（#18/#34/#37/#46/#60/#64/#70）、终端（#10/#32/#33/#38/#56/#59/#68）与 MCP（#52/#61）。
- **DUPLICATE**：1 条（#65 → #90）。
- 值得注意的是：**认证/算法/连接这类"连不上"的高频缺陷在 0.4.7x 线已批量收敛**，因此 #23 / #25 属于"更硬的边缘样本"，不能指望复用已有修法，必须拿日志。

---

## 七、建议的收口顺序

1. **零成本闭环（今天可做）**：关闭 A 类 8 条（#31/#62/#63/#77/#79/#83/#71），补齐回归证据。#71 需在 0.6.0 复测。
   - 注意：#79/#83 的评论"等 DBX 上游修复"与已合并的降级实现矛盾，关闭时请更正说明。
2. **评审换产出（最高性价比）**：合并 `codex/ssh/terminal-themes` → 直接关掉 #91。
3. **优先级最高的新工作**：#69 / #11 / #72 的主机密钥 `requestUserInput` 接入（方案现成，含 914 行任务分解），#75 可并入同一通道。
4. **用户可感知的功能缺口**：#90（zmodem receive）、#96（JSON 预览动作）、#78（目录上传）、#66（限速）。
5. **需先要料再动手**：#95（**建议尽快索取复现信息，这是唯一一条"输出会被吞"的缺陷**）、#94、#23、#25、#93。
6. **转上游或说明现状后关闭**：#74、#76、#57、#44（附 PR-A4 进度）。

---

## 附录：验证方法

- 拉取 issue 全量：`gh issue list --state all --limit 200 --json number,title,state,stateReason,labels,createdAt,closedAt,author,body`
- 逐条读评论：`gh issue view <n> --comments`
- 提交级追责：`git log --all --oneline --grep="#<n>"`
- 分支合并态：`git branch --merged main` / `--no-merged main`
- 本次盘点未安装或重启任何 DBX 环境，未改动仓库受控文件；本报告写在未跟踪目录 `.workbuddy/reports/`。
