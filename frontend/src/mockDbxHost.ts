const eventListeners = new Set<(event: DbxPluginEvent) => void>();
const binaryListeners = new Set<(event: DbxPluginBinaryEvent) => void>();
const appearanceListeners = new Set<(appearance: DbxPluginAppearance) => void>();
const contextListeners = new Set<(context: Record<string, unknown>) => void>();

const fixtureParams = new URLSearchParams(location.search);
// ?rw=1 模拟可写连接（默认只读），供拖放上传等写路径 UI 验证。
const writable = fixtureParams.get("rw") === "1";
// ?err=disconnect 在会话建立 4s 后模拟一次传输断开（ssh/session/state
// disconnected，单次不复发），供重连横幅/倒计时/立即重连/恢复提示的全流程 UI 验证。
const disconnectAfterMs = fixtureParams.get("err") === "disconnect" ? 4000 : 0;
let disconnectEmitted = false;
// 与 DBX globals.css 的 :root（pearl 浅色）和 .dark 规范块保持一致。
const light = fixtureParams.get("theme") === "light";

const context = {
  connectionId: "visual-connection",
  workbenchId: "visual-workbench",
  restored: false,
  workbenchState: { sftpPath: "/home/demo", splitRatio: 58, paneOrder: "terminal-left", visibleColumns: ["size", "modified", "permissions"] },
  connection: { name: "Production SSH", host: "192.168.1.64", port: 22, username: "user", color: "#3b82f6", readOnly: !writable },
};

const appearance: DbxPluginAppearance = {
  colorScheme: light ? "light" : "dark",
  colors: light
    ? { background: "rgb(255 255 255)", foreground: "rgb(10 10 10)", muted: "rgb(245 245 245)", mutedForeground: "rgb(115 115 115)", accent: "rgb(245 245 245)", accentForeground: "rgb(23 23 23)", border: "rgb(229 229 229)", destructive: "rgb(231 0 11)" }
    : { background: "rgb(19 20 22)", foreground: "rgb(215 215 219)", muted: "rgb(42 42 45)", mutedForeground: "rgb(151 152 157)", accent: "rgb(46 47 51)", accentForeground: "rgb(221 221 226)", border: "rgb(110 110 114 / 0.28)", destructive: "rgb(243 98 95)" },
  terminal: { fontFamily: "Cascadia Mono, Consolas, monospace", fontSize: 13 },
};

function terminalFrame(sequence: number, text: string) {
  const data = new TextEncoder().encode(text);
  const frame = new Uint8Array(9 + data.length);
  frame[0] = 0;
  new DataView(frame.buffer).setBigUint64(1, BigInt(sequence), false);
  frame.set(data, 9);
  return frame;
}

function base64(bytes: Uint8Array) {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

let sequence = 0;
function emitTerminal(text: string) {
  sequence += 1;
  const event = { channel: "ssh/terminal/out/visual-session", dataBase64: base64(terminalFrame(sequence, text)) };
  for (const listener of binaryListeners) listener(event);
}

// ---- 内存 fixture 树：路径感知的 sftp/list 与写操作（?mock=1 走通侧栏树/新建/压缩）----
interface MockNode {
  name: string;
  kind: "directory" | "file";
  size: number;
  modifiedAt: number;
  permissions: string;
  children?: MockNode[];
}

let mockStamp = 1786262400;
function nextMockStamp() {
  mockStamp += 3600;
  return mockStamp;
}
function mockDir(name: string, children: MockNode[] = []): MockNode {
  return { name, kind: "directory", size: 0, modifiedAt: nextMockStamp(), permissions: "0755", children };
}
function mockFile(name: string, size: number, permissions = "0644"): MockNode {
  return { name, kind: "file", size, modifiedAt: nextMockStamp(), permissions };
}

const mockTree: MockNode = mockDir("/", [
  mockDir("home", [
    mockDir("demo", [
      mockDir(".config"),
      mockDir("projects"),
      mockFile("deploy.sh", 2481, "0755"),
      mockFile("docker-compose.yml", 8192),
      mockFile("server.log", 741248),
    ]),
  ]),
  mockDir("etc", [mockFile("hosts", 221), mockDir("nginx", [mockFile("nginx.conf", 1264)])]),
  mockDir("tmp"),
  mockDir("var", [mockDir("log", [mockFile("syslog", 15432)])]),
  mockDir("root"),
]);

function normalizeMockPath(path: string): string {
  let value = (path || "/").trim() || "/";
  if (!value.startsWith("/")) value = `/${value}`;
  value = value.replace(/\/{2,}/g, "/");
  return value === "/" ? value : value.replace(/\/+$/, "");
}

function findMockNode(path: string): MockNode | null {
  const normalized = normalizeMockPath(path);
  if (normalized === "/") return mockTree;
  let node: MockNode = mockTree;
  for (const segment of normalized.slice(1).split("/")) {
    const next = node.children?.find((child) => child.name === segment);
    if (!next) return null;
    node = next;
  }
  return node;
}

function mockParentAndName(path: string): { parent: MockNode | null; name: string } {
  const normalized = normalizeMockPath(path);
  const index = normalized.lastIndexOf("/");
  const parentPath = index <= 0 ? "/" : normalized.slice(0, index);
  return { parent: findMockNode(parentPath), name: normalized.slice(index + 1) };
}

function mockEntryOf(node: MockNode, parentPath: string) {
  return {
    name: node.name,
    uri: `sftp:${parentPath === "/" ? "" : parentPath}/${node.name}`,
    kind: node.kind,
    ...(node.kind === "file" ? { size: node.size } : {}),
    modifiedAt: node.modifiedAt,
    permissions: node.permissions,
  };
}

function mockList(path: string) {
  const parentPath = normalizeMockPath(path);
  const node = findMockNode(parentPath);
  if (!node || node.kind !== "directory") throw new Error(`sftp: no such directory: ${parentPath}`);
  return (node.children ?? [])
    .map((child) => mockEntryOf(child, parentPath))
    .sort((a, b) => (a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === "directory" ? -1 : 1));
}

function mockWriteEntry(path: string, node: MockNode): { success: true } {
  const { parent, name } = mockParentAndName(path);
  if (!parent || parent.kind !== "directory" || !name || parent.children?.some((child) => child.name === name)) {
    throw new Error(`sftp: cannot write ${normalizeMockPath(path)}`);
  }
  parent.children!.push(node);
  return { success: true };
}

const fixtureDownloads = new Map<string, { fileName: string; size: number; offset: number }>();
const fixtureUploadCount = { value: 0 };
const settingsState = { quickSudo: true, sudoUsePty: false, sudoPasswordSet: true, totpConfigured: false, authFlowMode: "password_then_otp", passwordPromptHint: "", totpPromptHint: "" };

const request: DbxPluginApi["request"] = async <T = unknown>(method: string) =>
  (method === "host.getContext" ? context : null) as T;

const invoke: DbxPluginApi["invoke"] = async <T = unknown>(method: string, params?: unknown) => {
  let result: unknown;
  if (method === "ssh/session/open") {
    // A fresh session restarts sequence numbering at 1 (real sidecar
    // semantics): after an auto-reconnect the client resets its cursor to 0,
    // so continuing the global counter here would leave a permanent hole at
    // the old tail and spin the client's replay loop.
    sequence = 0;
    // Simulate a VS Code-style shell integration cycle (OSC 633) so the
    // command marker strip has something to render in the visual fixture.
    const osc = "\u001b]633;";
    const bel = "\u0007";
    const cycle = [
      `${osc}P;Cwd=/home/demo${bel}`,
      `${osc}A${bel}`,
      `${osc}E;systemctl status nginx${bel}`,
      "user@server:~$ systemctl status nginx\r\n",
      `${osc}C${bel}`,
      "● nginx.service - A high performance web server\r\n   Active: active (running)\r\n",
      `${osc}D;0${bel}`,
      `${osc}A${bel}`,
      "user@server:~$ ",
    ].join("");
    setTimeout(() => emitTerminal(`Welcome to DBX SSH/SFTP visual fixture\r\n${cycle}`), 30);
    if (disconnectAfterMs && !disconnectEmitted) {
      disconnectEmitted = true;
      setTimeout(() => {
        for (const listener of eventListeners) listener({ method: "ssh/session/state", params: { sessionId: "visual-session", state: "disconnected" } });
      }, disconnectAfterMs);
    }
    result = { sessionId: "visual-session", connectionId: context.connectionId, workbenchId: context.workbenchId, connected: true, sequence: 0, chunkSize: 262144, directoryTrackingSupported: true };
  } else if (method === "ssh/terminal/replay") result = { frameCount: 0, firstAvailableSequence: 1, tailSequence: sequence, complete: true };
  else if (method === "ssh/sessions/list") result = { sessions: [{ sessionId: "visual-session", connectionId: context.connectionId, workbenchId: context.workbenchId, readOnly: !writable, connected: true, sudoKeepalive: true, createdAt: Math.floor(Date.now() / 1000), authMethod: "private-key" }] };
  else if (method === "sftp/list" || method === "sudo/listDir") result = { entries: mockList(String((params as Record<string, unknown>)?.path || "/")) };
  else if (method === "sftp/home") result = { path: "/home/demo" };
  else if (method === "sftp/createDirectory") result = mockWriteEntry(String((params as Record<string, unknown>)?.path || ""), mockDir(String((params as Record<string, unknown>)?.path || "/").split("/").pop() || "folder"));
  else if (method === "sftp/touch") result = mockWriteEntry(String((params as Record<string, unknown>)?.path || ""), mockFile(String((params as Record<string, unknown>)?.path || "").split("/").pop() || "file.txt", 0));
  else if (method === "sftp/archive") {
    const input = params as Record<string, unknown>;
    const sources = Array.isArray(input.sourcePaths) ? (input.sourcePaths as string[]) : [];
    const total = sources.reduce((sum, source) => sum + (findMockNode(source)?.size || 1024), 0);
    result = mockWriteEntry(String(input.archivePath || ""), mockFile(String(input.archivePath || "").split("/").pop() || "archive.tar.gz", Math.max(total, 512)));
  }
  else if (method === "sftp/extract") {
    const input = params as Record<string, unknown>;
    const destination = String(input.destinationPath || "");
    result = mockWriteEntry(destination, mockDir(destination.split("/").pop() || "extracted", [mockFile("README", 64)]));
  }
  else if (method === "sftp/delete") {
    const { parent, name } = mockParentAndName(String((params as Record<string, unknown>)?.path || ""));
    const index = parent?.children?.findIndex((child) => child.name === name) ?? -1;
    if (!parent || index < 0) throw new Error(`sftp: no such file: ${name}`);
    parent.children!.splice(index, 1);
    result = { success: true };
  }
  else if (method === "sftp/rename") {
    const input = params as Record<string, unknown>;
    const node = findMockNode(String(input.sourcePath || ""));
    if (!node) throw new Error(`sftp: no such file: ${input.sourcePath}`);
    const { parent: sourceParent, name: sourceName } = mockParentAndName(String(input.sourcePath || ""));
    const sourceIndex = sourceParent?.children?.findIndex((child) => child.name === sourceName) ?? -1;
    sourceParent?.children?.splice(sourceIndex, 1);
    node.name = String(input.targetPath || "").split("/").pop() || node.name;
    result = mockWriteEntry(String(input.targetPath || ""), node);
  }
  else if (method === "sftp/exists") result = { exists: !!findMockNode(String((params as Record<string, unknown>)?.path || "")) };
  else if (method === "sftp/stat") {
    const node = findMockNode(String((params as Record<string, unknown>)?.path || ""));
    if (!node) throw new Error("sftp: no such file");
    result = { path: normalizeMockPath(String((params as Record<string, unknown>)?.path || "")), kind: node.kind, ...(node.kind === "file" ? { size: node.size } : {}), modifiedAt: node.modifiedAt, mode: node.permissions };
  }
  else if (method === "sftp/transfer/list") result = { tasks: [] };
  else if (method === "sftp/upload/start") result = { taskId: `visual-upload-${++fixtureUploadCount.value}`, chunkSize: 262144 };
  else if (method === "sftp/upload/finish") result = { success: true };
  else if (method === "sftp/transfer/cancel") result = { success: true };
  else if (method === "sftp/read") result = { dataBase64: base64(new TextEncoder().encode("#!/usr/bin/env bash\nset -euo pipefail\n\necho deploy\n")), truncated: false };
  else if (method === "sftp/download/start") {
    const remotePath = normalizeMockPath(String((params as Record<string, unknown>)?.remotePath || "download.bin"));
    const node = findMockNode(remotePath);
    const taskId = `visual-download-${fixtureDownloads.size + 1}`;
    const fileName = node?.kind === "file" ? node.name : remotePath.split("/").pop() || "download.bin";
    const size = node?.kind === "file" ? node.size : 32;
    fixtureDownloads.set(taskId, { fileName, size, offset: 0 });
    for (const listener of eventListeners) listener({ method: "sftp/transfer/progress", params: { taskId, sessionId: "visual-session", direction: "download", fileName, transferred: 0, size, status: "queued" } });
    result = { taskId, fileName, size, chunkSize: 262144 };
  } else if (method === "sftp/download/next") {
    const input = params as Record<string, unknown>;
    const taskId = String(input.taskId || "");
    const task = fixtureDownloads.get(taskId)!;
    const offset = Number(input.offset || 0);
    const length = Math.min(262144, task.size - offset);
    const payload = new Uint8Array(8 + length);
    new DataView(payload.buffer).setBigUint64(0, BigInt(offset), false);
    for (const listener of binaryListeners) listener({ channel: `sftp/download/${taskId}`, dataBase64: base64(payload) });
    task.offset = offset + length;
    for (const listener of eventListeners) listener({ method: "sftp/transfer/progress", params: { taskId, sessionId: "visual-session", direction: "download", fileName: task.fileName, transferred: task.offset, size: task.size, status: "running" } });
    result = { length, eof: task.offset >= task.size };
  } else if (method === "sftp/download/finish") {
    const taskId = String((params as Record<string, unknown>)?.taskId || "");
    const task = fixtureDownloads.get(taskId)!;
    for (const listener of eventListeners) listener({ method: "sftp/transfer/progress", params: { taskId, sessionId: "visual-session", direction: "download", fileName: task.fileName, transferred: task.size, size: task.size, status: "completed" } });
    fixtureDownloads.delete(taskId);
    result = { success: true };
  } else if (method === "ssh/exec") {
    const input = params as Record<string, unknown>;
    const command = String(input.command || "");
    const sudo = input.sudo === true;
    await new Promise((resolve) => setTimeout(resolve, 1500));
    if (!sudo && !command.includes("sudo")) {
      // ANSI colour codes exercise the control-sequence sanitization in the dialog.
      result = { success: true, output: `\u001b[32muid=1000(demo) gid=1000(demo)\u001b[0m\n$ ${command}`, exitCode: 0 };
    } else {
      setTimeout(() => emitTerminal("user@server:~$ sudo -S systemctl status nginx\r\n[sudo] password for user: \r\n● nginx.service - A high performance web server\r\n   Active: active (running)\r\n"), 30);
      result = { success: true, output: "● nginx.service - A high performance web server\n   Loaded: loaded (/lib/systemd/system/nginx.service; enabled)\n   Active: active (running) since Mon 2026-08-24 09:12:31 UTC; 3 days ago", exitCode: 0 };
    }
  }
  else if (method === "ssh/metrics") {
    const totalBytes = 16_573_006_848;
    const availableBytes = 11_012_874_240;
    result = {
      hostname: "web-01.demo.internal",
      kernel: "6.1.0-18-amd64",
      uptimeSeconds: 1_234_567,
      cpu: { cores: 8, percent: 23.4, load1: 0.42, load5: 0.51, load15: 0.48 },
      memory: { totalBytes, availableBytes, usedBytes: totalBytes - availableBytes, swapTotalBytes: 2_147_483_648, swapUsedBytes: 0 },
      disks: [
        { filesystem: "/dev/sda1", mount: "/", totalBytes: 52_723_200_512, usedBytes: 24_023_981_056, availableBytes: 26_005_927_936, percentUsed: 48 },
        { filesystem: "/dev/sdb1", mount: "/data", totalBytes: 105_550_471_168, usedBytes: 58_052_563_968, availableBytes: 47_497_871_360, percentUsed: 55 },
        { filesystem: "tmpfs", mount: "/dev/shm", totalBytes: 8_146_615_296, usedBytes: 0, availableBytes: 8_146_615_296, percentUsed: 0 },
      ],
    };
  }
  else if (method === "sftp/diskUsage") {
    result = { filesystem: "/dev/sda1", mount: "/", totalBytes: 52_723_200_512, usedBytes: 24_023_981_056, availableBytes: 26_005_927_936, percentUsed: 48 };
  }
  else if (method === "sftp/chmod") {
    const input = params as Record<string, unknown>;
    const node = findMockNode(String(input.path || ""));
    if (node) node.permissions = String(input.mode || node.permissions);
    result = { success: true };
  }
  else if (method === "ssh/settings/get") {
    result = { quickSudo: true, sudoUsePty: false, sudoPasswordSet: true, totpConfigured: false, authFlowMode: "password_then_otp", passwordPromptHint: "", totpPromptHint: "" };
  }
  else if (method === "ssh/settings/set") {
    const input = params as Record<string, unknown>;
    settingsState.quickSudo = typeof input.quickSudo === "boolean" ? input.quickSudo : settingsState.quickSudo;
    settingsState.authFlowMode = typeof input.authFlowMode === "string" ? input.authFlowMode : settingsState.authFlowMode;
    settingsState.passwordPromptHint = typeof input.passwordPromptHint === "string" ? input.passwordPromptHint : settingsState.passwordPromptHint;
    settingsState.totpPromptHint = typeof input.totpPromptHint === "string" ? input.totpPromptHint : settingsState.totpPromptHint;
    settingsState.sudoPasswordSet = typeof input.sudoPassword === "string" ? input.sudoPassword.length > 0 : settingsState.sudoPasswordSet;
    settingsState.totpConfigured = typeof input.totpSecret === "string" ? input.totpSecret.trim().length > 0 : settingsState.totpConfigured;
    result = { ...settingsState };
  }
  else if (method === "ssh/exec/cancel") result = { success: true };
  else if (method === "sudo/profiles/list") result = { profiles: [] };
  else if (method === "sudo/profiles/save" || method === "sudo/profiles/delete") result = { success: true };
  else if (method === "ssh/knownHosts/list") result = { entries: [] };
  else if (method === "keys/discover") result = { keys: [] };
  else if (method === "mcp/settings/get") result = { maxReadBytes: 8 * 1024 * 1024, maxUploadBytes: 64 * 1024 * 1024, maxDownloadBytes: 256 * 1024 * 1024 };
  else result = { success: true };
  return result as T;
};

window.dbxPlugin = {
  ready: Promise.resolve(context),
  context,
  appearance,
  locale: "en",
  request,
  invoke,
  notify: async () => undefined,
  sendBinary: async (channel, data) => {
    if (channel.startsWith("sftp/upload/")) {
      const bytes = typeof data === "string" ? Uint8Array.from(atob(data), (value) => value.charCodeAt(0)) : data instanceof Uint8Array ? data : new Uint8Array(data);
      const offset = Number(new DataView(bytes.buffer, bytes.byteOffset, 8).getBigUint64(0, false));
      const taskId = channel.slice("sftp/upload/".length);
      for (const listener of eventListeners) listener({ method: "sftp/upload/ack", params: { taskId, nextOffset: offset + Math.max(0, bytes.byteLength - 8) } });
      return;
    }
    if (!channel.startsWith("ssh/terminal/in/")) return;
    const bytes = typeof data === "string" ? Uint8Array.from(atob(data), (value) => value.charCodeAt(0)) : data instanceof Uint8Array ? data : new Uint8Array(data);
    const inputSequence = Number(new DataView(bytes.buffer, bytes.byteOffset, 8).getBigUint64(0, false));
    for (const listener of eventListeners) listener({ method: "ssh/terminal/inputAck", params: { sequence: inputSequence } });
  },
  onEvent: (listener) => { eventListeners.add(listener); return () => eventListeners.delete(listener); },
  onBinary: (listener) => { binaryListeners.add(listener); return () => binaryListeners.delete(listener); },
  onAppearanceChange: (listener) => { appearanceListeners.add(listener); listener(appearance); return () => appearanceListeners.delete(listener); },
  onContextChange: (listener) => { contextListeners.add(listener); listener(context); return () => contextListeners.delete(listener); },
  decodeBase64: (value) => Uint8Array.from(atob(value), (character) => character.charCodeAt(0)),
  encodeBase64: base64,
  workbenchState: { set: async () => undefined },
  clipboard: { readText: async () => "", writeText: async () => undefined },
  fileTransfer: {
    pick: async () => ({ files: [] }),
    read: async () => ({ dataBase64: "", length: 0, eof: true }),
    beginSave: async () => ({ handleId: "visual-save-handle-0001", chunkBytes: 262144 }),
    write: async (_handleId, offset, data) => ({ written: typeof data === "string" ? data.length : data.byteLength, nextOffset: offset + (typeof data === "string" ? data.length : data.byteLength) }),
    finish: async () => undefined,
    cancel: async () => undefined,
    onDragState: () => () => undefined,
    onDrop: () => () => undefined,
  },
};
