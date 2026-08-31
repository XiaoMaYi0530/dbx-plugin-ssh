import { describe, expect, it } from "vitest";
import {
  AGENT_MODES,
  approvalRemainingSecs,
  dropAgentPrompt,
  enqueueAgentPrompt,
  findAgentPrompt,
  type AgentPromptPayload,
} from "./agentTerminal";

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

describe("agent prompt queue helpers", () => {
  const first: AgentPromptPayload = {
    challengeId: "c1", sessionId: "s1", tool: "ssh_exec", command: "ls", risk: "low", requestedAt: 1000, timeoutSecs: 120,
  };
  const second: AgentPromptPayload = { ...first, challengeId: "c2", risk: "elevated" };
  const third: AgentPromptPayload = { ...first, challengeId: "c3", sessionId: "s2" };

  it("appends new challenges in arrival order", () => {
    expect(enqueueAgentPrompt([], first)).toEqual([first]);
    expect(enqueueAgentPrompt(enqueueAgentPrompt([], first), second)).toEqual([first, second]);
  });

  it("dedupes by challengeId and returns an equivalent copy instead of the same reference", () => {
    const queue = [first];
    const result = enqueueAgentPrompt(queue, { ...first });
    expect(result).toEqual([first]);
    expect(result).not.toBe(queue);
  });

  it("never mutates the input queue", () => {
    const queue = [first, second];
    enqueueAgentPrompt(queue, third);
    expect(queue).toEqual([first, second]);
    dropAgentPrompt(queue, first.challengeId);
    expect(queue).toEqual([first, second]);
  });

  it("removes only the matching challenge and preserves the order of the rest", () => {
    const queue = [first, second, third];
    expect(dropAgentPrompt(queue, second.challengeId)).toEqual([first, third]);
    expect(dropAgentPrompt(queue, first.challengeId)).toEqual([second, third]);
  });

  it("returns an equivalent array when dropping an unknown challengeId", () => {
    const queue = [first, second];
    expect(dropAgentPrompt(queue, "missing")).toEqual([first, second]);
  });

  it("finds a challenge by challengeId or returns undefined", () => {
    const queue = [first, second];
    expect(findAgentPrompt(queue, "c2")).toBe(second);
    expect(findAgentPrompt(queue, "missing")).toBeUndefined();
  });
});
