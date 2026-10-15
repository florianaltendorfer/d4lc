#!/usr/bin/env bash
set -euo pipefail

# Evaluates scripts/verify_visual_smoke.sh against hand-made artifact
# directories: a passed report succeeds, a skip is reported as SKIPPED and
# fails, and a missing report fails.

cd "$(dirname "$0")/.."

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fail() {
    echo "test_visual_smoke_status: $*" >&2
    exit 1
}

run_smoke() {
    D4LC_VISUAL_SMOKE_EVALUATE_ONLY=1 D4LC_VISUAL_SMOKE_DIR="$1" \
        ./scripts/verify_visual_smoke.sh
}

passed="$work/passed"
mkdir -p "$passed"
{
    echo "status=passed"
    echo "placeholder_check=passed"
    for index in 1 2 3 4 5 6; do echo "capture=$index.png"; done
} >"$passed/visual-smoke-report.txt"
run_smoke "$passed" >/dev/null || fail "a passed report must succeed"

skipped="$work/skipped"
mkdir -p "$skipped"
echo "no display available" >"$skipped/skip-report.txt"
status=0
output=$(run_smoke "$skipped") || status=$?
[ "$status" -eq 3 ] || fail "a skip must exit with status 3, got $status"
case "$output" in
    *SKIPPED*"no display available"*) ;;
    *) fail "a skip must be reported as SKIPPED with its reason, got: $output" ;;
esac

empty="$work/empty"
mkdir -p "$empty"
status=0
run_smoke "$empty" >/dev/null || status=$?
[ "$status" -eq 1 ] || fail "a missing report must exit with status 1, got $status"

echo "test_visual_smoke_status: ok"
