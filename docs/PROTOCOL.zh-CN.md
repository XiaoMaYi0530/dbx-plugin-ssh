# SSH/SFTP 插件协议

## 生命周期与状态隔离

Sidecar 是插件级共享进程，所有状态都必须以 `connectionId`、`sessionId` 或 `taskId` 为键。`connection/connect` 只接收并缓存宿主注入的连接配置；`connection/disconnect` 会关闭该连接下的终端、SFTP 子系统和传输任务。工作台不会接收密码字段。

第一阶段不声明 `test` 能力。真实 SSH 握手在 `ssh/session/open` 发起，主机密钥确认完成前不会调用密码认证。

## RPC

| 方法 | 作用 |
| --- | --- |
| `ssh/session/open`、`ssh/session/close` | 创建、关闭 PTY 会话 |
| `ssh/terminal/resize` | 调整 PTY 行列 |
| `ssh/terminal/replay` | 从指定序号补发终端输出 |
| `ssh/host-key/resolve` | 处理工作台内的主机密钥确认 |
| `ssh/exec` | 在会话连接上执行远程命令，可选 Quick Sudo 提权 |
| `ssh/exec/cancel` | 中止进行中的远程命令（按 `execId`） |
| `ssh/metrics` | 采集服务器指标（CPU/内存/负载/磁盘（含 inode 使用率）+ 网络接口速率 + Top CPU/内存进程，只读命令；`cached: true` 返回上次快照） |
| `ssh/host-key/check` | 连接维度主机密钥预检（探针三态：已知 / 变更 / 未知，不发认证） |
| `ssh/settings/get`、`ssh/settings/set` | 读取/运行时更新 Quick Sudo 编排设置 |
| `mcp/tools`、`mcp/call` | MCP 工具发现与执行（供 DBX MCP 桥 `dbx_call_plugin_tool` 调用；连接凭据以标准 lifecycle payload 转发，按 `connectionId` 池化） |
| `mcp/settings/get`、`mcp/settings/set` | MCP SFTP 尺寸限制策略（maxRead/maxUpload/maxDownload，持久化，`--mcp` 同源生效） |
| `sftp/chmod` | 修改远端路径权限位（八进制） |
| `sftp/diskUsage` | 路径所在挂载的磁盘用量 |
| `sftp/home`、`sftp/list`、`sftp/read` | 浏览、预览远端文件（`sftp/read` 支持可选 `offset` 分片续读，见下文） |
| `sftp/createDirectory`、`sftp/rename`、`sftp/delete` | SFTP 写操作 |
| `sftp/upload/start`、`finish` | 上传事务生命周期 |
| `sftp/download/start`、`next`、`finish` | 下载事务生命周期 |
| `sftp/stat`、`sftp/exists`、`sftp/touch`、`sftp/write` | 扩展文件操作：元信息单查、存在性检查、空文件创建、小文件直写 |
| `sftp/archive`、`sftp/extract` | 远端 tar.gz 打包与解压 |
| `sftp/copy`、`sftp/move` | 服务器内复制 / 剪切（逐项执行，目标存在需 `overwrite`） |
| `sftp/transfer/cancel` | 取消并清理临时状态 |
| `sftp/transfer/list`、`sftp/transfer/status` | 查询会话传输任务列表 / 单任务状态（含历史，会话维度过滤） |
| `sudo/stat`、`sudo/exists`、`sudo/touch` | sudo 元信息查询与空文件创建 |
| `sudo/listDir`、`sudo/readFile`、`sudo/writeFile` | sudo 目录浏览与文件读写 |
| `sudo/mkdir`、`sudo/remove`、`sudo/removeAll`、`sudo/chmod`、`sudo/rename` | sudo 写操作 |
| `keys/discover` | 本地 SSH 私钥发现（不返回私钥内容） |
| `ssh/knownHosts/list`、`ssh/knownHosts/remove` | known_hosts 条目管理（含 `@cert-authority` / `@revoked` 标记条目） |
| `ssh/sessions/list` | 只读会话清单：sidecar 当前跟踪的活跃会话（对齐 tiny-rdm ListSessions） |

## 运行时设置

`ssh/settings/get` 返回当前编排配置（密钥仅以布尔标记呈现，绝不下发明文）；`ssh/settings/set` 接受 `quickSudo`、`sudoUsePty`、`sudoPassword`（空串=清除回退登录密码）、`totpSecret`、`authFlowMode`、`passwordPromptHint`、`totpPromptHint`。更新通过共享编排锁立即作用于该连接的**所有存活会话**——终端自动应答与命令弹窗在下一次提示时即用新值（对齐 tiny-rdm 每次输出动态 resolve 的语义）；sidecar 重启或重开连接后恢复宿主下发的配置。工作台工具栏提供设置弹窗；宿主连接表单通过 manifest 字段（`quick_sudo`、`sudo_password`、`totp_secret`、`auth_flow_mode`、提示词、`sudo_use_pty`、超时与 keepalive、`jump_hosts`）提供持久化配置入口。

## 跳板机（ProxyJump）与连接存活

- `external_config.jump_hosts`（最多 3 跳）定义跳板链：每跳包含 `host`、`port`（缺省 22）、`username`、`authentication`（`password` / `private-key` / `private-key-password` / `agent`）及对应凭据字段，可选 `totp_secret` / 提示词 / `auth_flow_mode`。配置跳板后整条链替换 runtime 隧道，末跳直连目标 `host:port`；每跳主机密钥独立校验，登录期 keyboard-interactive 2FA 同样生效。会话关闭时按序断开整条链。
- keepalive 探测 3 次无应答即判定连接死亡（对齐 tiny-rdm 的 `keepaliveMaxFail`），终端转入断开态、由工作台重连。

## Quick Sudo 远程执行

`ssh/exec` 参数为 `sessionId`、`command`、`sudo`（可选，默认 false）、`timeoutSecs`（可选，5–300 秒）、`execId`（可选，用于取消），返回 `output` 与 `exitCode`；`ssh/exec/cancel` 携带 `execId` 中止执行中的命令并返回取消错误。

Quick Sudo（`sudo: true`）移植自 tiny-rdm 的 sudo 执行服务：

- 命令以 `sudo -S -p '' sh -c '…'` 执行，密码写入 stdin（优先 `connection_secrets.sudo_password`，缺省回退登录密码）；未配置密码时回退 `sudo -n`（NOPASSWD 或已缓存时间戳）。
- 执行期间持续监控提示流，识别密码 / TOTP / 组合提示（内置中英文模式，可用 `external_config.password_prompt_hint`、`totp_prompt_hint` 自定义），并依据 `external_config.auth_flow_mode`（`password_only` / `password_plus_otp` / `password_then_otp`，默认 `password_then_otp`）自动应答。
- TOTP 密钥来自 `connection_secrets.totp_secret`，支持 `otpauth://totp/…` URI、base32 密钥或 4–10 位静态码；按 RFC 6238（SHA1/SHA256/SHA512，6–8 位）现场计算验证码。多个密钥按行/分号分隔，轮换选择避开同窗口已用过的验证码（对齐 tiny-rdm 的 resolveRotatingOTP usage 标记）。**同一验证码在提交后的重放窗口（自身有效窗 + ±1 步长）内不会被再次注入**——服务器普遍接受相邻窗口验证码，重复提交必然失败；命中窗口时自动应答直接跳过（不再等待用户输入），并在编排日志标注 `otp auto-answer skipped`。该防重放状态按连接驻留内存，sidecar 重启即清零。
- `external_config.quick_sudo` 可对单连接停用（默认启用）；`sudo_use_pty` 可为需要 TTY 的 PAM 栈请求 PTY（默认关闭，此时提示走 stderr）。
- 检测到 `sorry, try again` 等认证失败标记立即报错；认证应答最多三轮。sudo 执行成功后按连接启动 `sudo -nv` 保活循环：每 4 分钟（`SUDO_KEEPALIVE_INTERVAL`）校验/续期时间戳，连续 2 次（`SUDO_KEEPALIVE_MAX_FAILURES`）校验失败自动停止（时间戳已失效，下次 sudo 执行会重新注册）；同一连接只保留一个循环，连接断开或会话清理时确定性中止。
- 只读连接拒绝 sudo 执行；密码与 TOTP 密钥仅停留在 sidecar 内存中，不下发工作台。

## 登录期 2FA（keyboard-interactive）

密码认证被拒绝或服务器未开放 `password` 方法时，自动降级 keyboard-interactive（PAM）认证：每一轮提示按 Quick Sudo 的同一套编排配置自动应答（密码 / TOTP / 组合提示，遵循 `auth_flow_mode`），未知提示留空交由服务器处理。覆盖 `PasswordAuthentication no` + PAM 2FA 的主机，最多 4 轮，复用连接超时。密钥+密码（`private-key-password`）回退路径同样适用。

## 终端内 Quick Sudo

工作台终端输出流经与 tiny-rdm `detectAndHandleSudo` 相同的状态机：

- 出现 sudo 密码提示（`[sudo] password for …` / `Password:`）时自动注入密码并回车；sudo 需要的 2FA 验证码在配置了 `totp_prompt_hint`（或处于 `password_plus_otp` 模式）时自动应答，每个认证序列只应答一次。
- 通用提示仅在配置自定义提示词时应答，避免误答其他交互程序（与 tiny-rdm 的保守策略一致）。
- 检测到 shell 提示符（行尾 `$` / `#`）即重置状态机。`quick_sudo` 关闭或只读连接时整体停用；每次自动应答发出 `ssh/auto-sudo` 事件（`kind` 为 `password` / `otp`）供宿主审计。

## Sudo 文件操作

`sudo/*` 方法族在不以 root 登录的前提下管理远端 root 文件，一比一对齐 tiny-rdm 的 Sudo 文件操作族（`StatSudo` / `ListDirSudo` / `ReadFileSudo` / `WriteFileSudo` / `MkdirSudo` / `RemoveSudo` / `RemoveAllSudo` / `ChmodSudo` / `RenameSudo`）。全部方法复用 `ssh/exec` 的 Quick Sudo 编排（密码 / TOTP 自动应答、`auth_flow_mode`、提示词、`sudo -nv` 保活），在远端以 sudo 权限执行命令并解析输出：stat 走 `stat -c`，list 走 `ls -la --time-style=+%s`，read 走 `dd` / `base64`，write 走 `dd of=`。

公共参数：每个方法都必填 `sessionId`（string，会话 id），下文参数表不再重复列出。共同错误情形：

- 只读连接拒绝全部写操作（`touch` / `write` / `mkdir` / `remove` / `removeAll` / `chmod` / `rename`），错误信息与 SFTP 写操作一致；
- sudo 不可用（未配置密码且 `sudo -n` 失败、密码 / TOTP 认证失败、超过应答轮次）立即报错；
- 路径不存在按各方法说明处理（`exists` 返回 `false`，其余报错）。

### sudo/stat

查询单个远端路径的元信息。

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | 远端绝对路径 |

返回 `{ path, kind, size, modifiedAt, mode, owner, group }`：`kind` 取 `file` / `directory` / `symlink` / `other`；`size` 为文件字节数（目录可省略）；`modifiedAt` 为 Unix 秒级时间戳；`mode` 为八进制权限位字符串（如 `0644`）；`owner` / `group` 为属主 / 属组名。错误：路径不存在；sudo 不可用。

### sudo/exists

检查路径是否存在，参数同 `sudo/stat`（`sessionId`、`path`）。返回 `{ exists: bool }`，路径不存在不算错误。

### sudo/touch

创建空文件，或刷新已有文件的访问 / 修改时间戳。

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | 目标远端路径 |

无附加返回字段。错误：父目录不存在或无权限；只读连接；sudo 不可用。

### sudo/listDir

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | 远端目录路径 |

返回 `{ path, entries }`，`entries` 为 `SftpEntry` 数组，结构与 `sftp/list` 完全一致（`name`、`uri`、`kind`、`size`、`modifiedAt`、`permissions`、`contentType`，可选字段缺省时省略）。错误：路径不存在或不是目录；sudo 不可用。

### sudo/readFile

按偏移分片读取文件内容。

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | 远端文件路径 |
| `offset` | number | 是 | 起始字节偏移 |
| `length` | number | 是 | 请求读取的字节数 |

返回 `{ dataBase64, truncated }`：内容以 base64 编码返回；实际返回少于 `length` 字节（如已到文件末尾）时 `truncated` 为 `true`，调用方据此推进 `offset` 续读。错误：路径不存在或不是普通文件；`offset` 超出文件大小；sudo 不可用。

### sudo/writeFile

整体覆写小文件（对应 tiny-rdm `WriteFileSudo`）。

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | 目标远端路径 |
| `dataBase64` | string | 是 | 完整文件内容（base64） |

解码后不得超过 4 MiB，超限直接报错——大文件必须改走 `sftp/upload/*` 上传槽。错误：载荷超限；父目录不存在或无权限；只读连接；sudo 不可用。

### sudo/mkdir

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | 待创建目录路径 |

错误：路径已存在；父目录不存在或无权限；只读连接；sudo 不可用。

### sudo/remove

删除单个文件或空目录，不递归。

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | 待删除路径 |

错误：路径不存在；目标是非空目录（改用 `sudo/removeAll`）；只读连接；sudo 不可用。

### sudo/removeAll

递归删除目录树。

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | 待删除路径 |

递归删除，不跟随符号链接——只删除链接本身，不触碰其指向的目标（对齐 tiny-rdm `RemoveAllSudo` 的防误删语义）。错误：路径不存在；只读连接；sudo 不可用。

### sudo/chmod

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | 远端路径 |
| `mode` | string / number | 是 | 八进制权限位，最大 `7777`（与 `sftp/chmod` 约定一致） |

错误：路径不存在；只读连接；sudo 不可用。

### sudo/rename

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `sourcePath` | string | 是 | 原路径 |
| `targetPath` | string | 是 | 新路径 |

重命名 / 移动，语义等价 `mv`。错误：`sourcePath` 不存在；`targetPath` 父目录不存在或无权限；只读连接；sudo 不可用。

## 扩展文件操作

`sftp/*` 扩展方法基于 russh-sftp 原生协议与远端 `tar` 命令，补齐 tiny-rdm `sftp_service.go` 的 `Stat` / `Exists` / `Touch` / `WriteFile` / `Archive` / `Extract` 能力，走常规 SFTP 通道（无 sudo）。公共参数：均必填 `sessionId`（string，会话 id），下文参数表不再重复列出；写操作（`touch` / `write` / `archive` / `extract`）被只读连接拒绝。

### sftp/stat

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | 远端绝对路径 |

返回结构与 `sudo/stat` 完全一致（`{ path, kind, size, modifiedAt, mode, owner, group }`）。错误：路径不存在。

### sftp/exists

参数同 `sftp/stat`（`sessionId`、`path`）。返回 `{ exists: bool }`，路径不存在不算错误。

### sftp/read

小文件直读（非传输槽，对应 tiny-rdm `ReadFile` 的偏移分片语义）。

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | 远端文件路径 |
| `offset` | number | 否 | 起始字节偏移，默认 `0`；省略或非法值按 `0` 处理 |
| `maxBytes` | number | 否 | 本次最多返回的字节数，默认 256 KiB，上限 1 MiB（至少为 1） |

返回 `{ dataBase64, truncated }`：内容 base64 编码；返回字节数达到 `maxBytes` 且文件还有剩余时 `truncated` 为 `true`，调用方以 `offset += 返回字节数` 续读。`offset` 在文件末尾或超出文件大小时返回空内容且 `truncated: false`（不报错，与 `sudo/readFile` 的「offset 超界报错」语义不同——SFTP 侧以空读表示 EOF）。错误：路径不存在或不是普通文件；无读取权限。

### sftp/touch

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | 目标远端路径 |

创建空文件或刷新时间戳。错误：父目录不存在或无权限；只读连接。

### sftp/write

小文件直写（非传输槽，对应 tiny-rdm `WriteFile`）。

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `remotePath` | string | 是 | 目标远端路径 |
| `dataBase64` | string | 是 | 完整文件内容（base64） |

先写入同目录临时 `.dbx-part` 文件，完成后原子替换目标路径，避免读到半写状态。解码后不得超过 4 MiB，超限报错并提示改走 `sftp/upload/*` 上传槽。错误：载荷超限；父目录不存在或无权限；只读连接。

### sftp/archive

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `sourcePaths` | string[] | 是 | 待打包的远端路径列表（至少一项） |
| `archivePath` | string | 是 | 输出归档路径（`.tar.gz`） |

在远端执行 `tar -czf` 打包为 tar.gz。返回 `{ path, size }`（归档路径与字节数）。错误：任一源路径不存在；输出父目录不存在或无权限；远端缺少 `tar`；只读连接。

### sftp/extract

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `archivePath` | string | 是 | 归档路径（`.tar.gz` / `.tgz` / `.tar`） |
| `destinationPath` | string | 是 | 解压目标目录 |
| `overwrite` | bool | 是 | 是否覆盖已存在的同名条目 |

仅支持 tar.gz / tgz / tar；传入 `.zip` 明确报错（不支持 zip）。`overwrite` 为 `false` 且目标目录下已存在同名条目时报错。错误：归档不存在；格式不支持；目标目录不存在或无权限；远端缺少 `tar`；只读连接。

### sftp/copy

服务器内复制（对应 tiny-rdm 的 server-internal copy & paste，`FsCopyMove` 语义）。源被复制**进** `toDir`，保留各自的基名；目录递归复制并保留权限（远端 `cp -a --`）。

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `from` | string / string[] | 是 | 单个或多个源路径 |
| `toDir` | string | 是 | 已存在的目标目录 |
| `overwrite` | bool | 否 | 目标已存在时是否覆盖（默认 `false`，不覆盖时报错） |

会话内 `sessionId` 与连接级 `connectionId` 二选一（`connectionId` 自动解析到该连接的存活会话）。返回 `{ success, results: [{ from, to, ok, error? }] }`：逐项执行、逐项回报，`error` 仅出现在失败项上；任一项失败则 `success` 为 `false`。`overwrite: false` 时先用远端 `test -e` 探测目标，已存在直接按项失败。同一目录内的 move 优先走 SFTP rename。错误（整体）：参数缺失或非法；只读连接。

### sftp/move

服务器内剪切 / 移动。参数与返回结构同 `sftp/copy`；远端 `mv -f --`（`overwrite: false` 时先探测目标，存在则按项报错），移动完成后源路径不复存在。sudo 模式不做（tiny-rdm 亦无 sudo copy/move）。远端 `cp` / `mv` 的执行预算为 300 秒。

## 服务器指标

`ssh/metrics` 以单条 POSIX 只读命令采集：`/proc` 读取器、两次 `/proc/stat` 采样（间隔 0.4s）算 CPU 利用率、`df -kP` 采集挂载、`df -iP` 采集 inode 使用率，另附网络与进程扩展——两次网络计数快照（间隔 1s；Linux 读 `/proc/net/dev`，macOS/BSD 回退 `netstat -ibn`）差分出每接口速率，`ps` 分别按 CPU 与内存排序取前 8 进程。

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `sessionId` | string | 是 | 会话 id |
| `cached` | bool | 否 | 为 `true` 时优先返回该会话上一次采集的快照（带 `cachedAt` 字段，Unix 秒）；无缓存时现采并回填。默认 `false` 现采（成功后同样回填缓存） |

返回（关键字段，camelCase）：

```
{
  "hostname": "...", "kernel": "...", "uptimeSeconds": 123456,
  "cpu": { "cores": 8, "percent": 12.3, "load1": 0.4, "load5": 0.3, "load15": 0.2 },
  "memory": { "totalBytes", "availableBytes", "usedBytes", "swapTotalBytes", "swapUsedBytes" },
  "disks": [{ "filesystem", "mount", "totalBytes", "usedBytes", "availableBytes", "percentUsed",
              "inodeUsePercent" }],
  "network":  [{ "name", "rxRate", "txRate", "rxTotal", "txTotal" }],
  "processes": [{ "pid", "user", "cpuPercent", "memPercent", "command" }],
  "topMemory": [{ "pid", "user", "cpuPercent", "memPercent", "command" }],
  "cachedAt": 1756300000        // 仅 cached 命中时出现
}
```

`network` 速率为两次快照间的字节/秒；`rxTotal` / `txTotal` 为第二次快照的累计字节数。`processes` 为按 CPU 排序的前 8 进程，`topMemory` 为按内存占用排序的前 8 进程（两者字段同构，均为扩展字段，旧 sidecar 可能缺失——调用方按可选处理）。`disks[].inodeUsePercent` 为该挂载点的 inode 使用率（百分比数值），仅当 `df -iP` 采集到对应挂载时出现（GNU/busybox 列布局差异由解析端吸收）。`command` 截断到 120 字符。快照缓存仅存内存（每会话一份），会话关闭即清除。只读命令，只读连接同样可用。

## 二进制通道

- `ssh/terminal/in/{sessionId}`：原始终端输入。
- `ssh/terminal/out/{sessionId}`：首字节为流类型，随后为大端 `u64` 单调序号，再后为终端数据。
- `sftp/upload/{taskId}`：大端 `u64` 文件偏移加最多 256 KiB 数据；偏移必须等于服务端期待值。
- `sftp/download/{taskId}`：大端 `u64` 文件偏移加最多 256 KiB 数据。

终端输出保留 2 MiB 环形缓存。前端检测到序号缺口后停止乱序输出并调用 `ssh/terminal/replay`。文件传输采用逐块 RPC 确认，不依赖广播队列可靠送达。

传输状态查询（只读，不产生副作用）：

- `sftp/transfer/list`：参数 `sessionId`。返回 `{ tasks: [...] }`——该会话进行中的上传 / 下载与近期历史（每个会话独立记录），元素结构 `{ taskId, sessionId, direction, fileName, size, transferred, status }`，`status` 取 `running` / `completed` / `cancelled`。
- `sftp/transfer/status`：参数 `taskId`。返回单个任务的同构状态对象；任务不存在时先查历史，仍无则报错。

## 主机密钥

`DBX_PLUGIN_DATA_DIR/known_hosts` 保存插件确认过的主机密钥，同时只读系统 `known_hosts`。未知主机通过 `ssh/host-key/prompt` 事件交给工作台确认；已知主机密钥变化直接拒绝，不能用一次确认覆盖。

框架级连接测试挑战采用事件 `connection/challenge` 和固定响应方法 `connection/challenge/resolve`。原型未声明 `test`，因此暂不触发该流程。

## 本地密钥与 known_hosts

本地能力，不依赖任何连接，均不携带 `sessionId`。对应 tiny-rdm 的 `DiscoverKeys` 与 `ListKnownHosts` / `RemoveKnownHost`。两类方法都只返回元信息（路径、算法、指纹），绝不返回私钥内容或口令。

### keys/discover

参数：无（空对象）。扫描 `~/.ssh` 下默认命名的私钥（`id_rsa` / `id_ed25519` / `id_ecdsa` 等，不含 `.pub`）以及 `~/.ssh/config` 中声明的 `IdentityFile`。返回 `{ keys: [...] }`，元素结构：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `path` | string | 私钥绝对路径 |
| `algorithm` | string | 公钥算法（如 `ssh-ed25519`、`ssh-rsa`、`ecdsa-sha2-nistp256`） |
| `fingerprint` | string | 公钥指纹，`SHA256:…` 格式 |
| `hasPassphrase` | bool | 私钥是否带口令 |

`~/.ssh` 不存在或无可识别私钥时返回空 `keys` 数组，不算错误。

### ssh/knownHosts/list

参数：无。列出插件自身 known_hosts 存储（`DBX_PLUGIN_DATA_DIR/known_hosts`）中的条目；系统 `~/.ssh/known_hosts` 保持只读、不参与管理。返回 `{ entries: [...] }`，元素结构：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `host` | string | 主机名或 IP |
| `port` | number | 端口（默认 22） |
| `keyType` | string | 密钥类型（如 `ssh-ed25519`） |
| `fingerprint` | string | 主机密钥指纹 |
| `marker` | string / null | OpenSSH 标记：`@cert-authority`（CA 信任）或 `@revoked`（吊销）；普通条目为 `null`。标记条目按其 host 字段（可为通配模式）展示与删除 |

列表按 `host` → `port` → `keyType` 稳定排序。通配模式的标记条目（如 `*.prod.example`）仅按字面 host 匹配删除，不做 glob 展开。

### ssh/knownHosts/remove

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `host` | string | 是 | 主机名或 IP |
| `port` | number | 是 | 端口 |

返回 `{ removed: n }`（实际删除的行数，可为 0；同一主机的标记行与普通行一并按 host 匹配删除）。条目删除后，对应主机再次连接会重新触发 `ssh/host-key/prompt` 确认。错误：known_hosts 文件读写失败。

### ssh/sessions/list

参数：无。返回 sidecar 内存中当前跟踪的会话清单（不发起任何 SSH 流量），用于诊断与多面板展示。返回 `{ sessions: [...] }`，按 `createdAt` 升序稳定排序：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `sessionId` | string | 会话 id（与 `ssh/session/open` 返回一致） |
| `connectionId` | string | 所属连接 id |
| `workbenchId` | string | 所属工作台 id |
| `readOnly` | bool | 只读连接标志 |
| `connected` | bool | 会话是否存活（传输层断开后为 `false`） |
| `sudoKeepalive` | bool | 该连接是否运行 Quick Sudo 时间戳保活循环 |
| `createdAt` | number | 会话创建时刻（Unix 秒） |
| `authMethod` | string | 该连接的认证方式名（`password` / `private-key` / `private-key-password` / `agent` / `none`；连接不在注册表时回退 `password`）。仅方法名，**不含任何凭据材料**；供工作台连接信息面板只读展示 |

内存态清单，sidecar 重启即清零；会话关闭（`ssh/session/close`、`workbench/close`、连接断开）后不再出现。
