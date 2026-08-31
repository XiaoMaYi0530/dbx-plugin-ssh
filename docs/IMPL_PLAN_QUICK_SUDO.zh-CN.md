# IMPL PLAN：Quick Sudo 全局配置集中管理（io.dbx.ssh）

> 上游需求：tiny-rdm 的「全局 Manual Sudo + Profile 级 QuickSudo（inherit/disable/override）」
> 能力对标，迁移为 DBX 插件形态的「多套全局 quick sudo 配置 + 连接级选择/覆盖」。
> 基线：PROTOCOL.zh-CN.md（v0.2.9）、FEATURE_PARITY.zh-CN.md L41（原结论「不做」，本文推翻）。
> 修订：2026-08-30 初版。

## 0. 目标与非目标

**目标**

1. 支持创建/编辑/删除**多套**全局 quick sudo 配置（名称 + sudo 密码 + TOTP 密钥 +
   2FA 认证流 + 提示词 + PTY 开关），集中管理、跨连接复用（典型场景：多台机器共享
   同一套运维 sudo 密码）。
2. 单个连接在「本连接配置」（现状：宿主 secret binding 下发的 sudo_password/totp 等）
   与「某个全局配置」之间**选择**；选择结果插件侧持久化，重连/重启保留；会话内可通过
   `ssh/settings/set` 临时切换。
3. UI 与 MCP 双通道：工作台设置弹窗选择来源 + 管理弹窗 CRUD；MCP 新增配置管理
   工具，`ssh_exec_sudo` 支持按 id/名称引用全局配置。
4. 终端 auto sudo（`TerminalAutoSudo`）与 exec 通道（`ssh/exec{sudo:true}`）自动
   使用所选来源的凭据——凭据解析收口到既有 orchestration 通路，不改自动应答状态机。

**非目标**

- 不动 manifest 连接表单字段（宿主 select 选项是静态的，无法动态列出全局配置；
  选择入口在工作台设置弹窗，不由宿主连接表单承载）。
- 不做凭据加密存储（本期插件数据目录 JSON 明文 + 0600 权限，与 mcp-settings.json
  同信任域；OS 钥匙串方案留待后续，见 §9）。
- 不改 Quick Sudo 总开关语义（`quick_sudo` 布尔、只读门禁、保活）。

## 1. 数据模型

存储文件 `<plugin_data_dir>/quick-sudo-profiles.json`（与 mcp-settings.json 同目录
同模式；`SshRuntime::data_dir` 已有）：

```json
{
  "version": 1,
  "profiles": [{
    "id": "uuid-v4",
    "name": "ops@prod",
    "sudoPassword": "…",
    "totpSecret": "…",
    "authFlowMode": "password_then_otp",
    "passwordPromptHint": "",
    "totpPromptHint": "",
    "sudoUsePty": true,
    "createdAt": 1756500000,
    "updatedAt": 1756500000
  }],
  "bindings": { "<connectionId>": "<profileId>" }
}
```

- 上限 `MAX_PROFILES = 20`（对齐前端 quickCommands 上限）。
- `name` trim 后非空、≤64 字符、**大小写不敏感唯一**（MCP 按名称引用需无歧义）。
- `authFlowMode` 经 `AuthFlowMode::parse` 归一化存 canonical 名。
- 提示词经 `exec::sanitize_prompt_hint` 消毒后入库。
- 写入：tmp + rename 原子替换；Unix 下 0600。损坏/缺失文件按空库处理（不阻断）。

**对外视图 `ProfileView`（list/save 返回）永不携带密钥**：`sudoPassword` /
`totpSecret` 只以 `sudoPasswordSet` / `totpConfigured` 布尔呈现。

## 2. 协议契约（新增方法，均无会话依赖）

### 2.1 `sudo/profiles/list`

- 参数：无。
- 返回 `{ "profiles": [ProfileView…] }`，按 `name` 排序。

### 2.2 `sudo/profiles/save`

- 参数（camelCase）：`id?`（有=更新须存在，无=新建）、`name`（必填）、
  `sudoPassword?`、`totpSecret?`（**空串/缺省=保持原值**；显式清除用
  `clearSudoPassword?` / `clearTotpSecret?` 布尔）、`authFlowMode?`（缺省
  `password_then_otp`）、`passwordPromptHint?`、`totpPromptHint?`、`sudoUsePty?`。
- 返回更新后的 `{ "profile": ProfileView }`。
- 错误：名称为空/超长/重复、id 不存在、超出上限。

### 2.3 `sudo/profiles/delete`

- 参数：`id`。
- 返回 `{ "success": true, "removed": bool }`（对齐 `ssh/knownHosts/remove` 语义）；
  级联清理指向该 profile 的 bindings，并对受影响连接的存活会话热更新 orchestration。

### 2.4 `ssh/settings/get|set` 扩展

- `get` 新增：`quickSudoProfileId`（当前生效绑定，空串=本连接）、
  `quickSudoProfileName`（展示用）。
- `set` 新增可选 `quickSudoProfileId`：非空须引用存在的 profile → 持久化 binding +
  对该连接全部存活会话热更新；空串 = 解除绑定。缺省 = 不改变。
- 绑定生效期间，profile 的凭据/提示词/认证流/PTY **整体覆盖**连接自身值与会话级
  `sudoPassword` 等覆盖值（来源互斥：选了全局就不吃本连接字段）；解除后恢复。

## 3. 凭据解析（收口点）

- `exec::SudoAuth` 结构不变。新增纯函数：
  - `AuthFlowMode::name()`（canonical 名，`ssh.rs::flow_mode_name` 改为委托）。
  - `sudo_profiles::apply_profile(auth: &mut SudoAuth, profile: &SudoProfile)`：
    覆盖 password/totp_secrets/hints/flow_mode，保留 otp_usage/committed_totp
    记账（字段级覆盖，不整体替换，与 settings_set 现有策略一致）。
  - `sudo_profiles::effective_use_pty(connection_pty, profile: Option<&>) -> bool`。
- 挂载点：
  - `open_session`：orchestration 构建后按 binding 应用 profile。
  - `exec`：`use_pty` 取 effective 值（orchestration 已含覆盖凭据）。
  - `settings_set`：含 `quickSudoProfileId` 时按新来源重建各会话 orchestration 字段。
  - `sudo/profiles/save|delete`：调用 `SshRuntime::refresh_profile_sessions`
    重新应用所有受影响绑定（配置改动即时生效于存活会话）。

## 4. MCP

新增工具（snake_case，沿用现有命名法）：

| 工具 | 参数 | 说明 |
| --- | --- | --- |
| `ssh_quick_sudo_profiles_list` | 无 | 列出全局配置（ProfileView，无密钥） |
| `ssh_quick_sudo_profiles_save` | 同 §2.2 | 创建/更新；返回 ProfileView |
| `ssh_quick_sudo_profiles_delete` | `id` | 删除；`{success, removed}` |

- `ssh_exec_sudo` 增加可选 `quickSudoProfile`（id **或** 精确名称）：命中即以该
  配置为基础凭据；调用内联 `sudoPassword`/`totpSecret` 显式给出时仍以调用方为准
  （显式覆盖引用）。工具描述同步更新。
- 三个管理工具属本地配置读写，不进 `is_write_tool` 只读门禁。
- MCP stdio（`--mcp`）与 DBX 内嵌模式共用 `SshRuntime::data_dir`，配置同库。

## 5. 前端（App.vue + i18n）

- 设置弹窗 Quick Sudo 区块顶部新增「凭据来源」select：`本连接配置` +
  全局配置列表（`sudo/profiles/list`）；选中全局配置时密码/TOTP/认证流/提示词/PTY
  控件禁用并显示摘要（已设密码 ✓ / TOTP ✓ / 认证流 / PTY）；「管理全局配置…」
  按钮打开管理弹窗。保存时随 `ssh/settings/set` 提交 `quickSudoProfileId`。
- 管理弹窗：列表（名称 + 徽标 + 更新时间 + 编辑/删除）与新建/编辑表单；删除走
  `window.confirm`（对齐 removeKnownHost）；CRUD 后刷新列表与设置弹窗选项。
- `SshSettings` 类型补 `quickSudoProfileId?` / `quickSudoProfileName?`。
- i18n：supplemental 新增键 ×7 语（键集一致性由既有 parity 测试保证）：
  `settingsCredentialSource, profileSourceConnection, profilesManage, profilesTitle,
  profilesHint, profilesEmpty, profilesAdd, profilesEdit, profilesDelete,
  profilesDeleteConfirm, profilesName, profilesNamePlaceholder, profilesLimit,
  profilesSaved, profilesDeleted, profilesNameRequired, profilesNameTaken,
  profilesPassword, profilesPasswordKeep, profilesTotp, profilesBoundSummary`。

## 6. 安全

- 密钥永不回显：list/save/get 返回只含布尔位；错误信息不含密钥值。
- 文件 0600 + 原子写；不落日志（密钥字段只进 stdin 管道与内存）。
- profile 内容不参与 shell 拼接（密码走 stdin、提示词仅用于匹配），无注入面。
- 依赖既有门禁不变：只读连接拒 sudo、`quick_sudo=false` 拒 exec sudo、写分块上限。

## 7. 测试计划（完成定义四件套）

1. **单测**（cargo test，不连 SSH）：
   - sudo_profiles：CRUD、更新空密码保持原值、显式清除、名称唯一/为空/超长拒绝、
     上限 20、损坏文件回退空库、ProfileView 序列化不含密钥、delete 级联清 bindings、
     apply_profile/effective_use_pty 覆盖语义。
   - mcp：tools 清单含三个新工具、save→list→delete 回环（临时 data_dir）、
     `ssh_exec_sudo` schema 含 `quickSudoProfile`、按名称/id 解析。
2. **smoke**：smoke_fs_test.py 增加 profiles 会话无关用例（list 空 → save → list →
   重复名报错 → update 保持密钥 → delete；响应全文不含密钥明文）；smoke_mcp.py 增加
   `ssh_quick_sudo_profiles_list` 调用。未注册时 SKIP 不 FAIL。
3. **对标/清单**：FEATURE_PARITY L41 改 ✅（推翻 2026-08-29「不做」，注明本文档）；
   PROTOCOL 增补 §2 方法与 settings 字段；PROGRESS 收尾记录。
4. **七语**：supplemental 键集 parity 测试 + `pnpm typecheck`。

## 8. 里程碑

| # | 任务 | 验收 |
| --- | --- | --- |
| M1 | sudo_profiles.rs（存储/CRUD/视图/单测） | cargo test 通过 |
| M2 | 解析接通（exec/ssh.rs/open_session/settings_set/refresh） | cargo test 通过 |
| M3 | main.rs 注册 + MCP 工具 | cargo test + mcp 单测 |
| M4 | 前端 UI + i18n 七语 | typecheck + vitest |
| M5 | smoke 用例 + 文档 + manifest 0.3.0 | scripts/test.sh 全绿 |

## 9. 风险与备注

- **明文落盘**：本期接受（与 tiny-rdm 未启用 local-vault 等价，信任域为本机用户）；
  后续可演进 OS keyring（keyring crate）或宿主全局 secret 通道，存储层已收敛在
  单模块内，替换成本低。
- **名称引用**：MCP 支持按名称引用便于人写；同名被删后引用失败报错明确
  （"Quick Sudo profile '…' not found"）。
- **绑定悬挂**：profile 删除级联清 binding；绑定指向已删除 profile（如手工改文件）
  时按无绑定处理（回退本连接），不报错不阻断连接。

## 10. 增补：入口桩（2026-08-30 晚，随 0.3.2）

用户反馈全局设置入口不可见。核查宿主：贡献点枚举仅三种，无插件级设置页；
`connection-provider.actions`（连接表单动作）是宿主面板上唯一可挂的控制桩。

- 工作台工具栏新增钥匙按钮直达管理弹窗（完整 CRUD，无会话依赖）——主入口。
- 连接表单动作 `quick-sudo-profiles`：点击 → 宿主 `connection/action` →
  插件返回配置清单 + 绑定状态纯文本摘要（`{message, fieldValues: null}`，
  密钥只报 set/not-set），并提示完整管理入口位置——面板上的可发现桩。
- 交互式多步管理（新建/编辑/删除）无法经单次 message 往返承载，仍归工作台 UI。

里程碑 M6：action_summary 纯函数 + main.rs 接线 + manifest actions 七语 +
smoke 用例 + `dbx-plugin package` schema 校验。
