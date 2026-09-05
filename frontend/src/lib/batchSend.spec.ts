import { describe, expect, it } from "vitest";
import {
  batchTargetLabel,
  normalizeBatchTargets,
  selectBatchTargets,
  summarizeBatchResults,
  toggleBatchTarget,
  type BatchSendTarget,
} from "./batchSend";

const row = (overrides: Partial<BatchSendTarget>): BatchSendTarget => ({
  sessionId: "s-1",
  connectionId: "conn-1",
  connected: true,
  host: "prod-01",
  port: 22,
  username: "ops",
  createdAt: 100,
  ...overrides,
});

describe("normalizeBatchTargets", () => {
  it("keeps list order and drops invalid or duplicate rows", () => {
    const targets = normalizeBatchTargets([
      row({ sessionId: "s-1" }),
      null,
      row({ sessionId: "s-1" }),
      { sessionId: "", connectionId: "conn-2" },
      row({ sessionId: "s-2", connected: false }),
      "junk",
    ]);
    expect(targets.map((target) => target.sessionId)).toEqual(["s-1", "s-2"]);
    expect(targets[0].host).toBe("prod-01");
    expect(targets[0].username).toBe("ops");
    expect(targets[1].connected).toBe(false);
  });

  it("returns empty on malformed payloads", () => {
    expect(normalizeBatchTargets(undefined)).toEqual([]);
    expect(normalizeBatchTargets({ sessions: [] })).toEqual([]);
  });
});

describe("batchTargetLabel", () => {
  it("prefers user@host and falls back to short session id", () => {
    expect(batchTargetLabel(row({}))).toBe("ops@prod-01");
    expect(batchTargetLabel(row({ username: "" }))).toBe("prod-01");
    expect(batchTargetLabel(row({ host: "", sessionId: "abcdef123456" }))).toBe("abcdef12");
  });
});

describe("toggleBatchTarget / selectBatchTargets", () => {
  it("toggles membership without duplicates", () => {
    let selected = toggleBatchTarget(["s-1"], "s-2");
    expect(selected).toEqual(["s-1", "s-2"]);
    selected = toggleBatchTarget(selected, "s-1");
    expect(selected).toEqual(["s-2"]);
  });

  it("selects all or only live sessions", () => {
    const targets = normalizeBatchTargets([
      row({ sessionId: "s-1" }),
      row({ sessionId: "s-2", connected: false }),
      row({ sessionId: "s-3" }),
    ]);
    expect(selectBatchTargets(targets, "all")).toEqual(["s-1", "s-2", "s-3"]);
    expect(selectBatchTargets(targets, "connected")).toEqual(["s-1", "s-3"]);
  });
});

describe("summarizeBatchResults", () => {
  it("counts sent and failed rows from the backend response", () => {
    const summary = summarizeBatchResults([
      { sessionId: "s-1", success: true },
      { sessionId: "s-2", success: false, error: "SSH session was not found" },
      { sessionId: "", success: true },
      "junk",
    ]);
    expect(summary.sent).toBe(1);
    expect(summary.failed).toBe(1);
    expect(summary.rows).toEqual([
      { sessionId: "s-1", success: true, error: undefined },
      { sessionId: "s-2", success: false, error: "SSH session was not found" },
    ]);
  });

  it("treats malformed payloads as zero counts", () => {
    expect(summarizeBatchResults(undefined)).toEqual({ rows: [], sent: 0, failed: 0 });
    expect(summarizeBatchResults("nope" as unknown as unknown[]).failed).toBe(0);
  });
});
