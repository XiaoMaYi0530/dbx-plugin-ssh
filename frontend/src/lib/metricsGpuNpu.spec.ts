import { describe, expect, it } from "vitest";
import {
  acceleratorSectionsPresent,
  memoryPercent,
  type GpuOverviewView,
  type NpuOverviewView,
} from "./metricsGpuNpu";

describe("memoryPercent", () => {
  it("computes a clamped percentage from used and total bytes", () => {
    expect(memoryPercent(2500, 10000)).toBe(25);
    expect(memoryPercent(0, 10000)).toBe(0);
    // 超用与负值都夹紧到 0-100。
    expect(memoryPercent(20000, 10000)).toBe(100);
    expect(memoryPercent(-5, 10000)).toBe(0);
  });

  it("returns null when the total is missing or non-positive", () => {
    expect(memoryPercent(100, 0)).toBeNull();
    expect(memoryPercent(100, null)).toBeNull();
    expect(memoryPercent(100, undefined)).toBeNull();
  });
});

describe("acceleratorSectionsPresent", () => {
  const gpu: GpuOverviewView = { available: false, gpus: [] };
  const npu: NpuOverviewView = { available: true, cann: null, devices: [] };

  it("hides the whole section for sidecars that report neither key", () => {
    expect(acceleratorSectionsPresent(undefined, undefined)).toBe(false);
  });

  it("shows the section as soon as either key exists, even when unavailable", () => {
    expect(acceleratorSectionsPresent(gpu, undefined)).toBe(true);
    expect(acceleratorSectionsPresent(undefined, npu)).toBe(true);
    expect(acceleratorSectionsPresent(gpu, npu)).toBe(true);
  });
});
