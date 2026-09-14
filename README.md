# DBX SSH & SFTP

[![CI](https://github.com/jinpy666/dbx-plugin-ssh/actions/workflows/ci.yml/badge.svg)](https://github.com/jinpy666/dbx-plugin-ssh/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/jinpy666/dbx-plugin-ssh?display_name=tag)](https://github.com/jinpy666/dbx-plugin-ssh/releases)

[English](README.en.md) · [产品宣传页](docs/MEDIA.zh-CN.md) · [特性与竞品对比](docs/COMPARISON.zh-CN.md) · [独立仓库迁移说明](docs/REPOSITORY_SPLIT.zh-CN.md)

DBX SSH & SFTP 是面向现代运维团队的服务器工作台：打开一个连接，就能完成终端操作、
文件传输、跳板访问、受控提权和 MCP 自动化。它把“登录服务器之后的下一小时”压缩成
一个连贯、可审计、可复用的工作流。

![DBX SSH & SFTP 工作台](docs/screenshots-a-ssh/01-toolbar-dark.png)

> 终端、SFTP、MCP 自动化和受控运维操作集中在一个 DBX 工作台中。

## 先看效果

[观看 87 秒真实操作演示](docs/media/dbx-ssh-live-demo.mp4) ·
[观看 15 秒功能预览](docs/media/dbx-ssh-demo.mp4)

![DBX SSH & SFTP 产品总览](docs/media/dbx-ssh-overview.png)

如果你正在工具之间来回切换：终端一个窗口、SFTP 一个窗口、跳板机靠配置文件、
自动化又要另写脚本，那么 DBX SSH & SFTP 的价值很直接——连接、终端、文件和自动化
都围绕同一个 DBX 连接上下文工作。

## 为什么值得用

| 你要完成的事 | DBX SSH & SFTP 给你的体验 |
| --- | --- |
| 快速处理线上问题 | PTY 终端、命令历史、快速命令、可取消命令和目录跟随 |
| 安全地传文件 | SFTP 预览、拖放、断点确认、进度、取消和原子替换 |
| 进入私网服务器 | 最多三跳 ProxyJump、连接保活和 Known Hosts 校验 |
| 限制高风险操作 | 只读模式、sudo 来源选择、命令白名单和宿主 secret binding |
| 把重复工作自动化 | MCP 工具复用连接、审批和权限边界 |

## 适合场景

- 日常 Linux/Unix 服务器巡检、日志查看和远程命令执行。
- 通过跳板机访问内网环境，并在终端与 SFTP 之间快速切换。
- 在受控权限下完成配置文件、脚本和构建产物的上传下载。

## 核心能力

- 支持密码、私钥、SSH Agent、键盘交互和无密码认证。
- 交互式终端支持 PTY、窗口调整、会话恢复、剪贴板、远程目录跟随和批量输入。
- SFTP 支持浏览、排序、预览、上传、下载、重命名、新建、拖放和递归删除。
- 大文件传输支持进度、取消、断点确认和原子替换，降低中断造成的半成品风险。
- 支持 Known Hosts 校验、变更主机密钥拒绝、只读模式和远程目录磁盘用量查看。
- 支持最多三跳 ProxyJump、连接保活、可取消远程命令、chmod 和 Quick Sudo。
- 支持 TOTP/键盘交互式双因素认证，以及面向自动化客户端的 MCP 工具接口。
- 界面支持简体中文、繁体中文、英语、西班牙语、意大利语、日语和葡萄牙语。

完整的能力矩阵和与 OpenSSH、Tabby、Termius、FinalShell 等方案的定位对比见
[特性与竞品对比](docs/COMPARISON.zh-CN.md)。

![快速命令与终端操作](docs/screenshots-a-ssh/02-quick-commands.png)

## MCP 自动化

推荐通过 DBX MCP 桥调用，以复用已保存连接、审批和 sudo 白名单。独立模式可运行：

```bash
DBX_SSH_MCP_READ_ONLY=1 backend/target/release/dbx-plugin-ssh --mcp
```

常用工具包括 `ssh_exec`、`ssh_metrics`、`sftp_list`、`sftp_upload` 和
`sftp_download`。完整配置、工具调用和安全边界见
[MCP 使用指南](docs/MCP_USAGE.zh-CN.md)与[SSH MCP 参考](docs/MCP.zh-CN.md)。

## 安全设计

密码、私钥口令、TOTP 和 sudo 凭据由 DBX 宿主 secret binding 管理。插件不会把
凭据写入配置文件、日志或导出内容。建议生产连接启用 Known Hosts 严格校验，
并按需启用只读模式和 sudo 命令白名单。

## 安装

从 [GitHub Releases](https://github.com/jinpy666/dbx-plugin-ssh/releases) 下载匹配平台的
`.dbxp` 包，在 DBX 插件中心选择本地安装。开发者也可以按照
[迁移与发布说明](docs/REPOSITORY_SPLIT.zh-CN.md) 构建候选包。

## 开发与验证

```bash
pnpm --dir frontend install
pnpm --dir frontend typecheck && pnpm --dir frontend test && pnpm --dir frontend build
cargo test --manifest-path backend/Cargo.toml
python3 scripts/validate_repo.py && node scripts/connection-forms/verify.mjs
scripts/test.sh --skip-host
```

协议、构建和完整集成验证说明位于 `docs/`；公开贡献请先阅读
独立仓库的迁移边界、公共依赖和发布前置条件见
[迁移说明](docs/REPOSITORY_SPLIT.zh-CN.md)。
