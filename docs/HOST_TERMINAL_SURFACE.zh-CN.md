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
| M1.5 | 工具栏插件入口（§6：设置→外观可开关，icon+事件声明式注册） | 宿主 2~3d + 插件 0.5d | — |
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

## 6. 工具栏插件入口（设置 → 外观 → 工具栏开关）

P1 的替代/补充形态：把插件入口挂进**既有**的主工具栏显隐体系——插件以
"icon + 事件声明"注册工具栏项，用户在 设置→外观 的工具栏开关网格里逐项
控制显隐，点击图标直接打开插件工作台。对用户零新概念（复用已习惯的开关
位置），对插件零运行时 UI（宿主代为渲染）。

### 6.1 现状锚点

| 事实 | 出处 |
| --- | --- |
| 工具栏显隐是固定 14 键布尔表（`dataTransfer`/`pluginCenter`/`ai`/`github`…），持久化在 editorSettings | `apps/desktop/src/stores/settingsStore.ts:921` |
| 开关网格与全局设置搜索共用 `TOOLBAR_VISIBILITY_ITEMS`（文件头注释明确"新增开关不得缺席搜索"） | `apps/desktop/src/lib/settings/settingsSearch.ts:62` |
| AppToolbar 以 `{value, label, icon(Lucide 组件), action}` 组装条目，溢出自动收进 More 菜单；`pluginCenter` 是"收进 More"的既有样例 | `apps/desktop/src/components/layout/AppToolbar.vue:443,512` |
| 插件图标解析已有公共件：`resolvePluginIcon(pluginId, contributionId?) → URL`（读插件包内资产） | `apps/desktop/src/lib/plugins/pluginIconResolver.ts:18` |

### 6.2 设计

**manifest（宿主 runtime 新贡献类型）**

```json
{
  "type": "toolbar-item",
  "id": "local-terminal",
  "label": "本地终端",
  "label_i18n": { "en": "Local terminal", "...": "..." },
  "icon": "assets/toolbar-local-terminal.svg",
  "workbench": "io.dbx.ssh.workbench",
  "context": { "localTerminal": true }
}
```

- v1 行为固定为 `打开 workbench`（用户诉求"直接打开插件"）：`workbench` 必须引用**同插件**已声明的 workbench 贡献，`context` 原样透传 webview（与 P0 的 `localTerminal` 直通配套，一键即得无连接本地终端）。
- label 七语复用 manifest 既有 `PluginContributionLocalization` 机制；`icon` 为插件包内资产路径，宿主经 `resolvePluginIcon` 解析。
- v2 预留：`event` 型（省略 `workbench`，改为把 `toolbarItem/<id>` 请求派发给插件后端，完全镜像 `contextMenu/<id>` 的既有模式），本期不实现。

**宿主 runtime**（`manifest.rs`）：`PluginContribution` 增 `ToolbarItem` 变体 + `id()` 分支 + 校验（id 全局去重、workbench 引用存在且同源、icon 资产存在于包内）；前端 registry 暴露合并后的工具栏项清单（按插件安装序 + 声明序）。

**desktop 侧**

1. `settingsStore`：**不动内置 `ToolbarItems`**（避免持久化形状 churn），新增独立动态记录 `pluginToolbarItems: Record<"plugin:<pluginId>/<itemId>", boolean>`——首次出现默认 `true`；加载时清理"插件已卸载"的陈旧键。
2. 设置→外观：开关网格在内置项后动态追加插件条目（开关 + `PluginIcon` 图标 + 七语 label）；设置搜索同步追加动态定义（`createToolbarVisibilitySettingsSearchDefinitions` 已接受 items 参数，天然可扩展）。
3. `AppToolbar`：内置项之后按 registry 追加动态项；`items.icon` 目前是 Lucide 组件类型，需扩为 `Component | 图片URL` 联合并适配渲染；点击 = `openPluginWorkbench(pluginId, workbench, { context }, { forceNew: false })`——复用既有 tab（重复点击不重载，与 §2 tab 复用规则一致）；溢出收 More 菜单的逻辑对动态项自动生效（纯 DOM 测量）。

**SSH 插件侧（0.5d）**：manifest 声明上述 toolbar-item（label 七语、icon、`workbench+context.localTerminal`）→ 工具栏一键开无连接本地终端；与 P0/M0 的 context 直通构成闭环。

### 6.3 版本与兼容

- 新贡献类型 = manifest schema 变更，**老宿主拒装**含该贡献的包 → 与 §2 P2 同策略：宿主先行发版，插件随后抬 `engines.dbx` 下限。
- 老插件完全不受影响（未声明 toolbar-item 则工具栏无动态项、无新开关）。
- 插件卸载：动态工具栏项与对应开关随 registry 移除，陈旧布尔键在下次加载时清理。

### 6.4 备选方案对比（已否决）

| 备选 | 否决理由 |
| --- | --- |
| 给 `ToolbarItems` 加索引签名混入插件键 | 持久化形状 churn、未知键归一化语义混乱、内置键与插件键生命周期不同 |
| 复用 `ContextMenu` 贡献 | 该类型绑定已存连接的侧栏菜单上下文，语义不符 |
| v1 即做 event 派发型 | 用户诉求是"直接打开插件"，openWorkbench 已覆盖；event 型留 v2 按需加 |

### 6.5 风险

- **R6 图标渲染适配**：`items.icon` 需扩联合类型（Lucide 组件 | 插件图标 URL），改动集中在一个渲染分支。
- **R7 动态开关缺席设置搜索**：settingsSearch 注释红线，动态定义必须并入搜索（见 6.2.2）。
- **R8 label 本地化**：`PluginContributionLocalization` 是否已覆盖 toolbar-item 类型的回退链需在实施时验证；最简回退为 manifest 内联 `label_i18n`。
- 其余同 §4（R1 无连接 tab 恢复语义、R5 评审归属）。

### 6.6 验收清单

- [ ] SSH 插件安装后：外观设置出现"本地终端"开关（带插件图标），默认开；设置搜索可搜到。
- [ ] 工具栏图标点击 → 无连接打开本地终端 tab；再次点击复用既有 tab 不重载。
- [ ] 开关关闭后图标从工具栏（含 More 菜单）消失，重启后状态保持。
- [ ] 卸载插件：工具栏项与开关消失，无陈旧键。
- [ ] 老宿主 + 新插件（含 toolbar-item）：按版本门槛拒绝或忽略，不崩。
