// folderDownloadOutcome 单测：sftp/download/finish 汇总载荷 → UI 结论的纯
// 整形。覆盖缺省/旧版载荷、失败计数回退、样例截断与文件名提取。
import { describe, expect, it } from "vitest";
import { folderDownloadOutcome } from "./sftpFolderDownload";

describe("folderDownloadOutcome", () => {
  it("tolerates absent or legacy payloads", () => {
    expect(folderDownloadOutcome(undefined)).toEqual({
      fileCount: 0,
      failedCount: 0,
      skippedCount: 0,
      localPath: "",
      failureSample: "",
      partial: false,
    });
    expect(folderDownloadOutcome({ localPath: "/dl/tree" })?.localPath).toBe("/dl/tree");
  });

  it("reports a clean completion", () => {
    const outcome = folderDownloadOutcome({
      localPath: "/dl/site",
      fileCount: 12,
      failedCount: 0,
      skippedCount: 2,
      failedFiles: [],
    });
    expect(outcome).toMatchObject({
      fileCount: 12,
      failedCount: 0,
      skippedCount: 2,
      partial: false,
      failureSample: "",
    });
  });

  it("keeps the full failed count and caps the inline sample at three names", () => {
    const outcome = folderDownloadOutcome({
      localPath: "/dl/site",
      fileCount: 9,
      failedCount: 5,
      failedFiles: [
        { path: "/r/a.txt", error: "denied" },
        { path: "/r/sub/b.bin", error: "denied" },
        { path: "/r/c.log", error: "denied" },
        { path: "/r/d.txt", error: "denied" },
      ],
    });
    expect(outcome.partial).toBe(true);
    expect(outcome.failedCount).toBe(5);
    // Only leaf names show inline; the 5th failure is the "…" marker, not d.txt.
    expect(outcome.failureSample).toBe("a.txt, b.bin, c.log, …");
  });

  it("falls back to the failedFiles length when the count is missing", () => {
    const outcome = folderDownloadOutcome({
      failedFiles: [{ path: "/r/only.txt" }],
    });
    expect(outcome.failedCount).toBe(1);
    expect(outcome.partial).toBe(true);
    expect(outcome.failureSample).toBe("only.txt");
  });

  it("ignores invalid numbers instead of crashing the notice", () => {
    const outcome = folderDownloadOutcome({
      fileCount: Number.NaN,
      failedCount: -3,
      skippedCount: 1.9,
      localPath: "  ",
    });
    expect(outcome.fileCount).toBe(0);
    expect(outcome.failedCount).toBe(0);
    expect(outcome.skippedCount).toBe(1);
    expect(outcome.localPath).toBe("");
  });
});
