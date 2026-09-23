<script setup lang="ts">
// Telnet 连接弹窗（P2-3）：Host/Port/退格键/回车键 + 可选 Expect 自动应答
// 规则与两个密文槽。规则语法与 SSH 触发器同款（tssh Expect* 文本或 JSON），
// 提交时整体交给 sidecar 校验（telnet/start 返回错误即回显）。
// 纯 UI：不做连接编排，App.vue 持有会话状态。
import { reactive, ref, watch } from "vue";
import { TriangleAlert } from "@lucide/vue";
import { workbenchMessage } from "../lib/i18n";
import { Dialog, DialogContent, DialogTitle } from "./ui/dialog";

export interface TelnetConnectOptions {
  host: string;
  port: number;
  enterMode: "crlf" | "cr" | "lf";
  backspaceMode: "del" | "ctrl_h";
  rules?: string;
  secret1?: string;
  secret2?: string;
}

interface Props {
  locale: string;
  open: boolean;
}
const props = defineProps<Props>();
const emit = defineEmits<{ "update:open": [boolean]; connect: [TelnetConnectOptions] }>();

const t = (key: string, values: Record<string, string | number> = {}) =>
  workbenchMessage(props.locale, key, values);

const form = reactive({
  host: "",
  port: "23",
  enterMode: "crlf" as TelnetConnectOptions["enterMode"],
  backspaceMode: "del" as TelnetConnectOptions["backspaceMode"],
  rules: "",
  secret1: "",
  secret2: "",
});
const hostError = ref(false);

watch(
  () => props.open,
  (open) => {
    if (open) hostError.value = false;
  },
);

function submit() {
  const host = form.host.trim();
  if (!host) {
    hostError.value = true;
    return;
  }
  const port = Number.parseInt(form.port, 10);
  emit("update:open", false);
  emit("connect", {
    host,
    port: Number.isInteger(port) && port > 0 && port <= 65535 ? port : 23,
    enterMode: form.enterMode,
    backspaceMode: form.backspaceMode,
    // 空规则不随请求下发（sidecar 将空串视为功能关闭，语义等价）。
    ...(form.rules.trim() ? { rules: form.rules } : {}),
    ...(form.secret1 ? { secret1: form.secret1 } : {}),
    ...(form.secret2 ? { secret2: form.secret2 } : {}),
  });
  form.secret1 = "";
  form.secret2 = "";
}
</script>

<template>
  <Dialog :open="open" @update:open="(open) => emit('update:open', open)">
    <DialogContent class="modal small-modal" @escape-key-down.prevent>
      <header>
        <DialogTitle>{{ t("telnet.dialogTitle") }}</DialogTitle>
        <button :title="t('close')" class="icon-button" @click="emit('update:open', false)"><X /></button>
      </header>
      <p class="muted telnet-security-note"><TriangleAlert class="h-3.5 w-3.5" />{{ t("telnet.security") }}</p>
      <div class="telnet-form-grid">
        <label class="settings-field">
          <span>{{ t("telnet.host") }}</span>
          <input v-model="form.host" class="mono" :placeholder="t('telnet.hostPlaceholder')" spellcheck="false" :aria-invalid="hostError" @keydown.enter="submit" @input="hostError = false" />
        </label>
        <label class="settings-field">
          <span>{{ t("telnet.port") }}</span>
          <input v-model="form.port" class="mono" inputmode="numeric" spellcheck="false" @keydown.enter="submit" />
        </label>
        <label class="settings-field">
          <span>{{ t("telnet.backspaceMode") }}</span>
          <select v-model="form.backspaceMode">
            <option value="del">{{ t("telnet.backspaceMode.del") }}</option>
            <option value="ctrl_h">{{ t("telnet.backspaceMode.ctrlH") }}</option>
          </select>
        </label>
        <label class="settings-field">
          <span>{{ t("telnet.enterMode") }}</span>
          <select v-model="form.enterMode">
            <option value="crlf">{{ t("telnet.enterMode.crlf") }}</option>
            <option value="cr">{{ t("telnet.enterMode.cr") }}</option>
            <option value="lf">{{ t("telnet.enterMode.lf") }}</option>
          </select>
        </label>
      </div>
      <details class="telnet-auto-login">
        <summary class="muted">{{ t("telnet.autoLogin") }}</summary>
        <p class="muted settings-note">{{ t("telnet.autoLoginHint") }}</p>
        <textarea v-model="form.rules" class="mono telnet-rules-input" rows="4" :placeholder="t('telnet.rulesPlaceholder')" spellcheck="false"></textarea>
        <div class="telnet-form-grid">
          <label class="settings-field">
            <span>{{ t("telnet.secret1") }}</span>
            <input v-model="form.secret1" type="password" autocomplete="off" spellcheck="false" />
          </label>
          <label class="settings-field">
            <span>{{ t("telnet.secret2") }}</span>
            <input v-model="form.secret2" type="password" autocomplete="off" spellcheck="false" />
          </label>
        </div>
      </details>
      <footer>
        <button @click="emit('update:open', false)">{{ t("cancel") }}</button>
        <button class="primary-button" @click="submit">{{ t("telnet.connect") }}</button>
      </footer>
    </DialogContent>
  </Dialog>
</template>

<style scoped>
.telnet-security-note {
  display: flex;
  align-items: flex-start;
  gap: 6px;
  margin: 0 0 8px;
  font-size: 11px;
  line-height: 1.5;
}
.telnet-security-note svg {
  flex: 0 0 14px;
  margin-top: 1px;
  color: var(--warning, #b45309);
}
.telnet-form-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 8px 10px;
  margin-bottom: 8px;
}
.telnet-form-grid select {
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 6px 8px;
  background: var(--background);
  color: var(--foreground);
  font-size: 12px;
}
.telnet-auto-login summary {
  cursor: pointer;
  font-size: 11px;
  user-select: none;
}
.telnet-auto-login[open] summary {
  margin-bottom: 6px;
}
.telnet-rules-input {
  width: 100%;
  margin-bottom: 8px;
  resize: vertical;
  font-size: 11px;
  line-height: 1.5;
}
</style>
