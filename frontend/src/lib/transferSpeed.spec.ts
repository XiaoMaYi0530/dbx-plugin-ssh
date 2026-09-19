import { describe, expect, it } from "vitest";
import { sampleTransferSpeed, type TransferSpeedSample } from "./transferSpeed";

function sample(overrides: Partial<TransferSpeedSample> = {}): TransferSpeedSample {
  return { transferred: 1000, windowStartedAt: 0, windowStartedTransferred: 0, speed: 0, ...overrides };
}

describe("sampleTransferSpeed", () => {
  it("starts a fresh window with zero speed", () => {
    expect(sampleTransferSpeed(undefined, 500, 10)).toEqual({ transferred: 500, windowStartedAt: 10, windowStartedTransferred: 500, speed: 0 });
  });

  it("keeps the previous window before the sampling interval elapsed", () => {
    const previous = sample({ speed: 2048 });
    const next = sampleTransferSpeed(previous, 1500, 500);
    expect(next.transferred).toBe(1500);
    expect(next.speed).toBe(2048);
    expect(next.windowStartedAt).toBe(0);
  });

  it("computes bytes per second across the window", () => {
    const previous = sample();
    const next = sampleTransferSpeed(previous, 3000, 1500);
    // 3000 字节 / 1.5s = 2000 B/s（新窗口瞬时值，无历史速度时直接采纳）。
    expect(next.speed).toBe(2000);
    expect(next.windowStartedAt).toBe(1500);
    expect(next.windowStartedTransferred).toBe(3000);
  });

  it("smooths the instantaneous rate with the previous speed", () => {
    const previous = sample({ speed: 1000 });
    const next = sampleTransferSpeed(previous, 5000, 1000);
    // 瞬时 5000 B/s 与历史 1000 B/s 各占一半。
    expect(next.speed).toBe(3000);
  });

  it("restarts the window when the counter regresses instead of dividing by a negative span", () => {
    const previous = sample({ speed: 9000 });
    const next = sampleTransferSpeed(previous, 400, 5000);
    expect(next).toEqual({ transferred: 400, windowStartedAt: 5000, windowStartedTransferred: 400, speed: 0 });
  });

  it("restarts the window on a non-monotonic clock", () => {
    const previous = sample({ windowStartedAt: 2000, speed: 9000 });
    const next = sampleTransferSpeed(previous, 400, 1000);
    expect(next.speed).toBe(0);
    expect(next.windowStartedAt).toBe(1000);
  });
});
