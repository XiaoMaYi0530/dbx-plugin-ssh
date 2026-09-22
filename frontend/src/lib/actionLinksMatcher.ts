// 终端动作链接匹配器（P1-2）：纯函数、零 xterm/零 DOM 依赖。
// 三类命中——IPv4（严格 0-255）、host:port（nc 连通性探测）、压缩包（解包命令）——
// 只负责"识别 + 给出建议命令文本"；点击执行、xterm link provider 注册与装饰
// 绘制分别留在 App.vue 接线层与 actionLinksAddon.ts。默认功能关闭（见
// sanitizeActionLinksSettings），只在设置开启后由 App.vue 注册生效。

/** 命中类型。 */
export type ActionLinkKind = "ipv4" | "host-port" | "archive";

/** 单行中的一个动作命中区间（end 为 exclusive），附点击建议命令。 */
export interface ActionLinkMatch {
  kind: ActionLinkKind;
  text: string;
  start: number;
  end: number;
  command: string;
}

/** 三类匹配器开关（设置 `action_links_matchers`）。缺省全开。 */
export interface ActionLinkMatcherToggles {
  ipv4: boolean;
  hostPort: boolean;
  archive: boolean;
}

/** 动作链接整体设置（`action_links_enabled` + `action_links_matchers`）。 */
export interface ActionLinksSettings {
  enabled: boolean;
  matchers: ActionLinkMatcherToggles;
}

export const ACTION_LINK_MATCHER_DEFAULTS: ActionLinkMatcherToggles = { ipv4: true, hostPort: true, archive: true };
export const ACTION_LINK_SETTINGS_DEFAULTS: ActionLinksSettings = { enabled: false, matchers: { ...ACTION_LINK_MATCHER_DEFAULTS } };

/** 单行命中上限（与关键词高亮同向的装饰数量护栏的一部分）。 */
export const ACTION_LINK_MATCHES_PER_LINE_LIMIT = 20;
/** 超长行不扫描（xterm 一行通常 ≤500 列；2048 已远超常规宽度）。 */
const MAX_LINE_SCAN_LENGTH = 2048;

// ---- 正则原料 --------------------------------------------------------------
// 0-255 严格八位组：25[0-5] | 2[0-4]d | 1dd | [1-9]?d（不吞前导 0 以外的形状）。
const OCTET = "(?:25[0-5]|2[0-4]\\d|1\\d\\d|[1-9]?\\d)";
const IPV4_SOURCE = `${OCTET}(?:\\.${OCTET}){3}`;
// 域名标签：字母数字开头结尾，中间可有连字符；域名至少两段。
const LABEL = "[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?";
const DOMAIN_SOURCE = `${LABEL}(?:\\.${LABEL})+`;
// host:port 的 host 支持 localhost / 域名 / IPv4；port 抓 1-5 位数字后做值域校验。
const HOST_PORT_SOURCE = `(?:localhost|${DOMAIN_SOURCE}|${IPV4_SOURCE}):(\\d{1,5})`;
// 压缩包：基名不含空白与 shell 危险字符，扩展名大小写不敏感；后缘 (?![.\w])
// 排除 .zipx / .tar.gz.bak 这类"更长的真扩展名"。
const ARCHIVE_SOURCE =
  "[^\\s\"'`()\\[\\]{}<>|;&*?\\\\]+\\.(?:zip|rar|7z|tar\\.gz|tgz|tar\\.bz2|tbz2|tar\\.xz|txz)(?![.\\w])";

const IPV4_REGEX = new RegExp(IPV4_SOURCE, "g");
const HOST_PORT_REGEX = new RegExp(HOST_PORT_SOURCE, "g");
const ARCHIVE_REGEX = new RegExp(ARCHIVE_SOURCE, "gi");

/**
 * host:port 的误报黑名单：host 末段命中这些源码/日志/配置扩展名时不算
 * （`app.py:42`、`server.log:1` 是"文件名:行号"，不是可连接端点）。
 */
const HOST_TLD_DENYLIST = new Set([
  "py", "js", "ts", "jsx", "tsx", "go", "rs", "log", "md", "txt", "json",
  "yaml", "yml", "toml", "html", "css", "scss", "java", "c", "h", "cpp",
  "cc", "hpp", "rb", "php", "sh", "bash", "zsh", "sql", "xml", "conf",
]);

/** 按类型给解包命令；rar 无内建工具，走 7z（与 7z 同一路径）。 */
function archiveCommand(fileName: string): string {
  const lower = fileName.toLowerCase();
  if (lower.endsWith(".zip")) return `unzip ${fileName}`;
  if (lower.endsWith(".rar") || lower.endsWith(".7z")) return `7z x ${fileName}`;
  if (lower.endsWith(".tar.gz") || lower.endsWith(".tgz")) return `tar -xzvf ${fileName}`;
  if (lower.endsWith(".tar.bz2") || lower.endsWith(".tbz2")) return `tar -xjvf ${fileName}`;
  return `tar -xJvf ${fileName}`; // .tar.xz / .txz
}

/**
 * 数值边界护栏：命中紧贴其它数字/点分链时不算——
 * `1.2.3.4.5`、`910.0.0.1`、`host:8080:443`、`example.com:8080.5`。
 * 前后是普通标点/路径分隔则放行（句子句点、CIDR `/24`、URL 路径等）。
 */
function touchesDigitChain(text: string, start: number, end: number): boolean {
  const before = start > 0 ? text[start - 1] : "";
  const beforeBefore = start > 1 ? text[start - 2] : "";
  const after = end < text.length ? text[end] : "";
  const afterAfter = end + 1 < text.length ? text[end + 1] : "";
  const leadingBad = /[0-9]/.test(before) || (before === "." && /[0-9]/.test(beforeBefore));
  const trailingBad = /[0-9]/.test(after) || (after === "." && /[0-9]/.test(afterAfter));
  return leadingBad || trailingBad;
}

function collectMatches(text: string, regex: RegExp, build: (hit: string, start: number, end: number) => string | null): Array<{ start: number; end: number; command: string }> {
  const collected: Array<{ start: number; end: number; command: string }> = [];
  regex.lastIndex = 0;
  let hit: RegExpExecArray | null;
  while ((hit = regex.exec(text)) !== null) {
    // 边界护栏拒绝后从命中起点 +1 继续扫，让内部候选也有被评估的机会
    // （"910.0.0.1" 的 "10.0.0.1" 等仍会被位置护栏拒绝，不会误放行）。
    const nextIndex = hit.index + 1;
    const command = build(hit[0], hit.index, hit.index + hit[0].length);
    if (command !== null) collected.push({ start: hit.index, end: hit.index + hit[0].length, command });
    regex.lastIndex = nextIndex;
    if (hit[0].length === 0) break; // 防零宽死循环（当前正则不会产出，兜底）。
  }
  return collected;
}

function matchIpv4(text: string): Array<{ start: number; end: number; command: string }> {
  return collectMatches(text, IPV4_REGEX, (hit, start, end) => (touchesDigitChain(text, start, end) ? null : `ping ${hit}`));
}

function matchHostPort(text: string): Array<{ start: number; end: number; command: string }> {
  return collectMatches(text, HOST_PORT_REGEX, (hit, start, end) => {
    if (touchesDigitChain(text, start, end)) return null;
    // 端口后又跟 ":数字"（1.2.3.4:80:443）不是单一端点，让位给裸 IPv4。
    if (text[end] === ":" && /[0-9]/.test(text[end + 1] ?? "")) return null;
    const separator = hit.lastIndexOf(":");
    const host = hit.slice(0, separator);
    const port = Number.parseInt(hit.slice(separator + 1), 10);
    if (!Number.isInteger(port) || port < 1 || port > 65535) return null;
    const lastLabel = host.slice(host.lastIndexOf(".") + 1).toLowerCase();
    if (HOST_TLD_DENYLIST.has(lastLabel)) return null;
    return `nc -vz ${host} ${port}`;
  });
}

function matchArchive(text: string): Array<{ start: number; end: number; command: string }> {
  // 压缩包无数字链问题（基名可含数字），只套统一的收集器。
  return collectMatches(text, ARCHIVE_REGEX, (hit) => archiveCommand(hit));
}

/**
 * 计算一行文本中的全部动作命中：
 * - 各开关开启的匹配器分别扫描，按 (start 升序, 命中更长优先) 排序后
 *   先到先得消除重叠（`10.0.0.1:8080` 整段 host:port 赢过内部裸 IPv4）；
 * - 每行命中数上限 `ACTION_LINK_MATCHES_PER_LINE_LIMIT`；
 * - 空文本 / 超长行 / 全关开关返回空数组；共享正则每次重置 lastIndex。
 */
export function matchActionLinks(text: string, toggles: ActionLinkMatcherToggles = ACTION_LINK_MATCHER_DEFAULTS): ActionLinkMatch[] {
  if (!text || text.length > MAX_LINE_SCAN_LENGTH) return [];
  const collected: Array<{ kind: ActionLinkKind; start: number; end: number; command: string }> = [];
  if (toggles.ipv4) collected.push(...matchIpv4(text).map((hit) => ({ ...hit, kind: "ipv4" as const })));
  if (toggles.hostPort) collected.push(...matchHostPort(text).map((hit) => ({ ...hit, kind: "host-port" as const })));
  if (toggles.archive) collected.push(...matchArchive(text).map((hit) => ({ ...hit, kind: "archive" as const })));
  if (!collected.length) return [];
  collected.sort((a, b) => a.start - b.start || (b.end - b.start) - (a.end - a.start));
  const matches: ActionLinkMatch[] = [];
  let lastEnd = -1;
  for (const hit of collected) {
    if (hit.start < lastEnd) continue;
    lastEnd = hit.end;
    matches.push({ kind: hit.kind, text: text.slice(hit.start, hit.end), start: hit.start, end: hit.end, command: hit.command });
    if (matches.length >= ACTION_LINK_MATCHES_PER_LINE_LIMIT) break;
  }
  return matches;
}

/** 布尔收紧：仅接受 boolean，其余回退默认。 */
function pickBoolean(value: unknown, fallback: boolean): boolean {
  return typeof value === "boolean" ? value : fallback;
}

/**
 * 归一动作链接设置（sidecar preferences 或手改 JSON 的降级路径）：
 * enabled 缺省 false（功能默认关闭），matchers 逐字段收紧、缺省全开。
 */
export function sanitizeActionLinksSettings(raw: unknown): ActionLinksSettings {
  const source = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  const matchersSource = source.matchers && typeof source.matchers === "object" ? (source.matchers as Record<string, unknown>) : {};
  return {
    enabled: pickBoolean(source.enabled, ACTION_LINK_SETTINGS_DEFAULTS.enabled),
    matchers: {
      ipv4: pickBoolean(matchersSource.ipv4, ACTION_LINK_MATCHER_DEFAULTS.ipv4),
      hostPort: pickBoolean(matchersSource.hostPort, ACTION_LINK_MATCHER_DEFAULTS.hostPort),
      archive: pickBoolean(matchersSource.archive, ACTION_LINK_MATCHER_DEFAULTS.archive),
    },
  };
}
