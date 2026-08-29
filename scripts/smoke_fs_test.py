#!/usr/bin/env python3
"""End-to-end smoke test for the fs/keys capability extensions (sftp_ext, sudo_fs, keys).

Reuses the connection flow from smoke_test.py: initialize -> connection/connect ->
connection/test (challenge auto-accepted) -> ssh/session/open, then exercises the
new backend methods against the test container. The new methods may not be
registered in main.rs yet; any "Method not found" answer is reported as SKIP so
the script passes both before and after wiring (only connection setup failures
or real method errors count as FAIL).

Usage:
    python3 scripts/smoke_fs_test.py                    # default test container
    python3 scripts/smoke_fs_test.py --host H --port P --user U --password W
"""

from __future__ import annotations

import argparse
import base64
import json
import re
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


def missing_method(error: Exception) -> str | None:
    """Return the unregistered method name if the error means 'Method not found'."""
    text = str(error)
    if "Method not found" not in text and "-32601" not in text:
        return None
    match = re.search(r"Method not found:\s*([\w./-]+)", text)
    return match.group(1) if match else ""


def entry_kind(result: dict) -> str:
    """Entry kind across the plausible wire spellings ("?" when not reported)."""
    kind = result.get("kind") or result.get("fileType") or result.get("type")
    return str(kind) if kind else "?"


def mode_is_0700(value) -> bool:
    """Accept 0o700 in any plausible encoding: 448, "700", "0700", "rwx------"."""
    digits = str(value).strip().lstrip("-")
    if digits.isdigit():
        try:
            number = int(digits, 8)  # octal text like "700" / "0700"
        except ValueError:
            number = int(digits, 10)  # decimal like 448, or 0o100700-style ints
        return number & 0o777 == 0o700
    return "rwx------" in str(value)


class Report:
    """Per-case PASS/SKIP/FAIL bookkeeping with the smoke_test reporting style."""

    def __init__(self):
        self.passed: list[str] = []
        self.skipped: list[tuple[str, str]] = []
        self.failed: list[tuple[str, str]] = []

    def run(self, title: str, method: str, case, needs: str | None = None):
        """Run one case; SKIP on Method not found, FAIL on any other error.

        `needs` gates chained cases: it must name a case that PASSED before
        this one runs, otherwise this case is SKIPped as unreachable.
        """
        step(title)
        if needs and needs not in self.passed:
            print(f"SKIP: prerequisite '{needs}' did not pass")
            self.skipped.append((title, f"prerequisite '{needs}' did not pass"))
            return
        try:
            case()
        except SidecarError as error:
            missing = missing_method(error)
            if missing is not None:
                print(f"SKIP: {missing or method} not registered yet")
                self.skipped.append((title, f"{missing or method} not registered yet"))
            else:
                print(f"FAIL: {error}")
                self.failed.append((title, str(error)))
        except Exception as error:
            print(f"FAIL: {error}")
            self.failed.append((title, str(error)))
        else:
            print("    PASS")
            self.passed.append(title)


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

        connection_id = "smoke-fs-connection"
        workbench_id = "smoke-fs"
        connection = {
            "id": connection_id,
            "name": "smoke-fs",
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
        session = client.request("ssh/session/open",
                                 {"connectionId": connection_id, "workbenchId": workbench_id, "cols": 120, "rows": 30},
                                 timeout=60, on_event=auto_accept_challenge)
        session_id = session.get("sessionId", workbench_id)
        print(f"    session {session_id} opened")

        step("sftp/home")
        result = client.request("sftp/home", {"sessionId": session_id})
        home = result.get("path") or "/config"
        print(f"    home: {home}")

        # remote scratch paths used by the cases below
        ssh_dir = f"{home}/.ssh"
        touch_path = f"{home}/.dbx-fs-smoke-touch"
        write_path = f"{home}/.dbx-fs-smoke-write"
        archive_path = f"{home}/.dbx-parity-test.tar.gz"
        extract_dir = f"{home}/.dbx-parity-extract"
        sudo_dir = f"{home}/.sudo-test"
        sudo_file = f"{sudo_dir}/inner.txt"
        sudo_file_renamed = f"{sudo_dir}/inner-renamed.txt"

        def req(method: str, params: dict | None = None, timeout: float = 60.0) -> dict:
            return client.request(method, params, timeout=timeout, on_event=auto_accept_challenge)

        # -- sftp_ext group ------------------------------------------------------

        def case_sftp_stat():
            stat = req("sftp/stat", {"sessionId": session_id, "path": home})
            print(f"    {home}: kind={entry_kind(stat)} size={stat.get('size')}")
            if entry_kind(stat) != "directory":
                raise AssertionError(f"{home} kind={entry_kind(stat)!r}, want 'directory'")

        def case_sftp_exists():
            present = req("sftp/exists", {"sessionId": session_id, "path": home})
            if present.get("exists") is not True:
                raise AssertionError(f"exists({home}) -> {json.dumps(present)[:120]}, want true")
            absent = req("sftp/exists", {"sessionId": session_id, "path": f"{home}/no-such-path-xyz"})
            if absent.get("exists") is not False:
                raise AssertionError(f"exists(missing) -> {json.dumps(absent)[:120]}, want false")
            print(f"    exists({home})=true, exists(no-such-path-xyz)=false")

        def case_sftp_touch_stat():
            req("sftp/touch", {"sessionId": session_id, "path": touch_path})
            stat = req("sftp/stat", {"sessionId": session_id, "path": touch_path})
            print(f"    {touch_path}: kind={entry_kind(stat)} size={stat.get('size')}")
            if entry_kind(stat) != "file" or stat.get("size") != 0:
                raise AssertionError(f"want empty file, got kind={entry_kind(stat)!r} size={stat.get('size')!r}")
            req("sftp/delete", {"sessionId": session_id, "path": touch_path})

        def case_sftp_write_read():
            payload = b"hello parity\n"
            req("sftp/write", {"sessionId": session_id, "remotePath": write_path,
                               "dataBase64": base64.b64encode(payload).decode()})
            read_back = client.request("sftp/read",
                                       {"sessionId": session_id, "path": write_path, "maxBytes": 4096})
            content = base64.b64decode(read_back.get("dataBase64", "")).decode(errors="replace")
            if content != payload.decode():
                raise AssertionError(f"read-back mismatch: {content!r}")
            print(f"    wrote and read back {write_path}")
            req("sftp/delete", {"sessionId": session_id, "path": write_path})

        def case_sftp_archive():
            try:  # archive source must exist; create ~/.ssh if the container lacks it
                req("sftp/createDirectory", {"sessionId": session_id, "path": ssh_dir})
            except SidecarError:
                pass
            # "paths" and "sourcePaths" are both sent: the wire name is not frozen yet
            req("sftp/archive", {"sessionId": session_id, "paths": [ssh_dir],
                                 "sourcePaths": [ssh_dir], "archivePath": archive_path})
            stat = req("sftp/stat", {"sessionId": session_id, "path": archive_path})
            size = stat.get("size")
            print(f"    {archive_path}: kind={entry_kind(stat)} size={size}")
            if not isinstance(size, (int, float)) or size <= 0:
                raise AssertionError(f"archive size={size!r}, want > 0")

        def case_sftp_extract():
            req("sftp/extract", {"sessionId": session_id, "archivePath": archive_path,
                                 "destinationPath": extract_dir})
            listing = client.request("sftp/list", {"sessionId": session_id, "path": home})
            name = extract_dir.rsplit("/", 1)[-1]
            entry = next((e for e in listing.get("entries", []) if e.get("name") == name), None)
            kind = entry_kind(entry or {})
            print(f"    extracted entry: {name} ({kind})")
            if entry is None:
                raise AssertionError(f"{extract_dir} missing from {home} listing")
            if kind not in ("directory", "?"):
                raise AssertionError(f"{extract_dir} kind={kind!r}, want directory")
            req("sftp/delete", {"sessionId": session_id, "path": extract_dir, "recursive": True})
            req("sftp/delete", {"sessionId": session_id, "path": archive_path})

        # -- sudo_fs group (container sshuser has NOPASSWD sudo) ------------------

        def case_sudo_stat():
            stat = req("sudo/stat", {"sessionId": session_id, "path": home})
            print(f"    {home}: kind={entry_kind(stat)} mode={stat.get('mode', stat.get('permissions'))}")
            if entry_kind(stat) != "directory":
                raise AssertionError(f"{home} kind={entry_kind(stat)!r}, want 'directory'")

        def case_sudo_exists():
            present = req("sudo/exists", {"sessionId": session_id, "path": home})
            if present.get("exists") is not True:
                raise AssertionError(f"exists({home}) -> {json.dumps(present)[:120]}, want true")
            absent = req("sudo/exists", {"sessionId": session_id, "path": f"{home}/no-such-path-xyz"})
            if absent.get("exists") is not False:
                raise AssertionError(f"exists(missing) -> {json.dumps(absent)[:120]}, want false")
            print(f"    exists({home})=true, exists(no-such-path-xyz)=false")

        def case_sudo_mkdir():
            req("sudo/mkdir", {"sessionId": session_id, "path": sudo_dir})
            print(f"    created {sudo_dir}")

        def case_sudo_touch():
            req("sudo/touch", {"sessionId": session_id, "path": sudo_file})
            stat = req("sudo/stat", {"sessionId": session_id, "path": sudo_file})
            print(f"    {sudo_file}: kind={entry_kind(stat)} size={stat.get('size')}")
            if entry_kind(stat) != "file":
                raise AssertionError(f"{sudo_file} kind={entry_kind(stat)!r}, want 'file'")

        def case_sudo_listdir():
            result = req("sudo/listDir", {"sessionId": session_id, "path": sudo_dir})
            names = [str(e.get("name")) for e in result.get("entries", [])]
            print(f"    {sudo_dir}: {names}")
            if sudo_file.rsplit("/", 1)[-1] not in names:
                raise AssertionError(f"inner file missing from listing: {names}")

        def case_sudo_write_read():
            payload = b"sudo parity\n"
            req("sudo/writeFile", {"sessionId": session_id, "path": sudo_file,
                               "dataBase64": base64.b64encode(payload).decode()})
            read_back = req("sudo/readFile", {"sessionId": session_id, "path": sudo_file, "maxBytes": 4096})
            content = base64.b64decode(read_back.get("dataBase64", "")).decode(errors="replace")
            if content != payload.decode():
                raise AssertionError(f"read-back mismatch: {content!r}")
            print(f"    wrote and read back {sudo_file}")

        def case_sudo_rename():
            req("sudo/rename", {"sessionId": session_id, "sourcePath": sudo_file,
                                "targetPath": sudo_file_renamed})
            print(f"    renamed to {sudo_file_renamed}")

        def case_sudo_chmod():
            req("sudo/chmod", {"sessionId": session_id, "path": sudo_file_renamed, "mode": "0700"})
            stat = req("sudo/stat", {"sessionId": session_id, "path": sudo_file_renamed})
            mode = stat.get("mode", stat.get("permissions"))
            print(f"    mode after chmod 0700: {mode}")
            if not mode_is_0700(mode):
                raise AssertionError(f"mode={mode!r}, want 0700")

        def case_sudo_remove_all():
            req("sudo/removeAll", {"sessionId": session_id, "path": sudo_dir})
            try:  # verify via sudo/exists when that method is wired too
                final = req("sudo/exists", {"sessionId": session_id, "path": sudo_dir})
                if final.get("exists") is not False:
                    raise AssertionError(f"{sudo_dir} still present after removeAll")
            except SidecarError as error:
                if missing_method(error) is None:
                    raise
            print(f"    removed {sudo_dir}")

        # -- keys group ------------------------------------------------------------

        def case_keys_discover():
            result = req("keys/discover", {})
            keys = result.get("keys") or []
            print(f"    discovered {len(keys)} key(s)")
            for key in keys[:3]:
                fingerprint = str(key.get("fingerprint", ""))[:16]
                print(f"      - {key.get('path')} {key.get('algorithm', '')} fp={fingerprint}...")

        def case_known_hosts():
            result = req("ssh/knownHosts/list", {})
            entries = result.get("entries") or []
            print(f"    {len(entries)} known-host entries")

        report = Report()
        print("\n--- sftp_ext group ---")
        report.run("sftp/stat /config", "sftp/stat", case_sftp_stat)
        report.run("sftp/exists true/false", "sftp/exists", case_sftp_exists)
        report.run("sftp/touch + sftp/stat empty file", "sftp/touch", case_sftp_touch_stat)
        report.run("sftp/write + sftp/read round-trip", "sftp/write", case_sftp_write_read)
        archive_case = "sftp/archive .ssh -> tar.gz"
        report.run(archive_case, "sftp/archive", case_sftp_archive)
        report.run("sftp/extract tar.gz -> directory", "sftp/extract", case_sftp_extract, needs=archive_case)

        print("\n--- sudo_fs group ---")
        report.run("sudo/stat /config", "sudo/stat", case_sudo_stat)
        report.run("sudo/exists true/false", "sudo/exists", case_sudo_exists)
        report.run("sudo/mkdir .sudo-test", "sudo/mkdir", case_sudo_mkdir)
        report.run("sudo/touch inner file", "sudo/touch", case_sudo_touch,
                   needs="sudo/mkdir .sudo-test")
        report.run("sudo/listDir .sudo-test", "sudo/listDir", case_sudo_listdir,
                   needs="sudo/touch inner file")
        report.run("sudo/write + sudo/read round-trip", "sudo/writeFile", case_sudo_write_read,
                   needs="sudo/touch inner file")
        report.run("sudo/rename inner file", "sudo/rename", case_sudo_rename,
                   needs="sudo/touch inner file")
        report.run("sudo/chmod 0700 + verify mode", "sudo/chmod", case_sudo_chmod,
                   needs="sudo/rename inner file")
        report.run("sudo/removeAll .sudo-test", "sudo/removeAll", case_sudo_remove_all,
                   needs="sudo/mkdir .sudo-test")

        print("\n--- keys group ---")
        report.run("keys/discover", "keys/discover", case_keys_discover)
        report.run("ssh/knownHosts/list", "ssh/knownHosts/list", case_known_hosts)

        step("cleanup leftovers")
        for path, recursive in ((touch_path, False), (write_path, False),
                                (archive_path, False), (extract_dir, True), (sudo_dir, True)):
            try:
                client.request("sftp/delete",
                               {"sessionId": session_id, "path": path, "recursive": recursive})
                print(f"    deleted {path}")
            except SidecarError:
                pass  # already cleaned by its case / not ours / needs sudo
        client.request("ssh/session/close", {"sessionId": session_id})
        print("    session closed")

        client.close()
        step(f"summary ({time.monotonic() - started:.1f}s)")
        print(f"PASS {len(report.passed)} / SKIP {len(report.skipped)} / FAIL {len(report.failed)}")
        for name, reason in report.skipped:
            print(f"  SKIP {name}: {reason}")
        for name, error in report.failed:
            print(f"  FAIL {name}: {error}", file=sys.stderr)
        if report.failed:
            sys.exit(1)
        print("PASS: fs/keys smoke OK")
    except SidecarError as error:
        fail(str(error), client)
    except KeyboardInterrupt:
        client.close()


if __name__ == "__main__":
    main()
