<script setup lang="ts">
import type { ContextMenuTriggerProps } from "reka-ui";
import type { HTMLAttributes } from "vue";
import { reactiveOmit } from "@vueuse/core";
import { ContextMenuTrigger, useForwardProps } from "reka-ui";
import { cn } from "../../../lib/utils";

const props = defineProps<ContextMenuTriggerProps & { class?: HTMLAttributes["class"] }>();

const delegatedProps = reactiveOmit(props, "class");
const forwardedProps = useForwardProps(delegatedProps);
</script>

<template>
  <!-- 不带 shadcn 的 select-none：触发区常包裹终端/文件列表等需要文本选择的区域。 -->
  <ContextMenuTrigger data-slot="context-menu-trigger" v-bind="forwardedProps" :class="cn(props.class)">
    <slot />
  </ContextMenuTrigger>
</template>
