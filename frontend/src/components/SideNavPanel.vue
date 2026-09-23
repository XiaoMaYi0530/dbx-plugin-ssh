<script setup lang="ts">
// SFTP 侧栏导航面板（对标 files 插件 SideNavPanel.vue）：tree（目录树，默认）/
// quick（快捷路径）双 tab，可收起为窄条再展开；tab 与收缩状态由 App.vue 持久化
// 到 localStorage。行右键统一上抛 node-context（打开 / 复制路径 / 复制文件名 /
// 压缩），由 App.vue 弹菜单。
// P2-1/P2-2/P2-4 追加：otp（OTP 验证码）/ import（会话导入）/ docker（容器管理）
// 三个面板内聚 tab——它们是纯面板内状态（不进 App.vue 的持久化协议，App.vue 的
// setSftpSideTab 契约保持 "tree" | "quick" 不变），由本组件 extraTab 记忆当前
// 激活项；切回 tree/quick 时清空并恢复上抛。
import { computed, ref } from "vue";
import { ChevronsLeft, ChevronsRight, Container, FileUp, Folder, FolderTree, Home, KeyRound, RefreshCw, Star } from "@lucide/vue";
import DirTree from "./DirTree.vue";
import DockerPanel from "./DockerPanel.vue";
import ImportWizard from "./ImportWizard.vue";
import OtpPanel from "./OtpPanel.vue";
import { Tabs, TabsList, TabsTrigger } from "./ui/tabs";
import type { DirTreeNode } from "../lib/sftpDirTree";

export interface SftpSideQuickPath {
  path: string;
  /** 展示文案（home 项用本地化「主目录」，其余直接展示路径）。 */
  label: string;
  home?: boolean;
}

type ExtraSideTab = "otp" | "import" | "docker";

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

/** 面板内聚 tab（otp/import/docker）；null = 回落到 App.vue 持久化的 tree/quick。 */
const extraTab = ref<ExtraSideTab | null>(null);
const activeTab = computed(() => extraTab.value ?? props.tab);

function onTreeContext(payload: { node: DirTreeNode }) {
  emit("node-context", { path: payload.node.path });
}

function onTabChange(value: string | number) {
  const next = String(value);
  if (next === "otp" || next === "import" || next === "docker") {
    extraTab.value = next;
    return;
  }
  extraTab.value = null;
  emit("update:tab", next as "tree" | "quick");
}
</script>

<template>
  <div v-if="!collapsed" class="sftp-side-panel">
    <Tabs :model-value="activeTab" class="sftp-side-tabs" @update:model-value="onTabChange">
      <TabsList class="sftp-side-tab-list">
        <TabsTrigger value="tree" class="sftp-side-tab" :title="t('sftpSide.tree')">
          <FolderTree />
        </TabsTrigger>
        <TabsTrigger value="quick" class="sftp-side-tab" :title="t('sftpQuickPath.title')">
          <Star />
        </TabsTrigger>
        <!-- Docker / OTP / Import 面板入口（Task P2-4/P2-1/P2-2 追加块） -->
        <TabsTrigger value="docker" class="sftp-side-tab" :title="t('docker.title')">
          <Container />
        </TabsTrigger>
        <TabsTrigger value="otp" class="sftp-side-tab" :title="t('otpPanel.title')">
          <KeyRound />
        </TabsTrigger>
        <TabsTrigger value="import" class="sftp-side-tab" :title="t('importWizard.title')">
          <FileUp />
        </TabsTrigger>
      </TabsList>
      <span class="sftp-side-spacer" />
      <button v-if="activeTab === 'tree'" type="button" :title="t('refresh')" @click="emit('refresh-tree')">
        <RefreshCw />
      </button>
      <button type="button" :title="t('sftpSide.collapse')" @click="emit('update:collapsed', true)">
        <ChevronsLeft />
      </button>
    </Tabs>
    <div class="sftp-side-body">
      <DockerPanel v-if="activeTab === 'docker'" :t="t" />
      <OtpPanel v-else-if="activeTab === 'otp'" :t="t" />
      <ImportWizard v-else-if="activeTab === 'import'" :t="t" />
      <DirTree
        v-else-if="activeTab === 'tree' && treeRoot"
        :nodes="[treeRoot]"
        :depth="0"
        :current-path="currentPath"
        :t="t"
        @toggle="emit('toggle-node', $event)"
        @open="emit('navigate', $event.path)"
        @context="onTreeContext"
      />
      <div v-else-if="activeTab === 'quick'" class="sftp-side-quick">
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
