import { describe, expect, it } from "vitest";
import { AGENT_MODES, approvalRemainingSecs } from "./agentTerminal";

describe("agent terminal mode contract", () => {
  it("keeps the canonical mode order off/auto/strict", () => {
    expect(AGENT_MODES).toEqual(["off", "auto", "strict"]);
    expect([...AGENT_MODES]).toHaveLength(3);
  });

  it("counts down from requestedAt + timeoutSecs against a millisecond clock", () => {
    // requestedAt=1000s, timeout=120s → 期限 1120s；now=1120_000ms 恰好为 0。
    const payload = { requestedAt: 1000, timeoutSecs: 120 };
    expect(approvalRemainingSecs(payload, 1_000_000)).toBe(120);
    expect(approvalRemainingSecs(payload, 1_060_500)).toBeCloseTo(59.5, 6);
    expect(approvalRemainingSecs(payload, 1_120_000)).toBe(0);
  });

  it("clamps negative remainders to zero once the deadline has passed", () => {
    const payload = { requestedAt: 1000, timeoutSecs: 30 };
    expect(approvalRemainingSecs(payload, 1_030_001)).toBe(0);
    expect(approvalRemainingSecs(payload, 2_000_000)).toBe(0);
  });

  it("treats a zero timeout as immediately expired at the deadline", () => {
    const payload = { requestedAt: 1000, timeoutSecs: 0 };
    expect(approvalRemainingSecs(payload, 1_000_000)).toBe(0);
    expect(approvalRemainingSecs(payload, 2_000_000)).toBe(0);
    // 未到期时仍返回微小的正剩余（0.001s），前端 250ms tick 会立即收口到 0。
    expect(approvalRemainingSecs(payload, 999_999)).toBeCloseTo(0.001, 6);
  });
});
