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
