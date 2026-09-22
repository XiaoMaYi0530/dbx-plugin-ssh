// 终端配色方案纯逻辑单测：内置目录完整性、亮暗分类、16 色展开、xterm 主题
// 合成，以及 Tabby / iTerm2 / Windows Terminal / Xresources 四种导入格式。
import { describe, expect, it } from "vitest";
import {
  ANSI_COLOR_NAMES,
  BUILTIN_TERMINAL_SCHEMES,
  builtinSchemeById,
  contrastRatio,
  filterTerminalSchemes,
  normalizeHexColor,
  parseHexColor,
  parseItermColorsScheme,
  parseSchemeImport,
  parseXresourcesScheme,
  schemeIdFromName,
  schemeToTerminalTheme,
  schemeTone,
  uniqueSchemeId,
  type TerminalColorScheme,
} from "./terminalScheme";

describe("颜色归一化", () => {
  it("接受 3/6/8 位 hex，大小写与空白宽松", () => {
    expect(normalizeHexColor("#ABC")).toBe("#aabbcc");
    expect(normalizeHexColor("  #A1B2C3 ")).toBe("#a1b2c3");
    expect(normalizeHexColor("#a1b2c380")).toBe("#a1b2c380");
  });

  it("alpha 为 ff 时压回 6 位，非法值返回 null", () => {
    expect(normalizeHexColor("#a1b2c3ff")).toBe("#a1b2c3");
    expect(normalizeHexColor("red")).toBeNull();
    expect(normalizeHexColor("#12345")).toBeNull();
    expect(normalizeHexColor(undefined)).toBeNull();
  });

  it("解析出 0-255 分量并计算对比度", () => {
    expect(parseHexColor("#ffffff")).toEqual({ r: 255, g: 255, b: 255 });
    const white = parseHexColor("#ffffff")!;
    const black = parseHexColor("#000000")!;
    expect(contrastRatio(white, black)).toBeCloseTo(21, 1);
  });
});

describe("内置目录（Tabby 迁移）", () => {
  it("覆盖 Tabby 社区全量方案并额外含 Tabby 两个内置方案", () => {
    expect(BUILTIN_TERMINAL_SCHEMES.length).toBeGreaterThan(180);
    expect(builtinSchemeById("tabby-default")?.name).toBe("Tabby Default");
    expect(builtinSchemeById("tabby-default-light")?.name).toBe("Tabby Default Light");
  });

  it("每个方案的 16 色均合法、id 唯一、来源标记为内置", () => {
    const ids = new Set<string>();
    for (const scheme of BUILTIN_TERMINAL_SCHEMES) {
      expect(ids.has(scheme.id), `duplicate id ${scheme.id}`).toBe(false);
      ids.add(scheme.id);
      expect(scheme.colors).toHaveLength(16);
      for (const color of scheme.colors) expect(normalizeHexColor(color)).not.toBeNull();
      expect(normalizeHexColor(scheme.foreground)).not.toBeNull();
      expect(normalizeHexColor(scheme.background)).not.toBeNull();
      expect(scheme.source).toBe("builtin");
    }
  });

  it("迁移值与 Tabby 上游一致（Dracula 采样）", () => {
    const dracula = builtinSchemeById("dracula")!;
    expect(dracula.foreground).toBe("#f8f8f2");
    expect(dracula.background).toBe("#1e1f29");
    expect(dracula.cursor).toBe("#bbbbbb");
    expect(dracula.colors.slice(0, 4)).toEqual(["#000000", "#ff5555", "#50fa7b", "#f1fa8c"]);
  });
});

describe("亮暗分类与筛选", () => {
  it("按背景亮度区分深底/浅底方案", () => {
    expect(schemeTone(builtinSchemeById("dracula")!)).toBe("dark");
    expect(schemeTone(builtinSchemeById("nord")!)).toBe("dark");
    expect(schemeTone(builtinSchemeById("solarized-light")!)).toBe("light");
    expect(schemeTone(builtinSchemeById("3024-day")!)).toBe("light");
  });

  it("按名称/id 关键词与亮暗筛选", () => {
    const dark = filterTerminalSchemes(BUILTIN_TERMINAL_SCHEMES, { tone: "dark" });
    const light = filterTerminalSchemes(BUILTIN_TERMINAL_SCHEMES, { tone: "light" });
    expect(dark.length + light.length).toBe(BUILTIN_TERMINAL_SCHEMES.length);
    expect(filterTerminalSchemes(BUILTIN_TERMINAL_SCHEMES, { query: "dracula" }).map((s) => s.id)).toContain("dracula");
    expect(filterTerminalSchemes(BUILTIN_TERMINAL_SCHEMES, { query: "NORD", tone: "dark" }).map((s) => s.id)).toEqual(["nord"]);
  });
});

describe("xterm 主题合成", () => {
  it("16 色按 ANSI 顺序展开，选区色缺失时按前景色合成", () => {
    const scheme: TerminalColorScheme = {
      id: "x", name: "X", foreground: "#112233", background: "#010203", cursor: "#445566",
      colors: ANSI_COLOR_NAMES.map(() => "#000000"), source: "custom",
    };
    const theme = schemeToTerminalTheme(scheme);
    expect(theme.background).toBe("#010203");
    expect(theme.foreground).toBe("#112233");
    expect(theme.cursor).toBe("#445566");
    expect(theme.cursorAccent).toBe("#010203");
    expect(theme.selectionBackground).toBe("#1122334d");
    for (const name of ANSI_COLOR_NAMES) expect(theme[name]).toBe("#000000");
  });

  it("方案自带选区色时优先使用", () => {
    const scheme = { ...builtinSchemeById("dracula")!, selectionBackground: "#44475a" };
    expect(schemeToTerminalTheme(scheme).selectionBackground).toBe("#44475a");
  });
});

describe("id 生成", () => {
  it("名称转 slug，重名追加序号", () => {
    expect(schemeIdFromName("Solarized Dark - Patched")).toBe("solarized-dark-patched");
    expect(schemeIdFromName("ayu_light")).toBe("ayu-light");
    expect(schemeIdFromName("+++")).toBe("scheme");
    expect(uniqueSchemeId("dracula", ["dracula", "dracula-2"])).toBe("dracula-3");
    expect(uniqueSchemeId("nord", ["dracula"])).toBe("nord");
  });
});

describe("Xresources 导入（Tabby 社区格式）", () => {
  const FIXTURE = [
    "!",
    "#define base00 #282828",
    "*.foreground:  #ebdbb2",
    "*.background:  base00",
    "*.cursorColor: #ebdbb2",
    ...Array.from({ length: 16 }, (_, index) => `*.color${index}: #12345${index % 10}`),
  ].join("\n");

  it("解析前景/背景/光标与 16 色，并做 #define 变量替换", () => {
    const parsed = parseXresourcesScheme(FIXTURE, "Gruvbox");
    expect(parsed).not.toBeNull();
    expect(parsed!.name).toBe("Gruvbox");
    expect(parsed!.foreground).toBe("#ebdbb2");
    expect(parsed!.background).toBe("#282828");
    expect(parsed!.cursor).toBe("#ebdbb2");
    expect(parsed!.colors).toHaveLength(16);
    expect(parsed!.colors[0]).toBe("#123450");
  });

  it("ANSI 色不足 16 个时拒绝（不产生半残方案）", () => {
    expect(parseXresourcesScheme("*.foreground: #fff\n*.background: #000\n*.color0: #111111", "Broken")).toBeNull();
  });
});

describe("iTerm2 .itermcolors 导入", () => {
  const plist = (red: string, green: string, blue: string) => [
    "<dict>",
    `<key>Red Component</key><real>${red}</real>`,
    `<key>Green Component</key><real>${green}</real>`,
    `<key>Blue Component</key><real>${blue}</real>`,
    "</dict>",
  ].join("");

  const FIXTURE = [
    "<plist version=\"1.0\"><dict>",
    ...Array.from({ length: 16 }, (_, index) => `<key>Ansi ${index} Color</key>${plist("0.5", "0", String(index / 16))}`),
    `<key>Foreground Color</key>${plist("1", "1", "1")}`,
    `<key>Background Color</key>${plist("0", "0", "0")}`,
    `<key>Cursor Color</key>${plist("1", "0", "0")}`,
    `<key>Selection Color</key>${plist("0.2", "0.2", "0.3")}`,
    "</dict></plist>",
  ].join("");

  it("浮点分量转 8 位 hex，并保留选区色", () => {
    const parsed = parseItermColorsScheme(FIXTURE, "My iTerm");
    expect(parsed).not.toBeNull();
    expect(parsed!.name).toBe("My iTerm");
    expect(parsed!.foreground).toBe("#ffffff");
    expect(parsed!.background).toBe("#000000");
    expect(parsed!.cursor).toBe("#ff0000");
    expect(parsed!.selectionBackground).toBe("#33334d");
    expect(parsed!.colors[1]).toBe("#800010");
  });
});

describe("统一导入入口", () => {
  const tabbyJson = JSON.stringify({
    name: "Handmade",
    foreground: "#abcdef",
    background: "#101010",
    cursor: "#abcdef",
    colors: ANSI_COLOR_NAMES.map((_, index) => `#00000${index.toString(16)}`),
  });

  it("识别 Tabby 单方案 JSON", () => {
    const result = parseSchemeImport(tabbyJson);
    expect(result.format).toBe("tabby-json");
    expect(result.schemes).toHaveLength(1);
    expect(result.schemes[0].name).toBe("Handmade");
  });

  it("识别 Windows Terminal JSON（含 selectionBackground）", () => {
    const windowsTerminal = JSON.stringify({
      name: "Campbell",
      black: "#0c0c0c", red: "#c50f1f", green: "#13a10e", yellow: "#c19c00",
      blue: "#0037da", magenta: "#881798", cyan: "#3a96dd", white: "#cccccc",
      brightBlack: "#767676", brightRed: "#e74856", brightGreen: "#16c60c",
      brightYellow: "#f9f1a5", brightBlue: "#3b78ff", brightMagenta: "#b4009e",
      brightCyan: "#61d6d6", brightWhite: "#f2f2f2",
      background: "#0c0c0c", foreground: "#cccccc", cursorColor: "#ffffff",
      selectionBackground: "#ffffff",
    });
    const result = parseSchemeImport(windowsTerminal);
    expect(result.format).toBe("tabby-json");
    expect(result.schemes[0].colors[15]).toBe("#f2f2f2");
    expect(result.schemes[0].selectionBackground).toBe("#ffffff");
  });

  it("识别 Tabby config.yaml 的 colorScheme / lightColorScheme 块", () => {
    const yaml = [
      "terminal:",
      "  fontSize: 14,",
      "  colorScheme:",
      "    name: 'My Dark'",
      "    background: '#282C34'",
      "    foreground: '#ABB2BF'",
      "    cursor: '#528BFF'",
      "    colors:",
      ...ANSI_COLOR_NAMES.map((_, index) => `      - '#12345${index.toString(16)}'`),
      "  lightColorScheme:",
      "    name: 'My Light'",
      "    background: '#FFFFFF'",
      "    foreground: '#101010'",
      `    colors: [${ANSI_COLOR_NAMES.map((_, index) => `'#1234${index.toString(16)}0'`).join(", ")}]`,
      "  other: value",
    ].join("\n");
    const result = parseSchemeImport(yaml);
    expect(result.format).toBe("tabby-yaml");
    expect(result.schemes.map((scheme) => scheme.name)).toEqual(["My Dark", "My Light"]);
    expect(result.schemes[0].background).toBe("#282c34");
    expect(result.schemes[1].cursor).toBe("#101010");
  });

  it("无法识别时返回 unknown 而非抛错", () => {
    expect(parseSchemeImport("hello world")).toEqual({ schemes: [], format: "unknown" });
    expect(parseSchemeImport("   ")).toEqual({ schemes: [], format: "unknown" });
  });
});
