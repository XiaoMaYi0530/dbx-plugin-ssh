import type { Terminal } from "@xterm/xterm";

/**
 * xterm.js still drops direct text commits on macOS when WKWebView/Safari (or
 * an IME in English mode) delivers `input` before a keyCode=229 `keydown`.
 * See xterm.js #5887, #6045 and #6144. This adapter owns only single-byte
 * printable text outside a real composition; every other input stays on
 * xterm's normal path.
 */

export type WebkitKeydownDecision = "pass" | "suppress";

export interface WebkitKeydownSample {
  key: string;
  keyCode: number;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  isComposing: boolean;
  at: number;
}

export interface WebkitInputSample {
  data: string | null;
  inputType: string;
  isComposing: boolean;
  at: number;
}

export interface WebkitInputController {
  keydown(sample: WebkitKeydownSample): WebkitKeydownDecision;
  input(sample: WebkitInputSample): string | undefined;
  compositionStart(): void;
  compositionEnd(at: number): void;
}

const NORMAL_KEYDOWN_WINDOW_MS = 150;
const COMPOSITION_SETTLE_MS = 5;

function directText(value: string | null): string | undefined {
  if (!value) return undefined;
  const normalized = value.replace(/\u00a0/g, " ");
  return /^[\x20-\x7e]$/.test(normalized) ? normalized : undefined;
}

function directKey(sample: WebkitKeydownSample): string | undefined {
  if (sample.ctrlKey || sample.metaKey || sample.altKey || sample.isComposing) return undefined;
  return directText(sample.key);
}

export function createWebkitInputController(): WebkitInputController {
  let composing = false;
  let compositionEndedAt = Number.NEGATIVE_INFINITY;
  let imeKeydownBeforeInput = false;
  let imeKeydownAfterInputExpected = false;
  let normalKeydown: { key: string; at: number } | undefined;

  function clearPendingKeys() {
    imeKeydownBeforeInput = false;
    imeKeydownAfterInputExpected = false;
    normalKeydown = undefined;
  }

  return {
    keydown(sample) {
      const key = directKey(sample);
      if (!key || composing) return "pass";

      if (sample.keyCode === 229) {
        // Do not let xterm arm _keyDownSeen or start its deferred textarea
        // diff. The matching insertText is emitted exactly once by input().
        if (imeKeydownAfterInputExpected) imeKeydownAfterInputExpected = false;
        else imeKeydownBeforeInput = true;
        normalKeydown = undefined;
        return "suppress";
      }

      // A normal keyboard event reaches xterm first and xterm emits it from
      // keydown. Remember it so the following DOM input event is not emitted
      // a second time by this fallback.
      imeKeydownAfterInputExpected = false;
      normalKeydown = { key, at: sample.at };
      return "pass";
    },

    input(sample) {
      const data = sample.inputType === "insertText" && !sample.isComposing ? directText(sample.data) : undefined;
      if (!data || composing || sample.at - compositionEndedAt <= COMPOSITION_SETTLE_MS) return undefined;

      if (imeKeydownBeforeInput) {
        imeKeydownBeforeInput = false;
        normalKeydown = undefined;
        return data;
      }

      if (normalKeydown && sample.at - normalKeydown.at <= NORMAL_KEYDOWN_WINDOW_MS && normalKeydown.key === data) {
        normalKeydown = undefined;
        return undefined;
      }

      // WKWebView/IME order: input arrives before its keyCode=229 keydown.
      // Emit now; the later keydown is suppressed by the branch above.
      normalKeydown = undefined;
      imeKeydownAfterInputExpected = true;
      return data;
    },

    compositionStart() {
      composing = true;
      clearPendingKeys();
    },

    compositionEnd(at) {
      composing = false;
      compositionEndedAt = at;
      clearPendingKeys();
    },
  };
}

export function isMacWebkitInputFallbackRequired(userAgent: string = navigator.userAgent): boolean {
  return /Macintosh|Mac OS X/i.test(userAgent);
}

export function installMacWebkitInputFallback(options: {
  terminal: Terminal;
  onData(data: string): void;
  userAgent?: string;
}): (() => void) | undefined {
  if (!isMacWebkitInputFallbackRequired(options.userAgent)) return undefined;
  if (options.terminal.options.screenReaderMode) return undefined;
  const element = options.terminal.element;
  const textarea = options.terminal.textarea;
  if (!element || !textarea) return undefined;

  const controller = createWebkitInputController();
  const targetsTextarea = (event: Event) => event.target === textarea;

  const onKeydown = (event: KeyboardEvent) => {
    if (!targetsTextarea(event)) return;
    if (options.terminal.options.screenReaderMode) return;
    const decision = controller.keydown({
      key: event.key,
      keyCode: event.keyCode,
      ctrlKey: event.ctrlKey,
      metaKey: event.metaKey,
      altKey: event.altKey,
      isComposing: event.isComposing,
      at: event.timeStamp,
    });
    if (decision === "suppress") event.stopImmediatePropagation();
  };

  const onInput = (event: InputEvent) => {
    if (!targetsTextarea(event)) return;
    if (options.terminal.options.screenReaderMode) return;
    const data = controller.input({
      data: event.data,
      inputType: event.inputType,
      isComposing: event.isComposing,
      at: event.timeStamp,
    });
    if (data === undefined) return;
    event.stopImmediatePropagation();
    // Stale direct commits would otherwise be rediscovered by a later
    // keyCode=229 textarea diff. Screen-reader mode skips this adapter above.
    textarea.value = "";
    options.onData(data);
  };

  const onCompositionStart = (event: CompositionEvent) => {
    if (targetsTextarea(event)) controller.compositionStart();
  };
  const onCompositionEnd = (event: CompositionEvent) => {
    if (targetsTextarea(event)) controller.compositionEnd(event.timeStamp);
  };

  // The listeners live on xterm's ancestor in capture phase, so they run
  // before xterm's capture listeners on the hidden textarea.
  element.addEventListener("keydown", onKeydown, true);
  element.addEventListener("input", onInput as EventListener, true);
  element.addEventListener("compositionstart", onCompositionStart as EventListener, true);
  element.addEventListener("compositionend", onCompositionEnd as EventListener, true);

  return () => {
    element.removeEventListener("keydown", onKeydown, true);
    element.removeEventListener("input", onInput as EventListener, true);
    element.removeEventListener("compositionstart", onCompositionStart as EventListener, true);
    element.removeEventListener("compositionend", onCompositionEnd as EventListener, true);
  };
}
