# MCP 集成

SSH/SFTP 插件有两种被 MCP 调用的方式。**推荐使用 DBX MCP 桥**——凭据来自 DBX 保存的连接，不另起进程、不另配凭据。

## 方式一：DBX MCP 桥（推荐）

DBX 的 MCP 服务器（`dbx mcp` 或桌面内置 MCP）内置两个通用插件桥工具：

| 工具 | 说明 |
| --- | --- |
| `dbx_list_plugin_tools` | 列出所有已装插件贡献的 MCP 工具（含本插件的 19 个 SSH/SFTP 工具及其 JSON Schema） |
| `dbx_call_plugin_tool` | 调用插件工具；传 `connectionId` 即引用 DBX 已保存的 SSH 连接，凭据由 DBX 解析转发，**工具参数里不出现任何密码** |

典型调用流（MCP 客户端视角）：

```
dbx_list_connections                      → 找到 SSH 连接 id
dbx_list_plugin_tools                     → 发现 io.dbx.ssh 的 ssh_exec / sftp_* / ssh_metrics 等工具
dbx_call_plugin_tool {
  pluginId: "io.dbx.ssh",
  tool: "ssh_exec_sudo",
  connectionId: "<DBX 连接 id>",
  arguments: { "command": "systemctl status nginx" }
}
```

实现细节：

- 桥由宿主 `dbx-mcp` 提供（`LocalBackend`），通过标准 sidecar 协议调用插件的 `mcp/tools`（工具发现）与 `mcp/call`（工具执行）方法；连接凭据以标准 connection lifecycle payload 转发（`PluginHost::connection_params_standalone`），与工作台连接共用同一份配置——包括跳板链、Quick Sudo、TOTP、提示词、`auth_flow_mode` 等全部设置项。
- 插件侧按 `connectionId` 维护连接池，断线自动重连；主机密钥沿用插件的 known_hosts 存储（首次未知主机需先在工作台连接一次确认，或在方式二中用 TOFU）。
- Scoped AI 会话中 `dbx_call_plugin_tool` 被禁用（可能触发变更操作），`dbx_list_plugin_tools` 保持可见。

### 客户端配置示例

```json
{
  "mcpServers": {
    "dbx": {
      "command": "/path/to/dbx",
      "args": ["mcp"]
    }
  }
}
```

## 方式二：独立 stdio 模式（无 DBX 宿主时）

插件二进制直接作为 MCP 服务器运行（MCP `2024-11-05`，换行分隔 JSON-RPC 2.0）：

```bash
dbx-plugin-ssh --mcp
```

此模式下没有 DBX 连接存储，凭据随每次调用内联传入（`host`/`username`/`password` 或 `privateKeyPath`，及 Quick Sudo/2FA 编排字段、`jumpHosts` 跳板链），按 `username@host:port` 进程内池化。未知主机密钥采用 **TOFU 首次信任**（记录后变化仍拒绝）。MCP 模式与 DBX 插件模式互斥：同一进程只运行其中一种。

## 工具一览（19 个，两种方式通用）

| 工具 | 说明 |
| --- | --- |
| `ssh_exec` / `ssh_exec_sudo` | 非交互远程命令；sudo 版注入密码并自动应答 2FA/TOTP |
| `ssh_metrics` | CPU/内存/负载/磁盘/运行时长（只读命令） |
| `ssh_test_connection` | 验证连通性与认证（含跳板链），返回延迟 |
| `ssh_list_known_hosts` / `ssh_remove_known_host` | 管理插件 known_hosts（不改系统 `~/.ssh/known_hosts`） |
| `ssh_close` | 关闭缓存的连接（方式二按连接键；方式一由 sidecar 生命周期管理） |
| `sftp_list_dir` / `sftp_stat` / `sftp_exists` | 浏览与检查远端路径 |
| `sftp_read_file` / `sftp_write_file` | 读写远端文件（文本或 base64） |
| `sftp_mkdir` / `sftp_remove` / `sftp_rename` / `sftp_chmod` | 目录与文件管理 |
| `sftp_disk_usage` | 路径所在挂载的磁盘用量 |
| `sftp_copy` / `sftp_move` | 服务器内复制 / 剪切（`from` 单值或数组 → `toDir`，逐项返回成败） |

## 与 tiny-rdm mcpctl 的关系

工具命名与语义对齐 tiny-rdm 的 `ssh_exec` / `ssh_exec_sudo` / `sftp_*` 工具族。tiny-rdm 用本地 profile 存储 + 审批；本插件在 DBX 桥模式下凭据与审批归宿主（DBX 连接存储 + MCP scope），独立模式用内联凭据 + TOFU，并额外提供 `ssh_metrics`、`sftp_chmod`、`sftp_disk_usage` 与 known_hosts 管理。
