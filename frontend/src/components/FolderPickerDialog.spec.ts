// @vitest-environment happy-dom
// FolderPickerDialog 组件测试：initialPath 直达、目录导航（下钻/上一级）、
// 无盘符平台回退默认目录浏览、确认emit 当前路径、关闭emit。
// Phase 6：组件壳迁 reka Dialog，内容 portal 到 document.body；测试直接查询
// body 下的真实 DOM（wrapper.find 看不到 portal 内容），事件用原生 dispatch。
import { afterEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount } from "@vue/test-utils";
import FolderPickerDialog from "./FolderPickerDialog.vue";

const TREE: Record<string, { parent: string | null; dirs: string[] }> = {
  "/home/demo": { parent: "/home", dirs: ["Downloads", "Documents"] },
  "/home": { parent: "/", dirs: ["demo"] },
  "/home/demo/Downloads": { parent: "/home/demo", dirs: [] },
};

function installInvoke(drives: string[] = []) {
  const invoke = vi.fn(async (method: string, params?: Record<string, unknown>) => {
    if (method === "local/fs/drives") return { drives };
    if (method === "local/fs/browse") {
      const path = typeof params?.path === "string" ? params.path : "/home/demo";
      const node = TREE[path];
      if (!node) throw new Error(`Cannot open '${path}' (mock)`);
      return {
        path,
        parent: node.parent,
        entries: node.dirs.map((name) => ({ name, path: `${path}/${name}`, is_dir: true })),
      };
    }
    throw new Error(`unexpected method ${method}`);
  });
  (window as unknown as { dbxPlugin: unknown }).dbxPlugin = { invoke };
  return invoke;
}

function mountPicker(props: { locale: string; initialPath?: string }) {
  return mount(FolderPickerDialog, { props, attachTo: document.body });
}

const q = <T extends HTMLElement>(selector: string) => document.body.querySelector<T>(selector);
const qa = <T extends HTMLElement>(selector: string) => [...document.body.querySelectorAll<T>(selector)];
const bodyText = () => document.body.textContent ?? "";

async function click(el: Element | null) {
  expect(el, "点击目标存在").toBeTruthy();
  el!.dispatchEvent(new MouseEvent("click", { bubbles: true, composed: true }));
  await flushPromises();
}

afterEach(() => {
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("FolderPickerDialog", () => {
  it("browses initialPath on mount and lists subdirectories", async () => {
    installInvoke();
    mountPicker({ locale: "zh-CN", initialPath: "/home/demo" });
    await flushPromises();
    const rows = qa(".folder-picker-row");
    // 上一级 + 两个子目录。
    expect(rows).toHaveLength(3);
    expect(bodyText()).toContain("Downloads");
    expect(bodyText()).toContain("Documents");
    expect(q<HTMLInputElement>(".folder-picker-path input")).toHaveProperty("value", "/home/demo");
  });

  it("navigates into a subdirectory and back up", async () => {
    installInvoke();
    mountPicker({ locale: "zh-CN", initialPath: "/home/demo" });
    await flushPromises();
    const downloadsRow = qa(".folder-picker-row").find((row) => row.textContent?.includes("Downloads"));
    await click(downloadsRow!);
    expect(q(".folder-picker-current")?.textContent).toBe("/home/demo/Downloads");
    expect(bodyText()).toContain("该目录下没有子文件夹");
    // 上一级回到 /home/demo。
    await click(qa(".folder-picker-row")[0]);
    expect(q(".folder-picker-current")?.textContent).toBe("/home/demo");
  });

  it("falls back to the default directory when the platform has no drives", async () => {
    const invoke = installInvoke([]);
    mountPicker({ locale: "zh-CN" });
    await flushPromises();
    expect(invoke).toHaveBeenCalledWith("local/fs/drives", {});
    expect(invoke).toHaveBeenCalledWith("local/fs/browse", {});
  });

  it("shows the drives page when drives exist and picks a drive", async () => {
    const invoke = installInvoke(["C:\\"]);
    mountPicker({ locale: "zh-CN" });
    await flushPromises();
    expect(bodyText()).toContain("C:\\");
    await click(q(".folder-picker-row"));
    expect(invoke).toHaveBeenCalledWith("local/fs/browse", { path: "C:\\" });
  });

  it("emits select with the current path and close on dismiss", async () => {
    installInvoke();
    const wrapper = mountPicker({ locale: "zh-CN", initialPath: "/home/demo" });
    await flushPromises();
    await click(q(".primary-button"));
    expect(wrapper.emitted("select")).toEqual([["/home/demo"]]);
    // 遮罩外点击 → reka pointer-down-outside → update:open(false) → close。
    const overlay = q('[data-slot="dialog-overlay"]');
    expect(overlay, "遮罩存在").toBeTruthy();
    overlay!.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, composed: true, button: 0 }));
    overlay!.dispatchEvent(new MouseEvent("click", { bubbles: true, composed: true, button: 0 }));
    await flushPromises();
    expect(wrapper.emitted("close")).toHaveLength(1);
  });
});
