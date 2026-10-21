#!/usr/bin/env bash
# Packs one release asset the way the build hook (lua/orca-term/install.lua) unpacks it:
# <out dir>/orca-term-<version>-<target>.tar.gz, holding only the binary `orca-term` and LICENSE,
# and beside it <asset>.sha256, as `shasum -a 256` writes it (`<hex>  <asset>`). The release
# workflow runs it once per target; the build hook's specs run it to make their fake release.
#
# Usage: package-release.sh <binary> <version> <target> <out dir>
set -euo pipefail

name=orca-term
if [ "$#" -ne 4 ]; then
    echo "usage: package-release.sh <binary> <version> <target> <out dir>" >&2
    exit 2
fi
binary="$1"
version="$2"
target="$3"
out="$4"
root="$(cd "$(dirname "$0")/../.." && pwd)"
asset="${name}-${version}-${target}.tar.gz"

staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT
cp "$binary" "$staging/$name"
chmod 755 "$staging/$name"
cp "$root/LICENSE" "$staging/LICENSE"

mkdir -p "$out"
tar -czf "$out/$asset" -C "$staging" "$name" LICENSE
(cd "$out" && shasum -a 256 "$asset" > "$asset.sha256")
echo "packed $out/$asset"
