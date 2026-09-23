// @vitest-environment happy-dom
// TerminalContextMenu 组件测试：纯解析助手（引擎表解析/URL 构造）为主，
// 组件渲染为辅（reka 弹层 portal 到 body，经 document 查询断言）。
import { describe, expect, it } from "vitest";
import { mount } from "@vue/test-utils";
import TerminalContextMenu, {
  buildSearchUrl,
  CTX_SEARCH_MAX_ENGINES,
  DEFAULT_CTX_SEARCH_ENGINES_TEXT,
  parseCtxSearchEnginesText,
  type CtxSearchEngine,
} from "./TerminalContextMenu.vue";

const engines: CtxSearchEngine[] = [
  { name: "Google", urlTemplate: "https://www.google.com/search?q=%s" },
  { name: "Bing", urlTemplate: "https://www.bing.com/search?q=%s" },
];
const t = (key: string) => key;

// reka 的 ContextMenu* 在 happy-dom 下 portal/presence 不落定：渲染测试用
// 轻量 stub 替换 ui/context-menu 三件套（保留 slot、disabled attr 与 select 事件），
// 组件自身的条目组合、两页视图与 emit 语义仍然被真实覆盖。
const contextMenuStubs = {
  ContextMenuContent: { template: "<div data-stub=\"content\"><slot /></div>" },
  ContextMenuItem: {
    emits: ["select"],
    template: "<div data-stub=\"item\" @click=\"$emit('select')\"><slot /></div>",
  },
  ContextMenuSeparator: { template: "<hr data-stub=\"separator\">" },
};

function mountMenu(props: Record<string, unknown> = {}) {
  return mount(TerminalContextMenu, {
    props: { open: true, hasSelection: true, canPaste: true, engines, t, ...props },
    global: { stubs: contextMenuStubs },
    attachTo: document.body,
  });
}

function itemTexts(wrapper: ReturnType<typeof mountMenu>) {
  return wrapper.findAll('[data-stub="item"]').map((item) => item.text().trim());
}

describe("parseCtxSearchEnginesText", () => {
  it("parses default google line", () => {
    expect(parseCtxSearchEnginesText(DEFAULT_CTX_SEARCH_ENGINES_TEXT)).toEqual([
      { name: "Google", urlTemplate: "https://www.google.com/search?q=%s" },
    ]);
  });

  it("accepts multiple engines, comments and surrounding whitespace", () => {
    const text = [
      "# 搜索引擎",
      "  Google | https://www.google.com/search?q=%s  ",
      "",
      "Bing|https://www.bing.com/search?q=%s",
    ].join("\n");
    expect(parseCtxSearchEnginesText(text).map((engine) => engine.name)).toEqual(["Google", "Bing"]);
  });

  it("drops invalid lines: missing separator, non-http template, missing %s placeholder", () => {
    const text = ["no-separator", "Empty|", "ftp://x|ftp://example.com/%s", "NoPlace|https://example.com", "Ok|https://example.com/?q=%s"].join("\n");
    expect(parseCtxSearchEnginesText(text)).toEqual([{ name: "Ok", urlTemplate: "https://example.com/?q=%s" }]);
  });

  it("keeps the first engine on duplicate names (case-insensitive) and caps the list", () => {
    const duplicated = ["A|https://a.example/?q=%s", "a|https://b.example/?q=%s"].join("\n");
    expect(parseCtxSearchEnginesText(duplicated)).toHaveLength(1);
    expect(parseCtxSearchEnginesText(duplicated)[0].urlTemplate).toBe("https://a.example/?q=%s");
    const many = Array.from({ length: CTX_SEARCH_MAX_ENGINES + 5 }, (_, index) => `E${index}|https://e${index}.example/?q=%s`).join("\n");
    expect(parseCtxSearchEnginesText(many)).toHaveLength(CTX_SEARCH_MAX_ENGINES);
  });

  it("returns an empty list for garbage input", () => {
    expect(parseCtxSearchEnginesText("")).toEqual([]);
    expect(parseCtxSearchEnginesText("|||")).toEqual([]);
  });
});

describe("buildSearchUrl", () => {
  it("substitutes the first %s with the encoded selection", () => {
    expect(buildSearchUrl(engines[0], "a b&c")).toBe("https://www.google.com/search?q=a%20b%26c");
  });

  it("returns null when the template has no placeholder", () => {
    expect(buildSearchUrl({ name: "X", urlTemplate: "https://example.com" }, "q")).toBeNull();
  });
});

describe("TerminalContextMenu rendering", () => {
  it("renders copy/paste/search-online/close and switches to the engine list", async () => {
    const wrapper = mountMenu();
    const texts = itemTexts(wrapper);
    expect(texts).toContain("terminalCopy");
    expect(texts).toContain("terminalPaste");
    expect(texts).toContain("ctxSearch.menuLabel");
    expect(texts).toContain("close");
    // 点击 Search online 进入引擎页：显示返回项与引擎名，主页入口消失。
    const searchItem = wrapper.findAll('[data-stub="item"]').find((item) => item.text().includes("ctxSearch.menuLabel"))!;
    await searchItem.trigger("click");
    const after = itemTexts(wrapper);
    expect(after).toContain("ctxSearch.back");
    expect(after).toContain("Google");
    expect(after).toContain("Bing");
    expect(after).not.toContain("ctxSearch.menuLabel");
    // 选中引擎：按 name 上抛 search 事件。
    const engineItem = wrapper.findAll('[data-stub="item"]').find((item) => item.text().includes("Bing"))!;
    await engineItem.trigger("click");
    expect(wrapper.emitted("search")?.[0]).toEqual([engines[1]]);
    wrapper.unmount();
  });

  it("disables copy and search online without a selection", () => {
    const wrapper = mountMenu({ hasSelection: false });
    const disabled = wrapper.findAll('[data-stub="item"]').filter((item) => item.attributes("disabled") !== undefined).map((item) => item.text().trim());
    expect(disabled).toContain("terminalCopy");
    expect(disabled).toContain("ctxSearch.menuLabel");
    wrapper.unmount();
  });

  it("hides the search-online entry when no engines resolve", () => {
    const wrapper = mountMenu({ engines: [] });
    const texts = itemTexts(wrapper);
    expect(texts).not.toContain("ctxSearch.menuLabel");
    expect(texts).toContain("close");
    wrapper.unmount();
  });
});
