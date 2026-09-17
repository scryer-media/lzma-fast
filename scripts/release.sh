#!/usr/bin/env bash
# Cut a release of this crate: run the checks the release workflow runs, create
# the signed tag and push it. .github/workflows/release.yml publishes to
# crates.io and creates the GitHub release when the tag arrives.
#
#   scripts/release.sh --dry-run   # report every problem, change nothing
#   scripts/release.sh             # check, tag, push the tag
#   scripts/release.sh --publish   # check, tag, `cargo publish` from this
#                                  # machine, push the tag (the first release,
#                                  # before trusted publishing exists)
#
#   --skip-tests   leave out `cargo test`; CI on the release commit and the
#                  workflow's own verify job both run it
#
# A dry run works on any branch and on a dirty tree and lists everything a real
# run would refuse, instead of stopping at the first. See docs/publishing.md.
set -euo pipefail

CRATE=lzma-fast
CHANGELOG=CHANGELOG.md
README=README.md

DRY_RUN=0 PUBLISH=0 SKIP_TESTS=0
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    --publish) PUBLISH=1 ;;
    --skip-tests) SKIP_TESTS=1 ;;
    *) echo "release: unknown option '$arg'" >&2; exit 2 ;;
  esac
done

cd "$(git rev-parse --show-toplevel)"

problems=0
# In a dry run a problem is reported and the run goes on; otherwise it is fatal.
problem() {
  echo "release: $*" >&2
  problems=$((problems + 1))
  [ "$DRY_RUN" = 1 ] || exit 1
}

version="$(cargo pkgid -p "$CRATE" | sed 's/.*[#@]//')"
minor="${version%.*}"
tag="v$version"
echo "release: $CRATE $version"

branch="$(git branch --show-current)"
[ "$branch" = "main" ] || problem "on '$branch', releases are cut from main"
dirty="$(git status --porcelain)"
[ -z "$dirty" ] || problem "the tree is not clean:"$'\n'"$dirty"
git verify-commit HEAD >/dev/null 2>&1 || problem "HEAD is not a signed commit"
if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  problem "tag $tag already exists"
fi

heading="$(grep -E -m1 "^## \[?$version([^0-9.]|\$)" "$CHANGELOG" || true)"
if [ -z "$heading" ]; then
  problem "$CHANGELOG has no '## $version' section"
elif echo "$heading" | grep -qi unreleased; then
  problem "$CHANGELOG still calls $version unreleased: '$heading'"
fi

# The README's dependency lines may name the full version or just major.minor.
if ! grep -Eq "^$CRATE = (\{ version = )?\"($version|$minor)\"" "$README"; then
  problem "$README shows neither \"$version\" nor \"$minor\" in its dependency lines"
fi

if [ "$SKIP_TESTS" = 1 ]; then
  echo "release: skipping cargo test"
else
  echo "release: cargo test --locked --workspace --release"
  cargo test --locked --workspace --release --no-fail-fast || problem "tests failed"
fi

echo "release: cargo publish --dry-run"
allow_dirty=()
[ "$DRY_RUN" = 1 ] && [ -n "$dirty" ] && allow_dirty=(--allow-dirty)
cargo publish -p "$CRATE" --locked --dry-run ${allow_dirty[@]+"${allow_dirty[@]}"} \
  || problem "cargo publish --dry-run failed"

if [ "$DRY_RUN" = 1 ]; then
  if [ "$problems" = 0 ]; then
    echo "release: dry run clean; a real run would tag $tag"
    exit 0
  fi
  echo "release: dry run found $problems problem(s)" >&2
  exit 1
fi

git tag -s "$tag" -m "$CRATE $version"
if [ "$PUBLISH" = 1 ]; then
  echo "release: cargo publish"
  cargo publish -p "$CRATE" --locked
fi
git push origin "$tag"
if [ "$PUBLISH" = 1 ]; then
  echo "release: published $version and pushed $tag; the workflow finds it on crates.io and only creates the GitHub release"
else
  echo "release: pushed $tag; the release workflow publishes it"
fi
