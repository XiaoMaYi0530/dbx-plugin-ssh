import { describe, expect, it } from "vitest";
import { workbenchMessage, workbenchMessageTable, type WorkbenchLocale } from "./i18n";

// `workbenchMessage` falls back to the *key itself* when a translation is
// missing (see `i18n.ts`), so a typo or a dependency on a retired key leaks the
// raw dotted key straight into the UI. `workbench.spec.ts` only aligns the seven
// locale tables with each other — it cannot see what the components actually
// ask for. This spec closes that gap by scanning the real call sites.
//
// Sources are inlined by Vite via `?raw`, so the check needs no filesystem
// access and runs in the default (node) test environment.
const SOURCES = import.meta.glob("../**/*.{vue,ts}", {
  query: "?raw",
  eager: true,
  import: "default",
}) as Record<string, string>;

/** `t("a.b")`, `props.t("a.b")`, `translate("a.b")`, `workbenchMessage("a.b")`. */
const CALL_REFERENCE = /(?<![\w$])(?:\$t|t|translate|workbenchMessage)\s*\(\s*(["'])([^"'`\n]+)\1/g;

/**
 * Property names that hold a translation key: `labelKey: "settingsNav.appearance"`.
 *
 * A generic `*Key` / `*Keys` pattern was tried first and matched a lot of things
 * that are emphatically not translations, so the names are listed instead:
 *   - `shiftKey` / `ctrlKey` / `altKey` / `metaKey` — DOM KeyboardEvent flags
 *   - `purposeKey` — a stable backend playbook key, mapped to
 *     `alertTriage.purpose.<key>` by `lib/alertTriage.ts` before it reaches `t`
 *   - `hostKey` / `authMethodPrivateKey` — SSH domain concepts, not copy
 *   - `hotkey` — a keybinding value that merely happens to end in "key"
 */
const KEY_PROPERTY_NAMES = ["labelKey", "labelKeys"] as const;

const KEY_PROPERTY_REFERENCE = new RegExp(
  `\\b(${KEY_PROPERTY_NAMES.join("|")})\\s*:\\s*(["'])([^"'\`\\n]+)\\2`,
  "g",
);

/** Dotted or flat identifier — rejects paths, sentences, CSS names and classes. */
const KEY_SHAPE = /^[A-Za-z][A-Za-z0-9_]*(?:\.[A-Za-z0-9]+)*$/;

interface KeyReference {
  key: string;
  file: string;
  kind: "call" | "property";
}

/**
 * Each pattern captures the key in a different group — the call form wraps the
 * whole literal in one group, the property form has the property name first — so
 * the index is explicit. Reading a fixed `match[2]` silently compared the quote
 * character instead of the key and dropped every property reference.
 */
const REFERENCE_PATTERNS = [
  { pattern: CALL_REFERENCE, keyGroup: 2, kind: "call" },
  { pattern: KEY_PROPERTY_REFERENCE, keyGroup: 3, kind: "property" },
] as const;

function collectReferences(): KeyReference[] {
  const found = new Map<string, KeyReference>();
  for (const [path, source] of Object.entries(SOURCES)) {
    // Specs deliberately probe boundaries and are not shipped code.
    if (/\.spec\.ts$/.test(path)) continue;
    const file = path.replace("../", "");
    for (const { pattern, keyGroup, kind } of REFERENCE_PATTERNS) {
      pattern.lastIndex = 0;
      for (const match of source.matchAll(pattern)) {
        const key = match[keyGroup];
        if (!KEY_SHAPE.test(key)) continue;
        const id = `${kind}:${key}`;
        // Keep the first sighting so the report points at a stable file.
        if (!found.has(id)) found.set(id, { key, file, kind });
      }
    }
  }
  return [...found.values()];
}

const REFERENCES = collectReferences();
const EN_TABLE = workbenchMessageTable("en");
const LOCALES: WorkbenchLocale[] = ["en", "es", "it", "ja", "pt-BR", "zh-CN", "zh-TW"];

describe("i18n key references in component sources", () => {
  it("scans the shipped sources rather than an empty set", () => {
    // Guards against a broken glob silently turning every check below into a
    // vacuous pass.
    const files = Object.keys(SOURCES);
    expect(files.length).toBeGreaterThan(100);
    expect(files.some((path) => path.endsWith("App.vue"))).toBe(true);
    expect(files.some((path) => path.endsWith("components/SettingsDialog.vue"))).toBe(true);
    expect(REFERENCES.length).toBeGreaterThan(300);
    // Per-kind floors: a wrong capture group once made the property scan match
    // nothing while the totals still looked healthy.
    expect(REFERENCES.filter((ref) => ref.kind === "call").length).toBeGreaterThan(300);
    expect(
      REFERENCES.filter((ref) => ref.kind === "property").length,
      "action tables reference keys through *Key properties; finding none means the property pattern stopped matching",
    ).toBeGreaterThan(5);
  });

  it("resolves every key requested by a translate call", () => {
    const missing = REFERENCES.filter((ref) => !(ref.key in EN_TABLE));
    const report = missing.map((ref) => `${ref.file} -> ${ref.key} (${ref.kind})`);
    expect(report, `unresolved translation keys:\n${report.join("\n")}`).toEqual([]);
  });

  it("never renders the raw key back to the user in any locale", () => {
    const leaked: string[] = [];
    for (const ref of REFERENCES) {
      for (const locale of LOCALES) {
        if (workbenchMessage(locale, ref.key) === ref.key) leaked.push(`${locale}:${ref.key}`);
      }
    }
    expect(leaked, `keys rendering as themselves: ${leaked.join(", ")}`).toEqual([]);
  });

  it("keeps the keys retired by the terminal-behaviour rework out of the tree", () => {
    // `terminalSelectCopy.section` and `terminalBehavior.copyOnSelect` were
    // folded into the Clipboard group and deleted from all seven locales.
    // A surviving reference would render the raw key where copy-on-select used
    // to be described.
    for (const retired of ["terminalSelectCopy.section", "terminalBehavior.copyOnSelect"]) {
      expect(retired in EN_TABLE, `${retired} should stay retired`).toBe(false);
      const stillUsed = REFERENCES.filter((ref) => ref.key === retired);
      expect(stillUsed, `${retired} is still referenced by ${stillUsed.map((r) => r.file).join(", ")}`).toEqual([]);
    }
  });

  it("covers the namespaces added by the terminal-behaviour rework", () => {
    // Locks the new surfaces into the scan: if a future refactor renames them
    // without updating every call site, this fails instead of going quiet.
    for (const namespace of ["terminalBehavior", "terminalHotkeys"]) {
      const used = REFERENCES.filter((ref) => ref.key.startsWith(`${namespace}.`));
      expect(used.length, `${namespace}.* has no scanned references`).toBeGreaterThan(5);
    }
  });
});
