import { describe, expect, it } from "vitest";
import {
  computeGutterRows,
  formatTimestamp,
  getRenderCellHeight,
  sanitizeGutterSettings,
  trimTimestampMap,
  GUTTER_TIMESTAMP_DEFAULT_FORMAT,
  type GutterBufferLike,
} from "./terminalGutter";

/** isWrapped 布尔表 → 最小 buffer 视图。 */
function bufferOf(wrapped: boolean[], type: "normal" | "alternate" = "normal"): GutterBufferLike {
  return {
    type,
    length: wrapped.length,
    getLine: (y: number) => (y >= 0 && y < wrapped.length ? { isWrapped: wrapped[y] } : undefined),
  };
}

/** 本地时区构造时间（避免 CI 时区影响断言）。 */
function localTime(parts: { year?: number; month?: number; day?: number; hours: number; minutes: number; seconds?: number; ms?: number }): number {
  return new Date(
    parts.year ?? 2026,
    parts.month ?? 8,
    parts.day ?? 23,
    parts.hours,
    parts.minutes,
    parts.seconds ?? 0,
    parts.ms ?? 0,
  ).getTime();
}

describe("gutter: formatTimestamp", () => {
  it("formats the default [HH:mm:ss] token set", () => {
    const time = localTime({ hours: 7, minutes: 5, seconds: 9 });
    expect(formatTimestamp(time)).toBe("[07:05:09]");
    expect(formatTimestamp(time, GUTTER_TIMESTAMP_DEFAULT_FORMAT)).toBe("[07:05:09]");
  });

  it("expands YYYY YY MM DD HH mm ss SSS tokens", () => {
    const time = localTime({ year: 2026, month: 8, day: 23, hours: 7, minutes: 5, seconds: 9, ms: 42 });
    expect(formatTimestamp(time, "YYYY-MM-DD HH:mm:ss.SSS")).toBe("2026-09-23 07:05:09.042");
    expect(formatTimestamp(time, "YY/MM/DD HH:mm")).toBe("26/09/23 07:05");
    expect(formatTimestamp(time, "HHmmss-SSS")).toBe("070509-042");
  });

  it("keeps literal text outside tokens intact", () => {
    const time = localTime({ hours: 23, minutes: 1, seconds: 2 });
    expect(formatTimestamp(time, "t=HH/mm")).toBe("t=23/01");
    expect(formatTimestamp(time, "[HH]")).toBe("[23]");
  });

  it("returns an empty string for invalid timestamps", () => {
    expect(formatTimestamp(Number.NaN)).toBe("");
  });

  it("falls back to the default format for empty or unknown formats", () => {
    const time = localTime({ hours: 7, minutes: 5, seconds: 9 });
    expect(formatTimestamp(time, "")).toBe("[07:05:09]");
  });
});

describe("gutter: computeGutterRows", () => {
  const flat = bufferOf(Array.from({ length: 100 }, () => false));

  it("maps viewport rows to pixel tops with 1-based ordinals", () => {
    const rows = computeGutterRows({
      cellHeight: 20,
      screenOffsetTop: 4,
      scrollTop: 10,
      buffer: flat,
      rows: 5,
      showLineNumbers: true,
      showTimestamps: false,
    });
    expect(rows).toEqual([
      { top: 4, text: "11" },
      { top: 24, text: "12" },
      { top: 44, text: "13" },
      { top: 64, text: "14" },
      { top: 84, text: "15" },
    ]);
  });

  it("skips wrapped continuation rows and keeps pixel slots aligned", () => {
    const wrapped = Array.from({ length: 100 }, (_, index) => index === 11 || index === 12);
    const rows = computeGutterRows({
      cellHeight: 20,
      screenOffsetTop: 4,
      scrollTop: 10,
      buffer: bufferOf(wrapped),
      rows: 5,
      showLineNumbers: true,
      showTimestamps: false,
    });
    // abs 11/12 是 wrapped 行：跳过，但像素槽位仍按视口行推进；
    // 行号取逻辑行首行的物理行号（1-based），wrapped 块只标一次。
    expect(rows).toEqual([
      { top: 4, text: "11" },
      { top: 64, text: "14" },
      { top: 84, text: "15" },
    ]);
  });

  it("returns an empty list for the alternate buffer", () => {
    const rows = computeGutterRows({
      cellHeight: 20,
      screenOffsetTop: 0,
      scrollTop: 0,
      buffer: bufferOf([false, false, false], "alternate"),
      rows: 3,
      showLineNumbers: true,
      showTimestamps: true,
    });
    expect(rows).toEqual([]);
  });

  it("returns an empty list when the render cell height is unavailable", () => {
    const rows = computeGutterRows({
      cellHeight: null,
      screenOffsetTop: 0,
      scrollTop: 0,
      buffer: flat,
      rows: 3,
      showLineNumbers: true,
      showTimestamps: true,
    });
    expect(rows).toEqual([]);
  });

  it("clamps a negative scrollTop and truncates past the buffer end", () => {
    const small = bufferOf([false, false, false]);
    expect(computeGutterRows({ cellHeight: 10, screenOffsetTop: 0, scrollTop: -5, buffer: small, rows: 5, showLineNumbers: true, showTimestamps: false })).toEqual([
      { top: 0, text: "1" },
      { top: 10, text: "2" },
      { top: 20, text: "3" },
    ]);
    expect(computeGutterRows({ cellHeight: 10, screenOffsetTop: 0, scrollTop: 2, buffer: small, rows: 5, showLineNumbers: true, showTimestamps: false })).toEqual([
      { top: 0, text: "3" },
    ]);
  });

  it("renders timestamps keyed by the logical start row", () => {
    const time = localTime({ hours: 9, minutes: 30, seconds: 1 });
    const timestamps = new Map([[10, time]]);
    const rows = computeGutterRows({
      cellHeight: 10,
      screenOffsetTop: 0,
      scrollTop: 10,
      buffer: flat,
      rows: 3,
      timestamps,
      showLineNumbers: false,
      showTimestamps: true,
    });
    expect(rows).toEqual([{ top: 0, text: "[09:30:01]" }, { top: 10, text: "" }, { top: 20, text: "" }]);
  });

  it("shows a timestamp on the wrapped head row only and can combine both columns", () => {
    const time = localTime({ hours: 9, minutes: 30, seconds: 1 });
    const wrapped = Array.from({ length: 20 }, (_, index) => index === 11);
    const rows = computeGutterRows({
      cellHeight: 10,
      screenOffsetTop: 0,
      scrollTop: 10,
      buffer: bufferOf(wrapped),
      rows: 3,
      timestamps: new Map([[10, time]]),
      showLineNumbers: true,
      showTimestamps: true,
      timestampFormat: "HH:mm",
    });
    expect(rows).toEqual([{ top: 0, text: "11 09:30" }, { top: 20, text: "13" }]);
  });
});

describe("gutter: trimTimestampMap", () => {
  it("drops entries before the retention floor", () => {
    const map = new Map<number, number>([[5, 1], [3000, 2], [3100, 3]]);
    trimTimestampMap(map, 3000);
    expect([...map.keys()]).toEqual([3000, 3100]);
  });
});

describe("gutter: getRenderCellHeight", () => {
  it("reads the private render dimensions when present", () => {
    const fake = { _core: { _renderService: { dimensions: { css: { cell: { height: 17.5 } } } } } };
    expect(getRenderCellHeight(fake)).toBe(17.5);
  });

  it("degrades to null instead of throwing when inaccessible", () => {
    expect(getRenderCellHeight(undefined)).toBe(null);
    expect(getRenderCellHeight({})).toBe(null);
    expect(getRenderCellHeight({ _core: {} })).toBe(null);
    expect(getRenderCellHeight({ _core: { _renderService: { dimensions: { css: { cell: { height: 0 } } } } } })).toBe(null);
    expect(getRenderCellHeight({ _core: { _renderService: { dimensions: { css: { cell: { height: Number.NaN } } } } } })).toBe(null);
  });
});

describe("gutter: sanitizeGutterSettings", () => {
  it("defaults to everything off with the default timestamp format", () => {
    expect(sanitizeGutterSettings(undefined)).toEqual({
      showLineNumbers: false,
      showTimestamps: false,
      timestampFormat: GUTTER_TIMESTAMP_DEFAULT_FORMAT,
    });
  });

  it("normalizes fields individually", () => {
    expect(sanitizeGutterSettings({ showLineNumbers: true, showTimestamps: "yes", timestampFormat: 42 })).toEqual({
      showLineNumbers: true,
      showTimestamps: false,
      timestampFormat: GUTTER_TIMESTAMP_DEFAULT_FORMAT,
    });
  });

  it("caps the format length and strips unsupported characters", () => {
    const sanitized = sanitizeGutterSettings({ timestampFormat: `YY ${"x".repeat(100)}<>` });
    expect(sanitized.timestampFormat.startsWith("YY ")).toBe(true);
    expect(sanitized.timestampFormat.length).toBeLessThanOrEqual(64);
    expect(sanitized.timestampFormat).not.toContain("<");
    expect(sanitizeGutterSettings({ timestampFormat: "   " }).timestampFormat).toBe(GUTTER_TIMESTAMP_DEFAULT_FORMAT);
  });
});
