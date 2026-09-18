#!/usr/bin/env bash
# Fetch the LZMA SDK source this crate was ported from, into the directory
# given as the only argument.
#
# The decode loops under src/lzma/decode_opt/ are translations of files in
# this tree, and the provenance and upstream-oracle jobs compare the crate
# against it. It is fetched by commit, not by tag or by GitHub's generated
# tarball: a commit id names exactly one tree, a tag can be moved, and an
# archive's bytes are not promised to stay the same. tools/asm-provenance
# additionally pins the SHA-256 of every file it assembles, and
# tests/upstream-oracle of every file it compiles.
set -euo pipefail

REPO=https://github.com/ip7z/7zip.git
COMMIT=0766b733fe3e06dd2a7f9a3cfbf2108ac73abd17 # tag 26.03

dest="${1:?usage: fetch-lzma-sdk.sh <destination>}"
git init --quiet "$dest"
git -C "$dest" fetch --quiet --depth 1 "$REPO" "$COMMIT"
git -C "$dest" -c advice.detachedHead=false checkout --quiet FETCH_HEAD
test "$(git -C "$dest" rev-parse HEAD)" = "$COMMIT"
echo "LZMA SDK at $COMMIT in $dest"
