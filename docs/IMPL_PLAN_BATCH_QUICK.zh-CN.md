# IMPL_PLAN：批量发送命令 + 全局快速命令

对标 tiny-rdm 批量发送（多会话发送命令）与快速命令全局化。tiny-rdm 参照实现：
`frontend/src/modules/ssh/`（useBatchSend / SshBatchSendPanel / batch-send.js）、
`backend/services/ssh_service.go`（writeInput → PTY stdin）。

## 背景与非目标

- 插件是"单 workbench 单连接单会话"架构，但 sidecar 是插件级共享进程：
  `ssh/sessions/list` 返回**跨连接**的全部活跃会话——批量发送以 sidecar 会话清单为
  目标集合，天然覆盖"多个打开的连接"。
- tiny-rdm 语义（保持对齐）：命令写入各会话 **PTY stdin**（交互 shell 内执行，
  cd/env 状态留在 shell、输出回显在各自终端）；结果只报**发送成功/失败**，
  不收集远端输出与退出码。
- 非目标：多会话标签/分屏（宿主工作台职责，维持 FEATURE_PARITY deferred 结论）；
  远端输出汇聚展示；向未连接会话发送。

## 快速命令全局化（问题根因）

快速命令此前存前端 `localStorage["ssh-quick-commands"]`。宿主每个插件工作台是
独立 webview，localStorage 按工作台分区 → 表现为"和连接绑定"。sidecar 数据目录
才是插件级共享存储（`quick-sudo-profiles.json` 同款先例），迁移后所有连接/工作台
共用一份，宿主重启、重装连接均保留。

## 契约（协议详见 PROTOCOL.zh-CN.md）

### 新增方法

| 方法 | 参数 | 返回 |
| --- | --- | --- |
| `ssh/quickCommands/list` | 无 | `{commands: [{id,name,command,createdAt,updatedAt}]}`（createdAt 升序=插入序） |
| `ssh/quickCommands/save` | `{id?, name?, command}`（id 空/缺省=新建；name 缺省取 command 截断） | `{quickCommand, created, commands}` |
| `ssh/quickCommands/delete` | `{id}` | `{removed, commands}` |
| `ssh/terminal/batchInput` | `{sessionIds: string[], command, appendNewline?=true}` | `{results: [{sessionId, success, error?}], sent, failed}` |

- 存储：`<data_dir>/quick-commands.json`（原子写 tmp+rename、0600、坏文件降级为空，
  对齐 quick-sudo-profiles.json）；上限 20 条、name ≤60、command ≤500（与前端
  lib/quickCommands.ts 常量一致）。
- batchInput：command 内 `\r\n`/`\n`/`\r` 归一为 `\r`，`appendNewline` 追加回车；
  逐会话 `terminal_tx` 投递（队列满/会话不存在记为该目标失败，不整体报错）；
  command 归一后上限 256 KiB。
- `ssh/sessions/list` 行**追加** `host`/`port`/`username`（连接注册表解析，缺失回退
  空/22/空）——只读展示字段，供批量目标列表显示 `user@host`。

### 前端

- 工具栏新增"批量发送"按钮（命令弹窗旁）；弹窗 = 目标会话多选（当前会话预选，
  全选/仅存活快捷键，断开/只读徽标）+ 命令输入 + 快速命令下拉联动 + 危险命令
  复用 `confirmRiskyPaste` 红色确认 + 逐会话发送结果。
- 快速命令 CRUD 改走后端 RPC；首挂载时后端为空且 localStorage 有旧数据则一次性
  迁移（save 循环）后清除本地键。发送语义不变（PTY 写入当前会话）。
- 纯函数入 `lib/batchSend.ts`（目标归一/选择/结果汇总）+ vitest。

## 验收

1. 后端 `cargo test`（quick_commands 存储 roundtrip/坏文件/上限/校验；batchInput
   payload 归一与结果聚合纯函数）。
2. 前端 `vue-tsc` + `vitest` + `build`；七语 key 对齐由 workbench.spec 既有比对覆盖。
3. `scripts/smoke_batch_quick_test.py`：quickCommands CRUD 全链路（无需 SSH 连接）+
   batchInput 对测试容器会话发送（unknown 目标计失败、PTY 回显含 marker）；
   Method-not-found → SKIP。
4. 对标/任务清单：FEATURE_PARITY（批量发送 deferred → 已实现差异说明）、
   PROGRESS-P-SSH 记录；PROTOCOL 新方法登记。

## 状态

- [x] 契约定稿（2026-09-04）
- [x] 后端实现 + 单测
- [x] 前端实现 + 单测
- [x] smoke + 文档同步
