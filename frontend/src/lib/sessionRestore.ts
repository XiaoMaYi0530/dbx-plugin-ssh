/**
 * Workbench remount session discovery.
 *
 * The host reopens a plugin workbench by destroying and recreating its
 * webview, without round-tripping `workbenchState` (the bridge does not
 * implement it) and with a freshly minted `workbenchId` on every sidebar
 * reopen. The persisted-sessionId attach path therefore never engages, and
 * the frontend must ask the sidecar for the connection's live session and
 * reattach to it instead of dialing a redundant SSH connection.
 */

export interface SessionSummary {
  sessionId: string;
  connectionId?: string;
  workbenchId?: string;
  connected?: boolean;
  /** Unix seconds; the sidecar lists sessions oldest-first. */
  createdAt?: number;
}

/**
 * Picks the sidecar session a remounting workbench should attach to: live
 * sessions of the same connection, preferring the same workbench id, newest
 * first (`createdAt` DESC, then list order). Returns "" when nothing lives —
 * the caller falls back to a fresh `ssh/session/open`.
 */
export function pickLiveSessionForReattach(
  sessions: SessionSummary[] | undefined,
  options: { connectionId: string; workbenchId: string },
): string {
  const connectionId = String(options.connectionId || "");
  if (!connectionId) return "";
  const candidates = (Array.isArray(sessions) ? sessions : []).filter(
    (session) =>
      session &&
      typeof session.sessionId === "string" &&
      session.sessionId &&
      session.connectionId === connectionId &&
      session.connected !== false,
  );
  if (candidates.length === 0) return "";
  const sameWorkbench =
    options.workbenchId && candidates.some((session) => session.workbenchId === options.workbenchId)
      ? candidates.filter((session) => session.workbenchId === options.workbenchId)
      : candidates;
  const newest = sameWorkbench.reduce((best, session) =>
    (session.createdAt ?? 0) >= (best.createdAt ?? 0) ? session : best,
  );
  return newest.sessionId;
}
