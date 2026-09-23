import { describe, expect, it } from "vitest";
import { createWebkitInputController, isMacWebkitInputFallbackRequired } from "./terminalWebkitInput";

const keydown = (key: string, keyCode: number, at: number) => ({
  key,
  keyCode,
  ctrlKey: false,
  metaKey: false,
  altKey: false,
  isComposing: false,
  at,
});
const input = (data: string, at: number) => ({ data, inputType: "insertText", isComposing: false, at });

describe("macOS WebKit terminal input fallback", () => {
  it("emits every character when input precedes keyCode=229 keydown", () => {
    const controller = createWebkitInputController();

    expect(controller.input(input("c", 10))).toBe("c");
    expect(controller.keydown(keydown("c", 229, 11))).toBe("suppress");
    expect(controller.input(input("l", 40))).toBe("l");
    expect(controller.keydown(keydown("l", 229, 41))).toBe("suppress");
    expect(controller.input(input("e", 70))).toBe("e");
    expect(controller.keydown(keydown("e", 229, 71))).toBe("suppress");
  });

  it("handles keyCode=229 before input without starting xterm's deferred diff", () => {
    const controller = createWebkitInputController();

    expect(controller.keydown(keydown("x", 229, 10))).toBe("suppress");
    expect(controller.input(input("x", 11))).toBe("x");
  });

  it("leaves normal keydown-first input on xterm's path", () => {
    const controller = createWebkitInputController();

    expect(controller.keydown(keydown("a", 65, 10))).toBe("pass");
    expect(controller.input(input("a", 11))).toBeUndefined();
    expect(controller.keydown(keydown("b", 66, 20))).toBe("pass");
    expect(controller.input(input("b", 21))).toBeUndefined();
  });

  it("does not intercept real composition input or its immediate commit", () => {
    const controller = createWebkitInputController();
    controller.compositionStart();

    expect(controller.keydown({ ...keydown("a", 229, 10), isComposing: true })).toBe("pass");
    expect(controller.input({ ...input("a", 11), isComposing: true })).toBeUndefined();
    controller.compositionEnd(20);
    expect(controller.input(input("a", 21))).toBeUndefined();
  });

  it("normalizes WebKit non-breaking space and ignores shortcuts/non-ASCII text", () => {
    const controller = createWebkitInputController();

    expect(controller.input(input("\u00a0", 10))).toBe(" ");
    expect(controller.keydown({ ...keydown("v", 229, 20), metaKey: true })).toBe("pass");
    expect(controller.input(input("你", 30))).toBeUndefined();
  });

  it("only enables the adapter on macOS", () => {
    expect(isMacWebkitInputFallbackRequired("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")).toBe(true);
    expect(isMacWebkitInputFallbackRequired("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe(false);
  });
});
