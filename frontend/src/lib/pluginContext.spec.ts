import { describe, expect, it } from "vitest";
import { readPluginMode, resolveWorkbenchId } from "./pluginContext";

// PR-A4 context.plugin 命名空间的读取助手。用例锁定两件事：A4 目标形状的
// 类型安全读取（含对旧 { localTerminal: true } 直通形状的硬切换拒绝），与
// 宿主权威 workbenchId 的旧宿主（Host API 1.0 不注入）fallback 兼容
// （实施计划 R-旧宿主兼容：fallback 必须保留并有用例）。
describe("pluginContext (PR-A4 context.plugin namespace)", () => {
  it("readPluginMode reads context.plugin.mode from the A4 shape", () => {
    expect(readPluginMode({ plugin: { mode: "local-terminal" } })).toBe("local-terminal");
    // 预留扩展位（P2 panel surface 等后续模式名原样透出，由调用方比较）。
    expect(readPluginMode({ plugin: { mode: "panel" } })).toBe("panel");
    expect(readPluginMode({ plugin: { mode: "local-terminal", extra: 1 } })).toBe("local-terminal");
  });

  it("readPluginMode rejects the legacy top-level shape and malformed payloads (hard switch)", () => {
    // 旧 { localTerminal: true } 直通形状不再触发本地模式。
    expect(readPluginMode({ localTerminal: true })).toBe("");
    expect(readPluginMode({ localTerminal: true, plugin: { mode: "local-terminal" } })).toBe("local-terminal");
    // 载荷必须是普通对象，mode 必须是字符串。
    expect(readPluginMode({ plugin: null })).toBe("");
    expect(readPluginMode({ plugin: "local-terminal" })).toBe("");
    expect(readPluginMode({ plugin: [{ mode: "local-terminal" }] })).toBe("");
    expect(readPluginMode({ plugin: {} })).toBe("");
    expect(readPluginMode({ plugin: { mode: 42 } })).toBe("");
    expect(readPluginMode({})).toBe("");
    expect(readPluginMode(undefined)).toBe("");
    expect(readPluginMode(null)).toBe("");
  });

  it("resolveWorkbenchId prefers the host-injected id", () => {
    expect(resolveWorkbenchId({ workbenchId: "host-workbench-1" }, "fallback")).toBe("host-workbench-1");
    expect(resolveWorkbenchId({ workbenchId: "host-workbench-1", restored: true }, "fallback")).toBe("host-workbench-1");
  });

  it("resolveWorkbenchId falls back on legacy hosts that do not inject the id", () => {
    expect(resolveWorkbenchId({}, "fallback")).toBe("fallback");
    expect(resolveWorkbenchId(undefined, "fallback")).toBe("fallback");
    expect(resolveWorkbenchId(null, "fallback")).toBe("fallback");
    // 空白与序列化空值视同缺失（与 normalizeConnectionText 的展示语义一致）。
    expect(resolveWorkbenchId({ workbenchId: "   " }, "fallback")).toBe("fallback");
    expect(resolveWorkbenchId({ workbenchId: "null" }, "fallback")).toBe("fallback");
    expect(resolveWorkbenchId({ workbenchId: 42 }, "fallback")).toBe("fallback");
  });
});
