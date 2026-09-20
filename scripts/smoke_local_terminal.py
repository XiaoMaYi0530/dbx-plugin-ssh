#!/usr/bin/env python3
"""Local terminal smoke test: local/terminal/* against a real sidecar (no SSH).

Drives the sidecar's local terminal end to end on the machine running this
script — no SSH server or docker container involved:

  1. local/terminal/start spawns the platform's login shell with shell
     integration injected (integration response carries shellIntegration),
  2. the output channel carries the OSC 133;A prompt mark (injection live),
  3. keyboard input (8-byte BE sequence + bytes) round-trips: an echo probe
     command's output comes back with OSC 133;D;0 (command completed clean),
  4. resize + local/session/list answer,
  5. local/session/close terminates the child and the exit event arrives
     (local/session/state {state: "exited"}).

Skips (exit 0) when the sidecar binary is unavailable — same self-gating
semantics as the live-container smokes. Run directly:

    python3 scripts/smoke_local_terminal.py [--binary backend/target/release/dbx-plugin-ssh]
"""

import argparse
import os
import struct
import sys
import tempfile
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__))))
from sidecar_client import SidecarClient  # noqa: E402


def collect_until(client, out_channel, in_channel, ready, done, deadline_s=15.0):
    """Pump frames via harmless list requests; type the probe line once the
    first prompt mark arrives; stop when `done(text)` holds."""
    collected = bytearray()
    sent = False
    end = time.time() + deadline_s
    while time.time() < end:
        client.request("local/session/list", timeout=5)
        for channel, payload in client.binary_frames:
            if channel == out_channel:
                collected.extend(payload)
        client.binary_frames.clear()
        text = bytes(collected)
        if ready(text) and not sent:
            sent = True
            client.send_binary(in_channel, struct.pack(">Q", 1) + b"echo DBX_SMOKE_$((6*7))_OK\r")
        if done(text):
            return text
        time.sleep(0.1)
    return bytes(collected)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="backend/target/release/dbx-plugin-ssh")
    args = parser.parse_args()
    if not os.path.isfile(args.binary):
        print(f"SKIP: sidecar binary not found: {args.binary}")
        return 0

    with tempfile.TemporaryDirectory(prefix="dbx-local-smoke-") as data_dir:
        client = SidecarClient.start(binary=args.binary, data_dir=data_dir)
        try:
            client.initialize()

            # shell 发现：本机至少能列出一个可启动 shell（多平台设置的数据源）。
            inventory = client.request("local/shells/list")
            shells = inventory["shells"]
            assert shells, f"no shells discovered on {inventory.get('platform')}"
            print("shells:", [f"{s['name']}({s['program']})" for s in shells])

            # 偏好往返：shell 选择 + 注入开关（workbench 设置面板的存储层）。
            client.request("local/preferences/set", {"localShell": shells[0]["program"], "localShellIntegration": False})
            prefs = client.request("local/preferences/get")
            assert prefs.get("localShell") == shells[0]["program"], prefs
            assert prefs.get("localShellIntegration") is False, prefs
            client.request("local/preferences/set", {"localShellIntegration": True})
            assert client.request("local/preferences/get").get("localShellIntegration") is True

            # 显式 shell 启动：返回的 shell 必须回显请求值（cwd 非法值回落家目录）。
            started = client.request(
                "local/terminal/start",
                {
                    "workbenchId": "smoke-wb",
                    "cols": 100,
                    "rows": 30,
                    "shell": shells[0]["program"],
                    "cwd": "/nonexistent-dir-should-fall-back",
                },
            )
            assert started["shell"] == shells[0]["program"], started
            session_id = started["sessionId"]
            print("started:", started)
            # 可注入的 shell（zsh/bash/fish/pwsh）才会置 true；cmd/unknown 为
            # false 且没有 OSC 标记，用例按 shellIntegration 分支。
            integrated = started.get("shellIntegration") is True

            out = f"local/terminal/out/{session_id}"
            text = collect_until(
                client,
                out,
                f"local/terminal/in/{session_id}",
                ready=lambda chunk: b"\x1b]133;A" in chunk,
                done=lambda chunk: b"\x1b]133;A" in chunk and b"DBX_SMOKE_42_OK" in chunk,
            )
            if integrated:
                assert b"\x1b]133;A" in text, f"no OSC 133;A prompt mark; tail={text[-160:]!r}"
                assert b"\x1b]133;D;0" in text, f"no OSC 133;D;0 exit-code mark; tail={text[-160:]!r}"
            assert b"DBX_SMOKE_42_OK" in text, f"echo missing; tail={text[-160:]!r}"
            print(f"echo round-trip OK ({len(text)} bytes, integration={integrated})")

            client.request(
                "local/terminal/resize", {"sessionId": session_id, "cols": 120, "rows": 40}
            )
            listing = client.request("local/session/list")
            assert any(s["sessionId"] == session_id for s in listing["sessions"]), "not listed"
            print("resize + list ok")

            client.request("local/session/close", {"sessionId": session_id})
            event = client.wait_event("local/session/state", timeout=10)
            assert event and event["params"]["state"] == "exited", f"no exit event: {event}"
            print("exit event ok:", event["params"])

            # cwd 继承路径的参数面：合法目录被接受（响应无直接回显，用 list+事件
            # 之外的方式难以观察；这里只断言非法值不致命——上面 start 已带非法
            # cwd 且会话成功创建）。
            print("LOCAL TERMINAL SMOKE PASS")
            return 0
        finally:
            client.close()


if __name__ == "__main__":
    raise SystemExit(main())
