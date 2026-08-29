# DBX SSH & SFTP Plugin

DBX Host API 1.1 SSH/SFTP plugin. Version `0.2.1` restores the workbench from
`origin/feature/ssh-sftp-workbench` as an independent Vue/Vite UI and Rust
Sidecar; it does not import DBX internal Vue components.

## Features

- Password, private-key, private-key→password, SSH Agent and none authentication.
- Persistent `known_hosts`, changed-key rejection, and Host-scoped fingerprint challenges before credentials are sent.
- Independent SSH/PTTY/SFTP state per `connectionId → workbenchId → sessionId`.
- xterm terminal with resize, replay, tab re-attach, clipboard menu, Bash/Zsh OSC 7 directory following, and multi-file `rz` upload.
- SFTP list/sort/columns, preview, rename, create, recursive delete without following symlinks, drag/drop, and read-only enforcement.
- 256 KiB streaming Host file handles, three transfer slots, offset/ACK recovery, cancel, progress/speed, `.part` upload and atomic replacement.
- Dark/light appearance synchronization and English, Spanish, Italian, Japanese, Portuguese, Simplified Chinese and Traditional Chinese UI.
- Quick Sudo: password injected over stdin for `sudo -S`, 2FA/TOTP prompts answered automatically (RFC 6238, otpauth/base32/static secrets), login-time keyboard-interactive 2FA, and in-terminal sudo auto-answer.
- ProxyJump chains (up to 3 hops), keepalive dead-connection detection, cancellable remote commands, server metrics panel, chmod, and per-directory disk usage.
- MCP stdio mode: run the binary with `--mcp` to expose `ssh_exec`, `ssh_exec_sudo`, `ssh_metrics` and a full `sftp_*` tool family to MCP clients (see [`docs/MCP.zh-CN.md`](docs/MCP.zh-CN.md)).

`0.2.1` requires the local Host API 1.1 changes on branch
`codex/plugin-host-ssh-enablers`. It is intentionally not published until the
Host/SDK contract is accepted.

## Install & verify

- `scripts/build.sh` — frontend checks + self-contained UI + `.dbxp` package into `dist/`.
- `scripts/install.sh` — install the newest `.dbxp` into the local DBX plugin store with the official `PluginPackageInstaller` (checksum + compatibility verified), then restart DBX. Options: `--app-data <dir>` (custom store), `--reinstall` (dev: drop the same version first), `--no-restart`. Needs the sibling `dbx-plugin-host-worktree` checkout (or `DBX_HOST_WORKTREE`).
- `scripts/test.sh` — full suite: backend unit tests, frontend typecheck/test/build, sidecar release build, `.dbxp` package, MCP stdio smoke, and the host-side install-pipeline integration test (installer + MCP bridge over a real sidecar). `--skip-host` skips that last step.
- `scripts/smoke_mcp.py` — standalone MCP stdio smoke against `--mcp` mode.
- `scripts/smoke_test.py` / `smoke_fs_test.py` — end-to-end sidecar smokes against a live SSH host (see script headers).

## Development

```bash
cd frontend
pnpm install
pnpm typecheck
pnpm test
pnpm build
cd ..
DBX_PLUGIN_SDK_ROOT=../dbx-plugin-host-worktree dbx-plugin package .
```

PowerShell uses `$env:DBX_PLUGIN_SDK_ROOT = "..\\dbx-plugin-host-worktree"`
before running `dbx-plugin package .`.

The visual fixture is available at `frontend/visual.html` when running Vite.
It supplies deterministic SSH/SFTP data for 1440×900 and 1920×1080 UI checks.

Framework validation notes are maintained in
[`docs/FRAMEWORK_FEEDBACK.zh-CN.md`](docs/FRAMEWORK_FEEDBACK.zh-CN.md).
