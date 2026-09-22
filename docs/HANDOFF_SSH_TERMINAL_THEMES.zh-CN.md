# 交接说明：Tabby 终端主题 / 行为 / 快捷键对标

面向 **integrator 与 reviewer**。按 `AGENTS.md` 的交接要求，记录基线 SHA、变更文件、测试、
风险与后续工作。本分支**未推送、未建 PR、未合并**（`.github/agent-flow.yml` 的
`merge_policy: integrator_only`）。

## 1. 基线与本分支

| 项 | 值 |
| --- | --- |
| 基线分支 | `main` |
| 基线 SHA | `acddf777ac5943adda4912e77090e59a0cc3726e`（`chore(ui): regenerate committed ui/ output`） |
| 工作分支 | `codex/ssh/terminal-themes` |
| 领先提交数 | 4 |
| 上游 | **无**（`git push` / 建 PR 均需 integrator 执行） |
| 变更规模 | 24 文件，**+6954 / -250** |
| 工作树路径 | `/Users/Jinpy/btroot/dbx-plugins/dbx-plugin-ssh-terminal-themes` |

### 提交清单

| 提交 | 类型 | 说明 |
| --- | --- | --- |
| `dc36c9df` | feat | 迁移 Tabby 配色（**192 套**）与多主题外观设置，拆分「配色方案」分类 |
| `f1c764c3` | feat | 对标 Tabby「Terminal」：Rendering / Keyboard / Mouse / Clipboard / Sound 五板块 + 新增「快捷键」分类与键位编辑器 |
| `a7cebce0` | fix | 恢复 `d983f8fc` 误删的 19 个 i18n 键；新增「翻译键引用」护栏 |
| `d12adc6c` | test | e2e 断言：确认恢复的 MCP 文案真的到达界面 |

## 2. 变更范围（按 ownership）

| 归属 | 路径 | 文件数 |
| --- | --- | --- |
| `frontend` | `frontend/src/**` | 21 |
| `tests` | `scripts/**` | 2 |
| 对标清单 | `docs/FEATURE_PARITY.zh-CN.md` | 1 |

**未触碰**（均属他人 ownership）：
`ui/`、`dist/`、`Cargo.lock`、`version_metadata`（integrator）；
`manifest.json`、`docs/PROTOCOL.zh-CN.md`（contract）；`backend/`（backend）。
本分支**无 Rust 侧改动**，协议未变。

### 新增文件（16）

```
frontend/src/lib/terminalSchemeCatalog.ts        192 套配色数据（生成物，见下）
frontend/src/lib/terminalScheme.ts               配色解析 / 应用
frontend/src/lib/terminalAppearance.ts           外观模型（字体、间距、光标、渲染器…）
frontend/src/lib/terminalBehavior.ts             Tabby「Terminal」行为模型
frontend/src/lib/terminalHotkeys.ts              可编辑键位注册表
frontend/src/components/TerminalAppearancePreview.vue   实时预览
frontend/src/components/TerminalSchemePicker.vue        配色选择器
frontend/src/components/TerminalHotkeyEditor.vue        键位录制编辑器
frontend/src/lib/terminalScheme.spec.ts
frontend/src/lib/terminalAppearance.spec.ts
frontend/src/lib/terminalBehavior.spec.ts
frontend/src/lib/terminalHotkeys.spec.ts
frontend/src/components/TerminalHotkeyEditor.spec.ts
frontend/src/lib/i18nKeyReferences.spec.ts       翻译键引用护栏（见 §5）
scripts/gen-terminal-schemes.py                  Catalog 生成脚本
scripts/smoke_ui_settings.mjs                    设置面板 e2e（见 §4）
```

### 修改的既有文件（8）

| 文件 | 改动要点 |
| --- | --- |
| `frontend/src/App.vue` | 接线：外观/行为/快捷键状态、响铃实现、右键语义、粘贴变换、链接修饰键、快捷键派发注册表 |
| `frontend/src/components/SettingsDialog.vue` | 9 个分类（新增 配色方案 / 快捷键）；「终端」板块重写为 Tabby 五分区 |
| `frontend/src/lib/i18n.ts` | 七语文案全量补齐；恢复 19 个被误删的键（键数 750 → **769**） |
| `frontend/src/lib/terminalInteraction.ts` | 退役被取代的解析器，仅保留仍在使用的能力 |
| `frontend/src/lib/terminalInteraction.spec.ts` | 移除已迁移到新 spec 的用例 |
| `frontend/src/lib/dangerousCommands.ts` | 多行粘贴告警改为可配置，危险命令守卫保持不变 |
| `frontend/src/style.css` | 响铃视觉闪烁 |
| `docs/FEATURE_PARITY.zh-CN.md` | 三轮对标的完整记录（含默认值逐项对照与「刻意不做」清单） |

> `terminalSchemeCatalog.ts` 由 `scripts/gen-terminal-schemes.py` 生成。数据源为 Tabby 的
> 内置配色定义（许可见 `docs/FEATURE_PARITY.zh-CN.md` 的「迁移来源与授权」）。**改动配色请改脚本后重跑**，
> 不要手改 Catalog。

## 3. 交付内容摘要

1. **配色**：192 套 Tabby 内置配色，独立「配色方案」分类；选择器带实时预览；支持明/暗与自定义。
2. **外观**：字体族 / 字号 / 行高 / 字间距、光标样式与闪烁、渲染器（含 WebGL）等，均有实时预览。
3. **终端行为**（对标 Tabby `Terminal` 页）：回滚行数、Alt 作 Meta、输入时滚动、右键语义（四档）、
   中键粘贴、词分隔符、链接修饰键、选中即复制、括号粘贴、多行粘贴告警、换行转空格、粘贴去空白、响铃（三档）。
4. **快捷键**：10 个可绑定动作（clipboard / view / navigation 三组），可在设置里录制改写、解绑、重置，
   冲突实时标注并显示占用者；绑定持久化。
5. **修复**：恢复 19 个被 `d983f8fc` 误删的翻译键（详见 §5）。

## 4. 本地验证结果

### 已执行（全部通过）

| 检查 | 命令 | 结果 |
| --- | --- | --- |
| 类型 | `vue-tsc --noEmit` | 零错误 |
| 单测 | `vitest run` | **70 文件 / 680 用例全绿** |
| 仓库结构 | `python3 scripts/validate_repo.py` | PASS |
| 连接表单 | `node scripts/connection-forms/verify.mjs` | PASS（502 组合 + 7 个超时作用域用例） |
| 设置面板 e2e | `node scripts/smoke_ui_settings.mjs` | **47 条断言 0 失败** |
| 工作台 e2e | `node scripts/smoke_ui_mock.mjs` | all green |

本分支新增单测 **6 文件 / 126 用例**；另有 1 个既有 spec（`terminalInteraction.spec.ts`）
因能力迁移到新模块而减少 7 例。全量由基线 **64 文件 / 561 用例** 变为 **70 文件 / 680 用例**（净 +119）。
基线数字是在 `main` 的临时 worktree 中实测所得，非推算。

> 环境提示：`smoke_ui_*.mjs` 依赖仓库外的 `playwright-core`（约定装在 `/tmp/dbx-ui-mock`）与
> 系统 Chrome；缺失时脚本**打印 SKIP 并以退出码 0 结束**——这是有意设计，别把 SKIP 当成通过。
> 跑 `smoke_ui_mock.mjs` 前需要 PATH 里有 `pnpm`，否则 `spawn pnpm ENOENT`。

### 未执行（有意留给 integrator / 人工）

| 项目 | 原因 |
| --- | --- |
| `pnpm --dir frontend build` | `frontend/build.mjs` 输出到 `../ui`，属 **integrator 所有权**；本分支全程未执行，`ui/` 仍是基线状态 |
| `cargo fmt / clippy / test` | 本分支无 `backend/` 改动，相关检查不受影响 |
| `scripts/test.sh`、`scripts/install.sh` | 契约规定 **安装与 DBX 重启为人工动作**（`security.install_and_restart: human_only`） |
| 真实 SSH 会话验证 | 契约禁止使用真实主机/凭据；终端的会话内行为（响铃、右键、粘贴）只在 mock 与单测层覆盖 |

## 5. 需 review 重点关注的变更

### 5.1 两处用户可见的行为变更

1. **终端搜索快捷键**：Windows/Linux 下由 `Ctrl+F` 改为 `Ctrl+Shift+F`（macOS 仍是 `⌘F`）。
   原因：`Ctrl+F` 会被远端 shell（readline）吃掉，占住它会让远端「前移一字符」失效。
   **该改动在设置里可改回**，不是硬编码。
2. **「选中即复制」并入 Clipboard 分区**，同时退役两个 i18n 键
   （`terminalSelectCopy.section`、`terminalBehavior.copyOnSelect`）。

### 5.2 默认值：刻意与 Tabby 不同

以下三项**保留本插件改动前的行为**，与 Tabby 默认值不一致，已在 `docs/FEATURE_PARITY.zh-CN.md`
逐项列表说明理由；如需与 Tabby 完全一致，改默认值即可（有单测锁定）：

| 设置 | 本插件默认 | Tabby 默认 |
| --- | --- | --- |
| 右键语义 `rightClick` | `paste` | `menu` |
| 选中即复制 `copyOnSelect` | `true` | `false` |
| 粘贴去空白 `trimWhitespaceOnPaste` | `false` | `true` |

### 5.3 翻译键引用护栏（`i18nKeyReferences.spec.ts`）

`workbenchMessage` 在查不到键时**会把 key 原样返回**，而既有的七语对齐断言只比对**语言之间**
的键集合——**一个在七语中同时缺失的键它抓不到**。新护栏静态扫描 130 个源文件、
903 处 `t("…")` 调用与 19 处 `labelKey`（共 617 条唯一引用），断言全部可解析。

该护栏上线即抓出 **19 个既有缺陷**：`d983f8fc`（trigger-driven SSH auth providers）对
`i18n.ts` 的改动是 **+28 / -112**，顺带删掉了 `terminalDropPrompt.*`、`metricsSwap`、
`mcpSettings.permissionMode*` 与 11 个 `errors.*`，而调用点一直留着——用户会看到
字面量 `metricsSwap`、**整个拖拽上传确认框未翻译**、MCP 权限下拉显示原始键。
`a7cebce0` 已按 `d983f8fc^` 的原文**逐字恢复 133 条（19 键 × 7 语）**，`git diff` 为 **+157 / -0**。

> `errors.terminalInputAckTimeout` 同被删除，但**全仓库零引用**（含 `backend/`），故未恢复。
> 若将来有代码路径需要它，需重新补七语。

## 6. 风险与接缝

| 风险 | 说明 | 缓解 |
| --- | --- | --- |
| `ui/` 未重建 | 本分支改了 `frontend/src`，但 `ui/` 仍是基线产物 | integrator 合并后必须执行 `pnpm --dir frontend build` |
| 快捷键基于 `KeyboardEvent.code` | 用物理键位而非 `event.key`，为的是让 `Ctrl+=` 与 `Ctrl+Shift+=` 不塌陷；代价是非 QWERTY 布局下显示字母可能与键帽不同 | 已在 parity 文档记录；用户可自行改绑 |
| 响铃需自行实现 | xterm 6.x 已移除 `bellStyle`，只剩 `onBell` 事件 | 本实现用 WebAudio 合称 + `inset` 闪烁，**无音频资源文件、无新运行时依赖** |
| 新 localStorage 键 | 新增 `ssh-terminal-appearance`、`ssh-terminal-behavior`、`ssh-terminal-hotkeys` | 读取端容忍 `SecurityError`（沙箱 iframe 为 opaque origin）；行为设置有旧键 `ssh-terminal-select-copy` 镜像降级 |
| 192 套配色一次渲染 | 列表若全量展开会拖慢设置面板 | 选择器带**名称搜索**与**色调筛选**（`filterTerminalSchemes`），列表区固定 `max-height: 210px` 可滚动，色块只取每套前 16 色 |
| 零新增运行时依赖 | 契约硬性约定 | 已核对；色彩/响铃均为原生实现 |

## 7. 后续工作

### integrator 必做

1. `pnpm --dir frontend build` 重建 `ui/`（本分支刻意未做）。
2. 版本号与 `CHANGELOG.md`（integrator 所有权）。
3. 推送分支并建 PR（当前**无上游**）。
4. 最终 `manifest.json` / 协议一致性核对（本分支未改 manifest 与协议）。
5. 如有 host worktree，跑 `scripts/test.sh` 完成宿主安装管线集成验证；安装与重启为人工动作。

### 建议但非阻塞

- 把 `scripts/smoke_ui_settings.mjs` 接入 `scripts/test.sh`，让设置面板 e2e 随套件回归。
- 七语全量 UI 走查（现只在单测层覆盖全部 7 语，e2e 只走 `en` 与 `zh-CN`）。
- 为 `TerminalAppearancePreview` / `TerminalSchemePicker` 补组件挂载级测试
  （当前只覆盖其纯函数模型）。
- 评估这些偏好是否应像其他偏好一样改走 sidecar 的 `local/preferences/set`（当前为纯前端 localStorage）。

## 8. 复现验证

```bash
export PATH="$HOME/.nvm/versions/node/v22.21.0/bin:$HOME/Library/pnpm:$HOME/.cargo/bin:$PATH"
cd /Users/Jinpy/btroot/dbx-plugins/dbx-plugin-ssh-terminal-themes

# 单测 + 类型
(cd frontend && node node_modules/vue-tsc/bin/vue-tsc.js --noEmit)
(cd frontend && node node_modules/vitest/vitest.mjs run)

# 仓库校验
python3 scripts/validate_repo.py
node scripts/connection-forms/verify.mjs

# 前端 e2e（缺 playwright-core / Chrome 时会 SKIP 并退出 0）
node scripts/smoke_ui_settings.mjs   # 47 assertions
node scripts/smoke_ui_mock.mjs       # needs pnpm on PATH
```

人工走查（前端 fixture，不写 `ui/`）：

```bash
cd frontend && node node_modules/vite/bin/vite.js --host 127.0.0.1 --port 5199 --strictPort
# 打开 http://127.0.0.1:5199/mock.html ，点右上角齿轮进入设置
```

> `--host 127.0.0.1` 不能省：不指定时 Vite v8 只监听 IPv6 `[::1]`，而内置预览走 IPv4 回环。
> 另可用 `?locale=zh-CN|zh-TW|ja|es|it|pt-BR`、`?theme=light`、`?render=dom`、`?err=disconnect` 做定点走查。

## 9. 关键文件索引

| 想了解什么 | 看哪里 |
| --- | --- |
| 对标项逐条状态、默认值对照、刻意不做的项 | `docs/FEATURE_PARITY.zh-CN.md`（第三、四轮章节） |
| 行为设置的数据模型与默认值 | `frontend/src/lib/terminalBehavior.ts` |
| 快捷键动作表与冲突检测 | `frontend/src/lib/terminalHotkeys.ts` |
| 配色数据来源与生成方式 | `scripts/gen-terminal-schemes.py` |
| 翻译键引用护栏的实现与理由 | `frontend/src/lib/i18nKeyReferences.spec.ts` 头部注释 |
| 设置面板 e2e 覆盖矩阵 | `scripts/smoke_ui_settings.mjs` |
