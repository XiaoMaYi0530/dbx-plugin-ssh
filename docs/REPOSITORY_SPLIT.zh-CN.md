# SSH 插件独立仓库迁移说明

## 迁移来源与历史

本仓库由 `/Users/Jinpy/btroot/dbx-plugins/ssh` 拆出，迁移基线为源 monorepo
commit `ce19beb4fa75cc79048e3ba8852c298d674fd9b3`。初始导入使用
`git subtree split --prefix=ssh`，因此保留了 SSH 目录相关提交历史。

源仓库当时的 `host` 是用户正在使用的子模块 checkout，本次迁移没有更新、reset、
清理或写入它。截图等被 monorepo 忽略但被 SSH README 引用的文档资产也复制到
`docs/screenshots-*`，避免独立 clone 后断链。

## 独立仓库内容

- 根目录直接包含 `manifest.json`、`dbx-plugin.toml`、`frontend/`、`backend/`、
  `assets/`、`ui/`、`scripts/`、`docs/` 和 `.github/`。
- `shared/frontend/` 只保留 SSH 实际使用的 `binaryEvent`、`editorTheme`、
  `themeSync` 及说明；没有迁入 LDAP、Files 或 Kafka 代码。
- `scripts/connection-forms/verify.mjs` 是连接表单契约的独立校验器，包含与 DBX
  Host 条件语义一致的可见性/必填级联检查及 SSH 七语言场景矩阵。
- `shared/sdk/rust/dbx-plugin-sdk/` 是与拆分基线匹配的最小 sidecar SDK vendoring，
  使 `cargo test` 不再依赖 `../host`。它只包含 sidecar 协议 SDK，不包含 DBX.app。

未来若发布公共包，应将前端适配层和连接表单契约分别移到版本化公共包；先在本仓库
保留兼容 shim，完成新旧宿主版本矩阵验证后再删除 vendored 副本。SDK 也应在 crates.io
或 DBX 官方 Git 依赖稳定后，把 `backend/Cargo.toml` 的 path dependency 切换到锁定
版本，并保留协议初始化/二进制帧回归测试。

## CI 与发布

- `.github/workflows/ci.yml` 校验 manifest、dbx-plugin.toml、Rust backend identity、
  连接表单，运行前端 typecheck/test/build、`cargo test`、离线 MCP stdio smoke，
  并在 Ubuntu 产出 Linux candidate `.dbxp`。
- `.github/workflows/release.yml` 仅在 `ssh-v<manifest.version>` GitHub Release 上
  调用 DBX 官方 reusable release workflow。它不内置 signing key、远端仓库或秘密。
- 当前 `manifest.json` 的 `source`/`homepage` 使用显式 `https://github.com/TODO/`
  占位值。创建 GitHub 仓库后，必须先替换这两个字段，再创建匹配的 release tag。

## Bug 回写与发布边界

独立仓库中的 bug 先在本仓库修复并通过 CI；若 DBX Host API、宿主安装器或公共 SDK
需要变更，应另开 DBX host/SDK 变更并在 issue/PR 中记录对应版本。不要把本仓库重新
指向 monorepo 的 `../shared`；需要跨插件复用时应发布公共包或同步回各独立仓库的
明确版本。

真实 SSH 服务、Docker 测试容器和 DBX.app host 安装管线不是离线 CI 的通过条件；没有
这些环境时相关 live/e2e 脚本必须输出 `SKIP`，不能伪报 `PASS`。
