/**
 * Modal initial-focus primitive.
 *
 * Native `autofocus` is inert for Vue-inserted DOM, so App.vue drives the
 * focus explicitly: on modal open the element marked with the `autofocus`
 * attribute (a positioning hint only at that point) gets focused, falling
 * back to the first interactive control. Tab trapping and Escape handling
 * are reka Dialog's job; only this pick decision stays unit-testable here.
 */

/** 容器内可聚焦元素选择器（disabled / hidden input / tabindex=-1 除外）。 */
const FOCUSABLE_SELECTOR = [
  "a[href]",
  "button:not([disabled])",
  'input:not([disabled]):not([type="hidden"])',
  "select:not([disabled])",
  "textarea:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
].join(", ");

/** 容器内文档顺序的可聚焦元素列表。 */
function focusableElements(root: ParentNode): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR));
}

/**
 * 弹层打开时的初始聚焦目标：带 `autofocus` 属性的元素优先（模板里保留的
 * 原生属性此时仅作聚焦定位提示，浏览器动态插入不会自动生效），否则容器内
 * 首个可聚焦控件；容器为空或没有任何可聚焦元素时返回 null。
 */
export function pickModalFocusTarget(container: ParentNode | null): HTMLElement | null {
  if (!container) return null;
  const marked = Array.from(container.querySelectorAll<HTMLElement>("[autofocus]")).find(
    (el) => !el.hasAttribute("disabled") && !el.hasAttribute("hidden"),
  );
  return marked ?? focusableElements(container)[0] ?? null;
}
