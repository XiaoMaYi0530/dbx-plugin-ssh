import { describe, expect, it } from "vitest";
import { filterQuickCommands, quickCommandText } from "./quickCommands";

const LIST = [
  { id: "1", name: "日志", command: "tail -f /var/log/nginx.log" },
  { id: "2", name: "Docker PS", command: "docker ps --format wide" },
  { id: "3", name: "端口", command: "netstat -tlnp | grep :8888" },
];

describe("filterQuickCommands", () => {
  it("returns everything for an empty/blank query", () => {
    expect(filterQuickCommands(LIST, "")).toHaveLength(3);
    expect(filterQuickCommands(LIST, "   ")).toHaveLength(3);
  });

  it("matches name and command case-insensitively", () => {
    expect(filterQuickCommands(LIST, "docker")).toHaveLength(1);
    expect(filterQuickCommands(LIST, "日志")[0]?.id).toBe("1");
    expect(filterQuickCommands(LIST, "8888")[0]?.id).toBe("3");
  });

  it("returns [] when nothing matches", () => {
    expect(filterQuickCommands(LIST, "no-such-thing")).toEqual([]);
  });

  it("tolerates an empty list", () => {
    expect(filterQuickCommands([], "x")).toEqual([]);
  });
});

describe("quickCommandText", () => {
  it("flattens multi-line commands with single-space joins and trims", () => {
    expect(quickCommandText("docker ps\n")).toBe("docker ps");
    expect(quickCommandText("systemctl status nginx\r\n")).toBe("systemctl status nginx");
    expect(quickCommandText("  echo a\necho b  ")).toBe("echo a echo b");
  });
  it("returns empty string for blank input", () => {
    expect(quickCommandText(" \n ")).toBe("");
  });
});
