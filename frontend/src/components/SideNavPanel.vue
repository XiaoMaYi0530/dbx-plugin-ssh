<script setup lang="ts">
// SFTP 侧栏导航面板（对标 files 插件 SideNavPanel.vue）：tree（目录树，默认）/
// quick（快捷路径）双 tab，可收起为窄条再展开；tab 与收缩状态由 App.vue 持久化
// 到 localStorage。行右键统一上抛 node-context（打开 / 复制路径 / 复制文件名 /
// 压缩），由 App.vue 弹菜单。
import { computed, ref } from "vue";
import { ChevronsLeft, ChevronsRight, Container, Folder, FolderTree, Home, RefreshCw, Star } from "@lucide/vue";
import DirTree from "./DirTree.vue";
import DockerPanel from "./DockerPanel.vue";
import { Tabs, TabsList, TabsTrigger } from "./ui/tabs";
import type { DirTreeNode } from "../lib/sftpDirTree";

export interface SftpSideQuickPath {
  path: string;
  /** 展示文案（home 项用本地化「主目录」，其余直接展示路径）。 */
  label: string;
  home?: boolean;
}

const props = defineProps<{
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

// —— Docker 面板入口（Task P2-4 追加块）—————————————————————
// docker tab 状态归本组件本地持有：App.vue 的 sftpSideTab 状态机与
// localStorage 持久化仍是 tree/quick 二值，docker 激活时不上抛 update:tab，
// 切回 tree/quick 时恢复上抛。DockerPanel 自行解析会话（ssh/sessions/list）。
const dockerActive = ref(false);
const tabValue = computed(() => (dockerActive.value ? "docker" : props.tab));

function onTreeContext(payload: { node: DirTreeNode }) {
  emit("node-context", { path: payload.node.path });
}

function onTabChange(value: string | number) {
  if (value === "docker") {
    dockerActive.value = true;
    return;
  }
  dockerActive.value = false;
  emit("update:tab", value as "tree" | "quick");
}
</script>

<template>
  <div v-if="!collapsed" class="sftp-side-panel">
    <Tabs :model-value="tabValue" class="sftp-side-tabs" @update:model-value="onTabChange">
      <TabsList class="sftp-side-tab-list">
        <TabsTrigger value="tree" class="sftp-side-tab" :title="t('sftpSide.tree')">
          <FolderTree />
        </TabsTrigger>
        <TabsTrigger value="quick" class="sftp-side-tab" :title="t('sftpQuickPath.title')">
          <Star />
        </TabsTrigger>
        <!-- Docker 面板入口（Task P2-4 追加块） -->
        <TabsTrigger value="docker" class="sftp-side-tab" :title="t('docker.title')">
          <Container />
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
      <!-- Docker 面板（Task P2-4 追加块）：激活时接管 body -->
      <DockerPanel v-if="dockerActive" :t="t" />
      <DirTree
        v-else-if="tab === 'tree' && treeRoot"
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
