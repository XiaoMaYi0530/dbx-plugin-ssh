import { describe, expect, it } from "vitest";
import {
  ACTION_LINK_MATCHES_PER_LINE_LIMIT,
  ACTION_LINK_SETTINGS_DEFAULTS,
  matchActionLinks,
  sanitizeActionLinksSettings,
} from "./actionLinksMatcher";
import { shouldRebuildHighlightRow } from "./keywordHighlight";

/** 只取命中区间与类型，命令文本单独断言（用例里更直观）。 */
function spans(text: string, toggles?: Parameters<typeof matchActionLinks>[1]) {
  return matchActionLinks(text, toggles).map(({ kind, text: hit, start, end }) => ({ kind, hit, start, end }));
}

describe("action links: IPv4 matcher", () => {
  it("matches a strict IPv4 with ping command", () => {
    const [hit] = matchActionLinks("server at 192.168.1.100 is up");
    expect(hit).toEqual({ kind: "ipv4", text: "192.168.1.100", start: 10, end: 23, command: "ping 192.168.1.100" });
  });

  it("accepts all octet edge values 0-255", () => {
    const text = "0.0.0.0 255.255.255.255 10.1.2.3";
    expect(spans(text)).toEqual([
      { kind: "ipv4", hit: "0.0.0.0", start: 0, end: 7 },
      { kind: "ipv4", hit: "255.255.255.255", start: 8, end: 23 },
      { kind: "ipv4", hit: "10.1.2.3", start: 24, end: 32 },
    ]);
  });

  it("rejects octets above 255 anywhere in the address", () => {
    expect(spans("256.1.1.1")).toEqual([]);
    expect(spans("1.1.1.256")).toEqual([]);
    expect(spans("host 999.10.10.10 down")).toEqual([]);
  });

  it("rejects a longer dotted chain like 1.2.3.4.5", () => {
    expect(spans("1.2.3.4.5")).toEqual([]);
    expect(spans("a.b 3.10.0.0.1 c")).toEqual([]);
  });

  it("keeps a trailing sentence dot and CIDR suffix usable", () => {
    expect(spans("connect to 8.8.8.8.")).toEqual([{ kind: "ipv4", hit: "8.8.8.8", start: 11, end: 18 }]);
    expect(spans("net 10.0.0.1/24 up")).toEqual([{ kind: "ipv4", hit: "10.0.0.1", start: 4, end: 12 }]);
  });

  it("rejects candidates glued to neighboring digits", () => {
    expect(spans("v910.0.0.1")).toEqual([]);
    expect(spans("id 2110.0.0.12")).toEqual([]);
  });
});

describe("action links: host:port matcher", () => {
  it("matches localhost, domain and IPv4 hosts with nc command", () => {
    const [local] = matchActionLinks("ssh localhost:22");
    expect(local).toEqual({ kind: "host-port", text: "localhost:22", start: 4, end: 16, command: "nc -vz localhost 22" });
    const [domain] = matchActionLinks("api.example.com:8443");
    expect(domain).toEqual({ kind: "host-port", text: "api.example.com:8443", start: 0, end: 20, command: "nc -vz api.example.com 8443" });
    const [ip] = matchActionLinks("db 10.0.0.7:5432");
    expect(ip).toEqual({ kind: "host-port", text: "10.0.0.7:5432", start: 3, end: 16, command: "nc -vz 10.0.0.7 5432" });
  });

  it("prefers the longer host:port over the bare IPv4 inside it", () => {
    expect(spans("10.0.0.1:8080")).toEqual([{ kind: "host-port", hit: "10.0.0.1:8080", start: 0, end: 13 }]);
  });

  it("rejects ports outside 1-65535", () => {
    expect(spans("localhost:0")).toEqual([]);
    expect(spans("example.com:65536")).toEqual([]);
    expect(spans("example.com:99999")).toEqual([]);
    expect(spans("example.com:65535")).toEqual([{ kind: "host-port", hit: "example.com:65535", start: 0, end: 17 }]);
  });

  it("rejects source and log file name false positives", () => {
    expect(spans("error in app.py:42")).toEqual([]);
    expect(spans("npm run app.js:12")).toEqual([]);
    expect(spans("build src/index.ts:7 fails")).toEqual([]);
    expect(spans("see main.go:101")).toEqual([]);
    expect(spans("panic in lib.rs:88")).toEqual([]);
    expect(spans("tail server.log:1")).toEqual([]);
    expect(spans("docs README.md:5")).toEqual([]);
  });

  it("rejects clock-like and chained-colon shapes", () => {
    expect(spans("at 12:34")).toEqual([]);
    // host:port 段让位（:80 后又跟 :443），裸 IPv4 仍保留可 ping。
    expect(matchActionLinks("1.2.3.4:80:443").map((hit) => hit.kind)).toEqual(["ipv4"]);
  });

  it("matches inside URLs and after @ or / without swallowing the rest", () => {
    expect(spans("curl https://example.com:8080/path")).toEqual([
      { kind: "host-port", hit: "example.com:8080", start: 13, end: 29 },
    ]);
    expect(spans("deploy@host1.corp:2222 now")).toEqual([
      { kind: "host-port", hit: "host1.corp:2222", start: 7, end: 22 },
    ]);
  });
});

describe("action links: archive matcher", () => {
  it("maps each archive type to its extraction command", () => {
    const commandOf = (text: string) => matchActionLinks(text)[0]?.command;
    expect(commandOf("fetch release.zip")).toBe("unzip release.zip");
    expect(commandOf("backup.tar.gz")).toBe("tar -xzvf backup.tar.gz");
    expect(commandOf("dump.TGZ")).toBe("tar -xzvf dump.TGZ");
    expect(commandOf("bundle.tar.bz2")).toBe("tar -xjvf bundle.tar.bz2");
    expect(commandOf("pkg.tbz2")).toBe("tar -xjvf pkg.tbz2");
    expect(commandOf("core.tar.xz")).toBe("tar -xJvf core.tar.xz");
    expect(commandOf("src.txz")).toBe("tar -xJvf src.txz");
    expect(commandOf("photo.7z")).toBe("7z x photo.7z");
    expect(commandOf("drivers.rar")).toBe("7z x drivers.rar");
  });

  it("matches full paths and ignores sentence tails", () => {
    expect(spans("unpack /tmp/dist/site.tar.gz now")).toEqual([
      { kind: "archive", hit: "/tmp/dist/site.tar.gz", start: 7, end: 28 },
    ]);
  });

  it("rejects lookalike extensions", () => {
    expect(spans("file.zipx")).toEqual([]);
    expect(spans("archive.tar.gz.bak")).toEqual([]);
    expect(spans("no-extension zip")).toEqual([]);
  });
});

describe("action links: cross-kind behavior", () => {
  it("respects the matcher toggles", () => {
    const text = "10.0.0.1:8080 data.zip";
    expect(matchActionLinks(text, { ipv4: false, hostPort: true, archive: false })).toHaveLength(1);
    expect(matchActionLinks(text, { ipv4: true, hostPort: false, archive: true }).map((hit) => hit.kind)).toEqual(["ipv4", "archive"]);
  });

  it("sorts by position and never overlaps", () => {
    const hits = matchActionLinks("ping 8.8.8.8 then db.example.com:5432 then pkg.zip");
    expect(hits.map((hit) => hit.text)).toEqual(["8.8.8.8", "db.example.com:5432", "pkg.zip"]);
    for (let index = 1; index < hits.length; index++) {
      expect(hits[index].start).toBeGreaterThanOrEqual(hits[index - 1].end);
    }
  });

  it("caps matches per line", () => {
    const text = Array.from({ length: 40 }, () => "10.0.0.1").join(" ");
    expect(matchActionLinks(text)).toHaveLength(ACTION_LINK_MATCHES_PER_LINE_LIMIT);
  });

  it("returns nothing for empty or oversize lines", () => {
    expect(matchActionLinks("")).toEqual([]);
    expect(matchActionLinks("10.0.0.1".repeat(600))).toEqual([]);
  });
});

describe("action links: settings sanitization", () => {
  it("defaults to disabled with every matcher enabled", () => {
    expect(sanitizeActionLinksSettings(undefined)).toEqual(ACTION_LINK_SETTINGS_DEFAULTS);
    expect(sanitizeActionLinksSettings({})).toEqual(ACTION_LINK_SETTINGS_DEFAULTS);
  });

  it("normalizes partial matcher objects without losing defaults", () => {
    expect(sanitizeActionLinksSettings({ enabled: true, matchers: { ipv4: false } })).toEqual({
      enabled: true,
      matchers: { ipv4: false, hostPort: true, archive: true },
    });
    expect(sanitizeActionLinksSettings({ enabled: "yes", matchers: "bad" }).matchers).toEqual(ACTION_LINK_SETTINGS_DEFAULTS.matchers);
  });
});

// 防自激验收（P1-2）：动作链接的虚线装饰复用关键词高亮的重建条件函数。
// xterm 在装饰注册/销毁后会再触发整幅重绘——"重绘但文本未变必须保留"是掐断
// "重绘→扫描→拆建→重绘"自激回路的关键，这里作为独立验收再断言一次。
describe("action links decoration rebuild policy (anti-oscillation)", () => {
  const base = { row: 5, viewportFrom: 0, viewportTo: 24 };

  it("rebuilds only when the row is dirty AND its text actually changed", () => {
    expect(shouldRebuildHighlightRow({ ...base, dirty: true, previousText: "10.0.0.1", currentText: "10.0.0.2" })).toBe(true);
  });

  it("keeps the decoration group when a redraw happens without a text change", () => {
    expect(shouldRebuildHighlightRow({ ...base, dirty: true, previousText: "10.0.0.1", currentText: "10.0.0.1" })).toBe(false);
  });

  it("releases rows that scrolled out of the viewport", () => {
    expect(shouldRebuildHighlightRow({ ...base, row: 30, dirty: false, previousText: "10.0.0.1", currentText: "10.0.0.1" })).toBe(true);
  });
});
