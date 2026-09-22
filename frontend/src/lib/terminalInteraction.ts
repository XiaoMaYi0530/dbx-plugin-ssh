/**
 * Terminal interaction gates and search persistence.
 *
 * Key routing and right-click behavior used to live here as ad-hoc resolvers;
 * they now come from the user-editable settings in `terminalBehavior.ts` and
 * `terminalHotkeys.ts`, so this module keeps only the pieces that are not
 * configurable: the drop gates and the search-option/seed helpers.
 */

/** Whether the browser runs on Apple hardware（Cmd 是主修饰键，electerm 同判定）。 */
export function isApplePlatform(userAgent: string = navigator.userAgent): boolean {
  return /mac/i.test(userAgent);
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
