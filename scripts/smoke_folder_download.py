#!/usr/bin/env python3
"""End-to-end smoke test for recursive folder downloads (issue #46,
`sftp/download/tree/start` + the shared chunk pipeline).

Flow: initialize -> connection/connect -> ssh/session/open, then build a
remote tree inside the test container (regular files across nested dirs, an
empty file, an empty directory, a symlink that must be skipped, and a
chmod-000 file whose read must fail), download the whole tree through
`sftp/download/tree/start` + `sftp/download/next`/`finish`, and compare the
local mirror against the plan: structure, file set and byte contents.

Also covers: empty-tree download, cancel semantics (the half-done local root
is removed), and the non-directory rejection.

Unregistered methods are reported as SKIP so the script passes both before
and after wiring (only connection setup failures or real method errors count
as FAIL).

Usage:
    python3 scripts/smoke_folder_download.py                    # 127.0.0.1:2222
    python3 scripts/smoke_folder_download.py --port 2246        # issue #46 container
    python3 scripts/smoke_folder_download.py --host H --port P --user U --password W
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import re
import shutil
import sys
import tempfile
import time
import uuid
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from sidecar_client import SidecarClient, SidecarError, lifecycle_params

REMOTE_BYTES = bytes((i * 7 + 3) % 256 for i in range(300 * 1024))  # spans 2 chunks


def step(name: str) -> None:
    print(f"\n==> {name}")


def q(value: str) -> str:
    """Single-quote shell escaping for remote scratch commands."""
    return "'" + value.replace("'", "'\\''") + "'"


def missing_method(error: Exception) -> str | None:
    text = str(error)
    if "Method not found" not in text and "-32601" not in text:
        return None
    match = re.search(r"Method not found:\s*([\w./-]+)", text)
    return match.group(1) if match else ""


def fail(message: str, client: SidecarClient | None = None) -> None:
    if client:
        client.close()
    print(f"\nFAIL: {message}", file=sys.stderr)
    sys.exit(1)


def auto_accept_challenge(event: dict) -> dict | None:
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


def run_case(title: str, method: str, case, report: dict) -> None:
    step(title)
    try:
        case()
    except SidecarError as error:
        missing = missing_method(error)
        if missing is not None:
            print(f"SKIP: {missing or method} not registered yet")
            report["skipped"].append(title)
        else:
            print(f"FAIL: {error}")
            report["failed"].append((title, str(error)))
    except AssertionError as error:
        print(f"FAIL: {error}")
        report["failed"].append((title, str(error)))
    except Exception as error:  # noqa: BLE001 - smoke reporting
        print(f"FAIL: {error}")
        report["failed"].append((title, str(error)))
    else:
        print("    PASS")
        report["passed"].append(title)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=2222)
    parser.add_argument("--user", default="sshuser")
    parser.add_argument("--password", default="DbxTest2026")
    args = parser.parse_args()

    started = time.monotonic()
    # 本机落盘目录指向临时目录，避免污染开发者真实的 ~/Downloads；必须在
    # sidecar 启动前注入（sidecar 启动时读取一次环境变量）。
    download_dir = Path(tempfile.mkdtemp(prefix="dbx-ssh-smoke-folder-dl-"))
    os.environ["DBX_SSH_DOWNLOAD_DIR"] = str(download_dir)
    # 独立插件数据目录：known_hosts 与主机密钥挑战与开发者本机隔离（容器重建
    # 后 127.0.0.1 的旧指纹会让会话打开直接失败）。
    data_dir = tempfile.mkdtemp(prefix="dbx-ssh-smoke-folder-dl-data-")
    client: SidecarClient | None = None
    session_id: str | None = None
    base: str | None = None
    report: dict = {"passed": [], "skipped": [], "failed": []}
    try:
        client = SidecarClient.start(timeout=30, data_dir=data_dir)
        step("plugin/initialize")
        client.initialize()

        connection_id = "smoke-folder-connection"
        connection = {
            "id": connection_id,
            "name": "smoke-folder-dl",
            "db_type": "ssh",
            "host": args.host,
            "port": args.port,
            "username": args.user,
            "password": args.password,
            "external_config": {"authentication": "password"},
        }
        step("connection/connect")
        client.request("connection/connect", lifecycle_params(connection))
        step("ssh/session/open")
        session = client.request(
            "ssh/session/open",
            {"connectionId": connection_id, "workbenchId": "smoke-folder-dl", "cols": 120, "rows": 30},
            timeout=60,
            on_event=auto_accept_challenge,
        )
        session_id = session.get("sessionId", "smoke-folder-dl")
        print(f"    session {session_id} opened")

        def req(method: str, params: dict | None = None, timeout: float = 60.0) -> dict:
            return client.request(method, params, timeout=timeout, on_event=auto_accept_challenge)

        step("sftp/home")
        home = req("sftp/home", {"sessionId": session_id}).get("path") or "/config"
        base = f"{home}/.dbx-folder-smoke-{uuid.uuid4().hex[:8]}"
        print(f"    remote tree root: {base}")

        # ---- build the remote tree via plain SFTP ops ---------------------
        req("sftp/createDirectory", {"sessionId": session_id, "path": base})
        for rel in ("a", "a/b", "empty-dir"):
            req("sftp/createDirectory", {"sessionId": session_id, "path": f"{base}/{rel}"})
        req("sftp/write", {"sessionId": session_id, "remotePath": f"{base}/top.txt",
                           "dataBase64": base64.b64encode(b"top-level\n").decode()})
        req("sftp/write", {"sessionId": session_id, "remotePath": f"{base}/a/mid.bin",
                           "dataBase64": base64.b64encode(REMOTE_BYTES).decode()})
        req("sftp/touch", {"sessionId": session_id, "path": f"{base}/a/b/deep.txt"})
        req("sftp/write", {"sessionId": session_id, "remotePath": f"{base}/locked.txt",
                           "dataBase64": base64.b64encode(b"unreadable\n").decode()})
        req("ssh/exec", {"sessionId": session_id,
                         "command": f"chmod 000 {q(base + '/locked.txt')} && ln -sfn a {q(base + '/link-dir')}"})

        def tree_start(remote: str) -> dict:
            return req("sftp/download/tree/start", {"sessionId": session_id, "remotePath": remote}, timeout=90)

        def download_tree(info: dict) -> dict:
            """Drives the shared chunk loop until eof, then finishes the task."""
            offset = 0
            while True:
                result = req("sftp/download/next", {"taskId": info["taskId"], "offset": offset}, timeout=60)
                assert result.get("eof") or result.get("length", 0) > 0, f"empty non-eof chunk: {result}"
                offset += int(result.get("length", 0))
                if result.get("eof"):
                    break
            return req("sftp/download/finish", {"taskId": info["taskId"]}, timeout=60)

        state: dict = {}

        def case_full_tree_download():
            info = tree_start(base)
            print(f"    start: {json.dumps(info, ensure_ascii=False)[:200]}")
            assert info.get("fileCount") == 4, f"fileCount mismatch: {info}"      # top/mid/deep/locked
            assert info.get("dirCount") == 3, f"dirCount mismatch: {info}"        # a, a/b, empty-dir
            assert info.get("skippedCount") == 1, f"symlink not skipped: {info}"  # link-dir
            assert int(info.get("size", 0)) == len(b"top-level\n") + len(REMOTE_BYTES) + len(b"unreadable\n"), info
            finish = download_tree(info)
            print(f"    finish: {json.dumps(finish, ensure_ascii=False)[:240]}")
            assert finish.get("failedCount") == 1, f"locked.txt failure not summarized: {finish}"
            assert finish.get("fileCount") == 4, finish
            local_root = finish.get("localPath")
            assert local_root and Path(local_root).is_dir(), f"localPath missing: {finish}"
            state["local_root"] = local_root

            mirror = sorted(str(p.relative_to(local_root)) + ("/" if p.is_dir() else "")
                            for p in Path(local_root).rglob("*"))
            expected = ["a/", "a/b/", "a/b/deep.txt", "a/mid.bin", "empty-dir/", "top.txt"]
            assert mirror == expected, f"structure mismatch:\n got {mirror}\nwant {expected}"
            assert (Path(local_root) / "top.txt").read_bytes() == b"top-level\n"
            assert (Path(local_root) / "a" / "mid.bin").read_bytes() == REMOTE_BYTES
            assert (Path(local_root) / "a" / "b" / "deep.txt").read_bytes() == b""
            failed_paths = [row.get("path", "") for row in finish.get("failedFiles", [])]
            assert any(path.endswith("locked.txt") for path in failed_paths), f"failedFiles: {failed_paths}"
            print(f"    mirror ok: {len(mirror)} entries; bytes match; 1 file failed as planned")

        def case_empty_tree_download():
            info = tree_start(f"{base}/empty-dir")
            assert info.get("fileCount") == 0 and int(info.get("size", 0)) == 0, info
            finish = download_tree(info)
            local_root = finish.get("localPath")
            assert local_root and Path(local_root).is_dir(), f"empty tree not materialized: {finish}"
            assert not any(Path(local_root).iterdir()), "empty tree gained entries"
            print(f"    empty tree preserved at {local_root}")

        def case_cancel_removes_local_root():
            before = {p.name for p in download_dir.iterdir() if p.is_dir()}
            info = tree_start(base)
            req("sftp/download/next", {"taskId": info["taskId"], "offset": 0}, timeout=60)
            req("sftp/transfer/cancel", {"taskId": info["taskId"]})
            after = {p.name for p in download_dir.iterdir() if p.is_dir()}
            assert after == before, f"cancelled tree left a local root behind: {after ^ before}"
            print("    cancelled root removed; download dir unchanged")

        def case_rejects_non_directory():
            try:
                tree_start(f"{base}/top.txt")
            except SidecarError as error:
                assert "directory" in str(error).lower(), f"unexpected error: {error}"
                print(f"    rejected as expected: {error}")
            else:
                raise AssertionError("tree/start accepted a regular file")

        run_case("folder download mirrors structure and bytes", "sftp/download/tree/start",
                 case_full_tree_download, report)
        run_case("empty tree downloads to an empty local folder", "sftp/download/tree/start",
                 case_empty_tree_download, report)
        run_case("cancel removes the half-done local root", "sftp/transfer/cancel",
                 case_cancel_removes_local_root, report)
        run_case("non-directory target is rejected", "sftp/download/tree/start",
                 case_rejects_non_directory, report)

        if not report["passed"] and not report["failed"]:
            print(f"\nSKIP: no folder-download case ran (all skipped)")
            return
        if report["failed"]:
            print(f"\nFAIL {len(report['failed'])} folder-download case(s):")
            for title, reason in report["failed"]:
                print(f"  - {title}: {reason}")
            sys.exit(1)
        print(f"\nPASS folder download smoke: {len(report['passed'])} case(s), "
              f"{len(report['skipped'])} skipped, in {time.monotonic() - started:.1f}s")
    finally:
        # 远端清理：恢复 locked.txt 权限再整树删除；失败不掩盖主流程结果。
        if client is not None and session_id and base:
            try:
                client.request(
                    "ssh/exec",
                    {"sessionId": session_id,
                     "command": f"chmod -R u+rwX {q(base)} 2>/dev/null; rm -rf {q(base)}"},
                    timeout=30, on_event=auto_accept_challenge)
                print(f"\ncleanup: remote tree removed ({base})")
            except Exception:
                print("\ncleanup: remote tree removal failed (left on container)")
        shutil.rmtree(download_dir, ignore_errors=True)
        shutil.rmtree(data_dir, ignore_errors=True)
        if client is not None:
            client.close()


if __name__ == "__main__":
    main()
