<script setup lang="ts">
import type { ToggleGroupItemProps } from "reka-ui";
import type { HTMLAttributes } from "vue";
import { reactiveOmit } from "@vueuse/core";
import { ToggleGroupItem, useForwardProps } from "reka-ui";
import { cn } from "../../../lib/utils";

const props = defineProps<ToggleGroupItemProps & { class?: HTMLAttributes["class"] }>();

const delegatedProps = reactiveOmit(props, "class");

const forwardedProps = useForwardProps(delegatedProps);
</script>

<template>
  <ToggleGroupItem
    data-slot="toggle-group-item"
    v-bind="forwardedProps"
    :class="cn('inline-flex shrink-0 cursor-pointer select-none items-center justify-center transition-colors outline-none data-disabled:pointer-events-none data-disabled:opacity-50', props.class)"
  >
    <slot />
  </ToggleGroupItem>
</template>
