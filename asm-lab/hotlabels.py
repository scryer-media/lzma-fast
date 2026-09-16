#!/usr/bin/env python3
"""Attribute samply samples to blocks inside a hand-written asm function.

samply does not symbolicate a local static binary, and even when it does it
resolves only to function symbols, so `LzmaDec_DecodeReal_3` swallows the whole
decode. This reads the raw module-relative frame addresses out of the profile
and buckets them by the nearest preceding branch target in `objdump -d` of the
linked binary, which reconstructs the asm's basic blocks.

    ./hotlabels.py <profile.json.gz> <binary> [func]   (default _LzmaDec_DecodeReal_3)
"""
import collections
import gzip
import json
import re
import subprocess
import sys

prof, binary = sys.argv[1], sys.argv[2]
func = sys.argv[3] if len(sys.argv) > 3 else "_LzmaDec_DecodeReal_3"

dis = subprocess.run(
    ["objdump", "-d", "--no-show-raw-insn", binary],
    capture_output=True, text=True, check=True).stdout.splitlines()

fn_start = fn_end = None
inside = False
insn_at = {}
for line in dis:
    m = re.match(r"^([0-9a-f]+) <(.+)>:", line)
    if m:
        if inside:
            fn_end = max(insn_at) + 4
            inside = False
        inside = m.group(2) == func
        if inside:
            fn_start = int(m.group(1), 16)
        continue
    if not inside:
        continue
    m = re.match(r"^\s*([0-9a-f]+):\s+(.*)$", line)
    if m:
        insn_at[int(m.group(1), 16)] = m.group(2).strip()
if fn_end is None:
    fn_end = max(insn_at) + 4
if fn_start is None:
    sys.exit(f"function {func} not found in {binary}")

targets = {fn_start}
for va, txt in insn_at.items():
    if not re.match(r"^(b|b\.|cb|tb)", txt):
        continue
    for m in re.finditer(r"0x([0-9a-f]+)", txt):
        t = int(m.group(1), 16)
        if fn_start <= t < fn_end:
            targets.add(t)
blocks = sorted(targets)

MACHO_BASE = 0x100000000
d = json.load(gzip.open(prof))
counts = collections.Counter()
total = 0
outside = 0
for t in d["threads"]:
    ft, st, ss = t["frameTable"], t["stackTable"], t["samples"]
    for si in range(ss["length"]):
        s = ss["stack"][si]
        if s is None:
            continue
        addr = ft["address"][st["frame"][s]]
        if addr is None or addr < 0:
            continue
        va = MACHO_BASE + addr
        if not (fn_start <= va < fn_end):
            outside += 1
            continue
        total += 1
        lo, hi, best = 0, len(blocks) - 1, blocks[0]
        while lo <= hi:
            mid = (lo + hi) // 2
            if blocks[mid] <= va:
                best, lo = blocks[mid], mid + 1
            else:
                hi = mid - 1
        counts[(best, va)] += 1

print(f"{func}: {total} samples in-function, {outside} elsewhere "
      f"({100*total/(total+outside):.1f}% of profile)")
by_block = collections.Counter()
for (b, va), c in counts.items():
    by_block[b] += c
print("\n-- by basic block (offset from function start) --")
for b, c in by_block.most_common(30):
    print(f"  +0x{b-fn_start:05x}  {100*c/total:5.2f}%  {c:6d}   {insn_at.get(b,'')[:58]}")
print("\n-- hottest single instructions --")
for (b, va), c in counts.most_common(30):
    print(f"  +0x{va-fn_start:05x}  {100*c/total:5.2f}%  (blk +0x{b-fn_start:05x})  "
          f"{insn_at.get(va,'')[:52]}")
