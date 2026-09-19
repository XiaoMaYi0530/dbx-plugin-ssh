/**
 * 远端路径输入规范化（UI_SCAN R3-P2-4）：路径栏提交前的统一入口。
 * - 基础归一：trim、decodeURIComponent、前导 `/`、重复 `/` 折叠、尾 `/` 去除；
 * - `.` / `..` 段消解（`/home/demo/../etc` → `/home/etc`，根上多余的 `..`
 *   收敛为 `/`），下游 joinRemote/exists 拼接与路径历史不再携带未规范路径；
 * - `~` 展开：仅在调用方提供 home 时生效（SSH 用户肌肉记忆；home 探测失败
 *   时保持原样交由后端报错，不静默改写）。
 */
export function resolveRemotePath(path: string, home?: string): string {
  let value = (path ?? "").trim();
  if (home && (value === "~" || value.startsWith("~/"))) {
    value = `${home.replace(/\/+$/, "")}${value.slice(1) || ""}`;
  }
  try {
    value = decodeURIComponent(value);
  } catch {
    // 含未闭合 %-序列等非法输入时保留原文，交由后端报错。
  }
  if (!value.startsWith("/")) value = `/${value}`;
  const resolved: string[] = [];
  for (const segment of value.split("/")) {
    if (!segment || segment === ".") continue;
    if (segment === "..") {
      resolved.pop();
      continue;
    }
    resolved.push(segment);
  }
  return `/${resolved.join("/")}`.replace(/\/{2,}/g, "/");
}

/** 分段回跳（issue #54）用的路径分段：name 为展示文本，path 为可跳转的规范绝对路径。 */
export interface RemotePathSegment {
  name: string;
  path: string;
}

/**
 * 把远端路径切成根 + 逐级前缀的分段（issue #54）：先经 resolveRemotePath 归一，
 * 根目录只返回单段（此时没有可回跳的前缀）；深路径返回 `["/", "aa", "bb", …]`
 * 形态，每段携带可直接交给 loadDirectory 的绝对路径，供路径栏渲染可点击 chip。
 */
export function splitRemotePathSegments(path: string): RemotePathSegment[] {
  const normalized = resolveRemotePath(path);
  if (normalized === "/") return [{ name: "/", path: "/" }];
  const parts = normalized.slice(1).split("/");
  return [
    { name: "/", path: "/" },
    ...parts.map((name, index) => ({
      name,
      path: `/${parts.slice(0, index + 1).join("/")}`,
    })),
  ];
}
