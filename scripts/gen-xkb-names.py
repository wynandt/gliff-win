#!/usr/bin/env python3
"""Regenerate crates/gliff-windows/tests/fixtures/xkb-layouts.tsv.

Lists every layout and layout/variant pair xkeyboard-config knows, from its
`rules/evdev.lst`, so the KLID table in `xkb.rs` can be tested against names
the server's libxkbcommon will accept. Usage:
  scripts/gen-xkb-names.py [/usr/share/X11/xkb/rules/evdev.lst] > fixture
"""
import sys

path = sys.argv[1] if len(sys.argv) > 1 else "/usr/share/X11/xkb/rules/evdev.lst"
section = None
rows = []
for line in open(path, encoding="utf-8"):
    line = line.rstrip("\n")
    if line.startswith("! "):
        section = line[2:].strip()
        continue
    if not line.strip():
        continue
    if section == "layout":
        name = line.split()[0]
        rows.append((name, "-"))
    elif section == "variant":
        variant, rest = line.split(None, 1)
        layout = rest.split(":", 1)[0].strip()
        rows.append((layout, variant))
print("# layout\tvariant ('-' for none), from xkeyboard-config rules/evdev.lst")
for layout, variant in sorted(set(rows)):
    print(f"{layout}\t{variant}")
