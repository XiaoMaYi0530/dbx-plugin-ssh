#!/usr/bin/env python3
"""MCP stdio smoke test for the plugin's --mcp server mode.

Spawns the sidecar with --mcp and verifies, against the real process:
  1. initialize handshake (protocol version + server info),
  2. tools/list contents and schemas,
  3. argument validation ordering on a connection-bound tool,
  4. a real tools/call round-trip (ssh_list_known_hosts).

Usage:
    python3 scripts/smoke_mcp.py [--binary backend/target/release/dbx-plugin-ssh-sftp]
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys

EXPECTED_TOOLS = [
    "ssh_exec",
    "ssh_exec_sudo",
    "ssh_metrics",
    "ssh_close",
    "ssh_test_connection",
    "ssh_list_known_hosts",
    "ssh_remove_known_host",
    "sftp_list_dir",
    "sftp_stat",
    "sftp_exists",
    "sftp_read_file",
    "sftp_write_file",
    "sftp_mkdir",
    "sftp_remove",
    "sftp_rename",
    "sftp_chmod",
    "sftp_disk_usage",
]


def send(proc: subprocess.Popen, payload: dict) -> None:
    assert proc.stdin is not None
    proc.stdin.write((json.dumps(payload) + "\n").encode())
    proc.stdin.flush()


def recv(proc: subprocess.Popen, want_id: int) -> dict:
    assert proc.stdout is not None
    while True:
        line = proc.stdout.readline()
        if not line:
            raise AssertionError("sidecar closed the stream before replying")
        message = json.loads(line)
        if message.get("id") == want_id:
            return message


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="backend/target/release/dbx-plugin-ssh-sftp")
    args = parser.parse_args()

    proc = subprocess.Popen(
        [args.binary, "--mcp"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
    )
    try:
        send(proc, {
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05", "capabilities": {},
                "clientInfo": {"name": "smoke", "version": "0"},
            },
        })
        init = recv(proc, 1)["result"]
        assert init["protocolVersion"] == "2024-11-05", init
        print(f"initialize ok: {init['serverInfo']['name']} {init['serverInfo']['version']}")

        send(proc, {"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
        tools = recv(proc, 2)["result"]["tools"]
        names = [tool["name"] for tool in tools]
        missing = [name for name in EXPECTED_TOOLS if name not in names]
        assert not missing, f"missing tools: {missing}"
        assert all(tool["inputSchema"].get("type") == "object" for tool in tools), "bad schemas"
        print(f"tools/list ok: {len(names)} tools")

        # A connection-bound tool without credentials must be rejected before
        # any network I/O — and jump-host validation must not be masked by it.
        send(proc, {
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "ssh_exec", "arguments": {
                "host": "203.0.113.1", "username": "u", "command": "true",
            }},
        })
        error = recv(proc, 3)["error"]["message"]
        assert "password" in error, f"unexpected error: {error}"
        print("parameter validation ok")

        # A connection-free tool round-trips through the real sidecar path.
        send(proc, {
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": {"name": "ssh_list_known_hosts", "arguments": {}},
        })
        result = recv(proc, 4)["result"]
        assert not result.get("isError", False), f"tool failed: {result}"
        parsed = json.loads(result["content"][0]["text"])
        assert isinstance(parsed["knownHosts"], list)
        print("tools/call round-trip ok")
    finally:
        if proc.stdin:
            proc.stdin.close()
        proc.wait(timeout=10)
    print("MCP smoke: all green")


if __name__ == "__main__":
    try:
        main()
    except AssertionError as error:
        print(f"FAIL: {error}", file=sys.stderr)
        sys.exit(1)
