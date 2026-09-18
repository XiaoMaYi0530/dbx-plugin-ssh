<script setup lang="ts">
import type { DialogOverlayProps } from "reka-ui";
import type { HTMLAttributes } from "vue";
import { reactiveOmit } from "@vueuse/core";
import { DialogOverlay } from "reka-ui";
import { cn } from "../../../lib/utils";

const props = defineProps<DialogOverlayProps & { class?: HTMLAttributes["class"] }>();

const delegatedProps = reactiveOmit(props, "class");
</script>

<template>
  <!-- 遮罩色用插件运行时外观变量（--color-overlay → --overlay），z 对齐弹层阶梯 modal(80)。 -->
  <DialogOverlay data-slot="dialog-overlay" v-bind="delegatedProps" :class="cn('data-open:animate-in data-closed:animate-out data-closed:fade-out-0 data-open:fade-in-0 bg-overlay duration-100 fixed inset-0 isolate z-[80]', props.class)">
    <slot />
  </DialogOverlay>
</template>
