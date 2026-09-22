// 终端配色方案（对标 Tabby 的 terminal.colorScheme 模型）：类型、内置目录索引、
// 亮暗分类、16 色 ANSI 展开、xterm 主题合成，以及外部方案导入解析。
//
// Tabby 的方案模型（tabby-core `TerminalColorScheme`）：
//   { name, foreground, background, cursor, colors[16] }
// colors 按 ANSI SGR 顺序排列（0-7 常规色，8-15 亮色）。本模块保持同构，
// 以便 Tabby / iTerm2 / Windows Terminal 方案能直接迁移，无需转换表。
//
// 纯逻辑模块：不依赖 DOM、Vue 或存储，便于单测覆盖解析与合成。

import { TABBY_BUILTIN_SCHEMES } from "./terminalSchemeCatalog";

/** 配色方案来源：内置目录（Tabby 迁移）或用户导入。 */
export type TerminalSchemeSource = "builtin" | "custom";

export interface TerminalColorScheme {
  /** 稳定 id（内置为目录元组第 0 位；自定义导入时生成）。 */
  id: string;
  /** 展示名：内置保留 Tabby 原方案名。 */
  name: string;
  foreground: string;
  background: string;
  cursor: string;
  /** ANSI SGR 0-15：black…white, brightBlack…brightWhite（16 位十六进制色）。 */
  colors: readonly string[];
  /** 可选选区底色（iTerm2 的 Selection Color / Windows Terminal 的 selectionBackground）。 */
  selectionBackground?: string;
  source: TerminalSchemeSource;
}

/** 目录元组布局：[id, name, foreground, background, cursor, colors(16)]。 */
export type TerminalSchemeTuple = readonly [string, string, string, string, string, readonly string[]];

/** ANSI 索引 → 名称（用于色卡的无障碍标签与调试）。 */
export const ANSI_COLOR_NAMES = [
  "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
  "brightBlack", "brightRed", "brightGreen", "brightYellow", "brightBlue",
  "brightMagenta", "brightCyan", "brightWhite",
] as const;

const HEX_LONG = /^#[0-9a-f]{6}$/;
const HEX_SHORT = /^#[0-9a-f]{3}$/;
const HEX_ALPHA = /^#[0-9a-f]{8}$/;

/** 归一化颜色串：接受 #rgb/#rrggbb/#rrggbbaa（大小写与空白宽松），否则返回 null。 */
export function normalizeHexColor(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim().toLowerCase();
  if (HEX_LONG.test(trimmed)) return trimmed;
  if (HEX_ALPHA.test(trimmed)) return trimmed === `${trimmed.slice(0, 7)}ff` ? trimmed.slice(0, 7) : trimmed;
  if (HEX_SHORT.test(trimmed)) {
    const [, r, g, b] = trimmed;
    return `#${r}${r}${g}${g}${b}${b}`;
  }
  return null;
}

export interface RgbColor { r: number; g: number; b: number }

/** 解析 hex 为 0-255 分量；非法返回 null（8 位色忽略 alpha）。 */
export function parseHexColor(value: unknown): RgbColor | null {
  const normalized = normalizeHexColor(value);
  if (!normalized) return null;
  return {
    r: Number.parseInt(normalized.slice(1, 3), 16),
    g: Number.parseInt(normalized.slice(3, 5), 16),
    b: Number.parseInt(normalized.slice(5, 7), 16),
  };
}

// 相对亮度（WCAG 2.x 的 sRGB 线性化）：用于亮暗分类与背景对比度提示。
function channelLuminance(channel: number): number {
  const srgb = channel / 255;
  return srgb <= 0.04045 ? srgb / 12.92 : ((srgb + 0.055) / 1.055) ** 2.4;
}

export function relativeLuminance(color: RgbColor): number {
  return 0.2126 * channelLuminance(color.r) + 0.7152 * channelLuminance(color.g) + 0.0722 * channelLuminance(color.b);
}

/** 前景/背景对比度（WCAG），用于给出「可读性偏低」提示。 */
export function contrastRatio(a: RgbColor, b: RgbColor): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  const [hi, lo] = la >= lb ? [la, lb] : [lb, la];
  return (hi + 0.05) / (lo + 0.05);
}

/**
 * 亮暗分类：以背景相对亮度 0.5 为界。Tabby 不做分类（两槽由用户自己挂），
 * 插件需要把 192 个方案按亮色/暗色分组呈现，故用背景亮度推断，
 * 阈值取 0.5 可正确区分 3024 Day / Solarized Light 等浅底方案。
 */
export function schemeTone(scheme: TerminalColorScheme): "dark" | "light" {
  const background = parseHexColor(scheme.background);
  return background && relativeLuminance(background) > 0.5 ? "light" : "dark";
}

function fromTuple(tuple: TerminalSchemeTuple): TerminalColorScheme {
  return {
    id: tuple[0],
    name: tuple[1],
    foreground: tuple[2],
    background: tuple[3],
    cursor: tuple[4],
    colors: [...tuple[5]],
    source: "builtin",
  };
}

/** 内置目录（Tabby 迁移，192 个方案）：首次访问时展开并冻结。 */
export const BUILTIN_TERMINAL_SCHEMES: readonly TerminalColorScheme[] = TABBY_BUILTIN_SCHEMES.map(fromTuple);

const BUILTIN_SCHEME_INDEX = new Map(BUILTIN_TERMINAL_SCHEMES.map((scheme) => [scheme.id, scheme]));

/** 按 id 取内置方案；未知 id（方案目录演进后被删除）返回 undefined。 */
export function builtinSchemeById(id: string | null | undefined): TerminalColorScheme | undefined {
  return id ? BUILTIN_SCHEME_INDEX.get(id) : undefined;
}

/** 名称 → 稳定 id（导入自定义方案时复用同一套 slug 规则）。 */
export function schemeIdFromName(name: string): string {
  const slug = name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return slug.length > 0 ? slug : "scheme";
}

/** 在既有 id 集合中分配唯一 id（同名重复导入时追加序号，不覆盖旧方案）。 */
export function uniqueSchemeId(base: string, taken: Iterable<string>): string {
  const used = new Set(taken);
  if (!used.has(base)) return base;
  let suffix = 2;
  while (used.has(`${base}-${suffix}`)) suffix += 1;
  return `${base}-${suffix}`;
}

/** 按名称/亮暗/来源过滤（设置页搜索框）；查询为空即全量。 */
export function filterTerminalSchemes(
  schemes: readonly TerminalColorScheme[],
  options: { query?: string; tone?: "dark" | "light" | "all" } = {},
): TerminalColorScheme[] {
  const query = (options.query ?? "").trim().toLowerCase();
  const tone = options.tone ?? "all";
  return schemes.filter((scheme) => {
    if (tone !== "all" && schemeTone(scheme) !== tone) return false;
    if (!query) return true;
    return scheme.name.toLowerCase().includes(query) || scheme.id.includes(query);
  });
}

/** 16 色 ANSI 展开为 xterm 主题字段（black…brightWhite）。 */
export function ansiThemeFields(scheme: TerminalColorScheme): Record<string, string> {
  const fields: Record<string, string> = {};
  scheme.colors.slice(0, 16).forEach((color, index) => {
    const name = ANSI_COLOR_NAMES[index];
    if (name) fields[name] = color;
  });
  return fields;
}

export interface TerminalThemeLike {
  background: string;
  foreground: string;
  cursor: string;
  cursorAccent: string;
  selectionBackground: string;
  selectionForeground?: string;
  [key: string]: string | undefined;
}

/**
 * 方案 → xterm ITheme（完整 16 色 + 前景/背景/光标/选区）。
 *
 * `selectionBackground` 优先用方案自带值（iTerm2/WT 可提供）；缺失时按方案
 * 前景色合成 30% 透明覆盖——直接复用宿主的固定选区色在浅底方案上会糊掉。
 * `backgroundSource: "host"` 时调用方改用宿主面板色覆盖前景/背景（Tabby 的
 * `background: 'theme'` 语义），ANSI 16 色仍取自方案。
 */
export function schemeToTerminalTheme(scheme: TerminalColorScheme): TerminalThemeLike {
  const foreground = parseHexColor(scheme.foreground);
  const fallbackSelection = foreground
    ? `#${[foreground.r, foreground.g, foreground.b].map((channel) => channel.toString(16).padStart(2, "0")).join("")}4d`
    : "#8080804d";
  return {
    background: scheme.background,
    foreground: scheme.foreground,
    cursor: scheme.cursor,
    cursorAccent: scheme.background,
    selectionBackground: normalizeHexColor(scheme.selectionBackground) ?? fallbackSelection,
    ...ansiThemeFields(scheme),
  };
}

// ---------------------------------------------------------------------------
// 方案导入（Tabby / iTerm2 / Windows Terminal / Xresources）
// ---------------------------------------------------------------------------

/** 导入方案：id 与来源标记由调用方（偏好层）分配。 */
export type ParsedSchemeImport = Omit<TerminalColorScheme, "id" | "source">;

function toScheme(
  name: string,
  foreground: unknown,
  background: unknown,
  cursor: unknown,
  colors: readonly unknown[],
  selection?: unknown,
): ParsedSchemeImport | null {
  const fg = normalizeHexColor(foreground);
  const bg = normalizeHexColor(background);
  if (!fg || !bg) return null;
  const normalizedColors = colors.slice(0, 16).map(normalizeHexColor);
  if (normalizedColors.length < 16 || normalizedColors.some((color) => color === null)) return null;
  return {
    name: name.trim().length > 0 ? name.trim() : "Imported scheme",
    foreground: fg,
    background: bg,
    cursor: normalizeHexColor(cursor) ?? fg,
    colors: normalizedColors as string[],
    selectionBackground: normalizeHexColor(selection) ?? undefined,
  };
}

/** Xresources（Tabby 社区方案格式）：`#define` 变量 + `*.key: value` 赋值。 */
export function parseXresourcesScheme(text: string, fallbackName = "Imported scheme"): ParsedSchemeImport | null {
  if (!/^\s*\*\./m.test(text)) return null;
  const variables = new Map<string, string>();
  for (const line of text.split("\n")) {
    if (!line.startsWith("#define")) continue;
    const parts = line.split(" ").map((part) => part.trim());
    if (parts.length >= 3 && parts[1]) variables.set(parts[1], parts[2]);
  }
  const values = new Map<string, string>();
  for (const line of text.split("\n")) {
    if (!line.startsWith("*.")) continue;
    const body = line.slice(2);
    const separator = body.indexOf(":");
    if (separator < 0) continue;
    const key = body.slice(0, separator).trim();
    const raw = body.slice(separator + 1).trim();
    // 与 Tabby 同款变量替换（`*.background: $bg` 这类引用）。
    values.set(key, variables.get(raw) ?? raw);
  }
  const colors: string[] = [];
  for (let index = 0; index < 16; index += 1) {
    const value = values.get(`color${index}`);
    if (value === undefined) break;
    colors.push(value);
  }
  // Xresources 无名称字段：一律用调用方给的兜底名（文件名或用户输入）。
  return toScheme(
    fallbackName,
    values.get("foreground"),
    values.get("background"),
    values.get("cursorColor") ?? values.get("cursor"),
    colors,
    values.get("selectionBackground"),
  );
}

// iTerm2 .itermcolors：plist XML，颜色以 0-1 浮点分量给出。
function componentToByte(raw: string): number {
  const value = Number.parseFloat(raw);
  if (!Number.isFinite(value)) return 0;
  return Math.max(0, Math.min(255, Math.round(value * 255)));
}

function floatTripletToHex(red: string, green: string, blue: string): string {
  return `#${[red, green, blue].map((raw) => componentToByte(raw).toString(16).padStart(2, "0")).join("")}`;
}

export function parseItermColorsScheme(text: string, fallbackName = "Imported scheme"): ParsedSchemeImport | null {
  if (!/<key>\s*(?:Ansi|Background)\s/i.test(text)) return null;
  const entries = new Map<string, string>();
  const blockPattern = /<key>([^<]+)<\/key>\s*<dict>([\s\S]*?)<\/dict>/g;
  for (const match of text.matchAll(blockPattern)) {
    const key = match[1].trim();
    const body = match[2];
    const component = (name: string) => {
      const found = body.match(new RegExp(`<key>\\s*${name}\\s*</key>\\s*<real>([^<]+)</real>`));
      return found ? found[1] : "0";
    };
    entries.set(key, floatTripletToHex(component("Red Component"), component("Green Component"), component("Blue Component")));
  }
  const colors: string[] = [];
  for (let index = 0; index < 16; index += 1) {
    const value = entries.get(`Ansi ${index} Color`);
    if (!value) break;
    colors.push(value);
  }
  return toScheme(
    fallbackName,
    entries.get("Foreground Color"),
    entries.get("Background Color"),
    entries.get("Cursor Color") ?? entries.get("Foreground Color"),
    colors,
    entries.get("Selection Color"),
  );
}

const WINDOWS_TERMINAL_KEYS = [
  "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
  "brightBlack", "brightRed", "brightGreen", "brightYellow", "brightBlue",
  "brightMagenta", "brightCyan", "brightWhite",
] as const;

function fromJsonObject(value: unknown, fallbackName: string): ParsedSchemeImport | null {
  if (!value || typeof value !== "object") return null;
  const record = value as Record<string, unknown>;
  const colors = WINDOWS_TERMINAL_KEYS.map((key) => record[key]);
  const name = typeof record.name === "string" && record.name.trim() ? record.name : fallbackName;
  return toScheme(
    name,
    record.foreground,
    record.background,
    record.cursorColor ?? record.cursor,
    colors,
    record.selectionBackground,
  );
}

function toStringArray(value: unknown): string[] {
  if (Array.isArray(value)) return value.filter((item): item is string => typeof item === "string");
  if (typeof value === "string") return [value];
  return [];
}

export interface SchemeImportResult {
  schemes: ParsedSchemeImport[];
  /** 识别到的格式（用于 UI 反馈）。 */
  format: "xresources" | "iterm2" | "windows-terminal" | "tabby-json" | "tabby-yaml" | "unknown";
}

function parseJsonImport(text: string): SchemeImportResult | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return null;
  }
  const schemes: ParsedSchemeImport[] = [];
  const collect = (value: unknown, fallbackName: string) => {
    const scheme = fromJsonObject(value, fallbackName);
    if (scheme) schemes.push(scheme);
  };
  if (Array.isArray(parsed)) {
    parsed.forEach((item, index) => collect(item, `Imported scheme ${index + 1}`));
  } else {
    const record = parsed as Record<string, unknown>;
    const terminal = (record.terminal ?? {}) as Record<string, unknown>;
    if (terminal.colorScheme) collect(terminal.colorScheme, "Tabby dark scheme");
    if (terminal.lightColorScheme) collect(terminal.lightColorScheme, "Tabby light scheme");
    for (const item of Array.isArray(terminal.customColorSchemes) ? terminal.customColorSchemes : []) {
      collect(item, "Tabby custom scheme");
    }
    if (schemes.length === 0) {
      // 单方案 JSON：Tabby 的 `{name, foreground, background, colors:[…]}` 形态。
      if (Array.isArray(record.colors)) {
        const colors = toStringArray(record.colors);
        const built = toScheme(
          typeof record.name === "string" ? record.name : "Imported scheme",
          record.foreground,
          record.background,
          record.cursor,
          colors,
          record.selection,
        );
        if (built) schemes.push(built);
      } else if (fromJsonObject(parsed, "Imported scheme")) {
        collect(parsed, "Imported scheme");
      }
    }
  }
  return schemes.length > 0 ? { schemes, format: "tabby-json" } : null;
}

/**
 * Tabby config.yaml 片段的最小解析：仅识别 `colorScheme:` / `lightColorScheme:`
 * 两个映射块（YAML 缩进扫描），块内接受 `key: value` 与 `colors:` 列表项。
 *
 * 刻意不引入 YAML 依赖（仓库约定：不新增运行时依赖）；`customColorSchemes`
 * 这类列表套映射的结构不支持，需用 JSON 形态导入。
 */
export function parseTabbyYamlSchemes(text: string): ParsedSchemeImport[] {
  const lines = text.split("\n");
  const results: ParsedSchemeImport[] = [];
  const blockPattern = /^(\s*)(colorScheme|lightColorScheme)\s*:\s*$/;
  for (let index = 0; index < lines.length; index += 1) {
    const match = lines[index].match(blockPattern);
    if (!match) continue;
    const indent = match[1].length;
    const key = match[2];
    const fields = new Map<string, string>();
    const colors: string[] = [];
    let inColors = false;
    for (let cursor = index + 1; cursor < lines.length; cursor += 1) {
      const line = lines[cursor];
      if (line.trim().length === 0) continue;
      const lineIndent = line.length - line.trimStart().length;
      if (lineIndent <= indent) break;
      const trimmed = line.trim();
      if (/^colors\s*:/.test(trimmed)) {
        inColors = true;
        const inline = trimmed.replace(/^colors\s*:\s*/, "").replace(/^\[|\]$/g, "").trim();
        // 列表项在本格式里一律带引号（YAML 标量），剥掉后再交给色值归一化。
        if (inline) colors.push(...inline.split(",").map((item) => unquoteYamlScalar(item)).filter(Boolean));
        continue;
      }
      if (inColors && trimmed.startsWith("-")) {
        colors.push(unquoteYamlScalar(trimmed.slice(1)));
        continue;
      }
      inColors = false;
      const separator = trimmed.indexOf(":");
      if (separator < 0) continue;
      const fieldKey = trimmed.slice(0, separator).trim();
      const raw = trimmed.slice(separator + 1).trim();
      fields.set(fieldKey, unquoteYamlScalar(raw));
    }
    const built = toScheme(
      fields.get("name") ?? (key === "lightColorScheme" ? "Tabby light scheme" : "Tabby dark scheme"),
      fields.get("foreground"),
      fields.get("background"),
      fields.get("cursor"),
      colors,
      fields.get("selection"),
    );
    if (built) results.push(built);
  }
  return results;
}

function unquoteYamlScalar(raw: string): string {
  const trimmed = raw.trim();
  if ((trimmed.startsWith("'") && trimmed.endsWith("'")) || (trimmed.startsWith('"') && trimmed.endsWith('"'))) {
    return trimmed.slice(1, -1);
  }
  return trimmed;
}

/**
 * 统一导入入口：按内容自动识别格式（JSON / iTerm2 plist / Xresources / Tabby YAML）。
 * 返回空数组表示无法识别——调用方给七语错误提示，不静默吞掉。
 */
export function parseSchemeImport(text: string, fallbackName = "Imported scheme"): SchemeImportResult {
  const trimmed = text.trim();
  if (trimmed.length === 0) return { schemes: [], format: "unknown" };
  if (trimmed.startsWith("{") || trimmed.startsWith("[")) {
    const json = parseJsonImport(trimmed);
    if (json) return json;
  }
  if (/<key>|<plist/i.test(trimmed)) {
    const iterm = parseItermColorsScheme(trimmed, fallbackName);
    if (iterm) return { schemes: [iterm], format: "iterm2" };
  }
  if (/^\s*\*\./m.test(trimmed)) {
    const xresources = parseXresourcesScheme(trimmed, fallbackName);
    if (xresources) return { schemes: [xresources], format: "xresources" };
  }
  const yaml = parseTabbyYamlSchemes(trimmed);
  if (yaml.length > 0) return { schemes: yaml, format: "tabby-yaml" };
  return { schemes: [], format: "unknown" };
}
