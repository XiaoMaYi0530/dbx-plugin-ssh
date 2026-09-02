<script setup lang="ts">
// SFTP 目录树（侧栏 tree tab，对标 files 插件 DirTree.vue）：递归渲染懒加载
// 节点。单击行 = 面板进入该目录，caret = 展开/收缩（首次展开经 App.vue 拉
// sftp/list），右键上抛统一侧栏菜单。
import { ChevronDown, ChevronRight, Folder, FolderOpen } from "@lucide/vue";
import type { DirTreeNode } from "../lib/sftpDirTree";

defineProps<{
  nodes: DirTreeNode[];
  depth: number;
  currentPath: string;
  t: (key: string, values?: Record<string, string | number>) => string;
}>();

const emit = defineEmits<{
  (event: "toggle", node: DirTreeNode): void;
  (event: "open", node: DirTreeNode): void;
  (event: "context", payload: { node: DirTreeNode; x: number; y: number }): void;
}>();
</script>

<template>
  <template v-for="node in nodes" :key="node.path">
    <div
      class="sftp-tree-row"
      :class="{ 'is-current': node.path === currentPath }"
      :style="{ paddingLeft: `${6 + depth * 12}px` }"
      :title="node.path"
      @click="emit('open', node)"
      @contextmenu.prevent.stop="emit('context', { node, x: $event.clientX, y: $event.clientY })"
    >
      <button type="button" class="sftp-tree-caret" @click.stop="emit('toggle', node)">
        <span v-if="node.loading" class="sftp-tree-spinner" />
        <ChevronDown v-else-if="node.expanded" />
        <ChevronRight v-else />
      </button>
      <FolderOpen v-if="node.expanded" class="sftp-tree-dir-icon" />
      <Folder v-else class="sftp-tree-dir-icon" />
      <span class="sftp-tree-name">{{ node.name === "/" ? t("sftpSide.root") : node.name }}</span>
    </div>
    <DirTree
      v-if="node.expanded && node.children.length"
      :nodes="node.children"
      :depth="depth + 1"
      :current-path="currentPath"
      :t="t"
      @toggle="emit('toggle', $event)"
      @open="emit('open', $event)"
      @context="emit('context', $event)"
    />
  </template>
</template>
