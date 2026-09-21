# 宿主插件 UI 贡献点与交互类型体系（对标 VS Code 扩展机制）

> 回答一个问题：**宿主需要多少个 UI 贡献点和交互类型，才能支撑后续所有插件的能力。**
> 结论先行：UI 贡献点 **5 → 11（另有 2 个可选小件）**，交互类型 **2 → 6**；
> 但对标 VS Code 的关键不是数量，而是三个机制——**command 原子、声明式摆放、
> 容错 manifest**。落地分三批，A 批仅宿主 2~3 天即解锁大部分能力。
>
> 本文是顶层设计；本地终端的具体宿主需求（全局入口/底部 dock）是本文的
> 一个实例，见 `docs/HOST_TERMINAL_SURFACE.zh-CN.md`。宿主事实沿用该文档
> 的调研锚点（manifest.rs:227 五类贡献点、PluginContributionsPanel 无连接
> 打开、pluginHostBridge.ts:246 openWorkbench、settingsStore 工具栏开关表）。

## 1. VS Code 机制的本质（四条可迁移原则）

| 原则 | VS Code 事实 | 对 DBX 的含义 |
| --- | --- | --- |
| **一切皆命令** | 所有可触发行为统一为 `contributes.commands`；菜单/键位/命令面板/状态栏只是命令的"摆放位置"（`menus` 贡献点 + `when` 子句） | DBX 应引入一等 **command** 概念，收编现在分散的 `connection-provider.actions`、context-menu 动作、openWorkbench 直调 |
| **表面与内容分离** | 宿主渲染一切 chrome（视图容器/状态栏/菜单/面板），扩展只声明"放哪、何时出现、点了跑什么"；扩展自主 UI 只存在于 webview 容器内 | 红线：插件永不渲染自己 webview 之外的东西——这正是本地终端全局入口需要宿主的根因，也应是永久架构约束 |
| **声明式 + 容错解析** | `contributes` 的未知键被忽略（向前兼容），扩展可以领先宿主发布 | DBX manifest `deny_unknown_fields`（老宿主拒装新字段）是所有演进的总闸——应改为"未知贡献类型跳过 + 插件中心警告" |
| **懒激活** | 声明 ≠ 运行，`activationEvents` 按需启动 sidecar/webview | 插件多起来后，贡献点声明零成本（不 spawn sidecar），首次激活才拉起——DBX 的 sidecar 模型天然支持（贡献点是纯数据） |

## 2. UI 贡献点：5 → 11（+2 可选）

### 现有 5 类的去留

| 现有 | 处置 | 说明 |
| --- | --- | --- |
| `connection-provider` | 保留 | 域特有（连接生命周期/表单/capabilities），VS Code 没有对应物，是 DBX 的领域优势 |
| `workbench` | 保留 + 扩展 | 加可选 `surface: "tab" \| "panel"`（已提案 P2/底部 dock）；panel 形态复用通用容器 |
| `filesystem-provider` | 保留 | 域特有（SFTP 挂载） |
| `context-menu` | **泛化为 `menus`** | 现类型绑死连接上下文；泛化为"位置词汇表 + when 子句 + command 引用"，见 §3 |
| `result-view` | 保留 | SQL 结果渲染器（尚未被插件使用，随 command 体系一并激活） |

### 新增 6 类（+2 可选）

| # | 贡献点 | VS Code 对应物 | 解决什么 | 批次 |
| --- | --- | --- | --- | --- |
| 6 | `command` | `contributes.commands` | **原子**：`{ id, title(七语), icon, when }`。v1 语义固定为"打开 workbench 带 context"（覆盖本地终端等入口诉求）；v2 增加派发型（转发 sidecar/插件后端执行） | A |
| 7 | `menus` | `contributes.menus` | command 的摆放位置词汇表：`commandPalette` / `connectionContext`（现 context-menu 的泛化）/ `objectExplorer` / `dataGrid` / `editorTitle` / `tabContext`，每项 `{ command, when }` | A |
| 8 | `toolbar-item` | （无直接对应，近似 Activity Bar 入口） | 全局工具栏图标 = command 摆放，用户在 设置→外观 逐项开关——已提案（M1.5，§6 完整设计） | A |
| 9 | `status-bar-item` | `StatusBarItem` | 常驻小指示：`{ text/icon/tooltip/command/when }`（连接状态、录制中、本地终端活动）——插件请求最频繁的 VS Code 表面之一 | B |
| 10 | `sidebar-view`（+ 通用 view-container） | `viewsContainers` + `views` | 通用左/右栏插件树视图：Kafka topic 树、LDAP 目录、SFTP 书签这类"常驻对象树"从各自 workbench webview 中拆出，标准化为宿主容器 + 插件 webview（桥协议零新增） | B |
| 11 | `object-viewer` | `customEditors` | 按"资源类型"打开插件渲染器：Redis key 可视化、Kafka message、JSON 列预览——宿主数据网格/对象树遇到该类型时 openWith 插件 | B |
| 可选 | `viewsWelcome` | `viewsWelcome` | 插件视图空态文案 | C |
| 可选 | `badge` | ActivityBar badge | 视图容器角标计数（传输中/待审批） | C |
| — | `keybinding` | `contributes.keybindings` | 折叠进 command（声明默认键位，用户在设置改），不独立成类型 | A（随 command） |

**数量答案**：**11 类**（5 保留/泛化 + 6 新增），可选 2 件按需。A 批（command + menus + toolbar-item，含 keybinding 折叠）宿主 2~3 天，解锁"任意插件零宿主改动获得全局入口"这一最大共性诉求；B 批按插件拉需求逐个上；C 批顺手。

## 3. 交互类型：2 → 6

现有交互只有两种形态：`openWorkbench`（带 context 开 webview）与宿主→插件事件（appearance/locale/context）。补齐为 6 个**宿主代渲染**的通用原语——插件零 UI 代码获得标准交互：

| # | 交互原语 | VS Code 对应 | 形态 |
| --- | --- | --- | --- |
| 1 | **command 派发** | commands.executeCommand | 命令面板 / menus / toolbar / 键位全部落到同一派发；v1 打开 workbench，v2 派发插件后端 |
| 2 | **notification + actions** | window.showInformationMessage | 桥 `notify(pluginId, { level, text, actions[], when })`，用户点击回传指定 action id |
| 3 | **quick-pick** | window.showQuickPick | 桥 `quickPick(pluginId, { items[], placeholder })` → 选中项回传 |
| 4 | **input** | window.showInputBox / createWebviewPanel 的表单 | 单行文本起步；表单复用 connection-provider 既有 fields 渲染器 |
| 5 | **activate / open-with** | openWorkbench / customEditor.open | 已有 openWorkbench；补 object-viewer 的 openWith(resource) |
| 6 | **event 订阅** | *Event<T>* | 已有 appearance/locale/context；补 `connection-state`（连接增删/断连）与 `active-object`（当前选中库/表/对象）——when 子句与事件共享同一词汇表 |

## 4. 关键架构决策（比数量更重要）

1. **manifest 容错化（元决策）**：宿主把"未知贡献类型拒装"改为"跳过 + 插件中心黄标警告"。这是解锁后续一切演进的开关——否则每加一个贡献点都要宿主发版 + 全体插件抬 `engines`。`deny_unknown_fields` 对**已知类型内部字段**仍可保留严格校验。若坚持严格路线，则每个新贡献点都必须"宿主先行发版"，成本见 R2。
2. **command 是唯一原子**：工具栏/菜单/面板/键位全部是 command 的摆放，禁止再出现"某表面私有动作"的平行机制（`connection-provider.actions` 给出一个迁移窗口：映射为 `connectionContext` 位置的 command）。
3. **when 词汇表最小集**：`connection.state`（connected/disconnected/none）、`object.type`（table/db/topic/…）、`surface`（tab/panel）、`readOnly`。由宿主在渲染时求值；词汇表扩充走宿主版本，不开放插件自定义表达式（避免 DSL 失控）。
4. **图标与 i18n 走既有公共件**：`resolvePluginIcon` + `PluginContributionLocalization`（toolbar-item 提案 §6.2 已验证可行）。
5. **显隐治理**：宿主代渲染的每一项都必须进入 设置→外观 的开关网格与设置搜索（M1.5 已确立该红线），避免工具栏被插件塞爆时用户无防御。

## 5. 与既有提案的关系及里程碑

| 里程碑 | 内容 | 归属 | 状态 |
| --- | --- | --- | --- |
| M0 | context.localTerminal 直通 + 自查自开（§2 P0） | 插件 | ✅ 已落地（1824c04） |
| A 批 | command + menus（context-menu 泛化）+ toolbar-item + keybinding 折叠 | 宿主 2~3 天 + 插件 0.5 天 | 待宿主排期 |
| B 批 | status-bar-item / sidebar-view / object-viewer | 宿主，按插件需求拉 | 待定 |
| C 批 | viewsWelcome / badge | 宿主顺手 | 待定 |
| P2 | workbench `surface:"panel"` + BottomDock | 宿主 3~5 天 + 插件 1 天 | 待宿主排期 |

## 6. 风险

- **R-menus**：`context-menu` 泛化为 `menus` 需要迁移策略——旧类型保留一个版本（宿主把旧声明翻译成新位置），避免破坏已发布插件。
- **R-palette**：命令面板需要宿主有全局快捷键入口（Ctrl/⌘+K 或 +P），实施前核对新键位表（同 R4）。
- **R-sidebar**：`sidebar-view` 的树交互（懒加载子节点、右键菜单）需要一个最小 webview 桥协议（tree/item 事件），比状态栏复杂——排期时单独评估。
- **R-compat**：若宿主不采纳容错解析，则 A/B 批每个新贡献点都叠加一次"宿主发版 + 插件抬门槛"的联动成本。
- 其余沿用 HOST_TERMINAL_SURFACE §4/§6.5（无连接 tab 恢复语义、评审归属：宿主改动走宿主仓流程）。
