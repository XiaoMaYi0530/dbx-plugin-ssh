import { describe, expect, it } from "vitest";
import { planHostFileDrop } from "./hostFileDrop";

const writable = { files: 1, connected: true, canWrite: true, sftpPaneOpen: true, terminalTransferBusy: false };

describe("planHostFileDrop", () => {
  it("uploads to the sftp directory when the file panel is open", () => {
    expect(planHostFileDrop(writable)).toEqual({ kind: "sftp" });
  });

  it("asks for a terminal landing directory when the sftp panel is closed", () => {
    expect(planHostFileDrop({ ...writable, sftpPaneOpen: false })).toEqual({ kind: "terminal" });
  });

  it("ignores drops while a terminal transfer is running", () => {
    expect(planHostFileDrop({ ...writable, sftpPaneOpen: false, terminalTransferBusy: true })).toEqual({ kind: "ignore" });
  });

  it("ignores drops without a writable connection", () => {
    expect(planHostFileDrop({ ...writable, connected: false })).toEqual({ kind: "ignore" });
    expect(planHostFileDrop({ ...writable, canWrite: false })).toEqual({ kind: "ignore" });
  });

  it("ignores empty drop payloads", () => {
    expect(planHostFileDrop({ ...writable, files: 0 })).toEqual({ kind: "ignore" });
  });
});
