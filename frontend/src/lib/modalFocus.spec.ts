// @vitest-environment happy-dom
// pickModalFocusTarget DOM 行为测试（弹层初始聚焦：autofocus 标记优先，
// 否则首个可聚焦控件；disabled/hidden 跳过，空容器返回 null）。
import { describe, expect, it } from "vitest";

import { pickModalFocusTarget } from "./modalFocus";

function buildModal(html: string): HTMLElement {
  // Phase 6 起弹窗壳为 reka Dialog：内容元素带 data-slot="dialog-content"。
  const content = document.createElement("div");
  content.dataset.slot = "dialog-content";
  content.className = "modal small-modal";
  content.innerHTML = html;
  document.body.appendChild(content);
  return content;
}

describe("pickModalFocusTarget", () => {
  it("prefers the element marked with autofocus (native attribute is inert for Vue-inserted DOM)", () => {
    const modal = buildModal(`
      <button id="close">x</button>
      <input id="draft" autofocus />
      <button id="run">Run</button>
    `);
    expect(pickModalFocusTarget(modal)?.id).toBe("draft");
    modal.remove();
  });

  it("falls back to the first focusable control when nothing is marked", () => {
    // 删除确认弹层模式：无 autofocus 标记 → 首个可聚焦（header 关闭钮）。
    const modal = buildModal(`
      <header><button id="x">x</button></header>
      <footer><button id="cancel">Cancel</button><button id="delete">Delete</button></footer>
    `);
    expect(pickModalFocusTarget(modal)?.id).toBe("x");
    modal.remove();
  });

  it("skips disabled and hidden controls when picking the fallback", () => {
    const modal = buildModal(`
      <button id="off" disabled>off</button>
      <input id="secret" type="hidden" />
      <select id="kind"><option>a</option></select>
      <textarea id="note"></textarea>
    `);
    expect(pickModalFocusTarget(modal)?.id).toBe("kind");
    modal.remove();
  });

  it("skips a disabled autofocus element and returns null for empty containers", () => {
    const modal = buildModal(`
      <input id="busy" autofocus disabled />
      <button id="ok">OK</button>
    `);
    expect(pickModalFocusTarget(modal)?.id).toBe("ok");
    modal.remove();
    expect(pickModalFocusTarget(null)).toBeNull();
    const empty = buildModal("<p>loading</p>");
    expect(pickModalFocusTarget(empty)).toBeNull();
    empty.remove();
  });
});
