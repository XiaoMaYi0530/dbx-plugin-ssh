#!/usr/bin/env bash
# Install (or upgrade) the newest dist/*.dbxp into a DBX plugin store using
# the official PluginPackageInstaller (checksum + compatibility verified),
# then restart DBX.
#
# Usage:
#   scripts/install.sh                    # install into the real DBX app store
#   scripts/install.sh --app-data <dir>   # target a custom DBX data directory
#   scripts/install.sh --reinstall        # dev only: drop the installed version first
#   scripts/install.sh --no-restart       # do not relaunch DBX afterwards
#
# Environment:
#   DBX_HOST_WORKTREE   host checkout used for the installer binary
#                       (default: sibling dbx-plugin-host-worktree)
#   DBX_TEST_APP        DBX.app bundle to relaunch (default: probe the host
#                       worktree, then `open -a DBX`)
set -euo pipefail
cd "$(dirname "$0")/.."

APP_DATA="${DBX_APP_DATA:-$HOME/Library/Application Support/com.dbx.app}"
REINSTALL=0
RESTART=1
while [ $# -gt 0 ]; do
  case "$1" in
    --app-data) APP_DATA="$2"; shift 2 ;;
    --reinstall) REINSTALL=1; shift ;;
    --no-restart) RESTART=0; shift ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

DBXP="$(ls -t dist/*.dbxp 2>/dev/null | head -1 || true)"
[ -n "$DBXP" ] || { echo "no .dbxp in dist/ — run scripts/build.sh first" >&2; exit 1; }
VERSION="$(python3 -c "import json;print(json.load(open('manifest.json'))['version'])")"

if [ -z "${DBX_HOST_WORKTREE:-}" ]; then
  for candidate in "$PWD/../dbx-plugin-host-worktree" "$HOME/dbx-plugin-host-worktree"; do
    [ -d "$candidate" ] && DBX_HOST_WORKTREE="$candidate" && break
  done
fi
[ -n "${DBX_HOST_WORKTREE:-}" ] && [ -d "$DBX_HOST_WORKTREE" ] || {
  echo "DBX host worktree not found (set DBX_HOST_WORKTREE)" >&2
  exit 1
}
export PATH="$HOME/.cargo/bin:$PATH"

INSTALLER="$DBX_HOST_WORKTREE/target/release/examples/install_plugin"
if [ ! -x "$INSTALLER" ]; then
  echo "==> building PluginPackageInstaller example"
  (cd "$DBX_HOST_WORKTREE" && cargo build -p dbx-core --example install_plugin --release)
fi

APP_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' \
  /Applications/DBX.app/Contents/Info.plist 2>/dev/null || echo 0.6.0)"

WAS_RUNNING=0
if pgrep -f "DBX.app/Contents/MacOS/dbx" >/dev/null 2>&1; then
  WAS_RUNNING=1
fi
echo "==> stopping DBX"
osascript -e 'quit app id "com.dbx.app"' >/dev/null 2>&1 || true
sleep 2
pkill -f "DBX.app/Contents/MacOS/dbx" 2>/dev/null || true
sleep 1

PLUGIN_STORE="$APP_DATA/plugins"
if [ "$REINSTALL" = 1 ]; then
  VERSION_DIR="$PLUGIN_STORE/io.dbx.ssh/versions/$VERSION"
  if [ -d "$VERSION_DIR" ]; then
    echo "==> dev reinstall: removing installed v$VERSION"
    rm -rf "$VERSION_DIR"
    python3 - "$PLUGIN_STORE/io.dbx.ssh/activations" "$VERSION" <<'PY'
import json, pathlib, sys
store = pathlib.Path(sys.argv[1])
version = sys.argv[2]
if store.is_dir():
    for record in store.glob("*.json"):
        try:
            if json.load(open(record)).get("version") == version:
                record.unlink()
        except Exception:
            pass
PY
  fi
fi

echo "==> installing $(basename "$DBXP") (v$VERSION) into $PLUGIN_STORE"
"$INSTALLER" "$PLUGIN_STORE" "$DBXP" "$APP_VERSION"

if [ "$RESTART" = 1 ] && [ "$WAS_RUNNING" = 1 ]; then
  echo "==> restarting DBX"
  if [ -n "${DBX_TEST_APP:-}" ] && [ -d "$DBX_TEST_APP" ]; then
    open "$DBX_TEST_APP"
  elif [ -d "$DBX_HOST_WORKTREE/target/debug/bundle/macos/DBX.app" ]; then
    open "$DBX_HOST_WORKTREE/target/debug/bundle/macos/DBX.app"
  else
    open -a DBX
  fi
fi
echo "installed v$VERSION"
