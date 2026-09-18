<script setup lang="ts">
// SFTP 目录树（侧栏 tree tab，对标 files 插件 DirTree.vue）：递归渲染懒加载
// 节点。单击行 = 面板进入该目录，caret = 展开/收缩（首次展开经 App.vue 拉
// sftp/list），右键上抛统一侧栏菜单。
import { ChevronDown, ChevronRight, Folder, FolderOpen } from "@lucide/vue";
import type { DirTreeNode } from "../lib/sftpDirTree";

const props = defineProps<{
  nodes: DirTreeNode[];
  depth: number;
  currentPath: string;
  t: (key: string, values?: Record<string, string | number>) => string;
}>();

const emit = defineEmits<{
  (event: "toggle", node: DirTreeNode): void;
  (event: "open", node: DirTreeNode): void;
  (event: "context", payload: { node: DirTreeNode }): void;
}>();

// R3-P2-6：树行键盘可达——roving tabindex（当前目录行 tabindex=0，其余 -1；
// 本层无当前行时首行兜底为 0，保证 Tab 始终可达）+ role="treeitem"。
function tabIndexFor(node: DirTreeNode, index: number): number {
  if (node.path === props.currentPath) return 0;
  const hasCurrent = props.nodes.some((candidate) => candidate.path === props.currentPath);
  return !hasCurrent && index === 0 ? 0 : -1;
}

function caretName(node: DirTreeNode): string {
  const label = node.name === "/" ? props.t("sftpSide.root") : node.name;
  return node.expanded ? props.t("sftpSide.collapseNode", { name: label }) : props.t("sftpSide.expandNode", { name: label });
}

// 方向键 roving 导航：树行分散在递归实例里，按 DOM 顺序在全部 .sftp-tree-row
// 中找相邻行移焦（无相邻行时不拦截，保持滚动等默认行为）。
function moveRowFocus(event: KeyboardEvent, offset: number) {
  const rows = Array.from(document.querySelectorAll<HTMLElement>(".sftp-tree-row"));
  const index = rows.indexOf(event.currentTarget as HTMLElement);
  const next = index >= 0 ? rows[index + offset] : undefined;
  if (next) {
    event.preventDefault();
    next.focus();
  }
}

function onRowKeydown(event: KeyboardEvent, node: DirTreeNode) {
  if (event.key === "Enter") {
    event.preventDefault();
    emit("open", node);
    return;
  }
  if (event.key === " ") {
    event.preventDefault();
    emit("toggle", node);
    return;
  }
  if (event.key === "ArrowDown") moveRowFocus(event, 1);
  else if (event.key === "ArrowUp") moveRowFocus(event, -1);
}
</script>

<template>
  <template v-for="(node, index) in nodes" :key="node.path">
    <div
      class="sftp-tree-row"
      :class="{ 'is-current': node.path === currentPath }"
      :style="{ paddingLeft: `${6 + depth * 12}px` }"
      :title="node.path"
      role="treeitem"
      :aria-expanded="node.expanded ? 'true' : 'false'"
      :tabindex="tabIndexFor(node, index)"
      @click="emit('open', node)"
      @keydown="onRowKeydown($event, node)"
      @contextmenu="emit('context', { node })"
    >
      <button type="button" class="sftp-tree-caret" :aria-label="caretName(node)" @click.stop="emit('toggle', node)">
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
