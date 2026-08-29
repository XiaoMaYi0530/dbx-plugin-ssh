#!/usr/bin/env bash
# Build the plugin frontend and package a .dbxp for the current platform.
# Requires: node + pnpm on PATH, cargo for the Rust sidecar.
# The dbx-plugin CLI bundles its own SDK (no DBX source checkout needed).
set -euo pipefail
cd "$(dirname "$0")/.."

if ! command -v pnpm >/dev/null; then
  export PATH="$HOME/Library/pnpm:$HOME/.nvm/versions/node/v22.21.0/bin:$PATH"
fi
export PATH="$HOME/.cargo/bin:$PATH"

echo "==> frontend: install + typecheck + test + build"
pnpm --dir frontend install --frozen-lockfile
pnpm --dir frontend typecheck
pnpm --dir frontend test
pnpm --dir frontend build

echo "==> package .dbxp"
unset DBX_PLUGIN_SDK_ROOT
# The dbx-plugin CLI ships with the SDK checkout; build it on first use.
if ! command -v dbx-plugin >/dev/null 2>&1; then
  HOST="${DBX_HOST_WORKTREE:-$PWD/../dbx-plugin-host-worktree}"
  if [ ! -x "$HOST/plugins/sdk/cli/target/release/dbx-plugin" ]; then
    (cd "$HOST/plugins/sdk/cli" && cargo build --release)
  fi
  export PATH="$HOST/plugins/sdk/cli/target/release:$PATH"
fi
NO_COLOR=1 dbx-plugin package .

echo
echo "Artifacts:"
ls -la dist/*.dbxp dist/*.artifact.json
