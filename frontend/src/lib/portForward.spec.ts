import { describe, expect, it } from "vitest";
import {
  applyForwardState,
  formatForwardBytes,
  formatForwardRoute,
  forwardStartParams,
  parseForwards,
  validateForwardForm,
  type ForwardFormDraft,
  type PortForward,
} from "./portForward";

const row = (overrides: Partial<PortForward>): PortForward => ({
  id: "fwd-1",
  sessionId: "s-1",
  connectionId: "conn-1",
  kind: "local",
  listenHost: "127.0.0.1",
  listenPort: 8080,
  boundPort: 8080,
  targetHost: "db.internal",
  targetPort: 5432,
  state: "active",
  connectionsTotal: 3,
  connectionsActive: 1,
  bytesUp: 100,
  bytesDown: 200,
  ...overrides,
});

describe("parseForwards", () => {
  it("parses a list payload and drops invalid rows", () => {
    const forwards = parseForwards({
      forwards: [
        row({}),
        { id: "" },
        null,
        row({ id: "fwd-2", kind: "remote", boundPort: 31234, listenPort: 0, state: "active" }),
      ],
    });
    expect(forwards.map((f) => f.id)).toEqual(["fwd-1", "fwd-2"]);
    expect(forwards[1]).toMatchObject({ kind: "remote", listenPort: 0, boundPort: 31234 });
  });

  it("parses a single start payload and defaults listen host", () => {
    const forwards = parseForwards({ forward: row({ listenHost: "" }) });
    expect(forwards).toHaveLength(1);
    expect(forwards[0].listenHost).toBe("127.0.0.1");
  });

  it("returns an empty list for junk payloads", () => {
    expect(parseForwards(null)).toEqual([]);
    expect(parseForwards({ junk: true })).toEqual([]);
  });
});

describe("applyForwardState", () => {
  it("updates the matching row state and error in place", () => {
    const next = applyForwardState([row({}), row({ id: "fwd-2" })], {
      id: "fwd-1",
      state: "error",
      error: "Server refused",
    });
    expect(next[0].state).toBe("error");
    expect(next[0].error).toBe("Server refused");
    expect(next[1].state).toBe("active");
  });

  it("ignores events without a usable id", () => {
    const rows = [row({})];
    expect(applyForwardState(rows, { state: "stopped" })).toBe(rows);
  });
});

describe("validateForwardForm", () => {
  const draft = (overrides: Partial<ForwardFormDraft>): ForwardFormDraft => ({
    kind: "local",
    listenHost: "127.0.0.1",
    listenPort: "8080",
    targetHost: "db.internal",
    targetPort: "5432",
    ...overrides,
  });

  it("accepts a complete form and server-picked ports", () => {
    expect(validateForwardForm(draft({}))).toBeNull();
    expect(validateForwardForm(draft({ listenPort: "0", targetPort: "65535" }))).toBeNull();
  });

  it("rejects a missing target host", () => {
    expect(validateForwardForm(draft({ targetHost: "  " }))).toBe("targetHost");
  });

  it("rejects non-integer and out-of-range ports", () => {
    expect(validateForwardForm(draft({ listenPort: "" }))).toBe("port");
    expect(validateForwardForm(draft({ listenPort: "abc" }))).toBe("port");
    expect(validateForwardForm(draft({ targetPort: "-1" }))).toBe("port");
    expect(validateForwardForm(draft({ targetPort: "65536" }))).toBe("port");
  });
});

describe("forwardStartParams", () => {
  it("coerces ports to numbers and trims hosts", () => {
    expect(
      forwardStartParams(
        { kind: "remote", listenHost: " 0.0.0.0 ", listenPort: "0", targetHost: " web ", targetPort: "3000" },
        "s-9",
      ),
    ).toEqual({
      sessionId: "s-9",
      kind: "remote",
      listenHost: "0.0.0.0",
      listenPort: 0,
      targetHost: "web",
      targetPort: 3000,
    });
  });
});

describe("formatForwardRoute", () => {
  it("renders both directions with the bound port", () => {
    expect(formatForwardRoute(row({}))).toBe("127.0.0.1:8080 → db.internal:5432");
    expect(
      formatForwardRoute(row({ kind: "remote", listenPort: 0, boundPort: 31234, targetHost: "web", targetPort: 3000 })),
    ).toBe("127.0.0.1:31234 ← web:3000");
  });
});

describe("formatForwardBytes", () => {
  it("scales into readable units", () => {
    expect(formatForwardBytes(0)).toBe("0 B");
    expect(formatForwardBytes(1023)).toBe("1023 B");
    expect(formatForwardBytes(2048)).toBe("2.0 KiB");
    expect(formatForwardBytes(5 * 1024 * 1024)).toBe("5.0 MiB");
  });
});
