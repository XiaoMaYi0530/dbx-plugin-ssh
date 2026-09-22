// PR-A4 context.plugin 命名空间（HOST_PLUGIN_UI_SPEC §4/§7.5/§11）：插件载荷
// 只进 context.plugin；workbenchId/restored/surface/connectionId 是宿主保留
// 字段，由宿主在最终 context 注入，插件不得伪造。本地终端直通以
// `plugin.mode === "local-terminal"` 判定——旧的顶层 `{ localTerminal: true }`
// 形状随宿主 A1 契约移除（读写两端同仓硬切换，不做双读兼容）。纯函数。
import { normalizeConnectionText } from "./connectionInfo";

/**
 * 读取 `context.plugin.mode`；载荷缺失/非对象/数组/模式非字符串一律返回
 * 空串，调用方以具体模式名比较（`readPluginMode(ctx) === "local-terminal"`）。
 */
export function readPluginMode(context: Record<string, unknown> | undefined | null): string {
  const plugin = context?.plugin;
  if (!plugin || typeof plugin !== "object" || Array.isArray(plugin)) return "";
  const mode = (plugin as Record<string, unknown>).mode;
  return typeof mode === "string" ? mode : "";
}

/**
 * 宿主权威 workbenchId：Host API 1.1+ 由宿主注入；旧宿主（1.0，不注入该
 * 字段）回落调用方本地生成的 id，保持会话按 workbench 实例隔离。
 */
export function resolveWorkbenchId(context: Record<string, unknown> | undefined | null, fallback: string): string {
  return normalizeConnectionText(context?.workbenchId) || fallback;
}
