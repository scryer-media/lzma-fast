#!/usr/bin/env bash
# Correctness gate for a decode-loop variant.
#
#   ./verify.sh build/7lzma-stock
#
# Decodes the two project fixtures plus a spread of small streams built with
# `xz --format=lzma` at several presets and lc/lp/pb settings, and byte-compares
# every result.  The lc/lp/pb spread matters because lc+lp drive `lc2_lpMask`
# (which does double duty as a shift amount) and pb drives `pbMask`, so a loop
# that only ever sees the default lc=3,lp=0,pb=2 can be wrong and look right.
set -u
BIN="${1:?usage: verify.sh <decoder-binary>}"
LAB="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Fixtures live in the repo (gitignored); resolve them from the git root so
# this works from any checkout or worktree.  Override with LZLAB_FIXTURES.
# --git-common-dir, not --show-toplevel: the fixtures are generated once and
# gitignored, so they live in the MAIN checkout even when this runs from a
# worktree.  The common git dir is shared by both, so its parent is the main
# checkout root.  Override with LZLAB_FIXTURES.
FIX="${LZLAB_FIXTURES:-$(dirname "$(git -C "$LAB" rev-parse --path-format=absolute --git-common-dir)")/bench/fixtures}"
WORK="${LZLAB_WORK:-/tmp}/lzverify"
mkdir -p "$WORK"

# --- corpora: different byte statistics stress different decoder paths -------
if [ ! -f "$WORK/text.bin" ]; then
  head -c 3000000 /usr/share/dict/words                > "$WORK/text.bin"   # literal-heavy
  head -c 2000000 /dev/urandom                         > "$WORK/rand.bin"   # incompressible
  cat "$WORK/text.bin" "$WORK/text.bin"                > "$WORK/rep.bin"    # one huge match
  tar cf - -C "$HOME/dev/supporting-codebases" 7zip 2>/dev/null \
    | head -c 100000000                                > "$WORK/src.bin"    # mixed lit/match
  printf 'a%.0s' $(seq 1 100000)                       > "$WORK/runs.bin"   # rep0 short matches
  head -c 100 /dev/urandom                             > "$WORK/tiny.bin"
  : > "$WORK/empty.bin"
fi

fail=0
check() { # check <name> <plain> <lzma>
  "$BIN" d "$3" "$WORK/out.bin" >/dev/null 2>&1 \
    && cmp -s "$2" "$WORK/out.bin" \
    && echo "  ok    $1" || { echo "  FAIL  $1"; fail=1; }
}

echo "verify $(basename "$BIN")"
for c in text rand rep src runs tiny empty; do
  for opt in "preset=0" "preset=6" "preset=9e" \
             "lc=0,lp=0,pb=0" "lc=0,lp=2,pb=0" "lc=1,lp=1,pb=1" \
             "lc=4,lp=0,pb=3" "lc=8,lp=0,pb=0" "lc=3,lp=0,pb=2,dict=1MiB"; do
    f="$WORK/$c.$(echo "$opt" | tr -d ',=').lzma"
    [ -f "$f" ] || xz --format=lzma --lzma1="$opt" -c "$WORK/$c.bin" > "$f" 2>/dev/null
    [ -s "$f" ] || continue
    check "$c [$opt]" "$WORK/$c.bin" "$f"
  done
done
for f in p256 payload; do
  [ -f "$FIX/$f.bin.lzma" ] && check "fixture $f" "$FIX/$f.bin" "$FIX/$f.bin.lzma"
done
[ $fail = 0 ] && echo "ALL OK" || echo "FAILURES"
exit $fail
