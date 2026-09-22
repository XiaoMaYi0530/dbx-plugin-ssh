/**
 * 端口映射（对标 Xshell/ssh(1) -L/-R）：把当前 SSH 会话上的转发映射解析成
 * 工作台面板行、校验添加表单并在 `ssh/forward/state` 事件到达时就地更新。
 * 本模块只做纯解析/校验/格式化，RPC 调用留在 App.vue。
 *
 * sidecar 协议（docs/PROTOCOL.zh-CN.md）：
 * - `ssh/forward/list` { connectionId? | sessionId? } → { forwards: row[] }
 * - `ssh/forward/start` { sessionId, kind, listenHost?, listenPort,
 *   targetHost, targetPort } → { forward: row }
 * - `ssh/forward/stop` { id } → { success, forward }
 * - `ssh/forward/state`（事件）{ id, state, error? }
 */

export type ForwardKind = "local" | "remote";

export interface PortForward {
  id: string;
  sessionId: string;
  connectionId: string;
  kind: ForwardKind;
  listenHost: string;
  listenPort: number;
  /** 实际绑定端口：本地 0 端口由 OS 挑选、远程 0 端口由服务端挑选。 */
  boundPort: number;
  targetHost: string;
  targetPort: number;
  state: "starting" | "active" | "stopped" | "error";
  error?: string;
  connectionsTotal: number;
  connectionsActive: number;
  bytesUp: number;
  bytesDown: number;
}

/** 添加映射表单的字符串态：输入框里的端口在提交前都是文本。 */
export interface ForwardFormDraft {
  kind: ForwardKind;
  listenHost: string;
  listenPort: string;
  targetHost: string;
  targetPort: string;
}

/** 表单校验错误码 → i18n `forwards.error.*`；null 表示通过。 */
export type ForwardFormError = "targetHost" | "port" | null;

function asForwardRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" ? (value as Record<string, unknown>) : null;
}

function stringField(record: Record<string, unknown>, key: string): string {
  const value = record[key];
  return typeof value === "string" ? value : "";
}

function numberField(record: Record<string, unknown>, key: string): number {
  const value = record[key];
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

function parseKind(value: unknown): ForwardKind {
  return value === "remote" ? "remote" : "local";
}

/** `ssh/forward/list` / `ssh/forward/start` 载荷 → 面板行；坏行直接丢弃。 */
export function parseForwards(payload: unknown): PortForward[] {
  const record = asForwardRecord(payload);
  const rows = record && Array.isArray(record.forwards) ? record.forwards : [];
  const single = record && record.forward ? [record.forward] : [];
  const parsed: PortForward[] = [];
  for (const row of [...rows, ...single]) {
    const source = asForwardRecord(row);
    if (!source || typeof source.id !== "string" || !source.id) continue;
    parsed.push({
      id: source.id,
      sessionId: stringField(source, "sessionId"),
      connectionId: stringField(source, "connectionId"),
      kind: parseKind(source.kind),
      listenHost: stringField(source, "listenHost") || "127.0.0.1",
      listenPort: numberField(source, "listenPort"),
      boundPort: numberField(source, "boundPort"),
      targetHost: stringField(source, "targetHost"),
      targetPort: numberField(source, "targetPort"),
      state: parseState(source.state),
      error: typeof source.error === "string" && source.error ? source.error : undefined,
      connectionsTotal: numberField(source, "connectionsTotal"),
      connectionsActive: numberField(source, "connectionsActive"),
      bytesUp: numberField(source, "bytesUp"),
      bytesDown: numberField(source, "bytesDown"),
    });
  }
  return parsed;
}

function parseState(value: unknown): PortForward["state"] {
  return value === "starting" || value === "stopped" || value === "error" ? value : "active";
}

/**
 * 就地套用一条 `ssh/forward/state` 事件：已知 id 更新状态列，未知 id 忽略
 * （由下一次 list 校正）。返回新数组，保持 Vue 响应式替换语义。
 */
export function applyForwardState(
  rows: PortForward[],
  event: { id?: unknown; state?: unknown; error?: unknown },
): PortForward[] {
  if (typeof event.id !== "string" || !event.id) return rows;
  return rows.map((row) =>
    row.id === event.id
      ? { ...row, state: parseState(event.state), error: typeof event.error === "string" && event.error ? event.error : undefined }
      : row,
  );
}

/**
 * 表单校验（与 sidecar `parse_spec` 同语义的客户端预检）：
 * - targetHost 必填（listenHost 缺省回落 127.0.0.1，由 sidecar 再兜底）；
 * - 两个端口必须是 0..=65535 整数；0 表示让 OS/服务端挑选。
 */
export function validateForwardForm(draft: ForwardFormDraft): ForwardFormError {
  if (!draft.targetHost.trim()) return "targetHost";
  for (const port of [draft.listenPort, draft.targetPort]) {
    const value = Number(port.trim());
    if (!port.trim() || !Number.isInteger(value) || value < 0 || value > 65535) return "port";
  }
  return null;
}

/** 校验通过后的 RPC 参数（端口转数字；listenHost 空串交给 sidecar 默认）。 */
export function forwardStartParams(draft: ForwardFormDraft, sessionId: string) {
  return {
    sessionId,
    kind: draft.kind,
    listenHost: draft.listenHost.trim(),
    listenPort: Number(draft.listenPort.trim()),
    targetHost: draft.targetHost.trim(),
    targetPort: Number(draft.targetPort.trim()),
  };
}

/** 面板行主文案：`127.0.0.1:8080 → db:5432`，0 端口显示实际绑定值。 */
export function formatForwardRoute(row: Pick<PortForward, "kind" | "listenHost" | "listenPort" | "boundPort" | "targetHost" | "targetPort">): string {
  const listen = row.boundPort > 0 ? row.boundPort : row.listenPort;
  const arrow = row.kind === "remote" ? "←" : "→";
  return `${row.listenHost}:${listen} ${arrow} ${row.targetHost}:${row.targetPort}`;
}

export function formatForwardBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let value = bytes;
  let unit = "KiB";
  for (const next of units) {
    value /= 1024;
    unit = next;
    if (value < 1024) break;
  }
  return `${value >= 100 ? Math.round(value) : value.toFixed(1)} ${unit}`;
}
