/**
 * Terminal interaction preferences (select-to-copy / right-click-to-paste).
 * The toggle is a pure-frontend behavior (no sidecar involvement), so it
 * persists in localStorage; "false" disables it, every other value (including
 * a missing entry) keeps the historical default of enabled.
 */

export type TerminalRightClickAction = "paste" | "menu";

export function sanitizeSelectCopyEnabled(raw: string | null): boolean {
  return raw !== "false";
}

/**
 * Right-click semantics with the copy-on-select mode enabled: a plain
 * right-click pastes straight from the clipboard (XShell/SecureCRT style),
 * while Shift+right-click keeps the full context menu reachable.
 */
export function resolveTerminalRightClickAction(options: { selectCopy: boolean; shiftKey: boolean }): TerminalRightClickAction {
  return options.selectCopy && !options.shiftKey ? "paste" : "menu";
}

export type TerminalKeyAction = "copy" | "paste" | "none";

/**
 * Keyboard shortcut routing inside the terminal (Windows Terminal/iTerm2
 * style): Ctrl/Cmd+V and Ctrl/Cmd+Shift+V paste, Ctrl/Cmd+C copies when a
 * selection exists and otherwise stays untouched so it keeps reaching the
 * remote shell as SIGINT.
 */
export function resolveTerminalKeyAction(options: { mod: boolean; shiftKey: boolean; key: string; hasSelection: boolean }): TerminalKeyAction {
  const key = options.key.toLowerCase();
  if (options.mod && key === "v") return "paste";
  if (options.mod && key === "c" && options.hasSelection) return "copy";
  return "none";
}

/** Whether the browser runs on Apple hardware（Cmd 是主修饰键，electerm 同判定）。 */
export function isApplePlatform(userAgent: string = navigator.userAgent): boolean {
  return /mac/i.test(userAgent);
}

/**
 * 全选快捷键判定（electerm/iTerm2 同款）：Apple 平台 Cmd+A 直选，其余平台
 * Ctrl+Shift+A。裸 Ctrl+A 永不命中——必须继续发给 readline 当"跳行首"。
 */
export function isTerminalSelectAllShortcut(options: { mod: boolean; shiftKey: boolean; metaKey: boolean; key: string; applePlatform: boolean }): boolean {
  const key = options.key.toLowerCase();
  if (key !== "a") return false;
  if (options.mod && options.shiftKey) return true;
  return options.applePlatform && options.metaKey;
}

export interface TerminalSearchOptions {
  caseSensitive: boolean;
  regex: boolean;
  wholeWord: boolean;
}

export const TERMINAL_SEARCH_OPTIONS_KEY = "ssh-terminal-search-options";
const TERMINAL_SEARCH_SEED_MAX_LENGTH = 200;

/**
 * Search toggle persistence: the stored shape is a JSON object; anything
 * malformed (or a missing entry) falls back to the all-off defaults instead
 * of throwing or leaking stale partial state.
 */
export function sanitizeSearchOptions(raw: string | null): TerminalSearchOptions {
  let parsed: unknown;
  try {
    parsed = raw == null ? undefined : JSON.parse(raw);
  } catch {
    parsed = undefined;
  }
  const source = (parsed && typeof parsed === "object" ? parsed : {}) as Record<string, unknown>;
  return {
    caseSensitive: source.caseSensitive === true,
    regex: source.regex === true,
    wholeWord: source.wholeWord === true,
  };
}

export function persistSearchOptions(options: TerminalSearchOptions): void {
  try {
    window.localStorage.setItem(TERMINAL_SEARCH_OPTIONS_KEY, JSON.stringify(options));
  } catch {
    // localStorage unavailable: the toggles stay session-scoped.
  }
}

/**
 * Search seed from the current terminal selection (iTerm2 "find selected
 * text"): only the first line is kept and clamped to a bounded length, so a
 * huge or multiline selection cannot turn into an unusable query.
 */
export function terminalSearchSeedFromSelection(selection: string): string {
  const firstLine = selection.split(/\r?\n/, 1)[0] ?? "";
  return firstLine.slice(0, TERMINAL_SEARCH_SEED_MAX_LENGTH);
}

/**
 * Base drop gate shared by every upload drop channel (terminal pane, SFTP
 * pane, host-level drop): needs an active writable session. Callers show a
 * refusal notice instead of dropping silently when this fails.
 */
export function canAcceptFileDrop(options: { connected: boolean; canWrite: boolean }): boolean {
  return options.connected && options.canWrite;
}

/**
 * Whether a file dropped onto the terminal pane can be uploaded right now.
 * The writable-session gate plus a file-transfer occupancy check: a running
 * protocol (ZMODEM or trzsz) owns the terminal data path so drops are
 * refused while one is busy.
 */
export function canAcceptTerminalDrop(options: { connected: boolean; canWrite: boolean; transferBusy: boolean }): boolean {
  return canAcceptFileDrop(options) && !options.transferBusy;
}

/**
 * Target directory typed into the terminal drop prompt: whitespace is
 * trimmed, trailing slashes collapse (the bare root "/" stays intact), and
 * an empty result means the input is unusable so the caller can keep the
 * confirm button disabled. The sidecar's normalize_remote_path is the final
 * authority — this only shapes the input before joinRemote().
 */
export function normalizeDropTargetDir(raw: string): string | null {
  const trimmed = raw.trim();
  if (!trimmed) return null;
  if (!trimmed.startsWith("/")) return trimmed;
  const collapsed = trimmed.replace(/\/+$/, "");
  return collapsed || "/";
}
