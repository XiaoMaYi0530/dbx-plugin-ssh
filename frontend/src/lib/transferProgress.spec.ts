import { describe, expect, it } from "vitest";
import { mergeTransferProgress, transferCancelReason, type TransferProgressState } from "./transferProgress";

function state(overrides: Partial<TransferProgressState> = {}): TransferProgressState {
  return { transferred: 0, staged: 0, size: 1000, ...overrides };
}

describe("mergeTransferProgress", () => {
  it("keeps the push counter untouched while staging", () => {
    const merged = mergeTransferProgress(state(), { transferred: 300, size: 1000, phase: "staging" });
    expect(merged).toEqual({ transferred: 0, staged: 300, size: 1000, phase: "staging" });
    // 无 phase 事件（下载/旧 sidecar）不会被误判成 staging。
    expect(mergeTransferProgress(state(), { transferred: 300, size: 1000 }).phase).toBeUndefined();
  });

  it("allows the counter reset at the staging→uploading boundary", () => {
    let merged = mergeTransferProgress(undefined, { transferred: 500, size: 1000, phase: "staging" });
    merged = mergeTransferProgress(merged, { transferred: 100, size: 1000, phase: "uploading" });
    expect(merged).toEqual({ transferred: 100, staged: 500, size: 1000, phase: "uploading" });
  });

  it("clamps progress monotonic within the uploading phase", () => {
    let merged = mergeTransferProgress(undefined, { transferred: 400, size: 1000, phase: "uploading" });
    merged = mergeTransferProgress(merged, { transferred: 250, size: 1000, phase: "uploading" });
    expect(merged.transferred).toBe(400);
    merged = mergeTransferProgress(merged, { transferred: 900, size: 1000, phase: "uploading" });
    expect(merged.transferred).toBe(900);
  });

  it("drops non-finite and negative counts instead of poisoning the state", () => {
    expect(mergeTransferProgress(undefined, { transferred: Number.NaN, size: 1000, phase: "staging" }).staged).toBe(0);
    expect(mergeTransferProgress(undefined, { transferred: -5, size: 1000, phase: "uploading" }).transferred).toBe(0);
    expect(mergeTransferProgress(state({ size: 1000 }), { transferred: 10, size: "bogus", phase: "staging" }).size).toBe(1000);
  });

  it("marks completed without a phase as fully transferred", () => {
    expect(mergeTransferProgress(state({ transferred: 400 }), { transferred: 400, size: 1000, status: "completed" })).toEqual({ transferred: 1000, staged: 0, size: 1000 });
  });

  it("behaves like the legacy monotonic merge for downloads without phases", () => {
    let merged = mergeTransferProgress(undefined, { transferred: 100, size: 1000 });
    merged = mergeTransferProgress(merged, { transferred: 60, size: 1000 });
    expect(merged.transferred).toBe(100);
    merged = mergeTransferProgress(merged, { transferred: 120, size: 1000, status: "completed" });
    expect(merged.transferred).toBe(1000);
  });
});

describe("transferCancelReason", () => {
  it("maps known error codes to stable slugs", () => {
    expect(transferCancelReason({ code: "transfer-cancelled" })).toBe("user");
    expect(transferCancelReason({ code: "transfer-terminal" })).toBe("user");
    expect(transferCancelReason({ code: "upload-ack-timeout" })).toBe("ack-timeout");
    expect(transferCancelReason({ code: "upload-read-failed" })).toBe("local-read-error");
    expect(transferCancelReason({ code: "upload-append-failed" })).toBe("append-failed");
    expect(transferCancelReason({ code: "upload-start-failed" })).toBe("start-failed");
  });

  it("falls back to client-error for unknown causes", () => {
    expect(transferCancelReason(new Error("boom"))).toBe("client-error");
    expect(transferCancelReason(undefined)).toBe("client-error");
    expect(transferCancelReason({ code: 42 })).toBe("client-error");
  });
});
