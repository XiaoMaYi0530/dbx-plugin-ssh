<script setup lang="ts">
// 终端外观实时预览：把当前生效的配色/字体/间距/光标渲染成一段 `ls` 输出与
// 字体样张，让用户在设置页里直接看到效果，而不是改完再去终端里验证。
//
// 刻意不复用真实 xterm 实例：预览需要随草稿即时重渲染，实例化 xterm 会带来
// fit 重排、WebGL context 占用与主题切换闪烁；这里用纯 DOM 复刻同一套色板
// 语义（16 色 + 前景/背景/光标/选区），预览与外层终端在数据源上同构
// （都来自 applySchemeToTerminalTheme 的结果），不会出现「预览好看实际不对」。
import { computed } from "vue";
import type { TerminalCursorStyle } from "../lib/terminalAppearance";
import type { TerminalThemeLike } from "../lib/terminalScheme";

const props = defineProps<{
  theme: TerminalThemeLike;
  fontFamily: string;
  fontSize: number;
  fontWeight: number | "normal" | "bold";
  fontWeightBold: number | "normal" | "bold";
  lineHeight: number;
  letterSpacing: number;
  cursorStyle: TerminalCursorStyle;
  cursorBlink: boolean;
  /** 方案前景/背景对比度（WCAG），低于 4.5 时提示可读性风险。 */
  contrastRatio: number;
  t: (key: string, values?: Record<string, string | number>) => string;
}>();

/** 取主题字段；缺字段时回退前景色，避免出现「无色」的空白块。 */
function c(key: string): string {
  return props.theme[key] ?? props.theme.foreground;
}

const shellStyle = computed(() => ({
  background: props.theme.background,
  color: props.theme.foreground,
  fontFamily: props.fontFamily,
  fontSize: `${Math.max(9, Math.min(props.fontSize, 20))}px`,
  fontWeight: String(props.fontWeight),
  lineHeight: String(props.lineHeight),
  letterSpacing: `${props.letterSpacing}px`,
}));

// 粗体样张用主题的 fontWeightBold 与 brightWhite 组合，直接暴露「粗体是否
// 走亮色」与粗体字重的差异。
const boldStyle = computed(() => ({ fontWeight: String(props.fontWeightBold), color: c("brightWhite") }));

const cursorStyle = computed(() => {
  const height = "1.05em";
  const base = { background: c("cursor"), display: "inline-block", verticalAlign: "text-bottom" };
  if (props.cursorStyle === "block") return { ...base, width: "0.62em", height };
  if (props.cursorStyle === "underline") return { ...base, width: "0.62em", height: "2px" };
  return { ...base, width: "2px", height };
});

// 选区底色直接取合成后的 selectionBackground：用户在方案里换色能立刻看到，
// 也顺带暴露「浅底方案用深色选区」这类不协调的组合。
const selectionStyle = computed(() => ({
  background: props.theme.selectionBackground,
  color: props.theme.foreground,
}));

const dimStyle = computed(() => ({ color: c("brightBlack") }));
const lowContrast = computed(() => props.contrastRatio < 4.5);
</script>

<template>
  <div class="appearance-preview" :style="shellStyle">
    <div class="preview-line">
      <span :style="{ color: c('brightGreen') }">sshuser@hktkosl</span><span :style="dimStyle">:</span><span :style="{ color: c('brightBlue') }">~/deploy</span><span>$ </span><span>ls -la</span><span :class="{ blink: cursorBlink }" :style="cursorStyle" />
    </div>
    <div class="preview-line" :style="dimStyle">total 48</div>
    <div class="preview-line">drwxr-xr-x 6 root root 4096 Sep 22 10:12 <span :style="{ color: c('brightBlue') }">.</span></div>
    <div class="preview-line">-rw-r--r-- 1 root root 1240 Sep 22 09:58 <span :style="{ color: c('yellow') }">README.md</span></div>
    <div class="preview-line">-rwxr-xr-x 1 root root 8192 Sep 20 07:44 <span :style="{ color: c('brightGreen'), fontWeight: String(fontWeightBold) }">deploy.sh</span></div>
    <div class="preview-line">lrwxrwxrwx 1 root root 9 Sep 18 12:02 <span :style="{ color: c('cyan') }">current</span> -&gt; <span :style="{ color: c('magenta') }">releases</span></div>
    <div class="preview-line"><span :style="{ color: c('red') }">-rw-r--r-- 1 root root 0 Sep 12 03:10 failed.log</span></div>
    <div class="preview-line"><span :style="selectionStyle">&nbsp;npm run build&nbsp;</span> <span :style="dimStyle"># selection</span></div>
    <div class="preview-line"><span :style="{ color: c('brightMagenta') }">✓</span> bundled 1.2s <span :style="{ color: c('brightYellow') }">△ 2 warnings</span> <span :style="{ color: c('brightRed'), fontWeight: String(fontWeightBold) }">✗ 1 error</span></div>
    <div class="preview-line">
      <span :style="dimStyle">0123456789</span>
      <span> ABCdefg</span>
      <span :style="boldStyle"> BOLD</span>
      <span> →{}[]#$%&amp; | 中文字宽 </span>
      <span :style="dimStyle">normal</span>
    </div>
    <p v-if="lowContrast" class="preview-warning">{{ t("terminalAppearance.lowContrast", { ratio: contrastRatio.toFixed(1) }) }}</p>
  </div>
</template>

<style scoped>
.appearance-preview {
  border: 1px solid var(--border);
  border-radius: 6px;
  /* 预览自带内边距：与终端内的 padding 设置无关，避免用户把内边距调到 0 后
     预览贴边看不出字号/行高变化。 */
  padding: 10px 12px;
  overflow: hidden;
  white-space: pre;
  font-variant-ligatures: none;
}
.preview-line { min-height: 1em; }
.blink { animation: preview-cursor-blink 1.1s steps(2, start) infinite; }
.preview-warning { margin: 8px 0 0; white-space: normal; font-size: 11px; color: var(--destructive); }
@keyframes preview-cursor-blink { 50% { opacity: 0; } }
@media (prefers-reduced-motion: reduce) { .blink { animation: none; } }
</style>
