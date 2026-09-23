<script setup lang="ts">
// SFTP 侧栏导航面板（对标 files 插件 SideNavPanel.vue）：tree（目录树，默认）/
// quick（快捷路径）双 tab，可收起为窄条再展开；tab 与收缩状态由 App.vue 持久化
// 到 localStorage。行右键统一上抛 node-context（打开 / 复制路径 / 复制文件名 /
// 压缩），由 App.vue 弹菜单。
// P2-1/P2-2 追加：otp（OTP 验证码）/ import（会话导入）两个面板内聚 tab。
// 它们是纯面板内状态（不进 App.vue 的持久化协议，App.vue 的 setSftpSideTab
// 契约保持 "tree" | "quick" 不变），由本组件内部 extraTab 记忆当前激活项。
import { computed, ref } from "vue";
import { ChevronsLeft, ChevronsRight, FileUp, Folder, FolderTree, Home, KeyRound, RefreshCw, Star } from "@lucide/vue";
import DirTree from "./DirTree.vue";
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

type ExtraSideTab = "otp" | "import";

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

/** 面板内聚 tab（otp/import）；null = 回落到 App.vue 持久化的 tree/quick。 */
const extraTab = ref<ExtraSideTab | null>(null);
const activeTab = computed(() => extraTab.value ?? props.tab);

function onTreeContext(payload: { node: DirTreeNode }) {
  emit("node-context", { path: payload.node.path });
}

function onTabChange(value: string | number) {
  emit("update:tab", value as "tree" | "quick");
}

/** tree/quick 走原持久化协议并清掉内聚 tab；otp/import 只记在面板内部。 */
function onTabsChange(value: string | number) {
  const next = String(value);
  if (next === "otp" || next === "import") {
    extraTab.value = next;
    return;
  }
  extraTab.value = null;
  onTabChange(value);
}
</script>

<template>
  <div v-if="!collapsed" class="sftp-side-panel">
    <Tabs :model-value="activeTab" class="sftp-side-tabs" @update:model-value="onTabsChange">
      <TabsList class="sftp-side-tab-list">
        <TabsTrigger value="tree" class="sftp-side-tab" :title="t('sftpSide.tree')">
          <FolderTree />
        </TabsTrigger>
        <TabsTrigger value="quick" class="sftp-side-tab" :title="t('sftpQuickPath.title')">
          <Star />
        </TabsTrigger>
        <TabsTrigger value="otp" class="sftp-side-tab" :title="t('otpPanel.title')">
          <KeyRound />
        </TabsTrigger>
        <TabsTrigger value="import" class="sftp-side-tab" :title="t('importWizard.title')">
          <FileUp />
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
        v-if="activeTab === 'tree' && treeRoot"
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
      <OtpPanel v-else-if="activeTab === 'otp'" :t="t" />
      <ImportWizard v-else-if="activeTab === 'import'" :t="t" />
    </div>
  </div>
  <div v-else class="sftp-side-rail">
    <button type="button" :title="t('sftpSide.expand')" @click="emit('update:collapsed', false)">
      <ChevronsRight />
    </button>
  </div>
</template>
