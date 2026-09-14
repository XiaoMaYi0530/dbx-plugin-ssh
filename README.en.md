# DBX SSH & SFTP

[中文](README.md) · [Repository split notes](docs/REPOSITORY_SPLIT.en.md)

DBX SSH & SFTP is a practical workspace for server operations. It brings an
interactive terminal, remote commands, SFTP file management, and secure
authentication into one consistent interface.

![DBX SSH & SFTP workspace](docs/screenshots-a-ssh/01-toolbar-dark.png)

## Use cases

- Inspect Linux and Unix servers, review logs, and run remote commands.
- Reach private environments through jump hosts while keeping terminal and SFTP in one workspace.
- Upload and download configuration files, scripts, and build artifacts under controlled permissions.

## Highlights

- Password, private-key, SSH Agent, keyboard-interactive, and passwordless authentication.
- Interactive PTY terminal with resize, session recovery, clipboard support,
  remote-directory tracking, and batch input.
- SFTP browsing, sorting, preview, upload, download, rename, create, drag and drop,
  and recursive delete.
- Large-file transfers with progress, cancellation, acknowledgements, and atomic replacement.
- Known Hosts verification, changed-key rejection, read-only mode, and directory disk usage.
- ProxyJump chains up to three hops, keepalive, cancellable remote commands, chmod,
  and Quick Sudo.
- TOTP and keyboard-interactive two-factor authentication, plus MCP tools for automation clients.
- Simplified Chinese, Traditional Chinese, English, Spanish, Italian, Japanese,
  and Portuguese UI.

![Quick commands and terminal operations](docs/screenshots-a-ssh/02-quick-commands.png)

## MCP automation

Use the DBX MCP bridge to reuse saved connections, approvals, and sudo allowlists.
For standalone read-only access:

```bash
DBX_SSH_MCP_READ_ONLY=1 backend/target/release/dbx-plugin-ssh --mcp
```

Useful tools include `ssh_exec`, `ssh_metrics`, `sftp_list`, `sftp_upload`, and
`sftp_download`. See the [MCP guide](docs/MCP_USAGE.en.md) and the
[SSH MCP reference](docs/MCP.zh-CN.md) for configuration and safety details.

## Security

Passwords, private-key passphrases, TOTP secrets, and sudo credentials are managed
through DBX host secret bindings. The plugin does not write credentials to config
files, logs, or exports. For production connections, enable strict Known Hosts
verification and use read-only mode or a sudo command allowlist when appropriate.

## Development

```bash
pnpm --dir frontend install
pnpm --dir frontend typecheck && pnpm --dir frontend test && pnpm --dir frontend build
cargo test --manifest-path backend/Cargo.toml
python3 scripts/validate_repo.py && node scripts/connection-forms/verify.mjs
scripts/test.sh --skip-host
```

Protocol, packaging, and integration details live under `docs/`. Contributors
should read the [repository split notes](docs/REPOSITORY_SPLIT.en.md) first.
