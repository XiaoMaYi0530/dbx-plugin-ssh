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

1. **manifest 两层容错（元决策，源码核实）**：现状是 `PluginContribution` 为内部标签枚举（`manifest.rs:226`，无 `#[serde(other)]`）、各贡献结构体 `deny_unknown_fields`——**未知 `type` 值与已知类型内的未知字段/枚举值都会让整个清单反序列化失败 → 拒装**。因此容错必须同时落在两层，缺一不可：
   - **类型层**：未知贡献 `type` → 跳过该条 + 插件中心黄标；
   - **值层**：已知类型内的未知字段、未知枚举值（如未来 `surface:"floating"` 落到只认 `"tab"|"panel"` 的老宿主）→ 同样跳过该条贡献并警告，**绝不整包拒装**。
   实现建议：反序列化到 `serde_json::Value` 逐条判定（先认 `type`，再按已知结构体 strict 解析，失败即降级记录），而非依赖 enum 级 `deny_unknown_fields` 的当前行为。这是解锁后续一切演进的开关——否则每加一个贡献点/枚举值都要宿主发版 + 全体插件抬 `engines`。
2. **command 是唯一原子**：工具栏/菜单/面板/键位全部是 command 的摆放，禁止再出现"某表面私有动作"的平行机制（`connection-provider.actions` 给出一个迁移窗口：映射为 `connectionContext` 位置的 command）。
3. **when 词汇表最小集**：`connection.state`（connected/disconnected/none）、`object.type`（table/db/topic/…）、`surface`（tab/panel）、`readOnly`。由宿主在渲染时求值；词汇表扩充走宿主版本，不开放插件自定义表达式（避免 DSL 失控）。
4. **图标与 i18n 走既有公共件**：`resolvePluginIcon` + `PluginContributionLocalization`（toolbar-item 提案 §6.2 已验证可行）。
5. **显隐治理**：宿主代渲染的每一项都必须进入 设置→外观 的开关网格与设置搜索（M1.5 已确立该红线），避免工具栏被插件塞爆时用户无防御。
6. **类型新增判据（防类型数膨胀）**：只有**渲染契约不同**才允许新增贡献类型；只是**数据不同**一律扩展现有类型的可选字段。判据钉死后，"11 类"只是当前推导结果而非目标——VS Code 40+ 类型的维护负担正是没守住这条判据的代价。
7. **command 全限定命名空间**：宿主解析时强制 `${pluginId}.${commandId}`（插件声明短 id，宿主拼全），宿主保留顶层命名空间（`workbench.*` / `app.*`）——防插件伪造宿主命令混入命令面板（VS Code 早期真实踩过）。同理 menus 位置名是宿主保留词表。

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

## 7. 兼容宪法（发布即冻结——本轮 review 增补）

> 本章回答一个更冷的问题：**这套设计里哪些东西一旦发布就再也改不动**，以及由此必须**在发布前**钉死的规则。每条都标注了冻结物与规约。

### 7.1 永久契约清单（只能加，不能改名/删除/改语义）

| 冻结物 | 规约 |
| --- | --- |
| 贡献类型名（`toolbar-item`…） | kebab-case；语义只可**收窄描述**不可改变行为；删除 = 破坏，弃用走 §7.4 |
| menus 位置词表（`commandPalette`/`connectionContext`…） | 将被写进成千上万份 manifest；新增安全、改名/删除破坏。发布前按"宿主现存全部表面"盘点一遍（连接侧栏/对象树/数据网格/SQL 编辑器/tab 栏/终端面板），宁可一次定全 |
| command 全限定 ID | `${pluginId}.${id}`；id 建议蛇形；宿主保留 `workbench.*`/`app.*` 顶层 |
| when 上下文键 + 语法 | 键：`connection.state`/`object.type`/`surface`/`readOnly`；语法 v1 钉死为 `==`、`!=`、`&&` 三种（**无正则、无 `in`、无自定义函数**）；任何语法扩展 = `host_api` major |
| 枚举值（`surface:"tab"|"panel"` 等） | 封闭枚举，扩值走宿主版本 + §4.1 值层容错；插件不得依赖"未知值回退到 tab"——跳过整条贡献才是契约 |
| `context` 透传键 | 宿主注入键保留清单：`connectionId`/`workbenchId`/`connection`/`restored`/`workbenchState`；插件自定义键**应当**自带前缀（如 `ssh.*`），宿主合并时永不改写插件键——今天双向都不遵守，发布前立规成本最低 |
| 桥 API（`window.dbxPlugin.*`、模态原语签名） | 与 manifest 同等是公共 API：major 内只加不改；`showQuickPick` 返回 `null`=用户取消 这类语义写进类型定义 |

### 7.2 command 的 v2 派发是最大的单向门（安全前置）

v1 的 command 是**纯数据**（打开 workbench + context），零执行面。v2 若让它可派发到插件 sidecar，**所有已发布 command 一夜之间变成 RPC 入口**——这是安全等级的跳变，事后不可逆。因此 v1 设计时必须预留、v2 发布前必须落地：

- 命令声明带 `dispatch: "data" | "rpc"`（缺省 data）；只有显式声明 rpc 的命令可派发——旧清单自动免疫；
- rpc 命令继承 sidecar 侧破坏性操作闸门（confirmDestructive/审计台账，SSH 插件已有先例），宿主至少提供统一"执行确认"可选弹层；
- 命令面板/菜单对 rpc 命令标注来源插件（防钓鱼：`ssh.重启生产机` 不能长得像宿主自带）。

### 7.3 模态原语契约（notify/quickPick/input）

- **单飞**：每插件同时至多一个模态；新请求顶替旧请求（旧的按取消结算）；
- **取消语义**：用户 Esc/关窗 → resolve `null`（不是 reject、不是挂死）；
- **超时**：宿主侧默认 30s 超时结算为 `null`（防 sidecar 死后模态永悬）；
- **排队**：跨插件的模态按到达顺序排队展示，不叠放；
- web/docker 宿主形态下模态原语必须**可用或明确禁用**（禁用时 API 返回 `null` 并附原因，不能挂死）。

### 7.4 弃用与迁移政策

- 任一贡献类型/位置/枚举值弃用：宿主**保留兼容翻译 N+2 个 minor 版本**（如旧 `context-menu` 声明 → 宿主内部翻译为 `menus` 摆放），期间插件中心对使用方黄标；
- 每类迁移必须先证明**语义无损**（旧 context-menu 的连接绑定语义 ⊆ 新位置词表），有损则不迁移、保留原类型共存；
- `connection-provider.actions` → commands 的迁移同政策（映射为 `connectionContext` 位置 command），给存量插件一个版本的迁移窗口。

### 7.5 一致性测试（把宪法变成可执行的门）

宿主仓新增 conformance 套件，CI 门禁：

1. **golden manifests**：每类贡献一份合法样例 + 期望宿主行为（渲染位置/开关项/设置搜索命中）；
2. **容错矩阵**：未知类型、已知类型未知字段、未知枚举值 → 均为"跳过+警告"，整包安装成功；
3. **升级矩阵**：老 manifest × 新宿主（逐版本样例回放）、新 manifest × 模拟老宿主（验证 `engines` 门槛文案而非崩溃）；
4. **行为回归**：模态单飞/超时、command 全限定拼接、when 语法拒绝正则（解析器对超集语法**必须报错**而非宽容）。

### 7.6 形态降级矩阵（非桌面宿主）

每个宿主代渲染表面在 web/docker 形态的行为必须显式定义（显示/隐藏/降级），不允许"碰巧能用"：toolbar/sidebar/status-bar 在 web 形态的存废、panel dock 在无窗环境的替代（回到 tab）、模态原语同 §7.3。降级行为写进各表面实现 PR 的验收项。

### 7.7 本轮 review 对既有结论的修正

- "11 类"降级为**判据的推导结果**（§4.6）：发布前应再审一遍 6 个新类型是否都有独立渲染契约（status-bar 与 toolbar 的渲染契约差异最小，若仅数据差异应合并为 command+摆放）；
- §4.1 的容错从"一个开关"升级为**两层规则**（源码核实：现状两层都会拒装）；
- `surface` 从"可选字段"升级为**封闭枚举 + 值层容错依赖**——它是最先吃到值层容错红利的字段，也是 P2 的前置依赖。
