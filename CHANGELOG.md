# Changelog

本文件记录 DBX SSH Terminal 的面向用户的版本变更。除非另有说明，版本日期以 GitHub Release 发布日为准。

This file records user-facing changes for DBX SSH Terminal. Unless noted otherwise, version dates follow the corresponding GitHub Release.

## [0.4.79] — 2026-09-18

### 修复 / Fixed

- **连接成功后 IP/关键词高亮一直闪**：0.4.78 的"连接后补一次重绘 + 重扫"只治了过渡丢帧，真正的病灶是装饰层自己喂出来的自激回路——每次重绘都把视口内的高亮装饰整组拆掉重建，而 xterm 在装饰注册/销毁后会再触发一次整幅重绘，于是"重绘 → 扫描 → 拆建 → 重绘"永不停歇（headless Chrome 实测：连接落定约 2.5 秒起，空闲终端 4 秒内 296 次整屏重绘、装饰 DOM 拆建各 2637 次；关掉高亮则 0 次）。现在只有"本帧重绘**且**文本确实变了"的行才重建装饰，滚出视口的行照旧释放，回路被掐断（同样场景：0 次拆建、0 次额外重绘）。顺带修正 `onRender` 视口相对行号与缓冲绝对行号的换算，避免缓冲区滚过一屏后原地改写（进度行、`\r` 覆盖）的行残留旧色块。
  **IP/keyword highlights kept flickering after a successful connect:** the 0.4.78 "force one repaint + rescan after connect" only papered over the dropped transition frame; the real cause was a self-sustaining loop in the decoration layer — every repaint tore down and rebuilt all in-viewport highlight decorations, and xterm fires another full repaint right after decoration registration/disposal, so "repaint → scan → rebuild → repaint" never stopped (headless Chrome: starting ~2.5s after the session settles, an idle terminal produced 296 full-screen repaints and 2637 decoration DOM add/remove pairs in 4s, versus 0 with highlighting off). Decorations are now rebuilt only for rows that were repainted *and* whose text actually changed, while rows scrolled out of the viewport are still released (same scenario: 0 rebuilds, 0 extra repaints). The viewport-relative → buffer-absolute mapping of `onRender` is also corrected so in-place rewrites (progress lines, `\r` overwrites) no longer leave stale colour blocks once the buffer has scrolled past one screen.

### 验证 / Validation

- 前端：460 个测试通过（新增 6 个覆盖装饰保留策略与行号换算的回归用例），类型检查与生产构建通过。浏览器走查（headless Chrome + `mock.html?fresh=1&slow=2` 连接流程）：空闲 12 秒装饰拆建 0 次、额外重绘 0 次；注入含 IP 的新输出后高亮即时出现且不再抖动；滚回历史、回看中写入、原地改写三类场景装饰数稳定。
  Frontend: 460 tests passed (6 new regression cases covering the rebuild policy and row mapping), type check and production build passed. Browser walkthrough (headless Chrome, `mock.html?fresh=1&slow=2` connect flow): 12s idle produced 0 decoration rebuilds and 0 extra repaints; injecting new output with an IP highlighted immediately without churn; scrollback, writing while scrolled back, and in-place rewrites all kept a stable decoration count.

## [0.4.78] — 2026-09-18

### 新增 / Added

- **界面字体实时跟随 DBX 全局字体**：插件 UI 与终端字体不再写死内置回退值，改为直接引用宿主 `--font-sans` / `--font-mono` 令牌——此前 `:root` 内联样式压过主题桥引用导致宿主字体永不生效；DBX 改全局字体后经令牌推送实时生效（依赖会在字体变更时主动推送 token 的宿主版本）。
  **UI and terminal fonts now follow the DBX global font in real time:** instead of hardcoding the plugin's built-in fallback fonts, fonts resolve through the host's `--font-sans` / `--font-mono` tokens — previously an inline `:root` style overrode the theme bridge's token references so the host font never applied. Font changes in DBX now take effect live via token push (requires a host build that pushes font tokens on change).

### 改进 / Improved

- **断线重连体验**：「重新连接」在凭据暂不可用（DBX/插件重载后连接注册表为空）时不再瞬间失败，进入最长 30 秒的有界自动重试并显示等待文案；期间从侧边栏重开该连接即自动连上。待宿主支持插件重载后主动重推连接配置后，将无需任何手动操作。
  **Reconnect UX:** clicking "Reconnect" when credentials are temporarily unavailable (connection registry empty after DBX/plugin reload) no longer fails instantly — it enters a bounded 30s auto-retry with a waiting hint; reopening the connection from the sidebar within the window connects automatically. Once the host re-pushes connection configs on plugin reload, no manual step is needed.

- **界面打磨**：录制记录列表改为标准列表项（行间单条发丝分隔线、行 hover 底色，消除双线）；全局 Quick Sudo 配置弹窗重排（高度随内容、工具行承载计数与主按钮「新建配置」、空态图标居中）。
  **UI polish:** the recordings list is now standard list items (single hairline separators, row hover background — no more double divider lines); the global Quick Sudo profiles dialog is restructured (content-height, a toolbar row with the count and a primary "New profile" button, centered empty state with icon).

- **UI 组件体系迁移**：全部对话框、工具栏弹出层、下拉选择、右键菜单、开关与通知横幅从手写实现迁移到 reka-ui（shadcn-vue 风格 wrapper，见 `frontend/src/components/ui/`）+ Tailwind 工具类；视觉与交互语义保持不变（Esc 分层关闭链、焦点归还、幽灵点击守卫等既有行为原样保留），单一文件产物形态不变。
  **UI component migration:** every dialog, toolbar popover, dropdown select, context menu, switch, and toast banner moved from hand-rolled implementations to reka-ui (shadcn-vue-style wrappers under `frontend/src/components/ui/`) with Tailwind utilities; visuals and interaction semantics are unchanged (the layered Esc close chain, focus return, and ghost-click guard are preserved), and the single-file bundle shape is intact.

- **无障碍提升**：弹窗标题接入 reka DialogTitle（读屏可正确公告对话框用途）；对话框 Tab 焦点圈定由 FocusScope 承担；右键菜单获得完整的方向键/Enter/Esc 键盘导航；下拉选择支持键盘检索与明确的选中态公告；通知/错误横幅改为 reka Toast，读屏经 aria-live 区域公告内容，悬停暂停倒计时、滑动关闭开箱即用；工具栏切换钮补齐 aria-pressed，传输状态条补 role="status"，title 提示对键盘聚焦同样生效。
  **Accessibility upgrades:** dialog headings now use reka DialogTitle (screen readers announce dialog purpose correctly); Tab focus trapping in dialogs is handled by FocusScope; context menus gained full arrow-key/Enter/Esc keyboard navigation; dropdown selects support keyboard typeahead and explicit selected-state announcement; notice/error banners are now reka Toasts announced via an aria-live region with pause-on-hover and swipe-to-dismiss; toolbar toggle buttons expose aria-pressed, transfer status bars expose role="status", and title tooltips also appear on keyboard focus.

### 修复 / Fixed

- **macOS 上连接成功后 IP/关键词高亮闪烁、点一下就好**：0.4.77 的连接成功过渡（卡片遮盖 → 终端显示）在 Mac WKWebView 上会漏一帧重绘，高亮装饰停留在过渡前状态，直到第一次点击/输入才刷新。连接落定为 connected 后下一帧主动做一次全量刷新 + 高亮视口重扫（等价于用户首次点击的效果），Windows 行为不变。
  **IP/keyword highlights flickered after a successful connect on macOS until the first click:** the 0.4.77 connect-success transition (card overlay → terminal reveal) drops a repaint frame under WKWebView, leaving highlight decorations stuck in their pre-transition state until the first click or keystroke. The session now forces a full refresh plus a highlight viewport rescan on the frame after the session settles into `connected` (equivalent to that first click). Windows behavior is unchanged.

- **设置弹窗左栏选中项隐形、hover 无反馈、药丸左角被削平**：迁移 reka Tabs 后，`.modal button:not(...)` 按钮复位选择器的实际特异性（`:not()` 按参数计，(0,3,1)）压过了左栏页签的选中/hover 规则 (0,3,0)，把选中药丸的背景抹成透明——白字落在白底上完全看不见；且 `.settings-body` 的 `overflow-y: auto` 使其成为水平裁剪盒，`.settings-layout` 抵消弹窗 padding 的负 margin 冲不出裁剪盒，左栏左边 16px 被整体切掉（药丸左角变直角、视觉贴边）。修复：复位选择器排除 `data-slot="tabs-trigger"`；负 margin 移到 `.settings-body` 自身（裁剪盒扩到弹窗边缘）；hover 底色从亮色主题下近乎隐形的 `--accent` 改为前景色 8% 混合（明暗两态均可见）；左栏水平留白 12px，与宿主设置侧栏一致。
  **Settings dialog sidebar: selected item invisible, no hover feedback, pill's left corners clipped flat:** after the reka Tabs migration, the `.modal button:not(...)` button-reset selector's real specificity (`:not()` counts its argument — (0,3,1)) overrode the sidebar tab active/hover rules (0,3,0) and stripped the active pill's background — white text on a white dialog. Worse, `.settings-body`'s `overflow-y: auto` turns it into a horizontal clip box, so `.settings-layout`'s negative margin (meant to cancel the modal padding) could not escape it — the sidebar's left 16px was sliced off (square left pill corners, pill hugging the edge). Fixes: the reset excludes `data-slot="tabs-trigger"`; the negative margin moved onto `.settings-body` itself so the clip box spans to the modal edge; hover uses an 8% foreground mix (visible in both themes) instead of the near-invisible light-theme `--accent`; sidebar horizontal padding is 12px, matching the DBX settings sidebar.

- **取消重连后点「连接」仍拿旧凭据空转**：在侧边栏改密码/连接信息触发重连、点「取消」、修正凭据后再点「连接」，仍用 sidecar 里的过期凭据反复失败，把 inactive 重试梯子耗尽才落到错误态。「连接」与「重连」改为同路径——先 `host.reopenConnection` 请宿主按最新配置重开连接、刷新凭据，再打开会话。自动重连梯子的第一级重试同样先刷新凭据：改密码后终端断开可自动恢复，不再呈现需要手动自救的假错误。
  **"Connect" after cancelling a reconnect still spun on stale credentials:** after editing the password/connection in the sidebar triggered a reconnect, clicking Cancel, fixing the credential and clicking "Connect" still failed repeatedly with the sidecar's expired credentials until the inactive-retry ladder exhausted. "Connect" now shares the "Reconnect" path — it first asks the host to reopen the connection with the latest config (`host.reopenConnection`) before opening the session. The first rung of the auto-reconnect ladder refreshes credentials the same way, so a terminal drop after a password edit self-heals instead of surfacing a false error.

- **Linux 老系统上插件启动即崩溃**：在 glibc 低于 2.39 的发行版（Ubuntu 22.04 / Debian 11 等）上 sidecar 无法加载，DBX 报 "Plugin 'io.dbx.ssh' exited with status exit status: 1"。Linux 构建改为全静态 musl 二进制（x86_64 / arm64），流水线强制校验包内二进制无动态链接，并在 debian:11（glibc 2.31）与 alpine（musl）容器中真实执行冒烟。
  **Plugin failed to start on older Linux systems:** on distros with glibc older than 2.39 (Ubuntu 22.04 / Debian 11, …) the sidecar failed the dynamic loader and DBX reported "Plugin 'io.dbx.ssh' exited with status exit status: 1". Linux builds are now fully static musl binaries (x86_64 / arm64); the pipeline hard-fails if the packaged binary is dynamically linked and smoke-executes it inside debian:11 (glibc 2.31) and alpine (musl) containers.

- **慢速网络下连接测试报费解的宿主超时**：未展开「高级选项」的连接，存储的 `connect_timeout_secs` 为 0，宿主把 `connection/test` 的 RPC 截止按宿主回退定为 10 秒，而插件 sidecar 的拨号预算默认 30 秒——拨号超过 10 秒时宿主直接杀掉请求，用户只看到 "request 'connection/test' timed out after 10 seconds"。现在 sidecar 的测试拨号预算对齐宿主截止并预留 1 秒（缺省 → 9 秒），超时时报出带补救指引的错误（"Increase 'SSH timeout' under Advanced options and retry"）。遗留宿主侧诉求：宿主物化连接配置时应应用 manifest 声明的 30s 默认值，而非回退到 10s。工作台会话打开路径行为不变。
  **Cryptic host RPC timeout on connection test over slow networks:** for connections whose Advanced options were never expanded, the stored `connect_timeout_secs` is 0 and the host applies its own 10s fallback as the `connection/test` RPC deadline, while the plugin sidecar budgeted 30s — any dial past 10s was killed by the host with a bare "request 'connection/test' timed out after 10 seconds". The sidecar now aligns its test dial budget to the host deadline with a 1s margin (9s when the field is absent) and reports an actionable timeout ("Increase 'SSH timeout' under Advanced options and retry"). Remaining host-side ask: the host should apply the manifest's 30s default when materializing connection configs instead of falling back to 10s. The workbench session-open path is unchanged.

### 验证 / Validation

- 前端：454 个测试通过，类型检查和生产构建通过；`smoke_ui_mock.mjs` 与 `smoke_ui_fresh_review.mjs` 浏览器走查（headless Chrome）全部通过。后端：`cargo test` 通过（含 preferences 白名单校验与合并语义）。流水线：`check_candidates.py` 强制 Linux 候选包内二进制为全静态 ELF（PT_INTERP 探测），CI 另在 debian:11 与 alpine 容器内执行包内二进制冒烟。
  Frontend: 454 tests passed; type checking and production build passed; `smoke_ui_mock.mjs` and `smoke_ui_fresh_review.mjs` browser walkthroughs (headless Chrome) are all green. Backend: `cargo test` passed (including preferences whitelist validation and merge semantics). Pipeline: `check_candidates.py` now hard-requires the Linux candidate's packaged binary to be a fully static ELF (PT_INTERP probe), and CI additionally smoke-executes the packaged binary inside debian:11 and alpine containers.

## [0.4.77] — 2026-09-17

发布地址 / Release: [ssh-v0.4.77](https://github.com/jinpy666/dbx-plugin-ssh/releases/tag/ssh-v0.4.77)

### 新增 / Added

- **Termius 风格连接卡片**：连接过程从裸 spinner 升级为完整卡片——主机信息与身份行、进度线动画、可展开的连接日志（尝试次数、host-key 确认、重试、失败分类）、取消按钮，以及"进度到顶 → 对号 → 短暂停留"的连接成功过渡。
  **Termius-style connect card:** the connecting state is now a full card — host identity, animated progress track, expandable connect log (attempts, host-key prompts, retries, failure categories), cancel, and a "fill → check → hold" success transition.

- **会话与输入可靠性**：同一连接在多个工作台间隔离 SSH 会话；快速输入增加有界背压；粘贴输入统一为 PTY Enter；多行快捷命令完整保留；host 主题等宽字体与终端同步。
  **Session and input reliability:** isolated SSH sessions per workbench on the same connection, bounded backpressure for rapid input, pasted input normalized to PTY Enter, multiline quick commands preserved, and host mono font tokens synced with the terminal.

### 改进 / Improved

- **私钥路径字段**：改为文本输入框 + 右侧「选择本机 SSH 密钥…」下拉（宿主原生特判），不再收缩成一个孤立的小箭头。
  **Private key path field:** now a text input with a native "pick a local SSH key" dropdown, instead of collapsing into a bare chevron.

### 修复 / Fixed

- 连接成功动画播放期间，SFTP/工具栏不再提前渲染（消除抖动）；终端关键词高亮装饰层不再穿透连接遮罩；输入时高亮不再整屏闪烁，且高亮跟随行编辑。
  During the connect-success animation, SFTP/toolbar no longer render early (no flicker); keyword-highlight decorations no longer paint through the connect overlay; highlights no longer flicker while typing and now follow line edits.

### 构建 / Build

- 新增 `.gitattributes` 统一 LF 检出，Windows 本地构建与 CI（Linux）产物逐字节一致。
  Added `.gitattributes` enforcing LF checkouts so Windows builds are byte-identical to CI builds.

### 验证 / Validation

- 前端：458 个测试通过，类型检查和生产构建通过。
  Frontend: 458 tests passed; type checking and production build passed.
- CI：仓库契约、前端、Rust sidecar、SSH 容器冒烟、Windows 连接配置回归全部通过。
  CI: repository contracts, frontend, Rust sidecar, SSH container smoke, and Windows connection-config regression all passed.

## [0.4.76] — 2026-09-15

发布地址 / Release: [ssh-v0.4.76](https://github.com/jinpy666/dbx-plugin-ssh/releases/tag/ssh-v0.4.76)

### 改进 / Improved

- **传输历史与下载工作流**：完善下载完成后的状态、进度展示和本地文件操作入口，并统一传输历史记录与恢复任务的界面反馈。
  **Transfer history and download workflows:** Improved completed-download state, progress presentation, local file actions, and the UI feedback shared by transfer history and resumable tasks.

- **跨平台 UI 与发布准备**：补齐运行时环境声明、国际化文案和发布安装脚本，确保构建后的插件 UI 与安装流程保持一致。
  **Cross-platform UI and release readiness:** Added runtime environment declarations, localized copy, and release installation handling so the packaged UI and install flow stay aligned.

### 验证 / Validation

- 前端：420 个测试通过，类型检查和生产构建通过。
  Frontend: 420 tests passed; type checking and production build passed.
- Rust：457 个测试通过，Clippy 严格检查通过。
  Rust: 457 tests passed; strict Clippy checks passed.
- MCP、SSH/SFTP、OTP、触发器、性能和 UI walkthrough smoke 全部通过。
  MCP, SSH/SFTP, OTP, trigger, performance, and UI walkthrough smoke tests all passed.

## [0.4.75] — 2026-09-15

发布地址 / Release: [ssh-v0.4.75](https://github.com/jinpy666/dbx-plugin-ssh/releases/tag/ssh-v0.4.75)

### 新增 / Added

- **触发器驱动的 SSH 认证流程**：支持 Expect 风格的终端触发器，可根据服务器输出匹配正则并发送文本、密钥槽位中的 secret，或执行本地命令；规则支持超时、等待间隔和条件回复。
  **Trigger-driven SSH authentication:** Added Expect-style terminal triggers that match regular expressions in server output and respond with text, secret-store values, or local commands, with timeout, delay, and conditional-response support.

- **外部凭据命令**：新增 `password_command` 和 `passphrase_command`，可在连接时从本地密码管理器或脚本获取登录密码、私钥口令；显式配置的凭据优先于命令结果。
  **External credential commands:** Added `password_command` and `passphrase_command` for retrieving login passwords and private-key passphrases from a local password manager or script at connection time; explicit credentials always take precedence.

- **系统剪贴板文件上传**：聚焦 SFTP 面板后，可使用 Ctrl/Cmd+V 将系统剪贴板中的本地文件直接上传到当前远程目录；Upload 按钮和拖放上传继续可用。
  **Native clipboard file uploads:** With the SFTP panel focused, Ctrl/Cmd+V can upload local files from the system clipboard into the current remote directory. The Upload button and drag-and-drop flow remain available.

- **SSH 算法兼容策略**：新增 `secure`、`compatible` 和 `legacy` 三种策略，分别用于严格安全连接、兼容 SHA-1 MAC 的旧服务器，以及需要更老旧 KEX/CBC 算法的服务器。
  **Configurable SSH algorithm policies:** Added `secure`, `compatible`, and `legacy` profiles for strict security, older servers requiring SHA-1 MACs, and legacy KEX/CBC compatibility.

- **下载完成后的本地操作**：下载记录现在可以直接打开文件或在文件管理器中定位，减少终端、文件管理器之间的切换。
  **Post-download local actions:** Completed downloads can now be opened directly or revealed in the system file manager, reducing context switching after transfers.

### 改进 / Improved

- 连接表单、生命周期参数和 MCP 连接参数统一支持触发器、外部凭据命令和 SSH 算法策略，配置行为保持一致。
  Connection forms, lifecycle parameters, and MCP connection arguments now share the same support for triggers, external credential commands, and SSH algorithm policies.

- 跳板机连接会继承 SSH 算法策略，避免多跳连接在中间节点上出现不一致的协商行为。
  ProxyJump connections inherit the SSH algorithm policy so multi-hop sessions negotiate consistently across intermediate hosts.

- 兼容模式仍将安全算法置于优先位置，只在必要时追加兼容算法；未知策略值会回退到更严格的 `secure` 配置，不会意外开启 legacy 算法。
  Compatible mode keeps secure algorithms first and only appends compatibility algorithms when needed; unknown policy values fail closed to the stricter `secure` profile instead of enabling legacy algorithms unexpectedly.

- MCP 工具描述补充了新参数、认证流程和安全边界，便于 DBX MCP 桥及其他 MCP 客户端正确发现和调用。
  MCP tool descriptions now document the new parameters, authentication flows, and safety boundaries so the DBX MCP bridge and other MCP clients can discover and use them correctly.

### 修复 / Fixed

- 修复下载完成后只能看到路径、无法从插件界面继续操作的问题。
  Fixed the post-download workflow where users could see the saved path but could not continue with a local file action from the plugin UI.

- 修复非法触发器配置可能被静默忽略的问题；无效 JSON、无法编译的正则、未知 secret 槽位和超出限制的规则现在会在连接阶段明确失败。
  Fixed invalid trigger configurations being silently ignored; malformed JSON, uncompileable regular expressions, unknown secret slots, and oversized rule sets now fail clearly during connection setup.

- 修复密码认证在使用外部密码命令时仍要求填写静态密码的问题。
  Fixed password authentication continuing to require a static password when an external password command is configured.

- 加强本地文件打开入口的路径校验：只有本插件已记录且成功完成的下载文件才允许通过插件操作系统接口打开，避免按钮成为任意本地路径打开入口。
  Hardened local-file actions so only files recorded as successfully completed downloads by this plugin can be opened through the plugin's OS integration, preventing the action from becoming an arbitrary local-path opener.

### 安全与兼容性 / Security & Compatibility

- 触发器发送的 secret 通过 DBX secret binding 解析，不写入普通配置、日志或 MCP 参数；服务器输出可能诱导触发器匹配，请只对可信服务器启用自动回复。
  Trigger secrets are resolved through DBX secret bindings and are not written to ordinary configuration, logs, or MCP arguments. Because server output can intentionally influence matching, enable automatic responses only for trusted servers.

- 新增能力要求 DBX `>=0.5.77` 和 Host API `>=1.0.0`。
  The new capabilities require DBX `>=0.5.77` and Host API `>=1.0.0`.

- 本版本保持现有 SSH/SFTP 连接配置兼容；未配置算法策略时使用 `compatible` 默认行为，现有连接无需迁移。
  Existing SSH/SFTP connection configurations remain compatible. Connections without an explicit algorithm policy use the `compatible` default and require no migration.

### 验证 / Validation

- 前端：420 个测试通过，类型检查和生产构建通过。
  Frontend: 420 tests passed; type checking and production build passed.

- Rust：456 个测试通过，格式检查、Clippy 和锁定依赖构建通过。
  Rust: 456 tests passed; formatting, Clippy, and locked-dependency builds passed.

- 发布包：Linux x64/arm64、macOS x64/arm64、Windows x64 均已构建并上传。
  Release packages: Linux x64/arm64, macOS x64/arm64, and Windows x64 were built and uploaded.

## 历史版本 / Previous releases

- [GitHub Releases](https://github.com/jinpy666/dbx-plugin-ssh/releases)
