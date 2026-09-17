#!/usr/bin/env bash
# Build and install the xz the .xz container tests are written against.
#
# ubuntu-24.04 ships xz 5.4.5 - the release Debian reverted to after the
# CVE-2024-3094 backdoor in 5.6.0/5.6.1 - and tests/xz_container.rs drives the
# system `xz` to produce the vectors it decodes. Filters that arrived in 5.6
# (--riscv among them) are rejected there, so those tests fail on the runner
# while passing everywhere the developer works. Rather than skip them on Linux,
# install a current xz ahead of the system one.
#
# The version and its checksum are pinned, the way every action in these
# workflows is pinned by SHA: a build tool fetched as "latest" over the network
# is an unreviewed input, and this particular tarball has a history that makes
# that more than a theoretical objection. 5.8.3 is well past the compromised
# releases, and is the version the tests are known to pass against locally.
#
# The build is static-only. Installing a shared liblzma into /usr/local would
# shadow the system one for everything that links it - tar, dpkg, systemd - and
# a CI job has no business doing that. Only the `xz` binary is wanted.
set -euo pipefail

VERSION=5.8.3
SHA256=3d3a1b973af218114f4f889bbaa2f4c037deaae0c8e815eec381c3d546b974a0
URL="https://github.com/tukaani-project/xz/releases/download/v${VERSION}/xz-${VERSION}.tar.gz"

echo "system xz before: $(xz --version | head -n1)"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"

curl --fail --silent --show-error --location --retry 3 --output "xz-${VERSION}.tar.gz" "$URL"
echo "${SHA256}  xz-${VERSION}.tar.gz" | sha256sum --check --strict -

tar xzf "xz-${VERSION}.tar.gz"
cd "xz-${VERSION}"

./configure --prefix=/usr/local --disable-shared --enable-static --disable-doc --quiet
make -j"$(nproc)" --silent
sudo make install --silent

# /usr/local/bin precedes /usr/bin on the runners, so this is what the tests
# will spawn. Fail loudly here rather than three test failures later.
hash -r
echo "xz now on PATH: $(command -v xz) -> $(xz --version | head -n1)"
xz --version | head -n1 | grep -q " ${VERSION}$"
