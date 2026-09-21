// 设置域共享模型（App.vue 抽出 SettingsDialog.vue 时下沉）：sidecar 返回的
// 设置/配置档/已知主机/本机密钥/MCP 限速结构，与纯格式化助手。字段与
// docs/PROTOCOL.zh-CN.md 的 ssh/settings、sudo/profiles、ssh/knownHosts、
// keys/discover、mcp/settings 各节一致。

export const MIB = 1024 * 1024;

export function mibField(bytes?: number) {
  return typeof bytes === "number" && bytes > 0 ? String(Math.round(bytes / MIB)) : "";
}

export function settingsErrorOf(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}

export interface SshSettings {
  quickSudo: boolean;
  sudoUsePty: boolean;
  sudoPasswordSet: boolean;
  totpConfigured: boolean;
  authFlowMode: string;
  passwordPromptHint: string;
  totpPromptHint: string;
  // revealSecrets: true 时回显的本连接原始凭据（设置弹窗预填用）。
  sudoPassword?: string;
  totpSecret?: string;
  // 全局 quick sudo 配置来源（空串 = 使用本连接自己的凭据）。
  quickSudoProfileId?: string;
  quickSudoProfileName?: string;
  // AI 终端同步执行模式（连接级；off 默认 / auto 分级 / strict 全审）。
  agentTerminalMode?: string;
  // 已记住的免审批命令（连接级原始行，sudoers 式 token 语义）。
  rememberedCommands?: string[];
}

// 全局 quick sudo 配置视图：密钥永不回显，只有已设置布尔位。
export interface SudoProfileView {
  id: string;
  name: string;
  sudoPasswordSet: boolean;
  totpConfigured: boolean;
  authFlowMode: string;
  passwordPromptHint: string;
  totpPromptHint: string;
  sudoUsePty: boolean;
  createdAt: number;
  updatedAt: number;
}

export interface KnownHostEntry {
  host: string;
  port: number;
  keyType: string;
  fingerprint: string;
}

export interface DiscoveredKey {
  path: string;
  algorithm: string;
  fingerprint: string;
  // The protocol doc names this field `hasPassphrase`; the current sidecar
  // serializes Rust's snake_case `has_passphrase`. Accept both spellings.
  hasPassphrase?: boolean;
  has_passphrase?: boolean;
}

export interface McpSizeSettings {
  maxReadBytes?: number;
  maxUploadBytes?: number;
  maxDownloadBytes?: number;
  // §1.3 MCP 权限档与连接作用域（新字段，旧 sidecar 不回即用默认值）。
  execPermissionMode?: string;
  connectionScope?: string[];
}
