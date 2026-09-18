/**
 * Bounded retry decision for ssh/session/open failures (P1-1).
 *
 * openSession() re-enters itself through the retry timer; the attempt counter
 * must survive those re-entries (only fresh, user-driven entry points reset
 * it), otherwise OPEN_RETRY_MAX never saturates and fast permanent failures
 * (auth / host-key rejections) retry forever with no error surface.
 * The decision itself is a pure function so the retry ladder, the fast-fail
 * window and the permanent-error short circuit stay unit-testable.
 */

import { classifyConnectError } from "./connectError";

export type ConnectRetryDecision = { kind: "retry"; attempt: number; delayMs: number } | { kind: "fail" };

/** Retry backoff base: attempt N waits 2s * N (1st retry 2s, 2nd 4s, 3rd 6s). */
export const OPEN_RETRY_BASE_DELAY_MS = 2000;

/**
 * A single attempt that already ran this long is a real dial (tens of
 * seconds), not a boot-restore race — retrying it just stacks one error
 * under minutes of spinner, so it fails instead of retrying.
 */
export const OPEN_RETRY_FAST_FAIL_WINDOW_MS = 8000;

/**
 * Bounded poll window for manual entry points that hit "Connection is not
 * active": the host only replays connection/connect into a live sidecar when
 * the user reopens the connection from the DBX sidebar, so the plugin retries
 * for this many rounds at a fixed cadence, giving the user a chance to reopen
 * it and letting the attempt self-heal.
 */
export const INACTIVE_RETRY_MAX = 10;
export const INACTIVE_RETRY_DELAY_MS = 3000;

/**
 * Permanent connect-error kinds. Auth and host-key rejections fail within
 * milliseconds and can never self-heal by retrying — the user must fix the
 * credential or the known-hosts entry first — so they skip the retry ladder
 * entirely and surface the friendly message plus the Reconnect button.
 */
export function isPermanentConnectError(cause: unknown): boolean {
  const message = cause instanceof Error ? cause.message : String(cause ?? "");
  const kind = classifyConnectError(message);
  return kind === "auth" || kind === "hostKey";
}

/**
 * Decides what openSession()'s catch block should do after a failed attempt.
 * `attempt` is the number of retries already consumed (0 = first attempt).
 */
export function decideConnectRetry(options: {
  cause: unknown;
  attempt: number;
  maxAttempts: number;
  /** Wall time the failed attempt took. */
  attemptMs: number;
  /** The sidecar reported "Connection is not active" (registry race). */
  inactive: boolean;
  bootRestore: boolean;
}): ConnectRetryDecision {
  // "Connection is not active" means the sidecar lost the connection registry
  // (e.g. sidecar restart); the host only refills it when the user reopens the
  // connection from the DBX sidebar. Manual entry points (reconnect button
  // etc.) poll for a bounded window so a reopen self-heals, then fail with
  // guidance. Boot restores keep the regular ladder because the host replays
  // the connect lifecycle on its own.
  if (options.inactive && !options.bootRestore) {
    if (options.attempt >= INACTIVE_RETRY_MAX) return { kind: "fail" };
    return { kind: "retry", attempt: options.attempt + 1, delayMs: INACTIVE_RETRY_DELAY_MS };
  }
  if (isPermanentConnectError(options.cause)) return { kind: "fail" };
  if (options.attempt >= options.maxAttempts) return { kind: "fail" };
  if (options.attemptMs >= OPEN_RETRY_FAST_FAIL_WINDOW_MS) return { kind: "fail" };
  const attempt = options.attempt + 1;
  return { kind: "retry", attempt, delayMs: OPEN_RETRY_BASE_DELAY_MS * attempt };
}
