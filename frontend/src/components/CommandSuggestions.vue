<script setup lang="ts">
// 命令模糊建议浮层（P1-1）：纯展示组件——条目渲染（匹配高亮 + 来源图标）、
// 键盘导航与填充语义都在 App（键盘经 xterm attachCustomKeyEventHandler 消费，
// 浮层只反映 activeIndex 并把点击/悬停上抛）。定位由 App 传入：锚点是光标
// 像素坐标，读不到时（anchor=null）降级为贴终端底部。
import { computed } from "vue";
import { History, Zap } from "@lucide/vue";
import type { CommandSuggestion } from "../lib/commandSuggestions";

interface Segment {
  text: string;
  matched: boolean;
}

const props = defineProps<{
  items: CommandSuggestion[];
  activeIndex: number;
  /** 光标像素坐标（相对终端宿主）；null = 定位不可用，贴终端底部。 */
  anchor: { x: number; y: number } | null;
  t: (key: string, values?: Record<string, string | number>) => string;
}>();

const emit = defineEmits<{
  activate: [index: number];
  fill: [item: CommandSuggestion];
}>();

const style = computed(() => {
  if (!props.anchor) return undefined;
  return { left: `${Math.max(0, props.anchor.x)}px`, top: `${Math.max(0, props.anchor.y)}px` };
});

/** 按 indices 把命令串切成高亮片段（未命中的字符合并成段）。 */
function segments(command: string, indices: number[]): Segment[] {
  const matched = new Set(indices);
  const out: Segment[] = [];
  let current: Segment | null = null;
  for (let i = 0; i < command.length; i += 1) {
    const isMatched = matched.has(i);
    if (!current || current.matched !== isMatched) {
      current = { text: command.charAt(i), matched: isMatched };
      out.push(current);
    } else {
      current.text += command.charAt(i);
    }
  }
  return out;
}

const sourceLabel = (item: CommandSuggestion) => (item.source === "quick" ? props.t("suggestions.sourceQuick") : props.t("suggestions.sourceHistory"));
</script>

<template>
  <div class="command-suggestions" :class="{ 'anchor-fallback': anchor === null }" :style="style" role="listbox" :aria-label="t('suggestions.title')">
    <button
      v-for="(item, index) in items"
      :key="`${item.source}-${item.command}`"
      type="button"
      class="suggestion-row"
      :class="{ active: index === activeIndex }"
      role="option"
      :aria-selected="index === activeIndex"
      :title="`${sourceLabel(item)} · ${t('suggestions.fillHint')}`"
      @mouseenter="emit('activate', index)"
      @mousedown.prevent
      @click="emit('fill', item)"
    >
      <History v-if="item.source === 'history'" class="suggestion-icon" aria-hidden="true" />
      <Zap v-else class="suggestion-icon" aria-hidden="true" />
      <span class="suggestion-command mono">
        <template v-for="(segment, segmentIndex) in segments(item.command, item.indices)" :key="segmentIndex"><mark v-if="segment.matched">{{ segment.text }}</mark><template v-else>{{ segment.text }}</template></template>
      </span>
      <span class="suggestion-source">{{ sourceLabel(item) }}</span>
    </button>
  </div>
</template>

<style scoped>
.command-suggestions {
  position: absolute;
  z-index: 30;
  max-width: 380px;
  min-width: 260px;
  max-height: 40vh;
  overflow-y: auto;
  background: var(--panel, #161b22);
  border: 1px solid var(--border, #30363d);
  border-radius: 8px;
  box-shadow: 0 8px 24px rgb(0 0 0 / 35%);
  padding: 4px;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.command-suggestions.anchor-fallback {
  left: 12px;
  bottom: 12px;
}

.suggestion-row {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  border: 0;
  background: transparent;
  color: inherit;
  text-align: left;
  padding: 4px 8px;
  border-radius: 6px;
  cursor: pointer;
  font-size: 12.5px;
  line-height: 1.4;
}

.suggestion-row.active {
  background: var(--accent, #2f6feb33);
}

.suggestion-icon {
  flex: none;
  width: 14px;
  height: 14px;
  opacity: 0.75;
}

.suggestion-command {
  flex: 1 1 auto;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.suggestion-command mark {
  background: transparent;
  color: var(--accent-fg, #4493f8);
  font-weight: 600;
}

.suggestion-source {
  flex: none;
  font-size: 11px;
  opacity: 0.65;
}
</style>
