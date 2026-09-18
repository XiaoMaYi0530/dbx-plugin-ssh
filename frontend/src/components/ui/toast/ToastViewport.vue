<script setup lang="ts">
import type { ToastViewportProps } from "reka-ui";
import type { HTMLAttributes } from "vue";
import { reactiveOmit } from "@vueuse/core";
import { ToastViewport, useForwardProps } from "reka-ui";
import { cn } from "../../../lib/utils";

defineOptions({
  inheritAttrs: false,
});

const props = withDefaults(defineProps<ToastViewportProps & { class?: HTMLAttributes["class"] }>(), {
  // SSH 插件内 F8 必须直达 PTY，禁掉 reka 默认的 viewport 聚焦热键。
  hotkey: () => [],
});

const delegatedProps = reactiveOmit(props, "class");
const forwarded = useForwardProps(delegatedProps);
</script>

<template>
  <!-- 定位/层级由插件 viewport 类（unlayered，style.css）负责，wrapper 不带几何样式。 -->
  <ToastViewport data-slot="toast-viewport" v-bind="{ ...$attrs, ...forwarded }" :class="cn(props.class)" />
</template>
