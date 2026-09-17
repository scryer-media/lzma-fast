#!/usr/bin/env bash
# Build the LZMA SDK's `7lzma` CLI against a chosen arm64 decode loop.
#
#   ./build.sh c                     -> build/7lzma-c        (no asm, C fast loop)
#   ./build.sh LzmaDecOpt-stock.S    -> build/7lzma-stock    (7-Zip's shipped loop)
#   ./build.sh variants/01-foo.S     -> build/7lzma-01-foo
#
# Flags mirror what 7-Zip's own makefile uses for a release build:
#   C/7zip_gcc_c.mak: CFLAGS_BASE = -O2 -c -Wall -Werror -Wextra \
#                                   -DNDEBUG -D_REENTRANT \
#                                   -D_FILE_OFFSET_BITS=64 -D_LARGEFILE_SOURCE
#   C/Util/Lzma/makefile.gcc + CPP/7zip/LzmaDec_gcc.mak add -DZ7_LZMA_DEC_OPT to
#   LzmaDec.c and assemble Asm/arm64/LzmaDecOpt.S with the same $(CFLAGS).
set -euo pipefail

LAB="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SDK="${SDK:?set SDK to a checkout of github.com/ip7z/7zip}"
C="$SDK/C"
OUT="$LAB/build"
CC="${CC:-clang}"

CFLAGS=(-O2 -c -DNDEBUG -D_REENTRANT -D_FILE_OFFSET_BITS=64 -D_LARGEFILE_SOURCE)
ASMFLAGS=(-DLAB_NOFLAG)
# PROB32=1 widens CLzmaProb to 32 bits on BOTH sides (C and asm must agree).
if [ "${PROB32:-0}" = 1 ]; then
  CFLAGS+=(-DZ7_LZMA_PROB32)
  ASMFLAGS+=(-D_LZMA_PROB32)
  SUFFIX="-p32"
else
  SUFFIX=""
fi

SRC=(7zFile 7zStream Alloc CpuArch LzFind LzFindMt LzFindOpt LzmaEnc Threads)

usage() { sed -n '2,12p' "$0"; exit 1; }
[ $# -eq 1 ] || usage

ASM="$1"
IS_C=0
if [ "$ASM" = "c" ]; then
  IS_C=1
  TAG=c
else
  [ -f "$LAB/$ASM" ] || { echo "no such asm: $LAB/$ASM" >&2; exit 1; }
  TAG="$(basename "$ASM" .S)"
  TAG="${TAG#LzmaDecOpt-}"
fi
TAG="$TAG$SUFFIX"

O="$OUT/$TAG"
mkdir -p "$O"

for f in "${SRC[@]}"; do
  [ "$O/$f.o" -nt "$C/$f.c" ] || "$CC" "${CFLAGS[@]}" -o "$O/$f.o" "$C/$f.c"
done
"$CC" "${CFLAGS[@]}" -o "$O/LzmaUtil.o" "$C/Util/Lzma/LzmaUtil.c"

OBJS=("$O"/*.o)
if [ "$IS_C" = 1 ]; then
  "$CC" "${CFLAGS[@]}" -o "$O/LzmaDec.o" "$C/LzmaDec.c"
else
  "$CC" "${CFLAGS[@]}" -DZ7_LZMA_DEC_OPT -o "$O/LzmaDec.o" "$C/LzmaDec.c"
  # -I"$LAB" so the variant's `#include "7zAsm.S"` picks up the lab's copy.
  "$CC" "${CFLAGS[@]}" "${ASMFLAGS[@]}" -I"$LAB" -o "$O/LzmaDecOpt.o" "$LAB/$ASM"
fi

OBJS=("$O"/*.o)
"$CC" -o "$OUT/7lzma-$TAG" "${OBJS[@]}"
echo "built $OUT/7lzma-$TAG"
