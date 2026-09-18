#!/usr/bin/env python3
"""Guard: the packaged sidecar must not be built from the CLI-bundled SDK.

``dbx-plugin package`` runs its own cargo build with the Rust SDK overridden to
``<DBX_PLUGIN_SDK_ROOT>/plugins/sdk/rust/dbx-plugin-sdk``; when that variable is
unset, the npm wrapper injects the CLI's bundled sdk-root, silently dropping
every vendored-SDK change (e.g. the 0.4.79 keystroke-ordering fix). Packaging
scripts must set ``DBX_PLUGIN_SDK_ROOT`` via ``scripts/sdk_root_shim.sh``
first. This check byte-greps the newest dist/*.dbxp sidecar for the bundled
root marker ``sdk-root`` — a substring of the CLI's install path only; neither
the vendored path nor the shim path (dbx-vendored-sdk.*) contains it.
"""
import re
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
packages = sorted((ROOT / "dist").glob("*.dbxp"), key=lambda p: p.stat().st_mtime)
if not packages:
    sys.exit("FAIL: no dist/*.dbxp found — run the package step first")
package = packages[-1]

with zipfile.ZipFile(package) as archive:
    binaries = [name for name in archive.namelist() if re.fullmatch(r"bin/[^/]+/dbx-plugin-ssh(?:\.exe)?", name)]
    if not binaries:
        sys.exit(f"FAIL: {package.name} has no sidecar under bin/")
    for name in binaries:
        if b"sdk-root" in archive.read(name):
            sys.exit(
                f"FAIL: {package.name}::{name} embeds a 'sdk-root' path — built from the CLI-bundled "
                'SDK. Package with DBX_PLUGIN_SDK_ROOT="$(bash scripts/sdk_root_shim.sh)".'
            )

print(f"OK: {package.name} sidecar not built from the CLI-bundled SDK")
