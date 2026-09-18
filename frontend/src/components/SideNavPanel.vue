<script setup lang="ts">
// SFTP 侧栏导航面板（对标 files 插件 SideNavPanel.vue）：tree（目录树，默认）/
// quick（快捷路径）双 tab，可收起为窄条再展开；tab 与收缩状态由 App.vue 持久化
// 到 localStorage。行右键统一上抛 node-context（打开 / 复制路径 / 复制文件名 /
// 压缩），由 App.vue 弹菜单。
import { ChevronsLeft, ChevronsRight, Folder, FolderTree, Home, RefreshCw, Star } from "@lucide/vue";
import DirTree from "./DirTree.vue";
import { Tabs, TabsList, TabsTrigger } from "./ui/tabs";
import type { DirTreeNode } from "../lib/sftpDirTree";

export interface SftpSideQuickPath {
  path: string;
  /** 展示文案（home 项用本地化「主目录」，其余直接展示路径）。 */
  label: string;
  home?: boolean;
}

defineProps<{
  tab: "tree" | "quick";
  collapsed: boolean;
  treeRoot: DirTreeNode | null;
  quickPaths: SftpSideQuickPath[];
  currentPath: string;
  t: (key: string, values?: Record<string, string | number>) => string;
}>();

const emit = defineEmits<{
  (event: "update:tab", tab: "tree" | "quick"): void;
  (event: "update:collapsed", collapsed: boolean): void;
  (event: "navigate", path: string): void;
  (event: "toggle-node", node: DirTreeNode): void;
  (event: "refresh-tree"): void;
  (event: "node-context", payload: { path: string }): void;
}>();

function onTreeContext(payload: { node: DirTreeNode }) {
  emit("node-context", { path: payload.node.path });
}

function onTabChange(value: string | number) {
  emit("update:tab", value as "tree" | "quick");
}
</script>

<template>
  <div v-if="!collapsed" class="sftp-side-panel">
    <Tabs :model-value="tab" class="sftp-side-tabs" @update:model-value="onTabChange">
      <TabsList class="sftp-side-tab-list">
        <TabsTrigger value="tree" class="sftp-side-tab" :title="t('sftpSide.tree')">
          <FolderTree />
        </TabsTrigger>
        <TabsTrigger value="quick" class="sftp-side-tab" :title="t('sftpQuickPath.title')">
          <Star />
        </TabsTrigger>
      </TabsList>
      <span class="sftp-side-spacer" />
      <button v-if="tab === 'tree'" type="button" :title="t('refresh')" @click="emit('refresh-tree')">
        <RefreshCw />
      </button>
      <button type="button" :title="t('sftpSide.collapse')" @click="emit('update:collapsed', true)">
        <ChevronsLeft />
      </button>
    </Tabs>
    <div class="sftp-side-body">
      <DirTree
        v-if="tab === 'tree' && treeRoot"
        :nodes="[treeRoot]"
        :depth="0"
        :current-path="currentPath"
        :t="t"
        @toggle="emit('toggle-node', $event)"
        @open="emit('navigate', $event.path)"
        @context="onTreeContext"
      />
      <div v-else-if="tab === 'quick'" class="sftp-side-quick">
        <button
          v-for="qp in quickPaths"
          :key="qp.path"
          type="button"
          :class="{ 'is-current': qp.path === currentPath }"
          :title="qp.path"
          @click="emit('navigate', qp.path)"
          @contextmenu="emit('node-context', { path: qp.path })"
        >
          <Home v-if="qp.home" aria-hidden="true" />
          <Folder v-else aria-hidden="true" />
          <span>{{ qp.label }}</span>
        </button>
      </div>
    </div>
  </div>
  <div v-else class="sftp-side-rail">
    <button type="button" :title="t('sftpSide.expand')" @click="emit('update:collapsed', false)">
      <ChevronsRight />
    </button>
  </div>
</template>
