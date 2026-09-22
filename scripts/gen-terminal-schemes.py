#!/usr/bin/env python3
"""Generate the plugin's built-in terminal color scheme catalog from Tabby.

Source (MIT):
  - tabby-community-color-schemes/schemes/*        (191 Xresources files)
  - tabby-terminal/src/colorSchemes.ts             (Tabby Default / Default Light)

Parser semantics mirror Tabby's own
`tabby-community-color-schemes/src/colorSchemes.ts`: `#define NAME value`
variables, then `*.key: value` assignments, then color0..color15 collected
in order while the index exists.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

SRC = Path("/tmp/tabby-src/tabby-community-color-schemes/schemes")
OUT = Path(sys.argv[1])

ANSI_NAMES = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    "brightBlack", "brightRed", "brightGreen", "brightYellow", "brightBlue",
    "brightMagenta", "brightCyan", "brightWhite",
]

HEX = re.compile(r"^#(?:[0-9a-fA-F]{6}|[0-9a-fA-F]{3})$")


def parse_scheme(text: str, name: str) -> dict | None:
    variables: dict[str, str] = {}
    for line in text.split("\n"):
        if not line.startswith("#define"):
            continue
        parts = [chunk.strip() for chunk in line.split(" ")]
        if len(parts) >= 3:
            variables[parts[1]] = parts[2]

    values: dict[str, str] = {}
    for line in text.split("\n"):
        if not line.startswith("*."):
            continue
        body = line[2:]
        if ":" not in body:
            continue
        key, value = body.split(":", 1)
        key = key.strip()
        value = value.strip()
        values[key] = variables.get(value, value)

    colors: list[str] = []
    index = 0
    while f"color{index}" in values:
        colors.append(values[f"color{index}"])
        index += 1

    if len(colors) < 16:
        print(f"  skip {name}: only {len(colors)} ANSI colors", file=sys.stderr)
        return None
    foreground = values.get("foreground", "")
    background = values.get("background", "")
    cursor = values.get("cursorColor", foreground)
    for label, value in (("foreground", foreground), ("background", background), ("cursor", cursor)):
        if not HEX.match(value):
            print(f"  skip {name}: invalid {label} {value!r}", file=sys.stderr)
            return None
    for value in colors[:16]:
        if not HEX.match(value):
            print(f"  skip {name}: invalid ANSI value {value!r}", file=sys.stderr)
            return None
    return {
        "name": name,
        "foreground": foreground,
        "background": background,
        "cursor": cursor,
        "colors": colors[:16],
    }


def slugify(name: str) -> str:
    slug = re.sub(r"[^a-z0-9]+", "-", name.strip().lower()).strip("-")
    return slug or "scheme"


def main() -> int:
    schemes: list[dict] = []

    # Tabby's own two defaults, verbatim from tabby-terminal/src/colorSchemes.ts.
    schemes.append({
        "name": "Tabby Default",
        "foreground": "#cacaca",
        "background": "#171717",
        "cursor": "#bbbbbb",
        "colors": [
            "#000000", "#ff615a", "#b1e969", "#ebd99c",
            "#5da9f6", "#e86aff", "#82fff7", "#dedacf",
            "#313131", "#f58c80", "#ddf88f", "#eee5b2",
            "#a5c7ff", "#ddaaff", "#b7fff9", "#ffffff",
        ],
    })
    schemes.append({
        "name": "Tabby Default Light",
        "foreground": "#4d4d4c",
        "background": "#ffffff",
        "cursor": "#4d4d4c",
        "colors": [
            "#000000", "#c82829", "#718c00", "#eab700",
            "#4271ae", "#8959a8", "#3e999f", "#ffffff",
            "#000000", "#c82829", "#718c00", "#eab700",
            "#4271ae", "#8959a8", "#3e999f", "#ffffff",
        ],
    })

    skipped: list[str] = []
    source_files = 0
    for path in sorted(SRC.iterdir(), key=lambda item: item.name.lower()):
        if path.is_dir():
            continue
        source_files += 1
        parsed = parse_scheme(path.read_text(encoding="utf-8", errors="replace"), path.name)
        if parsed:
            schemes.append(parsed)
        else:
            skipped.append(path.name)

    # ids must stay unique: they are the persisted preference value, and the
    # runtime slug rule (frontend/src/lib/terminalScheme.ts schemeIdFromName)
    # must stay identical to this one so imports dedupe correctly.
    seen: dict[str, str] = {}
    for scheme in schemes:
        base = slugify(scheme["name"])
        scheme["id"] = base
        if base in seen:
            suffix = 2
            while f"{base}-{suffix}" in seen:
                suffix += 1
            scheme["id"] = f"{base}-{suffix}"
            print(f"  id collision: {scheme['name']} -> {scheme['id']}", file=sys.stderr)
        seen[scheme["id"]] = scheme["name"]

    lines: list[str] = []
    lines.append("// 内置终端配色方案目录（自动生成，勿手工编辑）。")
    lines.append("//")
    lines.append("// 数据来源：Tabby（MIT License, https://github.com/Eugeny/tabby）")
    lines.append("//   - tabby-terminal/src/colorSchemes.ts —— Tabby Default / Tabby Default Light")
    lines.append(f"//   - tabby-community-color-schemes/schemes/* —— {source_files} 个 Xresources 方案")
    lines.append("//     （上游整理自 iTerm2-Color-Schemes，MIT License）")
    if skipped:
        lines.append(f"//     跳过 {len(skipped)} 个不完整方案（ANSI 色不足 16 个）：{', '.join(skipped)}")
    lines.append("//")
    lines.append("// 重新生成：scripts/gen-terminal-schemes.py（读取 Tabby 仓库 sparse checkout）。")
    lines.append("// 元组布局：[id, name, foreground, background, cursor, colors(ANSI 0-15)]。")
    lines.append("")
    lines.append('import type { TerminalSchemeTuple } from "./terminalScheme";')
    lines.append("")
    lines.append("export const TABBY_BUILTIN_SCHEMES: readonly TerminalSchemeTuple[] = [")
    for scheme in schemes:
        colors = ", ".join(f'"{value}"' for value in scheme["colors"])
        lines.append(
            f'  ["{scheme["id"]}", "{scheme["name"]}", "{scheme["foreground"]}", '
            f'"{scheme["background"]}", "{scheme["cursor"]}", [{colors}]],'
        )
    lines.append("];")
    lines.append("")

    OUT.write_text("\n".join(lines), encoding="utf-8")
    print(f"wrote {len(schemes)} schemes -> {OUT}")
    if skipped:
        print(f"skipped: {skipped}")
    print(f"ANSI order reference: {', '.join(ANSI_NAMES)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
