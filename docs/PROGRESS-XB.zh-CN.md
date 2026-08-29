# X-B 路交付报告（下一批增强建议 top5 实施）

日期：2026-08-28。工作目录：`~/btroot/dbx-plugins/ssh-sftp`，在未提交 batch3 + S-A/S-B/X 增强改动之上继续；
仅触碰 `scripts/smoke_test.py`、`scripts/smoke_batch3_test.py`、`frontend/src/**`（i18n.ts / workbench.spec.ts /
terminalCommandMarkers.ts / App.vue / style.css）、`docs/PROGRESS-XB.zh-CN.md`（本文件，新建）。
未执行任何 git commit/push；无新依赖。

## 0. 总体验证基线（本轮终值）

| 套件 | 结果 |
| --- | --- |
| backend `cargo test` | ✅ **107 passed / 0 failed**（0.26s；本轮后端零改动，与 S-C 基线一致） |
| frontend `pnpm typecheck` | ✅ vue-tsc --noEmit exit 0 |
| frontend `pnpm test` | ✅ vitest 2 个 spec **38/38**（workbench 32 + appearance 6） |
| frontend `pnpm build` | ✅ 自包含 `ui/index.html` 产出（chunk>500kB 容量告警为既有提示） |
| `scripts/smoke_test.py` | ✅ PASS（1.1s，真机容器；含新增 `sftp/list` 断言） |
| `scripts/smoke_fs_test.py` | ✅ PASS 17 / SKIP 0 / FAIL 0 |
| `scripts/smoke_mcp.py` | ✅ initialize / tools/list 19 tools / call 往返全绿 |
| `scripts/smoke_batch3_test.py` | ✅ **17 passed / 0 skipped / 0 failed**（6.1s，真机；基线 12 → 17） |

测试容器：`dbx-ssh-test`（linuxserver/openssh-server，127.0.0.1:2222），全部 smoke 真机真跑、无环境 SKIP。

## 1. 逐项完成情况（对齐 PROGRESS-TESTBASELINE §5 top5）

### 1.1 smoke_test.py 条目类型打印修正 + `sftp/list` 断言（建议 3 / O1）

- **修正**：`scripts/smoke_test.py` 原 153 行读 `entry.get('fileType', entry.get('type'))`，wire 契约实为
  `kind`（`backend/src/model.rs` `SftpEntry` / `backend/src/ssh.rs` `sftp_list_path`）。改读 `entry.get("kind")`，
  消除每次运行的 `None` 噪音。
- **新增断言**（原先只打印）：entries 非空；每个 entry 有非空 `name`、`kind ∈ {file, directory, symlink, other}`、
  `uri` 以 `sftp:/` 开头；home 列表至少含一个 `kind == "directory"` 条目。
- **真机证据**：`PASS: full sidecar chain OK in 1.1s`，打印
  `listed 6 entries in /config (kinds: ['directory', 'file'])`。

### 1.2 i18n 七语全量对齐断言（建议 2）

- **i18n.ts**：新增 dev/test 辅助导出 `workbenchMessageTable(locale)`——将 `messages`（嵌套 → 点分 key 扁平化）
  与 `supplemental` 合并成 `key -> template` 全表；`flattenMessageTable` 为模块私有。
- **workbench.spec.ts** 新增 2 个用例：
  1. `keeps every message key present in all seven locales`：以 en 的 key 集为基准，遍历其余 6 语断言
     key 集合完全一致（missing/extra 双向）且每条 value 为非空字符串；
  2. `keeps template placeholders aligned with the English source`：每条翻译的 `{placeholder}` 集合必须与 en 一致
     （防翻译漏占位符导致渲染静默丢值）。
- **首轮运行即抓到真漏**（"漏一语 typecheck 不报"的实锤）：`supplemental` 表的 es / it / ja / pt-BR 四语
  各缺 9 个公共 key（`cancel`、`close`、`create`、`save`、`error`、`sessionUnrecoverable`、
  `hostApiUnavailable`、`itemsSelected`、`readOnly`），此前一直静默英文回退。已按各语言补齐真实翻译
  （西 / 意 / 日 / 葡），非 key 本身、非英文占位。
- **证据**：vitest 38/38 全绿（35 → 38）；占位符对齐用例覆盖全部 key × 6 语无漂移。

### 1.3 smoke_batch3 目录级 copy/move 真机用例（建议 1 / O4）

`scripts/smoke_batch3_test.py` 新增 5 个真机用例（`sftp/copy`/`sftp/move` 未注册时沿用既有 SKIP 链），
全部对齐 tiny-rdm `FsCopyMove` 语义：

| 用例 | 覆盖点 |
| --- | --- |
| `sftp/copy copies a directory recursively` | 目录递归（`cp -a`）：两级目录 + 嵌套文件内容逐字节校验 + 拷贝后 `sftp/list` 的 `kind == "directory"` |
| `sftp/copy accepts a single-string from` | `from` 单字符串形态端到端（此前仅纯函数单测） |
| `sftp/copy and sftp/move block on existing directory targets` | 覆盖冲突（不带 `overwrite`）：copy 与 move 均逐项失败且 error 含 `already exists`，且被 block 的 move 源仍在 |
| `sftp/move relocates a directory` | 目录 move：源消失、目标存在、嵌套内容随迁 |
| `sftp/move overwrite replaces an existing file target` | 覆盖冲突（带 `overwrite`）：`mv -f` 替换同名文件目标、内容校验、源消失 |

- **实现备注**：`sftp/createDirectory` 为单级创建（`create_dir` 非 `_all`），种子目录树逐层 ensure。
- **真机证据**：17 passed / 0 skipped / 0 failed（6.1s）；基线 12 → 17。
- **清理**：新增 scratch 路径全部挂入既有 finally 清理（best-effort `sftp/delete recursive`），容器无残留。

### 1.4 终端标记条运行中时长 tick（建议 4，纯前端）

- **terminalCommandMarkers.ts**：新增纯函数 `runningCommandElapsedMs(startedAt, now)`（idle / 起点非法 /
  时钟回拨一律返回 null，由 vitest 覆盖 7 组断言）。
- **App.vue**（最小改动）：
  - `commandMarker` reactive 增加 `startedAt`；新增 `commandMarkerElapsed` ref；
  - `applyCommandMarker` 收到 `commandActive === true`（新 "E" 帧）时记录起点并启动 1s `setInterval` tick，
    `commandActive === false`（"D"/"A" 帧）即停；结束时长仍以 "D" 帧的 `durationMs` 为准（tick 停止后回落到终值）；
  - `resetCommandMarker` 与 `onBeforeUnmount` 均确定性停表；
  - 模板在 running 文案旁新增 `marker-elapsed` span，复用 `formatCommandDuration`（850ms / 1.2s / 2m05s 语义一致）。
- **style.css**：`.terminal-command-marker .marker-elapsed { flex: 0 0 auto; opacity: 0.8; font-variant-numeric: tabular-nums; }`，
  沿用既有 marker 样式族（无新样式体系）。
- **无新增文案**：时长为独立数字 span，未改 `terminalCommand.running` 模板，7 语无需变动。
- **证据**：vitest `ticks the running marker duration only while a command is active` 通过；typecheck 0 错。

### 1.5 全量验证收尾（建议 5 的仓内可做部分）

- `cargo test` 107/107、前端三件套（typecheck / vitest / build）、四份 smoke 全绿（见 §0 表）。
- **PROTOCOL.zh-CN.md 无需更新**：本轮无方法契约变化——`sftp/copy`/`sftp/move` 的 `from`（string/string[]）、
  目录递归（`cp -a`）、`overwrite` 探测语义、逐项 `results` 契约均已在册且与真机行为一致；tick 为纯前端展示层。
- FEATURE_PARITY* / TEST_MATRIX 未动（所有权约束）。若有收口点，仅列于本报告 §3。

## 2. 安全红线自查

- **命令转义**：本轮未新增任何远端命令构造；目录用例走既有 `sftp_copy.rs` 的 `shell_quote` 路径
  （`cp -a -- 'src' 'dst'`），真机验证了含点前缀的隐藏目录名（`.dbx-batch3-*`）全程引用正确。
- **只读面**：smoke 新增用例只操作 `$HOME` 下的 `.dbx-batch3-*` 隔离前缀路径，finally best-effort 清理，
  拒绝覆盖场景验证了默认 `overwrite: false` 的保护语义仍然生效。
- **前端**：无新依赖、无 eval/动态注入；tick 定时器在 `resetCommandMarker` 与 `onBeforeUnmount` 双路径确定性清理。

## 3. 遗留与待收口点（供主会话 / 后续批次）

1. **建议 5（sudo 保活与 OTP 防重放端到端真机 smoke）未在本批实施**：需可控时钟或接受 4 分钟真实等待，
   成本中等，建议单列批次（TESTBASELINE §5.5 原样保留）。
2. **FEATURE_PARITY / TEST_MATRIX 收口点**（仅登记，未改文档）：
   - i18n 七语 key 全对齐 + 占位符对齐已获自动化保证（可在 PARITY「多语言」相关行补一句 vitest 佐证）；
   - es/it/ja/pt-BR 曾缺的 9 个 supplemental key 已补齐（如 PARITY 记录过 i18n 覆盖状态，需同步）；
   - smoke_batch3 用例数 12 → 17（TEST_MATRIX「2026-08-28 本机基线」若再刷新请取 17）；
   - vitest 35 → 38、smoke_test.py 的 O1 观察项已消除（TESTBASELINE O1/O4 均可关单）。
3. 打包会覆盖 `backend/target/release` 二进制（O5 流程注意）：本轮未执行 `dbx-plugin package`，
   出包前请按 SKILL.md 顺序（构建→打包→smoke）单独走一遍。
4. 宿主集成段（scripts/test.sh 不带 --skip-host）与隧道真机验证仍属宿主依赖，未在本批范围。
