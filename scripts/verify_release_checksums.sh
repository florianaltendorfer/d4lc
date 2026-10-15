#!/usr/bin/env bash
# Verify release artifacts against their SHA256SUMS file.
#
# Every ARTIFACT must appear exactly once in SUMS (by file name, the format
# `sha256sum` writes) and its content must match that digest. Used by the
# packaging job, the .deb install test and publish_draft_release.sh, so the
# bytes that are tested, installed and uploaded are the bytes the checksum
# file names.
#
# Usage: verify_release_checksums.sh SUMS ARTIFACT...
set -euo pipefail

fail() {
    echo "::error::verify_release_checksums: $*" >&2
    exit 1
}

[ "$#" -ge 2 ] || fail "usage: verify_release_checksums.sh SUMS ARTIFACT..."
sums=$1
shift
[ -s "$sums" ] || fail "checksum file missing or empty: $sums"

for artifact in "$@"; do
    [ -s "$artifact" ] || fail "artifact missing or empty: $artifact"
    name="$(basename "$artifact")"
    expected="$(awk -v name="$name" '
        { line = $0; sub(/^[0-9a-f]+  \*?/, "", line) }
        line == name && length($1) == 64 && $1 ~ /^[0-9a-f]+$/ { print $1 }
    ' "$sums")"
    count="$(printf '%s' "$expected" | grep -c . || true)"
    [ "$count" -eq 1 ] || fail "$name is listed $count times in $(basename "$sums"), expected once"
    actual="$(sha256sum "$artifact" | cut -d' ' -f1)"
    [ "$actual" = "$expected" ] || fail "$name: sha256 $actual does not match $expected"
    echo "verified $name ($actual)"
done
