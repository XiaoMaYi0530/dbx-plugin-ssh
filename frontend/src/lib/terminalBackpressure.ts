/**
 * Large-output backpressure gate (IMPL_PLAN Task P2-7).
 *
 * When the terminal renderer falls behind the PTY (a huge `cat`, a build
 * log), the pending-byte backlog keeps growing and secondary per-row work
 * (gutter recompute, keyword-highlight / action-link scans) amplifies the
 * strain. This helper owns the small state machine around that condition:
 *
 * - `feed(pendingBytes)` flips between `normal` and `strained` with a
 *   hysteresis band (engage at `strainedBytes`, release only below
 *   `resumeBytes`) so a boundary-sitting backlog cannot flap the mode.
 * - while strained, `write(data)` splits the payload into `chunkBytes`
 *   frames so xterm parses (and reports completion for) bounded slices
 *   instead of one huge write.
 *
 * Pure decision helpers are exported separately so the spec can test the
 * thresholds without constructing the closure.
 */

export type OutputGateMode = "normal" | "strained";

export interface OutputGateThresholds {
  /** Pending bytes at or above this engage the strained mode (default 128 KiB). */
  strainedBytes: number;
  /** Pending bytes below this release the strained mode (default 64 KiB). */
  resumeBytes: number;
  /** Frame size used to split writes while strained (default 32 KiB). */
  chunkBytes: number;
}

export interface OutputGateOptions {
  strainedBytes?: number;
  resumeBytes?: number;
  chunkBytes?: number;
}

export interface OutputGate {
  readonly mode: OutputGateMode;
  /**
   * Re-evaluate the mode against the current pending-byte backlog.
   * Returns true when the mode flipped, so the caller can toggle the
   * suspended scans / surface a toast exactly once per transition.
   */
  feed(pendingBytes: number): boolean;
  /**
   * Frame `data` for the renderer: a single pass-through frame in normal
   * mode, `chunkBytes`-bounded slices (order preserved) while strained.
   */
  write(data: Uint8Array): Uint8Array[];
  /** Back to normal with a zero backlog (terminal recreation / session end). */
  reset(): void;
}

export const DEFAULT_STRAINED_BYTES = 128 * 1024;
export const DEFAULT_RESUME_BYTES = 64 * 1024;
export const DEFAULT_CHUNK_BYTES = 32 * 1024;

/** Coerce caller-supplied thresholds; invalid or degenerate values fall back to defaults. */
export function resolveOutputGateThresholds(options: OutputGateOptions = {}): OutputGateThresholds {
  const positive = (value: number | undefined, fallback: number) =>
    typeof value === "number" && Number.isFinite(value) && value > 0 ? value : fallback;
  const strainedBytes = positive(options.strainedBytes, DEFAULT_STRAINED_BYTES);
  // Hysteresis requires resume <= strained; equality degrades to a plain
  // threshold instead of misbehaving.
  const resumeBytes = Math.min(positive(options.resumeBytes, DEFAULT_RESUME_BYTES), strainedBytes);
  const chunkBytes = positive(options.chunkBytes, DEFAULT_CHUNK_BYTES);
  return { strainedBytes, resumeBytes, chunkBytes };
}

/**
 * Hysteresis: engage at `pendingBytes >= strainedBytes`, release only once the
 * backlog drops strictly below `resumeBytes`. Anything in between keeps the
 * current mode.
 */
export function resolveOutputMode(
  current: OutputGateMode,
  pendingBytes: number,
  thresholds: Pick<OutputGateThresholds, "strainedBytes" | "resumeBytes">,
): OutputGateMode {
  const pending = Number.isFinite(pendingBytes) && pendingBytes > 0 ? pendingBytes : 0;
  if (current === "normal") return pending >= thresholds.strainedBytes ? "strained" : "normal";
  return pending < thresholds.resumeBytes ? "normal" : "strained";
}

/** Split `data` into ordered `chunkBytes` frames (view into the same buffer, no copies). */
export function chunkOutput(data: Uint8Array, chunkBytes: number): Uint8Array[] {
  if (data.byteLength <= chunkBytes) return [data];
  const frames: Uint8Array[] = [];
  for (let offset = 0; offset < data.byteLength; offset += chunkBytes) {
    frames.push(data.subarray(offset, offset + chunkBytes));
  }
  return frames;
}

export function createOutputGate(options: OutputGateOptions = {}): OutputGate {
  const thresholds = resolveOutputGateThresholds(options);
  let mode: OutputGateMode = "normal";
  return {
    get mode() {
      return mode;
    },
    feed(pendingBytes: number): boolean {
      const next = resolveOutputMode(mode, pendingBytes, thresholds);
      if (next === mode) return false;
      mode = next;
      return true;
    },
    write(data: Uint8Array): Uint8Array[] {
      if (mode === "strained") return chunkOutput(data, thresholds.chunkBytes);
      return [data];
    },
    reset() {
      mode = "normal";
    },
  };
}
