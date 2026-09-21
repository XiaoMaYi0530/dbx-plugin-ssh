# 宿主层增强方案：本地终端全局入口 + 底部悬浮终端

> 面向 DBX 宿主（`btroot/dbx`）的增强需求与实施设计。插件侧已完成的能力见
> `docs/FEATURE_PARITY.zh-CN.md` 本地终端条目；本文只谈插件 webview 之外、
> 需要宿主配合的部分。所有宿主事实均标注 `file:line`（基于调研时宿主代码）。

## 0. 现状事实（方案地基）

| 事实 | 出处 |
| --- | --- |
| 宿主贡献点仅 5 类：`ConnectionProvider` / `Workbench` / `FilesystemProvider` / `ContextMenu` / `ResultView`，**无全局面板/入口类型** | `crates/dbx-plugin-runtime/src/plugins/manifest.rs:227` |
| `Workbench` 贡献只有 `id/label/description/icon` 四个字段 | 同上 `:638` |
| **插件中心已可无连接打开任意 workbench**：`PluginContributionsPanel.openWorkbench` → `queryStore.openPluginWorkbench`，`connectionId` 可选 | `apps/desktop/src/components/plugins/PluginContributionsPanel.vue:477`、`:1121` |
| 插件可编程开 tab：宿主桥 `host.openWorkbench(pluginId, contributionId, context?, {forceNew})`，context 任意 JSON | `apps/desktop/src/lib/plugins/pluginHostBridge.ts:246` |
| tab 复用规则：同 `pluginId+contributionId+connectionId` 复用既有 tab，**不重载 webview**；关闭 tab 时宿主发 `workbench/close`（插件已用它回收本地会话） | `apps/desktop/src/stores/queryStore.ts:3484` |
| 底部分屏有先例：SQL 编辑器"编辑器+结果面板"垂直分屏，拖拽用 `panelResizeState` | `apps/desktop/src/components/layout/SqlEditorWorkspace.vue:12` |
| 布局为 AppSidebar（左）+ AppTabBar/EditorGroup（主区），**无全局底部 dock / 状态栏** | `apps/desktop/src/components/layout/` |
| 崩溃恢复（boot 重放 tab 元数据 + 重连）的测试**只覆盖带 connectionId 的 tab**，无连接 tab 未定义 | `apps/desktop/src/stores/__tests__/queryStore.reconnectRestoredPluginTabs.spec.ts` |
| manifest 校验 `deny_unknown_fields`：workbench 贡献加新字段会被**老宿主拒装** | `manifest.rs`（serde 属性） |

## 1. 目标与非目标

**目标**
1. 本地终端的全局入口：不依赖任何 SSH 连接，一键可达、可发现。
2. 底部悬浮终端：任意页面（连接列表、SQL 编辑器、数据浏览）可呼出/收起的终端面板，Quake/VS Code 面板式交互（热键、可拖高、keep-alive）。

**非目标**：面板内多终端 tab 管理（沿用宿主 tab 体系）、移动/web 版、宿主自研 PTY（插件 sidecar 已有全链能力，禁止重复实现）。

## 2. 分层方案：P0 零宿主改动 → P1 宿主小改 → P2 底部悬浮面板

### P0：先把入口立起来（纯插件，零宿主改动）

两条既有通路直接可用：

1. **插件中心入口（已存在）**：`io.dbx.ssh.workbench` 出现在插件中心"工作台"区，无需连接即可打开。插件侧只需让工作台 webview 识别 `context.localTerminal === true` 时跳过 SSH 连接引导、直接进入本地终端模式。
2. **自查自开**：SSH 工作台内已有"本地终端"按钮；扩展为——无连接时经 `host.openWorkbench("io.dbx.ssh", "io.dbx.ssh.workbench", { localTerminal: true, workbenchId: <新 uuid> }, { forceNew: true })` 直接开一个纯本地终端 tab，从任意已打开的 SSH 工作台一键再开。

插件侧改动：App.vue 启动时读 `context.localTerminal`，置 `skipConnectFlow` 并自动调用 `startLocalTerminal()`；工作量约 0.5 天。

### P1：全局入口规范化（宿主小改，约 1 天）

AppSidebar 底部新增"插件工作台"区：读取前端插件 registry 中**未被任何 connection-provider 引用的** workbench 贡献（或全部 workbench 贡献，可配置），逐条渲染图标+名，点击 = `openPluginWorkbench(pluginId, contributionId)`（无 context）。

- 数据面零新协议：registry 已有全部信息；i18n 七语；设置项"在侧栏显示插件工作台"（默认开）。
- 这是通用设施（对所有插件生效），不为本插件开特例。

### P2：底部悬浮终端（宿主主特性，宿主 3~5 天 + 插件 1 天）

**核心决策：新增贡献点类型 vs 通用底部容器。** 推荐**宿主内置通用底部面板容器 + workbench 贡献加可选 `surface` 字段**，理由：

- 不新增 manifest 贡献类型 → installer/manifest 校验/安装链零改动；
- 容器是通用设施（日志面板、监控面板等未来可复用），符合"宿主做容器、插件做内容"的分层；
- `surface` 是可选字段，但 manifest `deny_unknown_fields` 意味着**老宿主会拒装**带此字段的包 → 必须随 `engines.host_api >= 1.2.0` 一同发布（宿主先发新版本，插件在 manifest 里声明 `>=1.2.0`；或由 integrator 决定宿主对未知字段的宽容化改造，二选一，倾向前者）。

#### 2.1 宿主侧

1. **manifest**（`manifest.rs:638`）：`PluginWorkbenchContribution` 增加可选 `surface?: "tab" | "panel"`，缺省 `"tab"`，行为完全向后兼容。
2. **BottomDock.vue**（`components/layout/`）：
   - 主窗口底部覆盖层（overlay 悬浮，盖住内容，Quake 式；v2 可加"挤压主区"的 dock 模式）；
   - 高度可拖（复用 `panelResizeState`），可整栏收起；
   - 固定触发钮：窗口右下角终端图标 + 活动指示点（有输出/命令运行时呼吸）；
   - 热键 `Ctrl/⌘+J`（VS Code 同款）呼出/收起；收起后焦点归还主区；
   - 内容复用 `PluginWorkbenchHost.vue`（webview 宿主组件原样嵌入，桥协议零新增——桥是 per-webview 的）；
   - 状态持久化（开/关、高度、上次的 contributionId）入 settingsStore。
3. **会话语义**：panel webview 与 tab webview 是两个实例（两个 workbenchId、两个独立本地会话）；隐藏 = keep-alive（会话保活，这正是悬浮终端的价值）；应用退出或显式卸载 → `workbench/close`。设置项"闲置 N 分钟自动收起并卸载"（卸载即终止会话，UI 明示）。
4. **快捷键冲突排查**：`Ctrl/⌘+J` 与现有键位表核对（实施前置项）。

#### 2.2 插件侧（约 1 天）

- `startLocalTerminal` 等 UI 在 `context.surface === "panel"` 时精简：隐藏 SFTP 侧栏按钮、大弹窗改精简排版（面板高度有限）；
- 其余能力（shell 选择器、注入、最近命令、命令标记、退出覆盖层）原样可用；
- 七语文案无新增（复用现有键）。

### 3. 里程碑

| 里程碑 | 内容 | 归属 | 工作量 |
| --- | --- | --- | --- |
| M0 | P0：context.localTerminal 直通 + 自查自开 | 插件 | 0.5d |
| M1 | P1：侧栏全局工作台区 | 宿主 | 1d |
| M2 | P2：surface 字段 + BottomDock + 插件 panel 适配 | 宿主+插件 | 4~6d |
| M3 | Windows 实测（ConPTY、热键、拖拽）随宿主发布 | 双方 | 1d |

发布顺序约束：P2 必须宿主先行（新 host_api 版本），插件 manifest 随后抬 `engines.host_api`。

### 4. 风险

- **R1 无连接 tab 的崩溃恢复语义未定义**：boot 恢复测试只覆盖带 connectionId 的 tab。期望行为 = 恢复 tab 元数据，webview 显示既有 `restartDisconnected`/本地会话退出态（插件已处理该态）；需宿主在 P0 落地时确认无连接 tab 不被 boot 恢复逻辑丢弃或误重连。
- **R2 manifest 兼容**：`surface` 字段在老宿主拒装 → 用 `engines.host_api >= 1.2.0` 版本门槛解决；integrator 定夺具体版本号。
- **R3 panel 常驻内存**：每 webview 数十 MB；mitigation 见 2.1.3 闲置卸载设置。
- **R4 热键冲突**：`Ctrl/⌘+J` 需与宿主键位表核对。
- **R5 评审归属**：宿主改动走宿主仓评审流程；本文档即宿主侧需求输入，插件 agent 不直接改宿主仓。

### 5. 验收清单

- [ ] P0：无连接打开工作台 → 直接进入本地终端；关闭 tab → 本地会话被回收（`workbench/close`）。
- [ ] P1：侧栏可见所有独立 workbench 贡献；点击打开；设置项可关。
- [ ] P2：任意页面 `Ctrl/⌘+J` 呼出/收起；面板内本地终端全功能可用（shell 选择/注入/标记/最近命令）；收起后会话保活（收起期间输出仍在累积，重新展开可见）；拖拽高度持久化；应用退出后面板会话终止。
- [ ] 回归：原 SSH workbench tab 行为不变；老宿主 + 新插件包（无 surface 字段时）行为不变。
