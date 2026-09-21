#!/usr/bin/env python3
"""Terminal input burst regression harness (#33 #71 mac 输入丢失/断开).

Drives the sidecar over framed stdio exactly the way the DBX embedded bridge
does and injects keystroke bursts on ssh/terminal/in/<sid>:

  phase 0  sanity probe (echo marker round-trip)
  phase A  fast typing   — 60 frames, 30 ms apart        (~33 cps)
  phase B  long press    — one char, 25 ms apart, 5 s    (~40 cps, mac repeat)
  phase C  burst         — 500 frames back-to-back

Pins the sidecar-side contract of the rapid-input bugs: every injected frame
must be acked (ssh/terminal/inputAck count == sent count), the PTY echo must
stay alive after every phase, and no ssh/session/state|ssh/terminal/error
event may fire. This harness exonerated the sidecar for #33/#71 (737/737
frames, incl. a 500-frame burst in ~4 ms, all acked, echo intact, process
alive) — use it as the control group when retesting the host bridge side:

    python3 scripts/terminal_burst_test.py                # local test container
    python3 scripts/terminal_burst_test.py --host H --port P --user U --password W
    python3 scripts/terminal_burst_test.py --binary PATH  # sidecar under test
"""

from __future__ import annotations

import argparse
import json
import struct
import subprocess
import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from sidecar_client import SidecarError, lifecycle_params  # noqa: E402

DEFAULT_BINARY = str(Path(__file__).resolve().parent.parent
                     / "backend" / "target" / "release" / "dbx-plugin-ssh")


class BurstClient:
    """SidecarClient-style driver with a continuous background reader.

    The host never stops reading sidecar stdout; a harness that only pumps
    frames while a request is in flight would add artificial backpressure and
    hide (or fake) the very stall under test.
    """

    def __init__(self, binary: str, data_dir: str):
        env = dict(__import__("os").environ)
        env["DBX_PLUGIN_DATA_DIR"] = data_dir
        self.proc = subprocess.Popen(
            [binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, env=env,
        )
        self.read_fd = self.proc.stdout.fileno()
        import select
        self._select = select.select
        self.next_id = 1
        self.lock = threading.Lock()
        self.events: list[tuple[float, str, dict]] = []
        self.out_frames: list[tuple[float, str, bytes]] = []
        self.stdout_closed_at: float | None = None
        self._stderr_tail = b""
        threading.Thread(target=self._read_loop, daemon=True).start()

    def _read_exact(self, n: int) -> bytes:
        buf = b""
        while len(buf) < n:
            chunk = self._read_raw(n - len(buf))
            if chunk is None:
                raise EOFError("sidecar stdout closed")
            buf += chunk
        return buf

    def _read_raw(self, n: int) -> bytes | None:
        import os
        try:
            return os.read(self.read_fd, n)
        except OSError:
            return None

    def _read_loop(self) -> None:
        while True:
            try:
                ready, _, _ = self._select([self.read_fd], [], [], 1.0)
                if not ready:
                    continue
                header = self._read_exact(5)
                kind, length = header[0], struct.unpack(">I", header[1:5])[0]
                payload = self._read_exact(length) if length else b""
            except (EOFError, OSError):
                self.stdout_closed_at = time.monotonic()
                return
            now = time.monotonic()
            if kind == 1:
                (clen,) = struct.unpack(">H", payload[:2])
                channel = payload[2:2 + clen].decode(errors="replace")
                with self.lock:
                    self.out_frames.append((now, channel, payload[2 + clen:]))
            else:
                try:
                    message = json.loads(payload)
                except Exception:
                    continue
                if "id" in message and "method" not in message:
                    with self.lock:
                        self.events.append((now, "__response__", message))
                else:
                    with self.lock:
                        self.events.append((now, message.get("method", "?"),
                                            message.get("params", {})))

    def _send_raw(self, kind: int, payload: bytes) -> None:
        self.proc.stdin.write(struct.pack(">BI", kind, len(payload)))
        self.proc.stdin.write(payload)
        self.proc.stdin.flush()

    def send_json(self, method: str, params: dict) -> int:
        with self.lock:
            request_id = self.next_id
            self.next_id += 1
        self._send_raw(0, json.dumps(
            {"jsonrpc": "2.0", "id": request_id, "method": method,
             "params": params}).encode())
        return request_id

    def request(self, method: str, params: dict, timeout: float = 60.0,
                on_event=None) -> dict:
        request_id = self.send_json(method, params)
        deadline = time.monotonic() + timeout
        seen = 0
        while time.monotonic() < deadline:
            with self.lock:
                events = self.events[seen:]
                seen = len(self.events)
            for (_, name, body) in events:
                if name == "__response__" and body.get("id") == request_id:
                    if body.get("error"):
                        raise SidecarError(str(body["error"])[:300])
                    return body.get("result") or {}
                if on_event and name != "__response__":
                    reply = on_event(name, body)
                    if reply:
                        self.send_json(reply["method"], reply.get("params", {}))
            time.sleep(0.02)
        raise TimeoutError(f"no response for {method} in {timeout}s")

    def send_input(self, session_id: str, sequence: int, data: bytes) -> None:
        channel = f"ssh/terminal/in/{session_id}".encode()
        payload = (struct.pack(">H", len(channel)) + channel
                   + struct.pack(">Q", sequence) + data)
        self._send_raw(1, payload)

    def snapshot(self):
        with self.lock:
            return list(self.events), list(self.out_frames), self.stdout_closed_at

    def terminal_text(self, out_channel: str, seconds: float, marker: str) -> str:
        deadline = time.monotonic() + seconds
        text = ""
        consumed = 0
        while time.monotonic() < deadline and marker not in text:
            _, frames, _ = self.snapshot()
            for (_, channel, payload) in frames[consumed:]:
                if channel == out_channel and len(payload) > 9:
                    # TerminalFrame::encode: [u8 stream][u64 BE sequence][data]
                    text += payload[9:].decode(errors="replace")
            consumed = len(frames)
            time.sleep(0.05)
        return text


def auto_accept(event_method: str, params: dict) -> dict | None:
    if event_method != "connection/challenge":
        return None
    inner = params.get("params") or params
    if "challengeId" not in inner:
        return None
    return {
        "method": "ssh/host-key/resolve",
        "params": {"challengeId": inner["challengeId"],
                    "operationId": inner.get("operationId"),
                    "accept": True, "remember": True},
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=2222)
    parser.add_argument("--user", default="sshuser")
    parser.add_argument("--password", default="DbxTest2026")
    parser.add_argument("--binary", default=DEFAULT_BINARY)
    parser.add_argument("--data-dir", default="/tmp/dbx-burst-data")
    args = parser.parse_args()

    client = BurstClient(args.binary, args.data_dir)
    failures: list[str] = []
    sequence = 0

    def probe(tag: str) -> bool:
        nonlocal sequence
        marker = f"ALIVE_{tag}_{int(time.time() * 1000) % 100000}"
        sequence += 1
        client.send_input(session_id, sequence, f"echo {marker}\r".encode())
        text = client.terminal_text(out_channel, 12, marker)
        events, _, closed = client.snapshot()
        bad = [(n, p) for _, n, p in events
               if n in ("ssh/session/state", "ssh/terminal/error")]
        dead = any(p.get("state") == "disconnected" or p.get("error")
                   for _, n, p in bad if n != "__response__")
        ok = marker in text and closed is None and not dead
        print(f"  probe[{tag}]: echoed={marker in text} stdout_closed={closed is not None} "
              f"state/error={bad[-3:] if bad else 'none'} -> {'OK' if ok else 'FAIL'}")
        return ok

    try:
        info = client.request("plugin/initialize",
                              {"host": {"protocolVersions": [1]}}, timeout=30)
        print(f"initialize ok: {json.dumps(info, ensure_ascii=False)[:120]}")
        connection = {
            "id": "burst-conn", "name": "burst", "db_type": "ssh",
            "host": args.host, "port": args.port,
            "username": args.user, "password": args.password,
            "external_config": {"authentication": "password"},
        }
        client.request("connection/connect", lifecycle_params(connection),
                       timeout=60, on_event=auto_accept)
        session = client.request("ssh/session/open",
                                 {"connectionId": "burst-conn",
                                  "workbenchId": "wb-burst", "cols": 120, "rows": 30},
                                 timeout=60, on_event=auto_accept)
        session_id = session["sessionId"]
        out_channel = f"ssh/terminal/out/{session_id}"
        print(f"session {session_id} open")

        time.sleep(1.5)
        client.terminal_text(out_channel, 8, "$")  # drain prompt/banner
        if not probe("0"):
            failures.append("phase 0 sanity")

        started = time.monotonic()
        for i in range(60):
            sequence += 1
            client.send_input(session_id, sequence, b"ab"[i % 2:i % 2 + 1])
            time.sleep(0.03)
        print(f"phase A fast-typing: 60 frames in {time.monotonic() - started:.2f}s")
        if not probe("A"):
            failures.append("phase A fast typing")

        started, count = time.monotonic(), 0
        while time.monotonic() - started < 5.0:
            sequence += 1
            client.send_input(session_id, sequence, b"x")
            count += 1
            time.sleep(0.025)
        print(f"phase B long-press: {count} frames in {time.monotonic() - started:.2f}s")
        if not probe("B"):
            failures.append("phase B long press")

        started = time.monotonic()
        for _ in range(500):
            sequence += 1
            client.send_input(session_id, sequence, b"y")
        print(f"phase C burst: 500 frames in {time.monotonic() - started:.3f}s")
        if not probe("C"):
            failures.append("phase C burst")

        time.sleep(1)
        events, frames, closed = client.snapshot()
        acks = len([e for e in events if e[1] == "ssh/terminal/inputAck"])
        outs = len([f for f in frames if f[1] == out_channel])
        alive = client.proc.poll() is None
        print(f"STATS sent={sequence} inputAcks={acks} outFrames={outs} "
              f"process_alive={alive} stdout_closed={closed is not None}")
        if acks != sequence:
            failures.append(f"inputAck loss: {acks}/{sequence}")
        if not alive:
            failures.append("sidecar process exited")
        if closed is not None:
            failures.append("sidecar stdout closed")
    finally:
        try:
            client.proc.stdin.close()
            client.proc.wait(timeout=5)
        except Exception:
            client.proc.kill()

    if failures:
        print(f"FAIL: {failures}")
        sys.exit(1)
    print("PASS: sidecar survives rapid terminal input without loss or disconnect")


if __name__ == "__main__":
    main()
