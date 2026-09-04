# A-SSH 路交付报告（终端工作台专业细节增强，对标 tiny-rdm SshPage）

日期：2026-08-29。工作目录：`~/btroot/dbx-plugins/ssh-sftp`，在未提交 batch3 + 历轮（S-A/S-B/X/XB/P-SSH）改动之上继续；
仅触碰本路所有权文件：`frontend/src/**`（App.vue、lib/{commandHistory,quickCommands,connectionInfo,terminalZoom,workbench.spec,i18n,mockDbxHost}）、
`backend/src/{model,ssh}.rs`、`scripts/smoke_batch3_test.py`、`docs/PROTOCOL.zh-CN.md`、
`docs/screenshots-a-ssh/`（新建截图）、`docs/PROGRESS-A-SSH.zh-CN.md`（本文件，新建）。
未执行任何 git commit/push；无新 npm/cargo 依赖。

## 0. 总体验证基线（本轮终值）

| 套件 | 基线 | 本轮终值 |
| --- | --- | --- |
| backend `cargo test` | 108 passed | ✅ **109 passed / 0 failed**（+1 新增 `auth_method_names_round_trip_for_display`；`session_info_payload` 测试就地扩展） |
| frontend `pnpm typecheck` | — | ✅ vue-tsc --noEmit exit 0 |
| frontend `pnpm test` | 41/41 | ✅ vitest 2 spec **51/51**（历轮新增 8 + 本轮认证方式标签 2） |
| frontend `pnpm build` | — | ✅ 自包含 `ui/index.html` 产出 |
| `scripts/smoke_test.py` | PASS | ✅ PASS 1.1s（真机容器） |
| `scripts/smoke_fs_test.py` | 17/0/0 | ✅ PASS 17 / SKIP 0 / FAIL 0 |
| `scripts/smoke_batch3_test.py` | 17/0/0 | ✅ **17 passed / 0 skipped / 0 failed**（sessions/list 用例扩展 authMethod 断言） |
| `scripts/smoke_sudo_otp_test.py` | 10/0/0 | ✅ 10 passed / 0 skipped / 0 failed |
| `scripts/smoke_mcp.py` | — | ✅ initialize / 参数校验 / tools-call 往返全绿 |

测试容器：`dbx-ssh-test`（linuxserver/openssh-server，127.0.0.1:2222），全部 smoke 对本地
`backend/target/release/dbx-plugin-ssh`（改动后重编）真机真跑、无环境 SKIP。
注意：`sidecar_client.py` 默认解析到 DBX **已安装副本**，本轮全部 smoke 显式
`DBX_PLUGIN_SIDECAR` 指向本地新构建二进制（旧副本会伪报 sessions/list 未注册等 FAIL，非回归）。

## 1. 任务 1：命令历史（命令弹窗）

`frontend/src/lib/commandHistory.ts`（纯函数）+ App.vue 接线：

- **内存环形**：`pushCommandHistory`（去重置顶、上限 `COMMAND_HISTORY_LIMIT=100`）；`ssh/exec` 成功提交即入历史（不论退出码）。
- **localStorage 非敏感持久化**：`isPersistableCommand` 过滤疑似内嵌凭据（`password|passwd|passphrase|token|secret|api-key|access-key : /=`）、超长（>200 字符）与多行命令，只有合法条目落盘；读回经 `sanitizeCommandHistory`（类型过滤/去重/截断）。
- **↑↓ 浏览**：`browseCommandHistory`（index=-1 为非浏览态；向上到顶停住，向下越过最新一条回到回退草稿；进入浏览态前备份当前草稿）。命令输入框 `@keydown.up/.down.prevent` 接入。
- **一键重发**：历史条目点击 → `rerunHistoryCommand` 回填输入框并立即执行（复用既有 `ssh/exec` 路径，服务端 shell 单引号转义不变）。
- 弹窗内“最近命令”区带清空按钮（`clearCommandHistory` 同步落盘）。

单测：workbench.spec「command history ring buffer」4 例（环形去重上限 / ↑↓ 浏览与草稿恢复 / 持久化过滤 / 读回消毒）。

## 2. 任务 2：快速命令栏（工具栏下拉）

`frontend/src/lib/quickCommands.ts`（纯函数 CRUD）+ App.vue 工具栏 Zap 下拉：

- localStorage CRUD，上限 20 条（`QUICK_COMMANDS_LIMIT`），名称 ≤60 / 命令 ≤500 字符；`normalizeQuickCommands`（读回消毒：丢非法、去重 id、超限截断）、`upsertQuickCommand`（原位更新/追加/挤掉最旧）、`removeQuickCommand`。
- 内联编辑器：名称+命令草稿、添加/保存/取消、`{count} / {limit}` 计数；条目行发送/编辑/删除三操作。
- **发送语义选择：PTY 写入而非 ssh/exec**。理由（已注释进代码）：快速命令是对齐 tiny-rdm 的“在当前交互 shell 中执行”的片段——输出需回显在终端、`cd`/`export` 等状态需留在当前 shell；`ssh/exec` 是独立非交互通道，不回显也不共享 shell 状态，不符合语义。命令原文按键盘输入写入（`sendTerminalBytes(text + "\r")`，用户可见可中断），**不经过任何 shell 拼接转义**（无注入面；既有 `shell_quote` 红线路径仅针对 `ssh/exec` 服务端构造，保持不变）。
- zmodem 传输中 / 命令执行中禁止发送；发送后回焦终端。

单测：「quick commands CRUD」3 例（读回规范化 / upsert 三语义 / 按 id 删除）。

## 3. 任务 3：连接信息面板（工具栏 Info 弹出）

App.vue `connection-info-popover`（只读摘要）：Host / Port / User / **Auth method（本轮新增）** / 只读标志 / Latency（echo 往返计时）。敏感字段（密码、私钥路径、passphrase、TOTP）一律不展示。

- **延迟只读计时**：`measureLatency` 复用既有 `ssh/exec` 跑 `echo dbx-rtt-probe`，计时整个 RPC 往返（含通道建立），校验探测回显；无新后端方法。展示走 `formatLatency`（`<1 ms` / 整 ms / 1s 以上一位小数 / 无值 `–`）。
- **认证方式（本轮新增交付）**：`ssh/sessions/list` 每行新增只读 `authMethod` 字段——
  - 后端 `backend/src/model.rs`：`AuthenticationMethod::method_name()`（枚举 → 规范名 `password`/`private-key`/`private-key-password`/`agent`/`none`，仅方法名，无凭据材料）；
  - `backend/src/ssh.rs`：`session_info_payload` 增加 `authMethod` 参数并注入 payload；`list_sessions` 按会话的 connection_id 从连接注册表查名（连接缺失回退 `password`，store poisoned 降级同理）；无 secrets 泄漏断言扩展（payload 序列化不含 `password`/`passphrase`）；
  - 前端：打开面板时拉取 `ssh/sessions/list` 定位当前会话行，`formatAuthMethodLabel`（connectionInfo.ts 纯函数：已知方法名走七语 i18n，未知值原样展示，空缺显示占位符）。

契约变化：`ssh/sessions/list` 返回行新增 `authMethod`（camelCase，向后兼容的纯增量），`docs/PROTOCOL.zh-CN.md` 已同步；smoke_batch3 `sessions/list` 用例扩展字段与取值域断言。

单测：cargo `auth_method_names_round_trip_for_display`（五个方法名往返）、`session_info_payload_carries_identity_and_liveness`（扩展 authMethod + 无泄漏）；vitest `formatAuthMethodLabel` 2 例（已知名本地化 / 未知原样 + 占位符）。

## 4. 任务 4：终端字体缩放

`frontend/src/lib/terminalZoom.ts`：`clampFontSize`（8–32 钳制）。App.vue：

- 工具栏 **A+ / A− 快捷按钮**（`adjustTerminalZoom(±1)`），步进 1px、区间 [8,32]（原基准 0.8x–2.0x 需求换算为绝对字号钳制，对宿主 13px 基准约 0.6x–2.5x，取可用性边界）。
- Ctrl/⌘+滚轮缩放、Ctrl/⌘+0 复位宿主基准字号（`resetTerminalZoom`）。
- **持久化**：`localStorage` 存绝对字号（读回经钳制），宿主 appearance 下发的 fontSize 仍作复位基准；变化后 500ms 防抖 toast 提示（`terminalZoom.fontSize` 七语）。

单测：「clamps terminal font zoom between 8 and 32」。

## 5. mock 走查截图（docs/screenshots-a-ssh/）

vite serve `frontend/mock.html`（mockDbxHost 视觉夹具，含 OSC 633 shell 集成模拟），Playwright 真浏览器走查：

| 文件 | 内容 |
| --- | --- |
| `01-toolbar-dark.png` | 深色主题工具栏全景（A−/A+、Zap、Info 等入口） |
| `02-quick-commands.png` | 快速命令下拉：条目行 + 内联编辑器 + 计数 |
| `03-command-history.png` | 命令弹窗：历史区 ↑↓ 重发入口 |
| `04-command-rerun.png` | 一键重发执行中的输出/退出码 |
| `05-connection-info.png` | 连接信息面板：Host/Port/User/**Auth method: Private key**/Latency |
| `06-font-zoom.png` | A+ 连按两次后的终端字号放大效果 |

mock 夹具同步补 `ssh/sessions/list`（返回 `authMethod: "private-key"`）支撑 05 的走查。

## 6. 七语确认

本轮新增/涉及 key 全部 ×7（en / zh-CN / zh-TW / es / it / ja / pt-BR）：
`connectionInfoAuth`、`authMethodPassword`、`authMethodPrivateKey`、`authMethodPrivateKeyPassword`、`authMethodAgent`、`authMethodNone`
（历轮同批：`terminalFontIncrease/Decrease`、`commandHistoryTitle/Resend/Clear`、`quickCommands*`、`connectionInfo*`、`terminalZoom.fontSize`）。
既有「七语 key 全对齐 + 占位符对齐」vitest 断言全量扫过 messages 表，51/51 通过即为自动化证据。

## 7. 安全红线自查

- **命令历史**：疑似内嵌凭据/超长/多行命令只留内存、不落 localStorage；`session_info_payload` 序列化不含凭据字段（单测断言）。
- **shell 执行路径**：命令历史重发走既有 `ssh/exec`（服务端 shell 单引号转义不变）；快速命令走 PTY 键盘写入原文，无字符串拼接进 shell 的面；连接信息延迟探测为固定只读 `echo`。
- **认证方式**：仅输出方法名枚举，密码/passphrase/密钥路径/TOTP 不出现在任何新 payload；私有方法 `method_name()` 为纯枚举映射（单测覆盖）。
- 无新依赖；新定时器（无）与既有清理路径不变；wb-*/CSS 变量双主题沿用既有 token（新面板复用 `popover`/`connection-info-grid` 既有样式类）。

## 8. 遗留与交接

1. `docs/FEATURE_PARITY*`、`TEST_MATRIX` 按约束未动（收口轮刷新：cargo 108→109、vitest 41→51、sessions/list authMethod 字段、本文件基线）。
2. 连接信息面板的认证方式反映 sidecar 连接注册表中的配置方法名；若宿主未来在 ConnectionSummary 中直接下发认证方式，可省去 `ssh/sessions/list` 往返（当前实现已按可选降级处理：拉取失败显示占位符）。
3. 字号缩放区间为绝对字号 [8,32] 而非 0.8x–2.0x 相对倍率（可用性优先，宿主基准字号可变，绝对钳制更可预测）；如需严格倍率语义可改为对基准字号乘算。
4. 截图 `06` 捕捉到缩放生效但未含 toast 文案（500ms 防抖竞态）；toast 文案已由单测+七语断言覆盖。
5. 全部改动未 git 提交（硬性约束），请主会话审阅 diff 后统一收口。

## 9. 2026-09-04 追加：宿主 1.1 theme 通道主题同步

宿主 `dev/plugin-framework-current`（cd3ee5a45，2026-09-03）向沙箱推送
`PluginBridgeTheme { appearance, tokens }`：init 携带 + env 消息实时推送，tokens
为宿主根节点解析后的 `--color-*` 设计令牌（明暗切换与自定义调色板均实时下发）；
宿主侧 `pluginAppearance` 契约尚未接线，`api.appearance`/`onAppearanceChange`
在真实宿主恒缺失。本任务让工作台跟随宿主主题实时切换：

- `env.d.ts`：新增 `DbxPluginTheme` 与 `DbxPluginApi.theme?`。
- `lib/hostTheme.ts`（新）：token→colors 映射（`--color-*` → appearance 契约
  字段）、`dbx-plugin-env` CustomEvent 订阅、输入校验（畸形输入降级不崩）。
- `lib/appearance.ts`：`resolveAppearance` 入参放宽为逐字段可选
  `DbxPluginAppearanceInput`（theme 通道只带颜色令牌；终端字体回退本地规范值）。
- `App.vue`：init 时 `api.appearance` 缺失改用 `api.theme` 初始化；appearance
  订阅不可用时订阅 env 主题推送（两套契约不同时挂，宿主未来同发也不互相覆盖）；
  退订随 `onBeforeUnmount` 清理。
- `mockDbxHost.ts`：按宿主形状镜像 `theme`（colors 反查 `--color-*` 令牌），
  浏览器 `mock.html?theme=light|dark` 可走查两条路径。

**验证**：`pnpm typecheck` 绿；vitest 112 全绿（新增 hostTheme.spec 7 例：token
映射、env detail 解析、缺字段回退、畸形输入）。sidecar 未改动。

**遗留**：宿主接线 `pluginAppearance`（terminal fontFamily/fontSize 下发）前，
缩放复位基准仍为本地 13px 规范值；届时插件侧无需再改（applyAppearance 已按
可选降级兼容完整 appearance）。本次无新增用户可见文案，七语无增量。
