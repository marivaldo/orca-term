#!/usr/bin/env bash
# Checks the release assets in <dir> before anything is published: for each target, the asset
# `orca-term-<version>-<target>.tar.gz` and its `.sha256` are there, the `.sha256` names that asset
# and matches it, and the asset holds exactly the binary `orca-term` and LICENSE; and <dir> holds
# nothing else. Any failure exits non-zero, so the release workflow creates no release.
#
# Usage: verify-release.sh <dir> <version> <target>...
set -euo pipefail

name=orca-term
if [ "$#" -lt 3 ]; then
    echo "usage: verify-release.sh <dir> <version> <target>..." >&2
    exit 2
fi
dir="$1"
version="$2"
shift 2
cd "$dir"

expected=0
for target in "$@"; do
    asset="${name}-${version}-${target}.tar.gz"
    for file in "$asset" "$asset.sha256"; do
        [ -f "$file" ] || { echo "verify: $file is missing" >&2; exit 1; }
    done
    named="$(cut -d' ' -f3- "$asset.sha256")"
    [ "$named" = "$asset" ] || { echo "verify: $asset.sha256 names $named" >&2; exit 1; }
    shasum -a 256 --check "$asset.sha256"
    contents="$(tar -tzf "$asset" | LC_ALL=C sort | paste -sd' ' -)"
    [ "$contents" = "LICENSE $name" ] || { echo "verify: $asset holds $contents" >&2; exit 1; }
    expected=$((expected + 2))
done

found="$(find . -maxdepth 1 -type f | wc -l | tr -d ' ')"
[ "$found" -eq "$expected" ] || { echo "verify: $found files, expected $expected" >&2; exit 1; }
echo "verify: $expected files, every checksum matches"
