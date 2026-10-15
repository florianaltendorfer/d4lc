#!/usr/bin/env bash
# scripts/startup_markers.sh — sourced by the release smoke scripts.
#
# The desktop app has no --help/--version, so "it started" is judged on its
# own startup log: every marker below is printed by the app on the way to a
# shown window, and a panic or a refused single-instance guard is a failure.
# Run the app with a timeout and treat exit status 124 (still running when
# the timeout fired) as the survival half of the verdict.

# Required startup lines (extended regexes). Each names the step it proves.
STARTUP_REQUIRED_MARKERS=(
    '\[d4lc\] single-instance: lock (acquired|handed over)'   # guard taken
    '\[d4lc\] tray: '                                          # tray init ran, either outcome
    '\[d4lc\] hotkeys: persistent backend wired'               # shell wiring finished
    '\[d4lc\] window observer: polling WM_STATE'               # window shown on X11
)

# Lines that must never appear in a healthy start.
STARTUP_FORBIDDEN_MARKERS=(
    'panicked at'
    'single-instance: .*refusing to start'
)

# Args: $1 = log path. Prints one line per violated marker and returns 1 when
# any is violated.
check_startup_log() {
    local log_path=$1
    local marker
    local ok=0
    for marker in "${STARTUP_REQUIRED_MARKERS[@]}"; do
        if ! grep -Eq "$marker" "$log_path"; then
            echo "missing startup line: $marker"
            ok=1
        fi
    done
    for marker in "${STARTUP_FORBIDDEN_MARKERS[@]}"; do
        if grep -Eq "$marker" "$log_path"; then
            echo "forbidden startup line present: $marker"
            ok=1
        fi
    done
    return "$ok"
}
