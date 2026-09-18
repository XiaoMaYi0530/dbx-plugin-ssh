<script setup lang="ts">
import type { ToggleGroupRootEmits, ToggleGroupRootProps } from "reka-ui";
import type { HTMLAttributes } from "vue";
import { reactiveOmit } from "@vueuse/core";
import { ToggleGroupRoot, useForwardPropsEmits } from "reka-ui";
import { cn } from "../../../lib/utils";

const props = defineProps<ToggleGroupRootProps & { class?: HTMLAttributes["class"] }>();
const emits = defineEmits<ToggleGroupRootEmits>();

const delegatedProps = reactiveOmit(props, "class");

const forwarded = useForwardPropsEmits(delegatedProps, emits);
</script>

<template>
  <ToggleGroupRoot v-slot="slotProps" data-slot="toggle-group" v-bind="forwarded" :class="cn('group/toggle-group flex w-fit items-center', props.class)">
    <slot v-bind="slotProps" />
  </ToggleGroupRoot>
</template>
