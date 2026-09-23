// 下载冲突策略（设置弹窗与下载询问弹窗共用）：rename（自动重命名，默认）/
// ask（询问我）/ overwrite（覆盖）。存储键与 sanitize 留在 App.vue（耦合
// 下载偏好的内存权威态与 sidecar preferences 同步）。

export type DownloadConflictPolicy = "rename" | "ask" | "overwrite";

export const DOWNLOAD_CONFLICT_POLICIES: readonly DownloadConflictPolicy[] = ["rename", "ask", "overwrite"];
