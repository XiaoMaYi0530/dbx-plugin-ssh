# DBX 插件 UI 贡献点体系与宿主交互规范（设计提案 v1）

> **一句话结论**：宿主 UI 贡献点从 **5 类扩展到 11 类（另有 2 个可选小件）**，
> 交互原语从 **2 个扩展到 6 个**；但对标 VS Code 的关键不在数量，而在三个机制
> ——**command 原子、声明式摆放、两层容错 manifest**。发布即冻结的部分与
> 必须在发布前钉死的规则见 §6 兼容宪法。
>
> **文档状态**：提案（待宿主仓评审排期）。本文合并并取代
> `HOST_TERMINAL_SURFACE.zh-CN.md` 与 `HOST_UI_CONTRIBUTIONS.zh-CN.md`；
> git 历史保留两文的演进过程。首个消费者（本地终端）的宿主需求在 §7。
>
> **评审归属**：宿主改动走宿主仓（`btroot/dbx`）评审流程；插件 agent 不直接
> 改宿主仓，本文即宿主侧需求与设计输入。

---

## 1. 背景与问题

### 1.1 现状事实（方案地基，均标注宿主代码锚点）

| 事实 | 出处 |
| --- | --- |
| 宿主贡献点仅 5 类：`ConnectionProvider` / `Workbench` / `FilesystemProvider` / `ContextMenu` / `ResultView`，**无全局面板/入口类型** | `crates/dbx-plugin-runtime/src/plugins/manifest.rs:227` |
| 贡献点为内部标签枚举（`tag="type"`，无 `#[serde(other)]`），各贡献结构体 `deny_unknown_fields`——**未知 `type` 值与已知类型内的未知字段/枚举值都会让整个清单反序列化失败 → 拒装** | 同上 `:226`、`:111/:120/:129` |
| `Workbench` 贡献只有 `id/label/description/icon` 四个字段 | 同上 `:638` |
| 插件中心已可无连接打开任意 workbench：`PluginContributionsPanel.openWorkbench` → `queryStore.openPluginWorkbench`，`connectionId` 可选 | `apps/desktop/src/components/plugins/PluginContributionsPanel.vue:477`、`:1121` |
| 插件可编程开 tab：宿主桥 `host.openWorkbench(pluginId, contributionId, context?, {forceNew})`，context 任意 JSON | `apps/desktop/src/lib/plugins/pluginHostBridge.ts:246` |
| tab 复用规则：同 `pluginId+contributionId+connectionId` 复用既有 tab，不重载 webview；关闭 tab 时宿主发 `workbench/close` | `apps/desktop/src/stores/queryStore.ts:3484` |
| 底部分屏有先例：SQL 编辑器垂直分屏，拖拽用 `panelResizeState` | `apps/desktop/src/components/layout/SqlEditorWorkspace.vue:12` |
| 布局为 AppSidebar（左）+ AppTabBar/EditorGroup（主区），**无全局底部 dock / 状态栏** | `apps/desktop/src/components/layout/` |
| 工具栏显隐是固定 14 键布尔表，持久化在 editorSettings | `apps/desktop/src/stores/settingsStore.ts:921` |
| 开关网格与全局设置搜索共用 `TOOLBAR_VISIBILITY_ITEMS`（文件头注释明确"新增开关不得缺席搜索"） | `apps/desktop/src/lib/settings/settingsSearch.ts:62` |
| AppToolbar 以 `{value, label, icon(Lucide 组件), action}` 组装条目，溢出收进 More 菜单 | `apps/desktop/src/components/layout/AppToolbar.vue:443,512` |
| 插件图标解析已有公共件：`resolvePluginIcon(pluginId, contributionId?) → URL` | `apps/desktop/src/lib/plugins/pluginIconResolver.ts:18` |
| 崩溃恢复测试只覆盖带 connectionId 的 tab，无连接 tab 未定义 | `apps/desktop/src/stores/__tests__/queryStore.reconnectRestoredPluginTabs.spec.ts` |

### 1.2 问题

1. **插件 UI 被锁死在 webview 内**：工具栏/侧栏/状态栏/底部面板等宿主 chrome，插件无法触达——本地终端的"全局入口"诉求撞的正是这堵墙。
2. **现有 5 类没有形成体系**：兄弟插件（kafka/ldap/files）实际只用 connection-provider + workbench 两类；context-menu/result-view 无人使用；能力增长只能靠"宿主加特例"。
3. **manifest 严格校验锁死演进**：每加一个贡献点/枚举值都要宿主发版 + 全体插件抬 `engines` 门槛。

---

## 2. VS Code 机制的四条可迁移原则

| 原则 | VS Code 事实 | 对 DBX 的含义 |
| --- | --- | --- |
| **一切皆命令** | 所有可触发行为统一为 `contributes.commands`；菜单/键位/命令面板/状态栏只是命令的"摆放位置"（`menus` + `when` 子句） | 引入一等 command 概念，收编分散的 `connection-provider.actions`、context-menu 动作、openWorkbench 直调 |
| **表面与内容分离** | 宿主渲染一切 chrome，扩展只声明"放哪、何时出现、点了跑什么"；扩展自主 UI 只存在于 webview 容器内 | 永久红线：插件永不渲染自己 webview 之外的东西 |
| **声明式 + 容错解析** | `contributes` 的未知键被忽略（向前兼容），扩展可以领先宿主发布 | manifest 改为两层容错（§5.1），这是解锁一切演进的元决策 |
| **懒激活** | 声明 ≠ 运行，`activationEvents` 按需启动 | DBX 的 sidecar 模型天然支持：贡献点是纯数据，不 spawn sidecar；首次激活才拉起 |

---

## 3. UI 贡献点：5 → 11（+2 可选）

### 3.1 现有 5 类的处置

| 现有 | 处置 | 说明 |
| --- | --- | --- |
| `connection-provider` | 保留 | 域特有（连接生命周期/表单/capabilities），VS Code 无对应物，是 DBX 的领域优势 |
| `workbench` | 保留 + 扩展 | 加可选 `surface: "tab" \| "panel"`（封闭枚举，见 §6.1）；panel 形态复用通用容器（§7.4） |
| `filesystem-provider` | 保留 | 域特有（SFTP 挂载） |
| `context-menu` | **泛化为 `menus`** | 现类型绑死连接上下文；泛化为"位置词表 + when 子句 + command 引用"（§3.3） |
| `result-view` | 保留 | SQL 结果渲染器，随 command 体系一并激活 |

### 3.2 新增 6 类（+2 可选）

| # | 贡献点 | VS Code 对应物 | 解决什么 | 批次 |
| --- | --- | --- | --- | --- |
| 6 | `command` | `contributes.commands` | **原子**：`{ id, title(七语), icon, when, dispatch }`。v1 语义固定为"打开 workbench 带 context"；v2 增加派发型（转发 sidecar 执行，见 §6.2 安全闸门） | A |
| 7 | `menus` | `contributes.menus` | command 的摆放位置词表（§3.4），每项 `{ command, when }` | A |
| 8 | `toolbar-item` | 近似 Activity Bar 入口 | 全局工具栏图标 = command 摆放，用户在 设置→外观 逐项开关（完整设计见 §7.3） | A |
| 9 | `status-bar-item` | `StatusBarItem` | 常驻小指示：`{ text/icon/tooltip/command/when }`（连接状态、录制中、终端活动） | B |
| 10 | `sidebar-view`（+ 通用 view-container） | `viewsContainers` + `views` | 通用侧栏树视图：Kafka topic 树、LDAP 目录、SFTP 书签从各自 workweb 拆出，标准化为宿主容器 + 插件 webview（桥协议零新增） | B |
| 11 | `object-viewer` | `customEditors` | 按"资源类型"打开插件渲染器：Redis key 可视化、Kafka message、JSON 列预览 | B |
| 可选 | `viewsWelcome` | `viewsWelcome` | 插件视图空态文案 | C |
| 可选 | `badge` | ActivityBar badge | 视图容器角标计数（传输中/待审批） | C |
| — | `keybinding` | `contributes.keybindings` | **折叠进 command**（声明默认键位，用户在设置改），不独立成类型 | A |

**类型新增判据（比数量更重要）**：只有**渲染契约不同**才允许新增贡献类型；只是**数据不同**一律扩展现有类型的可选字段。判据钉死后，"11 类"是当前推导结果而非目标（VS Code 40+ 类型的维护负担正是没守住这条判据的代价）。A/B 批实施时按判据复审：`status-bar-item` 与 `toolbar-item` 渲染契约差异最小，应先试合并为 command + 摆放。

### 3.3 `menus` 位置词表（v1）

`commandPalette` / `connectionContext`（现 context-menu 的泛化）/ `objectExplorer` / `dataGrid` / `editorTitle` / `tabContext`。

⚠️ 位置名是**永久契约**（§6.1）：将被写进成千上万份 manifest，新增安全、改名/删除破坏。发布前按宿主**现存全部表面**盘点一遍（连接侧栏/对象树/数据网格/SQL 编辑器/tab 栏/终端面板），宁可一次定全。

### 3.4 `when` 子句

- v1 语法钉死为 `==`、`!=`、`&&` 三种——**无正则、无 `in`、无自定义函数**；任何语法扩展 = `host_api` major。
- 上下文键最小集：`connection.state`（connected/disconnected/none）、`object.type`（table/db/topic/…）、`surface`（tab/panel）、`readOnly`。
- 宿主渲染时求值；词表扩充走宿主版本，不开放插件自定义表达式（避免 DSL 失控）。

---

## 4. 交互原语：2 → 6（宿主代渲染）

| # | 原语 | VS Code 对应 | 形态 |
| --- | --- | --- | --- |
| 1 | **command 派发** | commands.executeCommand | 命令面板 / menus / toolbar / 键位全部落到同一派发；v1 打开 workbench，v2 派发插件后端 |
| 2 | **notification + actions** | showInformationMessage | 桥 `notify(pluginId, { level, text, actions[], when })`，点击回传 action id |
| 3 | **quick-pick** | showQuickPick | 桥 `quickPick(pluginId, { items[], placeholder })` → 选中项回传 |
| 4 | **input** | showInputBox | 单行文本起步；表单复用 connection-provider 既有 fields 渲染器 |
| 5 | **activate / open-with** | openWorkbench / customEditor.open | 已有 openWorkbench；补 object-viewer 的 openWith(resource) |
| 6 | **event 订阅** | *Event<T>* | 已有 appearance/locale/context；补 `connection-state`（连接增删/断连）与 `active-object`（当前选中库/表/对象）——与 when 词表共享词汇 |

模态原语（2/3/4）的完整契约见 §6.3。

---

## 5. 关键架构决策

### 5.1 manifest 两层容错（元决策）

现状两层都会拒装（§1.1 第 2 行）。容错必须同时落在两层，缺一不可：

- **类型层**：未知贡献 `type` → 跳过该条 + 插件中心黄标；
- **值层**：已知类型内的未知字段、未知枚举值（如未来 `surface:"floating"` 落到只认 `"tab"|"panel"` 的老宿主）→ 同样跳过该条贡献并警告，**绝不整包拒装**。

实现建议：反序列化到 `serde_json::Value` 逐条判定（先认 `type`，再按已知结构体 strict 解析，失败即降级记录），而非依赖 enum 级 `deny_unknown_fields` 的当前行为。这是解锁后续一切演进的开关——否则每加一个贡献点/枚举值都要宿主发版 + 全体插件抬 `engines`。

### 5.2 其余决策

1. **command 是唯一原子**：工具栏/菜单/面板/键位全部是 command 的摆放，禁止"某表面私有动作"的平行机制（`connection-provider.actions` 给一个迁移窗口：映射为 `connectionContext` 位置的 command）。
2. **command 全限定命名空间**：宿主解析时强制 `${pluginId}.${commandId}`（插件声明短 id，宿主拼全）；宿主保留顶层命名空间 `workbench.*` / `app.*`——防插件伪造宿主命令混入命令面板（VS Code 早期真实踩过）。menus 位置名同理是宿主保留词表。
3. **图标与 i18n 走既有公共件**：`resolvePluginIcon` + `PluginContributionLocalization`（§7.3 已验证可行）。
4. **显隐治理**：宿主代渲染的每一项都必须进入 设置→外观 的开关网格与设置搜索，避免工具栏被插件塞爆时用户无防御。

---

## 6. 兼容宪法（发布即冻结）

> 本章回答：**哪些东西一旦发布就再也改不动**，以及必须**在发布前**钉死的规则。

### 6.1 永久契约清单（只能加，不能改名/删除/改语义）

| 冻结物 | 规约 |
| --- | --- |
| 贡献类型名（`toolbar-item`…） | kebab-case；语义只可收窄描述不可改变行为；删除 = 破坏，弃用走 §6.4 |
| menus 位置词表 | §3.3；发布前全表面盘点 |
| command 全限定 ID | `${pluginId}.${id}`；id 建议蛇形；宿主保留 `workbench.*`/`app.*` |
| when 上下文键 + 语法 | §3.4；语法超集必须解析报错而非宽容 |
| 枚举值（`surface:"tab"\|"panel"` 等） | 封闭枚举，扩值走宿主版本 + §5.1 值层容错；插件不得依赖"未知值回退 tab"——跳过整条贡献才是契约 |
| `context` 透传键 | 宿主注入键保留清单：`connectionId`/`workbenchId`/`connection`/`restored`/`workbenchState`；插件自定义键**应当**自带前缀（如 `ssh.*`），宿主合并时永不改写插件键 |
| 桥 API（`window.dbxPlugin.*`、模态原语签名） | 与 manifest 同等是公共 API：major 内只加不改；`showQuickPick` 返回 `null` = 用户取消等语义写进类型定义 |

### 6.2 command v2 派发是最大的单向门（安全前置）

v1 的 command 是**纯数据**（打开 workbench + context），零执行面。v2 若允许派发到插件 sidecar，**所有已发布 command 一夜之间变成 RPC 入口**——安全等级跳变，事后不可逆。因此：

- v1 命令声明带 `dispatch: "data" | "rpc"`（缺省 data）；只有显式 rpc 的命令可派发——旧清单自动免疫；
- rpc 命令继承 sidecar 侧破坏性操作闸门（confirmDestructive/审计台账，SSH 插件已有先例），宿主至少提供统一"执行确认"可选弹层；
- 命令面板/菜单对 rpc 命令标注来源插件（防钓鱼：`ssh.重启生产机` 不能长得像宿主自带）。

### 6.3 模态原语契约（notify/quickPick/input）

- **单飞**：每插件同时至多一个模态；新请求顶替旧请求（旧的按取消结算）；
- **取消语义**：用户 Esc/关窗 → resolve `null`（不是 reject、不是挂死）；
- **超时**：宿主侧默认 30s 超时结算为 `null`（防 sidecar 死后模态永悬）；
- **排队**：跨插件的模态按到达顺序排队展示，不叠放；
- web/docker 形态下必须**可用或明确禁用**（禁用时返回 `null` 并附原因，不能挂死）。

### 6.4 弃用与迁移政策

- 任一类型/位置/枚举值弃用：宿主**保留兼容翻译 N+2 个 minor 版本**（如旧 `context-menu` 声明 → 宿主内部翻译为 `menus` 摆放），期间插件中心对使用方黄标；
- 每类迁移必须先证明**语义无损**（旧 context-menu 的连接绑定 ⊆ 新位置词表），有损则不迁移、保留原类型共存；
- `connection-provider.actions` → commands 同政策，给存量插件一个版本的迁移窗口。

### 6.5 一致性测试（把宪法变成 CI 门）

1. **golden manifests**：每类贡献一份合法样例 + 期望宿主行为（渲染位置/开关项/设置搜索命中）；
2. **容错矩阵**：未知类型、已知类型未知字段、未知枚举值 → 均为"跳过+警告"，整包安装成功；
3. **升级矩阵**：老 manifest × 新宿主（逐版本样例回放）、新 manifest × 模拟老宿主（验证 `engines` 门槛文案而非崩溃）；
4. **行为回归**：模态单飞/超时、command 全限定拼接、when 语法拒绝超集（解析器必须报错而非宽容）。

### 6.6 形态降级矩阵（非桌面宿主）

每个宿主代渲染表面在 web/docker 形态的行为必须显式定义（显示/隐藏/降级）：toolbar/sidebar/status-bar 在 web 形态的存废、panel dock 在无窗环境的替代（回到 tab）、模态原语同 §6.3。降级行为写进各表面实现 PR 的验收项。

---

## 7. 首个消费者：本地终端的宿主需求

本地终端（插件侧已完成，见 `FEATURE_PARITY.zh-CN.md` 本地终端条目）暴露的缺口是本体系的第一个实例。分层方案：P0 零宿主改动 → P1 侧栏入口 → P2 底部悬浮面板。

### 7.1 P0：无连接直通（✅ 插件侧已落地，commit 1824c04）

- `context.localTerminal` 直通：宿主以 `{ localTerminal: true }` 打开本 workbench（插件中心无连接打开 / 桥调用）时，跳过 SSH 连接流程直接进入本地终端；无连接时 identity 显示"本地终端"。
- 自查自开：工作台内经 `host.openWorkbench(io.dbx.ssh.workbench, { localTerminal: true, workbenchId: uuid }, { forceNew: true })` 一键开独立本地终端 tab；无此桥的旧宿主隐藏入口。
- 关闭 tab → 宿主 `workbench/close` → 插件回收本地会话。
- mock 夹具 `?local=1` 覆盖直通路径；浏览器已验证。

### 7.2 P1：侧栏全局工作台区（宿主 ~1 天）

AppSidebar 底部新增"插件工作台"区：读取前端插件 registry 中的 workbench 贡献，逐条渲染图标+名，点击 = `openPluginWorkbench(pluginId, contributionId)`（无 context）。

- 数据面零新协议（registry 已有全部信息）；i18n 七语；设置项"在侧栏显示插件工作台"（默认开）；
- 通用设施（对所有插件生效），不为本插件开特例。

### 7.3 M1.5：工具栏插件入口（宿主 2~3 天 + 插件 0.5 天）

把插件入口挂进**既有**的主工具栏显隐体系（设置 → 外观 → 工具栏开关网格），对用户零新概念。

**manifest（v1 = toolbar-item 贡献）**

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

- v1 行为固定为 `打开 workbench`：`workbench` 必须引用**同插件**已声明的 workbench 贡献，`context` 原样透传（与 §7.1 直通闭环）；v2 预留 event 型（镜像 `contextMenu/<id>` 模式派发 sidecar）。
- label 七语复用 `PluginContributionLocalization`；icon 为插件包内资产，宿主经 `resolvePluginIcon` 解析。

**宿主 desktop 侧**

1. `settingsStore`：不动内置 `ToolbarItems`，新增独立动态记录 `pluginToolbarItems: Record<"plugin:<pluginId>/<itemId>", boolean>`——首次出现默认 `true`；加载时清理已卸载插件的陈旧键。
2. 设置 → 外观：开关网格在内置项后动态追加插件条目（开关 + 插件图标 + 七语 label）；设置搜索同步追加动态定义（`createToolbarVisibilitySettingsSearchDefinitions` 已接受 items 参数，天然可扩展——缺席搜索是该文件头注释的红线）。
3. `AppToolbar`：内置项之后按 registry 追加动态项；`items.icon` 扩为 `Component | 图片URL` 联合并适配渲染；点击 = `openPluginWorkbench(pluginId, workbench, { context }, { forceNew: false })`（复用既有 tab，重复点击不重载）；溢出收 More 菜单对动态项自动生效（纯 DOM 测量）。

**已否决备选**：`ToolbarItems` 加索引签名混入插件键（持久化形状 churn）；复用 `ContextMenu` 贡献（语义不符）；v1 即做 event 派发型（openWorkbench 已覆盖诉求）。

### 7.4 P2：底部悬浮终端（宿主 3~5 天 + 插件 1 天）

**核心决策**：宿主内置通用底部面板容器 + workbench 贡献加可选 `surface` 字段（`"tab"` 缺省 / `"panel"`）。不新增 manifest 贡献类型 → installer/安装链零改动；容器是通用设施（日志/监控面板未来可复用），符合"宿主做容器、插件做内容"分层；`surface` 依赖 §5.1 值层容错发布。

**宿主侧（BottomDock.vue）**

1. 主窗口底部覆盖层（Quake 式悬浮；v2 可加"挤压主区" dock 模式）；
2. 高度可拖（复用 `panelResizeState`），可整栏收起；
3. 固定触发钮：窗口右下角终端图标 + 活动指示点（有输出/命令运行时呼吸）；
4. 热键 `Ctrl/⌘+J`（VS Code 同款）呼出/收起；收起后焦点归还主区；实施前与宿主键位表核对；
5. 内容复用 `PluginWorkbenchHost.vue`（webview 宿主组件原样嵌入，桥协议零新增——桥是 per-webview 的）；
6. 状态持久化（开/关、高度、上次 contributionId）入 settingsStore。

**会话语义**：panel webview 与 tab webview 是两个实例（两个 workbenchId、两个独立本地会话）；隐藏 = keep-alive（会话保活，正是悬浮终端的价值）；应用退出或显式卸载 → `workbench/close`。设置项"闲置 N 分钟自动收起并卸载"（卸载即终止会话，UI 明示）。

**插件侧**：`context.surface === "panel"` 时精简 UI（隐藏 SFTP 侧栏按钮、弹窗改精简排版）；其余能力（shell 选择器、注入、最近命令、命令标记、退出覆盖层）原样可用。

---

## 8. 里程碑与工作量

| 里程碑 | 内容 | 归属 | 状态 |
| --- | --- | --- | --- |
| M0 | P0：context.localTerminal 直通 + 自查自开 | 插件 | ✅ 已落地（1824c04） |
| 前置 | manifest 两层容错（§5.1）+ conformance 套件骨架 | 宿主 ~1 天 | 建议最先 |
| A 批 | command + menus（context-menu 泛化）+ toolbar-item + keybinding 折叠 | 宿主 2~3 天 + 插件 0.5 天 | 待排期 |
| M1 | P1：侧栏全局工作台区 | 宿主 1 天 | 待排期 |
| M1.5 | 工具栏插件入口（§7.3） | 宿主 2~3 天 + 插件 0.5 天 | 待排期 |
| B 批 | status-bar-item / sidebar-view / object-viewer | 宿主，按插件需求拉 | 待定 |
| C 批 | viewsWelcome / badge | 宿主顺手 | 待定 |
| P2 | surface 字段 + BottomDock（§7.4） | 宿主 3~5 天 + 插件 1 天 | 待排期 |
| M3 | Windows 实测（ConPTY、热键、拖拽）随宿主发布 | 双方 | 1 天 |

发布顺序约束：P2 的 `surface` 枚举依赖值层容错先行（或宿主先行发版 + 插件抬 `engines`，二选一，倾向前者）。

---

## 9. 风险登记册

| # | 风险 | 缓解 |
| --- | --- | --- |
| R1 | 无连接 tab 的崩溃恢复语义未定义（boot 恢复测试只覆盖带 connectionId 的 tab） | 宿主在 P0 落地时确认无连接 tab 不被 boot 恢复丢弃或误重连；期望行为 = 恢复 tab 元数据，webview 显示既有 restartDisconnected/本地会话退出态（插件已处理该态） |
| R2 | manifest 兼容：新贡献类型/枚举值在老宿主的行为 | §5.1 两层容错（首选）；否则宿主先行发版 + 插件抬 `engines`（integrator 定版本号） |
| R3 | panel 常驻内存（每 webview 数十 MB） | §7.4 闲置自动收起并卸载设置 |
| R4 | 热键 `Ctrl/⌘+J` 冲突 | 实施前与宿主键位表核对；命令面板快捷键（Ctrl/⌘+K 或 +P）同查 |
| R5 | 评审归属 | 宿主改动走宿主仓流程；本文即需求输入 |
| R6 | toolbar 图标渲染适配（`items.icon` 联合类型） | 改动集中一个渲染分支 |
| R7 | 动态开关缺席设置搜索 | settingsSearch 红线，§7.3 已并入设计 |
| R8 | toolbar-item label 本地化回退链 | 实施时验证 `PluginContributionLocalization` 覆盖；最简回退 manifest 内联 `label_i18n` |
| R-menus | context-menu → menus 迁移破坏存量 | §6.4：N+2 兼容翻译 + 语义无损才迁 |
| R-sidebar | sidebar-view 树交互需要最小 webview 桥协议（懒加载/右键） | 比 status-bar 复杂，排期单独评估 |
| R-compat | 宿主不采纳容错解析 | 每个新贡献点叠加"宿主发版 + 插件抬门槛"联动成本 |
| R-v2rpc | command v2 派发把存量命令变成 RPC 入口 | §6.2：v1 首发 `dispatch` 判别字段 + 审批/审计闸门前置 |

---

## 10. 验收清单

**P0（已达成）**
- [x] 无连接打开工作台 → 直接进入本地终端；关闭 tab → 本地会话被回收（`workbench/close`）；mock `?local=1` 覆盖。

**容错与 conformance（前置）**
- [ ] 未知类型 / 已知类型未知字段 / 未知枚举值 → 跳过+黄标，整包安装成功（三层用例入 CI）。
- [ ] golden manifests + 升降级回放矩阵进宿主 CI 门禁。

**A 批 / M1 / M1.5**
- [ ] 命令面板列出插件 command（全限定 ID、来源标注）；`when` 语法超集解析报错。
- [ ] 侧栏可见所有独立 workbench 贡献；点击打开；设置项可关。
- [ ] SSH 插件安装后：外观设置出现"本地终端"开关（带插件图标，默认开，设置搜索可搜到）；工具栏图标点击 → 无连接本地终端 tab；再次点击复用既有 tab 不重载；开关关闭后图标（含 More 菜单）消失，重启保持；卸载后无陈旧键。
- [ ] 老宿主 + 新插件（含新贡献）：跳过+黄标或按 `engines` 门槛拒绝，不崩。

**P2**
- [ ] 任意页面 `Ctrl/⌘+J` 呼出/收起；面板内本地终端全功能（shell 选择/注入/标记/最近命令）；收起后会话保活（输出仍累积，重新展开可见）；拖拽高度持久化；应用退出后面板会话终止。

**回归**
- [ ] 原 SSH workbench tab 行为不变；老插件（无新贡献）在新宿主行为不变。
