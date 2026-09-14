# DBX SSH Terminal

[![CI](https://github.com/jinpy666/dbx-plugin-ssh/actions/workflows/ci.yml/badge.svg)](https://github.com/jinpy666/dbx-plugin-ssh/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/jinpy666/dbx-plugin-ssh?display_name=tag)](https://github.com/jinpy666/dbx-plugin-ssh/releases)

[中文](README.md) · [Showcase](docs/MEDIA.en.md) · [Feature comparison](docs/COMPARISON.en.md) · [Repository split notes](docs/REPOSITORY_SPLIT.en.md)

DBX SSH Terminal is a server terminal for modern operations teams. Open one connection
and move from terminal work to file transfer, jump-host access, guarded elevation, and
MCP automation without losing context. It turns the next hour after “logging into a
server” into one coherent, reusable, auditable workflow.

> SSH Terminal · SFTP · Remote Operations: one DBX workspace for interactive terminals, files, automation, and guarded server operations.

## See it in action

[Watch the 87-second live workflow demo (click to play)](docs/media/dbx-ssh-live-demo.mp4) ·
[Watch the 15-second feature preview](docs/media/dbx-ssh-demo.mp4)

![DBX SSH & SFTP product overview](docs/media/dbx-ssh-overview.png)

If your workflow jumps between a terminal, a separate SFTP client, jump-host config files,
and one-off automation scripts, DBX SSH & SFTP gives you a simpler center of gravity:
connections, terminal, files, and automation share the same DBX connection context.

## Why teams reach for it

| Your job | The DBX SSH & SFTP workflow |
| --- | --- |
| Resolve an incident quickly | PTY terminal, command history, quick commands, cancellable commands, and directory tracking |
| Move files safely | SFTP preview, drag and drop, progress, cancellation, acknowledgements, and atomic replacement |
| Reach private servers | Up to three ProxyJump hops, keepalive, and Known Hosts verification |
| Put guardrails around risk | Read-only mode, selectable sudo source, command allowlists, and host secret bindings |
| Automate repeatable work | MCP tools that reuse connections, approvals, and permission boundaries |

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

See the [feature and competitor comparison](docs/COMPARISON.en.md) for a capability matrix
covering OpenSSH + sftp, Tabby, Termius, FinalShell, electerm, and iSHell Pro.

> **Protocol roadmap:** the current release focuses on SSH/SFTP operations. RDP, Telnet, and
> additional remote-access protocols are still being strengthened; check the release notes for
> the exact capability in each version.

More screenshots live in the [showcase page](docs/MEDIA.en.md).

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

## Installation

Download the platform-specific `.dbxp` package from
[GitHub Releases](https://github.com/jinpy666/dbx-plugin-ssh/releases), then install it from
the DBX plugin center. Developers can build a candidate package using the
[repository split and release notes](docs/REPOSITORY_SPLIT.en.md).

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
