import { describe, expect, it } from "vitest";
import { pickLiveSessionForReattach, type SessionSummary } from "./sessionRestore";

const session = (overrides: Partial<SessionSummary>): SessionSummary => ({
  sessionId: "s-1",
  connectionId: "conn-1",
  workbenchId: "wb-1",
  connected: true,
  createdAt: 100,
  ...overrides,
});

describe("pickLiveSessionForReattach", () => {
  it("returns the same-workbench live session", () => {
    expect(
      pickLiveSessionForReattach(
        [session({ sessionId: "s-1" }), session({ sessionId: "s-2", connectionId: "conn-2" })],
        { connectionId: "conn-1", workbenchId: "wb-1" },
      ),
    ).toBe("s-1");
  });

  it("reattaches the connection's live session under a freshly minted workbenchId", () => {
    // Sidebar reopen replaces workbenchId: strict double-matching would
    // force a redundant SSH re-dial.
    expect(
      pickLiveSessionForReattach([session({ sessionId: "s-live", workbenchId: "wb-old" })], {
        connectionId: "conn-1",
        workbenchId: "wb-new",
      }),
    ).toBe("s-live");
  });

  it("prefers the newest createdAt when several live sessions exist", () => {
    const sessions = [
      session({ sessionId: "s-old", createdAt: 50, workbenchId: "wb-x" }),
      session({ sessionId: "s-new", createdAt: 200, workbenchId: "wb-y" }),
    ];
    expect(pickLiveSessionForReattach(sessions, { connectionId: "conn-1", workbenchId: "wb-z" })).toBe("s-new");
  });

  it("ignores dead sessions and other connections", () => {
    expect(
      pickLiveSessionForReattach(
        [session({ connected: false }), session({ connectionId: "conn-2", workbenchId: "wb-9" })],
        { connectionId: "conn-1", workbenchId: "wb-1" },
      ),
    ).toBe("");
  });

  it("returns empty on malformed payloads or missing connection context", () => {
    expect(pickLiveSessionForReattach(undefined, { connectionId: "conn-1", workbenchId: "wb-1" })).toBe("");
    expect(pickLiveSessionForReattach([null as unknown as SessionSummary], { connectionId: "conn-1", workbenchId: "wb-1" })).toBe("");
    expect(pickLiveSessionForReattach([session({})], { connectionId: "", workbenchId: "wb-1" })).toBe("");
  });
});
