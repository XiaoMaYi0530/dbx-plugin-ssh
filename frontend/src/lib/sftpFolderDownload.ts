// Pure shaping for recursive folder downloads (`sftp/download/tree/start` +
// the shared chunk pipeline). App.vue owns the invoke loop; this module turns
// the finish payload into the outcome the UI communicates: where the tree
// landed, how many files failed and which ones to name in the notice.

export interface FolderDownloadFinish {
  taskId?: string;
  localPath?: string;
  fileCount?: number;
  failedCount?: number;
  skippedCount?: number;
  failedFiles?: Array<{ path?: string; error?: string }>;
}

export interface FolderDownloadOutcome {
  /** Files the tree contained at scan time (failures included). */
  fileCount: number;
  /** Files that could not be downloaded (recorded, tree still completed). */
  failedCount: number;
  /** Symlinks / special entries skipped during the walk. */
  skippedCount: number;
  /** Local root folder, empty when the sidecar did not report one. */
  localPath: string;
  /** First failing paths for inline display, "..." marks the truncation. */
  failureSample: string;
  /** True when at least one file failed but the tree itself completed. */
  partial: boolean;
}

const SAMPLE_LIMIT = 3;

function count(value: unknown): number {
  const parsed = Number(value);
  return Number.isFinite(parsed) && parsed > 0 ? Math.floor(parsed) : 0;
}

function pathOf(entry: { path?: string } | undefined): string {
  const path = String(entry?.path || "").trim();
  // Only the tail matters in a one-line notice.
  return path.split("/").filter(Boolean).pop() || path;
}

/** Normalizes a `sftp/download/finish` result (tolerates legacy/absent payloads). */
export function folderDownloadOutcome(finish: FolderDownloadFinish | null | undefined): FolderDownloadOutcome {
  const failedFiles = Array.isArray(finish?.failedFiles) ? finish!.failedFiles : [];
  const failedCount = count(finish?.failedCount) || failedFiles.length;
  const sampleNames = failedFiles.map(pathOf).filter(Boolean).slice(0, SAMPLE_LIMIT);
  if (failedCount > sampleNames.length) sampleNames.push("…");
  return {
    fileCount: count(finish?.fileCount),
    failedCount,
    skippedCount: count(finish?.skippedCount),
    localPath: String(finish?.localPath || "").trim(),
    failureSample: sampleNames.join(", "),
    partial: failedCount > 0,
  };
}
