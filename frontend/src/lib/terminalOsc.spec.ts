import { describe, expect, it } from "vitest";
import { colorToOscRgb, handleOsc52ClipboardWrite, handleTerminalColorQuery, parseCssColorToRgb } from "./terminalOsc";

function fakeTerminal() {
  const written: string[] = [];
  return {
    written,
    input(data: string) {
      written.push(data);
    },
  };
}

describe("parseCssColorToRgb", () => {
  it("parses short and long hex colors", () => {
    expect(parseCssColorToRgb("#fff")).toEqual([255, 255, 255]);
    expect(parseCssColorToRgb("#0d1117")).toEqual([13, 17, 23]);
  });

  it("rejects transparent and fully transparent alpha", () => {
    expect(parseCssColorToRgb("transparent")).toBeNull();
    expect(parseCssColorToRgb("#0d111700")).toBeNull();
  });

  it("parses rgb()/rgba() with percent channels and slash alpha", () => {
    expect(parseCssColorToRgb("rgb(16, 32, 64)")).toEqual([16, 32, 64]);
    expect(parseCssColorToRgb("rgba(16 32 64 / 50%)")).toEqual([16, 32, 64]);
    expect(parseCssColorToRgb("rgb(100% 0% 0%)")).toEqual([255, 0, 0]);
  });

  it("returns null for unparsable colors", () => {
    expect(parseCssColorToRgb("")).toBeNull();
    expect(parseCssColorToRgb("not-a-color")).toBeNull();
    expect(parseCssColorToRgb("rgb(1, 2)")).toBeNull();
  });
});

describe("colorToOscRgb", () => {
  it("formats rgb:rr/gg/bb", () => {
    expect(colorToOscRgb("#0d1117")).toBe("rgb:0d/11/17");
    expect(colorToOscRgb("bogus")).toBe("");
  });
});

describe("handleTerminalColorQuery", () => {
  it("answers ? queries with the theme color via terminal.input", () => {
    const term = fakeTerminal();
    expect(handleTerminalColorQuery(term, 11, "#0d1117", "#000000", "?")).toBe(true);
    expect(term.written).toEqual(["\x1b]11;rgb:0d/11/17\x1b\\"]);
  });

  it("falls back when the theme color is unparsable", () => {
    const term = fakeTerminal();
    expect(handleTerminalColorQuery(term, 10, "bogus", "#c9d1d9", "?")).toBe(true);
    expect(term.written).toEqual(["\x1b]10;rgb:c9/d1/d9\x1b\\"]);
  });

  it("passes color-set sequences through unhandled", () => {
    const term = fakeTerminal();
    expect(handleTerminalColorQuery(term, 10, "#ffffff", "#000000", "rgb:ff/ff/ff")).toBe(false);
    expect(term.written).toEqual([]);
  });
});

describe("handleOsc52ClipboardWrite", () => {
  it("writes decoded utf-8 payload to the clipboard for target c", () => {
    const written: string[] = [];
    // "内容" = E5 86 85 E5 AE B9
    expect(handleOsc52ClipboardWrite("c;5YaF5a65", (text) => written.push(text))).toBe(true);
    expect(written).toEqual(["内容"]);
  });

  it("swallows read queries without responding", () => {
    const written: string[] = [];
    expect(handleOsc52ClipboardWrite("c;?", (text) => written.push(text))).toBe(true);
    expect(written).toEqual([]);
  });

  it("ignores primary-selection targets and malformed payloads", () => {
    const written: string[] = [];
    expect(handleOsc52ClipboardWrite("p;5YaF5a65", (text) => written.push(text))).toBe(false);
    expect(handleOsc52ClipboardWrite("c;@@@@", (text) => written.push(text))).toBe(true);
    expect(handleOsc52ClipboardWrite("no-semicolon", (text) => written.push(text))).toBe(false);
    expect(written).toEqual([]);
  });
});
