<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon, type ISearchOptions } from "@xterm/addon-search";
import { WebLinksAddon } from "@xterm/addon-web-links";
import {
  Archive,
  ArrowDown,
  ArrowLeftRight,
  ArrowUp,
  ArrowUpDown,
  ClipboardPaste,
  Columns3,
  Copy,
  Download,
  Eraser,
  File as FileIcon,
  FilePlus,
  FileText,
  FileUp,
  Folder,
  FolderPlus,
  Gauge,
  History,
  Home,
  Info,
  KeyRound,
  ListChecks,
  Loader2,
  Lock,
  PackageOpen,
  Pencil,
  PlugZap,
  RefreshCw,
  Save,
  Scissors,
  Search,
  Settings,
  ShieldCheck,
  SquareTerminal,
  TextSelect,
  Trash2,
  TriangleAlert,
  X,
  Zap,
} from "@lucide/vue";
import type { Detection as ZmodemDetection, Session as ZmodemSession, Sentry as ZmodemSentry } from "zmodem.js";
import { Osc7DirectoryParser } from "./lib/terminalDirectoryTracking";
import { describeReconnectCountdown, shouldReattachTerminal, terminalReconnectDelay, type ReconnectCountdown } from "./lib/terminalReconnect";
import { createZmodemSentry, sendZmodemFiles, type ZmodemUploadProgress } from "./lib/terminalZmodem";
import { sampleTransferSpeed, type TransferSpeedSample } from "./lib/transferSpeed";
import { buildPasteConfirmation, type PasteConfirmation } from "./lib/dangerousCommands";
import { expandSelection, filterSftpEntries, type SftpTypeFilter } from "./lib/sftpFileFilters";
import { pushPathHistory, sanitizePathHistories } from "./lib/sftpPathHistory";
import { browseCommandHistory, isPersistableCommand, pushCommandHistory, sanitizeCommandHistory } from "./lib/commandHistory";
import { normalizeQuickCommands, removeQuickCommand, upsertQuickCommand, type QuickCommand } from "./lib/quickCommands";
import { formatLatency, formatAuthMethodLabel, type KnownAuthMethod } from "./lib/connectionInfo";
import { clampFontSize } from "./lib/terminalZoom";
import { commandMarkerTooltip, formatCommandDuration, Osc633CommandParser, runningCommandElapsedMs, type Osc633StreamUpdates } from "./lib/terminalCommandMarkers";
import { advanceBatchProgress, batchProgressPercent, createBatchProgress, type BatchProgressState } from "./lib/sftpBatchProgress";
import { describeWorkbenchSessionStatus, type WorkbenchSessionStatus } from "./lib/sessionStatus";
import { sanitizeCommandOutput } from "./lib/terminalOutputText";
import { formatBytes, formatRate } from "./lib/format";
import { DBX_POPOVER, resolveAppearance, TERMINAL_ANSI } from "./lib/appearance";
import type { SshWorkbenchPaneOrder } from "./lib/workbenchLayout";
import { workbenchMessage } from "./lib/i18n";
import TextPreview from "./components/TextPreview.vue";
import TerminalSearchPanel from "./components/TerminalSearchPanel.vue";

interface SessionInfo {
  sessionId: string;
  connectionId: string;
  workbenchId?: string;
  connected: boolean;
  sequence: number;
  chunkSize: number;
  directoryTrackingSupported?: boolean;
  replay?: ReplayResult;
}

interface ReplayResult {
  frameCount: number;
  firstAvailableSequence: number;
  tailSequence: number;
  complete: boolean;
}

interface SftpEntry {
  name: string;
  uri: string;
  kind: "file" | "directory" | "symlink" | "other";
  size?: number;
  modifiedAt?: number;
  permissions?: string;
  contentType?: string;
}

interface SftpStatInfo {
  path: string;
  kind: SftpEntry["kind"];
  size?: number;
  modifiedAt?: number;
  mode?: string;
  owner?: string;
  group?: string;
}

// 服务器内复制/剪切/粘贴的剪贴板：仅当前连接内有效。
interface SftpClipboard {
  mode: "copy" | "cut";
  paths: string[];
  connectionId: string;
}

interface HostKeyPrompt {
  challengeId: string;
  operationId: string;
  host: string;
  port: number;
  keyType: string;
  fingerprint: string;
}

interface TransferTask {
  taskId: string;
  sessionId?: string;
  direction: "upload" | "download";
  fileName: string;
  size: number;
  transferred: number;
  status: "queued" | "running" | "completed" | "cancelled" | "failed";
  error?: string;
}

interface WorkbenchState {
  sessionId?: string;
  terminalSequence?: number;
  sftpPath?: string;
  followDirectory?: boolean;
  sudoMode?: boolean;
  splitRatio?: number;
  paneOrder?: SshWorkbenchPaneOrder;
  visibleColumns?: SftpColumn[];
}

interface ConnectionSummary {
  name?: string;
  host?: string;
  port?: number;
  username?: string;
  color?: string;
  readOnly?: boolean;
}

interface DownloadInfo {
  taskId: string;
  fileName: string;
  size: number;
  chunkSize: number;
}

interface ExecResult {
  output: string;
  exitCode: number;
}

interface ServerMetrics {
  hostname?: string | null;
  kernel?: string | null;
  uptimeSeconds?: number | null;
  cpu?: { cores?: number | null; percent?: number | null; load1?: number | null; load5?: number | null; load15?: number | null };
  memory?: { totalBytes?: number; availableBytes?: number; usedBytes?: number; swapTotalBytes?: number; swapUsedBytes?: number };
  disks?: Array<{ filesystem: string; mount: string; totalBytes: number; usedBytes: number; availableBytes: number; percentUsed: number }>;
  // Extensions reported by newer sidecars; when absent the network and
  // process sections simply stay hidden instead of erroring.
  network?: Array<{ name: string; rxRate: number; txRate: number; rxTotal: number; txTotal: number }>;
  processes?: Array<{ pid: number; user: string; cpuPercent: number; memPercent: number; command: string }>;
}

interface SshSettings {
  quickSudo: boolean;
  sudoUsePty: boolean;
  sudoPasswordSet: boolean;
  totpConfigured: boolean;
  authFlowMode: string;
  passwordPromptHint: string;
  totpPromptHint: string;
}

interface DiskUsage {
  filesystem: string;
  mount: string;
  totalBytes: number;
  usedBytes: number;
  availableBytes: number;
  percentUsed: number;
}

interface KnownHostEntry {
  host: string;
  port: number;
  keyType: string;
  fingerprint: string;
}

interface DiscoveredKey {
  path: string;
  algorithm: string;
  fingerprint: string;
  // The protocol doc names this field `hasPassphrase`; the current sidecar
  // serializes Rust's snake_case `has_passphrase`. Accept both spellings.
  hasPassphrase?: boolean;
  has_passphrase?: boolean;
}

interface McpSizeSettings {
  maxReadBytes?: number;
  maxUploadBytes?: number;
  maxDownloadBytes?: number;
}

type SftpColumn = "size" | "modified" | "permissions";
type SftpSortColumn = "name" | "size" | "modified";

const PREVIEWABLE_EXTENSIONS = new Set(["bash", "bat", "c", "cfg", "cmd", "conf", "cpp", "css", "csv", "go", "h", "hpp", "htm", "html", "ini", "java", "js", "json", "log", "md", "properties", "ps1", "py", "rs", "scss", "sh", "sql", "toml", "ts", "tsx", "txt", "vue", "xml", "yaml", "yml", "zsh"]);
const IMAGE_MIME_BY_EXTENSION: Record<string, string> = { png: "png", jpg: "jpeg", jpeg: "jpeg", gif: "gif", webp: "webp", svg: "svg+xml", bmp: "bmp", ico: "x-icon" };
// Only consulted before the NUL-byte scan; extension-less or mislabeled files are
// still caught by the content check after sftp/read.
const BINARY_PREVIEW_EXTENSIONS = new Set(["7z", "bin", "bz2", "class", "dll", "dmg", "dylib", "exe", "gz", "iso", "jar", "lz4", "o", "obj", "otf", "pdf", "pyc", "rar", "so", "tar", "tif", "tiff", "ttf", "war", "woff", "woff2", "xz", "zip", "zst"]);
const MAX_INLINE_PREVIEW_BYTES = 1024 * 1024;
const MAX_DIRECT_WRITE_BYTES = 4 * 1024 * 1024;
const MIB = 1024 * 1024;
const MAX_IMAGE_PREVIEW_BYTES = 20 * MIB;
// Above this size the browser download path buffers the whole file in memory, so ask first.
const WEB_DOWNLOAD_WARNING_BYTES = 512 * MIB;
const ZMODEM_DETECTION_TIMEOUT_MS = 5000;
// 粘贴防护：内容含换行或达到该字符数时先确认（对齐 tiny-rdm TerminalPane 阈值）。
const PASTE_CONFIRM_CHAR_THRESHOLD = 200;
// SFTP 路径历史：每连接最多保留 10 条，存 localStorage（对齐 tiny-rdm pathHistory）。
const SFTP_PATH_HISTORY_KEY = "sftp-path-history";
const SFTP_PATH_HISTORY_LIMIT = 10;
// Upper bound for out-of-order terminal frames held while waiting for the
// missing sequence; the replay path re-delivers anything dropped beyond it.
const TERMINAL_PENDING_FRAME_LIMIT = 1024;
const SFTP_QUICK_PATHS = ["/", "/home", "/tmp", "/etc", "/var", "/root"];
// 命令历史 / 快速命令 / 终端字号：localStorage 持久化（敏感命令不入持久层）。
const COMMAND_HISTORY_KEY = "ssh-command-history";
const QUICK_COMMANDS_KEY = "ssh-quick-commands";
const TERMINAL_FONT_SIZE_KEY = "ssh-terminal-font-size";

type TerminalSearchMatchState = "idle" | "match" | "no-match";

const terminalHost = ref<HTMLElement>();
const paneContainer = ref<HTMLElement>();
const uploadInput = ref<HTMLInputElement>();
const zmodemInput = ref<HTMLInputElement>();
const hostContext = ref<Record<string, unknown>>({});
// 宿主未下发 appearance 前的兜底：DBX `.dark` 规范令牌。
const appearance = ref(resolveAppearance());
const terminalState = ref<"connecting" | "connected" | "disconnected" | "error">("connecting");
const terminalError = ref("");
const sftpError = ref("");
const notice = ref("");
const session = ref<SessionInfo>();
const currentPath = ref("/");
const entries = ref<SftpEntry[]>([]);
const selectedPath = ref("");
const loadingFiles = ref(false);
const hostKeyPrompt = ref<HostKeyPrompt>();
const rememberHostKey = ref(true);
const splitRatio = ref(58);
const paneOrder = ref<SshWorkbenchPaneOrder>("terminal-left");
const followDirectory = ref(false);
const directoryTrackingSupported = ref<boolean | undefined>();
const visibleColumns = ref<SftpColumn[]>(["size", "modified"]);
const sort = ref<{ column: SftpSortColumn; direction: "asc" | "desc" }>({ column: "name", direction: "asc" });
const transferTasks = reactive<Record<string, TransferTask>>({});
const transferPanelOpen = ref(false);
const columnsOpen = ref(false);
const transferSpeeds = reactive<Record<string, number>>({});
const previewOpen = ref(false);
const previewTitle = ref("");
const previewText = ref("");
const previewLoading = ref(false);
const previewPath = ref("");
const previewSize = ref(0);
const previewEditable = ref(false);
const previewDraft = ref("");
const previewSaving = ref(false);
const previewMode = ref<"text" | "image">("text");
const previewImageUrl = ref("");
const previewImageZoomed = ref(false);
// Baseline snapshot of the content when it was opened (or last saved); the
// dirty marker compares the live draft against it.
const previewBaseline = ref("");
const previewBinary = ref(false);
const sudoMode = ref(false);
const quickSudo = ref(false);
const quickSudoSubmitting = ref(false);
const archiveBusy = ref(false);
const operationDialog = ref<"mkdir" | null>(null);
const operationDraft = ref("");
const deleteTarget = ref<SftpEntry>();
const deleteSubmitting = ref(false);
const renamingPath = ref("");
const renameDraft = ref("");
const renameSubmitting = ref(false);
const dragActive = ref(false);
const terminalMenu = ref<{ x: number; y: number }>();
const fileMenu = ref<{ x: number; y: number; entry: SftpEntry }>();
const zmodemState = ref<"idle" | "waiting" | "uploading">("idle");
const zmodemFileName = ref("");
const zmodemTransferred = ref(0);
const zmodemTotalSize = ref(0);
const zmodemSpeed = ref(0);
const commandOpen = ref(false);
const commandDraft = ref("");
const commandUseSudo = ref(true);
const commandRunning = ref(false);
const commandExecId = ref("");
const commandResult = ref<ExecResult>();
const commandError = ref("");
// 命令历史：内存环形 + localStorage 非敏感持久化；index 为 -1 表示未在浏览历史。
const commandHistory = ref<string[]>(loadCommandHistory());
const commandHistoryIndex = ref(-1);
const commandHistoryBackup = ref("");
// 快速命令：用户自定义片段（≤20 条），工具栏下拉一键发送到 PTY。
const quickCommands = ref<QuickCommand[]>(loadQuickCommands());
const quickMenuOpen = ref(false);
const quickDraft = reactive<{ id?: string; name: string; command: string }>({ name: "", command: "" });
// 连接信息面板（只读摘要 + echo 往返延迟）。
const connectionInfoOpen = ref(false);
const connectionLatency = ref<number | null>(null);
const connectionLatencyBusy = ref(false);
const connectionLatencyFailed = ref(false);
// 认证方式只读名称（来自 ssh/sessions/list 的 authMethod；仅方法名，无凭据）。
const connectionAuthMethod = ref("");
// 生效只读门禁（来自 ssh/sessions/list 行的 readOnly：表单 read_only ∥
// 宿主标准 read_only）。后端门禁为权威来源，前端据此禁用写操作。
const connectionReadOnly = ref(false);
const metricsOpen = ref(false);
const metrics = ref<ServerMetrics>();
const metricsLoading = ref(false);
const metricsError = ref("");
const settingsOpen = ref(false);
const settingsLoading = ref(false);
const settingsSaving = ref(false);
const settingsMeta = ref<SshSettings>();
const settingsDraft = reactive({
  quickSudo: true,
  sudoUsePty: false,
  sudoPassword: "",
  totpSecret: "",
  authFlowMode: "password_then_otp",
  passwordPromptHint: "",
  totpPromptHint: "",
});
const chmodTarget = ref<SftpEntry>();
const chmodDraft = ref("");
const chmodSubmitting = ref(false);
const diskUsage = ref<DiskUsage>();
const knownHosts = ref<KnownHostEntry[]>([]);
const knownHostsLoading = ref(false);
const knownHostsError = ref("");
const localKeys = ref<DiscoveredKey[]>([]);
const localKeysLoading = ref(false);
const localKeysError = ref("");
const mcpDraft = reactive({ readMiB: "", uploadMiB: "", downloadMiB: "" });
const mcpLoading = ref(false);
const mcpError = ref("");
const mcpSaving = ref(false);
const searchOpen = ref(false);
const searchMatchState = ref<TerminalSearchMatchState>("idle");
const searchResultIndex = ref(0);
const searchResultCount = ref(0);
const pasteConfirm = ref<PasteConfirmation>();
const terminalFontSize = ref(appearance.value.terminal.fontSize);
// True while attachSession sits inside its bounded backoff loop; turns the
// status pill and overlay into the dedicated "reconnecting" phase.
const reconnectPending = ref(false);
// Pure-display reconnect countdown for the status pill: seconds until the
// next retry plus the progress through the current backoff delay.
const reconnectCountdown = ref<ReconnectCountdown | null>(null);
let reconnectNextAt = 0;
let reconnectDelayMs = 0;
let reconnectCountdownTimer = 0;
const commandMarker = reactive({
  installed: false,
  active: false,
  command: "",
  exitCode: null as number | null,
  durationMs: null as number | null,
  cwd: "",
  // Start timestamp of the currently running command; drives the 1s tick that
  // keeps the marker strip duration live while a command is in flight.
  startedAt: null as number | null,
});
// Live elapsed milliseconds for the running marker (null when idle or finished).
const commandMarkerElapsed = ref<number | null>(null);
const sftpSearch = ref("");
const sftpTypeFilter = ref<SftpTypeFilter>("all");
const selectedUris = ref<string[]>([]);
const lastClickedUri = ref("");
const sftpClipboard = ref<SftpClipboard>();
const pasteBusy = ref(false);
const pathHistoryOpen = ref(false);
const pathHistories = reactive<Record<string, string[]>>(loadPathHistories());
const newFileDialog = ref(false);
const newFileDraft = ref("");
const newFileSubmitting = ref(false);
const attrsTarget = ref<SftpEntry>();
const attrsInfo = ref<SftpStatInfo>();
const attrsLoading = ref(false);
const attrsMode = ref("");
const attrsSubmitting = ref(false);
const batchDeleteOpen = ref(false);
const batchDeleteSubmitting = ref(false);
// Aggregated progress for multi-item batch operations (delete / archive);
// null while no batch is in flight.
const batchProgress = ref<BatchProgressState | null>(null);

let terminal: Terminal | undefined;
let fitAddon: FitAddon | undefined;
let searchAddon: SearchAddon | undefined;
let terminalPasteHandler: ((event: ClipboardEvent) => void) | undefined;
let terminalWheelHandler: ((event: WheelEvent) => void) | undefined;
let pasteConfirmResolver: ((accepted: boolean) => void) | undefined;
let zoomNoticeTimer = 0;
let resizeObserver: ResizeObserver | undefined;
let disposeInput: { dispose(): void } | undefined;
let unsubscribeEvent: (() => void) | undefined;
let unsubscribeBinary: (() => void) | undefined;
let unsubscribeAppearance: (() => void) | undefined;
let unsubscribeLocale: (() => void) | undefined;
let unsubscribeContext: (() => void) | undefined;
let unsubscribeFileDrag: (() => void) | undefined;
let unsubscribeFileDrop: (() => void) | undefined;
let persistTimer = 0;
let resizeTimer = 0;
let reconnectTimer = 0;
let reconnectAttempt = 0;
let disposed = false;
let lastSequence = 0;
let replayInFlight = false;
let binaryInputChain = Promise.resolve();
let terminalInputSequence = 0;
let noticeTimer = 0;
let commandMarkerTimer = 0;
let zmodemSentry: ZmodemSentry | null = null;
let zmodemSession: ZmodemSession | null = null;
let pendingZmodemFiles: File[] = [];
let zmodemDetectionTimer = 0;
let zmodemSampledAt = 0;
let zmodemSampledBytes = 0;
let pendingTerminalInput = "";
let activeTerminalSessionId = "";
const pendingTerminalFrames = new Map<number, { stream: number; data: Uint8Array }>();
const terminalInputAckWaiters = new Map<number, { resolve: () => void; reject: (error: Error) => void; timer: number }>();
const uploadAckWaiters = new Map<string, { nextOffset: number; resolve: () => void; reject: (error: Error) => void; timer: number }>();
const downloadChunkWaiters = new Map<string, { offset: number; resolve: (bytes: Uint8Array) => void; reject: (error: Error) => void; timer: number }>();
const transferSamples = new Map<string, TransferSpeedSample>();
// Download task ids the user cancelled from the transfer panel; lets the download
// loop distinguish a user cancel (notice) from a real failure (error banner).
const cancelledTransferTasks = new Set<string>();
const directoryParser = new Osc7DirectoryParser();
// OSC 633 shell-integration markers (pure frontend parse; no-op streams pass through).
const commandMarkerParser = new Osc633CommandParser();

const locale = ref("zh-CN");
const t = (key: string, values: Record<string, string | number> = {}) => workbenchMessage(locale.value, key, values);
const connectionId = computed(() => String(hostContext.value.connectionId || ""));
// Host API 1.1 provides a stable workbenchId in the host context; on 1.0 a
// locally generated id keeps session scoping per workbench instance.
const fallbackWorkbenchId = crypto.randomUUID();
const workbenchId = computed(() => String(hostContext.value.workbenchId || fallbackWorkbenchId));
const restored = computed(() => hostContext.value.restored === true);
const connection = computed<ConnectionSummary>(() => {
  const value = hostContext.value.connection;
  return value && typeof value === "object" ? (value as ConnectionSummary) : {};
});
const canWrite = computed(() => !connection.value.readOnly && !connectionReadOnly.value);
const selectedEntry = computed(() => entries.value.find((entry) => entry.uri === selectedPath.value));
const connected = computed(() => terminalState.value === "connected" && !!session.value);
const sessionStatus = computed<WorkbenchSessionStatus>(() => describeWorkbenchSessionStatus(terminalState.value, { reattaching: reconnectPending.value }));
// Reconnect countdown lifecycle: while the backoff loop is pending a 250ms
// tick recomputes the pure countdown; any exit from "reconnecting" stops it.
watch(reconnectPending, (pending) => {
  if (reconnectCountdownTimer) {
    window.clearInterval(reconnectCountdownTimer);
    reconnectCountdownTimer = 0;
  }
  if (!pending) {
    reconnectCountdown.value = null;
    return;
  }
  const update = () => {
    reconnectCountdown.value = describeReconnectCountdown({
      pending: true,
      attempt: reconnectAttempt,
      nextAt: reconnectNextAt,
      now: Date.now(),
      delayMs: reconnectDelayMs,
    });
  };
  update();
  reconnectCountdownTimer = window.setInterval(update, 250);
});
const commandOutputText = computed(() => (commandResult.value ? sanitizeCommandOutput(commandResult.value.output) : ""));
const quickSudoTitle = computed(() => `${t("quickSudo.label")}: ${quickSudo.value ? t("quickSudo.on") : t("quickSudo.off")}\n${t("quickSudo.hint")}`);
// Hover tooltip for the terminal command marker strip: full command, exit
// code, duration and working directory (localized, multi-line).
const commandMarkerDetails = computed(() => commandMarkerTooltip(
  {
    command: commandMarker.command,
    exitCode: commandMarker.exitCode,
    durationMs: commandMarker.durationMs,
    elapsedMs: commandMarkerElapsed.value,
    cwd: commandMarker.cwd,
  },
  {
    command: t("terminalCommand.tooltipCommand"),
    exitCode: t("terminalCommand.tooltipExitCode"),
    duration: t("terminalCommand.tooltipDuration"),
    directory: t("terminalCommand.tooltipDirectory"),
  },
));
const connectionIdentity = computed(() => {
  const host = connection.value.host || connection.value.name || connectionId.value;
  const identity = connection.value.username ? `${connection.value.username}@${host}` : host;
  const port = connection.value.port && connection.value.port !== 22 ? `:${connection.value.port}` : "";
  return `${identity}${port}`;
});
// 认证方式的本地化标签：已知方法名走 i18n，未知值原样展示（只读信息）。
const connectionAuthMethodLabel = computed(() => formatAuthMethodLabel(connectionAuthMethod.value, (method) => {
  const labels: Record<KnownAuthMethod, string> = {
    password: t("authMethodPassword"),
    "private-key": t("authMethodPrivateKey"),
    "private-key-password": t("authMethodPrivateKeyPassword"),
    agent: t("authMethodAgent"),
    none: t("authMethodNone"),
  };
  return labels[method];
}));
const toolbarStyle = computed(() => {
  const color = connection.value.color;
  if (!color) return undefined;
  return {
    backgroundColor: colorWithAlpha(color, 0.1),
    boxShadow: `inset 0 1px 0 ${colorWithAlpha(color, 0.18)}`,
  };
});
const terminalBasis = computed(() => ({ flexBasis: `${splitRatio.value}%` }));
const orderedPaneClass = computed(() => paneOrder.value === "sftp-left" ? "panes panes--reversed" : "panes");
const sortedEntries = computed(() => {
  const direction = sort.value.direction === "asc" ? 1 : -1;
  return [...entries.value].sort((left, right) => {
    if (left.kind === "directory" && right.kind !== "directory") return -1;
    if (left.kind !== "directory" && right.kind === "directory") return 1;
    let result = 0;
    if (sort.value.column === "size") result = (left.size ?? -1) - (right.size ?? -1);
    else if (sort.value.column === "modified") result = (left.modifiedAt ?? 0) - (right.modifiedAt ?? 0);
    else result = left.name.localeCompare(right.name, undefined, { numeric: true, sensitivity: "base" });
    return result * direction;
  });
});
const transferList = computed(() => Object.values(transferTasks).sort((left, right) => right.taskId.localeCompare(left.taskId)));
const activeTransfers = computed(() => transferList.value.filter((task) => task.status === "queued" || task.status === "running").length);
const zmodemBusy = computed(() => zmodemState.value !== "idle");
const zmodemPercent = computed(() => zmodemTotalSize.value > 0 ? Math.min(100, Math.round((zmodemTransferred.value / zmodemTotalSize.value) * 100)) : 0);
const sftpGridStyle = computed(() => ({
  gridTemplateColumns: ["minmax(120px, 1fr)", visibleColumns.value.includes("size") ? "72px" : "", visibleColumns.value.includes("modified") ? "128px" : "", visibleColumns.value.includes("permissions") ? "84px" : ""].filter(Boolean).join(" "),
  minWidth: `${180 + (visibleColumns.value.includes("size") ? 78 : 0) + (visibleColumns.value.includes("modified") ? 134 : 0) + (visibleColumns.value.includes("permissions") ? 90 : 0)}px`,
}));
const sftpFiltersActive = computed(() => sftpSearch.value.trim() !== "" || sftpTypeFilter.value !== "all");
const visibleEntries = computed(() => filterSftpEntries(sortedEntries.value, sftpSearch.value, sftpTypeFilter.value));
const selectedEntries = computed(() => entries.value.filter((entry) => selectedUris.value.includes(entry.uri)));
const currentPathHistory = computed(() => pathHistories[connectionId.value] || []);
const previewDirty = computed(() => previewEditable.value && previewDraft.value !== previewBaseline.value);
const previewEditableAllowed = computed(() => canWrite.value && previewMode.value === "text" && !previewBinary.value && previewSize.value <= MAX_DIRECT_WRITE_BYTES);
const mcpInputsValid = computed(() => [mcpDraft.readMiB, mcpDraft.uploadMiB, mcpDraft.downloadMiB]
  .every((value) => /^\d+$/.test(value.trim()) && Number.parseInt(value.trim(), 10) > 0));

function initialState(): WorkbenchState {
  const value = hostContext.value.workbenchState;
  return value && typeof value === "object" ? (value as WorkbenchState) : {};
}

function restoreUiState() {
  const state = initialState();
  currentPath.value = typeof state.sftpPath === "string" ? normalizeRemotePath(state.sftpPath) : "/";
  splitRatio.value = typeof state.splitRatio === "number" && state.splitRatio >= 35 && state.splitRatio <= 80 ? state.splitRatio : 58;
  paneOrder.value = state.paneOrder === "sftp-left" ? "sftp-left" : "terminal-left";
  followDirectory.value = state.followDirectory === true;
  sudoMode.value = state.sudoMode === true && canWrite.value;
  visibleColumns.value = Array.isArray(state.visibleColumns) ? state.visibleColumns.filter((column): column is SftpColumn => ["size", "modified", "permissions"].includes(column)) : ["size", "modified"];
  lastSequence = typeof state.terminalSequence === "number" ? state.terminalSequence : 0;
}

function writeWorkbenchState() {
  return window.dbxPlugin.workbenchState?.set({
    sessionId: session.value?.sessionId,
    terminalSequence: lastSequence,
    sftpPath: currentPath.value,
    followDirectory: followDirectory.value,
    sudoMode: sudoMode.value,
    splitRatio: splitRatio.value,
    paneOrder: paneOrder.value,
    visibleColumns: visibleColumns.value,
  }).catch(() => undefined);
}

function persistState() {
  window.clearTimeout(persistTimer);
  persistTimer = window.setTimeout(() => {
    void writeWorkbenchState();
  }, 150);
}

function showNotice(message: string) {
  notice.value = message;
  window.clearTimeout(noticeTimer);
  noticeTimer = window.setTimeout(() => (notice.value = ""), 3500);
}

function showError(cause: unknown, target: "terminal" | "sftp" = "sftp") {
  const message = cause instanceof Error ? cause.message : String(cause);
  if (target === "terminal") terminalError.value = message;
  else sftpError.value = message;
}

function terminalTheme() {
  const colors = appearance.value.colors;
  return {
    background: colors.background,
    foreground: colors.foreground,
    cursor: colors.foreground,
    cursorAccent: colors.background,
    selectionBackground: appearance.value.colorScheme === "dark" ? "#5f6f8a88" : "#93b4e088",
    ...TERMINAL_ANSI[appearance.value.colorScheme],
  };
}

function applyAppearance(next: DbxPluginAppearance) {
  // 宿主可能缺字段（1.0 或部分下发），按 DBX 规范色板补齐。
  const resolved = resolveAppearance(next);
  appearance.value = resolved;
  const root = document.documentElement;
  root.dataset.theme = resolved.colorScheme;
  root.style.colorScheme = resolved.colorScheme;
  root.style.setProperty("--background", resolved.colors.background);
  root.style.setProperty("--foreground", resolved.colors.foreground);
  root.style.setProperty("--muted", resolved.colors.muted);
  root.style.setProperty("--muted-foreground", resolved.colors.mutedForeground);
  root.style.setProperty("--accent", resolved.colors.accent);
  root.style.setProperty("--accent-foreground", resolved.colors.accentForeground);
  root.style.setProperty("--border", resolved.colors.border);
  root.style.setProperty("--destructive", resolved.colors.destructive);
  root.style.setProperty("--popover", DBX_POPOVER[resolved.colorScheme]);
  root.style.setProperty("--ssh-terminal-background", resolved.colors.background);
  root.style.setProperty("--ui-font-family", resolved.ui.fontFamily);
  root.style.setProperty("--terminal-font-family", resolved.terminal.fontFamily);
  if (terminal) {
    terminal.options.theme = terminalTheme();
    terminal.options.fontFamily = resolved.terminal.fontFamily;
    // 宿主下发的字体大小即缩放基准；外观切换后回到基准值，
    // 但用户 A+/A- 调过的字号（localStorage）优先于宿主基准。
    const persistedFontSize = loadPersistedTerminalFontSize();
    terminalFontSize.value = persistedFontSize ?? resolved.terminal.fontSize;
    terminal.options.fontSize = terminalFontSize.value;
    scheduleFit();
  }
}

function createTerminal() {
  if (!terminalHost.value || terminal) return;
  terminalFontSize.value = loadPersistedTerminalFontSize() ?? appearance.value.terminal.fontSize;
  terminal = new Terminal({
    convertEol: false,
    cursorBlink: true,
    cursorStyle: "block",
    fontFamily: appearance.value.terminal.fontFamily,
    fontSize: terminalFontSize.value,
    lineHeight: 1.15,
    scrollback: 25_000,
    theme: terminalTheme(),
  });
  fitAddon = new FitAddon();
  searchAddon = new SearchAddon();
  terminal.loadAddon(fitAddon);
  terminal.loadAddon(searchAddon);
  terminal.loadAddon(new WebLinksAddon());
  terminal.open(terminalHost.value);
  terminal.attachCustomKeyEventHandler(handleTerminalKey);
  searchAddon.onDidChangeResults(({ resultCount, resultIndex }) => {
    if (!searchOpen.value) return;
    searchResultCount.value = resultCount;
    searchResultIndex.value = resultCount > 0 && resultIndex >= 0 ? resultIndex + 1 : 0;
    searchMatchState.value = resultCount > 0 ? "match" : "no-match";
  });
  disposeInput = terminal.onData((data) => {
    if (!session.value || zmodemBusy.value) return;
    trackPendingInput(data);
    sendTerminalBytes(new TextEncoder().encode(data));
  });
  // 捕获阶段的 paste 监听：拦截 Ctrl+V 之外的所有粘贴路径（浏览器右键菜单等），
  // 统一走风险确认后再写入终端。
  terminalPasteHandler = (event) => interceptTerminalPaste(event);
  terminalHost.value.addEventListener("paste", terminalPasteHandler, true);
  terminalWheelHandler = (event) => handleTerminalWheel(event);
  terminalHost.value.addEventListener("wheel", terminalWheelHandler, { passive: false, capture: true });
  resizeObserver = new ResizeObserver(scheduleFit);
  resizeObserver.observe(terminalHost.value);
  scheduleFit();
}

function handleTerminalKey(event: KeyboardEvent) {
  const mod = event.ctrlKey || event.metaKey;
  if (event.type !== "keydown") return true;
  if (mod && (event.key === "f" || event.key === "F")) {
    openTerminalSearch();
    return false;
  }
  if (mod && event.key === "0") {
    resetTerminalZoom();
    return false;
  }
  if (event.key === "Escape" && searchOpen.value) {
    closeTerminalSearch();
    return false;
  }
  if (mod && (event.key === "v" || event.key === "V")) {
    // 返回 false 会阻止默认行为与原生 paste 事件，避免与确认流程重复写入。
    void pasteFromClipboardToTerminal();
    return false;
  }
  return true;
}

function handleTerminalWheel(event: WheelEvent) {
  if (!(event.ctrlKey || event.metaKey)) return;
  event.preventDefault();
  adjustTerminalZoom(event.deltaY < 0 ? 1 : -1);
}

function adjustTerminalZoom(delta: number) {
  const current = terminalFontSize.value;
  const next = clampFontSize(current, delta);
  if (next === current) return;
  applyTerminalFontSize(next);
}

function resetTerminalZoom() {
  const base = appearance.value.terminal.fontSize;
  if (terminalFontSize.value === base) return;
  applyTerminalFontSize(base);
}

function loadPersistedTerminalFontSize(): number | null {
  try {
    const raw = window.localStorage.getItem(TERMINAL_FONT_SIZE_KEY);
    const parsed = raw == null ? Number.NaN : Number(raw);
    return Number.isFinite(parsed) ? clampFontSize(parsed, 0) : null;
  } catch {
    return null;
  }
}

function applyTerminalFontSize(size: number) {
  terminalFontSize.value = size;
  if (terminal) {
    terminal.options.fontSize = size;
    scheduleFit();
  }
  try {
    window.localStorage.setItem(TERMINAL_FONT_SIZE_KEY, String(size));
  } catch {
    // localStorage 不可用时字号仅对当前会话生效。
  }
  window.clearTimeout(zoomNoticeTimer);
  zoomNoticeTimer = window.setTimeout(() => showNotice(t("terminalZoom.fontSize", { size })), 500);
}

function openTerminalSearch() {
  if (!terminal) return;
  terminalMenu.value = undefined;
  searchOpen.value = true;
}

function closeTerminalSearch() {
  searchOpen.value = false;
  resetSearchResults();
  searchAddon?.clearDecorations();
  terminal?.focus();
}

function clearTerminalSearch() {
  resetSearchResults();
  searchAddon?.clearDecorations();
}

function resetSearchResults() {
  searchMatchState.value = "idle";
  searchResultCount.value = 0;
  searchResultIndex.value = 0;
}

function runTerminalSearch(query: string, options: { caseSensitive: boolean; regex: boolean; wholeWord: boolean }, direction: "next" | "prev") {
  if (!searchAddon || !query) return;
  const searchOptions: ISearchOptions = {
    caseSensitive: options.caseSensitive,
    regex: options.regex,
    wholeWord: options.wholeWord,
    decorations: {
      matchBackground: "#64748b55",
      matchOverviewRuler: "#64748b",
      activeMatchBackground: "#3b82f655",
      activeMatchColorOverviewRuler: "#3b82f6",
    },
  };
  if (direction === "prev") searchAddon.findPrevious(query, searchOptions);
  else searchAddon.findNext(query, searchOptions);
}

function trackPendingInput(data: string) {
  if (data.includes("\u001b")) return;
  for (const character of data) {
    if (character === "\r" || character === "\n" || character === "\u0003") pendingTerminalInput = "";
    else if (character === "\u007f") pendingTerminalInput = pendingTerminalInput.slice(0, -1);
    else if (character >= " ") pendingTerminalInput += character;
  }
}

function sendTerminalBytes(data: Uint8Array) {
  const sessionId = session.value?.sessionId;
  if (!sessionId) return;
  const sequence = ++terminalInputSequence;
  const payload = new Uint8Array(8 + data.byteLength);
  writeU64(payload, 0, sequence);
  payload.set(data, 8);
  binaryInputChain = binaryInputChain
    .then(async () => {
      const acknowledged = waitForTerminalInputAck(sequence);
      await window.dbxPlugin.sendBinary(`ssh/terminal/in/${sessionId}`, payload);
      await acknowledged;
    })
    .catch((cause) => showError(cause, "terminal"));
}

function waitForTerminalInputAck(sequence: number) {
  return new Promise<void>((resolve, reject) => {
    const timer = window.setTimeout(() => {
      terminalInputAckWaiters.delete(sequence);
      reject(new Error("SSH terminal input acknowledgement timed out"));
    }, 15_000);
    terminalInputAckWaiters.set(sequence, { resolve, reject, timer });
  });
}

function scheduleFit() {
  window.clearTimeout(resizeTimer);
  resizeTimer = window.setTimeout(() => {
    if (!terminal || !fitAddon || !terminalHost.value?.clientWidth || !terminalHost.value.clientHeight) return;
    try {
      fitAddon.fit();
      if (session.value) {
        void window.dbxPlugin.notify("ssh/terminal/resize", { sessionId: session.value.sessionId, cols: terminal.cols, rows: terminal.rows }).catch(() => undefined);
      }
    } catch {
      // The iframe can briefly be detached while DBX switches tabs.
    }
  }, 20);
}

function stopCommandMarkerTick() {
  if (commandMarkerTimer) {
    window.clearInterval(commandMarkerTimer);
    commandMarkerTimer = 0;
  }
  commandMarkerElapsed.value = null;
}

function startCommandMarkerTick(startedAt: number) {
  stopCommandMarkerTick();
  commandMarkerTimer = window.setInterval(() => {
    commandMarkerElapsed.value = runningCommandElapsedMs(startedAt, Date.now());
  }, 1000);
  commandMarkerElapsed.value = runningCommandElapsedMs(startedAt, Date.now());
}

function resetCommandMarker() {
  commandMarkerParser.reset();
  stopCommandMarkerTick();
  commandMarker.installed = false;
  commandMarker.active = false;
  commandMarker.command = "";
  commandMarker.exitCode = null;
  commandMarker.durationMs = null;
  commandMarker.cwd = "";
  commandMarker.startedAt = null;
}

function applyCommandMarker(updates: Osc633StreamUpdates) {
  if (updates.shellIntegrationInstalled !== undefined) commandMarker.installed = updates.shellIntegrationInstalled;
  if (updates.commandActive !== undefined) commandMarker.active = updates.commandActive;
  if (updates.command !== undefined) commandMarker.command = updates.command;
  // A fresh "E" frame starts a new command: clear the previous result so the
  // strip flips to the running state. The "A" frame's lastExitCode=null reset
  // is ignored on purpose — the finished result stays visible at the prompt
  // until the next command starts.
  if (updates.commandActive === true) {
    commandMarker.exitCode = null;
    commandMarker.durationMs = null;
    // A fresh "E" frame also starts the 1s tick so the marker strip shows a
    // live duration while the command runs; the final durationMs from the
    // "D" frame takes over once the tick stops.
    commandMarker.startedAt = Date.now();
    startCommandMarkerTick(commandMarker.startedAt);
  } else if (updates.commandActive === false) {
    stopCommandMarkerTick();
  }
  if (updates.lastExitCode !== undefined && updates.lastExitCode !== null) commandMarker.exitCode = updates.lastExitCode;
  if (updates.lastCommandDuration !== undefined) commandMarker.durationMs = updates.lastCommandDuration;
  if (updates.cwd !== undefined) {
    commandMarker.cwd = updates.cwd;
    // OSC 633 Cwd doubles as a directory-follow fallback when the backend could
    // not install OSC 7 tracking but the remote shell integration emits 633 frames.
    if (followDirectory.value && directoryTrackingSupported.value === false && updates.cwd) {
      void loadDirectory(updates.cwd, true);
    }
  }
}

function writeTerminalOutput(data: Uint8Array) {
  for (const path of directoryParser.push(data)) {
    if (followDirectory.value) void loadDirectory(path, true);
  }
  applyCommandMarker(commandMarkerParser.push(data));
  terminal?.write(data);
}

function resetZmodemSentry() {
  zmodemSentry = createZmodemSentry({
    send: sendTerminalBytes,
    toTerminal: writeTerminalOutput,
    onDetect: handleZmodemDetection,
    onRetract() {},
  });
}

function handleZmodemDetection(detection: ZmodemDetection) {
  if (!pendingZmodemFiles.length || detection.get_session_role() !== "send") {
    detection.deny();
    if (pendingZmodemFiles.length) finishZmodemUpload(new Error(t("zmodemUploadOnly")));
    return;
  }
  try {
    zmodemSession = detection.confirm();
  } catch (cause) {
    finishZmodemUpload(cause);
    return;
  }
  window.clearTimeout(zmodemDetectionTimer);
  zmodemState.value = "uploading";
  zmodemSampledAt = performance.now();
  zmodemSampledBytes = 0;
  const files = pendingZmodemFiles;
  void sendZmodemFiles(zmodemSession, files, updateZmodemProgress)
    .then(() => {
      showNotice(t("zmodemUploadComplete", { count: files.length }));
      finishZmodemUpload();
      void loadDirectory();
    })
    .catch(finishZmodemUpload);
}

function updateZmodemProgress(progress: ZmodemUploadProgress) {
  zmodemFileName.value = progress.file.name;
  zmodemTransferred.value = progress.totalTransferred;
  zmodemTotalSize.value = progress.totalSize;
  const now = performance.now();
  const elapsed = now - zmodemSampledAt;
  if (elapsed >= 250 || progress.totalTransferred === progress.totalSize) {
    const speed = elapsed > 0 ? ((progress.totalTransferred - zmodemSampledBytes) * 1000) / elapsed : 0;
    zmodemSpeed.value = zmodemSpeed.value ? zmodemSpeed.value * 0.65 + speed * 0.35 : speed;
    zmodemSampledAt = now;
    zmodemSampledBytes = progress.totalTransferred;
  }
}

function finishZmodemUpload(cause?: unknown) {
  const wasActive = zmodemState.value !== "idle";
  cancelZmodemUpload();
  if (!wasActive) return;
  if (cause) showError(new Error(t("zmodemUploadFailed", { error: cause instanceof Error ? cause.message : String(cause) })), "terminal");
  terminal?.focus();
}

/**
 * Silently tears the ZMODEM state down (abort the wire session, drop pending
 * files, reset the overlay, rebuild the sentry). Used both after a completed
 * or failed upload and when the SSH session is closed mid-transfer — without
 * it a closed session would leave zmodemBusy stuck true and terminal input
 * routed into a dead sentry.
 */
function cancelZmodemUpload() {
  window.clearTimeout(zmodemDetectionTimer);
  if (zmodemSession && !zmodemSession.has_ended()) {
    try { zmodemSession.abort(); } catch {}
  }
  pendingZmodemFiles = [];
  zmodemSession = null;
  zmodemState.value = "idle";
  zmodemFileName.value = "";
  zmodemTransferred.value = 0;
  zmodemTotalSize.value = 0;
  zmodemSpeed.value = 0;
  resetZmodemSentry();
}

function handleBinary(event: DbxPluginBinaryEvent) {
  const sessionId = activeTerminalSessionId || session.value?.sessionId;
  if (sessionId && event.channel === `ssh/terminal/out/${sessionId}`) {
    const payload = window.dbxPlugin.decodeBase64(event.dataBase64);
    if (payload.length < 9) return;
    const sequence = readU64(payload, 1);
    if (sequence <= lastSequence) return;
    pendingTerminalFrames.set(sequence, { stream: payload[0], data: payload.slice(9) });
    drainTerminalFrames();
    return;
  }
  const taskId = event.channel.startsWith("sftp/download/") ? event.channel.slice("sftp/download/".length) : "";
  const waiter = downloadChunkWaiters.get(taskId);
  if (!waiter) return;
  const payload = window.dbxPlugin.decodeBase64(event.dataBase64);
  if (payload.length < 8 || readU64(payload, 0) !== waiter.offset) return;
  window.clearTimeout(waiter.timer);
  downloadChunkWaiters.delete(taskId);
  waiter.resolve(payload.slice(8));
}

function drainTerminalFrames() {
  let frame = pendingTerminalFrames.get(lastSequence + 1);
  while (frame) {
    pendingTerminalFrames.delete(lastSequence + 1);
    lastSequence += 1;
    if (frame.stream === 2) {
      const state = new TextDecoder().decode(frame.data);
      if (state === "directory-tracking-unavailable") {
        followDirectory.value = false;
        directoryTrackingSupported.value = false;
        showNotice(t("directoryTrackingUnavailable"));
      } else {
        terminalState.value = "disconnected";
        terminalError.value = state === "ssh-transport-disconnected" ? t("transportDisconnected") : state || t("disconnected");
      }
    } else {
      try {
        if (!zmodemSentry) resetZmodemSentry();
        zmodemSentry?.consume(frame.data.slice().buffer);
      } catch (cause) {
        if (zmodemBusy.value) finishZmodemUpload(cause);
        else {
          resetZmodemSentry();
          writeTerminalOutput(frame.data);
        }
      }
    }
    frame = pendingTerminalFrames.get(lastSequence + 1);
  }
  persistState();
  // A stalled gap must not grow the pending map without bound: once the
  // buffer overshoots, drop it and let the replay re-deliver everything
  // after the last in-order sequence.
  if (pendingTerminalFrames.size > TERMINAL_PENDING_FRAME_LIMIT) {
    pendingTerminalFrames.clear();
  }
  const firstPending = Math.min(...pendingTerminalFrames.keys());
  if (Number.isFinite(firstPending) && firstPending > lastSequence + 1 && !replayInFlight && session.value) {
    replayInFlight = true;
    void window.dbxPlugin.invoke<ReplayResult>("ssh/terminal/replay", { sessionId: session.value.sessionId, afterSequence: lastSequence })
      .then((result) => {
        if (!result.complete) {
          terminalState.value = "error";
          terminalError.value = t("sessionUnrecoverable");
        }
      })
      .catch((cause) => showError(cause, "terminal"))
      .finally(() => {
        replayInFlight = false;
        drainTerminalFrames();
      });
  }
}

function handleEvent(event: DbxPluginEvent) {
  if (event.method === "ssh/terminal/inputAck") {
    const sequence = Number(event.params.sequence);
    const waiter = terminalInputAckWaiters.get(sequence);
    if (waiter) {
      window.clearTimeout(waiter.timer);
      terminalInputAckWaiters.delete(sequence);
      waiter.resolve();
    }
    return;
  }
  if (event.method === "ssh/host-key/prompt" || event.method === "connection/challenge") {
    hostKeyPrompt.value = event.params as unknown as HostKeyPrompt;
    return;
  }
  if (event.method === "ssh/host-key/notice") {
    showError(String(event.params.message || "SSH host-key warning"), "terminal");
    return;
  }
  if (event.method === "ssh/session/state" && event.params.sessionId === session.value?.sessionId) {
    if (event.params.state === "disconnected") {
      terminalState.value = "disconnected";
      reconnectPending.value = false;
      terminalError.value = t("transportDisconnected");
    }
    return;
  }
  if (event.method === "sftp/upload/ack") {
    const taskId = String(event.params.taskId || "");
    const waiter = uploadAckWaiters.get(taskId);
    if (waiter && Number(event.params.nextOffset) === waiter.nextOffset) {
      window.clearTimeout(waiter.timer);
      uploadAckWaiters.delete(taskId);
      waiter.resolve();
    }
    return;
  }
  if (event.method === "sftp/transfer/progress") updateTransfer(event.params);
}

function updateTransfer(params: Record<string, unknown>) {
  const taskId = String(params.taskId || "");
  if (!taskId) return;
  const existing = transferTasks[taskId];
  const transferred = Number(params.transferred ?? existing?.transferred ?? 0);
  const sample = sampleTransferSpeed(transferSamples.get(taskId), transferred, performance.now());
  transferSamples.set(taskId, sample);
  transferSpeeds[taskId] = sample.speed;
  transferTasks[taskId] = {
    taskId,
    sessionId: String(params.sessionId || existing?.sessionId || ""),
    direction: params.direction === "download" ? "download" : existing?.direction || "upload",
    fileName: String(params.fileName || existing?.fileName || ""),
    size: Number(params.size ?? existing?.size ?? 0),
    transferred,
    status: normalizeTransferStatus(params.status, existing?.status),
    error: typeof params.error === "string" ? params.error : existing?.error,
  };
  if (!existing && (transferTasks[taskId].status === "queued" || transferTasks[taskId].status === "running")) openTransferPanel();
}

function normalizeTransferStatus(value: unknown, fallback: TransferTask["status"] = "running"): TransferTask["status"] {
  return ["queued", "running", "completed", "cancelled", "failed"].includes(String(value)) ? String(value) as TransferTask["status"] : fallback;
}

async function openSession(forceNew = false) {
  if (!connectionId.value || !workbenchId.value) return;
  window.clearTimeout(reconnectTimer);
  reconnectAttempt = 0;
  // A session opened over a stale one must not inherit a stuck ZMODEM
  // overlay (zmodemBusy would keep swallowing terminal input).
  cancelZmodemUpload();
  terminalState.value = "connecting";
  terminalError.value = "";
  reconnectPending.value = false;
  resetCommandMarker();
  if (forceNew && session.value) await closeSession(false);
  createTerminal();
  try {
    const info = await window.dbxPlugin.invoke<SessionInfo>("ssh/session/open", {
      connectionId: connectionId.value,
      workbenchId: workbenchId.value,
      cols: terminal?.cols || 120,
      rows: terminal?.rows || 32,
    }, { timeoutMs: 120_000 });
    activeTerminalSessionId = info.sessionId;
    session.value = info;
    lastSequence = 0;
    directoryTrackingSupported.value = info.directoryTrackingSupported ?? true;
    terminalState.value = "connected";
    const replay = await window.dbxPlugin.invoke<ReplayResult>("ssh/terminal/replay", {
      sessionId: info.sessionId,
      afterSequence: 0,
    });
    if (!replay.complete) throw new Error(t("sessionUnrecoverable"));
    await afterSessionConnected();
  } catch (cause) {
    terminalState.value = "error";
    activeTerminalSessionId = "";
    showError(cause, "terminal");
  }
}

async function attachSession(sessionId: string) {
  terminalState.value = "connecting";
  activeTerminalSessionId = sessionId;
  try {
    const info = await window.dbxPlugin.invoke<SessionInfo>("ssh/session/attach", {
      connectionId: connectionId.value,
      workbenchId: workbenchId.value,
      afterSequence: lastSequence,
    }, { timeoutMs: 15_000 });
    if (info.sessionId !== sessionId) throw new Error("The attached SSH session changed unexpectedly");
    session.value = info;
    terminalState.value = "connected";
    reconnectPending.value = false;
    if (info.replay && !info.replay.complete) {
      terminalState.value = "error";
      terminalError.value = t("sessionUnrecoverable");
      return;
    }
    reconnectAttempt = 0;
    await afterSessionConnected();
  } catch (cause) {
    if (!shouldReattachTerminal({ disposed, state: terminalState.value, expectedSessionId: sessionId, currentSessionId: initialState().sessionId })) {
      terminalState.value = "error";
      reconnectPending.value = false;
      activeTerminalSessionId = "";
      showError(cause, "terminal");
      return;
    }
    const delay = terminalReconnectDelay(reconnectAttempt++);
    reconnectNextAt = Date.now() + delay;
    reconnectDelayMs = delay;
    reconnectPending.value = true;
    reconnectTimer = window.setTimeout(() => void attachSession(sessionId), delay);
    terminalError.value = t("reattachingTerminal");
  }
}

async function afterSessionConnected() {
  terminalError.value = "";
  terminal?.focus();
  scheduleFit();
  await writeWorkbenchState();
  if (followDirectory.value) await setDirectoryTracking(true);
  void refreshQuickSudoSetting();
  await Promise.all([loadDirectory(currentPath.value), restoreTransfers()]);
}

async function closeSession(updateStatus = true) {
  const sessionId = session.value?.sessionId;
  session.value = undefined;
  activeTerminalSessionId = "";
  quickSudo.value = false;
  // Closing mid-ZMODEM aborts the transfer silently instead of leaving the
  // busy overlay and the dead sentry attached to the workbench.
  cancelZmodemUpload();
  pendingTerminalFrames.clear();
  lastSequence = 0;
  terminalInputSequence = 0;
  reconnectPending.value = false;
  resetCommandMarker();
  if (sessionId) await window.dbxPlugin.invoke("ssh/session/close", { sessionId }).catch(() => undefined);
  if (updateStatus) {
    terminalState.value = "disconnected";
    terminalError.value = t("disconnected");
  }
  persistState();
}

async function reconnect() {
  terminal?.clear();
  await closeSession(false);
  await openSession();
}

async function restoreTransfers() {
  if (!session.value) return;
  const result = await window.dbxPlugin.invoke<{ tasks: TransferTask[] }>("sftp/transfer/list", { sessionId: session.value.sessionId }).catch(() => ({ tasks: [] }));
  for (const task of result.tasks) transferTasks[task.taskId] = task;
}

async function resolveHostKey(accept: boolean) {
  const prompt = hostKeyPrompt.value;
  if (!prompt) return;
  hostKeyPrompt.value = undefined;
  try {
    await window.dbxPlugin.invoke("connection/challenge/resolve", {
      challengeId: prompt.challengeId,
      operationId: prompt.operationId,
      accept,
      remember: accept && rememberHostKey.value,
    });
  } catch (cause) {
    showError(cause, "terminal");
  }
}

async function loadHome() {
  if (!session.value) return;
  const result = await window.dbxPlugin.invoke<{ path: string }>("sftp/home", { sessionId: session.value.sessionId });
  await loadDirectory(result.path);
}

async function loadDirectory(path = currentPath.value, fromTerminal = false) {
  if (!session.value) return;
  const normalized = normalizeRemotePath(path);
  loadingFiles.value = true;
  if (!fromTerminal) sftpError.value = "";
  try {
    const result = await window.dbxPlugin.invoke<{ entries: SftpEntry[] }>(sudoMode.value ? "sudo/listDir" : "sftp/list", {
      sessionId: session.value.sessionId,
      path: normalized,
    });
    entries.value = result.entries;
    currentPath.value = normalized;
    selectedPath.value = "";
    clearRowSelection();
    rememberPathHistory(normalized);
    persistState();
    void refreshDiskUsage();
  } catch (cause) {
    const message = cause instanceof Error ? cause.message : String(cause);
    if (fromTerminal) showNotice(t("followDirectoryFailed", { path: normalized, error: message }));
    else sftpError.value = message;
  } finally {
    loadingFiles.value = false;
  }
}

function toggleSudoMode() {
  if (!connected.value || !canWrite.value || loadingFiles.value) return;
  sudoMode.value = !sudoMode.value;
  persistState();
  void loadDirectory();
}

async function refreshQuickSudoSetting() {
  const sessionId = session.value?.sessionId;
  if (!sessionId) {
    quickSudo.value = false;
    return;
  }
  try {
    const meta = await window.dbxPlugin.invoke<SshSettings>("ssh/settings/get", { sessionId });
    quickSudo.value = meta.quickSudo === true;
  } catch {
    quickSudo.value = false;
  }
}

async function toggleQuickSudo() {
  if (!connected.value || quickSudoSubmitting.value) return;
  const sessionId = session.value?.sessionId;
  if (!sessionId) return;
  terminalMenu.value = undefined;
  const previous = quickSudo.value;
  const next = !previous;
  quickSudo.value = next;
  quickSudoSubmitting.value = true;
  try {
    await window.dbxPlugin.invoke<SshSettings>("ssh/settings/set", { sessionId, quickSudo: next });
  } catch (cause) {
    quickSudo.value = previous;
    showError(cause, "terminal");
  } finally {
    quickSudoSubmitting.value = false;
  }
}

function goParent() {
  void loadDirectory(parentPath(currentPath.value));
}

async function setDirectoryTracking(enabled: boolean) {
  if (!session.value) return;
  if (enabled && directoryTrackingSupported.value === false) {
    showNotice(t("directoryTrackingUnsupported"));
    followDirectory.value = false;
    return;
  }
  if (pendingTerminalInput) {
    showNotice(t("followDirectoryInputPending"));
    return;
  }
  try {
    await window.dbxPlugin.invoke("ssh/terminal/directoryTracking", { sessionId: session.value.sessionId, enabled });
    followDirectory.value = enabled;
    directoryParser.reset();
    persistState();
  } catch (cause) {
    showError(cause, "terminal");
  }
}

function togglePaneOrder() {
  paneOrder.value = paneOrder.value === "terminal-left" ? "sftp-left" : "terminal-left";
  persistState();
  void nextTick(scheduleFit);
}

function startDividerDrag(event: PointerEvent) {
  const container = paneContainer.value;
  if (!container) return;
  const pointerId = event.pointerId;
  const move = (next: PointerEvent) => {
    const bounds = container.getBoundingClientRect();
    const fromLeft = ((next.clientX - bounds.left) / bounds.width) * 100;
    const terminalPercent = paneOrder.value === "terminal-left" ? fromLeft : 100 - fromLeft;
    splitRatio.value = Math.max(35, Math.min(80, terminalPercent));
    scheduleFit();
  };
  const stop = () => {
    container.releasePointerCapture(pointerId);
    container.removeEventListener("pointermove", move);
    container.removeEventListener("pointerup", stop);
    container.removeEventListener("pointercancel", stop);
    persistState();
  };
  container.setPointerCapture(pointerId);
  container.addEventListener("pointermove", move);
  container.addEventListener("pointerup", stop);
  container.addEventListener("pointercancel", stop);
}

function toggleColumn(column: SftpColumn) {
  visibleColumns.value = visibleColumns.value.includes(column) ? visibleColumns.value.filter((value) => value !== column) : [...visibleColumns.value, column];
  persistState();
}

function toggleSort(column: SftpSortColumn) {
  sort.value = sort.value.column === column ? { column, direction: sort.value.direction === "asc" ? "desc" : "asc" } : { column, direction: "asc" };
}

function sortIcon(column: SftpSortColumn) {
  if (sort.value.column !== column) return ArrowUpDown;
  return sort.value.direction === "asc" ? ArrowUp : ArrowDown;
}

async function openEntry(entry: SftpEntry) {
  if (previewOpen.value && previewDirty.value && !window.confirm(t("editSave.closeConfirm"))) return;
  if (entry.kind === "directory") {
    await loadDirectory(pathFromUri(entry.uri));
    return;
  }
  if (entry.kind !== "file") return;
  if (isImagePreviewable(entry)) {
    await openImagePreview(entry, IMAGE_MIME_BY_EXTENSION[imagePreviewExtension(entry.name)]);
    return;
  }
  // Size-0 files have nothing to sniff: always open them in the text editor so
  // their content can be created from scratch. Anything else unpreviewable downloads.
  if (!isPreviewable(entry) && (entry.size || 0) > 0) {
    await downloadEntry(entry);
    return;
  }
  previewMode.value = "text";
  previewImageUrl.value = "";
  previewImageZoomed.value = false;
  previewBinary.value = false;
  previewOpen.value = true;
  previewLoading.value = true;
  previewTitle.value = entry.name;
  previewText.value = "";
  previewPath.value = pathFromUri(entry.uri);
  previewSize.value = entry.size || 0;
  previewEditable.value = false;
  previewDraft.value = "";
  previewBaseline.value = "";
  try {
    // sudo 模式下文本文件改走 sudo/readFile，避免无权限文件预览失败。
    const result = sudoMode.value
      ? await window.dbxPlugin.invoke<{ dataBase64: string; truncated: boolean }>("sudo/readFile", {
          sessionId: session.value?.sessionId,
          path: pathFromUri(entry.uri),
          length: MAX_INLINE_PREVIEW_BYTES,
        })
      : await window.dbxPlugin.invoke<{ dataBase64: string; truncated: boolean }>("sftp/read", {
          sessionId: session.value?.sessionId,
          path: pathFromUri(entry.uri),
          maxBytes: MAX_INLINE_PREVIEW_BYTES,
        });
    if (result.truncated) {
      previewOpen.value = false;
      await downloadEntry(entry);
      return;
    }
    const bytes = window.dbxPlugin.decodeBase64(result.dataBase64);
    previewBinary.value = hasBinaryExtension(entry.name) || containsNullByte(bytes);
    previewText.value = new TextDecoder("utf-8", { fatal: false }).decode(bytes);
    previewBaseline.value = previewText.value;
  } catch (cause) {
    previewText.value = cause instanceof Error ? cause.message : String(cause);
    previewBaseline.value = previewText.value;
  } finally {
    previewLoading.value = false;
  }
}

async function openImagePreview(entry: SftpEntry, mime: string) {
  previewMode.value = "image";
  previewBinary.value = false;
  previewImageUrl.value = "";
  previewImageZoomed.value = false;
  previewOpen.value = true;
  previewLoading.value = true;
  previewTitle.value = entry.name;
  previewText.value = "";
  previewPath.value = pathFromUri(entry.uri);
  previewSize.value = entry.size || 0;
  previewEditable.value = false;
  previewDraft.value = "";
  previewBaseline.value = "";
  try {
    const result = await window.dbxPlugin.invoke<{ dataBase64: string; truncated: boolean }>("sftp/read", {
      sessionId: session.value?.sessionId,
      path: pathFromUri(entry.uri),
      maxBytes: MAX_IMAGE_PREVIEW_BYTES,
    });
    if (result.truncated) {
      previewOpen.value = false;
      await downloadEntry(entry);
      return;
    }
    previewImageUrl.value = `data:image/${mime};base64,${result.dataBase64}`;
  } catch (cause) {
    previewOpen.value = false;
    showError(cause);
  } finally {
    previewLoading.value = false;
  }
}

function fileExtension(name: string) {
  return name.includes(".") ? name.split(".").pop()?.toLowerCase() || "" : "";
}

function imagePreviewExtension(name: string) {
  const extension = fileExtension(name);
  return extension in IMAGE_MIME_BY_EXTENSION ? extension : "";
}

function isImagePreviewable(entry: SftpEntry) {
  const size = entry.size || 0;
  return !!imagePreviewExtension(entry.name) && size > 0 && size <= MAX_IMAGE_PREVIEW_BYTES;
}

function hasBinaryExtension(name: string) {
  return BINARY_PREVIEW_EXTENSIONS.has(fileExtension(name));
}

function containsNullByte(bytes: Uint8Array) {
  return bytes.includes(0);
}

function isPreviewable(entry: SftpEntry) {
  if ((entry.size || 0) > MAX_INLINE_PREVIEW_BYTES) return false;
  return PREVIEWABLE_EXTENSIONS.has(fileExtension(entry.name));
}

function confirmDiscardPreviewEdits() {
  return !previewDirty.value || window.confirm(t("editSave.closeConfirm"));
}

function closePreview() {
  if (!confirmDiscardPreviewEdits()) return;
  previewOpen.value = false;
  previewEditable.value = false;
  previewDraft.value = "";
  previewImageUrl.value = "";
  previewImageZoomed.value = false;
}

function beginPreviewEdit() {
  if (!previewEditableAllowed.value) return;
  previewDraft.value = previewText.value;
  previewEditable.value = true;
}

function cancelPreviewEdit() {
  if (!confirmDiscardPreviewEdits()) return;
  previewEditable.value = false;
  previewDraft.value = "";
}

async function savePreview() {
  const sessionId = session.value?.sessionId;
  if (!sessionId || previewSaving.value || !previewPath.value) return;
  previewSaving.value = true;
  try {
    const bytes = new TextEncoder().encode(previewDraft.value);
    if (bytes.byteLength > MAX_DIRECT_WRITE_BYTES) {
      showError(new Error(t("sftpAttrs.sizeLimit")));
      return;
    }
    const dataBase64 = window.dbxPlugin.encodeBase64(bytes);
    if (sudoMode.value) {
      await window.dbxPlugin.invoke("sudo/writeFile", {
        sessionId,
        path: previewPath.value,
        dataBase64,
      });
    } else {
      await window.dbxPlugin.invoke("sftp/write", {
        sessionId,
        remotePath: previewPath.value,
        dataBase64,
      });
    }
    previewText.value = previewDraft.value;
    previewSize.value = bytes.byteLength;
    previewBaseline.value = previewText.value;
    previewEditable.value = false;
    previewDraft.value = "";
    showNotice(t("editSave.saved", { name: previewTitle.value }));
    await loadDirectory();
  } catch (cause) {
    showError(cause);
  } finally {
    previewSaving.value = false;
  }
}

function isArchiveName(name: string) {
  return /\.(tar\.gz|tgz|tar)$/i.test(name);
}

function archiveDirectoryName(name: string) {
  if (/\.(tar\.gz|tgz)$/i.test(name)) return name.replace(/\.(tar\.gz|tgz)$/i, "");
  return name.replace(/\.tar$/i, "");
}

async function archiveEntry(entry: SftpEntry) {
  const sessionId = session.value?.sessionId;
  if (!sessionId || archiveBusy.value || entry.kind !== "directory") return;
  fileMenu.value = undefined;
  archiveBusy.value = true;
  const archiveName = `${entry.name}.tar.gz`;
  try {
    await window.dbxPlugin.invoke("sftp/archive", {
      sessionId,
      sourcePaths: [pathFromUri(entry.uri)],
      archivePath: joinRemote(currentPath.value, archiveName),
    }, { timeoutMs: 30 * 60 * 1000 });
    showNotice(t("archive.done", { name: archiveName }));
    await loadDirectory();
  } catch (cause) {
    showError(cause);
  } finally {
    archiveBusy.value = false;
  }
}

async function extractEntry(entry: SftpEntry) {
  const sessionId = session.value?.sessionId;
  if (!sessionId || archiveBusy.value) return;
  fileMenu.value = undefined;
  archiveBusy.value = true;
  const directoryName = archiveDirectoryName(entry.name);
  try {
    await window.dbxPlugin.invoke("sftp/extract", {
      sessionId,
      archivePath: pathFromUri(entry.uri),
      destinationPath: joinRemote(currentPath.value, directoryName),
      overwrite: false,
    }, { timeoutMs: 30 * 60 * 1000 });
    showNotice(t("extract.done", { name: directoryName }));
    await loadDirectory();
  } catch (cause) {
    showError(cause);
  } finally {
    archiveBusy.value = false;
  }
}

function beginRename(entry: SftpEntry) {
  if (!canWrite.value) return;
  selectedPath.value = entry.uri;
  renamingPath.value = entry.uri;
  renameDraft.value = entry.name;
  void nextTick(() => document.querySelector<HTMLInputElement>(".rename-input")?.select());
}

async function commitRename(entry: SftpEntry) {
  const name = renameDraft.value.trim();
  if (!session.value || !name || name === entry.name) {
    renamingPath.value = "";
    return;
  }
  renameSubmitting.value = true;
  try {
    const sourcePath = pathFromUri(entry.uri);
    const targetPath = joinRemote(currentPath.value, name);
    if (sudoMode.value) {
      await window.dbxPlugin.invoke("sudo/rename", { sessionId: session.value.sessionId, sourcePath, targetPath });
    } else {
      await window.dbxPlugin.invoke("sftp/rename", { sessionId: session.value.sessionId, sourcePath, targetPath });
    }
    renamingPath.value = "";
    await loadDirectory();
  } catch (cause) {
    showError(cause);
  } finally {
    renameSubmitting.value = false;
  }
}

async function createDirectory() {
  const name = operationDraft.value.trim();
  if (!session.value || !name) return;
  const path = joinRemote(currentPath.value, name);
  try {
    if (sudoMode.value) {
      await window.dbxPlugin.invoke("sudo/mkdir", { sessionId: session.value.sessionId, path });
    } else {
      await window.dbxPlugin.invoke("sftp/createDirectory", { sessionId: session.value.sessionId, path });
    }
    operationDialog.value = null;
    await loadDirectory();
  } catch (cause) {
    showError(cause);
  }
}

async function confirmDelete() {
  if (!session.value || !deleteTarget.value) return;
  deleteSubmitting.value = true;
  try {
    const path = pathFromUri(deleteTarget.value.uri);
    if (sudoMode.value) {
      await window.dbxPlugin.invoke(deleteTarget.value.kind === "directory" ? "sudo/removeAll" : "sudo/remove", {
        sessionId: session.value.sessionId,
        path,
      });
    } else {
      await window.dbxPlugin.invoke("sftp/delete", {
        sessionId: session.value.sessionId,
        path,
        recursive: deleteTarget.value.kind === "directory",
      });
    }
    deleteTarget.value = undefined;
    await loadDirectory();
    showNotice(t("deleted"));
  } catch (cause) {
    showError(cause);
  } finally {
    deleteSubmitting.value = false;
  }
}

// ---------------------------------------------------------------------------
// SFTP 面板：搜索/多选/批量/新建文件/属性/路径历史/复制粘贴
// ---------------------------------------------------------------------------

function loadPathHistories(): Record<string, string[]> {
  try {
    const raw = window.localStorage.getItem(SFTP_PATH_HISTORY_KEY);
    const parsed = raw ? JSON.parse(raw) : null;
    return sanitizePathHistories(parsed, SFTP_PATH_HISTORY_LIMIT);
  } catch {
    return {};
  }
}

function persistPathHistories() {
  try {
    window.localStorage.setItem(SFTP_PATH_HISTORY_KEY, JSON.stringify(pathHistories));
  } catch {
    // localStorage 不可用时路径历史仅保留在内存中。
  }
}

function rememberPathHistory(path: string) {
  const key = connectionId.value;
  if (!key || !path) return;
  const next = pushPathHistory(pathHistories, key, path, SFTP_PATH_HISTORY_LIMIT);
  for (const connection of Object.keys(next)) pathHistories[connection] = next[connection];
  persistPathHistories();
}

function clearRowSelection() {
  selectedUris.value = [];
  lastClickedUri.value = "";
}

function selectFile(entry: SftpEntry, event?: MouseEvent) {
  selectedPath.value = entry.uri;
  if (event?.shiftKey && lastClickedUri.value) {
    const expanded = expandSelection(selectedUris.value, lastClickedUri.value, entry.uri, visibleEntries.value.map((item) => item.uri));
    if (expanded.length > selectedUris.value.length || selectedUris.value.includes(entry.uri)) {
      selectedUris.value = expanded;
      return;
    }
  }
  if (event?.ctrlKey || event?.metaKey) {
    selectedUris.value = selectedUris.value.includes(entry.uri)
      ? selectedUris.value.filter((uri) => uri !== entry.uri)
      : [...selectedUris.value, entry.uri];
  } else {
    selectedUris.value = [entry.uri];
  }
  lastClickedUri.value = entry.uri;
}

async function confirmBatchDelete() {
  const sessionId = session.value?.sessionId;
  const targets = selectedEntries.value;
  if (!sessionId || !targets.length || batchDeleteSubmitting.value) return;
  batchDeleteSubmitting.value = true;
  let progress = createBatchProgress(targets.length);
  batchProgress.value = progress;
  try {
    for (const entry of targets) {
      const path = pathFromUri(entry.uri);
      try {
        if (sudoMode.value) {
          await window.dbxPlugin.invoke(entry.kind === "directory" ? "sudo/removeAll" : "sudo/remove", { sessionId, path });
        } else {
          await window.dbxPlugin.invoke("sftp/delete", { sessionId, path, recursive: entry.kind === "directory" });
        }
        progress = advanceBatchProgress(progress, { name: entry.name, ok: true });
      } catch (cause) {
        progress = advanceBatchProgress(progress, { name: entry.name, ok: false });
        throw cause;
      }
      batchProgress.value = progress;
    }
    batchDeleteOpen.value = false;
    clearRowSelection();
    await loadDirectory();
    showNotice(t("deleted"));
  } catch (cause) {
    showError(cause);
    await loadDirectory();
  } finally {
    batchDeleteSubmitting.value = false;
    batchProgress.value = null;
  }
}

async function batchArchive() {
  const sessionId = session.value?.sessionId;
  const targets = selectedEntries.value;
  if (!sessionId || !targets.length || archiveBusy.value) return;
  archiveBusy.value = true;
  let progress = createBatchProgress(targets.length);
  batchProgress.value = progress;
  try {
    let done = 0;
    for (const entry of targets) {
      const archiveName = `${entry.name}.tar.gz`;
      try {
        await window.dbxPlugin.invoke("sftp/archive", {
          sessionId,
          sourcePaths: [pathFromUri(entry.uri)],
          archivePath: joinRemote(currentPath.value, archiveName),
        }, { timeoutMs: 30 * 60 * 1000 });
        progress = advanceBatchProgress(progress, { name: archiveName, ok: true });
      } catch (cause) {
        progress = advanceBatchProgress(progress, { name: archiveName, ok: false });
        throw cause;
      }
      batchProgress.value = progress;
      done += 1;
    }
    showNotice(t("sftpBatch.archiveDone", { count: done }));
    await loadDirectory();
  } catch (cause) {
    showError(cause);
    await loadDirectory();
  } finally {
    archiveBusy.value = false;
    batchProgress.value = null;
  }
}

function openNewFileDialog() {
  if (!connected.value || !canWrite.value) return;
  newFileDraft.value = "";
  newFileDialog.value = true;
}

async function createNewFile() {
  const sessionId = session.value?.sessionId;
  const name = newFileDraft.value.trim();
  if (!sessionId || !name || newFileSubmitting.value) return;
  newFileSubmitting.value = true;
  try {
    await window.dbxPlugin.invoke(sudoMode.value ? "sudo/touch" : "sftp/touch", {
      sessionId,
      path: joinRemote(currentPath.value, name),
    });
    newFileDialog.value = false;
    showNotice(t("sftpNewFile.done", { name }));
    await loadDirectory();
  } catch (cause) {
    showError(cause);
  } finally {
    newFileSubmitting.value = false;
  }
}

async function openAttributes(entry: SftpEntry) {
  const sessionId = session.value?.sessionId;
  if (!sessionId) return;
  fileMenu.value = undefined;
  attrsTarget.value = entry;
  attrsInfo.value = undefined;
  attrsMode.value = entry.permissions || "";
  attrsLoading.value = true;
  try {
    attrsInfo.value = await window.dbxPlugin.invoke<SftpStatInfo>(sudoMode.value ? "sudo/stat" : "sftp/stat", {
      sessionId,
      path: pathFromUri(entry.uri),
    });
    if (attrsInfo.value?.mode) attrsMode.value = attrsInfo.value.mode;
  } catch (cause) {
    showError(cause);
  } finally {
    attrsLoading.value = false;
  }
}

function closeAttributes() {
  attrsTarget.value = undefined;
  attrsInfo.value = undefined;
}

async function saveAttributesPermissions() {
  const sessionId = session.value?.sessionId;
  const entry = attrsTarget.value;
  const mode = attrsMode.value.trim();
  if (!sessionId || !entry || !mode || attrsSubmitting.value) return;
  attrsSubmitting.value = true;
  try {
    await window.dbxPlugin.invoke(sudoMode.value ? "sudo/chmod" : "sftp/chmod", {
      sessionId,
      path: pathFromUri(entry.uri),
      mode,
    });
    showNotice(t("permissionsUpdated"));
    if (attrsInfo.value) attrsInfo.value = { ...attrsInfo.value, mode };
    await loadDirectory();
  } catch (cause) {
    showError(cause);
  } finally {
    attrsSubmitting.value = false;
  }
}

function remoteBasename(path: string) {
  const index = path.lastIndexOf("/");
  return index < 0 ? path : path.slice(index + 1);
}

function copySelectedEntries(mode: "copy" | "cut") {
  const entry = fileMenu.value?.entry;
  if (!entry) return;
  const uris = selectedUris.value.includes(entry.uri) && selectedUris.value.length > 1 ? selectedUris.value : [entry.uri];
  sftpClipboard.value = { mode, paths: uris.map((uri) => pathFromUri(uri)), connectionId: connectionId.value };
  fileMenu.value = undefined;
  showNotice(t("sftpCopy.done", { count: sftpClipboard.value.paths.length }));
}

async function pasteClipboard() {
  const clip = sftpClipboard.value;
  const sessionId = session.value?.sessionId;
  if (!sessionId || pasteBusy.value) return;
  if (!clip || clip.connectionId !== connectionId.value || !clip.paths.length) {
    showNotice(t("sftpPaste.empty"));
    return;
  }
  if (!canWrite.value) return;
  // 粘贴前逐项检测目标是否已存在；存在则弹覆盖确认。
  const conflicting: string[] = [];
  for (const from of clip.paths) {
    try {
      const result = await window.dbxPlugin.invoke<{ exists: boolean }>("sftp/exists", {
        sessionId,
        path: joinRemote(currentPath.value, remoteBasename(from)),
      });
      if (result.exists) conflicting.push(remoteBasename(from));
    } catch {
      // 存在性检测失败不阻断粘贴，交由后端执行时报错。
    }
  }
  let overwrite = false;
  if (conflicting.length) {
    if (!window.confirm(t("sftpPaste.overwriteConfirm", { count: conflicting.length, names: conflicting.slice(0, 5).join(", ") }))) return;
    overwrite = true;
  }
  pasteBusy.value = true;
  try {
    await window.dbxPlugin.invoke<{ success: boolean; results: Array<{ from: string; to: string; ok: boolean; error?: string }> }>(
      clip.mode === "cut" ? "sftp/move" : "sftp/copy",
      {
        connectionId: connectionId.value,
        from: clip.paths,
        toDir: currentPath.value,
        overwrite,
      },
      { timeoutMs: 30 * 60 * 1000 },
    );
    if (clip.mode === "cut") sftpClipboard.value = undefined;
    showNotice(t("sftpPaste.done", { count: clip.paths.length }));
    await loadDirectory();
  } catch (cause) {
    const message = cause instanceof Error ? cause.message : String(cause);
    if (/method not found/i.test(message)) showNotice(t("sftpPaste.backendMissing"));
    else showError(cause);
  } finally {
    pasteBusy.value = false;
  }
}

function goToPath(path: string) {
  pathHistoryOpen.value = false;
  void loadDirectory(path);
}

async function chooseUpload() {
  if (!connected.value || !canWrite.value) return;
  openTransferPanel();
  if (!window.dbxPlugin.fileTransfer) {
    uploadInput.value?.click();
    return;
  }
  try {
    const selection = await window.dbxPlugin.fileTransfer.pick({ multiple: true });
    await uploadHandleFiles(selection.files);
    await loadDirectory();
    if (selection.files.length) showNotice(t("uploaded", { count: selection.files.length }));
  } catch (cause) {
    showError(cause);
  }
}

async function uploadHandleFiles(files: Array<{ handleId: string; name: string; size: number }>) {
  if (!window.dbxPlugin.fileTransfer || !files.length) return;
  await runWithConcurrency(files, 3, async (file) => {
      try {
        await uploadSource(file.name, file.size, async (offset, length) => {
          const result = await window.dbxPlugin.fileTransfer!.read(file.handleId, offset, length);
          return window.dbxPlugin.decodeBase64(result.dataBase64);
        });
      } finally {
        await window.dbxPlugin.fileTransfer!.cancel(file.handleId).catch(() => undefined);
      }
  });
}

async function uploadLocalFiles(files: readonly File[]) {
  openTransferPanel();
  await runWithConcurrency([...files], 3, (file) => uploadSource(file.name, file.size, async (offset, length) => new Uint8Array(await file.slice(offset, offset + length).arrayBuffer())));
  await loadDirectory();
  if (files.length) showNotice(t("uploaded", { count: files.length }));
}

async function uploadSource(name: string, size: number, readChunk: (offset: number, length: number) => Promise<Uint8Array>) {
  if (!session.value) return;
  const info = await window.dbxPlugin.invoke<{ taskId: string; chunkSize: number }>("sftp/upload/start", {
    sessionId: session.value.sessionId,
    remotePath: joinRemote(currentPath.value, name),
    size,
  });
  transferTasks[info.taskId] = { taskId: info.taskId, sessionId: session.value.sessionId, direction: "upload", fileName: name, size, transferred: 0, status: "queued" };
  try {
    let offset = 0;
    while (offset < size) {
      const chunk = await readChunk(offset, info.chunkSize);
      if (!chunk.byteLength) throw new Error("Local file ended before its declared size");
      const payload = new Uint8Array(8 + chunk.byteLength);
      writeU64(payload, 0, offset);
      payload.set(chunk, 8);
      const nextOffset = offset + chunk.byteLength;
      const ack = waitForUploadAck(info.taskId, nextOffset);
      await window.dbxPlugin.sendBinary(`sftp/upload/${info.taskId}`, payload);
      await ack;
      offset = nextOffset;
    }
    await window.dbxPlugin.invoke("sftp/upload/finish", { taskId: info.taskId }, { timeoutMs: 30 * 60 * 1000 });
  } catch (cause) {
    await window.dbxPlugin.invoke("sftp/transfer/cancel", { taskId: info.taskId }).catch(() => undefined);
    throw cause;
  }
}

function waitForUploadAck(taskId: string, nextOffset: number) {
  return new Promise<void>((resolve, reject) => {
    const timer = window.setTimeout(async () => {
      uploadAckWaiters.delete(taskId);
      try {
        const status = await window.dbxPlugin.invoke<{ transferred: number; status: string }>("sftp/transfer/status", { taskId });
        if (status.transferred >= nextOffset && status.status === "running") resolve();
        else reject(new Error("SFTP upload acknowledgement timed out"));
      } catch (cause) {
        reject(cause instanceof Error ? cause : new Error(String(cause)));
      }
    }, 30_000);
    uploadAckWaiters.set(taskId, { nextOffset, resolve, reject, timer });
  });
}

async function downloadEntry(entry: SftpEntry) {
  fileMenu.value = undefined;
  openTransferPanel();
  if (!session.value || entry.kind !== "file") return;
  const fileTransfer = window.dbxPlugin.fileTransfer;
  // Web/Docker mode has no host save dialog; the whole file is buffered in browser
  // memory before saving, so warn before starting large downloads.
  if (!fileTransfer && (entry.size || 0) > WEB_DOWNLOAD_WARNING_BYTES && !window.confirm(t("webDownload.largeWarning", { name: entry.name, size: formatBytes(entry.size || 0) }))) return;
  let info: DownloadInfo | undefined;
  let target: { handleId: string; chunkBytes: number } | undefined;
  const chunks = fileTransfer ? undefined : ([] as Uint8Array[]);
  try {
    info = await window.dbxPlugin.invoke<DownloadInfo>("sftp/download/start", {
      sessionId: session.value.sessionId,
      remotePath: pathFromUri(entry.uri),
    });
    transferTasks[info.taskId] = { taskId: info.taskId, sessionId: session.value.sessionId, direction: "download", fileName: info.fileName, size: info.size, transferred: 0, status: "queued" };
    target = fileTransfer ? await fileTransfer.beginSave({ name: info.fileName, size: info.size }) : undefined;
    let offset = 0;
    while (offset < info.size) {
      const chunkPromise = waitForDownloadChunk(info.taskId, offset);
      const nextPromise = window.dbxPlugin.invoke<{ length: number; eof: boolean }>("sftp/download/next", { taskId: info.taskId, offset });
      // Cancellation interrupts via the chunk waiter; swallow the rejection of the
      // in-flight request so it cannot surface as an unhandled promise rejection.
      nextPromise.catch(() => undefined);
      const result = await nextPromise;
      const chunk = await chunkPromise;
      if (chunk.byteLength !== result.length) throw new Error("SFTP download chunk length mismatch");
      if (!result.eof && result.length === 0) throw new Error("SFTP download returned an empty chunk before end of file");
      if (chunks) {
        chunks.push(chunk);
        offset += chunk.byteLength;
        const task = transferTasks[info.taskId];
        if (task) {
          task.status = "running";
          task.transferred = offset;
        }
      } else {
        const write = await fileTransfer!.write(target!.handleId, offset, chunk);
        offset = write.nextOffset;
      }
      if (result.eof) break;
    }
    if (target) {
      await fileTransfer!.finish(target.handleId);
      target = undefined;
    } else if (chunks) {
      saveBrowserDownload(chunks, info.fileName);
      const task = transferTasks[info.taskId];
      if (task) task.status = "completed";
    }
    await window.dbxPlugin.invoke("sftp/download/finish", { taskId: info.taskId });
    cancelledTransferTasks.delete(info.taskId);
    showNotice(t("downloaded", { name: info.fileName }));
  } catch (cause) {
    if (info) {
      const waiter = downloadChunkWaiters.get(info.taskId);
      if (waiter) {
        window.clearTimeout(waiter.timer);
        downloadChunkWaiters.delete(info.taskId);
      }
    }
    if (target && fileTransfer) await fileTransfer.cancel(target.handleId).catch(() => undefined);
    if (info) await window.dbxPlugin.invoke("sftp/transfer/cancel", { taskId: info.taskId }).catch(() => undefined);
    if (info && cancelledTransferTasks.delete(info.taskId)) {
      const task = transferTasks[info.taskId];
      if (task) task.status = "cancelled";
      showNotice(t("transferStatus.cancelled"));
    } else {
      showError(cause);
    }
  }
}

function saveBrowserDownload(chunks: Uint8Array[], fileName: string) {
  // Runtime chunks always come from decodeBase64 (ArrayBuffer-backed); the
  // ArrayBufferLike generic just doesn't fit BlobPart's stricter view typing.
  const blob = new Blob(chunks as unknown as BlobPart[]);
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = fileName;
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  // Give the browser time to start the download before releasing the blob.
  window.setTimeout(() => URL.revokeObjectURL(url), 30_000);
}

function waitForDownloadChunk(taskId: string, offset: number) {
  return new Promise<Uint8Array>((resolve, reject) => {
    const timer = window.setTimeout(() => {
      downloadChunkWaiters.delete(taskId);
      reject(new Error("SFTP download chunk timed out"));
    }, 30_000);
    downloadChunkWaiters.set(taskId, { offset, resolve, reject, timer });
  });
}

async function cancelTransfer(task: TransferTask) {
  if (task.direction === "download") {
    // Reject the pending chunk waiter so the download loop exits immediately
    // instead of waiting for its 30s timeout; the backend cancel follows below.
    const waiter = downloadChunkWaiters.get(task.taskId);
    if (waiter) {
      window.clearTimeout(waiter.timer);
      downloadChunkWaiters.delete(task.taskId);
      waiter.reject(new Error(t("transferStatus.cancelled")));
    }
    cancelledTransferTasks.add(task.taskId);
  }
  await window.dbxPlugin.invoke("sftp/transfer/cancel", { taskId: task.taskId }).catch((cause) => showError(cause));
}

async function runWithConcurrency<T>(items: T[], limit: number, worker: (item: T) => Promise<void>) {
  const queue = [...items];
  await Promise.all(Array.from({ length: Math.min(limit, queue.length) }, async () => {
    while (queue.length) {
      const item = queue.shift();
      if (item !== undefined) await worker(item);
    }
  }));
}

function onUploadInput(event: Event) {
  const input = event.target as HTMLInputElement;
  const files = Array.from(input.files || []);
  input.value = "";
  if (files.length) void uploadLocalFiles(files).catch(showError);
}

function onDrop(event: DragEvent) {
  dragActive.value = false;
  if (!canWrite.value) return;
  const files = Array.from(event.dataTransfer?.files || []);
  if (files.length) void uploadLocalFiles(files).catch(showError);
}

async function copyTerminalSelection() {
  const text = terminal?.getSelection() || "";
  if (!text) return;
  try {
    await window.dbxPlugin.clipboard?.writeText(text);
    showNotice(t("terminalCopied"));
  } catch (cause) {
    showError(new Error(t("terminalCopyFailed", { error: cause instanceof Error ? cause.message : String(cause) })), "terminal");
  }
  terminalMenu.value = undefined;
  terminal?.focus();
}

async function pasteTerminal() {
  terminalMenu.value = undefined;
  try {
    const text = await window.dbxPlugin.clipboard?.readText();
    await sendConfirmedPaste(text || "");
  } catch (cause) {
    showError(new Error(t("terminalPasteFailed", { error: cause instanceof Error ? cause.message : String(cause) })), "terminal");
    terminal?.focus();
  }
}

async function pasteFromClipboardToTerminal() {
  try {
    const text = await window.dbxPlugin.clipboard?.readText();
    await sendConfirmedPaste(text || "");
  } catch (cause) {
    showError(new Error(t("terminalPasteFailed", { error: cause instanceof Error ? cause.message : String(cause) })), "terminal");
  }
}

function interceptTerminalPaste(event: ClipboardEvent) {
  event.preventDefault();
  event.stopPropagation();
  const text = event.clipboardData?.getData("text/plain") || "";
  if (!text) return;
  void sendConfirmedPaste(text);
}

async function sendConfirmedPaste(text: string) {
  if (!text) return;
  const accepted = await confirmRiskyPaste(text);
  if (!accepted) {
    terminal?.focus();
    return;
  }
  if (!session.value || zmodemBusy.value) return;
  trackPendingInput(text);
  sendTerminalBytes(new TextEncoder().encode(text));
  terminal?.focus();
}

function confirmRiskyPaste(text: string): Promise<boolean> {
  const confirmation = buildPasteConfirmation(text);
  if (!confirmation.required) return Promise.resolve(true);
  return new Promise((resolve) => {
    pasteConfirmResolver = resolve;
    pasteConfirm.value = confirmation;
  });
}

function resolvePasteConfirm(accepted: boolean) {
  pasteConfirm.value = undefined;
  const resolve = pasteConfirmResolver;
  pasteConfirmResolver = undefined;
  resolve?.(accepted);
}

function selectAllTerminal() {
  terminal?.selectAll();
  terminalMenu.value = undefined;
  terminal?.focus();
}

function clearTerminal() {
  terminal?.clear();
  terminalMenu.value = undefined;
  terminal?.focus();
}

function chooseZmodem() {
  terminalMenu.value = undefined;
  zmodemInput.value?.click();
}

function openCommandDialog() {
  commandOpen.value = true;
  commandError.value = "";
  commandHistoryIndex.value = -1;
  commandHistoryBackup.value = "";
}

function loadCommandHistory(): string[] {
  try {
    return sanitizeCommandHistory(JSON.parse(window.localStorage.getItem(COMMAND_HISTORY_KEY) || "null"));
  } catch {
    return [];
  }
}

function persistCommandHistory() {
  try {
    // 疑似内嵌凭据 / 超长 / 多行的命令只留在内存，不写 localStorage。
    window.localStorage.setItem(COMMAND_HISTORY_KEY, JSON.stringify(commandHistory.value.filter(isPersistableCommand)));
  } catch {
    // localStorage 不可用时命令历史仅保留在内存中。
  }
}

// ↑↓ 在命令输入框中浏览历史；进入浏览态前备份当前草稿，回到最新一条之下时恢复。
function browseCommandHistoryUp() {
  commandHistoryBackup.value = commandHistoryIndex.value === -1 ? commandDraft.value : commandHistoryBackup.value;
  const step = browseCommandHistory(commandHistory.value, commandHistoryIndex.value, "up", commandHistoryBackup.value);
  commandHistoryIndex.value = step.index;
  commandDraft.value = step.draft;
}

function browseCommandHistoryDown() {
  const step = browseCommandHistory(commandHistory.value, commandHistoryIndex.value, "down", commandHistoryBackup.value);
  commandHistoryIndex.value = step.index;
  commandDraft.value = step.draft;
}

// 一键重发：把历史条目回填输入框并立即执行。
function rerunHistoryCommand(command: string) {
  if (commandRunning.value) return;
  commandDraft.value = command;
  commandHistoryIndex.value = -1;
  void runCommand();
}

function clearCommandHistory() {
  commandHistory.value = [];
  commandHistoryIndex.value = -1;
  persistCommandHistory();
}

async function runCommand() {
  const sessionId = session.value?.sessionId;
  const command = commandDraft.value.trim();
  if (!sessionId || !command || commandRunning.value) return;
  commandRunning.value = true;
  commandError.value = "";
  commandResult.value = undefined;
  const execId = typeof crypto.randomUUID === "function" ? crypto.randomUUID() : `exec-${Date.now()}-${Math.random().toString(16).slice(2)}`;
  commandExecId.value = execId;
  try {
    commandResult.value = await window.dbxPlugin.invoke<ExecResult>("ssh/exec", {
      sessionId,
      execId,
      command,
      sudo: commandUseSudo.value,
    }, { timeoutMs: 120_000 });
    // 执行成功提交即入历史（不论退出码），与输入框 ↑↓、一键重发共用同一份。
    commandHistory.value = pushCommandHistory(commandHistory.value, command);
    persistCommandHistory();
    commandHistoryIndex.value = -1;
    commandHistoryBackup.value = "";
  } catch (cause) {
    commandError.value = cause instanceof Error ? cause.message : String(cause);
  } finally {
    commandRunning.value = false;
    commandExecId.value = "";
  }
}

async function cancelCommand() {
  const execId = commandExecId.value;
  if (!execId || !commandRunning.value) return;
  await window.dbxPlugin.invoke("ssh/exec/cancel", { execId }).catch((cause) => showError(cause));
}

// ---------------------------------------------------------------------------
// 快速命令栏：localStorage CRUD + PTY 一键发送
// ---------------------------------------------------------------------------

function loadQuickCommands(): QuickCommand[] {
  try {
    return normalizeQuickCommands(JSON.parse(window.localStorage.getItem(QUICK_COMMANDS_KEY) || "null"));
  } catch {
    return [];
  }
}

function persistQuickCommands() {
  try {
    window.localStorage.setItem(QUICK_COMMANDS_KEY, JSON.stringify(quickCommands.value));
  } catch {
    // localStorage 不可用时快速命令仅保留在内存中。
  }
}

function addQuickCommand() {
  const command = quickDraft.command.trim();
  if (!command) return;
  if (!quickDraft.id && quickCommands.value.length >= 20) return;
  const id = quickDraft.id || (typeof crypto.randomUUID === "function" ? crypto.randomUUID() : `qc-${Date.now()}-${Math.random().toString(16).slice(2)}`);
  quickCommands.value = upsertQuickCommand(quickCommands.value, { id, name: quickDraft.name, command });
  persistQuickCommands();
  quickDraft.id = undefined;
  quickDraft.name = "";
  quickDraft.command = "";
}

// 点击条目的编辑按钮：载入编辑器（携带 id 即更新语义），再次添加即保存。
function editQuickCommand(item: QuickCommand) {
  quickDraft.id = item.id;
  quickDraft.name = item.name;
  quickDraft.command = item.command;
}

function deleteQuickCommand(id: string) {
  quickCommands.value = removeQuickCommand(quickCommands.value, id);
  persistQuickCommands();
}

// 发送语义：快速命令是"在当前交互 shell 中执行"的片段（对齐 tiny-rdm），
// 必须走 PTY 写入——输出直接回显在终端里、cd/env 等状态留在当前 shell；
// ssh/exec 是独立非交互通道，不回显也不共享 shell 状态，不符合语义。
// 命令原文按键盘输入写入（用户可见可中断），不经过任何 shell 拼接转义。
function sendQuickCommand(item: QuickCommand) {
  if (!session.value || zmodemBusy.value || commandRunning.value) return;
  quickMenuOpen.value = false;
  const text = item.command.replace(/\r?\n/g, " ").trim();
  if (!text) return;
  trackPendingInput(`${text}\r`);
  sendTerminalBytes(new TextEncoder().encode(`${text}\r`));
  terminal?.focus();
}

// ---------------------------------------------------------------------------
// 连接信息面板（只读）
// ---------------------------------------------------------------------------

function toggleQuickMenu() {
  const next = !quickMenuOpen.value;
  fileMenu.value = undefined;
  terminalMenu.value = undefined;
  transferPanelOpen.value = false;
  columnsOpen.value = false;
  pathHistoryOpen.value = false;
  connectionInfoOpen.value = false;
  quickMenuOpen.value = next;
}

function toggleConnectionInfo() {
  const next = !connectionInfoOpen.value;
  fileMenu.value = undefined;
  terminalMenu.value = undefined;
  transferPanelOpen.value = false;
  columnsOpen.value = false;
  pathHistoryOpen.value = false;
  quickMenuOpen.value = false;
  connectionInfoOpen.value = next;
  if (next) {
    void measureLatency();
    void refreshConnectionAuthMethod();
  }
}

// 认证方式：读取 ssh/sessions/list 当前会话行的 authMethod（只读方法名，
// 不含任何凭据材料）。失败时面板显示占位符，不影响其他信息。
async function refreshConnectionAuthMethod() {
  const sessionId = session.value?.sessionId;
  if (!sessionId) return;
  try {
    const result = await window.dbxPlugin.invoke<{ sessions: Array<{ sessionId?: string; authMethod?: string; readOnly?: boolean }> }>(
      "ssh/sessions/list",
      {},
      { timeoutMs: 15_000 },
    );
    const mine = result.sessions?.find((row) => row.sessionId === sessionId);
    connectionAuthMethod.value = typeof mine?.authMethod === "string" && mine.authMethod ? mine.authMethod : "";
    connectionReadOnly.value = mine?.readOnly === true;
  } catch {
    connectionAuthMethod.value = "";
  }
}

// 延迟测量：复用既有 ssh/exec 跑一条 echo 只读命令，计时整个 RPC 往返
// （含通道建立），无需新增后端方法。测量值仅用于展示，不参与任何逻辑。
async function measureLatency() {
  const sessionId = session.value?.sessionId;
  if (!sessionId || connectionLatencyBusy.value) return;
  connectionLatencyBusy.value = true;
  connectionLatencyFailed.value = false;
  const startedAt = performance.now();
  try {
    const result = await window.dbxPlugin.invoke<ExecResult>("ssh/exec", {
      sessionId,
      command: "echo dbx-rtt-probe",
      timeoutSecs: 8,
    }, { timeoutMs: 15_000 });
    if (!result.output.includes("dbx-rtt-probe")) throw new Error("unexpected probe output");
    connectionLatency.value = performance.now() - startedAt;
  } catch {
    connectionLatency.value = null;
    connectionLatencyFailed.value = true;
  } finally {
    connectionLatencyBusy.value = false;
  }
}

let metricsTimer = 0;

async function refreshMetrics() {
  if (!session.value) return;
  metricsLoading.value = true;
  try {
    metrics.value = await window.dbxPlugin.invoke<ServerMetrics>("ssh/metrics", { sessionId: session.value.sessionId }, { timeoutMs: 30_000 });
    metricsError.value = "";
  } catch (cause) {
    metricsError.value = cause instanceof Error ? cause.message : String(cause);
  } finally {
    metricsLoading.value = false;
  }
}

function openMetrics() {
  metricsOpen.value = true;
  void refreshMetrics();
  window.clearInterval(metricsTimer);
  metricsTimer = window.setInterval(() => {
    if (metricsOpen.value && !metricsLoading.value) void refreshMetrics();
  }, 5000);
}

function closeMetrics() {
  metricsOpen.value = false;
  window.clearInterval(metricsTimer);
}

// Peak rate across every interface normalizes the per-interface bars; the
// network/process sections only render when the sidecar reports the fields,
// so older backends simply hide them.
const metricsRatePeak = computed(() => {
  let peak = 0;
  for (const net of metrics.value?.network ?? []) peak = Math.max(peak, net.rxRate, net.txRate);
  return peak > 0 ? peak : 1;
});

function networkRateShare(net: { rxRate: number; txRate: number }) {
  return Math.min(100, Math.round((Math.max(net.rxRate, net.txRate) / metricsRatePeak.value) * 100));
}

const metricsProcGridStyle = { gridTemplateColumns: "52px 76px 52px 56px minmax(0, 1fr)" };

async function refreshDiskUsage() {
  if (!session.value) return;
  diskUsage.value = await window.dbxPlugin
    .invoke<DiskUsage>("sftp/diskUsage", { sessionId: session.value.sessionId, path: currentPath.value }, { timeoutMs: 30_000 })
    .catch(() => undefined);
}

function beginChmod(entry: SftpEntry) {
  if (!canWrite.value) return;
  chmodTarget.value = entry;
  chmodDraft.value = entry.permissions || "";
  fileMenu.value = undefined;
}

async function openSettings() {
  settingsOpen.value = true;
  settingsLoading.value = true;
  void loadKnownHosts();
  void loadLocalKeys();
  void loadMcpSettings();
  try {
    const meta = await window.dbxPlugin.invoke<SshSettings>("ssh/settings/get", { sessionId: session.value?.sessionId });
    settingsMeta.value = meta;
    settingsDraft.quickSudo = meta.quickSudo;
    settingsDraft.sudoUsePty = meta.sudoUsePty;
    settingsDraft.authFlowMode = meta.authFlowMode || "password_then_otp";
    settingsDraft.passwordPromptHint = meta.passwordPromptHint || "";
    settingsDraft.totpPromptHint = meta.totpPromptHint || "";
    settingsDraft.sudoPassword = "";
    settingsDraft.totpSecret = "";
  } catch (cause) {
    showError(cause);
  } finally {
    settingsLoading.value = false;
  }
}

function settingsErrorOf(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}

async function loadKnownHosts() {
  knownHostsLoading.value = true;
  knownHostsError.value = "";
  try {
    const result = await window.dbxPlugin.invoke<{ entries: KnownHostEntry[] }>("ssh/knownHosts/list", {});
    knownHosts.value = result.entries;
  } catch (cause) {
    knownHosts.value = [];
    knownHostsError.value = settingsErrorOf(cause);
  } finally {
    knownHostsLoading.value = false;
  }
}

async function removeKnownHost(entry: KnownHostEntry) {
  if (!window.confirm(t("knownHosts.removeConfirm", { host: `${entry.host}:${entry.port}` }))) return;
  try {
    await window.dbxPlugin.invoke("ssh/knownHosts/remove", { host: entry.host, port: entry.port });
    showNotice(t("knownHosts.removed", { host: `${entry.host}:${entry.port}` }));
  } catch (cause) {
    knownHostsError.value = settingsErrorOf(cause);
  } finally {
    await loadKnownHosts();
  }
}

async function loadLocalKeys() {
  localKeysLoading.value = true;
  localKeysError.value = "";
  try {
    const result = await window.dbxPlugin.invoke<{ keys: DiscoveredKey[] }>("keys/discover", {});
    localKeys.value = result.keys.map((key) => ({ ...key, hasPassphrase: key.hasPassphrase ?? key.has_passphrase === true }));
  } catch (cause) {
    localKeys.value = [];
    localKeysError.value = settingsErrorOf(cause);
  } finally {
    localKeysLoading.value = false;
  }
}

async function loadMcpSettings() {
  mcpLoading.value = true;
  mcpError.value = "";
  try {
    const result = await window.dbxPlugin.invoke<McpSizeSettings>("mcp/settings/get", {});
    mcpDraft.readMiB = mibField(result.maxReadBytes);
    mcpDraft.uploadMiB = mibField(result.maxUploadBytes);
    mcpDraft.downloadMiB = mibField(result.maxDownloadBytes);
  } catch (cause) {
    mcpError.value = settingsErrorOf(cause);
  } finally {
    mcpLoading.value = false;
  }
}

function mibField(bytes?: number) {
  return typeof bytes === "number" && bytes > 0 ? String(Math.round(bytes / MIB)) : "";
}

async function saveMcpSettings() {
  if (!mcpInputsValid.value || mcpSaving.value) return;
  mcpSaving.value = true;
  mcpError.value = "";
  try {
    await window.dbxPlugin.invoke("mcp/settings/set", {
      maxReadBytes: Number.parseInt(mcpDraft.readMiB.trim(), 10) * MIB,
      maxUploadBytes: Number.parseInt(mcpDraft.uploadMiB.trim(), 10) * MIB,
      maxDownloadBytes: Number.parseInt(mcpDraft.downloadMiB.trim(), 10) * MIB,
    });
    showNotice(t("mcpLimits.saved"));
  } catch (cause) {
    mcpError.value = settingsErrorOf(cause);
  } finally {
    mcpSaving.value = false;
  }
}

async function saveSettings() {
  if (!session.value || settingsSaving.value) return;
  settingsSaving.value = true;
  try {
    const updates: Record<string, unknown> = {
      quickSudo: settingsDraft.quickSudo,
      sudoUsePty: settingsDraft.sudoUsePty,
      authFlowMode: settingsDraft.authFlowMode,
      passwordPromptHint: settingsDraft.passwordPromptHint,
      totpPromptHint: settingsDraft.totpPromptHint,
    };
    if (settingsDraft.sudoPassword) updates.sudoPassword = settingsDraft.sudoPassword;
    if (settingsDraft.totpSecret.trim()) updates.totpSecret = settingsDraft.totpSecret;
    const meta = await window.dbxPlugin.invoke<SshSettings>("ssh/settings/set", { sessionId: session.value.sessionId, ...updates });
    settingsMeta.value = meta;
    settingsDraft.sudoPassword = "";
    settingsDraft.totpSecret = "";
    showNotice(t("settingsSaved"));
  } catch (cause) {
    showError(cause);
  } finally {
    settingsSaving.value = false;
  }
}

async function clearStoredSecrets() {
  if (!session.value) return;
  try {
    const meta = await window.dbxPlugin.invoke<SshSettings>("ssh/settings/set", {
      sessionId: session.value.sessionId,
      sudoPassword: "",
      totpSecret: "",
    });
    settingsMeta.value = meta;
    showNotice(t("settingsSecretsCleared"));
  } catch (cause) {
    showError(cause);
  }
}

async function confirmChmod() {
  const entry = chmodTarget.value;
  const mode = chmodDraft.value.trim();
  if (!session.value || !entry || !mode) return;
  chmodSubmitting.value = true;
  try {
    if (sudoMode.value) {
      await window.dbxPlugin.invoke("sudo/chmod", {
        sessionId: session.value.sessionId,
        path: pathFromUri(entry.uri),
        mode,
      });
    } else {
      await window.dbxPlugin.invoke("sftp/chmod", {
        sessionId: session.value.sessionId,
        path: pathFromUri(entry.uri),
        mode,
      });
    }
    chmodTarget.value = undefined;
    await loadDirectory();
    showNotice(t("permissionsUpdated"));
  } catch (cause) {
    showError(cause);
  } finally {
    chmodSubmitting.value = false;
  }
}

function onZmodemInput(event: Event) {
  const input = event.target as HTMLInputElement;
  const files = Array.from(input.files || []);
  input.value = "";
  if (!files.length || !connected.value) return;
  pendingZmodemFiles = files;
  zmodemState.value = "waiting";
  zmodemFileName.value = files[0]?.name || "";
  zmodemTransferred.value = 0;
  zmodemTotalSize.value = files.reduce((sum, file) => sum + file.size, 0);
  resetZmodemSentry();
  sendTerminalBytes(new TextEncoder().encode("rz\r"));
  zmodemDetectionTimer = window.setTimeout(() => finishZmodemUpload(new Error(t("zmodemNotAvailable"))), ZMODEM_DETECTION_TIMEOUT_MS);
}

function showTerminalMenu(event: MouseEvent) {
  event.preventDefault();
  terminalMenu.value = { x: Math.min(event.clientX, window.innerWidth - 190), y: Math.min(event.clientY, window.innerHeight - 250) };
  fileMenu.value = undefined;
}

function showFileMenu(event: MouseEvent, entry: SftpEntry) {
  event.preventDefault();
  selectedPath.value = entry.uri;
  fileMenu.value = { x: Math.min(event.clientX, window.innerWidth - 190), y: Math.min(event.clientY, window.innerHeight - 290), entry };
  terminalMenu.value = undefined;
}

function closeMenus() {
  terminalMenu.value = undefined;
  fileMenu.value = undefined;
  transferPanelOpen.value = false;
  columnsOpen.value = false;
  pathHistoryOpen.value = false;
  quickMenuOpen.value = false;
  connectionInfoOpen.value = false;
}

function openTransferPanel() {
  terminalMenu.value = undefined;
  fileMenu.value = undefined;
  columnsOpen.value = false;
  transferPanelOpen.value = true;
}

function pathFromUri(uri: string) {
  return uri.replace(/^sftp:/, "") || "/";
}

function normalizeRemotePath(path: string) {
  let value = path.trim() || "/";
  try { value = decodeURIComponent(value); } catch {}
  if (!value.startsWith("/")) value = `/${value}`;
  value = value.replace(/\/{2,}/g, "/");
  return value === "/" ? value : value.replace(/\/+$/, "");
}

function joinRemote(parent: string, name: string) {
  return `${parent === "/" ? "" : parent.replace(/\/+$/, "")}/${name.replace(/^\/+/, "")}`;
}

function parentPath(path: string) {
  const normalized = path.replace(/\/+$/, "");
  const index = normalized.lastIndexOf("/");
  return index <= 0 ? "/" : normalized.slice(0, index);
}

function shortFingerprint(fingerprint: string) {
  if (fingerprint.length <= 20) return fingerprint;
  return `${fingerprint.slice(0, 17)}…`;
}

function formatUptime(seconds: number) {
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (days >= 1) return t("uptimeDays", { count: days, hours });
  if (hours >= 1) return t("uptimeHours", { count: hours, minutes });
  return t("uptimeMinutes", { count: minutes });
}

function colorWithAlpha(color: string, alpha: number) {
  const match = color.trim().match(/^#([0-9a-f]{3}|[0-9a-f]{6})$/i);
  if (!match) return `color-mix(in srgb, ${color} ${Math.round(alpha * 100)}%, transparent)`;
  const hex = match[1].length === 3 ? [...match[1]].map((part) => `${part}${part}`).join("") : match[1];
  const red = Number.parseInt(hex.slice(0, 2), 16);
  const green = Number.parseInt(hex.slice(2, 4), 16);
  const blue = Number.parseInt(hex.slice(4, 6), 16);
  return `rgb(${red} ${green} ${blue} / ${alpha})`;
}

function formatModified(value?: number) {
  if (!value) return "";
  return new Intl.DateTimeFormat(locale.value, { dateStyle: "short", timeStyle: "short" }).format(new Date(value * 1000));
}

function transferPercent(task: TransferTask) {
  return task.size > 0 ? Math.min(100, Math.round((task.transferred / task.size) * 100)) : task.status === "completed" ? 100 : 0;
}

function readU64(bytes: Uint8Array, offset: number) {
  return Number(new DataView(bytes.buffer, bytes.byteOffset + offset, 8).getBigUint64(0, false));
}

function writeU64(bytes: Uint8Array, offset: number, value: number) {
  new DataView(bytes.buffer, bytes.byteOffset + offset, 8).setBigUint64(0, BigInt(value), false);
}

async function waitForHostApi(timeoutMs = 8000) {
  const deadline = Date.now() + timeoutMs;
  while (!window.dbxPlugin && Date.now() < deadline) await new Promise((resolve) => setTimeout(resolve, 50));
  if (!window.dbxPlugin) throw new Error(t("hostApiUnavailable"));
  return window.dbxPlugin;
}

async function initialize() {
  const api = await waitForHostApi();
  hostContext.value = await Promise.any([
    api.ready,
    api.request<Record<string, unknown>>("host.getContext"),
  ]);
  locale.value = api.locale || "zh-CN";
  restoreUiState();
  if (api.appearance) applyAppearance(api.appearance);
  unsubscribeAppearance = api.onAppearanceChange?.(applyAppearance);
  unsubscribeLocale = api.onLocaleChange?.((nextLocale) => (locale.value = nextLocale || "zh-CN"));
  unsubscribeContext = api.onContextChange?.((context) => {
    hostContext.value = context;
  });
  unsubscribeEvent = api.onEvent(handleEvent);
  unsubscribeBinary = api.onBinary(handleBinary);
  unsubscribeFileDrag = api.fileTransfer?.onDragState((active) => (dragActive.value = active));
  unsubscribeFileDrop = api.fileTransfer?.onDrop((files) => {
    dragActive.value = false;
    void uploadHandleFiles(files).then(() => loadDirectory()).catch(showError);
  });
  await nextTick();
  createTerminal();
  if (!connectionId.value || !workbenchId.value) throw new Error("DBX did not provide connectionId/workbenchId");
  const state = initialState();
  if (restored.value) {
    terminalState.value = "disconnected";
    terminalError.value = t("restartDisconnected");
    return;
  }
  if (typeof state.sessionId === "string" && state.sessionId) await attachSession(state.sessionId);
  else await openSession();
}

watch([splitRatio, paneOrder, followDirectory, sudoMode, visibleColumns], persistState, { deep: true });

onMounted(() => {
  document.addEventListener("click", closeMenus);
  void initialize().catch((cause) => {
    terminalState.value = "error";
    showError(cause, "terminal");
  });
});

onBeforeUnmount(() => {
  disposed = true;
  window.clearTimeout(persistTimer);
  void writeWorkbenchState();
  window.clearTimeout(resizeTimer);
  window.clearTimeout(reconnectTimer);
  window.clearInterval(reconnectCountdownTimer);
  window.clearTimeout(noticeTimer);
  window.clearTimeout(zmodemDetectionTimer);
  window.clearTimeout(zoomNoticeTimer);
  window.clearInterval(metricsTimer);
  stopCommandMarkerTick();
  resolvePasteConfirm(false);
  if (terminalHost.value) {
    if (terminalPasteHandler) terminalHost.value.removeEventListener("paste", terminalPasteHandler, true);
    if (terminalWheelHandler) terminalHost.value.removeEventListener("wheel", terminalWheelHandler, true);
  }
  document.removeEventListener("click", closeMenus);
  unsubscribeEvent?.();
  unsubscribeBinary?.();
  unsubscribeAppearance?.();
  unsubscribeLocale?.();
  unsubscribeContext?.();
  unsubscribeFileDrag?.();
  unsubscribeFileDrop?.();
  resizeObserver?.disconnect();
  disposeInput?.dispose();
  terminal?.dispose();
  for (const waiter of uploadAckWaiters.values()) {
    window.clearTimeout(waiter.timer);
    waiter.reject(new Error("Workbench detached"));
  }
  for (const waiter of terminalInputAckWaiters.values()) {
    window.clearTimeout(waiter.timer);
    waiter.reject(new Error("Workbench detached"));
  }
  for (const waiter of downloadChunkWaiters.values()) {
    window.clearTimeout(waiter.timer);
    waiter.reject(new Error("Workbench detached"));
  }
});
</script>

<template>
  <main class="workbench">
    <header class="toolbar" :style="toolbarStyle">
      <div class="identity">
        <span v-if="connection.color" class="connection-color" :style="{ backgroundColor: connection.color }" />
        <strong>{{ connectionIdentity }}</strong>
        <span v-if="connection.readOnly || connectionReadOnly" class="read-only-badge">{{ t("readOnly") }}</span>
        <span class="session-pill" :class="`session-${sessionStatus}`"><span class="session-dot" aria-hidden="true" />{{ t(`sessionStatus.${sessionStatus}`) }}<span v-if="sessionStatus === 'reconnecting' && reconnectCountdown" class="session-pill-countdown mono">{{ t("sessionStatus.reconnectCountdown", { seconds: reconnectCountdown.seconds, attempt: reconnectCountdown.attempt }) }}</span></span>
      </div>
      <div class="toolbar-actions">
        <button class="icon-button icon-neutral" :title="paneOrder === 'terminal-left' ? t('moveSftpLeft') : t('moveTerminalLeft')" @click="togglePaneOrder"><ArrowLeftRight /></button>
        <button class="icon-button" :title="t('terminalFontDecrease')" @click="adjustTerminalZoom(-1)"><span class="font-step-label" aria-hidden="true">A−</span></button>
        <button class="icon-button" :title="t('terminalFontIncrease')" @click="adjustTerminalZoom(1)"><span class="font-step-label" aria-hidden="true">A+</span></button>
        <button class="icon-button icon-emerald" :title="t('reconnect')" :disabled="terminalState === 'connecting'" @click="reconnect"><PlugZap /></button>
        <button class="icon-button icon-emerald" :class="{ 'is-active': quickSudo }" :title="quickSudoTitle" :aria-pressed="quickSudo" :disabled="!connected" @click="toggleQuickSudo"><ShieldCheck /></button>
        <label class="follow-directory-control" :title="t('followTerminal')">
          <button class="switch-control" type="button" role="switch" :aria-checked="followDirectory" :disabled="!connected" @click="setDirectoryTracking(!followDirectory)"><span /></button>
          <span>{{ t("followTerminal") }}</span>
        </label>
        <span class="toolbar-separator" aria-hidden="true" />
        <button class="icon-button icon-amber" :title="t('home')" :disabled="!connected" @click="loadHome"><Home /></button>
        <button class="icon-button icon-cyan" :title="t('refresh')" :disabled="!connected || loadingFiles" @click="loadDirectory()"><RefreshCw :class="{ spinning: loadingFiles }" /></button>
        <button class="icon-button icon-neutral" :title="t('commandTitle')" :disabled="!connected" @click="openCommandDialog"><SquareTerminal /></button>
        <div class="menu-anchor">
          <button class="icon-button icon-amber" :title="t('quickCommands')" :disabled="!connected" @click.stop="toggleQuickMenu"><Zap /></button>
          <section v-if="quickMenuOpen" class="popover quick-commands-popover" @click.stop>
            <h3>{{ t("quickCommands") }}</h3>
            <div v-if="!quickCommands.length" class="empty compact">{{ t("quickCommandsEmpty") }}</div>
            <div v-for="item in quickCommands" :key="item.id" class="quick-command-row">
              <button class="quick-command-send" :title="item.command" @click="sendQuickCommand(item)">
                <strong>{{ item.name }}</strong>
                <span class="mono">{{ item.command }}</span>
              </button>
              <button class="icon-button" :title="t('quickCommandsEdit')" @click="editQuickCommand(item)"><Pencil /></button>
              <button class="icon-button" :title="t('delete')" @click="deleteQuickCommand(item.id)"><Trash2 /></button>
            </div>
            <footer class="quick-command-editor">
              <input v-model="quickDraft.name" :placeholder="t('quickCommandsName')" :maxlength="60" />
              <input v-model="quickDraft.command" class="mono" :placeholder="t('quickCommandsCommand')" :maxlength="500" @keydown.enter="addQuickCommand" />
              <div class="quick-command-editor-actions">
                <button class="primary-button" :disabled="!quickDraft.command.trim() || (!quickDraft.id && quickCommands.length >= 20)" @click="addQuickCommand">{{ quickDraft.id ? t("save") : t("quickCommandsAdd") }}</button>
                <button v-if="quickDraft.id" @click="quickDraft.id = undefined; quickDraft.name = ''; quickDraft.command = ''">{{ t("cancel") }}</button>
                <span class="quick-command-limit">{{ t("quickCommandsLimit", { count: quickCommands.length, limit: 20 }) }}</span>
              </div>
            </footer>
          </section>
        </div>
        <button class="icon-button icon-emerald" :title="t('metrics')" :disabled="!connected" @click="openMetrics"><Gauge /></button>
        <div class="menu-anchor">
          <button class="icon-button icon-neutral" :title="t('connectionInfo')" @click.stop="toggleConnectionInfo"><Info /></button>
          <section v-if="connectionInfoOpen" class="popover connection-info-popover" @click.stop>
            <h3>{{ t("connectionInfo") }}</h3>
            <dl class="connection-info-grid">
              <dt>{{ t("connectionInfoHost") }}</dt><dd class="mono">{{ connection.host || connection.name || "–" }}</dd>
              <dt>{{ t("connectionInfoPort") }}</dt><dd class="mono">{{ connection.port || 22 }}</dd>
              <dt>{{ t("connectionInfoUser") }}</dt><dd class="mono">{{ connection.username || "–" }}</dd>
              <dt>{{ t("connectionInfoAuth") }}</dt><dd>{{ connectionAuthMethodLabel }}</dd>
              <template v-if="connection.readOnly || connectionReadOnly"><dt>{{ t("readOnly") }}</dt><dd>{{ t("yes") }}</dd></template>
              <dt>{{ t("connectionInfoLatency") }}</dt>
              <dd>
                <span class="mono">{{ connectionLatencyBusy ? t("connectionInfoMeasuring") : formatLatency(connectionLatency) }}</span>
                <span v-if="connectionLatencyFailed && !connectionLatencyBusy" class="task-error">{{ t("connectionInfoFailed") }}</span>
                <button class="link-button" :disabled="connectionLatencyBusy || !connected" @click="measureLatency">{{ t("connectionInfoMeasure") }}</button>
              </dd>
            </dl>
          </section>
        </div>
        <button class="icon-button icon-violet" :title="t('settings')" :disabled="!connected" @click="openSettings"><Settings /></button>
        <div class="menu-anchor">
          <button class="icon-button icon-violet" :title="t('customizeColumns')" @click.stop="fileMenu = undefined; terminalMenu = undefined; transferPanelOpen = false; columnsOpen = !columnsOpen"><Columns3 /></button>
          <div v-if="columnsOpen" class="popover columns-popover" @click.stop>
            <label v-for="column in (['size', 'modified', 'permissions'] as SftpColumn[])" :key="column"><input type="checkbox" :checked="visibleColumns.includes(column)" @change="toggleColumn(column)" />{{ t(column) }}</label>
          </div>
        </div>
        <div class="menu-anchor">
          <button class="icon-button icon-blue" :title="t('transfers')" @click.stop="transferPanelOpen = !transferPanelOpen"><ListChecks /><span v-if="activeTransfers" class="activity-dot" /></button>
          <section v-if="transferPanelOpen" class="popover transfer-popover" @click.stop>
            <h3>{{ t("transfers") }}</h3>
            <div v-if="!transferList.length" class="empty compact">{{ t("noTransfers") }}</div>
            <article v-for="task in transferList" :key="task.taskId" class="transfer-card">
              <div class="transfer-title"><FileUp v-if="task.direction === 'upload'" /><Download v-else /><span>{{ task.fileName || task.taskId }}</span><strong>{{ transferPercent(task) }}%</strong></div>
              <progress :value="transferPercent(task)" max="100" />
              <div class="transfer-meta"><span>{{ t(`transferStatus.${task.status}`) }}</span><span>{{ formatBytes(task.transferred) }} / {{ formatBytes(task.size) }}</span><span v-if="transferSpeeds[task.taskId]">{{ formatBytes(transferSpeeds[task.taskId]) }}/s</span></div>
              <button v-if="task.status === 'queued' || task.status === 'running'" class="link-button" @click="cancelTransfer(task)">{{ t("cancel") }}</button>
              <p v-if="task.error" class="task-error">{{ task.error }}</p>
            </article>
          </section>
        </div>
        <button class="icon-button icon-teal" :title="t('upload')" :disabled="!connected || !canWrite" @click.stop="chooseUpload"><FileUp /></button>
        <button class="icon-button icon-amber" :title="t('newFolder')" :disabled="!connected || !canWrite" @click="operationDraft = ''; operationDialog = 'mkdir'"><FolderPlus /></button>
        <button class="icon-button icon-amber" :title="t('sftpNewFile.action')" :disabled="!connected || !canWrite" @click="openNewFileDialog"><FilePlus /></button>
      </div>
    </header>

    <div v-if="notice" class="notice">{{ notice }}</div>
    <div v-if="sftpError" class="error-banner"><span>{{ sftpError }}</span><button @click="sftpError = ''"><X /></button></div>

    <section ref="paneContainer" :class="orderedPaneClass">
      <section class="terminal-pane" :style="terminalBasis" @contextmenu="showTerminalMenu">
        <div ref="terminalHost" class="terminal-host" />
        <TerminalSearchPanel
          v-if="searchOpen"
          :locale="locale"
          :match-state="searchMatchState"
          :result-index="searchResultIndex"
          :result-count="searchResultCount"
          @find-next="(query, options) => runTerminalSearch(query, options, 'next')"
          @find-previous="(query, options) => runTerminalSearch(query, options, 'prev')"
          @clear="clearTerminalSearch"
          @close="closeTerminalSearch"
        />
        <div v-if="terminalState !== 'connected'" class="terminal-overlay">
          <Loader2 v-if="terminalState === 'connecting'" class="spinning large-icon" />
          <svg v-else class="terminal-state-icon" viewBox="0 0 64 64" role="img" aria-label="SSH">
            <rect x="5" y="8" width="54" height="48" rx="9" fill="#111827" />
            <rect x="8" y="11" width="48" height="42" rx="6" fill="#1f2937" stroke="#60a5fa" stroke-width="2" />
            <path d="m17 23 9 9-9 9" fill="none" stroke="#86efac" stroke-linecap="round" stroke-linejoin="round" stroke-width="4" />
            <path d="M31 41h15" fill="none" stroke="#e5e7eb" stroke-linecap="round" stroke-width="4" />
          </svg>
          <p>{{ sessionStatus === "connecting" ? t("connecting") : sessionStatus === "reconnecting" ? terminalError || t("sessionStatus.reconnecting") : terminalError || t("disconnected") }}</p>
          <button v-if="terminalState !== 'connecting'" class="primary-button" @click="reconnect">{{ t("reconnect") }}</button>
        </div>
        <div v-if="commandMarker.installed" class="terminal-command-marker" :class="{ active: commandMarker.active, failed: !commandMarker.active && commandMarker.exitCode !== null && commandMarker.exitCode !== 0 }" :title="commandMarkerDetails" @click="terminal?.focus()">
          <Loader2 v-if="commandMarker.active" class="spinning" />
          <TriangleAlert v-else-if="commandMarker.exitCode" />
          <Info v-else />
          <span v-if="commandMarker.active" class="marker-text">{{ t("terminalCommand.running", { command: commandMarker.command || "…" }) }}</span>
          <span v-if="commandMarker.active && commandMarkerElapsed !== null" class="marker-elapsed mono">{{ formatCommandDuration(commandMarkerElapsed) }}</span>
          <span v-else-if="commandMarker.exitCode !== null" class="marker-text">{{ t("terminalCommand.finished", { code: commandMarker.exitCode, duration: formatCommandDuration(commandMarker.durationMs) }) }}</span>
          <span v-else class="marker-text">{{ t("terminalCommand.hint") }}</span>
          <span v-if="commandMarker.cwd" class="marker-cwd mono">{{ commandMarker.cwd }}</span>
        </div>
        <div v-if="zmodemBusy" class="zmodem-status">
          <Loader2 class="spinning" />
          <span>{{ zmodemState === "waiting" ? t("zmodemWaiting") : t("zmodemUploading", { name: zmodemFileName, percent: zmodemPercent }) }}</span>
          <span v-if="zmodemSpeed">{{ formatBytes(zmodemSpeed) }}/s</span>
        </div>
      </section>

      <div class="divider" @pointerdown="startDividerDrag" />

      <section class="sftp-pane" :class="{ 'drag-active': dragActive }" @dragenter.prevent="dragActive = true" @dragover.prevent @dragleave.self="dragActive = false" @drop.prevent="onDrop">
        <div class="path-toolbar">
          <button class="icon-button" :title="t('parentFolder')" :disabled="currentPath === '/'" @click="goParent"><ArrowUp /></button>
          <input v-model="currentPath" spellcheck="false" @keydown.enter="loadDirectory()" />
          <div class="menu-anchor">
            <button class="icon-button" :title="t('sftpPathHistory.title')" :disabled="!connected" @click.stop="fileMenu = undefined; terminalMenu = undefined; transferPanelOpen = false; columnsOpen = false; pathHistoryOpen = !pathHistoryOpen"><History /></button>
            <div v-if="pathHistoryOpen" class="popover path-history-popover" @click.stop>
              <strong class="path-history-title">{{ t("sftpPathHistory.title") }}</strong>
              <button v-for="item in currentPathHistory" :key="item" class="path-item mono" :title="item" @click="goToPath(item)">{{ item }}</button>
              <div v-if="!currentPathHistory.length" class="empty compact">{{ t("sftpPathHistory.empty") }}</div>
              <strong class="path-history-title">{{ t("sftpQuickPath.title") }}</strong>
              <button v-for="item in SFTP_QUICK_PATHS" :key="item" class="path-item mono" :title="item" @click="goToPath(item)">{{ item }}</button>
            </div>
          </div>
          <button class="icon-button" :title="t('sftpPaste.action')" :disabled="!connected || !canWrite || !sftpClipboard || pasteBusy" @click="pasteClipboard"><ClipboardPaste /></button>
          <label class="follow-directory-control" :title="!canWrite ? t('readOnly') : t('sudo.modeHint')">
            <button class="switch-control" type="button" role="switch" :aria-checked="sudoMode" :disabled="!connected || !canWrite" @click="toggleSudoMode"><span /></button>
            <span>{{ t("sudo.mode") }}</span>
          </label>
        </div>
        <div class="sftp-filter-bar">
          <label class="sftp-search-input">
            <Search />
            <input v-model="sftpSearch" type="search" :placeholder="t('sftpSearch.placeholder')" spellcheck="false" />
            <button v-if="sftpSearch" class="sftp-search-clear" :title="t('cancel')" @click.prevent="sftpSearch = ''"><X /></button>
          </label>
          <select v-model="sftpTypeFilter" class="sftp-type-filter" :title="t('sftpFilter.all')">
            <option value="all">{{ t("sftpFilter.all") }}</option>
            <option value="directory">{{ t("sftpFilter.folders") }}</option>
            <option value="file">{{ t("sftpFilter.files") }}</option>
          </select>
        </div>
        <div v-if="selectedUris.length > 1" class="sftp-batch-bar">
          <span>{{ t("sftpBatch.selected", { count: selectedUris.length }) }}</span>
          <template v-if="batchProgress">
            <progress class="batch-progress-bar" :value="batchProgressPercent(batchProgress)" max="100" />
            <span class="batch-progress mono">{{ t("sftpBatch.progress", { done: batchProgress.done, total: batchProgress.total }) }}</span>
          </template>
          <button :disabled="!canWrite || archiveBusy || batchDeleteSubmitting" @click="batchArchive"><Archive />{{ t("sftpBatch.archive") }}</button>
          <button class="danger" :disabled="!canWrite || archiveBusy || batchDeleteSubmitting" @click="batchDeleteOpen = true"><Trash2 />{{ t("sftpBatch.delete") }}</button>
          <button @click="clearRowSelection"><X />{{ t("sftpBatch.clear") }}</button>
        </div>
        <div class="file-table">
          <div class="file-rows">
            <div class="file-header" :style="sftpGridStyle">
              <button @click="toggleSort('name')">{{ t("name") }}<component :is="sortIcon('name')" /></button>
              <button v-if="visibleColumns.includes('size')" @click="toggleSort('size')">{{ t("size") }}<component :is="sortIcon('size')" /></button>
              <button v-if="visibleColumns.includes('modified')" @click="toggleSort('modified')">{{ t("modified") }}<component :is="sortIcon('modified')" /></button>
              <span v-if="visibleColumns.includes('permissions')">{{ t("permissions") }}</span>
            </div>
            <div v-if="loadingFiles" class="empty"><Loader2 class="spinning" />{{ t("loading") }}</div>
            <button
              v-for="entry in visibleEntries"
              v-else
              :key="entry.uri"
              class="file-row"
              :class="{ selected: selectedPath === entry.uri || selectedUris.includes(entry.uri) }"
              :style="sftpGridStyle"
              @click="selectFile(entry, $event)"
              @dblclick="openEntry(entry)"
              @contextmenu="showFileMenu($event, entry)"
            >
              <span class="file-name">
                <Folder v-if="entry.kind === 'directory'" class="folder-icon" />
                <FileIcon v-else-if="entry.kind === 'file'" />
                <FileText v-else />
                <input
                  v-if="renamingPath === entry.uri"
                  v-model="renameDraft"
                  class="rename-input"
                  :disabled="renameSubmitting"
                  @click.stop
                  @dblclick.stop
                  @keydown.enter.stop="commitRename(entry)"
                  @keydown.escape.stop="renamingPath = ''"
                  @blur="commitRename(entry)"
                />
                <span v-else>{{ entry.name }}</span>
              </span>
              <span v-if="visibleColumns.includes('size')" class="numeric">{{ entry.kind === "file" ? formatBytes(entry.size) : "" }}</span>
              <span v-if="visibleColumns.includes('modified')">{{ formatModified(entry.modifiedAt) }}</span>
              <span v-if="visibleColumns.includes('permissions')" class="mono">{{ entry.permissions }}</span>
            </button>
            <div v-if="!loadingFiles && !visibleEntries.length" class="empty">{{ entries.length ? t("sftpSearch.noMatch") : t("emptyFolder") }}</div>
          </div>
          <footer class="file-footer"><span>{{ sftpFiltersActive ? t("sftpSearch.footerMatch", { matched: visibleEntries.length, total: entries.length }) : t("items", { count: entries.length }) }}</span><span v-if="diskUsage" :title="`${diskUsage.filesystem} → ${diskUsage.mount}`">{{ formatBytes(diskUsage.availableBytes) }} {{ t("diskFreeOf", { total: formatBytes(diskUsage.totalBytes) }) }}</span><span>{{ currentPath }}</span></footer>
        </div>
        <div v-if="dragActive" class="drop-overlay"><FileUp /><strong>{{ t("upload") }}</strong></div>
      </section>
    </section>

    <nav v-if="terminalMenu" class="context-menu" :style="{ left: terminalMenu.x + 'px', top: terminalMenu.y + 'px' }" @click.stop>
      <button :disabled="!terminal?.hasSelection()" @click="copyTerminalSelection"><Copy />{{ t("terminalCopy") }}</button>
      <button :disabled="!connected || zmodemBusy" @click="pasteTerminal"><ClipboardPaste />{{ t("terminalPaste") }}</button>
      <button @click="selectAllTerminal"><TextSelect />{{ t("terminalSelectAll") }}</button>
      <button @click="openTerminalSearch"><Search />{{ t("terminalSearch.open") }}</button>
      <button @click="clearTerminal"><Eraser />{{ t("terminalClear") }}</button>
      <button :disabled="!connected" @click="toggleQuickSudo()"><ShieldCheck />{{ t("quickSudo.label") }} · {{ quickSudo ? t("quickSudo.on") : t("quickSudo.off") }}</button>
      <hr />
      <button :disabled="!connected || zmodemBusy || !canWrite" @click="chooseZmodem"><FileUp />{{ t("zmodemUpload") }}</button>
    </nav>

    <nav v-if="fileMenu" class="context-menu" :style="{ left: fileMenu.x + 'px', top: fileMenu.y + 'px' }" @click.stop>
      <button v-if="fileMenu.entry.kind === 'directory' || isPreviewable(fileMenu.entry) || isImagePreviewable(fileMenu.entry)" @click="openEntry(fileMenu.entry)"><Folder v-if="fileMenu.entry.kind === 'directory'" /><FileText v-else />{{ fileMenu.entry.kind === "directory" ? t("openFolder") : t("preview") }}</button>
      <button v-if="fileMenu.entry.kind === 'file'" @click="downloadEntry(fileMenu.entry)"><Download />{{ t("download") }}</button>
      <button :disabled="!canWrite" @click="beginRename(fileMenu.entry); fileMenu = undefined"><Pencil />{{ t("rename") }}</button>
      <button @click="copySelectedEntries('copy')"><Copy />{{ t("sftpCopy.copy") }}</button>
      <button :disabled="!canWrite" @click="copySelectedEntries('cut')"><Scissors />{{ t("sftpCopy.cut") }}</button>
      <button :disabled="!canWrite" @click="beginChmod(fileMenu.entry)"><Lock />{{ t("permissionsEdit") }}</button>
      <button @click="openAttributes(fileMenu.entry)"><Info />{{ t("sftpAttrs.action") }}</button>
      <button v-if="fileMenu.entry.kind === 'directory'" :disabled="!canWrite || archiveBusy" @click="archiveEntry(fileMenu.entry)"><Archive />{{ t("archive.action") }}</button>
      <button v-if="fileMenu.entry.kind === 'file' && isArchiveName(fileMenu.entry.name)" :disabled="!canWrite || archiveBusy" @click="extractEntry(fileMenu.entry)"><PackageOpen />{{ t("extract.action") }}</button>
      <hr />
      <button class="danger" :disabled="!canWrite" @click="deleteTarget = fileMenu.entry; fileMenu = undefined"><Trash2 />{{ t("delete") }}</button>
    </nav>

    <section v-if="previewOpen" class="modal-backdrop" @mousedown.self="closePreview">
      <article class="modal preview-modal">
        <header>
          <h2>
            {{ previewTitle }}
            <span v-if="previewDirty" class="preview-dirty"><span class="preview-dirty-dot" />{{ t("editSave.unsaved") }}</span>
            <span v-else-if="previewBinary" class="preview-binary-badge">{{ t("binaryFile.badge") }}</span>
          </h2>
          <div v-if="previewEditableAllowed" class="preview-actions">
            <template v-if="!previewEditable">
              <button :title="t('editSave.edit')" @click="beginPreviewEdit"><Pencil />{{ t("editSave.edit") }}</button>
            </template>
            <template v-else>
              <button :title="t('cancel')" @click="cancelPreviewEdit">{{ t("cancel") }}</button>
              <button :title="t('editSave.save')" :disabled="previewSaving" @click="savePreview"><Loader2 v-if="previewSaving" class="spinning" /><Save v-else />{{ t("editSave.save") }}</button>
            </template>
          </div>
          <button class="icon-button" @click="closePreview"><X /></button>
        </header>
        <div v-if="previewLoading" class="empty"><Loader2 class="spinning" />{{ t("loading") }}</div>
        <div v-else-if="previewMode === 'image'" class="preview-image-stage">
          <img class="preview-image" :class="{ 'preview-image--full': previewImageZoomed }" :src="previewImageUrl" :alt="previewTitle" :title="previewImageZoomed ? t('imagePreview.zoomOut') : t('imagePreview.zoomIn')" @click="previewImageZoomed = !previewImageZoomed" />
        </div>
        <TextPreview v-else :text="previewText" :file-name="previewTitle" :appearance="appearance" :editable="previewEditable" @change="previewDraft = $event" />
      </article>
    </section>

    <section v-if="operationDialog === 'mkdir'" class="modal-backdrop" @mousedown.self="operationDialog = null">
      <article class="modal small-modal">
        <header><h2>{{ t("newFolder") }}</h2><button class="icon-button" @click="operationDialog = null"><X /></button></header>
        <input v-model="operationDraft" autofocus @keydown.enter="createDirectory" />
        <footer><button @click="operationDialog = null">{{ t("cancel") }}</button><button class="primary-button" :disabled="!operationDraft.trim()" @click="createDirectory">{{ t("confirm") }}</button></footer>
      </article>
    </section>

    <section v-if="commandOpen" class="modal-backdrop" @mousedown.self="commandOpen = false">
      <article class="modal command-modal">
        <header><h2>{{ t("commandTitle") }}</h2><button class="icon-button" @click="commandOpen = false"><X /></button></header>
        <input
          v-model="commandDraft"
          class="mono"
          spellcheck="false"
          autofocus
          :placeholder="t('commandPlaceholder')"
          :disabled="commandRunning"
          @keydown.enter="runCommand"
          @keydown.up.prevent="browseCommandHistoryUp"
          @keydown.down.prevent="browseCommandHistoryDown"
        />
        <div v-if="commandHistory.length" class="command-history">
          <div class="command-history-header">
            <span>{{ t("commandHistoryTitle") }}</span>
            <button class="link-button" @click="clearCommandHistory">{{ t("commandHistoryClear") }}</button>
          </div>
          <div class="command-history-list">
            <button
              v-for="item in commandHistory"
              :key="item"
              class="command-history-item mono"
              :title="t('commandHistoryResend')"
              @click="rerunHistoryCommand(item)"
            >{{ item }}</button>
          </div>
        </div>
        <label class="quick-sudo-control" :title="t('quickSudoHint')">
          <button class="switch-control" type="button" role="switch" :aria-checked="commandUseSudo" :disabled="commandRunning" @click="commandUseSudo = !commandUseSudo"><span /></button>
          <span>{{ t("quickSudo") }}</span>
        </label>
        <div v-if="commandRunning" class="command-output"><Loader2 class="spinning" /><span>{{ t("commandRunning") }}</span></div>
        <pre v-else-if="commandResult" class="command-output mono">{{ commandOutputText || t("commandNoOutput") }}<span class="command-exit">exit {{ commandResult.exitCode }}</span></pre>
        <p v-if="commandError" class="task-error">{{ commandError }}</p>
        <footer>
          <button @click="commandOpen = false">{{ t("close") }}</button>
          <button v-if="commandRunning" @click="cancelCommand"><X />{{ t("commandCancel") }}</button>
          <button class="primary-button" :disabled="!commandDraft.trim() || commandRunning" @click="runCommand">
            <Loader2 v-if="commandRunning" class="spinning" />
            <SquareTerminal v-else />
            {{ t("commandRun") }}
          </button>
        </footer>
      </article>
    </section>

    <section v-if="metricsOpen" class="modal-backdrop" @mousedown.self="closeMetrics">
      <article class="modal metrics-modal">
        <header>
          <h2>{{ t("metrics") }}<span v-if="metrics?.hostname" class="metrics-host"> · {{ metrics.hostname }}</span></h2>
          <button class="icon-button" @click="closeMetrics"><X /></button>
        </header>
        <div v-if="metricsLoading && !metrics" class="empty"><Loader2 class="spinning" />{{ t("loading") }}</div>
        <p v-else-if="metricsError" class="task-error">{{ metricsError }} <button class="link-button" @click="refreshMetrics">{{ t("refresh") }}</button></p>
        <template v-else-if="metrics">
          <div class="settings-body">
            <div class="metrics-grid">
              <div class="metric-card">
                <strong>{{ metrics.cpu?.percent ?? "–" }}%</strong>
                <span>{{ t("metricsCpu") }}</span>
                <small v-if="metrics.cpu?.cores">{{ metrics.cpu.cores }} vCPU · {{ metrics.cpu?.load1 ?? "–" }} / {{ metrics.cpu?.load5 ?? "–" }} / {{ metrics.cpu?.load15 ?? "–" }}</small>
              </div>
              <div class="metric-card">
                <strong>{{ metrics.memory?.totalBytes ? Math.round(((metrics.memory.usedBytes ?? 0) / metrics.memory.totalBytes) * 100) : "–" }}%</strong>
                <span>{{ t("metricsMemory") }}</span>
                <small v-if="metrics.memory?.totalBytes">{{ formatBytes(metrics.memory.usedBytes) }} / {{ formatBytes(metrics.memory.totalBytes) }}<template v-if="metrics.memory.swapTotalBytes"> · swap {{ formatBytes(metrics.memory.swapUsedBytes ?? 0) }}</template></small>
              </div>
              <div class="metric-card" v-if="metrics.uptimeSeconds != null">
                <strong>{{ formatUptime(metrics.uptimeSeconds) }}</strong>
                <span>{{ t("metricsUptime") }}</span>
                <small v-if="metrics.kernel">{{ metrics.kernel }}</small>
              </div>
            </div>
            <div v-if="metrics.disks?.length" class="metrics-disks">
              <div v-for="disk in metrics.disks" :key="disk.mount" class="disk-row">
                <span class="mono">{{ disk.mount }}</span>
                <progress :value="Math.min(100, disk.percentUsed)" max="100" :class="{ 'disk-warn': disk.percentUsed >= 85 }" />
                <span class="numeric">{{ formatBytes(disk.usedBytes) }} / {{ formatBytes(disk.totalBytes) }} · {{ Math.round(disk.percentUsed) }}%</span>
              </div>
            </div>
            <div v-if="metrics.network?.length">
              <h3 class="settings-section-title">{{ t("metricsNetwork") }}</h3>
              <div class="metrics-disks">
                <div
                  v-for="net in metrics.network"
                  :key="net.name"
                  class="disk-row"
                  :title="`rx ${formatBytes(net.rxTotal)} · tx ${formatBytes(net.txTotal)}`"
                >
                  <span class="mono">{{ net.name }}</span>
                  <progress :value="networkRateShare(net)" max="100" />
                  <span class="numeric">↓ {{ formatRate(net.rxRate) }} · ↑ {{ formatRate(net.txRate) }}</span>
                </div>
              </div>
            </div>
            <div v-if="metrics.processes?.length">
              <h3 class="settings-section-title">{{ t("metricsProc") }}</h3>
              <div class="file-header" :style="metricsProcGridStyle">
                <span>{{ t("metricsProcPid") }}</span>
                <span>{{ t("metricsProcUser") }}</span>
                <span class="numeric">{{ t("metricsProcCpu") }}</span>
                <span class="numeric">{{ t("metricsProcMem") }}</span>
                <span>{{ t("metricsProcCommand") }}</span>
              </div>
              <div v-for="proc in metrics.processes" :key="proc.pid" class="file-row" :style="metricsProcGridStyle">
                <span class="mono">{{ proc.pid }}</span>
                <span class="mono">{{ proc.user }}</span>
                <span class="numeric">{{ proc.cpuPercent }}%</span>
                <span class="numeric">{{ proc.memPercent }}%</span>
                <span class="mono" :title="proc.command">{{ proc.command }}</span>
              </div>
            </div>
            <p class="metrics-hint muted">{{ t("metricsRefreshHint") }}</p>
          </div>
        </template>
        <footer><button @click="closeMetrics">{{ t("close") }}</button></footer>
      </article>
    </section>

    <section v-if="chmodTarget" class="modal-backdrop" @mousedown.self="chmodTarget = undefined">
      <article class="modal small-modal">
        <header><h2>{{ t("permissionsEdit") }} · {{ chmodTarget.name }}</h2><button class="icon-button" @click="chmodTarget = undefined"><X /></button></header>
        <input v-model="chmodDraft" class="mono" spellcheck="false" :placeholder="t('permissionsPlaceholder')" @keydown.enter="confirmChmod" />
        <p class="muted">{{ t("permissionsHint") }}</p>
        <footer><button @click="chmodTarget = undefined">{{ t("cancel") }}</button><button class="primary-button" :disabled="!chmodDraft.trim() || chmodSubmitting" @click="confirmChmod">{{ t("confirm") }}</button></footer>
      </article>
    </section>

    <section v-if="deleteTarget" class="modal-backdrop" @mousedown.self="deleteTarget = undefined">
      <article class="modal small-modal destructive-modal">
        <header><h2>{{ t("deleteTitle") }}</h2><button class="icon-button" @click="deleteTarget = undefined"><X /></button></header>
        <div class="destructive-copy"><span class="destructive-icon"><Trash2 /></span><div><strong>{{ deleteTarget.name }}</strong><p class="muted">{{ t("deleteMessage") }}</p></div></div>
        <footer><button @click="deleteTarget = undefined">{{ t("cancel") }}</button><button class="danger-button" :disabled="deleteSubmitting" @click="confirmDelete"><Trash2 />{{ t("delete") }}</button></footer>
      </article>
    </section>

    <section v-if="batchDeleteOpen" class="modal-backdrop" @mousedown.self="batchDeleteOpen = false">
      <article class="modal small-modal destructive-modal">
        <header><h2>{{ t("sftpBatch.deleteTitle") }}</h2><button class="icon-button" @click="batchDeleteOpen = false"><X /></button></header>
        <div class="destructive-copy"><span class="destructive-icon"><Trash2 /></span><div><strong>{{ t("sftpBatch.selected", { count: selectedEntries.length }) }}</strong><p class="muted">{{ t("sftpBatch.deleteMessage") }}</p></div></div>
        <div v-if="batchProgress" class="batch-progress-row"><progress class="batch-progress-bar" :value="batchProgressPercent(batchProgress)" max="100" /><span class="batch-progress mono">{{ t("sftpBatch.progress", { done: batchProgress.done, total: batchProgress.total }) }}</span></div>
        <footer><button @click="batchDeleteOpen = false" :disabled="batchDeleteSubmitting">{{ t("cancel") }}</button><button class="danger-button" :disabled="batchDeleteSubmitting" @click="confirmBatchDelete"><Loader2 v-if="batchDeleteSubmitting" class="spinning" /><Trash2 v-else />{{ t("delete") }}</button></footer>
      </article>
    </section>

    <section v-if="newFileDialog" class="modal-backdrop" @mousedown.self="newFileDialog = false">
      <article class="modal small-modal">
        <header><h2>{{ t("sftpNewFile.title") }}</h2><button class="icon-button" @click="newFileDialog = false"><X /></button></header>
        <input v-model="newFileDraft" autofocus spellcheck="false" :placeholder="t('sftpNewFile.placeholder')" @keydown.enter="createNewFile" />
        <footer><button @click="newFileDialog = false">{{ t("cancel") }}</button><button class="primary-button" :disabled="!newFileDraft.trim() || newFileSubmitting" @click="createNewFile"><Loader2 v-if="newFileSubmitting" class="spinning" />{{ t("confirm") }}</button></footer>
      </article>
    </section>

    <section v-if="attrsTarget" class="modal-backdrop" @mousedown.self="closeAttributes">
      <article class="modal small-modal attrs-modal">
        <header><h2>{{ t("sftpAttrs.title") }} · {{ attrsTarget.name }}</h2><button class="icon-button" @click="closeAttributes"><X /></button></header>
        <div v-if="attrsLoading" class="empty compact"><Loader2 class="spinning" />{{ t("loading") }}</div>
        <template v-else-if="attrsInfo">
          <dl class="attrs-grid">
            <dt>{{ t("sftpAttrs.path") }}</dt><dd class="mono">{{ attrsInfo.path }}</dd>
            <dt>{{ t("sftpAttrs.type") }}</dt><dd>{{ t(`sftpAttrs.kind.${attrsInfo.kind}`) }}</dd>
            <dt>{{ t("size") }}</dt><dd class="numeric">{{ attrsInfo.kind === "directory" ? "–" : formatBytes(attrsInfo.size || 0) }}</dd>
            <dt>{{ t("sftpAttrs.permissions") }}</dt><dd class="mono">{{ attrsInfo.mode || "–" }}</dd>
            <dt>{{ t("sftpAttrs.owner") }}</dt><dd>{{ [attrsInfo.owner, attrsInfo.group].filter(Boolean).join(":") || "–" }}</dd>
            <dt>{{ t("sftpAttrs.modified") }}</dt><dd>{{ formatModified(attrsInfo.modifiedAt) || "–" }}</dd>
          </dl>
          <label class="attrs-permissions-edit">
            <span>{{ t("sftpAttrs.permissions") }}</span>
            <input v-model="attrsMode" class="mono" spellcheck="false" :placeholder="t('permissionsPlaceholder')" @keydown.enter="saveAttributesPermissions" />
          </label>
          <p class="muted">{{ t("permissionsHint") }}</p>
        </template>
        <footer>
          <button @click="closeAttributes">{{ t("close") }}</button>
          <button class="primary-button" :disabled="!canWrite || !attrsMode.trim() || attrsSubmitting" @click="saveAttributesPermissions"><Loader2 v-if="attrsSubmitting" class="spinning" />{{ t("sftpAttrs.save") }}</button>
        </footer>
      </article>
    </section>

    <section v-if="settingsOpen" class="modal-backdrop" @mousedown.self="settingsOpen = false">
      <article class="modal settings-modal">
        <header><h2>{{ t("settings") }}</h2><button class="icon-button" @click="settingsOpen = false"><X /></button></header>
        <div class="settings-body">
          <div v-if="settingsLoading" class="empty compact"><Loader2 class="spinning" />{{ t("loading") }}</div>
          <template v-else>
            <label class="quick-sudo-control">
              <button class="switch-control" type="button" role="switch" :aria-checked="settingsDraft.quickSudo" @click="settingsDraft.quickSudo = !settingsDraft.quickSudo"><span /></button>
              <span>{{ t("settingsQuickSudo") }}</span>
            </label>
            <label class="settings-field">
              <span>{{ t("settingsSudoPassword") }}</span>
              <input v-model="settingsDraft.sudoPassword" type="password" autocomplete="off" :placeholder="settingsMeta?.sudoPasswordSet ? t('settingsConfigured') : t('settingsSudoPasswordPlaceholder')" />
            </label>
            <label class="settings-field">
              <span>{{ t("settingsTotp") }}</span>
              <textarea v-model="settingsDraft.totpSecret" rows="2" spellcheck="false" :placeholder="settingsMeta?.totpConfigured ? t('settingsConfigured') : t('settingsTotpPlaceholder')" />
            </label>
            <label class="settings-field">
              <span>{{ t("settingsFlowMode") }}</span>
              <select v-model="settingsDraft.authFlowMode">
                <option value="password_then_otp">{{ t("flowThenOtp") }}</option>
                <option value="password_plus_otp">{{ t("flowPlusOtp") }}</option>
                <option value="password_only">{{ t("flowOnly") }}</option>
              </select>
            </label>
            <label class="settings-field">
              <span>{{ t("settingsPasswordHint") }}</span>
              <input v-model="settingsDraft.passwordPromptHint" spellcheck="false" :placeholder="t('settingsHintPlaceholder')" />
            </label>
            <label class="settings-field">
              <span>{{ t("settingsTotpHint") }}</span>
              <input v-model="settingsDraft.totpPromptHint" spellcheck="false" :placeholder="t('settingsHintPlaceholder')" />
            </label>
            <label class="quick-sudo-control">
              <button class="switch-control" type="button" role="switch" :aria-checked="settingsDraft.sudoUsePty" @click="settingsDraft.sudoUsePty = !settingsDraft.sudoUsePty"><span /></button>
              <span>{{ t("settingsUsePty") }}</span>
            </label>
            <p class="muted settings-note">{{ t("settingsNote") }}</p>
          </template>

          <h3 class="settings-section-title">{{ t("knownHosts.title") }}</h3>
          <div v-if="knownHostsLoading" class="empty compact"><Loader2 class="spinning" />{{ t("loading") }}</div>
          <p v-else-if="knownHostsError" class="task-error">{{ knownHostsError }} <button class="link-button" @click="loadKnownHosts">{{ t("refresh") }}</button></p>
          <div v-else-if="!knownHosts.length" class="empty compact">{{ t("knownHosts.empty") }}</div>
          <ul v-else class="settings-list">
            <li v-for="(entry, index) in knownHosts" :key="`${entry.host}:${entry.port}:${entry.keyType}:${index}`">
              <div class="settings-list-main">
                <strong class="mono">{{ entry.host }}:{{ entry.port }}</strong>
                <span class="muted">{{ entry.keyType }} · <span class="mono" :title="entry.fingerprint">{{ shortFingerprint(entry.fingerprint) }}</span></span>
              </div>
              <button class="icon-button" :title="t('delete')" @click="removeKnownHost(entry)"><Trash2 /></button>
            </li>
          </ul>

          <h3 class="settings-section-title">{{ t("keysPanel.title") }}</h3>
          <div v-if="localKeysLoading" class="empty compact"><Loader2 class="spinning" />{{ t("loading") }}</div>
          <p v-else-if="localKeysError" class="task-error">{{ localKeysError }} <button class="link-button" @click="loadLocalKeys">{{ t("refresh") }}</button></p>
          <div v-else-if="!localKeys.length" class="empty compact">{{ t("keysPanel.empty") }}</div>
          <ul v-else class="settings-list">
            <li v-for="key in localKeys" :key="key.path">
              <div class="settings-list-main">
                <strong class="mono" :title="key.path">{{ key.path }}</strong>
                <span class="muted">{{ key.algorithm }} · <span class="mono" :title="key.fingerprint">{{ shortFingerprint(key.fingerprint) }}</span><template v-if="key.hasPassphrase"> · {{ t("keysPanel.hasPassphrase") }}</template></span>
              </div>
              <KeyRound class="settings-key-icon" />
            </li>
          </ul>
          <p class="muted settings-note">{{ t("keysPanel.hint") }}</p>

          <h3 class="settings-section-title">{{ t("mcpLimits.title") }}</h3>
          <div v-if="mcpLoading" class="empty compact"><Loader2 class="spinning" />{{ t("loading") }}</div>
          <template v-else>
            <div class="mcp-limits">
              <label class="settings-field">
                <span>{{ t("mcpLimits.read") }}</span>
                <input v-model="mcpDraft.readMiB" type="number" min="1" step="1" inputmode="numeric" />
              </label>
              <label class="settings-field">
                <span>{{ t("mcpLimits.upload") }}</span>
                <input v-model="mcpDraft.uploadMiB" type="number" min="1" step="1" inputmode="numeric" />
              </label>
              <label class="settings-field">
                <span>{{ t("mcpLimits.download") }}</span>
                <input v-model="mcpDraft.downloadMiB" type="number" min="1" step="1" inputmode="numeric" />
              </label>
            </div>
            <p v-if="!mcpInputsValid" class="task-error">{{ t("mcpLimits.invalid") }}</p>
            <p v-if="mcpError" class="task-error">{{ mcpError }} <button class="link-button" @click="loadMcpSettings">{{ t("refresh") }}</button></p>
            <button class="link-button" :disabled="!mcpInputsValid || mcpSaving" @click="saveMcpSettings"><Loader2 v-if="mcpSaving" class="spinning" />{{ t("save") }}</button>
          </template>
        </div>
        <footer>
          <button :disabled="!settingsMeta?.sudoPasswordSet && !settingsMeta?.totpConfigured" @click="clearStoredSecrets"><Trash2 />{{ t("settingsClearSecrets") }}</button>
          <button @click="settingsOpen = false">{{ t("close") }}</button>
          <button class="primary-button" :disabled="settingsSaving" @click="saveSettings"><Loader2 v-if="settingsSaving" class="spinning" />{{ t("settingsSave") }}</button>
        </footer>
      </article>
    </section>
    <section v-if="hostKeyPrompt" class="modal-backdrop">
      <article class="modal host-key-modal">
        <header><h2>Verify SSH host key</h2></header>
        <p>Confirm this fingerprint before DBX sends credentials.</p>
        <dl><dt>Server</dt><dd>{{ hostKeyPrompt.host }}:{{ hostKeyPrompt.port }}</dd><dt>Key type</dt><dd>{{ hostKeyPrompt.keyType }}</dd><dt>Fingerprint</dt><dd class="fingerprint">{{ hostKeyPrompt.fingerprint }}</dd></dl>
        <label class="remember"><input v-model="rememberHostKey" type="checkbox" /> Remember this key</label>
        <footer><button @click="resolveHostKey(false)">Reject</button><button class="primary-button" @click="resolveHostKey(true)">Trust and connect</button></footer>
      </article>
    </section>

    <section v-if="pasteConfirm" class="modal-backdrop" @mousedown.self="resolvePasteConfirm(false)">
      <article class="modal small-modal" :class="{ 'destructive-modal': pasteConfirm.danger }">
        <header>
          <h2>{{ pasteConfirm.danger ? t("terminalDanger.title") : t("terminalPasteConfirm.title") }}</h2>
          <button class="icon-button" @click="resolvePasteConfirm(false)"><X /></button>
        </header>
        <div v-if="pasteConfirm.danger" class="destructive-copy">
          <span class="destructive-icon"><TriangleAlert /></span>
          <div>
            <strong>{{ t("terminalDanger.detected") }}</strong>
            <p class="task-error mono">{{ pasteConfirm.hits.map((hit) => hit.label).join(" · ") }}</p>
            <p class="muted">{{ t("terminalDanger.desc") }}</p>
          </div>
        </div>
        <p v-else class="muted">{{ t("terminalPasteConfirm.desc") }}</p>
        <p class="muted">{{ t("terminalPasteConfirm.summary", { lines: pasteConfirm.lines, chars: pasteConfirm.chars }) }}<template v-if="pasteConfirm.lines > 1"> · {{ t("terminalPasteConfirm.multiline") }}</template></p>
        <pre class="command-output mono">{{ pasteConfirm.preview }}</pre>
        <footer>
          <button @click="resolvePasteConfirm(false)">{{ t("cancel") }}</button>
          <button :class="pasteConfirm.danger ? 'danger-button' : 'primary-button'" @click="resolvePasteConfirm(true)">{{ t("terminalPasteConfirm.confirm") }}</button>
        </footer>
      </article>
    </section>

    <input ref="uploadInput" class="hidden" type="file" multiple @change="onUploadInput" />
    <input ref="zmodemInput" class="hidden" type="file" multiple @change="onZmodemInput" />
  </main>
</template>

<style scoped>
/* SFTP 面板扩展（工作包 B）：搜索/类型过滤、批量条、路径历史、属性弹窗。 */
.sftp-filter-bar { display: flex; align-items: center; gap: 6px; border-bottom: 1px solid var(--border); padding: 5px 7px; }
.sftp-search-input { display: flex; flex: 1; min-width: 0; height: 26px; align-items: center; gap: 5px; border: 1px solid var(--border); border-radius: 5px; padding: 0 7px; background: var(--background); }
.sftp-search-input:focus-within { border-color: color-mix(in srgb, var(--primary) 70%, var(--border)); }
.sftp-search-input svg { width: 13px; height: 13px; flex: 0 0 13px; color: var(--muted-foreground); }
.sftp-search-input input { min-width: 0; flex: 1; border: 0; padding: 0; background: transparent; color: var(--foreground); font-size: 12px; outline: none; }
.sftp-search-clear { display: grid; width: 16px; height: 16px; flex: 0 0 16px; border: 0; border-radius: 50%; padding: 0; place-items: center; background: transparent; color: var(--muted-foreground); cursor: pointer; }
.sftp-search-clear:hover { background: var(--accent); color: var(--foreground); }
.sftp-search-clear svg { width: 11px; height: 11px; }
.sftp-type-filter { height: 26px; border: 1px solid var(--border); border-radius: 5px; padding: 0 3px; background: var(--background); color: var(--foreground); font-size: 12px; }
.sftp-batch-bar { display: flex; align-items: center; gap: 6px; border-bottom: 1px solid var(--border); padding: 5px 8px; background: color-mix(in srgb, var(--primary) 8%, var(--background)); color: var(--muted-foreground); font-size: 11px; }
.sftp-batch-bar span { min-width: 0; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sftp-batch-bar button { display: inline-flex; height: 22px; align-items: center; gap: 4px; border: 1px solid var(--border); border-radius: 4px; padding: 0 7px; background: var(--background); color: var(--foreground); font-size: 11px; cursor: pointer; }
.sftp-batch-bar button:hover:not(:disabled) { background: var(--accent); }
.sftp-batch-bar button.danger { color: var(--destructive); }
.sftp-batch-bar button svg { width: 12px; height: 12px; }
.sftp-batch-bar .batch-progress { flex: 0 0 auto; overflow: visible; font-variant-numeric: tabular-nums; }
.batch-progress-bar { flex: 0 1 140px; height: 6px; min-width: 80px; accent-color: var(--primary); }
.batch-progress-row { display: flex; align-items: center; gap: 8px; margin-top: 4px; }
.batch-progress-row .batch-progress-bar { flex: 1; }
.path-history-popover { display: flex; width: 250px; max-height: min(320px, 50vh); flex-direction: column; padding: 6px; overflow: auto; }
.path-history-title { margin: 4px; color: var(--muted-foreground); font-size: 10px; letter-spacing: .04em; text-transform: uppercase; }
.path-history-popover .path-item { display: block; width: 100%; height: 26px; overflow: hidden; border: 0; border-radius: 4px; padding: 0 7px; background: transparent; color: var(--foreground); font-size: 11px; text-align: left; text-overflow: ellipsis; white-space: nowrap; cursor: pointer; }
.path-history-popover .path-item:hover { background: var(--accent); }
.attrs-grid { display: grid; grid-template-columns: auto 1fr; gap: 6px 14px; margin: 0; font-size: 12px; }
.attrs-grid dt { color: var(--muted-foreground); white-space: nowrap; }
.attrs-grid dd { margin: 0; overflow-wrap: anywhere; }
.attrs-permissions-edit { display: flex; align-items: center; gap: 8px; font-size: 12px; }
.attrs-permissions-edit span { flex: 0 0 auto; color: var(--muted-foreground); }
.attrs-permissions-edit input { flex: 1; }

/* 工具栏 A+/A- 字号步进按钮（复用 Ctrl+滚轮的 clampFontSize 语义） */
.font-step-label { font-size: 11px; font-weight: 600; line-height: 1; letter-spacing: 0; }

/* 命令历史下拉（命令弹窗内）：↑↓ 浏览 + 点击一键重发 */
.command-history { display: flex; flex-direction: column; gap: 4px; }
.command-history-header { display: flex; align-items: center; justify-content: space-between; color: var(--muted-foreground); font-size: 10px; letter-spacing: .04em; text-transform: uppercase; }
.command-history-list { display: flex; max-height: 168px; flex-direction: column; gap: 1px; overflow: auto; }
.command-history-item { display: block; width: 100%; overflow: hidden; border: 1px solid transparent; border-radius: 4px; padding: 4px 8px; background: transparent; color: var(--foreground); font-size: 11px; text-align: left; text-overflow: ellipsis; white-space: nowrap; cursor: pointer; }
.command-history-item:hover { background: var(--accent); border-color: var(--border); }

/* 快速命令栏（工具栏下拉）：发送 / 编辑 / 删除 + 底部新增编辑器 */
.quick-commands-popover { display: flex; width: min(360px, calc(100vw - 24px)); max-height: min(480px, calc(100vh - 60px)); flex-direction: column; gap: 4px; padding: 8px; overflow: auto; }
.quick-commands-popover h3 { margin: 2px 4px 6px; font-size: 12px; }
.quick-command-row { display: flex; align-items: center; gap: 2px; }
.quick-command-row .icon-button { width: 24px; height: 24px; flex: 0 0 24px; }
.quick-command-row .icon-button svg { width: 12px; height: 12px; }
.quick-command-send { display: flex; min-width: 0; flex: 1; flex-direction: column; align-items: flex-start; gap: 1px; border: 1px solid transparent; border-radius: 4px; padding: 4px 7px; background: transparent; color: var(--foreground); text-align: left; cursor: pointer; }
.quick-command-send:hover { background: var(--accent); border-color: var(--border); }
.quick-command-send strong { max-width: 100%; overflow: hidden; font-size: 11px; text-overflow: ellipsis; white-space: nowrap; }
.quick-command-send .mono { max-width: 100%; overflow: hidden; color: var(--muted-foreground); font-size: 10px; text-overflow: ellipsis; white-space: nowrap; }
.quick-command-editor { display: flex; flex-direction: column; gap: 5px; border-top: 1px solid var(--border); margin-top: 4px; padding-top: 8px; }
.quick-command-editor input { width: 100%; height: 28px; border: 1px solid var(--border); border-radius: 5px; padding: 0 8px; background: var(--background); color: var(--foreground); font-size: 12px; }
.quick-command-editor input:focus { border-color: color-mix(in srgb, var(--primary) 70%, var(--border)); }
.quick-command-editor-actions { display: flex; align-items: center; gap: 6px; }
.quick-command-editor-actions .quick-command-limit { flex: 1; overflow: hidden; color: var(--muted-foreground); font-size: 10px; text-align: right; text-overflow: ellipsis; white-space: nowrap; }
.quick-command-editor-actions button { height: 26px; border: 1px solid var(--border); border-radius: 5px; padding: 0 10px; background: var(--background); color: var(--foreground); font-size: 11px; cursor: pointer; }
.quick-command-editor-actions .primary-button { background: var(--primary); color: var(--primary-foreground); }

/* 连接信息面板（工具栏下拉，只读） */
.connection-info-popover { width: min(300px, calc(100vw - 24px)); padding: 8px 12px 12px; }
.connection-info-popover h3 { margin: 4px 0 8px; font-size: 12px; }
.connection-info-grid { display: grid; grid-template-columns: auto 1fr; gap: 6px 14px; margin: 0; font-size: 12px; }
.connection-info-grid dt { color: var(--muted-foreground); white-space: nowrap; }
.connection-info-grid dd { display: flex; min-width: 0; align-items: center; gap: 8px; margin: 0; overflow-wrap: anywhere; }
.connection-info-grid .task-error { font-size: 10px; }
.connection-info-grid .link-button { flex: 0 0 auto; align-self: center; font-size: 10px; }
</style>
