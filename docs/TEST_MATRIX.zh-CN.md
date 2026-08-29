# 验收矩阵

| 范围 | 原型门槛 | 当前状态 |
| --- | --- | --- |
| Sidecar 编译 | Windows x64 Debug/Release | 已通过 |
| 单元测试 | 主机密钥、路径、序号、补发、临时路径 | 7/7 通过 |
| 工作台 | TypeScript、Vite 自包含构建 | 已通过 |
| Host H1 | 跨版本数据目录 | 5/5 通过 |
| Host H2/H3 | 文件句柄、连接挑战 | 11/11 通过 |
| 插件包 | Windows x64 未签名候选包 | 已生成并通过 Host 安装/生命周期冒烟，本地未发布 |
| Host 真实 SSH/SFTP | 密码、指纹、变化密钥拒绝、PTY、resize、0 B/4 MiB、取消、重连 | 已通过 |
| Windows 桌面完整 UI | 沙箱、剪贴板、文件选择/保存、双栏交互 | 待启动应用验收 |
| DBX Web/Docker | 同包、服务器侧 Sidecar、本地文件句柄 | 待启动集成环境 |
| 长稳与突发 | 30 分钟、序号补发 | 待真实服务器 |
| 大文件 | 100 MiB、1 GiB、SHA-256 | 完整阶段；H2 正式流实现后执行 |
| 五平台包 | Windows、macOS x2、Linux x2 | 工作流已配置，未触发 |

真实验收时应记录 DBX 版本、插件包 SHA-256、SSH 服务端版本、网络延迟/丢包条件、传输文件 SHA-256、残留临时文件检查和终端首尾序号。

## 2026-08-29 本机基线（darwin-arm64，batch3 + S-A/S-B/X-B/A-SSH 全轮收口终值）

> B-SSH-COLLECT 收口基线（未提交 batch3 + 历轮增强工作区，2026-08-29 全量复测）；上方 Windows
> 平台矩阵为早期验收记录，本机未复核、不做改动。明细见 `PROGRESS-COLLECT-FINAL.zh-CN.md`
> 与 `PROGRESS-TESTBASELINE.zh-CN.md`。

| 套件 | 结果 |
| --- | --- |
| backend cargo test | **109/109** 通过，0 ignored（A-SSH 新增 `auth_method_names_round_trip_for_display` 等；S-A 增量：metrics 快照缓存 2、inode/topMemory 解析 3、optional_u64、MCP offset schema） |
| frontend typecheck / vitest / build | 全过；vitest **51/51**（workbench 45 + appearance 6；含 OSC 633 八用例、session status 三用例、输出净化三用例、i18n 七语全对齐两用例、tick/命令历史/快速命令/authMethod 用例）；自包含 ui/index.html（2,317,270 B）产出 |
| smoke_test.py | PASS 1.1s（真机容器 dbx-ssh-test；已含 `sftp/list` kind 断言，O1 关单） |
| smoke_fs_test.py | PASS 17 / SKIP 0 / FAIL 0 |
| smoke_mcp.py | PASS（19 个 MCP 工具；MCP.zh-CN.md 已同步 19） |
| smoke_batch3_test.py | PASS **17/17**（12→17：目录递归 copy、单字符串 from、目录 move、目录目标冲突、overwrite 替换；metrics 网络/Top 进程/inode/快照缓存、sftp/read offset 分页、knownHosts marker、copy/move 语义、sessions/list 含 authMethod 断言） |
| smoke_sudo_otp_test.py | PASS **10/10**（sudo 保活/OTP 编排端到端真机） |
| dbx-plugin package | 0.2.2 darwin-arm64 出包完整（5 文件 + checksums；**4,290,327 B，sha256 `a67bb683a2db1ee67e37ae8b6d96447a44fe160fb07e55b459289a450edf347e`**；出包后对重编二进制复跑 smoke_test PASS 1.2s） |
| 性能基线（PROGRESS-P-SSH §3，真机） | 终端 PTY 流灌入 **71.9 MiB/s**（5 MiB 环形缓存）；SFTP 上传（网络）**220–237 MB/s**、上传（本地 spool）633.9–1078 MB/s；SFTP 下载 **113–118 MB/s**；replay 载荷 2.00 MiB / 601 帧 ≤ 2 MiB 上限（SHA-256 校验一致） |

仍保持未验收（环境/CI 依赖）：DBX Web/Docker、长稳与突发、大文件 100 MiB/1 GiB、五平台包、
宿主 plugin_tools_bridge 集成段（WIP-SKIP，宿主合流后必跑）。

## 下一批增强建议（2026-08-29 收口后余量，详见 PROGRESS-COLLECT-FINAL.zh-CN.md）

仓内已完成并关单：smoke_test.py kind 字段对齐（O1）、smoke_batch3 目录级用例（O4）、
i18n 七语全量对齐断言、终端标记条运行中时长 tick、sudo 保活/OTP 端到端真机 smoke
（smoke_sudo_otp_test.py 10 用例）。
依赖宿主/CI（单列）：宿主管线集成回归（test.sh 全量 + plugin_tools_bridge）、DBX Web/Docker
浏览器兜底 e2e、长稳与大文件 100 MiB/1 GiB、五平台包矩阵、多会话语义与端口转发归属决策。
