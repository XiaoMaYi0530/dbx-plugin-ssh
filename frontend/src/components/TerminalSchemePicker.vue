<script setup lang="ts">
// 配色方案选择器（对标 Tabby 的 color-scheme-selector）：搜索 + 亮暗过滤 +
// 16 色色卡列表，点击即把方案写入当前选中的槽位（深色档 / 浅色档）。
//
// 独立成组件的原因：192 个内置方案 + 自定义方案的长列表会显著撑大设置弹窗的
// 模板与响应式状态，而它自身是自洽的（入参是 id + 目录，出参是「选了谁」），
// 与设置弹窗其余表单无耦合。
import { computed, ref } from "vue";
import { Search } from "@lucide/vue";
import { filterTerminalSchemes, schemeTone, type TerminalColorScheme } from "../lib/terminalScheme";

const props = defineProps<{
  darkSchemeId: string | null;
  lightSchemeId: string | null;
  customSchemes: readonly TerminalColorScheme[];
  schemes: readonly TerminalColorScheme[];
  hostFollowLabel: string;
  t: (key: string, values?: Record<string, string | number>) => string;
}>();

const emit = defineEmits<{
  (e: "pick", payload: { slot: "dark" | "light"; id: string | null }): void;
}>();

const slot = ref<"dark" | "light">("dark");
const query = ref("");
const tone = ref<"all" | "dark" | "light">("all");

const SLOT_HINT: Record<"dark" | "light", string> = {
  dark: "terminalAppearance.darkScheme",
  light: "terminalAppearance.lightScheme",
};

const currentId = computed(() => (slot.value === "dark" ? props.darkSchemeId : props.lightSchemeId));

// 槽位切换后清空过滤，避免「切槽位却看不到任何方案」的困惑。
function selectSlot(next: "dark" | "light") {
  slot.value = next;
  query.value = "";
  tone.value = "all";
}

const visible = computed(() => filterTerminalSchemes(props.schemes, { query: query.value, tone: tone.value }));

function swatchColors(scheme: TerminalColorScheme): string[] {
  return scheme.colors.slice(0, 16);
}
</script>

<template>
  <div class="scheme-picker">
    <div class="picker-slots" role="tablist">
      <button
        v-for="item in (['dark', 'light'] as const)"
        :key="item"
        type="button"
        role="tab"
        class="picker-slot"
        :class="{ active: slot === item }"
        :aria-selected="slot === item"
        @click="selectSlot(item)"
      >
        <span class="picker-slot-label">{{ t(SLOT_HINT[item]) }}</span>
        <span class="picker-slot-value">{{ item === 'dark' ? (darkSchemeId || hostFollowLabel) : (lightSchemeId || hostFollowLabel) }}</span>
      </button>
    </div>
    <div class="picker-search">
      <Search class="picker-search-icon" aria-hidden="true" />
      <input v-model="query" spellcheck="false" :placeholder="t('terminalAppearance.searchPlaceholder')" :aria-label="t('terminalAppearance.searchPlaceholder')" />
      <span class="picker-tones">
        <button
          v-for="item in (['all', 'dark', 'light'] as const)"
          :key="item"
          type="button"
          class="picker-tone"
          :class="{ active: tone === item }"
          @click="tone = item"
        >{{ t(item === 'all' ? 'terminalAppearance.toneAll' : item === 'dark' ? 'terminalAppearance.toneDark' : 'terminalAppearance.toneLight') }}</button>
      </span>
    </div>
    <ul class="picker-list" role="listbox">
      <li>
        <button
          type="button"
          class="picker-row picker-row--follow"
          :class="{ selected: currentId === null }"
          role="option"
          :aria-selected="currentId === null"
          @click="emit('pick', { slot, id: null })"
        >
          <span class="picker-name">{{ hostFollowLabel }}</span>
        </button>
      </li>
      <li v-for="scheme in visible" :key="scheme.id">
        <button
          type="button"
          class="picker-row"
          :class="{ selected: currentId === scheme.id }"
          role="option"
          :aria-selected="currentId === scheme.id"
          @click="emit('pick', { slot, id: scheme.id })"
        >
          <span class="picker-name" :title="scheme.name">{{ scheme.name }}</span>
          <span v-if="scheme.source === 'custom'" class="picker-badge">{{ t('terminalAppearance.importSection') }}</span>
          <span class="picker-swatch" aria-hidden="true">
            <i v-for="(color, index) in swatchColors(scheme)" :key="index" :style="{ background: color }" />
          </span>
          <span class="picker-tone-tag" :data-tone="schemeTone(scheme)">{{ t(schemeTone(scheme) === 'dark' ? 'terminalAppearance.toneDark' : 'terminalAppearance.toneLight') }}</span>
        </button>
      </li>
      <li v-if="!visible.length" class="picker-empty">{{ t("terminalAppearance.emptyFilter") }}</li>
    </ul>
  </div>
</template>

<style scoped>
.scheme-picker { display: flex; flex-direction: column; gap: 8px; min-height: 0; }
.picker-slots { display: grid; grid-template-columns: 1fr 1fr; gap: 6px; }
.picker-slot {
  display: flex; flex-direction: column; gap: 2px; align-items: flex-start;
  padding: 6px 8px; border: 1px solid var(--border); border-radius: var(--radius);
  background: var(--background); color: var(--foreground); cursor: pointer; text-align: left;
}
.picker-slot.active { border-color: var(--primary); background: color-mix(in srgb, var(--primary) 12%, var(--background)); }
.picker-slot-label { font-size: 11px; color: var(--muted-foreground); }
.picker-slot-value { font-size: 12px; font-weight: 600; }
.picker-search { display: flex; align-items: center; gap: 6px; }
.picker-search-icon { width: 13px; height: 13px; color: var(--muted-foreground); flex: 0 0 auto; }
.picker-search input { flex: 1 1 auto; min-width: 0; }
.picker-tones { display: flex; gap: 2px; flex: 0 0 auto; }
.picker-tone {
  padding: 2px 6px; font-size: 11px; border: 1px solid var(--border);
  border-radius: var(--radius); background: var(--background); color: var(--muted-foreground); cursor: pointer;
}
.picker-tone.active { border-color: var(--primary); color: var(--foreground); }
.picker-list {
  list-style: none; margin: 0; padding: 2px; display: flex; flex-direction: column; gap: 1px;
  max-height: 210px; overflow-y: auto; border: 1px solid var(--border); border-radius: var(--radius);
}
.picker-row {
  display: flex; align-items: center; gap: 8px; width: 100%;
  padding: 4px 6px; border: 1px solid transparent; border-radius: 4px;
  background: transparent; color: var(--foreground); cursor: pointer; text-align: left;
}
.picker-row:hover { background: var(--accent); }
.picker-row.selected { border-color: var(--primary); background: color-mix(in srgb, var(--primary) 12%, transparent); }
.picker-name { flex: 1 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 12px; }
.picker-badge { flex: 0 0 auto; font-size: 10px; color: var(--muted-foreground); border: 1px solid var(--border); border-radius: 3px; padding: 0 3px; }
.picker-swatch { flex: 0 0 auto; display: inline-flex; gap: 1px; }
.picker-swatch i { display: block; width: 7px; height: 12px; border-radius: 1px; }
.picker-tone-tag { flex: 0 0 auto; font-size: 10px; color: var(--muted-foreground); width: 28px; text-align: right; }
.picker-empty { padding: 10px; text-align: center; font-size: 12px; color: var(--muted-foreground); }
</style>
