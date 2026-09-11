# 实施计划：凭据静态加密（vault）+ 传输历史落盘 + SFTP 书签

日期：2026-09-11。来源：omarsenusi/sshbool 对比评审（Tauri v2 + russh 桌面工作台；
对标点：`crypto.rs` Argon2id/AES-256-GCM 信封加密与 keychain 托管、
`migrations/0004_transfers.sql` transfer_jobs/transfer_items 持久化与
sftp_bookmarks、`migrations/0002_vault.sql` vault 信封结构）。

两个特性均为**纯插件侧**改动：无宿主改动（硬性规则 8）、无协议破坏性变更；
新增依赖 4 个（`aes-gcm`、`keyring`、`zeroize`、`rand`，全部归 WP-A）。
用户已拍板两项关键决策：密钥托管 = OS keychain 承载 DEK（keyfile 回落）；
传输范围 = 历史落盘 + 书签（不做断点续传）。

## 0. 目标与非目标

### 特性 A：凭据静态加密（vault）
- `quick-sudo-profiles.json` 中 `sudoPassword` / `totpSecret` 不再明文落盘，
  改为 AES-256-GCM 信封密文（字段级）。
- 密钥托管双档：OS keychain（macOS Keychain / Windows CredMan / Linux Secret
  Service，经 `keyring`）优先；不可用（web Docker、无 dbus 的 headless Linux）
  回落同目录 0600 密钥文件，envelope 头记 `storage` 档位。
- 内存 `SudoProfile` 结构、`profile_view` / `sudo/profiles/reveal` / MCP 布尔面
  **完全不变**（密钥仍只在内存态出现，视图永不回显）。

非目标：不做主密码 / 解锁 UX（sshbool 的 Argon2id 主密码模型明确不采纳——
宿主插件形态下无人值守 sudo 自动应答要求重启后免解锁）；不加密
quick-commands / agent-modes / mcp-settings / known_hosts（无密钥或公开数据）；
连接级凭据仍归宿主 secret binding（硬性规则 3 不变）。

### 特性 B1：传输历史落盘
- 传输任务状态跃迁（start / complete / cancel / fail）写入
  `transfer-history.json`，跨 sidecar 重启可查；全局环形上限 200 条。
- 新增 `sftp/transfer/history` 只读查询（持久化历史 + 内存 live 合并）；
  现有 `sftp/transfer/list` 语义不动。

非目标：断点续传（PROGRESS §3 既有登记，单独立项）；不进 MCP 工具面；
不做历史清空方法（YAGNI，环形覆盖即可）。

### 特性 B2：SFTP 路径书签
- 全局命名路径清单（不按连接分），上限 20 条，镜像 quick_commands 模式。
- 新增 `sftp/bookmarks/list|save|delete` 三方法 + 工作台路径栏星标收藏 /
  下拉跳转 UI + 七语文案。

非目标：不按连接 / 宿主分组（后续可扩展）；路径不做存在性校验（书签可指向
未挂载路径）。

## 1. 决策记录（实施中不得静默偏离）

| # | 决策 | 理由 |
| --- | --- | --- |
| D1 | DEK = 随机 32B，AES-256-GCM 字段级信封，密文 `base64(nonce‖ct)`，AAD 绑定 `"<field>|<profileId>"` | 防密文在字段/档案间挪用；无主密码故无需 KDF，Argon2id 不引入 |
| D2 | KeyProvider trait 注入：`KeychainProvider`（keyring，service=`io.dbx.ssh`，account=`vault-dek-v1`）+ `KeyfileProvider`（`<data_dir>/vault.key`，0600）；解析顺序 = envelope `storage` 字段指档，缺档（首次写入）尝试 keychain、出错回落 keyfile | 测试可用 Keyfile+tempdir 钉死路径，不依赖 CI keychain；回落保证 web Docker 等无 keychain 环境可用（文档声明弱保证） |
| D3 | 文件升 `version:2` + `crypto:{scheme:"aead-v1",storage:…}`；密钥字段改名 `sudoPasswordEnc` / `totpSecretEnc`（空值存空串不加密）；元数据（name/hints/mode/bindings）保持明文 | 列表/下拉/options 不需要解密即可渲染；v1 文件 load 后立即 best-effort 重写 v2 |
| D4 | 解密失败 / 密钥丢失（keychain 条目被删）：该字段按空处理、metadata 保留、对应 `sudoPasswordSet=false` | 沿用「坏文件不破坏工作台」既有策略；密文可丢，档案结构不可丢 |
| D5 | DEK 内存态 `Zeroizing` 包装；现有 sudo_profiles 单测全部改走 Keyfile 注入变体 | macOS 本机跑测试不得弹 keychain 授权框 |
| D6 | 传输历史为 JSON 整文件原子重写（tmp+rename，0600），仅状态跃迁触发；跨进程（embedded 与 stdio `--mcp` 共享数据目录）last-writer-wins | 历史是 best-effort UX 数据非审计账；#4 执行审计已为跨进程正确性选 JSONL append，二者定位不同 |
| D7 | 重启后遗留 `running`/`queued` 历史态在加载时标 `failed`（error 注明 sidecar 中断），不新增状态枚举 | 前端 `transferStatus` 已有 failed 文案，协议枚举零膨胀 |
| D8 | 书签存储 `sftp-bookmarks.json`（version 化、tmp+rename 原子写、0600、坏文件回退空库），字段 `{id,label,path,createdAt,updatedAt}`，label 1–64 唯一（大小写不敏感）、path 非空≤1024 | 完全镜像 quick_commands 的模式、校验与测试风格，零新心智模型 |
| D9 | 三个新方法均工作台方法，不注册 MCP 工具 | 与 sudo/profiles/reveal 同理：工作台功能面，MCP 通道不需要 |

### 决策修订（2026-09-11 当日，用户反馈驱动）

| # | 修订 | 理由 |
| --- | --- | --- |
| D2a | **默认档反转为 keyfile**：`resolve_provider(None)` 不再探测 keychain；keychain 仅在 env `DBX_SSH_VAULT_STORAGE=keychain` 显式选入或读取遗留 keychain 档信封时使用。keychain 解析进程级缓存（至多弹一次授权）；遗留 keychain 档文件首次成功解密时**自动迁移**为 keyfile 档并删除 keychain 条目；被拒绝时该进程不重试、文件保持原档位（不以空密文覆盖） | 用户真机反馈"每次启动反复输入几次密码"：macOS keychain 对访问方二进制做 ACL 校验，插件每次更新二进制变化即重弹授权框，且工作台 + stdio MCP 双进程各弹一轮——与无人值守 sudo 自动应答的产品前提根本冲突。keyfile 档零弹窗、跨更新稳定；安全声明回落为"防拷贝/备份外泄"（文档已同步） |

## 2. 协议契约（PROTOCOL.zh-CN.md 同步内容，camelCase）

### 2.1 `sftp/transfer/history`（新增）
- 参数：`sessionId?`（可选过滤）、`limit?`（默认 50，上限 200）。
- 返回：`{ tasks: [...] }`，持久化历史与内存 live 按 taskId 去重合并、新→旧
  排序；元素 `{ taskId, sessionId, connectionId, direction, fileName, size,
  transferred, status, startedAt, finishedAt, error? }`；`status` ∈
  `running` / `completed` / `cancelled` / `failed`（沿用现有枚举）。
- 无活动连接也可查询（纯本地数据）。

### 2.2 `sftp/bookmarks/list`（新增）
- 无参数。返回 `{ bookmarks: [视图…] }` 按 label 排序；视图 `{ id, label, path,
  createdAt, updatedAt }`。

### 2.3 `sftp/bookmarks/save`（新增）
- 参数：`id?`（有=更新须存在，无=新建）、`label`、`path`。
- 返回 `{ bookmark: 视图, created }`。
- 错误：label 为空/超长/重复（大小写不敏感）、path 为空/超长、id 不存在、
  超出上限 20。

### 2.4 `sftp/bookmarks/delete`（新增）
- 参数：`id`。返回 `{ success, removed }`；未知 id 报错。

## 3. 实施拆分（工作包与文件所有权——并发 agent 互不重叠）

### WP-A 后端 vault（Agent A 独占）
- 新增 `ssh/backend/src/vault.rs`：`KeyStorage`、`KeyProvider` trait 与两个
  实现、`seal`/`open`（AAD 绑定）、DEK 生成与 `Zeroizing`。
- 改 `ssh/backend/src/sudo_profiles.rs`：v2 文件格式、字段级加密/解密、
  v1→v2 迁移、解密失败回退、`load_store_with`/`save_store_with` 注入变体
  （既有测试全部改走 Keyfile+tempdir）。
- 独占 `ssh/backend/Cargo.toml`（+`aes-gcm` `keyring` `zeroize` `rand`）与
  Cargo.lock。
- `mod vault;` 声明由主会话预置，A 不改 main.rs。

### WP-B 后端传输历史 + 书签（Agent B 独占）
- 新增 `ssh/backend/src/transfer_history.rs`：`transfer-history.json` 读写、
  环形上限 200、状态跃迁记录 API、加载时遗留 running 标 failed（D6/D7）。
- 新增 `ssh/backend/src/sftp_bookmarks.rs`：镜像 quick_commands（D8）。
- 改 `ssh/backend/src/ssh.rs`：传输生命周期四类跃迁点挂钩（upload finish、
  download finish、cancel、错误路径）；提供 live+历史合并查询。
- 改 `ssh/backend/src/main.rs`（仅路由段）：注册 `sftp/transfer/history`、
  `sftp/bookmarks/list|save|delete` 四个路由臂。
- 不加依赖、不改 Cargo.toml。

### WP-C 前端（Agent C 独占）
- 新增 `ssh/frontend/src/lib/sftpBookmarks.ts`：类型、RPC 封装、排序/校验
  纯函数 + `sftpBookmarks.spec.ts`（镜像 sftpPathHistory 模式）。
- 改 `ssh/frontend/src/App.vue`：路径栏星标收藏 + 书签下拉（跳转/删除）；
  传输面板增加「历史」区（读 `sftp/transfer/history`，failed 显示原因）。
- 改 `ssh/frontend/src/lib/i18n.ts`：七语（ar/de/en/es/ja/pt/zh-CN）新增键，
  保持 locale 键齐平。
- 仅动前端，不跑构建安装（pnpm typecheck/test 验证）。

### WP-E 收口（主会话）
- PROTOCOL.zh-CN.md §2.1–2.4 同步 + sudo/profiles 节补「密钥静态加密」说明；
- FEATURE_PARITY.zh-CN.md 增「sshbool 对标（2026-09-11）」节；
- PROGRESS-P-SSH.zh-CN.md 收尾记录；smoke 用例与 `scripts/test.sh` 全量验证。

## 4. 验证与完成定义（四件套映射）

| 件 | 内容 |
| --- | --- |
| 单测 | backend：vault roundtrip / v1→v2 迁移 / 坏密文回退 / AAD 挪用拒绝 / keychain 缺失回落 / 历史环形与中断标注 / 书签校验矩阵；frontend：sftpBookmarks.spec |
| smoke | `scripts/test.sh`：新四方法注册与参数校验（未实现路径 SKIP 语义按规则 5） |
| 对标/任务清单 | FEATURE_PARITY sshbool 节 + PROGRESS 收尾记录（本计划即任务清单） |
| 七语文案 | 书签 UI 与传输历史区全部新键 ×7 locale |

真机复验点（宿主环境）：keychain 实际存取（macOS 首次写入后 `security
find-generic-password` 可见）、reveal 预填流、v1 明文文件升级、书签/历史 UI
交互。主会话集成后统一执行 `cargo test` + `pnpm typecheck` + `pnpm test` +
`scripts/test.sh`。

## 5. 风险与边界

- `keyring` Linux 后端依赖 Secret Service（dbus）：web Docker 自动落 keyfile
  档，静态加密保证降级为「防拷贝/备份外泄」，文档明示。
- keychain 条目被用户/系统清理 → 密钥永久丢失，密钥字段按空处理（D4），
  档案 metadata 保留可重填。
- 传输历史跨进程 last-writer-wins（D6）：极端并发下可能丢少量历史条目，
  可接受；执行审计（#4）不在此列。
- 新方法对宿主老版本无影响（宿主不感知插件方法集，optional 特性），
  规则 3 合规。

## 6. 实施顺序建议

WP-A（独立）→ WP-B（独立）→ WP-C（依赖 §2 契约，已定型可并行）→ 主会话
集成 → 全量验证 → WP-E 文档收口。三个工作包由并发 agent 同步执行；
`main.rs` 的 `mod` 声明由主会话预置以消除文件冲突。
