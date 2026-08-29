# 测试基线盘点与文档收口（S-C，2026-08-28 全量收口版）

> **本版定位**：S-A（后端增强，含第二轮 metrics inode/topMemory、sftp/read offset、
> 快照缓存）与 S-B（前端 OSC 633 / session-status / 输出净化）均已完成后，S-C 执行的
> **合流后全量收口基线**。本版取代并发期间的瞬时快照版（当时 cargo 98→100、vitest 21
> 存在并发漂移）；本版所有数字均为 S-A/S-B 停止改动后的稳定复测值，无并发漂移。
> S-C 全程只读源码（backend/、frontend/、scripts/），仅写入自己所有的文档；
> 未执行任何 git commit/push。

## 1. 环境与元信息

| 项 | 值 |
| --- | --- |
| 机器 / 平台 | darwin 25.6.0 arm64（macOS） |
| 工作区 | `main` 分支 @ 86b4ca8 + 未提交 batch3/S-A/S-B 改动（未 commit） |
| 工具链 | cargo/rustc 1.88.0（russh 0.60.3）、node v22.21.0、pnpm 10.27.0、vitest 4.1.10、dbx-plugin CLI 0.1.0 |
| SSH 测试容器 | `dbx-ssh-test`（linuxserver/openssh-server）Up 13 hours，127.0.0.1:2222 通 |
| sidecar 二进制 | `backend/target/release/dbx-plugin-ssh`（DBX_PLUGIN_SIDECAR 注入 smoke；构建时 0 重编译，与源码同快照） |

## 2. 基线结果表

| # | 套件 | 结果 | 明细 |
| --- | --- | --- | --- |
| 1 | backend `cargo test` | ✅ 全绿 | **107 passed / 0 failed / 0 ignored**（0.30s；复核 S-A 报告值 107，一致）。新增覆盖：metrics 快照缓存 2 用例（`metrics_snapshot_cache_serves_and_cleans_per_session`、`cached_metrics_payload_marks_age_without_mutating_the_snapshot`）、inode/topMemory 解析 3 用例（`parses_inode_usage_across_df_dialects`、`inode_usage_merges_into_matching_mounts_only`、`top_memory_processes_are_parsed_independently`）、`optional_u64_falls_back_on_missing_or_invalid`、MCP tools/list 断言 `sftp_read_file` schema 含 `offset` |
| 2 | frontend `pnpm typecheck` | ✅ | vue-tsc --noEmit exit 0 |
| 3 | frontend `pnpm test` | ✅ | vitest：2 个 spec 文件 **35/35**（workbench **29** + appearance 6；复核 S-B 报告值 35，一致）。workbench 内含 S-B 新增 describe：OSC 633 命令标记 8 用例、session status 语义 3 用例、输出净化 3 用例 |
| 4 | frontend `pnpm build` | ✅ | 自包含 `ui/index.html`（2.29MB）产出；chunk>500kB 容量告警为既有提示，非错误 |
| 5 | `scripts/smoke_test.py` | ✅ PASS | 全链路（connect→挑战→PTY→echo→SFTP 读写往返→关闭）1.2s，真机容器 |
| 6 | `scripts/smoke_fs_test.py` | ✅ | **PASS 17 / SKIP 0 / FAIL 0**（sftp_ext 6 + sudo_fs 9 + keys 2） |
| 7 | `scripts/smoke_mcp.py` | ✅ | initialize / tools/list **19 tools** / 参数校验 / call 往返全绿 |
| 8 | `scripts/smoke_batch3_test.py` | ✅ | **12 passed / 0 skipped / 0 failed**（6.1s，真机）：ssh/metrics 网络接口、Top 进程、**inode 使用率**、**快照缓存**（cached 现采/命中 cachedAt）、**sftp/read offset 分页**（head 截断/offset 16 续读/超界空读）、sessions/list、knownHosts marker、sftp/copy 新建/拒绝覆盖/覆盖/批量、sftp/move |
| 9 | `dbx-plugin package .` | ✅ | `dist/io.dbx.ssh-0.2.2-darwin-arm64.dbxp`（4,267,311 B，sha256 `173985ff9a43a4eeb4fc60f30519e99e259e79e123bd52d0a55774e0e82d4713`）；包内 5 文件齐：`manifest.json`、`ui/index.html`（2,291,158 B）、`bin/darwin-arm64/dbx-plugin-ssh`（8,075,616 B）、`assets/plugin.svg`、`checksums.json` |

备注：

- 全部 smoke 按脚本 SKIP 语义真跑，无任何段因环境缺失而 SKIP（容器可用、sidecar 注入成功）。
- `dbx-plugin package` 以 CLI 自身参数重编 release（本次 1m02s，覆盖 `backend/target` 产物）——
  与上一版基线相同的行为，smoke 与出包为两次编译但同一源码快照。正式验收仍建议按 SKILL.md
  双冒烟流程对**安装路径**下的二进制再跑一遍。
- `scripts/test.sh` 的宿主安装管线段（PluginPackageInstaller + MCP bridge 集成）**仍未纳入基线**：
  需 `../dbx-plugin-host-worktree` 且 bridge 测试处于 WIP-SKIP 状态，留待主会话集成验收。

## 3. 失败分类（A 半成品 / B 回归 / C 环境缺失 / D 真实缺陷）

**结论：0 失败。A/B/C/D 四类均无命中项**；全部套件一次通过，无需「失败重跑一次再定论」仲裁。

观察项（非失败，不阻塞）：

| # | 级别 | 观察 | 证据（file:line） | 建议 | 状态 |
| --- | --- | --- | --- | --- | --- |
| O1 | 显示瑕疵 | `smoke_test.py` 打印 SFTP 条目类型为 `None`：脚本读 `fileType`/`type` 字段，wire 字段实为 `kind` | scripts/smoke_test.py:153 vs backend/src/sftp_ext.rs:45（`"kind": kind`） | 改用 `entry.get("kind")`（smoke_fs_test.py 的 `entry_kind()` 是正确写法）；顺带给 `sftp/list` 加断言（当前只打印） | **仍未修**（scripts/ 非 S-C 所有权，本轮不修任何代码） |
| O2 | 文档过时 | MCP.zh-CN.md「工具一览 17 个」实际 19 个 | backend/src/mcp.rs `sftp_copy`/`sftp_move` | 补两行、17→19 | **已解决**：复核确认 docs/MCP.zh-CN.md:56 现为「工具一览（19 个）」且含 `sftp_copy`/`sftp_move` 行（S-A 已改） |
| O3 | 文档过时 | FEATURE_PARITY 头部方法臂数过时 | backend/src/main.rs 分发臂 | 修正数字 | **已解决**：并发版已改「约 60」；本轮复核实际为 **68 个分发方法臂**，已按 68 修正 |
| O4 | 覆盖缺口 | smoke_batch3 的 copy/move 只覆盖**文件级**；目录级递归 copy、`from` 单字符串形态无真机用例（单测层已覆盖 `parse_request_accepts_single_string_and_array`、same_directory/rename candidates） | scripts/smoke_batch3_test.py 用例清单；backend/src/sftp_copy.rs 单测 | 下批 smoke 补目录递归/单字符串/覆盖冲突真机用例 | 开放 |
| O5 | 流程注意 | 打包会覆盖 `backend/target/release` 二进制，并发跑 smoke 与 package 有二进制替换窗口 | 本次打包日志「Compiling … 1m02s」 | 维持 test.sh 顺序（构建→打包→smoke），勿并行 | 流程约定，长期有效 |

## 4. 对标清单核对收口（本轮 diff）

### 4.1 逐项核对结论

以读到的当前代码为准，逐项复核 FEATURE_PARITY / BATCH3 / TEST_MATRIX 勾选与实现：

- **既有 ✅ 项**（SFTP 基础、diskUsage、PTY/回放、exec+sudo、ZMODEM、known_hosts、
  主机密钥、keys/discover、Stat/Exists/Touch/WriteFile、Archive/Extract、Sudo 文件族 10 方法、
  终端缓冲/命令中止、MCP settings、隧道整合）：全部有 `main.rs` 方法臂 + 单测或真机 smoke
  支撑，**无错勾**。
- **S-A 完成但未勾项（本轮补齐）**：
  - `ssh/metrics` 快照缓存（`GetLastSnapshot` 对齐）：ssh.rs:1823/581、main.rs:345 —— cargo 2 用例 + smoke_batch3 真机；
  - metrics `inodeUsePercent` + `topMemory`：metrics.rs:76/103 + 双方言解析单测 —— smoke_batch3 真机；
  - `sftp/read` 可选 `offset` 分片续读（`ReadFile(offset,length)` 对齐）：ssh.rs:1656-1666、main.rs:205、mcp.rs:516/1102 —— smoke_batch3 真机三段。
- **S-B 完成但未勾项（本轮补齐）**：
  - OSC 633 命令标记（对标 tiny-rdm `frontend/src/modules/ssh/osc633-parser.js`，文件实存已核）：
    `frontend/src/lib/terminalCommandMarkers.ts` + App.vue:56/352/796/2682 —— vitest 8 用例；
  - 会话状态展示（对标 `session-status.js`）：`frontend/src/lib/sessionStatus.ts` + App.vue:57/440/2612 —— vitest 3 用例；`reconnecting` 为插件扩展；多会话择优（`findPreferredSshSession`）不适用未移植（单 workbench 单连接，S-B 报告结论复核一致）；
  - 命令输出净化（对标 `terminal-output.js`）：`frontend/src/lib/terminalOutputText.ts` + App.vue:58/441 —— vitest 3 用例；`.mcp_ctl_*` hook 特判不适用未移植（本插件无该注入路径）。
- **七语核对**：`terminalCommand` / `sessionStatus` 两组新 key 在 i18n.ts 的 7 个语言块
  各出现一次（terminalCommand: L64/269/468/667/866/1065/1270；sessionStatus 同构）；
  workbench.spec.ts 的七语 requiredKeys 循环覆盖新 key，35/35 通过即为自动化证据。
- **TEST_MATRIX**：原 Windows 平台矩阵为历史验收记录，本机不可复核，保持不动；
  「2026-08-28 本机基线」段更新为合流后终值（107/35/12/12、出包 sha256）。

### 4.2 本轮文档改动清单（收口 diff）

| 文件 | 改动 |
| --- | --- |
| `docs/FEATURE_PARITY.zh-CN.md` | ① 头部「约 60 个方法臂」→「68 个分发方法臂」（O3 终值）；② `SSH 指标` 行补网络速率/Top CPU+内存进程/inode/快照缓存；③ `SFTP 基础` 行补 `offset` 分片续读；④ 新增 3 行：OSC 633 命令标记、会话状态展示、命令输出净化（S-B）；⑤ 「第三批任务」段补合流后增强说明与终值基线（107/35/12） |
| `docs/FEATURE_PARITY_BATCH3.zh-CN.md` | 文末新增「## 合流后增强核对（2026-08-28 晚，S-C 全量收口复核）」：6 行增强项 ×（对标 / 落地证据 file:line / 验证命令）明细表，并把集成验证段落与终值基线互链 |
| `docs/TEST_MATRIX.zh-CN.md` | 「2026-08-28 本机基线」段：并发快照措辞 → 合流后全量收口；cargo 100→107、vitest 21→35、smoke_batch3 7→12、出包补 sha256；「下一批建议」同步（MCP 17→19 已完成移除） |
| `docs/PROGRESS-TESTBASELINE.zh-CN.md` | 本文件整体重写为全量收口版 |

未改动：docs/MCP.zh-CN.md、docs/PROTOCOL.zh-CN.md（S-A 所有权，已由 S-A 同步到位，本轮仅复核）。

## 5. 下一批增强建议 top5（按价值排序）

**插件仓内可独立完成：**

1. **smoke_batch3 补目录级与边界真机用例**（O4）：`sftp/copy` 目录递归、`from` 单字符串形态、
   `move` 目标存在冲突分支。sftp_copy.rs 纯函数单测已覆盖解析/候选路径，但 wire 层目录递归
   只有单测没有真机段；成本一次容器会话内可完成。
2. **i18n 全量七语对齐断言**：现有 spec 只断言关键 key；建议从 i18n.ts 导出 `messages`
   （dev-only export）后在 vitest 里遍历比较 7 语 key 集合，把「漏一语 typecheck 不报」的坑
   永久关掉。改动约 1 行 export + 1 个用例，价值随功能增长线性上升。
3. **smoke_test.py `kind` 字段修正 + `sftp/list` 断言**（O1）：`entry.get("kind")` 对齐 wire
   契约并消除每次运行的 `None` 噪音；顺带把打印改为断言。半小时内完成。
4. **终端命令标记条「运行中」时长实时 tick**：S-B 遗留项——标记条目前只在 D 帧（命令结束）
   后显示最终时长；加一个 UI tick 定时器即可在运行中显示跳动时长（对标 tiny-rdm 体验）。
   纯前端小改。
5. **sudo 保活与 OTP 防重放的端到端真机 smoke**：当前仅 exec.rs 纯函数单测；可在容器上验证
   「长连接 4 分钟后续期」「同码二次提交跳过注入」的端到端行为。需可控时钟或接受 4 分钟等待，
   成本中等，属安全语义的关键路径补验。

**依赖宿主 / 环境 / CI（单列，不进 top5 排序）：**

6. 宿主管线集成回归：`scripts/test.sh`（不带 --skip-host）跑通 PluginPackageInstaller +
   dbx-mcp `plugin_tools_bridge`（当前 WIP-SKIP；宿主补丁合流后转必跑）。
7. DBX Web/Docker 模式 e2e：fileTransfer 缺失时浏览器兜底（File API 上传 / Blob 下载）的
   真实宿主环境验证（对应 TEST_MATRIX「待启动集成环境」行）。
8. 长稳与大文件：30 分钟会话、序号补发、100 MiB/1 GiB SHA-256 校验（真实网络与时间预算）。
9. 五平台包矩阵：Windows x64、macOS x2、Linux x2 工作流触发与安装冒烟（CI 依赖）。
10. 多会话语义（batch-send / 分屏 / 会话择优）：架构级，需宿主先提供多会话 API；S-B 已明确
    不移植 `findPreferredSshSession`，登记为宿主依赖项长期跟踪。

## 6. 遗留

- 全部改动（batch3 + S-A + S-B + 本轮文档）**未 git 提交**（硬性约束：S-C 禁止 commit/push）；
  请主会话审阅 diff 后统一收口提交。
- 宿主 worktree 集成段（PluginPackageInstaller + plugin_tools_bridge）未验证（§2 备注 / §5.6）。
- 容器 `AllowTcpForwarding yes` 的隧道语义真机验证未覆盖（宿主传输层整合路径，归宿主联调）。
- O1（smoke_test.py kind 字段）与 O4（目录级 smoke 用例）开放给下批，本轮只诊断不修。
- mock 页需 vite dev server 才能渲染（裸模块说明符，静态 http 服务白屏）——fixture 属性而非
  缺陷；主会话终验时注意用 `pnpm --dir frontend dev` 打开。
