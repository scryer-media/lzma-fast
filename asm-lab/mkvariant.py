#!/usr/bin/env python3
"""Generate a variant .S from LzmaDecOpt-stock.S by applying named edits.

Each edit is an exact (old, new) string pair applied to the stock text; a miss
is a hard error so a variant can never silently be a copy of the stock file.

    ./mkvariant.py <name> <edits-module.py>

is not how it is used -- variants are built by small inline scripts that import
`apply` from here.
"""
import pathlib

LAB = pathlib.Path(__file__).resolve().parent
STOCK = (LAB / "LzmaDecOpt-stock.S").read_text()


def apply(out_name, header, edits, base=None):
    s = STOCK if base is None else (LAB / base).read_text()
    for i, (old, new) in enumerate(edits):
        if old not in s:
            raise SystemExit(f"{out_name}: edit #{i} did not match:\n{old[:300]}")
        if s.count(old) != 1:
            raise SystemExit(f"{out_name}: edit #{i} matched {s.count(old)}x")
        s = s.replace(old, new)
    s = s.replace(
        "// LzmaDecOpt.S -- ARM64-ASM version",
        header.rstrip() + "\n// LzmaDecOpt.S -- ARM64-ASM version", 1)
    p = LAB / "variants" / out_name
    p.write_text(s)
    print(f"wrote {p.relative_to(LAB)}")
    return p
