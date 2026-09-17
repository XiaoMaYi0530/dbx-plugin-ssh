// @vitest-environment happy-dom
// FolderPickerDialog 组件测试：initialPath 直达、目录导航（下钻/上一级）、
// 无盘符平台回退默认目录浏览、确认emit 当前路径、关闭emit。
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

afterEach(() => {
  vi.restoreAllMocks();
});

describe("FolderPickerDialog", () => {
  it("browses initialPath on mount and lists subdirectories", async () => {
    installInvoke();
    const wrapper = mount(FolderPickerDialog, { props: { locale: "zh-CN", initialPath: "/home/demo" } });
    await flushPromises();
    const rows = wrapper.findAll(".folder-picker-row");
    // 上一级 + 两个子目录。
    expect(rows).toHaveLength(3);
    expect(wrapper.text()).toContain("Downloads");
    expect(wrapper.text()).toContain("Documents");
    expect(wrapper.find(".folder-picker-path input").element).toHaveProperty("value", "/home/demo");
  });

  it("navigates into a subdirectory and back up", async () => {
    installInvoke();
    const wrapper = mount(FolderPickerDialog, { props: { locale: "zh-CN", initialPath: "/home/demo" } });
    await flushPromises();
    const downloadsRow = wrapper.findAll(".folder-picker-row").find((row) => row.text().includes("Downloads"));
    await downloadsRow!.trigger("click");
    await flushPromises();
    expect(wrapper.find(".folder-picker-current").text()).toBe("/home/demo/Downloads");
    expect(wrapper.text()).toContain("该目录下没有子文件夹");
    // 上一级回到 /home/demo。
    await wrapper.findAll(".folder-picker-row")[0].trigger("click");
    await flushPromises();
    expect(wrapper.find(".folder-picker-current").text()).toBe("/home/demo");
  });

  it("falls back to the default directory when the platform has no drives", async () => {
    const invoke = installInvoke([]);
    mount(FolderPickerDialog, { props: { locale: "zh-CN" } });
    await flushPromises();
    expect(invoke).toHaveBeenCalledWith("local/fs/drives", {});
    expect(invoke).toHaveBeenCalledWith("local/fs/browse", {});
  });

  it("shows the drives page when drives exist and picks a drive", async () => {
    const invoke = installInvoke(["C:\\"]);
    const wrapper = mount(FolderPickerDialog, { props: { locale: "zh-CN" } });
    await flushPromises();
    expect(wrapper.text()).toContain("C:\\");
    await wrapper.find(".folder-picker-row").trigger("click");
    await flushPromises();
    expect(invoke).toHaveBeenCalledWith("local/fs/browse", { path: "C:\\" });
  });

  it("emits select with the current path and close on dismiss", async () => {
    installInvoke();
    const wrapper = mount(FolderPickerDialog, { props: { locale: "zh-CN", initialPath: "/home/demo" } });
    await flushPromises();
    await wrapper.find(".primary-button").trigger("click");
    expect(wrapper.emitted("select")).toEqual([["/home/demo"]]);
    await wrapper.find(".modal-backdrop").trigger("mousedown");
    expect(wrapper.emitted("close")).toHaveLength(1);
  });
});
