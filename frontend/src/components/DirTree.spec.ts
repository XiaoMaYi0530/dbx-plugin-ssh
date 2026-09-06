// @vitest-environment happy-dom
// DirTree 组件测试：行单击 open、caret 展开/收缩 toggle（阻断 open）、右键 context、
// 当前目录高亮、懒加载 spinner、展开/收起图标切换、递归子节点渲染与事件冒泡。
import { describe, expect, it } from "vitest";
import { mount } from "@vue/test-utils";
import DirTree from "./DirTree.vue";
import type { DirTreeNode } from "../lib/sftpDirTree";

// t prop 用确定性假实现：仅翻译根目录文案，其余原样返回 key，断言与 i18n 表解耦。
const t = (key: string) => (key === "sftpSide.root" ? "ROOT" : key);

function node(partial: Partial<DirTreeNode> & { path: string; name: string }): DirTreeNode {
  return { expanded: false, loaded: false, loading: false, children: [], ...partial };
}

function mountTree(nodes: DirTreeNode[], currentPath = "/") {
  return mount(DirTree, { props: { nodes, depth: 0, currentPath, t } });
}

describe("DirTree", () => {
  it("renders one row per node and localizes the '/' root label", () => {
    const wrapper = mountTree([node({ path: "/", name: "/" }), node({ path: "/var", name: "var" })]);
    const rows = wrapper.findAll(".sftp-tree-row");
    expect(rows).toHaveLength(2);
    expect(rows[0].find(".sftp-tree-name").text()).toBe("ROOT");
    expect(rows[1].find(".sftp-tree-name").text()).toBe("var");
    expect(rows[0].attributes("title")).toBe("/");
  });

  it("marks the row matching currentPath with is-current", () => {
    const wrapper = mountTree([node({ path: "/var", name: "var" }), node({ path: "/etc", name: "etc" })], "/etc");
    expect(wrapper.findAll(".sftp-tree-row.is-current")).toHaveLength(1);
    expect(wrapper.find(".sftp-tree-row.is-current .sftp-tree-name").text()).toBe("etc");
  });

  it("indents rows by depth via padding-left", () => {
    const wrapper = mountTree([node({ path: "/", name: "/" })]);
    // depth 0 → 6px, depth 1 → 18px (6 + 12 * depth).
    expect(wrapper.find(".sftp-tree-row").attributes("style")).toContain("padding-left: 6px");
    const nested = mount(DirTree, { props: { nodes: [node({ path: "/var", name: "var" })], depth: 1, currentPath: "/", t } });
    expect(nested.find(".sftp-tree-row").attributes("style")).toContain("padding-left: 18px");
  });

  it("clicking a row emits open with the node (panel navigates)", async () => {
    const target = node({ path: "/var", name: "var" });
    const wrapper = mountTree([node({ path: "/", name: "/" }), target]);
    await wrapper.findAll(".sftp-tree-row")[1].trigger("click");
    expect(wrapper.emitted("open")?.[0]).toEqual([target]);
  });

  it("clicking the caret emits toggle only and does not emit open", async () => {
    const target = node({ path: "/var", name: "var" });
    const wrapper = mountTree([target]);
    await wrapper.find(".sftp-tree-caret").trigger("click");
    expect(wrapper.emitted("toggle")?.[0]).toEqual([target]);
    expect(wrapper.emitted("open")).toBeUndefined();
  });

  it("shows the loading spinner instead of a chevron while a node loads", () => {
    const wrapper = mountTree([node({ path: "/var", name: "var", loading: true })]);
    expect(wrapper.find(".sftp-tree-spinner").exists()).toBe(true);
    expect(wrapper.find(".sftp-tree-caret svg").exists()).toBe(false);
  });

  it("swaps chevron-down + folder-open when expanded, chevron-right + folder when collapsed", () => {
    const wrapper = mountTree([node({ path: "/a", name: "a", expanded: true }), node({ path: "/b", name: "b" })]);
    const rows = wrapper.findAll(".sftp-tree-row");
    const expandedCaret = rows[0].find(".sftp-tree-caret svg");
    const collapsedCaret = rows[1].find(".sftp-tree-caret svg");
    expect(expandedCaret.classes()).toContain("lucide-chevron-down");
    expect(collapsedCaret.classes()).toContain("lucide-chevron-right");
    expect(rows[0].find("svg.lucide-folder-open").exists()).toBe(true);
    expect(rows[1].find("svg.lucide-folder-open").exists()).toBe(false);
    expect(rows[1].find("svg.lucide-folder").exists()).toBe(true);
  });

  it("right-click emits context with the node and pointer coordinates", async () => {
    const target = node({ path: "/var", name: "var" });
    const wrapper = mountTree([target]);
    await wrapper.find(".sftp-tree-row").trigger("contextmenu", { clientX: 120, clientY: 80 });
    expect(wrapper.emitted("context")?.[0]).toEqual([{ node: target, x: 120, y: 80 }]);
  });

  it("renders expanded children one level deeper and bubbles their events to the parent emits", async () => {
    const child = node({ path: "/var/log", name: "log" });
    const root = node({ path: "/var", name: "var", expanded: true, loaded: true, children: [child] });
    const wrapper = mountTree([root]);
    const rows = wrapper.findAll(".sftp-tree-row");
    expect(rows).toHaveLength(2);
    expect(rows[1].attributes("style")).toContain("padding-left: 18px");

    // Child events bubble through the recursive instance up to the same emits.
    await rows[1].trigger("click");
    expect(wrapper.emitted("open")?.[0]).toEqual([child]);
    await rows[1].find(".sftp-tree-caret").trigger("click");
    expect(wrapper.emitted("toggle")?.[0]).toEqual([child]);
    await rows[1].trigger("contextmenu", { clientX: 5, clientY: 9 });
    expect(wrapper.emitted("context")?.[0]).toEqual([{ node: child, x: 5, y: 9 }]);
  });

  it("does not render child rows for an expanded node whose children are empty", () => {
    const wrapper = mountTree([node({ path: "/var", name: "var", expanded: true, loaded: true, children: [] })]);
    expect(wrapper.findAll(".sftp-tree-row")).toHaveLength(1);
  });
});
