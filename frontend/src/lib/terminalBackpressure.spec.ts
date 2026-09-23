// 大输出保护状态机（IMPL_PLAN Task P2-7）：迟滞阈值（128KiB 触发 / 64KiB 恢复）、
// strained 分帧写入（32KiB）、reset 复位；非法阈值回落默认值。
import { describe, expect, it } from "vitest";
import {
  chunkOutput,
  createOutputGate,
  DEFAULT_CHUNK_BYTES,
  DEFAULT_RESUME_BYTES,
  DEFAULT_STRAINED_BYTES,
  resolveOutputGateThresholds,
  resolveOutputMode,
} from "./terminalBackpressure";

const KB = 1024;

function bytes(length: number, fill = 0x61): Uint8Array {
  return new Uint8Array(length).fill(fill);
}

describe("resolveOutputMode", () => {
  const thresholds = { strainedBytes: 128 * KB, resumeBytes: 64 * KB };

  it("stays normal below the engage threshold", () => {
    expect(resolveOutputMode("normal", 0, thresholds)).toBe("normal");
    expect(resolveOutputMode("normal", 127 * KB, thresholds)).toBe("normal");
  });

  it("engages strained at or above strainedBytes", () => {
    expect(resolveOutputMode("normal", 128 * KB, thresholds)).toBe("strained");
    expect(resolveOutputMode("normal", 10 * 1024 * KB, thresholds)).toBe("strained");
  });

  it("holds strained inside the hysteresis band", () => {
    expect(resolveOutputMode("strained", 127 * KB, thresholds)).toBe("strained");
    expect(resolveOutputMode("strained", 64 * KB, thresholds)).toBe("strained");
  });

  it("releases strained only below resumeBytes", () => {
    expect(resolveOutputMode("strained", 64 * KB - 1, thresholds)).toBe("normal");
    expect(resolveOutputMode("strained", 0, thresholds)).toBe("normal");
  });

  it("treats non-finite or negative backlogs as zero", () => {
    expect(resolveOutputMode("normal", Number.NaN, thresholds)).toBe("normal");
    expect(resolveOutputMode("normal", -5, thresholds)).toBe("normal");
    expect(resolveOutputMode("strained", Number.NaN, thresholds)).toBe("normal");
  });
});

describe("resolveOutputGateThresholds", () => {
  it("defaults to 128KiB / 64KiB / 32KiB", () => {
    expect(resolveOutputGateThresholds()).toEqual({
      strainedBytes: DEFAULT_STRAINED_BYTES,
      resumeBytes: DEFAULT_RESUME_BYTES,
      chunkBytes: DEFAULT_CHUNK_BYTES,
    });
  });

  it("keeps valid custom values", () => {
    expect(resolveOutputGateThresholds({ strainedBytes: 300 * KB, resumeBytes: 100 * KB, chunkBytes: 8 * KB })).toEqual({
      strainedBytes: 300 * KB,
      resumeBytes: 100 * KB,
      chunkBytes: 8 * KB,
    });
  });

  it("falls back on invalid values and caps resume at strained", () => {
    expect(resolveOutputGateThresholds({ strainedBytes: -1, resumeBytes: Number.NaN, chunkBytes: 0 })).toEqual({
      strainedBytes: DEFAULT_STRAINED_BYTES,
      resumeBytes: DEFAULT_RESUME_BYTES,
      chunkBytes: DEFAULT_CHUNK_BYTES,
    });
    // resume >= strained 会被钳到 strained，迟滞退化为普通阈值但不失真。
    expect(resolveOutputGateThresholds({ strainedBytes: 10 * KB, resumeBytes: 500 * KB }).resumeBytes).toBe(10 * KB);
  });
});

describe("chunkOutput", () => {
  it("passes through payloads at or below the chunk size without copying", () => {
    const data = bytes(32 * KB);
    const frames = chunkOutput(data, 32 * KB);
    expect(frames).toHaveLength(1);
    expect(frames[0]).toBe(data);
  });

  it("splits large payloads into ordered chunkBytes frames with a partial tail", () => {
    const data = bytes(70 * KB, 0x62);
    const frames = chunkOutput(data, 32 * KB);
    expect(frames.map((frame) => frame.byteLength)).toEqual([32 * KB, 32 * KB, 6 * KB]);
    // 帧必须是原缓冲的顺序视图，内容逐字节一致。
    let offset = 0;
    for (const frame of frames) {
      expect(frame[0]).toBe(0x62);
      expect(Array.from(data.subarray(offset, offset + frame.byteLength))).toEqual(Array.from(frame));
      offset += frame.byteLength;
    }
    expect(offset).toBe(70 * KB);
  });

  it("returns a single frame for empty payloads", () => {
    const data = new Uint8Array(0);
    const frames = chunkOutput(data, 32 * KB);
    expect(frames).toHaveLength(1);
    expect(frames[0]).toBe(data);
  });
});

describe("createOutputGate", () => {
  it("starts in normal mode and writes pass through as a single frame", () => {
    const gate = createOutputGate();
    expect(gate.mode).toBe("normal");
    const data = bytes(64 * KB);
    const frames = gate.write(data);
    expect(frames).toEqual([data]);
  });

  it("engages on feed, frames writes while strained, and resumes below the release threshold", () => {
    const gate = createOutputGate();
    expect(gate.feed(200 * KB)).toBe(true);
    expect(gate.mode).toBe("strained");
    // 迟滞带内保持 strained。
    expect(gate.feed(100 * KB)).toBe(false);
    expect(gate.mode).toBe("strained");

    const data = bytes(70 * KB);
    const frames = gate.write(data);
    expect(frames.map((frame) => frame.byteLength)).toEqual([32 * KB, 32 * KB, 6 * KB]);

    // 回调把积压消化到恢复阈值以下后才退出 strained，此后写入恢复直通。
    expect(gate.feed(63 * KB)).toBe(true);
    expect(gate.mode).toBe("normal");
    expect(gate.write(data)).toEqual([data]);
  });

  it("does not re-frame a large write once the backlog recovers mid-flight", () => {
    const gate = createOutputGate();
    gate.feed(128 * KB);
    gate.feed(0);
    expect(gate.mode).toBe("normal");
    expect(gate.write(bytes(96 * KB))).toHaveLength(1);
  });

  it("reset returns to normal mode", () => {
    const gate = createOutputGate();
    gate.feed(999 * KB);
    expect(gate.mode).toBe("strained");
    gate.reset();
    expect(gate.mode).toBe("normal");
    expect(gate.feed(128 * KB)).toBe(true);
  });
});
