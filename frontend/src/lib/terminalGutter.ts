// 终端行号/时间戳 gutter（P1-3）：纯函数 + xterm 私有渲染尺寸访问封装。
// 零 DOM 依赖：computeGutterRows 只吃"最小 buffer 视图"（type/length/getLine），
// 由 App.vue 用真实 terminal.buffer.active 传入；gutter 的 DOM 呈现与 rAF 节流
// 在 TerminalGutter.vue，时间戳采集（写入完成盖戳 + 回车重盖）留在 App.vue。
// xterm 的渲染单元格高度在私有 API `_core._renderService.dimensions` 上，读不到
// 时整体降级隐藏 gutter（不报错、不影响终端本体）。

/** 时间戳格式的默认值（设置 `terminal_timestamp_format`）。 */
export const GUTTER_TIMESTAMP_DEFAULT_FORMAT = "[HH:mm:ss]";
/** 格式串上限：防手改 JSON 把 preferences 撑爆（与后端 sanitize 同界）。 */
export const GUTTER_TIMESTAMP_FORMAT_MAX = 64;
/** 时间戳 Map 的保留窗：视口顶端之前再留 3000 行，更早的条目裁掉。 */
export const GUTTER_TIMESTAMP_RETENTION_ROWS = 3000;

/** gutter 需要的 buffer 最小视图（真实 IBuffer 结构兼容）。 */
export interface GutterBufferLike {
  readonly type?: string;
  readonly length: number;
  getLine(y: number): { isWrapped: boolean } | undefined;
}

/** 一个 gutter 行：相对终端画布顶端的像素偏移 + 要显示的文本。 */
export interface GutterRow {
  top: number;
  text: string;
}

export interface ComputeGutterRowsInput {
  /** 渲染单元格高（CSS px）；null = 读不到私有渲染尺寸，调用方隐藏 gutter。 */
  cellHeight: number | null;
  /** 首个视口行的像素起点（xterm 画布内边距等引入的顶部偏移）。 */
  screenOffsetTop: number;
  /** 视口顶端的缓冲绝对行号（buffer.viewportY）。 */
  scrollTop: number;
  buffer: GutterBufferLike;
  /** 视口行数（terminal.rows）。 */
  rows: number;
  /** 逻辑行首行绝对行号 → 写入时刻（epoch ms）。 */
  timestamps?: ReadonlyMap<number, number>;
  showLineNumbers: boolean;
  showTimestamps: boolean;
  timestampFormat?: string;
}

/**
 * 时间戳格式化：支持 YYYY / YY / MM / DD / HH / mm / ss / SSS 令牌，
 * 其余字符原样保留。ms 非法返回空串；空格式回退默认值。
 * 注意令牌按大小写区分（MM=月 / mm=分），交替顺序 YYYY 必须在 YY 之前。
 */
const TIMESTAMP_TOKEN_SOURCE = /(YYYY|SSS|YY|MM|DD|HH|mm|ss)/g;

export function formatTimestamp(ms: number, format: string = GUTTER_TIMESTAMP_DEFAULT_FORMAT): string {
  if (!Number.isFinite(ms)) return "";
  const pattern = format || GUTTER_TIMESTAMP_DEFAULT_FORMAT;
  const date = new Date(ms);
  const pad = (value: number, width: number) => String(value).padStart(width, "0");
  return pattern.replace(TIMESTAMP_TOKEN_SOURCE, (token: string) => {
    switch (token) {
      case "YYYY": return String(date.getFullYear());
      case "YY": return pad(date.getFullYear() % 100, 2);
      case "MM": return pad(date.getMonth() + 1, 2);
      case "DD": return pad(date.getDate(), 2);
      case "HH": return pad(date.getHours(), 2);
      case "mm": return pad(date.getMinutes(), 2);
      case "ss": return pad(date.getSeconds(), 2);
      case "SSS": return pad(date.getMilliseconds(), 3);
      default: return token;
    }
  });
}

/**
 * 计算 gutter 各行：
 * - alternate buffer（vim/htop 全屏应用）返回空——行号/时间戳在应用自绘画面
 *   上没有稳定语义；
 * - wrapped 行（上一逻辑行的折行延续）跳过——时间戳与行号只标逻辑行首；
 * - 像素 top = screenOffsetTop + 视口槽位 × cellHeight，与跳行无关
 *   （gutter 必须与终端文本逐行对齐，跳过的槽位留白）；
 * - 行号取逻辑行首的物理行号（1-based）：O(1) 且随缓冲稳定，wrapped 块只计一次；
 * - 滚出缓冲末尾的槽位截断；scrollTop 越界钳到 [0, length-1]。
 */
export function computeGutterRows(input: ComputeGutterRowsInput): GutterRow[] {
  const { cellHeight, screenOffsetTop, scrollTop, buffer, rows, timestamps, showLineNumbers, showTimestamps, timestampFormat } = input;
  if (buffer.type === "alternate") return [];
  if (cellHeight === null || !Number.isFinite(cellHeight) || cellHeight <= 0) return [];
  if (rows <= 0 || buffer.length <= 0) return [];
  if (!showLineNumbers && !showTimestamps) return [];
  const start = Math.max(0, Math.min(Math.trunc(scrollTop), buffer.length - 1));
  const out: GutterRow[] = [];
  for (let slot = 0; slot < rows; slot++) {
    const absoluteRow = start + slot;
    if (absoluteRow >= buffer.length) break;
    const line = buffer.getLine(absoluteRow);
    if (!line || line.isWrapped) continue;
    const parts: string[] = [];
    if (showLineNumbers) parts.push(String(absoluteRow + 1));
    if (showTimestamps) {
      const writtenAt = timestamps?.get(absoluteRow);
      parts.push(writtenAt === undefined ? "" : formatTimestamp(writtenAt, timestampFormat));
    }
    out.push({ top: screenOffsetTop + slot * cellHeight, text: parts.join(" ").trimEnd() });
  }
  return out;
}

/**
 * 时间戳 Map 裁剪：删除视口顶端 `GUTTER_TIMESTAMP_RETENTION_ROWS` 行之前的
 * 条目。原地修改并返回同一 Map（App.vue 持有权威 Map，避免每次重建）。
 */
export function trimTimestampMap(map: Map<number, number>, minRow: number): Map<number, number> {
  for (const key of map.keys()) {
    if (key < minRow) map.delete(key);
  }
  return map;
}

/**
 * 读 xterm 私有渲染尺寸里的单元格高（CSS px）。读不到（渲染器未就绪、
 * WebGL 恢复中、上游结构变化）返回 null——调用方据此隐藏 gutter 降级，
 * 绝不抛错。入参收 `unknown` 以便单测用假对象直接驱动。
 */
type RenderDimensionsCarrier = {
  _core?: {
    _renderService?: {
      dimensions?: {
        css?: {
          cell?: { height?: unknown };
        };
      };
    };
  };
};

export function getRenderCellHeight(terminal: unknown): number | null {
  const height = (terminal as RenderDimensionsCarrier | undefined | null)?._core?._renderService?.dimensions?.css?.cell?.height;
  return typeof height === "number" && Number.isFinite(height) && height > 0 ? height : null;
}

/** gutter 设置（`terminal_show_line_numbers` 等）的运行时形状。 */
export interface GutterSettings {
  showLineNumbers: boolean;
  showTimestamps: boolean;
  timestampFormat: string;
}

/** 格式串收紧：白名单字符 + 长度截断；清洗后为空回退默认格式。 */
function sanitizeTimestampFormat(value: unknown): string {
  if (typeof value !== "string") return GUTTER_TIMESTAMP_DEFAULT_FORMAT;
  const cleaned = Array.from(value.slice(0, GUTTER_TIMESTAMP_FORMAT_MAX))
    .filter((char) => /[A-Za-z0-9\[\]:.\- /,]/.test(char))
    .join("");
  return cleaned.trim() ? cleaned : GUTTER_TIMESTAMP_DEFAULT_FORMAT;
}

/**
 * 归一 gutter 设置（sidecar preferences 或手改 JSON 的降级路径）：
 * 两个开关缺省 false（功能默认关闭），格式串缺省 "[HH:mm:ss]"。
 */
export function sanitizeGutterSettings(raw: unknown): GutterSettings {
  const source = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  return {
    showLineNumbers: source.showLineNumbers === true,
    showTimestamps: source.showTimestamps === true,
    timestampFormat: sanitizeTimestampFormat(source.timestampFormat),
  };
}
