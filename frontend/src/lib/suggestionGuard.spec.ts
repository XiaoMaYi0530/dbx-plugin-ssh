import { describe, expect, it } from "vitest";
import {
  canShowSuggestions,
  createSuggestionGuardState,
  isPagerKeystroke,
  isSuppressiveCommand,
  type SuggestionGuardState,
} from "./suggestionGuard";

function evaluate(input: {
  alternateActive?: boolean;
  lastCommand?: string | null;
  typingChar?: string | null;
  lineEmpty?: boolean;
}, state: SuggestionGuardState = createSuggestionGuardState()) {
  return canShowSuggestions(
    {
      alternateActive: input.alternateActive ?? false,
      lastCommand: input.lastCommand ?? null,
      typingChar: input.typingChar ?? null,
      lineEmpty: input.lineEmpty ?? true,
    },
    state,
  );
}

describe("isSuppressiveCommand", () => {
  it("flags the known fullscreen/pager programs", () => {
    for (const command of ["btop", "htop", "less +F file", "man bash", "more file", "nano a.txt", "nvim", "top", "vi", "vim", "watch df"]) {
      expect(isSuppressiveCommand(command), command).toBe(true);
    }
  });

  it("matches program names regardless of paths and leading whitespace", () => {
    expect(isSuppressiveCommand("/usr/bin/htop")).toBe(true);
    expect(isSuppressiveCommand("  vim app.rs")).toBe(true);
    expect(isSuppressiveCommand("env LESS=-R less log")).toBe(true);
  });

  it("flags journalctl without --no-pager but not with it", () => {
    expect(isSuppressiveCommand("journalctl -u nginx")).toBe(true);
    expect(isSuppressiveCommand("journalctl -u nginx --no-pager")).toBe(false);
  });

  it("flags following tails but not plain tails", () => {
    expect(isSuppressiveCommand("tail -f app.log")).toBe(true);
    expect(isSuppressiveCommand("tail -Fn app.log")).toBe(true);
    expect(isSuppressiveCommand("tail -n 20 app.log")).toBe(false);
  });

  it("leaves ordinary commands alone", () => {
    expect(isSuppressiveCommand("ls -la")).toBe(false);
    expect(isSuppressiveCommand("git status")).toBe(false);
    expect(isSuppressiveCommand("docker compose up -d")).toBe(false);
    expect(isSuppressiveCommand(null)).toBe(false);
    expect(isSuppressiveCommand("")).toBe(false);
  });
});

describe("isPagerKeystroke", () => {
  it("flags pager navigation keys only at an empty line", () => {
    for (const char of [" ", "b", "g", "G", "n", "N", "q", "/", "?", ":"]) {
      expect(isPagerKeystroke(char, true), char).toBe(true);
      expect(isPagerKeystroke(char, false), char).toBe(false);
    }
  });

  it("ignores printable typing and control keys", () => {
    expect(isPagerKeystroke("a", true)).toBe(false);
    expect(isPagerKeystroke(null, true)).toBe(false);
  });
});

describe("canShowSuggestions", () => {
  it("shows suggestions for ordinary typing in the normal buffer", () => {
    const result = evaluate({ typingChar: "d", lastCommand: "ls" });
    expect(result.show).toBe(true);
  });

  it("hides while an alternate-buffer program owns the screen", () => {
    const result = evaluate({ alternateActive: true, typingChar: "j" });
    expect(result.show).toBe(false);
  });

  it("latches suppression after a suppressive command runs", () => {
    const entered = evaluate({ lastCommand: "htop", typingChar: null });
    expect(entered.show).toBe(false);
    expect(entered.state.suppressed).toBe(true);
    const typing = evaluate({ typingChar: "x", lastCommand: "htop" }, entered.state);
    expect(typing.show).toBe(false);
  });

  it("lifts the latch on Ctrl+C or q", () => {
    const entered = evaluate({ lastCommand: "less app.log", typingChar: null }).state;
    const ctrlC = evaluate({ typingChar: "\u0003", lastCommand: "less app.log" }, entered);
    expect(ctrlC.show).toBe(false);
    expect(ctrlC.state.suppressed).toBe(false);
    const q = evaluate({ typingChar: "q", lastCommand: "less app.log" }, entered);
    expect(q.state.suppressed).toBe(false);
  });

  it("lifts the latch when a non-suppressive command runs next", () => {
    const entered = evaluate({ lastCommand: "watch -n1 df", typingChar: null }).state;
    const after = evaluate({ typingChar: "l", lastCommand: "ls -la" }, entered);
    expect(after.show).toBe(true);
    expect(after.state.suppressed).toBe(false);
  });

  it("applies the pager keystroke heuristic at the line start", () => {
    expect(evaluate({ typingChar: "/", lineEmpty: true }).show).toBe(false);
    expect(evaluate({ typingChar: "/", lineEmpty: false }).show).toBe(true);
    expect(evaluate({ typingChar: "g", lineEmpty: true }).show).toBe(false);
    expect(evaluate({ typingChar: "g", lineEmpty: false }).show).toBe(true);
  });
});
