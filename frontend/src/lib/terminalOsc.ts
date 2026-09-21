// 终端 OSC 序列应答（对标 electerm 的 terminal-color-query / osc52-addon）：
// - OSC 10/11（前景/背景色查询）：vim/tmux/neovim 启动时会查询终端真实颜色来
//   定调色板；xterm 内核不回应这类查询，程序只能假设黑底白字，浅色主题下配色
//   错乱。这里按当前主题色补答 `?` 查询（`rgb:rr/gg/bb`）；颜色"设置"分支
//   （data 为具体颜色）不拦截，交回 xterm 内核默认处理（改主题内建色）。
// - OSC 52（远程写剪贴板）：TUI 程序（vim/tmux/yazi 等）经转义序列写系统剪贴
//   板。只实现写入（目标 c）；读取查询（`?`）静默吞掉——把用户剪贴板回传给远
//   端主机属于隐私泄漏，主流终端同样默认不回应。

export interface OscQueryTerminal {
  /** 回写响应字节流（wasUserInput=false：应答不当作键盘敲击回显）。 */
  input(data: string, wasUserInput: boolean): void;
}

const OSC_PREFIX = "\x1b]";
const OSC_SUFFIX = "\x1b\\";

// OSC 52 payload 的防御上限：正常 TUI 复制远小于此，超限视为异常按不支持丢弃。
const OSC52_MAX_PAYLOAD = 1 << 20;

function hexByte(value: number): string {
  return value.toString(16).padStart(2, "0");
}

function clampByte(value: number): number {
  return Math.max(0, Math.min(255, Math.round(value)));
}

function parseChannel(value: string): number | null {
  const trimmed = value.trim();
  if (!trimmed) return null;
  if (trimmed.endsWith("%")) {
    const percent = Number(trimmed.slice(0, -1));
    return Number.isFinite(percent) ? clampByte((percent * 255) / 100) : null;
  }
  const num = Number(trimmed);
  return Number.isFinite(num) ? clampByte(num) : null;
}

function parseAlpha(value: string | undefined): number | null {
  if (value === undefined) return 1;
  const trimmed = value.trim();
  if (!trimmed) return 1;
  if (trimmed.endsWith("%")) {
    const percent = Number(trimmed.slice(0, -1));
    return Number.isFinite(percent) ? percent / 100 : null;
  }
  const num = Number(trimmed);
  return Number.isFinite(num) ? num : null;
}

function parseHexColor(color: string): [number, number, number] | null {
  const match = color.match(/^#([0-9a-f]{3}|[0-9a-f]{4}|[0-9a-f]{6}|[0-9a-f]{8})$/i);
  if (!match) return null;
  const raw = match[1] ?? "";
  const expanded = raw.length <= 4 ? raw.split("").map((char) => char + char).join("") : raw;
  // 4/8 位带 alpha：全透明在终端背景语义下等于"没有颜色"，与 electerm 一致拒答。
  if (expanded.length === 8) {
    const alpha = parseInt(expanded.slice(6, 8), 16) / 255;
    if (alpha <= 0) return null;
  }
  return [parseInt(expanded.slice(0, 2), 16), parseInt(expanded.slice(2, 4), 16), parseInt(expanded.slice(4, 6), 16)];
}

function parseRgbColor(color: string): [number, number, number] | null {
  const match = color.match(/^rgba?\((.*)\)$/i);
  if (!match) return null;
  const [channelsPart, slashAlpha] = (match[1] ?? "").split("/").map((part) => part.trim());
  const parts = (channelsPart ?? "").includes(",")
    ? (channelsPart ?? "").split(",").map((part) => part.trim())
    : (channelsPart ?? "").split(/\s+/).filter(Boolean);
  if (parts.length < 3) return null;
  const channels = parts.slice(0, 3).map(parseChannel);
  if (channels.some((channel) => channel === null)) return null;
  const alpha = parseAlpha(slashAlpha || parts[3]);
  if (alpha === null || alpha <= 0) return null;
  return [channels[0] ?? 0, channels[1] ?? 0, channels[2] ?? 0];
}

/** CSS 颜色串 → 8bit RGB；解析失败（含 transparent/全透明）返回 null。 */
export function parseCssColorToRgb(color: string): [number, number, number] | null {
  if (typeof color !== "string") return null;
  const trimmed = color.trim();
  if (!trimmed || trimmed.toLowerCase() === "transparent") return null;
  return parseHexColor(trimmed) ?? parseRgbColor(trimmed);
}

/** xterm 颜色查询应答格式：rgb:rr/gg/bb（各通道独立 8bit 十六进制）。 */
export function colorToOscRgb(color: string): string {
  const rgb = parseCssColorToRgb(color);
  return rgb ? `rgb:${rgb.map(hexByte).join("/")}` : "";
}

/**
 * 回应 OSC 10/11 的 `?` 查询；非查询（设置颜色）返回 false 交回内核默认处理。
 * fallbackColor 覆盖主题色解析失败的极端情况（宿主下发非法颜色值）。
 */
export function handleTerminalColorQuery(
  terminal: OscQueryTerminal,
  identifier: number,
  color: string,
  fallbackColor: string,
  data: string,
): boolean {
  if (typeof data !== "string" || data.trim() !== "?") return false;
  const oscColor = colorToOscRgb(color) || colorToOscRgb(fallbackColor);
  if (!oscColor) return false;
  terminal.input(`${OSC_PREFIX}${identifier};${oscColor}${OSC_SUFFIX}`, false);
  return true;
}

function decodeUtf8Base64(payload: string): string | null {
  try {
    const binary = atob(payload.replace(/\s+/g, ""));
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
    return new TextDecoder("utf-8").decode(bytes);
  } catch {
    return null;
  }
}

/**
 * OSC 52 写入路径：`52;c;<base64>` → 解码后写系统剪贴板，返回 true（已处理）。
 * 目标不含 c（primary/secondary）返回 false 交回内核（无默认处理=忽略）；
 * 读取查询（`?`）与清空语义返回 true 且不动作（不回应、不清空）。
 */
export function handleOsc52ClipboardWrite(data: string, writeText: (text: string) => unknown): boolean {
  if (!data) return false;
  const separator = data.indexOf(";");
  if (separator === -1) return false;
  const target = data.slice(0, separator);
  const payload = data.slice(separator + 1);
  if (!target.includes("c")) return false;
  if (payload === "?") return true;
  if (!payload || payload.length > OSC52_MAX_PAYLOAD) return true;
  const text = decodeUtf8Base64(payload);
  if (text === null) return true;
  void writeText(text);
  return true;
}
