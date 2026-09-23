# Spike 报告：X11 转发在 russh 0.62 上的可行性

- 任务来源：IMPL_PLAN v2 Task P3-3（X11 转发 spike，只产出报告不实现功能）
- 分支：`codex/ssh/parity-x11-spike`
- 依据版本：`backend/Cargo.toml:40` 锁定 `russh = { version = "0.62", features = ["des"] }`，实际解析为 **russh 0.62.7**
- 源码证据路径统一缩写为 `russh/` = `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/russh-0.62.7/`
- 编译验证：临时例子 `backend/examples/x11_spike.rs`（不提交、不进依赖树）`cargo check --example x11_spike` 通过，证明下述 API 序列签名真实可用
- 本报告仅为可行性结论，不代表已实现功能

## 1. 结论表

| 能力 | 结论 | 证据（russh/ 下源码） |
| --- | --- | --- |
| 客户端发送 `x11-req`（channel request） | **可行**：公开 API `Channel::request_x11`，无需手工构造 PDU | `src/channels/mod.rs:562`（公开方法，参数含 `single_connection`/`auth_protocol`/`auth_cookie`/`screen_number`）；`src/channels/mod.rs:265`（write_half 委托，`ChannelMsg::RequestX11`）；`src/client/mod.rs:1598-1605`（事件循环分发）；`src/client/session.rs:144-169`（底层编码，第 159 行字面量 `"x11-req"`，同时编码 `want_reply`、`single_connection`、协议/cookie/screen） |
| 客户端接受服务端反向打开的 `x11` channel | **可行**：`client::Handler` trait 方法 `server_channel_open_x11`，可直接覆写；注意 russh **默认实现是 accept**，插件必须显式覆写做 gate | 协议解析 `src/parsing.rs:51-56`（`"x11"` → `ChannelType::X11 { originator_address, originator_port }`）与 `src/parsing.rs:129`（枚举定义）；分发 `src/client/encrypted.rs:825-838`；trait 定义与文档 `src/client/mod.rs:2564-2580`（注释明确 accept/reject/drop 语义，默认实现 `reply.accept()`） |
| direct-tcpip 兜底（连本机 DISPLAY TCP 端口） | **可行但受限**：`Handle::channel_open_direct_tcpip` 现成，backend 已在用；前提是本机 X server 监听 TCP | `src/client/mod.rs:837`；现有调用 `backend/src/ssh.rs:293`、`backend/src/forward.rs:439` |
| 是否需要 fork/升级 russh | **不需要**：X11 解析与分发无 feature 门控（russh `Cargo.toml [features]` 无 x11 项，仅 `_bench/async-trait/aws-lc-rs/default/des/dsa/legacy-ed25519-pkcs8-parser/ring` 等），随默认构建可用 | russh-0.62.7 `Cargo.toml:36-48` |

补充说明：

- `Handle::channel_open_x11`（`src/client/mod.rs:807-818`，对应 `Msg::ChannelOpenX11`，`src/client/mod.rs:174`）是**客户端主动以 `x11` 类型开通道**的 API（用于客户端充当转发服务端一侧）。本场景（接受服务端反向打开）应使用 `Handler::server_channel_open_x11` 回调，不要混淆两者。
- `ChannelMsg::RequestX11` 枚举定义见 `src/channels/mod.rs:65`；russh 的文档注释同时引用 RFC4254 §6.3 提醒 cookie 的安全问题（`src/channels/mod.rs:259-264`）。

### X server 本机监听行为（direct-tcpip 兜底的前提）

| 平台 | 默认行为 | 备注 |
| --- | --- | --- |
| macOS（XQuartz） | **默认禁用 TCP**（等效 `-nolisten tcp`） | 需在 XQuartz 偏好设置勾选 "Allow clients to connect from other computers" 并重启 X11；本机应用实际走 launchd unix socket（`/private/tmp/com.apple.launchd.*/org.xquartz:N`），不走 TCP |
| Windows（VcXsrv/Xming） | 默认监听 TCP `6000+n` | 除非启动参数带 `-nolisten tcp`；多显示器/多 display 编号注意端口偏移 |
| Linux（Xorg） | 发行版普遍默认 `-nolisten tcp`（Debian/Ubuntu/Fedora 等） | XWayland 同样默认不开 TCP；连接方须为本机同用户 |
| 兜底路线评估 | 能通，但要求用户改本机 X server 配置，体验差、误配面大 | 不建议作为主路线；仅在 unix socket 桥接不可用时作为调试手段 |

## 2. 推荐实现路线（API 序列草图，非实现）

前提：backend 已有 `impl client::Handler for SshClient`（`backend/src/ssh.rs:635`）与 `channel_open_session` 用法（`backend/src/exec.rs:893` 等），路线完全贴合现有架构。

```text
会话开启 X11（连接级开关，read_only 连接必须拒绝）:
1. handle.channel_open_session()                       // Channel<Msg>
2. channel.request_x11(
       want_reply        = false,
       single_connection = false,                       // 允许多条 x11 channel 复用
       auth_protocol     = "MIT-MAGIC-COOKIE-1",
       auth_cookie       = <本会话随机生成的 16 字节假 cookie 的 hex>,
       screen_number     = 0,
   )                                                    // channels/mod.rs:562
3. 正常 exec/shell；服务端会为远端进程注入 DISPLAY（通常 :10.0）

服务端反向打开 x11 channel（Handler 回调，backend/src/ssh.rs 的 SshClient 上覆写）:
4. fn server_channel_open_x11(channel, originator_address, originator_port, reply, session)
   - gate：仅当本连接开启过 X11 且非 read_only 才继续，否则 reply.reject(...)
   - 读 channel 首包，校验 X11 auth proto/cookie == 第 2 步下发的假 cookie，不匹配即 reject
5. reply.accept().await
6. 解析本机 $DISPLAY：
   - unix:/path:N（macOS launchd socket、Linux）→ 连接 unix socket（优先）
   - host:N → 连接 127.0.0.1:(6000+N)（兜底）
7. channel.into_stream() ↔ 本机 socket 双向桥接（tokio::io::copy × 2）
   // into_stream: channels/mod.rs:661
8. 任一侧关闭时对称关闭另一侧；会话断开时清理全部 x11 桥接任务
```

direct-tcpip 兜底（仅调试用）：第 6-7 步替换为 `handle.channel_open_direct_tcpip("127.0.0.1", 6000+N, "127.0.0.1", 0)`，其余相同——但要先满足上文"X server 监听 TCP"前提。

编译验证过的调用序列见 `backend/examples/x11_spike.rs`（临时文件，未提交；`cargo check --example x11_spike` 通过）。

## 3. 安全警示清单

1. **X 协议面全量暴露**：X11 协议无加密且授权粒度极粗——能连上 display 的远端进程可读取全部按键/剪贴板、截屏、注入输入到本机所有窗口。桥接等于把本机 X server 的控制权交给远端主机上的任意进程。仅对用户显式开启且远端可信的连接提供。
2. **read_only 连接默认禁用**：`read_only` 模式的会话不得发送 `x11-req`，`server_channel_open_x11` 回调对未启用 X11 的连接必须 `reject`。
3. **不要泄露真实 X cookie**：`request_x11` 的 cookie 参数会明文发给服务端（并被 sshd 用于 xauth 配置）。应生成一次性假 cookie 下发，由本机桥接层校验首包后再放行；真实 `XAUTHORITY` 不出本机。
4. **russh 默认 accept 的风险**：`server_channel_open_x11` 默认实现直接 `reply.accept()`（`src/client/mod.rs:2572-2575`）。插件必须覆写并做会话级 gate，否则恶意/被入侵的服务端可随时反向打开 x11 channel。
5. **本机监听面**：优先 unix socket 桥接，避免诱导用户开启 X server TCP 监听；TCP 兜底目标固定 `127.0.0.1`，不绑定 `0.0.0.0`。
6. **资源控制**：每条 x11 channel 产生两个桥接任务，需按连接限制并发 x11 channel 数量并随会话关闭统一回收，防 DoS。
7. **originator 字段不可信**：`originator_address/port` 来自服务端声明，仅用于日志，不用于任何访问判定。

## 4. 工作量估算与立项建议

| 工作包 | 估算 |
| --- | --- |
| 协议层：request_x11 封装、Handler 回调 gate、假 cookie 生成与首包校验 | 1-1.5 人日 |
| 本机 DISPLAY 解析与桥接（unix socket 优先 + TCP 兜底，跨平台探测 XQuartz launchd socket） | 1.5-2 人日 |
| 前端 UI（连接级开关、错误提示）与 manifest/贡献点核对 | 1-1.5 人日 |
| 测试（OpenSSH `-X` 对拍、read_only 拒绝路径、单测）+ 文档 | 1-1.5 人日 |
| **合计** | **4.5-6.5 人日（建议按 5±1.5 人日排期）** |

**立项建议**：建议进入 M4，定位为"标准 OpenSSH `-X` 对等"能力，排在核心 parity 项之后；若 M4 排期紧张，可作为 M4 stretch/首个 M5 项。理由：russh 0.62 原生支持、无需 fork/升级、与现有 `client::Handler`/forward 架构零冲突，边际成本低；安全面已明确可控（假 cookie + 会话 gate + read_only 禁用）。direct-tcpip 兜底不单独立项（成本≈0，但受"X server 需开 TCP 监听"限制，仅作诊断手段）。
