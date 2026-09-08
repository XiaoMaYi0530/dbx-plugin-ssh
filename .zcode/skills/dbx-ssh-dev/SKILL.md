---
name: dbx-ssh-dev
description: DBX SSH/SFTP 插件的开发规范、构建、测试与安装全流程。当用户要求构建/测试/安装/打包该插件、排查 sidecar 或宿主（DBX worktree）问题、新增 SFTP/SSH/sudo 能力、或涉及 scripts/ 下自动化脚本时使用。
---

# DBX SSH/SFTP 插件开发规范

本项目是 DBX 数据库客户端的 SSH/SFTP 插件：Vue/Vite 沙箱前端 + Rust sidecar，
通过 `dbx-plugin` CLI 打包为 `.dbxp` 安装进 DBX。能力面覆盖隧道/代理、连接管理、
外观（DBX 已有能力，**整合不重复实现**）与终端/SFTP/sudo 全链路。

> 仓库位于 `~/btroot/dbx-plugins/ssh-sftp`（插件族工作区，兄弟目录
> ldap/、files/、shared/ 为 LDAP/Files 插件与公共文档，见工作区 README）。

## 目录结构

```
ssh/                  # 即 ~/btroot/dbx-plugins/ssh-sftp
├── manifest.json          # 插件契约（字段/权限/贡献点/七语）
├── dbx-plugin.toml        # 打包配置（include: assets, ui）
├── frontend/              # Vue3 工作台（单文件 App.vue 为主）
│   └── src/lib/i18n.ts    # 七语文案（zh-CN/zh-TW/en/es/it/ja/pt）
├── backend/               # Rust sidecar（russh + russh-sftp）
│   └── src/{main,ssh,exec,sudo_fs,sftp_ext,keys,mcp,model,host_key}.rs
├── scripts/               # 自动化（见下）
├── docs/FEATURE_PARITY.zh-CN.md   # 对标清单（能力×状态）
└── ../host/               # 宿主子仓库（t8y2/dbx dev/plugin-framework-current
                           #   + 本地二开提交；旧路径 ~/btroot/dbx-plugin-host-worktree
                           #   为指向它的符号链接）
```

## 环境准备（一次性）

```bash
# Node 经 nvm；pnpm 在 ~/Library/pnpm；Rust 固定 1.88.0（宿主要求 rust-version）
export PATH="$HOME/.nvm/versions/node/v22.21.0/bin:$HOME/Library/pnpm:$HOME/.cargo/bin:$PATH"
npm install -g @dbx-app/plugin-cli @dbx-app/cli @dbx-app/mcp-server
# 插件本地打包不需要 DBX 源码：npm CLI 自带 SDK（sdk-root）
# 仅测试宿主/装进数据目录的集成验证才需要 ../host 子仓库（scripts/host-sync.sh init）
```

## 开发规范（硬性约定）

1. **协议命名**：sidecar 方法 `<域>/<动作>`（如 `sftp/upload/start`），参数/返回字段
   camelCase；同族方法参数保持一致（sudo 家族用 `path`，上传族用 `remotePath`，
   rename 用 `sourcePath/targetPath`）。新增方法必须同步 `docs/PROTOCOL.zh-CN.md`。
2. **兼容基线**：目标 Host API 1.0（`engines.host_api: ">=1.0.0"`）。宿主 1.1 特性
   （appearance/locale 实时事件、fileTransfer、workbenchState）一律 optional 降级，
   缺失时功能可降但不可死。**用 `visible_when` 等宿主能力，宿主 worktree 已补实现。**
3. **不重复宿主**：隧道/代理/跳板走 DBX 传输层——sidecar 用 `runtime.host/port` 拨号、
   用原始 `connection.host/port` 做主机密钥校验；禁止新增与宿主重叠的表单字段。
4. **安全红线**：远端命令一律 shell 单引号转义；写操作过 `ensure_writable`（只读连接）；
   sudo 写入分块 ≤256KiB；`removeAll` 拒绝 `/`；私钥只出指纹不出内容。
5. **前端**：新文案七语全补（漏一语 typecheck 不报，靠 review）；错误走
   `showError(cause, "terminal"|"sftp")`；无新依赖；`fileTransfer` 缺失（web/docker
   模式）必须有浏览器兜底（File API 上传 / Blob 下载）。
6. **测试**：新能力 = 单测（纯解析，不连 SSH）+ smoke 用例（scripts/smoke_*.py，
   未注册方法 SKIP 而非 FAIL）+ 对标清单状态更新，三者齐才算完成。

## 构建与测试

```bash
scripts/build.sh            # 前端三件套(typecheck/test/build) + dbx-plugin package
scripts/test.sh             # 全套：cargo test → 前端三件套 → release 构建 → 打包
                            #   → MCP stdio smoke → 宿主安装管线集成测试（all green 才算过）
scripts/smoke_test.py       # 无 UI 全链路冒烟：连接→挑战→PTY→SFTP→关闭
scripts/smoke_fs_test.py    # 新能力冒烟（sudo/sftp_ext/keys，17 用例）
scripts/smoke_mcp.py        # --mcp stdio 模式冒烟
scripts/sidecar_client.py   # stdio-framed 协议客户端库（直接驱动 sidecar 调试）
```

- 测试 SSH 容器：`docker run -d --name dbx-ssh-test -p 2222:2222
  -e USER_NAME=sshuser -e USER_PASSWORD=DbxTest2026 -e PASSWORD_ACCESS=true
  linuxserver/openssh-server`，另开 `AllowTcpForwarding yes`（改
  `/config/sshd/sshd_config` 后重启容器）供隧道语义验证。
- sidecar 协议：5 字节帧头 `[kind:u8][len:u32 BE]`+payload；binary 帧内
  `[u16 channel_len][channel][data]`；终端输入/上传块带 8 字节 BE 前缀
  （sequence/offset）——细节全封装在 sidecar_client.py，勿手搓。
- 主机密钥挑战测试：改容器 host key 或换 DBX_PLUGIN_DATA_DIR 触发 unknown 态。

## 安装与交付

```bash
scripts/install.sh                # 官方 PluginPackageInstaller 安装 + 重启 DBX
scripts/install.sh --reinstall    # 同版本重装（开发迭代用）
scripts/install.sh --app-data <dir> --no-restart   # 自定义存储/不重启
scripts/install.sh --skip-host 的 test.sh   # 跳过宿主管线（无 worktree 时）
```

- 安装语义：`plugins/<id>/versions/<ver>/` + `activations/<20位序号>-<uuid>.json`
  （`{sequence, version, previousVersion, packageSha256, activatedAt}`，camelCase）。
- **坑**：installer example 二进制有构建缓存——宿主 manifest 结构体改动后必须
  `rm ../host/target/release/examples/install_plugin` 强制重编，
  否则报 `unknown field` 假错误。
- 测试宿主 DBX.app：`../host/target/debug/bundle/macos/DBX.app`
  （框架分支构建）。正式版 /Applications/DBX.app 无插件框架，装了也没用。
  宿主前端/Rust 改动后需 `pnpm tauri build --debug` 重建（~10min）。
- 双冒烟验收：`DBX_PLUGIN_SIDECAR=<安装路径>/bin/darwin-arm64/dbx-plugin-ssh
  python3 scripts/smoke_test.py && python3 scripts/smoke_fs_test.py`（对安装副本跑，
  确认装的二进制就是测的二进制）。

## 宿主子仓库（../host）

- 分支策略：`dev/plugin-framework-current` = origin/main + 少量本地二开提交；
  `scripts/host-sync.sh sync` 定期合并上游（冲突默认 `-X ours` 本地补丁优先），
  上游覆盖所需功能后再逐个移除二开提交。切勿对 host/ 跑
  `git submodule update --force`（会把 checkout 拉回可拉取的上游提交、丢本地补丁）。
- 本地补丁（已提交 46cac99/1a7d760，勿丢）：
  - `pluginHostBridge.ts` structuredCloneSafe：**structuredClone 不能克隆 Vue
    Proxy**，会 DataCloneError 导致工作台永停 connecting——JSON round-trip 兜底
  - `plugins/host.rs` 生命周期参数带 `operationId`；`queryStore.ts` 工作台
    context 带 `workbenchId`+connection 摘要
  - 条件字段（visible_when/required_when）+ `list_local_ssh_keys` + 短 tab 标题
- 宿主/插件并行开发时按目录划界（backend/ vs frontend/，宿主 worktree 独立仓），
  main.rs 的方法注册由主会话统一接线防冲突。

## 常见故障速查

| 症状 | 根因 |
| --- | --- |
| 工作台永停"正在连接" | 宿主桥 DataCloneError（见上）或 sidecar 未注册该方法 |
| `Missing operationId` | 宿主 1.0 未传——sidecar 已本地生成，见 main.rs `operation_id()` |
| 指纹确认后连接失败 | 旧 dial 超时 bug（已修：挑战期间挂起拨号超时） |
| 打包报 lockfile `--locked` | 新依赖未入 Cargo.lock：用 CLI 同款 patch 参数 regenerate |
| 安装后插件中心报 unknown field | installer/宿主二进制是旧构建缓存，删掉重编 |
| 打包报 Cargo.lock v4 does not understand | /usr/local/bin 旧 cargo 1.69 抢占 PATH；构建/打包前确保 $HOME/.cargo/bin 在 PATH 首位 |
| Go 插件打包报 go.work 与模块 go 版本冲突 | npm CLI 包装注入捆绑 SDK root；直调 plugin-cli-darwin-arm64/bin/dbx-plugin 并 env -u DBX_PLUGIN_SDK_ROOT |
| dbx-plugin: command not found | nvm PATH 未加载；CLI 在 ~/.nvm/.../bin |
