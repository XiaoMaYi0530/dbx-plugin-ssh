import { describe, expect, it } from "vitest";
import { resolveTerminalRightClickAction, sanitizeSelectCopyEnabled } from "./terminalInteraction";

describe("terminal interaction preferences (select-to-copy / right-click-paste)", () => {
  it("defaults select-to-copy to enabled and only honors an explicit 'false'", () => {
    expect(sanitizeSelectCopyEnabled(null)).toBe(true);
    expect(sanitizeSelectCopyEnabled("")).toBe(true);
    expect(sanitizeSelectCopyEnabled("true")).toBe(true);
    expect(sanitizeSelectCopyEnabled("garbage")).toBe(true);
    expect(sanitizeSelectCopyEnabled("false")).toBe(false);
  });

  it("routes plain right-click to paste only while the mode is on", () => {
    expect(resolveTerminalRightClickAction({ selectCopy: true, shiftKey: false })).toBe("paste");
    // Shift+right-click keeps the context menu reachable even in paste mode.
    expect(resolveTerminalRightClickAction({ selectCopy: true, shiftKey: true })).toBe("menu");
    // Mode off: right-click always opens the menu (historical behavior).
    expect(resolveTerminalRightClickAction({ selectCopy: false, shiftKey: false })).toBe("menu");
    expect(resolveTerminalRightClickAction({ selectCopy: false, shiftKey: true })).toBe("menu");
  });
});
