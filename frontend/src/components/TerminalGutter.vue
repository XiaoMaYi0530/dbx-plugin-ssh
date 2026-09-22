<script setup lang="ts">
// 终端左侧 gutter（P1-3）：行号 / 时间戳列。纯呈现组件——行数据由 App.vue 从
// computeGutterRows 算好传入（onRender/onScroll/onResize 触发、rAF 节流在两侧
// 各兜一层），这里只做一次"每帧最多提交一次"的渲染节流。
// 绝对定位覆盖在 .terminal-pane 左缘；xterm 侧经 --dbx-gutter-width 同步加宽
// 左内边距，gutter 永远只占 padding 环带，不遮终端文本。pointer-events 关闭，
// 点击/选择行为完全穿透给终端。
import { onBeforeUnmount, ref, watch } from "vue";
import type { GutterRow } from "../lib/terminalGutter";

const props = defineProps<{
  /** 已算好的 gutter 行（像素 top 与终端画布对齐）。 */
  rows: GutterRow[];
  /** gutter 宽（px），与 App.vue 下发的 --dbx-gutter-width 同值。 */
  width: number;
}>();

const renderedRows = ref<GutterRow[]>([]);
let frameId = 0;

// rAF 节流：大输出下 rows 引用可能一帧多次变化，只保留最后一次提交渲染。
watch(
  () => props.rows,
  (rows) => {
    if (frameId) return;
    frameId = window.requestAnimationFrame(() => {
      frameId = 0;
      renderedRows.value = rows;
    });
  },
  { immediate: true },
);

onBeforeUnmount(() => {
  if (frameId) cancelAnimationFrame(frameId);
});
</script>

<template>
  <div class="terminal-gutter" :style="{ width: `${width}px` }" aria-hidden="true">
    <span v-for="row in renderedRows" :key="row.top" class="terminal-gutter-row mono" :style="{ top: `${row.top}px` }">{{ row.text }}</span>
  </div>
</template>

<style scoped>
.terminal-gutter {
  position: absolute;
  z-index: 1; /* 终端宿主 z-index: 0，浮层 z-index: 2——gutter 夹在中间。 */
  top: 0;
  bottom: 0;
  left: 0;
  overflow: hidden;
  pointer-events: none;
  background: var(--ssh-terminal-background);
  border-right: 1px solid color-mix(in srgb, var(--border) 55%, transparent);
}
.terminal-gutter-row {
  position: absolute;
  left: 0;
  right: 0;
  text-align: right;
  padding-right: 6px;
  font-size: 11px;
  line-height: 1;
  color: var(--muted-foreground);
  white-space: pre;
  opacity: 0.85;
}
</style>
