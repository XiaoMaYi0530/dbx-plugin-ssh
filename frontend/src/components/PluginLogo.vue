<script setup lang="ts">
// 插件品牌标记：与 assets/plugin.svg 同形（SSH 折线 + 光标横杠）。
// 描边走 currentColor 以适配徽标底色与主题（原文件硬编码的天蓝在
// --primary 底上不可读）；改形请与 assets/plugin.svg 两处同步。
</script>

<template>
  <svg class="plugin-logo" viewBox="0 0 64 64" aria-hidden="true">
    <path d="M9 10 29 32 9 54" fill="none" stroke="currentColor" stroke-width="10" stroke-linecap="round" stroke-linejoin="round" />
    <path d="M37 53h18" fill="none" stroke="currentColor" stroke-width="10" stroke-linecap="round" />
  </svg>
</template>
