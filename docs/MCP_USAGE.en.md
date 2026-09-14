# SSH MCP usage

See [the SSH MCP reference](MCP.zh-CN.md) for the tool inventory and security
boundaries. The offline verification path is:

```bash
cargo build --release --manifest-path backend/Cargo.toml
python3 scripts/smoke_mcp.py --binary backend/target/release/dbx-plugin-ssh
```

This smoke does not require a live SSH server. Live connection checks require
explicit credentials and must remain `SKIP` when their environment is absent.
