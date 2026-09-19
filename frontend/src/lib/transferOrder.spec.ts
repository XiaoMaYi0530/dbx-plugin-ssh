import { describe, expect, it } from "vitest";
import {
  compareTransferTasks,
  isLiveTransferStatus,
  sortTransferTasks,
  type TransferOrderInput,
} from "./transferOrder";

function task(overrides: Partial<TransferOrderInput> = {}): TransferOrderInput {
  return { taskId: "t1", status: "running", ...overrides };
}

const IDS = (tasks: TransferOrderInput[]) => tasks.map((task) => task.taskId);

describe("transfer status classification", () => {
  it("treats only queued/running as live", () => {
    expect(isLiveTransferStatus("queued")).toBe(true);
    expect(isLiveTransferStatus("running")).toBe(true);
    expect(isLiveTransferStatus("completed")).toBe(false);
    expect(isLiveTransferStatus("cancelled")).toBe(false);
    expect(isLiveTransferStatus("failed")).toBe(false);
  });
});

describe("compareTransferTasks (issue #18 stable order)", () => {
  it("orders live tasks first, earliest start on top", () => {
    const list = [
      task({ taskId: "old-done", status: "completed", startedAt: 500 }),
      task({ taskId: "live-2", startedAt: 2_000 }),
      task({ taskId: "live-1", startedAt: 1_000 }),
    ];
    expect(IDS(sortTransferTasks(list))).toEqual(["live-1", "live-2", "old-done"]);
  });

  it("orders terminal tasks newest-start first to align with the history section", () => {
    const list = [
      task({ taskId: "done-1", status: "completed", startedAt: 1_000 }),
      task({ taskId: "done-3", status: "failed", startedAt: 3_000 }),
      task({ taskId: "done-2", status: "cancelled", startedAt: 2_000 }),
    ];
    expect(IDS(sortTransferTasks(list))).toEqual(["done-3", "done-2", "done-1"]);
  });

  it("falls back to joinedAt when a live row has no startedAt", () => {
    const list = [
      task({ taskId: "restored", joinedAt: 5_000 }),
      task({ taskId: "fresh", joinedAt: 4_000 }),
      task({ taskId: "timestamped", startedAt: 4_500 }),
    ];
    expect(IDS(sortTransferTasks(list))).toEqual(["fresh", "timestamped", "restored"]);
  });

  it("rows without any timestamp sort last within their liveness group", () => {
    const list = [
      task({ taskId: "z" }),
      task({ taskId: "a", startedAt: 2_000 }),
      task({ taskId: "y" }),
      task({ taskId: "b", startedAt: 3_000 }),
    ];
    expect(IDS(sortTransferTasks(list))).toEqual(["a", "b", "y", "z"]);
  });

  it("breaks every tie with taskId so any input permutation yields one order", () => {
    const base = [
      task({ taskId: "b", startedAt: 2_000 }),
      task({ taskId: "a", startedAt: 2_000 }),
      task({ taskId: "z" }),
      task({ taskId: "y" }),
      task({ taskId: "c", startedAt: 3_000 }),
    ];
    const expected = IDS(sortTransferTasks(base));
    expect(expected).toEqual(["a", "b", "c", "y", "z"]);
    for (let rotate = 1; rotate < base.length; rotate += 1) {
      const rotated = [...base];
      rotated.push(...rotated.splice(0, rotate));
      expect(IDS(sortTransferTasks(rotated))).toEqual(expected);
    }
  });

  it("sorts without mutating the input", () => {
    const list = [task({ taskId: "b" }), task({ taskId: "a" })];
    sortTransferTasks(list);
    expect(IDS(list)).toEqual(["b", "a"]);
    expect(compareTransferTasks(task({ taskId: "a" }), task({ taskId: "a" }))).toBe(0);
  });
});
