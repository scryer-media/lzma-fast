#!/usr/bin/env bash
# Build JWasm, the MASM-compatible assembler the LZMA SDK's own Linux build
# uses for Asm/x86 (`MY_ASM = jwasm` in CPP/7zip/7zip_gcc.mak), and print the
# path of the binary.
#
# Pinned by commit for the reason fetch-lzma-sdk.sh gives. v2.20 is the
# latest non-prerelease tag.
set -euo pipefail

REPO=https://github.com/Baron-von-Riedesel/JWasm.git
COMMIT=ac54827ff40b77ecd6e77ed0866a43fc60cd5fe1 # tag v2.20

dest="${1:?usage: build-jwasm.sh <build directory>}"
git init --quiet "$dest"
git -C "$dest" fetch --quiet --depth 1 "$REPO" "$COMMIT"
git -C "$dest" -c advice.detachedHead=false checkout --quiet FETCH_HEAD
test "$(git -C "$dest" rev-parse HEAD)" = "$COMMIT"
make --silent -C "$dest" -f GccUnix.mak -j"$(nproc)" >&2
echo "$(cd "$dest" && pwd)/build/GccUnixR/jwasm"
