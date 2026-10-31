#!/usr/bin/env python3
"""Regenerates the scancode fixture and the `match` arms of keymap.rs.

  scripts/gen-keymap.py fixture <keymaps.csv>   writes tests/fixtures/set1-evdev.tsv
  scripts/gen-keymap.py arms                    prints the two `match` bodies

The source is keycodemapdb data/keymaps.csv (https://gitlab.com/keycodemap/keycodemapdb,
commit 54788039), used under its BSD-3-Clause option; see
crates/gliff-windows/tests/fixtures/set1-evdev.NOTICE.
"""
import csv
import pathlib
import sys

FIXTURE = pathlib.Path(__file__).resolve().parent.parent / "crates/gliff-windows/tests/fixtures/set1-evdev.tsv"


def fixture(csv_path):
    rows, names = {}, {}
    for r in csv.DictReader(open(csv_path, newline="")):
        code = int(r["Linux Keycode"], 0)
        if code and r["Linux Name"]:
            names.setdefault(code, r["Linux Name"])
        s = r["AT set1 keycode"]
        if s and code:
            rows.setdefault(int(s, 16), code)
    out = ["# set1 (E0 prefix as 0xE0xx)\tevdev\tname\n"]
    for v in sorted(rows):
        out.append("0x%04x\t%d\t%s\n" % (v, rows[v], names.get(rows[v], "")))
    FIXTURE.write_text("".join(out))
    print(len(rows), "rows")


def arms():
    for title, lo_hi in (("normal", (0, 0xFF)), ("e0", (0xE000, 0xE0FF))):
        print("//", title)
        for line in FIXTURE.read_text().splitlines():
            if line.startswith("#"):
                continue
            v, code, name = line.split("\t")
            v, code = int(v, 16), int(code)
            if lo_hi[0] <= v <= lo_hi[1]:
                print("        0x%02x => %d, // %s" % (v & 0xFF, code, name or "(unnamed)"))


if __name__ == "__main__":
    {"fixture": lambda: fixture(sys.argv[2]), "arms": arms}[sys.argv[1]]()
