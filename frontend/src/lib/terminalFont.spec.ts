// 终端字体设置（issue #31）纯逻辑单测：持久化解析、非法值回退、
// 用户设置优先于宿主基准。存储统一注入，不触碰真实 localStorage。
import { describe, expect, it } from "vitest";
import {
  TERMINAL_FONT_FAMILY_KEY,
  TERMINAL_FONT_SIZE_KEY,
  loadTerminalFontOverride,
  parsePersistedTerminalFontFamily,
  parsePersistedTerminalFontSize,
  persistTerminalFontFamily,
  persistTerminalFontSize,
  resolveTerminalFont,
} from "./terminalFont";

const HOST = { fontFamily: "'JetBrains Mono', Consolas, monospace", fontSize: 13 };

describe("parsePersistedTerminalFontSize", () => {
  it("未设置时返回 null（由调用方回退宿主值）", () => {
    expect(parsePersistedTerminalFontSize(null)).toBeNull();
  });

  it("非法值回退：非数字与空串均返回 null", () => {
    expect(parsePersistedTerminalFontSize("abc")).toBeNull();
    expect(parsePersistedTerminalFontSize("")).toBeNull();
  });

  it("合法数字原样返回；越界 clamp 到 [8, 32] 边界", () => {
    expect(parsePersistedTerminalFontSize("15")).toBe(15);
    expect(parsePersistedTerminalFontSize("200")).toBe(32);
    expect(parsePersistedTerminalFontSize("-5")).toBe(8);
  });
});

describe("parsePersistedTerminalFontFamily", () => {
  it("未设置（null）与空白串都视为跟随宿主", () => {
    expect(parsePersistedTerminalFontFamily(null)).toBeNull();
    expect(parsePersistedTerminalFontFamily("   ")).toBeNull();
  });

  it("设置后返回 trim 过的用户字体串", () => {
    expect(parsePersistedTerminalFontFamily("  'Fira Code', monospace ")).toBe("'Fira Code', monospace");
  });
});

describe("resolveTerminalFont", () => {
  it("未设置时整体回退宿主值", () => {
    expect(resolveTerminalFont({ fontFamily: null, fontSize: null }, HOST)).toEqual(HOST);
  });

  it("设置后用户值覆盖宿主值", () => {
    expect(resolveTerminalFont({ fontFamily: "Menlo, monospace", fontSize: 18 }, HOST)).toEqual({
      fontFamily: "Menlo, monospace",
      fontSize: 18,
    });
  });

  it("只设置单项时另一项仍跟随宿主", () => {
    expect(resolveTerminalFont({ fontFamily: "Menlo, monospace", fontSize: null }, HOST)).toEqual({
      fontFamily: "Menlo, monospace",
      fontSize: HOST.fontSize,
    });
    expect(resolveTerminalFont({ fontFamily: null, fontSize: 20 }, HOST)).toEqual({
      fontFamily: HOST.fontFamily,
      fontSize: 20,
    });
  });
});

describe("loadTerminalFontOverride / persistTerminalFont*", () => {
  it("从注入存储读取两项设置", () => {
    const storage = {
      getItem: (key: string) => (key === TERMINAL_FONT_SIZE_KEY ? "20" : key === TERMINAL_FONT_FAMILY_KEY ? "Menlo" : null),
    };
    expect(loadTerminalFontOverride(storage)).toEqual({ fontFamily: "Menlo", fontSize: 20 });
  });

  it("键缺失或存储抛错时归一为跟随宿主", () => {
    expect(loadTerminalFontOverride({ getItem: () => null })).toEqual({ fontFamily: null, fontSize: null });
    expect(
      loadTerminalFontOverride({
        getItem: () => {
          throw new Error("blocked");
        },
      }),
    ).toEqual({ fontFamily: null, fontSize: null });
  });

  it("写入用户值；null 删键（恢复跟随宿主）", () => {
    const written = new Map<string, string>();
    const removed: string[] = [];
    const storage = {
      setItem: (key: string, value: string) => void written.set(key, value),
      removeItem: (key: string) => removed.push(key),
    };
    persistTerminalFontFamily("Menlo", storage);
    persistTerminalFontSize(16, storage);
    expect(written.get(TERMINAL_FONT_FAMILY_KEY)).toBe("Menlo");
    expect(written.get(TERMINAL_FONT_SIZE_KEY)).toBe("16");
    persistTerminalFontFamily(null, storage);
    persistTerminalFontSize(null, storage);
    expect(removed).toEqual([TERMINAL_FONT_FAMILY_KEY, TERMINAL_FONT_SIZE_KEY]);
  });

  it("存储抛错时静默（仅失去持久化，不崩溃）", () => {
    const storage = {
      setItem: () => {
        throw new Error("blocked");
      },
      removeItem: () => {
        throw new Error("blocked");
      },
    };
    expect(() => persistTerminalFontFamily("Menlo", storage)).not.toThrow();
    expect(() => persistTerminalFontSize(16, storage)).not.toThrow();
  });
});
