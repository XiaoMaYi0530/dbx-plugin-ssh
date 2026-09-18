<script setup lang="ts">
import type { ToastActionProps } from "reka-ui";
import type { HTMLAttributes } from "vue";
import { reactiveOmit } from "@vueuse/core";
import { ToastAction, useForwardProps } from "reka-ui";
import { cn } from "../../../lib/utils";

defineOptions({
  inheritAttrs: false,
});

// altText 为 reka 必填（toast 关闭后动作仍可达的说明文本），调用方用动作标签本身。
const props = defineProps<ToastActionProps & { class?: HTMLAttributes["class"] }>();

const delegatedProps = reactiveOmit(props, "class");
const forwarded = useForwardProps(delegatedProps);
</script>

<template>
  <ToastAction data-slot="toast-action" v-bind="{ ...$attrs, ...forwarded }" :class="cn(props.class)">
    <slot />
  </ToastAction>
</template>
