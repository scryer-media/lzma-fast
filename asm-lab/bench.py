#!/usr/bin/env python3
"""Timing for a decoder command, robust to a busy machine.

    ./bench.py --label NAME --mib 256 -n 7 -- cmd arg...

Reports MIN-of-N child *user CPU* time, not median wall time.  Rationale: this
box is shared with other builds, so wall time picks up scheduling
delay that has nothing to do with the code under test.  User CPU time excludes
time the process was not running, and the minimum is the run least polluted by
cache/DVFS interference from the neighbours.  Wall time is still printed so a
pathological run is visible.
"""
import argparse
import os
import resource
import subprocess
import sys
import time

p = argparse.ArgumentParser()
p.add_argument("--label", required=True)
p.add_argument("--mib", type=float, required=True, help="decoded MiB (for MiB/s)")
p.add_argument("-n", type=int, default=7)
p.add_argument("--quiet", action="store_true")
p.add_argument("cmd", nargs=argparse.REMAINDER)
a = p.parse_args()
cmd = a.cmd[1:] if a.cmd and a.cmd[0] == "--" else a.cmd

cpu, wall = [], []
for _ in range(a.n):
    r0 = resource.getrusage(resource.RUSAGE_CHILDREN)
    t = time.monotonic()
    rc = subprocess.run(cmd, stdout=subprocess.DEVNULL,
                        stderr=subprocess.DEVNULL).returncode
    wall.append(time.monotonic() - t)
    r1 = resource.getrusage(resource.RUSAGE_CHILDREN)
    cpu.append((r1.ru_utime - r0.ru_utime) + (r1.ru_stime - r0.ru_stime))
    if rc != 0:
        sys.exit(f"{a.label}: command failed rc={rc}: {' '.join(cmd)}")

best = min(cpu)
print(f"{a.label:<26} {best:7.4f} s cpu {a.mib/best:8.1f} MiB/s  "
      f"min-wall {min(wall):6.3f}  cpu: {' '.join(f'{c:.3f}' for c in cpu)}")
if not a.quiet:
    with open(os.environ.get("BENCH_LOG", "/dev/null"), "a") as f:
        f.write(f"{a.label}\t{best:.4f}\t{a.mib/best:.1f}\n")
