import { describe, expect, it } from "vitest";
import { distroBadge } from "./distroBadge";

describe("distro badge mapping", () => {
  it("maps every known osId to a label and a color", () => {
    const known = ["ubuntu", "debian", "centos", "rhel", "fedora", "alpine", "arch", "rocky", "almalinux", "opensuse", "oracle", "amazon", "kali", "suse"];
    for (const osId of known) {
      const badge = distroBadge(osId);
      expect(badge, osId).not.toBeNull();
      expect(badge!.label).toHaveLength(1);
      expect(badge!.color).toMatch(/^#[0-9a-fA-F]{6}$/);
      expect(badge!.name).toBeTruthy();
    }
  });

  it("keeps centos and rhel visually distinct", () => {
    expect(distroBadge("centos")!.color).not.toBe(distroBadge("rhel")!.color);
  });

  it("matches osId case-insensitively and trims whitespace", () => {
    expect(distroBadge(" Ubuntu ")!.label).toBe(distroBadge("ubuntu")!.label);
  });

  it("prefers the osPretty text for the tooltip name", () => {
    expect(distroBadge("ubuntu", "Ubuntu 22.04.5 LTS")!.name).toBe("Ubuntu 22.04.5 LTS");
    // osPretty 缺省回退映射表标准名（§1.5：两字段成对缺省，但容错处理单缺）。
    expect(distroBadge("ubuntu")!.name).toBe("Ubuntu");
  });

  it("falls back to the generic gray badge with the osPretty text", () => {
    const unknown = distroBadge("netbsd", "NetBSD 10.0");
    expect(unknown).toEqual({ label: "?", color: "#6b7280", name: "NetBSD 10.0" });
    const noOsId = distroBadge(undefined, "Debian GNU/Linux 12");
    expect(noOsId).toEqual({ label: "?", color: "#6b7280", name: "Debian GNU/Linux 12" });
    // Unknown osId without osPretty still degrades to the generic badge.
    expect(distroBadge("plan9")).toEqual({ label: "?", color: "#6b7280", name: "plan9" });
  });

  it("renders nothing when the sidecar omits both fields", () => {
    expect(distroBadge(undefined)).toBeNull();
    expect(distroBadge(undefined, "  ")).toBeNull();
    expect(distroBadge("")).toBeNull();
  });
});
