export type SshWorkbenchPaneOrder = "terminal-left" | "sftp-left";

export function getSshWorkbenchSplitLayout(order: SshWorkbenchPaneOrder) {
  const reversed = order === "sftp-left";
  return {
    rtl: reversed,
    flexDirection: reversed ? "row-reverse" : "row",
  } as const;
}

/**
 * Resolves the SFTP pane visibility for a restored workbench: the persisted
 * per-workbench flag wins; a fresh workbench falls back to the global
 * "open by default" preference.
 */
export function resolveSftpPaneOpen(state: { sftpPaneOpen?: unknown }, defaultOpen: boolean): boolean {
  return typeof state.sftpPaneOpen === "boolean" ? state.sftpPaneOpen : defaultOpen;
}

/**
 * Parses the persisted global preference ("false" = keep new workbenches on the
 * terminal-only layout; anything else, including missing values, keeps the
 * historical always-open default).
 */
export function sanitizeSftpPaneDefaultOpen(raw: string | null): boolean {
  return raw !== "false";
}
