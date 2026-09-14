# SSH MCP 使用说明

SSH MCP 的协议、工具列表和安全边界见 [MCP 参考](MCP.zh-CN.md)。离线验证可直接
运行：

```bash
cargo build --release --manifest-path backend/Cargo.toml
python3 scripts/smoke_mcp.py --binary backend/target/release/dbx-plugin-ssh
```

该 smoke 不需要真实 SSH 服务；涉及真实连接的参数必须显式提供，并在环境不可用时
按脚本输出 `SKIP`，不能把离线通过当作 live 连接通过。
