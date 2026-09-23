import { canAcceptTerminalDrop } from "./terminalInteraction";

/**
 * 桌面宿主把 OS 级拖放以已打开的句柄推给插件（bridge filedrop）。落点按
 * 工作台面板状态分流：SFTP 面板打开 → 上传到当前目录；终端独占 → 走落点
 * 询问；连接不可写或没有文件时忽略，避免产生用户未预期的上传任务。
 */
export type HostFileDropTarget =
  | { kind: "sftp" }
  | { kind: "terminal" }
  | { kind: "ignore" };

export function planHostFileDrop(input: {
  files: number;
  connected: boolean;
  canWrite: boolean;
  sftpPaneOpen: boolean;
  terminalTransferBusy: boolean;
}): HostFileDropTarget {
  if (!input.files || !input.connected || !input.canWrite) return { kind: "ignore" };
  if (input.sftpPaneOpen) return { kind: "sftp" };
  // 本入口已按 sftpPaneOpen 分流（上一行），到达此处的拖拽必属终端独占场景。
  if (canAcceptTerminalDrop({ connected: input.connected, canWrite: input.canWrite, transferBusy: input.terminalTransferBusy, sftpPaneOpen: false })) {
    return { kind: "terminal" };
  }
  return { kind: "ignore" };
}
