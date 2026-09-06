// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";

// mockDbxHost 是可视化夹具（mock.html 的宿主模拟器）。这里的用例锁定两个
// 曾经缺失的行为：默认 reattach 启动路径的终端回放内容（P2-2）与
// ?err=disconnect 在 attach 路径的可达性/单发语义（P2-1）。
const MOCK_URL = "http://localhost:5291/mock.html";

interface RecordedEvent {
  method: string;
  params: Record<string, unknown>;
}

async function loadMock(search: string) {
  vi.resetModules();
  window.location.href = `${MOCK_URL}${search}`;
  await import("./mockDbxHost");
  return window.dbxPlugin;
}

function frameText(data: Uint8Array) {
  return new TextDecoder().decode(data.slice(9));
}

function stateEvents(events: RecordedEvent[]) {
  return events.filter((event) => event.method === "ssh/session/state");
}

afterEach(() => {
  vi.useRealTimers();
  vi.resetModules();
});

describe("mockDbxHost fixture", () => {
  it("replays Welcome + OSC 633 transcript on the default reattach startup path (P2-2)", async () => {
    vi.useFakeTimers();
    const plugin = await loadMock("");
    const frames: Uint8Array[] = [];
    plugin.onBinary((event) => {
      if (event.channel.startsWith("ssh/terminal/out/") && event.data) frames.push(event.data);
    });
    const events: RecordedEvent[] = [];
    plugin.onEvent((event) => events.push(event as unknown as RecordedEvent));

    await plugin.invoke("ssh/session/attach", { connectionId: "visual-connection", workbenchId: "visual-workbench", afterSequence: 0 });
    await vi.advanceTimersByTimeAsync(50);

    const text = frames.map(frameText).join("");
    expect(text).toContain("Welcome to DBX SSH/SFTP visual fixture");
    expect(text).toContain("\u001b]633;A\u0007");
    expect(text.endsWith("user@server:~$ ")).toBe(true);
    // 默认参数不断开。
    await vi.advanceTimersByTimeAsync(5000);
    expect(stateEvents(events)).toEqual([]);
  });

  it("fires ?err=disconnect on the attach startup path exactly once (P2-1)", async () => {
    vi.useFakeTimers();
    const plugin = await loadMock("?err=disconnect");
    const events: RecordedEvent[] = [];
    plugin.onEvent((event) => events.push(event as unknown as RecordedEvent));

    await plugin.invoke("ssh/session/attach", { connectionId: "visual-connection", workbenchId: "visual-workbench", afterSequence: 0 });
    await vi.advanceTimersByTimeAsync(30);
    expect(stateEvents(events)).toEqual([]);

    await vi.advanceTimersByTimeAsync(4000);
    expect(stateEvents(events)).toEqual([
      { method: "ssh/session/state", params: { sessionId: "visual-session", state: "disconnected" } },
    ]);

    // 自动重连（再次 attach / open）后单发不复发。
    await plugin.invoke("ssh/session/attach", {});
    await plugin.invoke("ssh/session/open", {});
    await vi.advanceTimersByTimeAsync(5000);
    expect(stateEvents(events)).toHaveLength(1);
  });

  it("keeps ?err=disconnect working on the explicit open path (manual reconnect)", async () => {
    vi.useFakeTimers();
    const plugin = await loadMock("?err=disconnect");
    const events: RecordedEvent[] = [];
    plugin.onEvent((event) => events.push(event as unknown as RecordedEvent));

    await plugin.invoke("ssh/session/open", {});
    await vi.advanceTimersByTimeAsync(4000);
    expect(stateEvents(events)).toEqual([
      { method: "ssh/session/state", params: { sessionId: "visual-session", state: "disconnected" } },
    ]);
  });

  // R3-P2-2 夹具缺陷收口：撞名 rename 不得丢失源文件（旧实现先摘源再写目标，
  // mockWriteEntry 撞名抛错后源节点已丢）。
  it("sftp/rename overwrites an existing target atomically without losing the source", async () => {
    const plugin = await loadMock("");
    await plugin.invoke("sftp/rename", { sourcePath: "/home/demo/deploy.sh", targetPath: "/home/demo/docker-compose.yml" });
    const list = (await plugin.invoke("sftp/list", { path: "/home/demo" })) as { entries: Array<{ name: string }> };
    const names = list.entries.map((entry) => entry.name).sort();
    // deploy.sh 已改名落位到 docker-compose.yml（覆盖），server.log 不受影响，
    // 且源位置不会出现"既无 deploy.sh 也无 docker-compose.yml"的丢源状态。
    expect(names).toContain("docker-compose.yml");
    expect(names).toContain("server.log");
    expect(names).not.toContain("deploy.sh");
    expect(names.filter((name) => name === "docker-compose.yml")).toHaveLength(1);
    const moved = (await plugin.invoke("sftp/stat", { path: "/home/demo/docker-compose.yml" })) as { kind: string };
    expect(moved.kind).toBe("file");
  });

  it("sftp/rename moves across directories without duplicating or dropping nodes", async () => {
    const plugin = await loadMock("");
    await plugin.invoke("sftp/rename", { sourcePath: "/home/demo/server.log", targetPath: "/tmp/server.log" });
    const home = (await plugin.invoke("sftp/list", { path: "/home/demo" })) as { entries: Array<{ name: string }> };
    const tmp = (await plugin.invoke("sftp/list", { path: "/tmp" })) as { entries: Array<{ name: string }> };
    expect(home.entries.map((entry) => entry.name)).not.toContain("server.log");
    expect(tmp.entries.map((entry) => entry.name)).toContain("server.log");
  });
});
