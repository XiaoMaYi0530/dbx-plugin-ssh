# B-SSH-COLLECT 收口报告（对标清单与测试矩阵终值，2026-08-29）

> 本轮为 B-SSH-COLLECT 收口路：对「未提交 batch3 + 历轮增强（S-A/S-B/X-B/A-SSH）」工作区做
> 基线复核、对标清单逐项补勾与测试矩阵终值刷新。全程只读源码（backend/、frontend/、scripts/），
> 仅写入 docs/ 收口文档；未执行任何 git commit/push；无代码改动、无新依赖。

## 1. 基线终值表（2026-08-29 全量复测，一次通过、无重跑）

| # | 套件 | 终值 | 明细 |
| --- | --- | --- | --- |
| 1 | backend `cargo test` | ✅ **109 passed / 0 failed / 0 ignored**（0.27s） | 历轮增量：ReplayBuffer 5 MiB 压测（P-SSH）、`auth_method_names_round_trip_for_display` + `session_info_payload` authMethod 扩展（A-SSH）、metrics 快照缓存 2 + inode/topMemory 3 + optional_u64（S-A） |
| 2 | frontend `pnpm typecheck` | ✅ vue-tsc --noEmit exit 0 | — |
| 3 | frontend `pnpm test` | ✅ vitest **51/51**（workbench 45 + appearance 6，285ms） | 含 OSC 633 八用例、session status 三用例、输出净化三用例、i18n 七语 key/占位符全对齐两用例、时长 tick、命令历史 4、快速命令 3、authMethod 2、zoom clamp |
| 4 | frontend `pnpm build` | ✅ 自包含 `ui/index.html`（2,317,270 B） | chunk>500kB 容量告警为既有提示，非错误 |
| 5 | `scripts/smoke_test.py` | ✅ PASS 1.1s（真机容器 dbx-ssh-test，127.0.0.1:2222） | 已含 `sftp/list` kind 取值域断言（X-B，O1 关单） |
| 6 | `scripts/smoke_fs_test.py` | ✅ PASS 17 / SKIP 0 / FAIL 0 | sftp_ext 6 + sudo_fs 9 + keys 2 |
| 7 | `scripts/smoke_mcp.py` | ✅ initialize / **19 tools** / 参数校验 / call 往返全绿 | MCP.zh-CN.md 已同步 19 |
| 8 | `scripts/smoke_batch3_test.py` | ✅ **17 passed / 0 skipped / 0 failed**（6.2s） | 12→17（X-B 目录级 5 例）；含 sftp/read offset 三段、metrics 网络/Top 进程/inode/快照缓存、sessions/list authMethod 断言（A-SSH） |
| 9 | `scripts/smoke_sudo_otp_test.py` | ✅ **10 passed / 0 skipped / 0 failed**（29.6s） | sudo 保活/OTP 编排端到端真机（TESTBASELINE §5.5 关单） |
| 10 | `dbx-plugin package .` | ✅ `dist/io.dbx.ssh-0.2.2-darwin-arm64.dbxp`（**4,290,327 B**） | **sha256 `a67bb683a2db1ee67e37ae8b6d96447a44fe160fb07e55b459289a450edf347e`**；包内 5 文件齐（manifest.json / ui/index.html / bin/darwin-arm64/dbx-plugin-ssh 8,112,320 B / assets/plugin.svg / checksums.json）；出包后对重编二进制复跑 smoke_test PASS 1.2s |

环境与口径：

- 工具链：cargo/rustc 1.88.0（russh 0.60.3）、node v22.21.0、pnpm、vitest 4.1.10、dbx-plugin CLI 0.1.0。
- 全部 smoke 显式 `DBX_PLUGIN_SIDECAR=backend/target/release/dbx-plugin-ssh`（本地构建产物），
  无任何段因环境缺失 SKIP。
- **出包注意（新）**：`/usr/local/bin` 下存在旧 cargo 1.69.0，`dbx-plugin package` 从 PATH 解析 cargo；
  PATH 未置 `$HOME/.cargo/bin` 在前时会因 Cargo.lock v4 报 `lock file version 4 ... does not understand`
  假错。出包前必须 `export PATH="$HOME/.cargo/bin:..."`（本轮已实锤并绕过）。
- 性能基线沿用 `PROGRESS-P-SSH.zh-CN.md` §3 真机数据（本轮未重跑 perf）：终端 PTY 流灌入
  **71.9 MiB/s**（5 MiB 环形缓存）；SFTP 上传（网络）**220–237 MB/s**（本地 spool 633.9–1078 MB/s）；
  SFTP 下载 **113–118 MB/s**；replay 2.00 MiB / 601 帧 ≤ 2 MiB 上限；SHA-256 校验均一致。

## 2. 方法臂数复核（O3 口径修正）

- 复核结论：`backend/src/main.rs` 的 `handle_request` 分发表为 **67 个分发方法臂、68 个方法名**——
  `"ssh/host-key/resolve" | "connection/challenge/resolve"` 共用一臂（main.rs:174）。
- 此前文档「68 个分发方法臂」的口径实际统计的是**方法名数**（68），本轮修正为「67 臂 / 68 名」，
  `docs/FEATURE_PARITY.zh-CN.md` 头部已更新。
- 68 个方法名按域清点（臂=方法名，host-key/resolve 双名臂 +1）：connection 3、ssh 18、sftp 24、
  sudo 11、filesystem 6、keys 1、mcp 4、workbench 1，合计 67 臂 / 68 方法名。

## 3. 对标清单收口 diff 摘要（本轮文档改动）

### 3.1 docs/FEATURE_PARITY.zh-CN.md

| 改动 | 证据 |
| --- | --- |
| 头部方法臂口径修正（68 臂 → 67 臂 / 68 方法名，注明双名臂 main.rs:174） | §2 |
| OSC 633 行补 X-B「运行中时长 1s tick」 | terminalCommandMarkers.ts `runningCommandElapsedMs` + App.vue:389-393/879 |
| 新增 4 行：命令历史、快速命令栏、连接信息面板（authMethod 契约增量）、终端字体缩放（A-SSH） | commandHistory.ts:5/12/26/42、quickCommands.ts:10/23/42/60 + App.vue:2430、model.rs:46 + ssh.rs:599/609/1236/1249 + connectionInfo.ts:17、terminalZoom.ts:6 |
| 「第三批任务」段落终值更新：补 X-B 四项与 A-SSH 四项，终值 109/51/五 smoke 全绿/出包 sha256 | §1 |

历轮已勾项复核（无错勾）：SFTP 基础（含 offset）、diskUsage、PTY/回放、exec+sudo、ZMODEM、
OSC 633、session-status、输出净化、known_hosts、主机密钥预检、keys/discover、Stat/Exists/Touch/
WriteFile、Archive/Extract、Sudo 文件族 10 方法、终端缓冲/命令中止、metrics 增强（网络/Top 进程/
inode/topMemory/快照缓存）、MCP settings、隧道整合——均对码成立。

### 3.2 docs/FEATURE_PARITY_BATCH3.zh-CN.md

| 改动 | 内容 |
| --- | --- |
| 新增「X-B 轮核对」节 | smoke kind 修正 + sftp/list 断言（smoke_test.py:153-159）、i18n 七语 key/占位符全对齐断言（workbench.spec.ts:111/129 + es/it/ja/pt-BR 各补 9 个 supplemental key）、smoke_batch3 目录级用例 12→17（smoke_batch3_test.py:511-519 等 5 例）、终端标记条运行中时长 tick |
| 新增「A-SSH 轮核对」节 | 对齐前几轮格式：命令历史 / 快速命令 / 连接信息（authMethod 契约增量）/ 字体缩放绝对字号钳制，四行 ×（对标 / 落地证据 file:line / 验证）明细表 |

### 3.3 docs/TEST_MATRIX.zh-CN.md

- 「2026-08-28 本机基线」段整体刷新为「2026-08-29 本机基线（全轮收口终值）」：
  cargo 107→**109**、vitest 35→**51**、smoke_batch3 12→**17**、新增 smoke_sudo_otp 10/10 行、
  新增性能基线行（71.9 MiB/s / 220–237 / 113–118 MB/s）、出包 sha256 更新为 `a67bb683…f347e`。
- 「下一批建议」段更新：仓内五项（kind 修正/目录用例/i18n 断言/时长 tick/sudo+OTP smoke）全部标注
  已完成关单；余量为宿主/CI 依赖项单列。

## 4. 下一批建议

### 4.1 仓内可独立完成（top3）

1. **mock 走查 Playwright 冒烟脚本化**：`frontend/mock.html`（mockDbxHost 夹具）目前靠人工 vite serve +
   浏览器走查（A-SSH 截图六张）；建议落一个 `scripts/smoke_ui_mock.py`（Playwright 无头）覆盖
   工具栏入口/快速命令/连接信息/缩放四条路径，纳入 test.sh，防 UI 回归零成本巡检。
2. **出包后安装副本双冒烟固化**：本轮出包后已手工对重编二进制复跑 smoke_test；建议把
   「package → 对 dist 包内二进制（解包或安装路径）复跑 smoke_test + smoke_fs」固化为
   `scripts/package_and_verify.sh`，关掉 PROGRESS-TESTBASELINE O5 的二进制替换窗口风险。
3. **性能基线纳入例行收口**：`scripts/perf_baseline_test.py`（3 案例 0 SKIP）目前只在 P-SSH 轮跑过；
   建议每轮收口必跑并回填 TEST_MATRIX 性能行（71.9 MiB/s / 220–237 / 113–118 MB/s 作为回归阈值 ±20%）。

> **关单记录（patrol 2026-08-29）**：建议 2 ✅ `scripts/package_and_verify.sh`
> （打包→从 .dbxp 提取二进制→smoke_test/smoke_fs/smoke_batch3 三份实跑全绿，O5 替换窗口关闭）；
> 建议 3 ✅ `scripts/test.sh` 新增 live smoke + perf 例行段（容器门控 + SKIP 语义；perf 首跑
> 69.6 MiB / 220.2 / 101.8 MB/s，均在 ±20% 阈值内）。
> 建议 1 ✅ `scripts/smoke_ui_mock.mjs`（vite dev + playwright-core(仓外 /tmp 安装，项目零新依赖) + 系统 Chrome 无头；
> 断言 session-pill/terminal-host/terminal-pane/toolbar-actions/toolbar-separator 五锚点 + 截图留档 docs/screenshots-ui-mock/；
> 依赖缺失自 SKIP；已接入 test.sh 自门控段；实跑全绿）。

### 4.2 依赖宿主 / 环境 / CI（单列，不进排序）

4. 宿主管线集成回归：`scripts/test.sh` 不带 `--skip-host`（PluginPackageInstaller +
   `plugin_tools_bridge` MCP 桥，当前 WIP-SKIP；宿主合流后转必跑）。
5. DBX Web/Docker 模式 e2e：fileTransfer 缺失时浏览器兜底（File API 上传 / Blob 下载）真实宿主验证。
6. 长稳与大文件：30 分钟会话、序号补发、100 MiB / 1 GiB SHA-256（真实网络与时间预算）。
7. 五平台包矩阵：Windows x64、macOS x2、Linux x2 工作流触发与安装冒烟。
8. 架构级宿主依赖：多会话语义（分屏/批量发送/会话择优）与端口转发 -L/-R 归属决策；
   隧道语义真机验证（容器 `AllowTcpForwarding yes`，宿主传输层整合路径）。

## 5. 遗留

1. 全部改动（batch3 + S-A/S-B/X-B/A-SSH + 本轮文档）**未 git 提交**（硬性约束）；请主会话审阅 diff
   后统一收口提交。
2. 宿主集成段（§4.2.4）与隧道真机验证未覆盖，归宿主联调。
3. 上一轮出包 sha256 `173985ff…4713`（2026-08-28）与本轮 `a67bb683…f347e` 差异来自 A-SSH/X-B
   增量二进制与 ui 产物，属预期演进，非构建不可复现。
4. 出包 PATH 坑（§1 出包注意）建议在 SKILL.md「常见故障速查」补一行（SKILL.md 非本轮所有权，留主会话）。

## 配置项显隐/必填核查 + MCP 面核对（2026-08-29 任务轮）

- **显隐/必填搭配核查结论（逐项对照 manifest ↔ model.rs from_lifecycle_params）**：
  现有搭配整体正确——password/private_key_path 的 required_when 与后端凭据校验一致；
  agent_socket 合法可选（dial 有 SSH_AUTH_SOCK 环境回退，ssh.rs:2742）；sudo_password/
  sudo_use_pty 门在 quick_sudo 下正确；**totp_secret/auth_flow_mode/两个 prompt hint
  必须保持常显**（服务登录期 keyboard-interactive 2FA，sudo_auth_for 同时用于登录认证，
  ssh.rs:1126），不能并入 quick_sudo 显隐。唯一搭配微调：read_only 描述补明
  「Quick Sudo stays off while enabled」（read_only 时 auto_sudo 服务端禁用）。
- **新增契约测试** `manifest_connection_fields_stay_in_sync_with_parsing`（直读
  manifest.json）：字段清单镜像、secret binding 白名单、required_when 链、quick_sudo
  显隐门、2FA 字段常显五组断言，防后续漂移。
- **MCP 面核对**：工具 schema 补齐 JSON type 三元组收口（并发会话半途的
  connection_properties 重构由本轮完成，19 工具全部带类型描述）；写工具门禁核验——
  `is_write_tool` × `registered_connection_is_read_only` 对 read_only 连接拒绝
  ssh_exec_sudo/sftp_write_file/mkdir/remove/rename/chmod/copy/move，与工作台
  ensure_writable 同构（mcp.rs:866-880），普通 ssh_exec 保持只读会话可用。
- **验证**：cargo **116 passed**（+6）、smoke 全链 PASS、前端 typecheck/vitest 51/
  build 绿、smoke_ui_mock 走查全绿（docs/screenshots-ui-mock/ 刷新）。
- 遗留：ssh 容器段真机验证需容器环境（本会话仅 fs/memory 段可跑）；宿主 e2e 另行安排。

## 第 3 轮对抗式审查与修复（2026-08-29 任务轮）

- **settings/set 运行时覆盖输入校验**：发现 prompt hint 无清洗/限长——控制字符、
  ANSI 序列可经 `ssh/settings/set`（及宿主下发的 external_config）直入存储连接与
  全部活会话，而 `classify_auth_prompt` 只对 prompt 侧做归一化，带控制字符的 hint
  永远匹配不上，自动应答静默失效。修复：exec.rs 新增 `sanitize_prompt_hint`
  （剥 ANSI/控制字符 + `MAX_PROMPT_HINT_LEN=512` 按 char boundary 截断），
  settings_set 的 hint 路径与 `sudo_auth_for`（连接路径）统一走清洗；
  `classify_auth_prompt` 对存量 hint 同步归一化（双保险）。auth_flow_mode 非法值
  （`AuthFlowMode::parse` 兜底）、totpSecret 畸形（`parse_totp_secrets` 逐行过滤、
  多行混杂保留合法行）、sudoPassword 空白（清除覆盖回退登录密码）核验为安全降级，
  补显式对抗单测钉住行为。
- **jump chain（跳板链）边界**：三处真实缺口——① host 含内部空格/URI 语法
  （`ssh://`）被静默接受成坏 TCP 目标（新增 `validate_host_field`，目标 host 同样
  生效，fail fast）；② 同 host:port 重复跳板未拒（链不会推进，纯误配置，
  parse_jump_hosts 判重拒绝）；③ 跳板 password/private_key_passphrase 走
  `optional_string` 被 trim，首尾空格凭据被静默改写（新增 `credential_string`
  原样读取；连接级 private_key_passphrase 同修）。port 0/65536/超界、空 username
  已有校验，补对抗断言。
- **known_hosts 解析边界**：marker 识别从「任意 `@` 前缀」收紧为
  `@cert-authority`/`@revoked` 白名单（伪造 marker 行按字面首 token 匹配，
  消除 nth(1) 误删面）；新增 `MAX_KNOWN_HOSTS_LINE_LEN=8192`，超长行在 list
  跳过、在 remove 原样保留（防巨行解析放大与误删）。hashed `|1|…` 条目保持可见、
  CRLF/空行/注释/畸形 key/无 key 行均不崩溃不误删（对抗测试覆盖）。
- **前端会话韧性**：① `loadPathHistories` 只查 `typeof object`，数组/数字/
  含非字符串元素的历史会污染响应式状态——新增 `sanitizePathHistories` 纯函数
  逐条校验+限长，spec 覆盖坏 JSON 与结构漂移；② zmodem 传输中关闭/重开会话
  原样残留 busy 状态与死 sentry（zmodemBusy 吞掉后续终端输入）——新增
  `cancelZmodemUpload` 静默收尾，挂入 openSession/closeSession，`finishZmodemUpload`
  复用且取消后的迟到 abort 报错静默；③ `pendingTerminalFrames` 乱序缓冲无上限，
  加 `TERMINAL_PENDING_FRAME_LIMIT=1024` 溢出清空并走既有 replay 重投。
- **验证**：cargo **125 passed**（+9：exec 4、model 3、host_key 2）；
  `python3 scripts/smoke_test.py` 全链 PASS（本机容器环境可用，ssh 会话/SFTP
  往返实跑非 SKIP）；前端 typecheck/vitest **52**（+1）/build 绿；
  `node scripts/smoke_ui_mock.mjs` 全绿；`scripts/test.sh` 汇总 all green。
- 遗留/不修项：① 连接级 `sudo_password` 仍经 `optional_string` trim（与
  `SudoAuth::new` 的「空白即回退登录密码」降级语义耦合，改动影响面大于收益，
  记录不改）；② host 字段 IPv6 裸地址/域名字符集不做白名单（dial 侧已失败可控，
  过度收紧有误伤风险）；③ `attachSession` 退避循环依赖宿主 workbenchState.sessionId
  判同会话重试，语义系前轮设计，本轮未动；④ 无外来半成品干扰：开工基线
  cargo 116 绿，工作区 unstaged 改动均为前两轮与本轮的已验证产物。

## 插件更名 ssh-sftp → ssh + 遗留修复（2026-08-29 任务轮 4）

- **更名**（用户指示）：目录 `ssh-sftp/`→`ssh/`；插件 id `io.dbx.ssh-sftp`→`io.dbx.ssh`
  （manifest 24 处/脚本 8 文件/main.rs 4 处）；crate/binary `dbx-plugin-ssh-sftp`→
  `dbx-plugin-ssh`（Cargo.toml + dbx-plugin.toml binary + manifest entrypoints.executable）；
  前端包名 `@xynanan/dbx-ssh-ui`；MCP serverInfo name `dbx-ssh`；日志前缀 `[ssh-sftp]`→
  `[ssh]`。**协议方法前缀 `ssh/*`、`sftp/*` 保持不变**（SFTP 是独立协议域，避免破坏性
  协议变更；MCP 工具名同理）。repo 级 AGENTS.md/README/skills/shared 活文档同步；
  files/ldap 源码注释级历史对齐说明保留旧名（不影响功能）。跨插件无功能性 id 依赖
  （grep 证实，全部为注释）。
- **遗留修复**：连接级 `sudo_password` 解析不再 trim（第 3 轮遗留；沿用 credential_string
  原样读取，纯空白仍由 SudoAuth「空白即回退登录密码」兜底）+ 回归单测。
- **验证**：cargo **127 passed**（+2）、test.sh 全绿（backend/frontend/package/MCP smoke/
  mock walkthrough）；host 安装段 SKIP——宿主 worktree 的 plugin_tools_bridge WIP 未编译
  （前轮已登记的宿主侧前置项，正好佐证 MCP 桥在建）。
- 残留注意：用户级 skill `~/.zcode/skills/dbx-ssh-sftp-dev` 与本仓 `.zcode` 内 skill 已更名
  `dbx-ssh-dev`（仓内）；用户级目录如需同步请手动更名。旧安装的 `io.dbx.ssh-sftp` 插件
  记录需卸载后装新包（id 已变）。
