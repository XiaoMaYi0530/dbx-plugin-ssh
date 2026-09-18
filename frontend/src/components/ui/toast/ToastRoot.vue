<script setup lang="ts">
import type { ToastRootEmits, ToastRootProps } from "reka-ui";
import type { HTMLAttributes } from "vue";
import { reactiveOmit } from "@vueuse/core";
import { ToastRoot, useForwardPropsEmits } from "reka-ui";
import { cn } from "../../../lib/utils";

defineOptions({
  inheritAttrs: false,
});

const props = defineProps<ToastRootProps & { class?: HTMLAttributes["class"] }>();
const emits = defineEmits<ToastRootEmits>();

const delegatedProps = reactiveOmit(props, "class");
const forwarded = useForwardPropsEmits(delegatedProps, emits);
</script>

<template>
  <!-- 视觉样式由插件横幅类（.notice / .error-banner，unlayered）提供。 -->
  <ToastRoot data-slot="toast" v-bind="{ ...$attrs, ...forwarded }" :class="cn(props.class)">
    <slot />
  </ToastRoot>
</template>
