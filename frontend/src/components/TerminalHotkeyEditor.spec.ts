// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from "vitest";
import { nextTick } from "vue";
import { mount, type VueWrapper } from "@vue/test-utils";
import TerminalHotkeyEditor from "./TerminalHotkeyEditor.vue";
import { defaultTerminalHotkeys, formatHotkeyDisplay, TERMINAL_HOTKEY_ACTIONS, type TerminalHotkeyBindings } from "../lib/terminalHotkeys";

// 组件只做展示与录制，绑定表的权威态在 App；因此断言口径集中在「上抛了什么」，
// 而不是「组件内部存了什么」——上抛结果就是契约。

function translator(key: string, values?: Record<string, string | number>): string {
  return values?.name ? `${key}<${values.name}>` : key;
}

let wrapper: VueWrapper | undefined;

afterEach(() => {
  wrapper?.unmount();
  wrapper = undefined;
});

function factory(bindings: TerminalHotkeyBindings = defaultTerminalHotkeys(true), applePlatform = true) {
  wrapper = mount(TerminalHotkeyEditor, {
    props: { bindings, applePlatform, t: translator },
    attachTo: document.body,
  });
  return wrapper;
}

function labelKeyOf(actionId: string): string {
  return TERMINAL_HOTKEY_ACTIONS.find((action) => action.id === actionId)!.labelKey;
}

/** 按动作显示名定位该行（行序由分组与动作表顺序决定，用文本定位更稳）。 */
function row(actionId: string) {
  const target = translator(labelKeyOf(actionId));
  const found = wrapper!.findAll(".hotkey-row").find((node) => node.find(".hotkey-label").text() === target);
  if (!found) throw new Error(`row not found: ${actionId}`);
  return found;
}

/** 录制态监听挂在 window 捕获阶段，因此事件从 window 派发；派发后等一次刷新再看 DOM。 */
async function press(code: string, options: { meta?: boolean; ctrl?: boolean; alt?: boolean; shift?: boolean; key?: string } = {}) {
  window.dispatchEvent(
    new KeyboardEvent("keydown", {
      code,
      key: options.key ?? code,
      metaKey: Boolean(options.meta),
      ctrlKey: Boolean(options.ctrl),
      altKey: Boolean(options.alt),
      shiftKey: Boolean(options.shift),
      bubbles: true,
      cancelable: true,
    }),
  );
  await nextTick();
}

function updates(): TerminalHotkeyBindings[] {
  return (wrapper!.emitted("update") ?? []).map((args) => (args as [TerminalHotkeyBindings])[0]);
}

function lastUpdate(): TerminalHotkeyBindings {
  const list = updates();
  expect(list.length).toBeGreaterThan(0);
  return list[list.length - 1];
}

describe("TerminalHotkeyEditor 分组与回显", () => {
  it("按剪贴板/视图/导航三组渲染全部动作", () => {
    factory();
    const text = wrapper!.text();
    for (const key of ["terminalHotkeys.groupClipboard", "terminalHotkeys.groupView", "terminalHotkeys.groupNavigation"]) {
      expect(text).toContain(translator(key));
    }
    for (const action of TERMINAL_HOTKEY_ACTIONS) {
      expect(text).toContain(translator(action.labelKey));
    }
    expect(wrapper!.findAll(".hotkey-row")).toHaveLength(TERMINAL_HOTKEY_ACTIONS.length);
  });

  it("Apple 平台按符号显示组合串，其它平台按 Ctrl/Shift 文字", () => {
    factory(defaultTerminalHotkeys(true), true);
    expect(row("search").text()).toContain(formatHotkeyDisplay("Meta+F", true));

    wrapper!.unmount();
    factory(defaultTerminalHotkeys(false), false);
    expect(row("search").text()).toContain(formatHotkeyDisplay("Ctrl+Shift+F", false));
  });

  it("未绑定的动作显出「未绑定」占位而不是空单元格", () => {
    factory({ ...defaultTerminalHotkeys(true), clear: [] });
    expect(row("clear").find(".hotkey-unbound").text()).toBe(translator("terminalHotkeys.unbound"));
  });

  it("搜索按动作名过滤，无匹配时给空态", async () => {
    factory();
    await wrapper!.find(".hotkey-search").setValue("terminalHotkeys.actionCopy");
    expect(wrapper!.findAll(".hotkey-row")).toHaveLength(1);
    expect(wrapper!.find(".hotkey-row").find(".hotkey-label").text()).toBe(translator("terminalHotkeys.actionCopy"));

    await wrapper!.find(".hotkey-search").setValue("no-such-action-xyz");
    expect(wrapper!.findAll(".hotkey-row")).toHaveLength(0);
    expect(wrapper!.text()).toContain(translator("terminalHotkeys.empty"));
  });
});

describe("TerminalHotkeyEditor 录制", () => {
  it("点击键位进入录制，按下带修饰键的组合后整表上抛", async () => {
    factory();
    await row("search").find(".hotkey-chip").trigger("click");
    expect(row("search").find(".hotkey-chip").classes()).toContain("recording");

    await press("KeyZ", { meta: true, key: "z" });

    const next = lastUpdate();
    expect(next.search).toEqual(["Meta+Z"]);
    // 其余动作原样带过，避免整表替换时丢绑定。
    expect(next.copy).toEqual(defaultTerminalHotkeys(true).copy);
    expect(row("search").find(".hotkey-chip").classes()).not.toContain("recording");
  });

  it("裸键（无修饰键）被拒绝且保持录制，随后仍可正常绑定", async () => {
    factory();
    await row("search").find(".hotkey-chip").trigger("click");

    await press("KeyZ", { key: "z" });
    expect(updates()).toHaveLength(0);
    expect(wrapper!.text()).toContain(translator("terminalHotkeys.invalidCombo"));
    expect(row("search").find(".hotkey-chip").classes()).toContain("recording");

    await press("KeyZ", { meta: true, key: "z" });
    expect(lastUpdate().search).toEqual(["Meta+Z"]);
  });

  it("只按修饰键本身不算一击，继续等待主键", async () => {
    factory();
    await row("search").find(".hotkey-chip").trigger("click");

    await press("ShiftLeft", { shift: true, key: "Shift" });
    expect(updates()).toHaveLength(0);
    expect(row("search").find(".hotkey-chip").classes()).toContain("recording");

    await press("KeyY", { ctrl: true, shift: true, key: "Y" });
    expect(lastUpdate().search).toEqual(["Ctrl+Shift+Y"]);
  });

  it("Escape 取消录制、不改动绑定", async () => {
    factory();
    await row("search").find(".hotkey-chip").trigger("click");
    await press("Escape", { key: "Escape" });

    expect(updates()).toHaveLength(0);
    const chip = row("search").find(".hotkey-chip");
    expect(chip.classes()).not.toContain("recording");
    expect(chip.text()).toBe(formatHotkeyDisplay("Meta+F", true));
  });

  it("再点同一个键位即取消录制", async () => {
    factory();
    const chip = row("search").find(".hotkey-chip");
    await chip.trigger("click");
    expect(row("search").find(".hotkey-chip").classes()).toContain("recording");

    await row("search").find(".hotkey-chip").trigger("click");
    expect(row("search").find(".hotkey-chip").classes()).not.toContain("recording");
    expect(updates()).toHaveLength(0);
  });

  it("新绑定落在末尾、改写落在原位", async () => {
    factory();
    await row("search").find(".hotkey-add").trigger("click");
    await press("KeyJ", { meta: true, key: "j" });
    expect(lastUpdate().search).toEqual(["Meta+F", "Meta+J"]);

    wrapper!.unmount();
    factory(defaultTerminalHotkeys(true));
    // 改写第二条：不得改变条目顺序（顺序影响匹配时的可预期性）。
    await row("paste").findAll(".hotkey-chip")[0].trigger("click");
    await press("KeyP", { meta: true, key: "p" });
    expect(lastUpdate().paste[0]).toBe("Meta+P");
  });

  it("同一动作内写入重复组合时自动去重", async () => {
    factory();
    await row("search").find(".hotkey-add").trigger("click");
    await press("KeyF", { meta: true, key: "f" });
    expect(lastUpdate().search).toEqual(["Meta+F"]);
  });

  it("达到每动作绑定数上限后不再提供新增入口", async () => {
    factory({ ...defaultTerminalHotkeys(true), search: ["Meta+F", "Meta+G", "Meta+H"] });
    expect(row("search").find(".hotkey-add").exists()).toBe(false);

    // 未达上限的动作仍保留新增入口。
    expect(row("clear").find(".hotkey-add").exists()).toBe(true);
  });
});

describe("TerminalHotkeyEditor 移除、复位与冲突", () => {
  it("移除单个键位只删该条", async () => {
    factory({ ...defaultTerminalHotkeys(false), paste: ["Ctrl+V", "Ctrl+Shift+V"] });
    await row("paste").findAll(".hotkey-remove")[0].trigger("click");
    expect(lastUpdate().paste).toEqual(["Ctrl+Shift+V"]);
  });

  it("单项复位与整体复位都回到当前平台默认", async () => {
    factory({ ...defaultTerminalHotkeys(true), search: [] });
    await row("search").find(".hotkey-reset").trigger("click");
    expect(lastUpdate().search).toEqual(defaultTerminalHotkeys(true).search);

    wrapper!.unmount();
    factory({ ...defaultTerminalHotkeys(false), search: [], copy: [] }, false);
    await wrapper!.find(".hotkey-toolbar .link-button").trigger("click");
    expect(lastUpdate()).toEqual(defaultTerminalHotkeys(false));
  });

  it("被其它动作占用的组合带冲突类与占用者提示，但不阻止写入", async () => {
    factory({ ...defaultTerminalHotkeys(true), copy: ["Meta+Y"], paste: ["Meta+Y"] });
    const chip = row("copy").find(".hotkey-chip");
    expect(chip.classes()).toContain("conflict");
    expect(chip.attributes("title")).toBe(translator("terminalHotkeys.conflict", { name: translator(labelKeyOf("paste")) }));
    expect(chip.text()).toBe(formatHotkeyDisplay("Meta+Y", true));
  });
});
