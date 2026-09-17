#!/usr/bin/env bash
# Cut a release of lzma-fast: run every check the release workflow will run, then
# create the signed tag and push it. Publishing to crates.io and the GitHub
# release are done by .github/workflows/release.yml when the tag arrives.
#
#   scripts/release.sh            # check, tag, push the tag
#   scripts/release.sh --dry-run  # check only
#
# See docs/publishing.md for the release-day sequence.
set -euo pipefail

CRATE=lzma-fast
CHANGELOG=crates/lzma-fast/CHANGELOG.md
DRY_RUN=0
[ "${1:-}" = "--dry-run" ] && DRY_RUN=1

cd "$(git rev-parse --show-toplevel)"

fail() { echo "release: $*" >&2; exit 1; }

branch="$(git branch --show-current)"
[ "$branch" = "main" ] || fail "on '$branch', releases are cut from main"
[ -z "$(git status --porcelain)" ] || fail "the tree is not clean"
git verify-commit HEAD >/dev/null 2>&1 || fail "HEAD is not a signed commit"

version="$(cargo pkgid -p "$CRATE" | sed 's/.*[#@]//')"
tag="v$version"
echo "release: $CRATE $version"

git rev-parse -q --verify "refs/tags/$tag" >/dev/null && fail "tag $tag already exists"

heading="$(grep -m1 "^## .*$version" "$CHANGELOG" || true)"
[ -n "$heading" ] || fail "$CHANGELOG has no '## ... $version' section"
echo "$heading" | grep -qi unreleased && fail "$CHANGELOG still calls $version unreleased: '$heading'"

grep -q "^lzma-fast = \"$version\"\|lzma-fast = { version = \"$version\"" crates/lzma-fast/README.md 2>/dev/null \
  || [ "$CRATE" != "lzma-fast" ] || fail "crates/lzma-fast/README.md does not show version $version in its dependency lines"

echo "release: cargo test --locked --workspace"
cargo test --locked --workspace --no-fail-fast
echo "release: cargo publish --dry-run"
cargo publish -p "$CRATE" --locked --dry-run

if [ "$DRY_RUN" = 1 ]; then
  echo "release: dry run, not tagging $tag"
  exit 0
fi

git tag -s "$tag" -m "$CRATE $version"
git push origin "$tag"
echo "release: pushed $tag; the release workflow publishes it"
