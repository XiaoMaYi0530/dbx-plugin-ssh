#!/usr/bin/env python3
"""End-to-end sidecar smoke test against a local SSH container.

Drives the installed sidecar binary over its stdio-framed protocol:
initialize -> connection/connect -> (auto-accept host-key challenge) ->
ssh/session/open -> run a command -> read terminal output -> sftp list.

Usage:
    python3 scripts/smoke_test.py                    # default test container
    python3 scripts/smoke_test.py --host H --port P --user U --password W
"""

from __future__ import annotations

import argparse
import base64
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from sidecar_client import SidecarClient, SidecarError, lifecycle_params


def step(name: str):
    print(f"\n==> {name}")


def fail(message: str, client: SidecarClient | None = None):
    if client:
        client.close()
    print(f"\nFAIL: {message}", file=sys.stderr)
    sys.exit(1)


def auto_accept_challenge(event: dict) -> dict | None:
    """Auto-accept host-key challenges while a request is in flight."""
    if event.get("method") != "connection/challenge":
        return None
    params = event.get("params", {}).get("params") or event.get("params", {})
    if "challengeId" not in params:
        return None
    print(f"    host-key challenge: {params.get('keyType')} {str(params.get('fingerprint'))[:32]}...")
    return {
        "method": "ssh/host-key/resolve",
        "params": {
            "challengeId": params["challengeId"],
            "operationId": params.get("operationId"),
            "accept": True,
            "remember": True,
        },
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=2222)
    parser.add_argument("--user", default="sshuser")
    parser.add_argument("--password", default="DbxTest2026")
    args = parser.parse_args()

    started = time.monotonic()
    client = SidecarClient.start(timeout=30)
    try:
        step("plugin/initialize")
        info = client.initialize()
        print(json.dumps(info, ensure_ascii=False)[:200])

        connection_id = "smoke-test-connection"
        workbench_id = "smoke-workbench"
        connection = {
            "id": connection_id,
            "name": "smoke",
            "db_type": "ssh",
            "host": args.host,
            "port": args.port,
            "username": args.user,
            "password": args.password,
            "external_config": {"authentication": "password"},
        }

        step("connection/connect")
        result = client.request("connection/connect", lifecycle_params(connection))
        print(json.dumps(result, ensure_ascii=False))

        step("connection/test (challenge auto-accepted, remembered)")
        test_started = time.monotonic()
        result = client.request("connection/test", lifecycle_params(connection), timeout=90,
                                on_event=auto_accept_challenge)
        print(f"    test ok in {time.monotonic() - test_started:.1f}s: "
              f"{json.dumps(result, ensure_ascii=False)[:100]}")

        step("ssh/session/open")
        open_started = time.monotonic()
        session = client.request("ssh/session/open",
                                 {"connectionId": connection_id, "workbenchId": workbench_id, "cols": 120, "rows": 30},
                                 timeout=60, on_event=auto_accept_challenge)
        session_id = session.get("sessionId", workbench_id)
        print(f"    session {session_id} opened in {time.monotonic() - open_started:.1f}s")

        step("wait for shell prompt, then run 'echo SMOKE_OK'")
        # terminal input frames carry a u64 BE sequence prefix; output frames
        # are [sequence u64][payload] on ssh/terminal/out/<sessionId>
        import struct as _struct
        deadline = time.monotonic() + 20
        prompt_seen = False
        while time.monotonic() < deadline and not prompt_seen:
            for frame in list(client.binary_frames):
                channel, data = frame
                client.binary_frames.remove(frame)
                if channel.startswith("ssh/terminal/out/") and (b"$ " in data or b"# " in data):
                    prompt_seen = True
                    break
            if not prompt_seen:
                client.timeout = max(0.5, deadline - time.monotonic())
                try:
                    client._pump(None)
                except SidecarError:
                    break
        if not prompt_seen:
            fail("shell prompt did not appear within 20s")
        print("    shell prompt received")
        client.send_binary(f"ssh/terminal/in/{session_id}", _struct.pack(">Q", 1) + b"echo SMOKE_OK_MARKER\r")
        deadline = time.monotonic() + 15
        saw_marker = False
        while time.monotonic() < deadline and not saw_marker:
            for frame in list(client.binary_frames):
                channel, data = frame
                client.binary_frames.remove(frame)
                if channel.startswith("ssh/terminal/out/") and b"SMOKE_OK_MARKER" in data:
                    saw_marker = True
                    break
            if not saw_marker:
                client.timeout = max(0.5, deadline - time.monotonic())
                try:
                    client._pump(None)
                except SidecarError:
                    break
        if not saw_marker:
            fail("terminal did not echo SMOKE_OK_MARKER within 15s")
        print("    terminal output received")

        step("sftp/home + sftp/list")
        home = client.request("sftp/home", {"sessionId": session_id})
        print(f"    home: {json.dumps(home, ensure_ascii=False)}")
        path = home.get("path", "/")
        listing = client.request("sftp/list", {"sessionId": session_id, "path": path})
        entries = listing.get("entries", [])
        if not entries:
            fail(f"sftp/list returned no entries for {path}")
        # Wire contract: entries carry `kind` (file/directory/symlink/other),
        # not `fileType`/`type` (see backend/src/model.rs SftpEntry).
        valid_kinds = {"file", "directory", "symlink", "other"}
        for entry in entries:
            if not entry.get("name"):
                fail(f"sftp/list entry without name: {json.dumps(entry)[:120]}")
            kind = entry.get("kind")
            if kind not in valid_kinds:
                fail(f"sftp/list entry {entry.get('name')!r} has bad kind {kind!r}")
            if not entry.get("uri", "").startswith("sftp:/"):
                fail(f"sftp/list entry {entry.get('name')!r} has bad uri {entry.get('uri')!r}")
        if not any(entry.get("kind") == "directory" for entry in entries):
            fail(f"sftp/list found no directory entry in {path}")
        print(f"    listed {len(entries)} entries in {path} "
              f"(kinds: {sorted({e['kind'] for e in entries})})")
        for entry in entries[:5]:
            print(f"      - {entry.get('name')} ({entry.get('kind')})")

        step("sftp upload + read round-trip")
        payload = b"dbx smoke test\n"
        write_path = f"{path.rstrip('/')}/.dbx_smoke_test"
        upload = client.request("sftp/upload/start",
                                {"sessionId": session_id, "remotePath": write_path, "size": len(payload)})
        task_id = upload.get("taskId")
        client.send_binary(f"sftp/upload/{task_id}", _struct.pack(">Q", 0) + payload)
        # append_upload runs on the sidecar worker pool; give it a beat to
        # land before finishing (atomic rename) the transfer
        time.sleep(1)
        client.request("sftp/upload/finish", {"taskId": task_id})
        read_back = client.request("sftp/read", {"sessionId": session_id, "path": write_path, "maxBytes": 4096})
        read_content = base64.b64decode(read_back.get("dataBase64", "")).decode(errors="replace")
        if "dbx smoke test" not in read_content:
            fail(f"sftp round-trip mismatch: {read_content!r}")
        client.request("sftp/delete", {"sessionId": session_id, "path": write_path})
        print(f"    wrote and read back {write_path}")

        step("session close")
        client.request("ssh/session/close", {"sessionId": session_id})
        print("    closed")

        client.close()
        print(f"\nPASS: full sidecar chain OK in {time.monotonic() - started:.1f}s")
    except SidecarError as error:
        fail(str(error), client)
    except KeyboardInterrupt:
        client.close()


if __name__ == "__main__":
    main()
