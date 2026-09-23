<script lang="ts">
// 纯解析/构造助手放模块作用域：设置页解析与搜索 URL 构造可被单测直接导入，
// 不需要挂载组件（reka 弹层在测试环境渲染进 portal，断言成本高）。

/** 在线搜索引擎：urlTemplate 中 %s 为选中文本占位（encodeURIComponent 后代入）。 */
export interface CtxSearchEngine {
  name: string;
  urlTemplate: string;
}

export const CTX_SEARCH_ENGINE_NAME_MAX = 40;
export const CTX_SEARCH_TEMPLATE_MAX = 400;
export const CTX_SEARCH_MAX_ENGINES = 12;
export const DEFAULT_CTX_SEARCH_ENGINES_TEXT = "Google|https://www.google.com/search?q=%s";

/**
 * 设置页的原始文本（每行 `name|url_template`，# 开头注释）解析为引擎表。
 * 非法行静默丢弃：缺分隔符、空名、模板非 http(s)、缺 %s 占位、重名（保留首个）、
 * 超过上限截断。解析永远成功（最坏返回空表 → 菜单隐藏 Search online 项）。
 */
export function parseCtxSearchEnginesText(text: string): CtxSearchEngine[] {
  const engines: CtxSearchEngine[] = [];
  const seen = new Set<string>();
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line || line.startsWith("#")) continue;
    const separator = line.indexOf("|");
    if (separator <= 0) continue;
    const name = line.slice(0, separator).trim().slice(0, CTX_SEARCH_ENGINE_NAME_MAX);
    const urlTemplate = line.slice(separator + 1).trim().slice(0, CTX_SEARCH_TEMPLATE_MAX);
    if (!name || !urlTemplate) continue;
    if (!/^https?:\/\//i.test(urlTemplate) || !urlTemplate.includes("%s")) continue;
    const key = name.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    engines.push({ name, urlTemplate });
    if (engines.length >= CTX_SEARCH_MAX_ENGINES) break;
  }
  return engines;
}

/** 把选中文本代入模板首个 %s；模板缺占位符时返回 null（调用方静默放弃）。 */
export function buildSearchUrl(engine: CtxSearchEngine, query: string): string | null {
  if (!engine.urlTemplate.includes("%s")) return null;
  return engine.urlTemplate.replace("%s", encodeURIComponent(query));
}
</script>

<script setup lang="ts">
import { ref, watch } from "vue";
import { ArrowLeft, ClipboardPaste, Copy, Globe, Search, X } from "@lucide/vue";
import { ContextMenuContent, ContextMenuItem, ContextMenuSeparator } from "./ui/context-menu";

// 终端右键菜单内容（IMPL_PLAN Task P2-8）：Copy / Paste / Search online / Close
// 为组件自有项，插件域的其他菜单项（本地最近命令、全选、搜索面板、清屏、
// sudo、传输上传）由 App.vue 经默认插槽注入，保持在既有位置。
// 挂载点仍在 App.vue 的 reka <ContextMenu> 根内：contextmenu 事件、右键行为
// 四档（off/menu/paste/clipboard）与 preventDefault 语义由既有 showTerminalMenu 承担。
const props = defineProps<{
  /** 菜单开关镜像：关闭时把两页视图复位回主页。 */
  open: boolean;
  hasSelection: boolean;
  canPaste: boolean;
  engines: CtxSearchEngine[];
  t: (key: string, values?: Record<string, string | number>) => string;
}>();

const emit = defineEmits<{
  (e: "copy"): void;
  (e: "paste"): void;
  (e: "search", engine: CtxSearchEngine): void;
  (e: "close"): void;
}>();

const t = props.t;

// "Search online" 用两页视图承载引擎子列表：ui/context-menu wrapper 尚未引入
// reka 的 Sub 系列（ContextMenuSub*），MVP 以页内切换替代嵌套子菜单。
const view = ref<"main" | "engines">("main");
watch(
  () => props.open,
  (open) => {
    if (!open) view.value = "main";
  },
);
</script>

<template>
  <ContextMenuContent>
    <ContextMenuItem :disabled="!hasSelection" @select="emit('copy')"><Copy />{{ t("terminalCopy") }}</ContextMenuItem>
    <ContextMenuItem :disabled="!canPaste" @select="emit('paste')"><ClipboardPaste />{{ t("terminalPaste") }}</ContextMenuItem>
    <template v-if="engines.length">
      <ContextMenuSeparator />
      <ContextMenuItem v-if="view === 'main'" :disabled="!hasSelection" @select="view = 'engines'"><Search />{{ t("ctxSearch.menuLabel") }}</ContextMenuItem>
      <template v-else>
        <ContextMenuItem @select="view = 'main'"><ArrowLeft />{{ t("ctxSearch.back") }}</ContextMenuItem>
        <ContextMenuItem v-for="engine in engines" :key="engine.name" @select="emit('search', engine)"><Globe />{{ engine.name }}</ContextMenuItem>
      </template>
    </template>
    <!-- 插件自有菜单项经默认插槽注入（位置：Search online 与 Close 之间）。 -->
    <slot />
    <ContextMenuSeparator />
    <ContextMenuItem @select="emit('close')"><X />{{ t("close") }}</ContextMenuItem>
  </ContextMenuContent>
</template>
