<script setup lang="ts">
import type { ToastCloseProps } from "reka-ui";
import type { HTMLAttributes } from "vue";
import { reactiveOmit } from "@vueuse/core";
import { ToastClose, useForwardProps } from "reka-ui";
import { cn } from "../../../lib/utils";

defineOptions({
  inheritAttrs: false,
});

const props = defineProps<ToastCloseProps & { class?: HTMLAttributes["class"] }>();

const delegatedProps = reactiveOmit(props, "class");
const forwarded = useForwardProps(delegatedProps);
</script>

<template>
  <ToastClose data-slot="toast-close" v-bind="{ ...$attrs, ...forwarded }" :class="cn(props.class)">
    <slot />
  </ToastClose>
</template>
