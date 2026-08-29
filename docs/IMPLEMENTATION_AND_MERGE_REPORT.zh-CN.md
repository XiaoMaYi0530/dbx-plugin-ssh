# SSH/SFTP 插件 0.2.1 代码修改与合并报告

日期：2026-08-10  
Host 分支：`codex/plugin-host-ssh-enablers`  
插件分支：`codex/plugin-poc`  
旧版基线：`origin/feature/ssh-sftp-workbench`

## 本轮问题与处理结果

| 问题 | 原因 | 处理 |
| --- | --- | --- |
| 工作台语言不跟随 DBX | 插件在 Host 初始化前把 SDK 默认 `en` 缓存在无响应依赖的 `computed` 中 | Host 增加 `onLocaleChange` 实时事件；插件在 `ready` 后读取语言并使用响应式 `ref`；无法读取或不支持的语言回退 `zh-CN` |
| 字体未完整跟随 | Host 仅提供编辑器字体与字号，插件普通 UI 使用固定系统字体 | `appearance.ui.fontFamily` 接入 DBX 界面字体；`appearance.terminal` 继续驱动 xterm 和代码预览字体/字号 |
| iframe 滚动条和全局外观不一致 | 沙箱不继承父页面 CSS，且插件固定声明 `color-scheme: dark` | 插件根据 Host 外观设置 `color-scheme`，补充明暗主题通用细滚动条、界面字体和终端字体 CSS 变量 |
| 工具栏、边距、图标与旧版不同 | 独立插件 UI 使用了原型样式和 checkbox | 按旧分支恢复 36px 工具栏、连接色背景/色条、彩色操作图标、Switch、SFTP 表格密度；终端内边距按要求改为左侧 10px |
| 断开占位图缺失 | 独立插件未携带 DBX 的 `DatabaseIcon` 组件 | 以内联 SVG 复用旧分支 `public/icons/database/ssh.svg`，恢复断开/重连占位图 |
| 切换超过三个标签后重新登录 | DBX 全局 `KeepAlive` 上限为 4，插件页被 LRU 淘汰；会话状态写入存在 150ms 延迟 | 缓存上限提高到 16；SSH 建连/附着成功后立即等待 `workbenchState` 持久化，淘汰后也按 `sessionId` 附着而非新建 SSH 连接 |
| 闲置后出现 `Last login` 和系统未激活文字 | 这是新 SSH Shell 的服务器登录横幅，不是插件错误；缓存淘汰导致了新的登录 | 修复标签缓存和会话附着后不再因切页创建新 Shell；服务器真正断线时仍明确要求手动重连 |
| 插件显示渐变 P | 使用了 CLI 模板默认图标 | Manifest 与连接提供者继续引用 `assets/plugin.svg`，内容替换为旧分支 SSH SVG；插件中心自动使用新图标。连接树若 Host 当前不显示贡献点图标，不额外修改其行为 |
| SSH 隧道配置看似存在但插件未使用 | DBX 已把传输层解析后的 `runtime.host/port` 注入生命周期，Sidecar 却只读取原始 `connection.host/port` | Sidecar 使用运行时端点建立 TCP 连接，但以原始目标地址完成主机密钥校验和 `known_hosts` 身份记录 |

## DBX 全局设置接入机制

- 明暗主题由 `PluginWorkbenchHost.currentAppearance()` 读取 DBX 根节点允许列表内的颜色令牌和 `useTheme().isDark`，通过 `window.dbxPlugin.appearance` 初始化，并通过 `onAppearanceChange` 实时更新。
- 工作台界面字体读取 `settingsStore.editorSettings.uiFontFamily`。
- xterm 与只读代码预览读取 `settingsStore.editorSettings.fontFamily/fontSize`。
- 语言读取 `vue-i18n` 的当前 `appLocale`。语言切换只发送 Host Bridge 消息，不重载 iframe，也不会断开 SSH 会话。
- Manifest 中的插件名称、表单字段和贡献点翻译仍由 DBX 插件注册表按 `localizations` 解析；插件工作台内部文案由插件自己的七国语言表解析。

## SSH 隧道可用性确认

DBX 公共传输层会先解析 SSH 隧道，再调用插件的 `connection/test`、`connection/connect` 或工作台生命周期，并在参数中同时提供：

- `connection.host/port`：最终 SSH 目标的逻辑身份，用于展示与主机密钥验证；
- `runtime.host/port`：DBX 隧道建立后的实际 TCP 入口，通常为本机或运行 DBX Server 的回环地址和临时端口。

0.2.1 已按上述语义分离连接端点与安全身份，因此配置表单中的 DBX 公共 SSH 隧道对本插件具备真实代码链路。已增加单元测试保证运行时端点不会污染目标主机的 `known_hosts` 身份。尚需用两个独立 SSH 主机做一次跳板机实机验收；当前单台 `192.168.1.64` 只能覆盖目标 SSH/SFTP 链路，不能完整证明跨主机转发。

## 代码归属与 PR 拆分

### 应提交到 `t8y2/dbx` 的 Host/SDK 改动

- Manifest Schema：条件字段、`path`、`multiple-workbenches`、Host API 1.1 权限与能力。
- 插件工作台 Host Bridge：稳定 `workbenchId`、64 KiB 状态、关闭生命周期、语言/外观/字体、剪贴板、文件句柄和拖放。
- 连接生命周期：`operationId` 挑战、插件数据目录、运行时传输层端点。
- DBX UI：插件连接表单、连接右键多工作台入口、插件图标解析、标签状态持久化和缓存上限。
- Rust Host/安装器/运行时、Tauri 文件系统权限、开发文档和回归测试。

这些改动位于 `E:\xynanan\dbx-plugin-host-worktree`，目标分支为 `codex/plugin-host-ssh-enablers`。

### 由独立插件仓库维护的改动

- `manifest.json`、七国语言元数据和 SSH SVG。
- 独立 Vue/Vite 工作台、xterm、CodeMirror、旧版等价样式与交互。
- Rust Sidecar 的认证、主机密钥、PTY、SFTP、目录跟随、ZMODEM、传输和会话隔离。
- 插件测试、打包工作流、框架反馈和本报告。

这些改动位于 `E:\xynanan\dbx-ssh-sftp-plugin`，目标分支为 `codex/plugin-poc`。

## 验证记录

- 插件前端：TypeScript 检查通过；Vitest 4/4 通过；生产构建通过。
- 插件 Sidecar：Rust 测试 12/12 通过。
- Host：Vue/TypeScript 全量检查通过；Bridge、条件表单和连接挑战定向测试 31/31 通过（其中 Plugin Host Bridge 14/14）。
- Host：Windows debug 桌面完整构建通过，新二进制已从独立目标目录启动且窗口响应正常。
- Windows x64 候选包生成成功，大小 3,609,522 字节，SHA-256：`a343c1e9a3c93a3ba22d2f445ed06b1e8fdd206c8d43ed4ebbf83bc597e11724`。
- `192.168.1.64:22` 实机烟测通过：包安装、UI 读取、生命周期、主机指纹、终端命令、SFTP 目录、4 MiB/空文件上传下载、取消、变化密钥拒绝、断开。
- 0.2.1 已写入验证数据目录和默认 DBX 数据目录，激活记录均为 `0.2.0 → 0.2.1`；默认数据目录已使用新 Host 启动，不需要再次导入插件包。

## 建议合并顺序

1. 将 Host 分支重放到插件框架最新目标分支，拆成可审查的 Host API/Schema、桌面桥接、Core 生命周期与测试文档提交。
2. Host API 1.1 合并后，插件固定最低 DBX/Host API 版本并重建五平台候选包。
3. 独立插件仓库合并 `codex/plugin-poc`，完成 Windows 与 Docker/Web 发布门槛测试。
4. 最后提交商店元数据；签名、发布和上游 PR 均需单独确认。

本轮没有推送远端、发布包或创建上游 PR。
