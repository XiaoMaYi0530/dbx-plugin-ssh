<script setup lang="ts">
import type { PopoverContentEmits, PopoverContentProps } from "reka-ui";
import type { HTMLAttributes } from "vue";
import { reactiveOmit } from "@vueuse/core";
import { PopoverContent, PopoverPortal, useForwardPropsEmits } from "reka-ui";
import { cn } from "../../../lib/utils";

defineOptions({
  inheritAttrs: false,
});

const props = withDefaults(defineProps<PopoverContentProps & { class?: HTMLAttributes["class"] }>(), {
  align: "center",
  sideOffset: 4,
});
const emits = defineEmits<PopoverContentEmits>();

const delegatedProps = reactiveOmit(props, "class");

const forwarded = useForwardPropsEmits(delegatedProps, emits);
</script>

<template>
  <PopoverPortal>
    <!-- @focus-outside.prevent：子视图切换（如快捷命令编辑器）会卸载当前聚焦的
         按钮，焦点回落 document.body；reka DismissableLayer 对 focus-outside 默认
         dismiss，会把整个 popover 关掉。迁移前弹层只响应「外部点击 + Esc」，
         焦点外移从不关闭，这里阻止 dismiss 以恢复该语义（pointer-down-outside
         不受影响，外部点击/ Esc 关闭照常）。
         @click.stop：迁移前面板自带 @click.stop，内部点击从不到达 document 收口
         （onDocumentClickCloseMenus）。少了它时，点击处理器若在目标阶段移除自身
         元素（列表态 → 编辑器子视图切换），click 冒泡到 document 时 target 已
         脱离 DOM，closest('[data-slot="popover-content"]') 守卫失效、误关弹层。 -->
    <PopoverContent
      data-slot="popover-content"
      v-bind="{ ...$attrs, ...forwarded }"
      @focus-outside.prevent
      @click.stop
      :class="
        cn(
          'bg-popover text-popover-foreground data-open:animate-in data-closed:animate-out data-closed:fade-out-0 data-open:fade-in-0 data-closed:zoom-out-95 data-open:zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 ring-foreground/10 flex flex-col gap-2.5 rounded-md p-2.5 shadow-md ring-1 duration-100 z-50 w-72 origin-(--reka-popover-content-transform-origin) outline-hidden',
          props.class,
        )
      "
    >
      <slot />
    </PopoverContent>
  </PopoverPortal>
</template>
