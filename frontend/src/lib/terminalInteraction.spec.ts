// 键位路由与右键语义已迁往可编辑设置：键位在 terminalHotkeys.spec.ts，
// 右键/粘贴/响铃等终端行为在 terminalBehavior.spec.ts。本文件只覆盖不可配置的
// 门禁与搜索辅助逻辑。
import { describe, expect, it } from "vitest";
import { canAcceptFileDrop, canAcceptTerminalDrop, isApplePlatform, normalizeDropTargetDir, sanitizeSearchOptions, terminalSearchSeedFromSelection } from "./terminalInteraction";

describe("Apple platform detection", () => {
  it("detects Apple platforms from the user agent", () => {
    expect(isApplePlatform("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")).toBe(true);
    expect(isApplePlatform("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe(false);
    expect(isApplePlatform("Mozilla/5.0 (X11; Linux x86_64)")).toBe(false);
  });
});

describe("terminal search option persistence", () => {
  it("falls back to all-off defaults for missing or malformed storage", () => {
    expect(sanitizeSearchOptions(null)).toEqual({ caseSensitive: false, regex: false, wholeWord: false });
    expect(sanitizeSearchOptions("")).toEqual({ caseSensitive: false, regex: false, wholeWord: false });
    expect(sanitizeSearchOptions("not json {")).toEqual({ caseSensitive: false, regex: false, wholeWord: false });
    expect(sanitizeSearchOptions('{"caseSensitive":"yes"}')).toEqual({ caseSensitive: false, regex: false, wholeWord: false });
  });

  it("honors only strict true flags and drops unknown fields", () => {
    expect(sanitizeSearchOptions('{"caseSensitive":true,"wholeWord":true,"hacker":1}')).toEqual({ caseSensitive: true, regex: false, wholeWord: true });
    expect(sanitizeSearchOptions('{"regex":true}')).toEqual({ caseSensitive: false, regex: true, wholeWord: false });
  });
});

describe("terminal search seed from selection", () => {
  it("keeps the first line and clamps it to a bounded length", () => {
    expect(terminalSearchSeedFromSelection("")).toBe("");
    expect(terminalSearchSeedFromSelection("hello world")).toBe("hello world");
    expect(terminalSearchSeedFromSelection("first\r\nsecond")).toBe("first");
    expect(terminalSearchSeedFromSelection("first\nsecond\nthird")).toBe("first");
    expect(terminalSearchSeedFromSelection("a".repeat(500)).length).toBe(200);
  });
});

describe("terminal drop acceptance", () => {
  it("requires a connected, writable session with no file transfer protocol owning the stream", () => {
    expect(canAcceptTerminalDrop({ connected: true, canWrite: true, transferBusy: false })).toBe(true);
    expect(canAcceptTerminalDrop({ connected: false, canWrite: true, transferBusy: false })).toBe(false);
    expect(canAcceptTerminalDrop({ connected: true, canWrite: false, transferBusy: false })).toBe(false);
    expect(canAcceptTerminalDrop({ connected: true, canWrite: true, transferBusy: true })).toBe(false);
  });
});

describe("shared file drop gate", () => {
  it("is the writable-session gate every drop channel shares, without the terminal-occupancy term", () => {
    expect(canAcceptFileDrop({ connected: true, canWrite: true })).toBe(true);
    expect(canAcceptFileDrop({ connected: false, canWrite: true })).toBe(false);
    expect(canAcceptFileDrop({ connected: true, canWrite: false })).toBe(false);
    // terminal gate degrades to the shared gate when nothing owns the stream
    expect(canAcceptTerminalDrop({ connected: true, canWrite: true, transferBusy: false })).toBe(canAcceptFileDrop({ connected: true, canWrite: true }));
  });
});

describe("terminal drop prompt target directory", () => {
  it("treats blank input as unusable so the confirm button stays disabled", () => {
    expect(normalizeDropTargetDir("")).toBeNull();
    expect(normalizeDropTargetDir("   ")).toBeNull();
    expect(normalizeDropTargetDir("\t / \n")).toBe("/");
  });

  it("collapses trailing slashes while keeping the bare root intact", () => {
    expect(normalizeDropTargetDir("/")).toBe("/");
    expect(normalizeDropTargetDir("//")).toBe("/");
    expect(normalizeDropTargetDir("/data/uploads/")).toBe("/data/uploads");
    expect(normalizeDropTargetDir("/data/uploads///")).toBe("/data/uploads");
  });

  it("keeps ordinary paths and inner slashes untouched", () => {
    expect(normalizeDropTargetDir("/var/log")).toBe("/var/log");
    expect(normalizeDropTargetDir("  /home/user/uploads  ")).toBe("/home/user/uploads");
  });
});
