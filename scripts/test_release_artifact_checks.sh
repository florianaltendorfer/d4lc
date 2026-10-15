#!/usr/bin/env bash
# Tests scripts/check_expected_artifacts.sh and
# scripts/verify_release_checksums.sh on throwaway files.
set -euo pipefail
cd "$(dirname "$0")/.."
expected="$PWD/scripts/check_expected_artifacts.sh"
checksums="$PWD/scripts/verify_release_checksums.sh"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"

fail() {
    echo "test_release_artifact_checks: $*" >&2
    exit 1
}
ok() { "$@" >/dev/null 2>&1 || fail "expected success: $*"; }
refused() { "$@" >/dev/null 2>&1 && fail "expected refusal: $*"; true; }

printf a >a.tar.gz
printf b >b.deb
printf c >c.AppImage
: >empty.txt
sha256sum a.tar.gz b.deb >SHA256SUMS

# check_expected_artifacts.sh
ok "$expected" 3 a.tar.gz b.deb SHA256SUMS
refused "$expected" 3 a.tar.gz b.deb               # one too few
refused "$expected" 2 a.tar.gz b.deb c.AppImage    # one too many
refused "$expected" 3 a.tar.gz b.deb ""            # unset workflow output
refused "$expected" 2 a.tar.gz missing.deb
refused "$expected" 2 a.tar.gz empty.txt
refused "$expected" 2 a.tar.gz .                   # not a regular file
refused "$expected" 0
refused "$expected" x a.tar.gz

# verify_release_checksums.sh
ok "$checksums" SHA256SUMS a.tar.gz b.deb
refused "$checksums" SHA256SUMS c.AppImage         # not listed
refused "$checksums" empty.txt a.tar.gz            # empty checksum file
refused "$checksums" SHA256SUMS
printf x >>b.deb
refused "$checksums" SHA256SUMS a.tar.gz b.deb     # tampered
printf b >b.deb
sha256sum a.tar.gz >>SHA256SUMS
refused "$checksums" SHA256SUMS a.tar.gz           # listed twice

echo "test_release_artifact_checks: ok"
