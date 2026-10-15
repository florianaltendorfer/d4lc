#!/usr/bin/env bash
# Real-browser integration tests of the build imports, reported one by one.
#
# Tier 1 (always): a real Chromium driven over CDP against loopback test
# pages (first response, deadline on a busy page, browser dying mid-load).
# No network access. A missing browser fails this script: it is reported as
# NOT RUN, never as passed.
#
# Tier 2 (opt-in): the live provider smokes against the real sites. Each runs
# only when its variable names a real build (see the test docs in
# crates/d4lc-app/src/actions/build_download/tests.rs):
#   D4LC_D2CORE_SMOKE_ID, D4LC_D4BUILDS_SMOKE_ID, D4LC_MOBALYTICS_SMOKE_URL.
# An unset variable is reported as NOT RUN.
set -euo pipefail
cd "$(dirname "$0")/.."

loopback_tests=(
    actions::build_download::tests::browser_captures_the_initial_json_response
    actions::build_download::tests::browser_collection_ends_at_the_deadline_on_a_page_that_never_goes_quiet
    actions::build_download::tests::a_browser_that_dies_while_loading_fails_the_collection
    actions::build_download::tests::browser_returns_the_accepted_response_without_waiting_for_load_or_quiet
    actions::build_download::tests::browser_launch_flags_reach_chromium_unmangled
    actions::build_download::tests::cancel_ends_a_running_collection_and_the_browser
    actions::build_download::tests::a_load_event_that_never_fires_is_no_navigation_failure
)

browser=""
if [ -n "${D4LC_CHROME_BIN:-}" ]; then
    browser=$D4LC_CHROME_BIN
else
    for candidate in chromium chromium-browser google-chrome google-chrome-stable; do
        if command -v "$candidate" >/dev/null 2>&1; then
            browser=$candidate
            break
        fi
    done
fi
if [ -z "$browser" ]; then
    echo "[browser] NOT RUN: no Chromium/Chrome found (install one or set D4LC_CHROME_BIN)" >&2
    exit 3
fi
echo "[browser] browser: $browser ($("$browser" --version 2>/dev/null || echo "version unknown"))"

run_ignored() {
    local log=$1
    shift
    cargo test -q -p d4lc-app --bin d4lc-app -- --ignored --exact "$@" >"$log" 2>&1
}

log_dir=target/browser-integration
mkdir -p "$log_dir"
if ! run_ignored "$log_dir/loopback.log" "${loopback_tests[@]}"; then
    cat "$log_dir/loopback.log" >&2
    echo "[browser] FAILED: loopback real-browser tests" >&2
    exit 1
fi
expected="test result: ok. ${#loopback_tests[@]} passed"
if ! grep -q "$expected" "$log_dir/loopback.log"; then
    cat "$log_dir/loopback.log" >&2
    echo "[browser] FAILED: expected '$expected'" >&2
    exit 1
fi
for name in "${loopback_tests[@]}"; do
    echo "[browser] PASSED (real Chromium, loopback): ${name##*::}"
done

live_failed=0
run_live() {
    local variable=$1 test=$2
    if [ -z "${!variable:-}" ]; then
        echo "[browser] NOT RUN (live provider): $test ($variable unset)"
        return
    fi
    if run_ignored "$log_dir/$test.log" "actions::build_download::tests::$test"; then
        echo "[browser] PASSED (live provider): $test"
    else
        echo "[browser] FAILED (live provider): $test, see $log_dir/$test.log" >&2
        live_failed=1
    fi
}
run_live D4LC_D2CORE_SMOKE_ID d2core_live_download_smoke
run_live D4LC_D4BUILDS_SMOKE_ID d4builds_live_download_smoke
run_live D4LC_MOBALYTICS_SMOKE_URL mobalytics_live_download_smoke
exit "$live_failed"
