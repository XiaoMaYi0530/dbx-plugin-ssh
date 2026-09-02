#!/usr/bin/env python3
"""MCP stdio smoke test for the plugin's --mcp server mode.

Spawns the sidecar with --mcp and verifies, against the real process:
  1. initialize handshake (protocol version + server info),
  2. tools/list contents and schemas,
  3. argument validation ordering on a connection-bound tool,
  4. a real tools/call round-trip (ssh_list_known_hosts),
  5. global Quick Sudo profile management round-trip (save/list/delete),
     including the quickSudoProfile argument on ssh_exec_sudo,
  6. local-side validation of the transfer tools (sftp_upload /
     sftp_download) failing before any connection attempt,
  7. production misoperation guards: destructive commands require
     confirmDestructive (gate fires before credential validation), and a
     second server started with DBX_SSH_MCP_READ_ONLY=1 enforces the
     read-only gates process-wide (write tools refused, ssh_exec limited
     to whitelisted inspection commands, confirmation cannot override),
  8. the stdio app-bridge path failing with an actionable error when the
     DBX app has not published its bridge port (empty app-data dir, no-op
     launch command — no UI, no SSH server involved).

With --host (plus --username/--password, or the DBX_SSH_SMOKE_PASSWORD
environment variable) a live section additionally runs a real round-trip
against an SSH server: ssh_exec, ssh_metrics, sftp_pwd, sftp_list_dir and
an sftp_upload → sftp_download loop compared byte-for-byte. The container
from docs (linuxserver/openssh-server on 127.0.0.1:2222, user `sshuser`)
is the intended target; credentials never live in this file.

Usage:
    python3 scripts/smoke_mcp.py [--binary backend/target/release/dbx-plugin-ssh]
                                 [--host 127.0.0.1 --port 2222
                                  --username sshuser --password ...]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import uuid

EXPECTED_TOOLS = [
    "ssh_exec",
    "ssh_exec_sudo",
    "ssh_metrics",
    "ssh_close",
    "ssh_test_connection",
    "ssh_list_known_hosts",
    "ssh_remove_known_host",
    "ssh_quick_sudo_profiles_list",
    "ssh_quick_sudo_profiles_save",
    "ssh_quick_sudo_profiles_delete",
    "sftp_list_dir",
    "sftp_stat",
    "sftp_exists",
    "sftp_pwd",
    "sftp_read_file",
    "sftp_write_file",
    "sftp_mkdir",
    "sftp_remove",
    "sftp_rename",
    "sftp_chmod",
    "sftp_copy",
    "sftp_move",
    "sftp_disk_usage",
    "sftp_upload",
    "sftp_download",
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


def call_tool(proc: subprocess.Popen, next_id: list, name: str, arguments: dict) -> dict:
    next_id[0] += 1
    send(proc, {
        "jsonrpc": "2.0", "id": next_id[0], "method": "tools/call",
        "params": {"name": name, "arguments": arguments},
    })
    message = recv(proc, next_id[0])
    if "error" in message:
        raise AssertionError(f"{name} errored: {message['error'].get('message')}")
    result = message["result"]
    if result.get("isError", False):
        raise AssertionError(f"{name} failed: {result}")
    return json.loads(result["content"][0]["text"])


def live_round_trip(proc: subprocess.Popen, args, id_base: int) -> None:
    """Real-server section: exec, metrics, browse, and an upload/download
    loop compared byte-for-byte (mirrors what an MCP client such as ZCode
    does end to end)."""
    next_id = [id_base]
    connection = {
        "host": args.host, "port": args.port,
        "username": args.username, "password": args.password,
    }

    # Standalone MCP mode TOFU-trusts unknown host keys, so a first-time
    # connection succeeds without a prior manual confirmation.
    pong_test = call_tool(proc, next_id, "ssh_test_connection", dict(connection))
    assert pong_test["ok"] is True, pong_test
    print("ssh_test_connection ok")

    pong = call_tool(proc, next_id, "ssh_exec", {
        **connection, "command": "echo smoke-$((40+2))",
    })
    assert pong["output"].strip() == "smoke-42", pong
    assert pong["exitCode"] == 0, pong

    # The destructive gate holds on the live path too: a catastrophic
    # command is refused on a healthy connection without the flag.
    next_id[0] += 1
    send(proc, {
        "jsonrpc": "2.0", "id": next_id[0], "method": "tools/call",
        "params": {"name": "ssh_exec", "arguments": {
            **connection, "command": "rm -rf /etc",
        }},
    })
    refused_live = recv(proc, next_id[0])["error"]["message"]
    assert "confirmDestructive" in refused_live, refused_live
    print("live destructive-command gate ok")

    metrics = call_tool(proc, next_id, "ssh_metrics", dict(connection))
    assert metrics, "empty metrics"

    home = call_tool(proc, next_id, "sftp_pwd", dict(connection))["home"]
    assert home.startswith("/"), home

    listing = call_tool(proc, next_id, "sftp_list_dir", {
        **connection, "path": home,
    })
    assert isinstance(listing["entries"], list), listing

    # Transfer loop: upload a random payload, read it back via download,
    # compare SHA-256 digests on both sides.
    payload = uuid.uuid4().hex.encode() * 257  # ~8 KiB deterministic blob
    digest = hashlib.sha256(payload).hexdigest()
    remote_path = f"/tmp/smoke-mcp-{uuid.uuid4().hex}.bin"
    local_dir = tempfile.mkdtemp(prefix="smoke-mcp-")
    local_path = f"{local_dir}/roundtrip.bin"
    try:
        uploaded = call_tool(proc, next_id, "sftp_upload", {
            **connection, "localPath": _spool(payload), "remotePath": remote_path,
        })
        assert uploaded["bytes"] == len(payload), uploaded
        remote_digest = call_tool(proc, next_id, "ssh_exec", {
            **connection, "command": f"sha256sum {remote_path}",
        })["output"].split()[0]
        assert remote_digest == digest, f"remote sha mismatch: {remote_digest}"

        call_tool(proc, next_id, "sftp_download", {
            **connection, "remotePath": remote_path, "localPath": local_path,
        })
        with open(local_path, "rb") as handle:
            assert hashlib.sha256(handle.read()).hexdigest() == digest, "local sha mismatch"

        # The downloaded file exists: a second download without overwrite
        # must be refused (proving the local-target guard on a live call).
        send(proc, {
            "jsonrpc": "2.0", "id": next_id[0] + 1, "method": "tools/call",
            "params": {"name": "sftp_download", "arguments": {
                **connection, "remotePath": remote_path, "localPath": local_path,
            }},
        })
        next_id[0] += 1
        refused = recv(proc, next_id[0])["error"]["message"]
        assert "Local path already exists" in refused, refused
    finally:
        try:
            call_tool(proc, next_id, "sftp_remove", {
                **connection, "path": remote_path,
            })
        except AssertionError as cleanup_error:
            print(f"cleanup warning: {cleanup_error}")
    call_tool(proc, next_id, "ssh_close", dict(connection))
    print("live round-trip ok (exec/metrics/pwd/list/upload/download)")


def _spool(data: bytes) -> str:
    handle = tempfile.NamedTemporaryFile(prefix="smoke-mcp-src-", delete=False)
    handle.write(data)
    handle.close()
    return handle.name


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="backend/target/release/dbx-plugin-ssh")
    parser.add_argument("--host", help="enable the live section against this SSH server")
    parser.add_argument("--port", type=int, default=2222)
    parser.add_argument("--username", default="sshuser")
    parser.add_argument(
        "--password",
        default=os.environ.get("DBX_SSH_SMOKE_PASSWORD", ""),
        help="live-section password (or set DBX_SSH_SMOKE_PASSWORD)",
    )
    args = parser.parse_args()
    if args.host and not args.password:
        parser.error("--host requires --password or DBX_SSH_SMOKE_PASSWORD")

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

        # Global Quick Sudo profiles: save/list/delete round-trip with a
        # runtime-assembled test secret; responses must never echo it.
        secret = f"smoke-{uuid.uuid4().hex}"
        profile_name = f"smoke-{uuid.uuid4().hex[:8]}"
        send(proc, {
            "jsonrpc": "2.0", "id": 5, "method": "tools/call",
            "params": {"name": "ssh_quick_sudo_profiles_save", "arguments": {
                "name": profile_name,
                "sudoPassword": secret,
                "totpSecret": "JBSWY3DPEHPK3PXP",
                "authFlowMode": "password_plus_otp",
                "sudoUsePty": True,
            }},
        })
        saved = recv(proc, 5)["result"]
        assert not saved.get("isError", False), f"save failed: {saved}"
        saved_profile = json.loads(saved["content"][0]["text"])
        profile_id = saved_profile["profile"]["id"]
        assert saved_profile["created"] is True, saved_profile
        assert saved_profile["profile"]["sudoPasswordSet"] is True, saved_profile
        assert secret not in saved["content"][0]["text"], "save echoed the secret"

        send(proc, {
            "jsonrpc": "2.0", "id": 6, "method": "tools/call",
            "params": {"name": "ssh_quick_sudo_profiles_list", "arguments": {}},
        })
        listed = recv(proc, 6)["result"]
        listed_text = listed["content"][0]["text"]
        assert secret not in listed_text, "list echoed the secret"
        listed_profiles = json.loads(listed_text)["profiles"]
        assert any(profile.get("sudoPasswordSet") is True for profile in listed_profiles), listed_text

        # ssh_exec_sudo schema exposes quickSudoProfile; an unknown reference
        # must fail fast with a clear error (before any connection attempt).
        schema_properties = next(
            tool["inputSchema"]["properties"] for tool in tools if tool["name"] == "ssh_exec_sudo"
        )
        assert "quickSudoProfile" in schema_properties, "ssh_exec_sudo lacks quickSudoProfile"
        send(proc, {
            "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": {"name": "ssh_exec_sudo", "arguments": {
                "host": "203.0.113.1", "username": "u", "command": "true",
                "quickSudoProfile": "no-such-profile",
            }},
        })
        missing_profile = recv(proc, 7)["error"]["message"]
        assert "not found" in missing_profile, f"unexpected error: {missing_profile}"

        send(proc, {
            "jsonrpc": "2.0", "id": 8, "method": "tools/call",
            "params": {"name": "ssh_quick_sudo_profiles_delete", "arguments": {"id": profile_id}},
        })
        removed = recv(proc, 8)["result"]
        assert json.loads(removed["content"][0]["text"])["removed"] is True, removed
        print("quick sudo profiles round-trip ok")

        # Agent terminal routing is embedded-bridge only: stdio mode must
        # refuse runInTerminal before any connection I/O (ids skip into the
        # 20s to stay clear of the transfer section below).
        exec_schema = next(
            tool["inputSchema"]["properties"] for tool in tools if tool["name"] == "ssh_exec"
        )
        assert "runInTerminal" in exec_schema, "ssh_exec lacks runInTerminal"
        # Strict MCP hosts drop undeclared arguments, so connectionId must be
        # part of the advertised schema or runInTerminal is unreachable there.
        assert "connectionId" in exec_schema, "ssh_exec lacks connectionId"
        send(proc, {
            "jsonrpc": "2.0", "id": 20, "method": "tools/call",
            "params": {"name": "ssh_exec", "arguments": {
                "host": "203.0.113.1", "username": "u",
                "command": "true", "runInTerminal": True,
            }},
        })
        bridge_error = recv(proc, 20)["error"]["message"]
        assert "runInTerminal needs a saved DBX connection" in bridge_error, \
            f"unexpected error: {bridge_error}"
        print("runInTerminal stdio refusal ok")

        # Transfer tools validate their local side before dialing, so a bad
        # local path must fail fast (no SSH server involved) with a clear
        # error naming the offending parameter.
        send(proc, {
            "jsonrpc": "2.0", "id": 9, "method": "tools/call",
            "params": {"name": "sftp_upload", "arguments": {
                "host": "203.0.113.1", "username": "u",
                "localPath": "/no/such/smoke-file.bin", "remotePath": "/tmp/x",
            }},
        })
        upload_error = recv(proc, 9)["error"]["message"]
        assert "Cannot read local file" in upload_error, f"unexpected error: {upload_error}"
        # A local target whose parent is an existing *file* cannot have its
        # parent directories created — a deterministic, dial-free failure.
        parent_as_file = tempfile.NamedTemporaryFile(suffix=".txt", delete=False)
        parent_as_file.close()
        send(proc, {
            "jsonrpc": "2.0", "id": 10, "method": "tools/call",
            "params": {"name": "sftp_download", "arguments": {
                "host": "203.0.113.1", "username": "u",
                "remotePath": "/tmp/x",
                "localPath": parent_as_file.name + "/smoke.bin",
            }},
        })
        download_error = recv(proc, 10)["error"]["message"]
        assert "Cannot create local directory" in download_error, (
            f"unexpected error: {download_error}"
        )
        print("transfer tool local validation ok")

        # Production misoperation guards: destructive commands demand an
        # explicit confirmDestructive flag; the refusal fires before
        # credential validation, and the flag lets the call proceed.
        send(proc, {
            "jsonrpc": "2.0", "id": 11, "method": "tools/call",
            "params": {"name": "ssh_exec", "arguments": {
                "host": "203.0.113.1", "username": "u",
                "command": "mkfs.ext4 /dev/sda1",
            }},
        })
        refused = recv(proc, 11)["error"]["message"]
        assert "confirmDestructive" in refused and "password" not in refused, refused
        send(proc, {
            "jsonrpc": "2.0", "id": 12, "method": "tools/call",
            "params": {"name": "ssh_exec", "arguments": {
                "host": "203.0.113.1", "username": "u",
                "command": "mkfs.ext4 /dev/sda1",
                "confirmDestructive": True,
            }},
        })
        gated_through = recv(proc, 12)["error"]["message"]
        assert "password" in gated_through, gated_through
        print("destructive-command gate ok")

        if args.host:
            live_round_trip(proc, args, id_base=100)
        else:
            print("live section skipped (no --host)")
    finally:
        if proc.stdin:
            proc.stdin.close()
        proc.wait(timeout=10)
    read_only_server_section(args)
    bridge_unreachable_section(args)
    print("MCP smoke: all green")


def read_only_server_section(args: argparse.Namespace) -> None:
    """Second sidecar started with DBX_SSH_MCP_READ_ONLY=1: the whole
    process must pass the read-only gates (operator kill switch for
    production access)."""
    env = dict(os.environ, DBX_SSH_MCP_READ_ONLY="1")
    proc = subprocess.Popen(
        [args.binary, "--mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env,
    )
    next_id = [50]

    def expect_error(command: str, name: str = "ssh_exec", arguments: dict | None = None):
        arguments = {"host": "203.0.113.1", "username": "u", "command": command, **(arguments or {})}
        next_id[0] += 1
        send(proc, {
            "jsonrpc": "2.0", "id": next_id[0], "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        })
        return recv(proc, next_id[0])["error"]["message"]

    try:
        # Write-class tools are rejected outright.
        write_refusal = expect_error("uptime", name="ssh_exec_sudo")
        assert "read-only" in write_refusal, write_refusal

        # ssh_exec keeps inspection commands: the gate passes and the call
        # proceeds to fail on the missing password (no gate complaint).
        inspection = expect_error("df -h")
        assert "password" in inspection, inspection

        # Unrecognized mutating commands are refused by the whitelist before
        # any dialing, destructive patterns are refused outright, and
        # confirmDestructive cannot override a read-only server.
        whitelist = expect_error("systemctl restart nginx")
        assert "not recognized" in whitelist, whitelist
        destructive = expect_error("rm -rf /etc")
        assert "Refused on read-only" in destructive, destructive
        override = expect_error("rm -rf /etc", arguments={"confirmDestructive": True})
        assert "Refused on read-only" in override, override
        print("read-only server gate ok (DBX_SSH_MCP_READ_ONLY=1)")
    finally:
        if proc.stdin:
            proc.stdin.close()
        proc.wait(timeout=10)


def bridge_unreachable_section(args: argparse.Namespace) -> None:
    """Third sidecar pointed at an empty app-data dir: stdio
    `ssh_exec{runInTerminal:true}` must go down the app-bridge ensure path
    (launch attempt is the no-op `:` command, so no UI pops up) and come back
    with the actionable "DBX app bridge" error — no SSH server, no network
    beyond the local port-file poll. The ensure poll runs the full app-start
    budget (30s) before the error, which is exactly the behavior under test.
    """
    app_data = tempfile.mkdtemp(prefix="smoke-mcp-appdata-")
    env = dict(os.environ, DBX_APP_DATA_DIR=app_data, DBX_APP_LAUNCH_CMD=":")
    proc = subprocess.Popen(
        [args.binary, "--mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env,
    )
    try:
        send(proc, {
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {}},
        })
        recv(proc, 1)
        send(proc, {
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "ssh_exec", "arguments": {
                "connectionId": "no-such-connection", "command": "true",
                "runInTerminal": True,
            }},
        })
        error = recv(proc, 2)["error"]["message"]
        assert "DBX app bridge" in error, f"unexpected error: {error}"
        print("app-bridge unreachable error ok (DBX app bridge …)")
    finally:
        if proc.stdin:
            proc.stdin.close()
        proc.wait(timeout=10)


if __name__ == "__main__":
    try:
        main()
    except AssertionError as error:
        print(f"FAIL: {error}", file=sys.stderr)
        sys.exit(1)
