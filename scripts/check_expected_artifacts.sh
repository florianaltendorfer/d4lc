#!/usr/bin/env bash
# Refuse an incomplete release artifact set before any upload or release
# mutation: exactly COUNT paths must be given (an empty workflow output is an
# empty argument and counts as missing), and every one must be an existing,
# non-empty regular file. All problems are listed, then the script fails.
#
# Usage: check_expected_artifacts.sh COUNT PATH...
set -euo pipefail

[[ "${1:-}" =~ ^[1-9][0-9]*$ ]] || {
    echo "::error::check_expected_artifacts: usage: check_expected_artifacts.sh COUNT PATH..." >&2
    exit 2
}
expected_count=$1
shift

problems=()
[ "$#" -eq "$expected_count" ] \
    || problems+=("$# artifact paths given, expected exactly $expected_count")
for asset in "$@"; do
    if [ -z "$asset" ]; then
        problems+=("an artifact path is empty (a workflow output was not set)")
    elif [ ! -f "$asset" ]; then
        problems+=("missing: $asset")
    elif [ ! -s "$asset" ]; then
        problems+=("empty: $asset")
    fi
done

if [ "${#problems[@]}" -gt 0 ]; then
    echo "::error::Expected release artifacts missing or empty; refusing to upload or publish an incomplete release:" >&2
    printf '  - %s\n' "${problems[@]}" >&2
    exit 1
fi

echo "All expected release artifacts present and non-empty:"
for asset in "$@"; do
    printf '  - %s (%s bytes)\n' "$asset" "$(wc -c <"$asset")"
done
