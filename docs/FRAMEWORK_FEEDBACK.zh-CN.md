# DBX Host API 1.1 / SSH-SFTP 0.2.1 验证记录

验证分支：Host `codex/plugin-host-ssh-enablers`；插件 `codex/plugin-poc`；基线为
`origin/feature/ssh-sftp-workbench` 的 4 个 SSH 专属提交。所有文件均仅保存在本地，未推送、未发布、未创建 PR。

## 已实现

- Manifest 表单支持 `visible_when`、`required_when`、`path` 与 `multiple-workbenches`，隐藏字段不参与前后端必填校验。
- 每个插件标签获得稳定 `workbenchId`、`restored` 和不超过 64 KiB 的 `workbenchState`；切换标签可附着原会话，真正关闭标签才调用 `workbench/close`。
- `window.dbxPlugin.appearance`、实时外观变更、`host.clipboard` 已接入。
- `host.fileTransfer` 使用工作台专属不可猜句柄、256 KiB 分块、流式目标 16 GiB 上限和最多 32 个句柄。桌面端使用系统对话框，并通过 Host 原生文件命令写入同目录临时文件后原子替换；Web 优先使用 OPFS 流式暂存，无 OPFS 时限制为 1 GiB；沙箱/原生拖放转换为不透明句柄。
- 连接测试、生命周期和工作台 RPC 由 Host 注入随机 `operationId`；Host 丢弃无作用域挑战，并拒绝过期、伪造和重复 resolve。Sidecar 对 `operationId` 再校验。
- Rust Sidecar 实现五种认证、持久化 `known_hosts`、PTY、SFTP、2 MiB 终端序号缓存、Bash/Zsh OSC 7、`rz` 上传、流式传输、只读限制及会话/任务隔离。
- 工作台还原旧版紧凑工具栏、双栏换位/拖动、xterm、文件表格/排序/列设置/菜单、行内重命名、弹窗、CodeMirror 只读预览、传输列表和七国语言。

## 已通过的自动验证

- Host Vue/TypeScript 全量类型检查通过。
- Host 插件桥接、条件表单和连接挑战定向测试 31/31 通过。
- `dbx-core` Host API 1.1 `cargo check` 通过。
- 插件 Sidecar 测试 12/12 通过；前端类型检查、4 项协议/i18n 测试和生产构建通过。
- Windows x64 0.2.1 实机包通过 `192.168.1.64:22` 的终端、SFTP 目录、4 MiB/空文件往返、取消及变化主机密钥拒绝烟测。
- 本地浏览器视觉夹具验证 1440×900 暗色与 1920×1080 亮色；左右换位、列菜单、新建目录弹窗和 760px 代码预览可用。

## 仍需实机/CI 验收

- Windows DBX 桌面重启加载 0.2.1 后，继续复测除密码外的四种认证、标签附着、剪贴板、原生拖放和桌面原子保存。
- Docker/Web 验证浏览器文件对象、OPFS/下载行为、用户隔离与临时文件清理策略。
- 100 MiB、1 GiB、三并发、取消/断网/进程退出、30 分钟终端和突发补发压力测试。
- macOS x64/arm64、Linux x64/arm64 候选包构建与安装；0.2.1 回滚。

这些项目是发布门槛；未经用户再次确认不执行推送、发布或上游 PR。

## 补充（2026-08-28）：appearance 桥接在本地宿主 worktree 落地 + 白色主题

上文 1.1 分支（codex/plugin-host-ssh-enablers）本地不存在；appearance 此前在
`dev/plugin-framework-current` 工作树上缺失，插件在真实宿主里拿不到
`api.appearance`，永远停留在深色兜底。本轮补齐：

- 宿主 `pluginHostBridge.ts`：构造函数新增 `appearance` 参数；`init` 消息携带
  外观快照；新增 `forwardAppearance()` 以 `type: "appearance"` 实时推送；注入 SDK
  暴露 `dbxPlugin.appearance` getter 与 `onAppearanceChange`。
- 宿主新增 `pluginAppearance.ts`：从 DBX 根节点读取设计令牌（`--background`
  等允许列表，随 `.dark`/调色板类实时变化）+ 编辑器字体设置，组装外观快照；
  `PluginWorkbenchHost` 监听 isDark/调色板/字体变化只推送、不重载 iframe。
- 插件 `frontend/src/lib/appearance.ts`：DBX 规范双色板（浅色 = pearl 白底
  `rgb(255 255 255)` 等，深色 = `.dark` 块），宿主缺字段时按方案回退；
  xterm 浅色 ANSI 参考 VS Code Light+（白底下可读）。
- 语义：Host API 1.0 无 appearance 时插件仍深色兜底（可降不可死）。
- 验证：宿主 pluginHostBridge/pluginAppearance 单测 32/32、宿主 typecheck、
  插件 typecheck + vitest 21/21 + 构建通过；mock 夹具 1440×900 亮/暗双截图核对。
  实机验收仍需重建宿主（`pnpm tauri build --debug`）后观察主题实时联动。
