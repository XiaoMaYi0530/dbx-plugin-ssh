# NyaTerm 对标开发执行方案：多阶段 × 多 Agent 并行 × CI 持续验证

> 配套文档：实施任务卡见 [IMPL_PLAN_NYATERM_PARITY.zh-CN.md](./IMPL_PLAN_NYATERM_PARITY.zh-CN.md)（v2）。
> 本方案定义**怎么跑**：阶段节奏、agent 分工拓扑、分支/PR/CI 流、验证门与冲突治理。
> 全程遵守 `.github/agent-flow.yml` 既有契约：`one_branch_per_agent`、`merge_policy: integrator_only`、ownership 划分、本地验证 8 条、CI 六门、`install_and_restart: human_only`。

## 1. 里程碑与阶段

| 里程碑 | 内容（任务卡编号） | 波次 | 出口标准 |
| --- | --- | --- | --- |
| **M1（P1，无新依赖/无 manifest 变更）** | 8a 建议浮层、8b 动作链接、8c gutter、9a/9b GPU+NPU、10b 传输配置+重复策略 | W1（3 agent 并行） | 六任务全绿合入；headless 走查过；TEST_MATRIX 增行 |
| **M2（P2，新依赖+新协议面）** | 5 OTP 库、4 会话导入、2b Telnet、9c Docker、10a watcher 回传、10c symlink、8e 背景图、8d 搜索、8f 大输出保护 | W2a/W2b（3–4 agent） | 九任务合入；新依赖评审记录；sidecar→UI 事件通道结论落 PROTOCOL |
| **M3（P3，协议级扩展，逐项独立评审）** | 2c 串口、2d VNC、7 X11（spike 先行）、2e RDP（立项材料先行） | W3（每项 1 agent） | 每项独立 PR + 评审记录；RDP 未过评审不进实现 |

依赖引入窗口：每个里程碑开始时一次性提交依赖清单（用途/许可证/维护状态）走评审，中途不加。
M2 依赖包：`rqrr`、`image`、`zip`、`encoding_rs`、`sha3`、`cbc`、`pbkdf2`、`notify`；M3：`serialport`、vnc 引擎。

## 2. Agent 并行拓扑

### 2.1 隔离规则（沿用 a4 多 agent 先例）

- 每 agent：独立 worktree（`.worktrees/parity-<task>`）+ 独立分支 `codex/ssh/parity-<task>`（如 `parity-gpu`、`parity-suggest`、`parity-telnet`）。禁止两个 agent 进同一 checkout。
- 分支基线：从**最新 `codex/ssh/nyaterm-parity-integration`** 切出（含全部已验证集成成果）；PR 目标同为 integration 分支。
- ownership 约束（agent-flow）：backend agent 只碰 `backend/`，frontend agent 只碰 `frontend/`，跨栈任务在同一 agent 内完成跨栈部分；`Cargo.lock`/`ui/`/版本号归 integrator。

### 2.2 W1 分工（3 agent）

| Agent | 任务卡 | 主要文件 | 潜在热点 |
| --- | --- | --- | --- |
| A（backend→frontend） | 9a GPU + 9b NPU | 新 `backend/src/metrics_gpu.rs`、`metrics.rs` 白名单一行、监控面板新组件 | metrics.rs 仅改白名单一行 |
| B（frontend） | 8b 动作链接 + 8c gutter | 新 `lib/actionLinks*.ts`、`lib/terminalGutter.ts`、`components/TerminalGutter.vue`、设置注册 | 共用高亮防抖基建 |
| C（混合） | 8a 建议浮层 + 10b 传输配置 | 扩展 `lib/commandHistory.ts`、新 `lib/commandSuggestions.ts`、`components/CommandSuggestions.vue`、新 `lib/transferQueue.ts`、`backend` 的 `sftp/rename-unique` | App.vue onData 接线 |

**热点治理（App.vue / i18n.ts 是全部任务的必争之地）**：

1. 逻辑一律进**新文件**（`lib/`、`components/`），App.vue 只允许"接线点"改动：import + 单行调用 + 设置注册。
2. i18n 文案各任务只**追加**自己的 key 块（不改他人行），合并时天然无冲突。
3. 同一 wave 内两个 agent 都要动 App.vue 同一区域时，按接线点清单错峰（W1：A 不动 App.vue 终端区；B 动 decoration 注册点；C 动 onData/上传入口）。
4. wave 结束由 integrator 做集成 merge（模仿 a4-integration-check：先到先合，后到 rebase；冲突超过 3 个 hunk 的退回作者重放）。

### 2.3 W2 分工（3–4 agent，两批）

- **W2a（backend 重）**：A=OTP 库（新 `otp.rs`，算法/存储/协议先做，面板留 W2b）、B=会话导入（新 `connection_import.rs` + 解析器 fixture 测试）、C=Telnet（新 `telnet_session.rs`，复用 triggers Expect）。
- **W2b（frontend 重 + 收尾）**：D=Docker 面板（backend `docker.rs` 先 PR，面板次 PR，两半合入）、E=watcher 回传 + symlink、F=背景图 + 搜索 + 大输出保护（三个 frontend 小任务串行）+ OTP/导入的前端面板。

### 2.4 W3

每任务单 agent + 独立评审：串口、VNC、X11（spike 报告先行，评审通过才立项实现）、RDP（立项材料先行）。涉协议新增的（VNC 帧通道、X11、串口临时会话）按 agent-flow `command_execution_changes: human_review_required` 走评审后再实现。

## 3. 每任务标准流程（对齐 agent-flow 五阶段）

```
discovery   读任务卡 + 相关 nyaterm 源码 + 本仓库落点文件
contract    涉协议/manifest 的先改 docs/PROTOCOL.zh-CN.md（ownership: contract）；
            P1/P2 全部不动 manifest.json
implement   TDD：先写失败测试（fixture/纯函数单测）→ 最小实现 → 全绿
validate    本地 8 条（agent-flow validation.local 全量）：
            validate_repo.py / connection-forms verify / pnpm typecheck / test / build /
            cargo fmt --check / clippy -D warnings / cargo test --locked
review      push 分支 → gh pr create（base: nyaterm-parity-integration）→ CI 六门
            → 人工 review（**agent 不自合**，merge_policy: integrator_only）
integrate   integrator merge → integration 分支前进 → 通知 wave 内其他 agent rebase
```

## 4. CI 与合入流

- **CI 触发**：`ci.yml` 在 pull_request 上跑六门 `validate / frontend / backend / windows-regression / ssh-smoke / candidates-check`；分支直推不触发——**一切变更走 PR**。
- **合入节奏**：
  - 任务级：task PR → integration（逐个合，CI 全绿 + 人工批准）。
  - 里程碑级：每 wave 结束，integration → main 开**里程碑 PR**（M1/M2/M3 各一个），integrator 审合。main 保持"随时可发布"状态。
- **命令清单**（agent 可执行部分）：
  - `gh pr create --base codex/ssh/nyaterm-parity-integration --title ... --body ...`
  - `gh pr checks <n>` / `gh run watch <id>` 盯门禁
  - 失败处置：CI 红 → agent 修复后 push 同分支重跑；flaky（ssh-smoke 容器类）→ 重跑一次并在 PR 记录，连续两次 flaky 开 issue 给 ci-smoke-hardening 线。
- **基线 PR**：本次集成基线即 [#98](https://github.com/jinpy666/dbx-plugin-ssh/pull/98)（integration → main），其合入即 M0 里程碑。

## 5. 持续验证（四层门）

| 层 | 时机 | 内容 | 当前基线 |
| --- | --- | --- | --- |
| L1 本地 | 每次提交前 | agent-flow validation.local 8 条 | cargo 575 / vitest 698 / tsc 0 |
| L2 CI | 每个 PR | 六门（含 Windows 回归 + ssh-smoke 容器冒烟） | PR #98 起跑 |
| L3 wave 集成 | 每 wave 末（integrator） | 全量 L1 + `ui/` 重生成 + headless Chrome 走查（`mock.html?fresh=1` 连接流程）+ 相关 smoke 脚本（`smoke_local_terminal.py` / `smoke_forward_test.py` / `smoke_login_mfa_test.py`）+ TEST_MATRIX 增行 | 每 wave 记录到 PROGRESS |
| L4 里程碑 | main 合入前（human） | DBX 宿主真机验收：安装/重连/新功能手测（`install_and_restart: human_only`） | 每 milestone 一次 |

**不可倒退的回归防线**（每个 wave 的 L3 必测，防集成磨损失守）：

1. 高亮防抖：空闲 12s 零额外重绘/零装饰拆建（0.4.79 回归口径）；
2. macOS 快速输入不丢键（0.6.0 修复 + #33/#71 诊断浮层读数 keys≈sends≈acks）；
3. 拖放门禁：只读连接三条上传链路全部拒绝并七语提示；
4. 本地终端：shell 切换/退出码标记/重载接回（a4 验收口径）；
5. 端口转发与登录 MFA 冒烟脚本持续全绿。

## 6. 风险与协调

| 风险 | 缓解 |
| --- | --- |
| App.vue/i18n.ts 冲突热点 | §2.2 新文件优先 + 接线点最小化 + wave 末 integrator 统一融合 |
| 多 agent 同基线漂移 | 任务分支短命（≤3 天）；wave 内 rebase 于 integration；超时降级为串行 |
| 新依赖安全/维护 | 里程碑开始一次性评审；`deny_unknown_fields` 契约下 manifest 全程冻结 |
| sidecar→UI 事件通道未知（10a watcher） | W2b 前先出半天 spike（现有传输进度/终端输出事件形态），结论写 PROTOCOL 后再实现 |
| CI flaky（容器冒烟） | 重跑一次记录；连续两次 flaky 转 issue 给 ci 线，不阻塞其他任务合入 |
| 长任务（9c Docker） | 拆 backend/PR 与 frontend/PR 两半，backend 先行不阻塞 wave |

## 7. 立即行动清单（M1/W1 启动）

1. integrator 合 PR #98（M0：集成基线进 main）。
2. 从 integration 切三个 worktree/分支：`parity-gpu`（A）、`parity-actions-gutter`（B）、`parity-suggest-transfer`（C）。
3. 各 agent 按 IMPL_PLAN v2 对应任务卡 TDD 开工；每日以 `gh pr checks` 汇报门禁状态。
4. W1 末：integrator 跑 L3 集成验证 → 开 M1 里程碑 PR（integration → main）。
