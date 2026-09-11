// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";
import type { TriageResult } from "./lib/alertTriage";

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

  // round2：锁定 ssh/alert/triage mock 的契约形状（镜像后端 alert_triage::
  // TriageResult：normalized/category/suggestions + purposeKey），让告警排查
  // 弹窗在 mock.html 可无手填走查（P1-1 焦点/Esc 修复的浏览器级验证面）。
  it("ssh/alert/triage mirrors the sidecar TriageResult shape for JSON and plain-text payloads", async () => {
    const plugin = await loadMock("");
    const payloadText = JSON.stringify({ alertId: "a-1", title: "Disk pressure", severity: "Critical", source: "Node", data: { used: "87%" } });
    const json = (await plugin.invoke("ssh/alert/triage", { payload: payloadText })) as TriageResult;
    expect(json.category).toBe("disk");
    // severity/source 小写化；message 缺失时回退整段 payload（后端 openocta
    // 兼容语义，不因 title 存在而变空）；data 对象转 pretty JSON。
    expect(json.normalized.alertId).toBe("a-1");
    expect(json.normalized.title).toBe("Disk pressure");
    expect(json.normalized.message).toBe(payloadText);
    expect(json.normalized.severity).toBe("critical");
    expect(json.normalized.source).toBe("node");
    expect(json.normalized.dataJson).toContain('"used"');
    // disk 分类命中后端 Disk playbook 的同款命令清单。
    expect(json.suggestions).toEqual([
      { command: "df -h", purposeKey: "diskUsage" },
      { command: "du -x -d 1 / | sort -rh | head -15", purposeKey: "diskDu" },
    ]);

    const plain = (await plugin.invoke("ssh/alert/triage", { payload: "  kernel: oom killer triggered on pid 4211  " })) as TriageResult;
    expect(plain.category).toBe("oom");
    // 纯文本告警：trim 后整段作为 message，severity 兜底 unknown。
    expect(plain.normalized.message).toBe("kernel: oom killer triggered on pid 4211");
    expect(plain.normalized.severity).toBe("unknown");
    for (const suggestion of plain.suggestions) {
      expect(typeof suggestion.command).toBe("string");
      expect(suggestion.purposeKey.length).toBeGreaterThan(0);
    }
  });

  // round2：onLocaleChange 夹具补齐（env.d.ts 宿主 1.1 形状）：?locale= 定初值，
  // __dbxMockSetLocale 模拟宿主 updateLocale 推送，供 i18n 切换链走查。
  it("exposes onLocaleChange with ?locale= initial value and runtime switching", async () => {
    const plugin = await loadMock("?locale=ja");
    expect(plugin.locale).toBe("ja");
    const seen: string[] = [];
    const unsubscribe = plugin.onLocaleChange!((locale) => seen.push(locale));
    // 订阅即回调当前 locale（与 mock 的 onAppearanceChange/onContextChange 同构）。
    expect(seen).toEqual(["ja"]);

    const setLocale = (window as unknown as { __dbxMockSetLocale?: (next: string) => void }).__dbxMockSetLocale;
    expect(typeof setLocale).toBe("function");
    setLocale!("zh-CN");
    expect(plugin.locale).toBe("zh-CN");
    expect(seen).toEqual(["ja", "zh-CN"]);
    unsubscribe();
    setLocale!("en");
    expect(plugin.locale).toBe("en");
    expect(seen).toEqual(["ja", "zh-CN"]);
  });
});
