#!/usr/bin/env bash
# Generates the benchmark fixtures under bench/fixtures. Deterministic payload
# (seeded), so numbers are comparable across machines given the same tools.
# Requires: python3, xz (XZ Utils), 7zz (7-Zip). Needs ~4.5 GiB free.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
out="$here/bench/fixtures"
mkdir -p "$out"
cd "$out"

if [ ! -f payload.bin ]; then
  echo "generating payload.bin (1 GiB, semi-compressible, seed 7)"
  python3 - <<'PY'
import os, random
random.seed(7)
words = [os.urandom(random.randint(3, 9)).hex() for _ in range(4000)]
with open('payload.bin', 'wb') as f:
    for _ in range(1024):
        chunk = bytearray()
        while len(chunk) < 1 << 20:
            if random.random() < 0.7:
                chunk += random.choice(words).encode() + b' '
            else:
                chunk += os.urandom(random.randint(16, 256))
        f.write(chunk[:1 << 20])
PY
fi
# NOTE: os.urandom makes the random-byte runs non-deterministic; the token
# stream and the compression ratio are stable, byte identity across machines
# is not. Compare timings on one machine, not fixture hashes across machines.

[ -f p256.bin ] || head -c 268435456 payload.bin > p256.bin

# LZMA1 (LZMA-alone container, 13-byte header) at xz preset 5, single thread.
[ -f p256.bin.lzma ]    || xz -T1 -5 --format=lzma -c p256.bin    > p256.bin.lzma
[ -f payload.bin.lzma ] || xz -T1 -5 --format=lzma -c payload.bin > payload.bin.lzma

# LZMA2 inside xz, preset 5, single stream.
[ -f p256.bin.xz ] || xz -T1 -5 -c p256.bin > p256.bin.xz

# LZMA2 inside 7z: st = one stream, mt = 7-Zip's multi-threaded chunking.
[ -f st.7z ] || 7zz a -bso0 -bsp0 -mx=5 -m0=lzma2 -mmt=1  st.7z payload.bin
[ -f mt.7z ] || 7zz a -bso0 -bsp0 -mx=5 -m0=lzma2 -mmt=on mt.7z payload.bin

ls -l "$out"
