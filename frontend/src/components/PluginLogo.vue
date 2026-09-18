<script setup lang="ts">
// 直接引用插件真实 logo（单一来源：assets/plugin.svg，宿主插件中心同款图形）。
// 原文件硬编码的 #0ea5e9 描边落在 --primary 徽标底上不可读，由 style.css 的
// .plugin-logo svg path 规则把 stroke 压成 currentColor（表示属性可被 CSS
// 规则覆盖）——形状与宿主展示一致，颜色随使用处主题自适应。
import pluginLogo from "../../../assets/plugin.svg?raw";
</script>

<template>
  <span class="plugin-logo" aria-hidden="true" v-html="pluginLogo" />
</template>
