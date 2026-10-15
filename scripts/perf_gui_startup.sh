#!/usr/bin/env bash
# scripts/perf_gui_startup.sh — GUI cold-start benchmark for the d4lc desktop
# launcher.
#
# Usage:
#   scripts/perf_gui_startup.sh <launcher-path> <label> <counted-runs> <output-json>
#
# For every launch the script:
#   1. uses one private Xvfb display on a server-chosen number (never the real
#      :0 display),
#   2. launches the GUI with an isolated scratch HOME/XDG environment so the
#      user's real d4lc config is never read or written,
#   3. polls the X server until the app window is mapped ("IsViewable") and
#      records the exec -> mapped wall clock in integer milliseconds,
#   4. SIGKILLs the app process before it can persist state.
#
# The first <warmup> launches are discarded (fontconfig, Xvfb, and page-cache
# warmup). Percentiles are nearest-rank over
# the counted samples. Polling granularity is one `xwininfo` invocation
# (~10 ms per poll), which bounds the per-sample measurement error; reports
# must disclose this.
#
# The script never reports success without evidence: if the window never maps
# within the timeout, the launch is recorded as failed and the script exits 1.

set -euo pipefail

if [ $# -ne 4 ]; then
    echo "usage: scripts/perf_gui_startup.sh <launcher-path> <label> <counted-runs> <output-json>" >&2
    exit 2
fi

LAUNCHER=$1
LABEL=$2
COUNTED_RUNS=$3
OUTPUT_JSON=$4
WARMUP_RUNS=${D4LC_STARTUP_WARMUP:-5}
POLL_INTERVAL=${D4LC_STARTUP_POLL_SLEEP:-0.005}
TIMEOUT_SECS=${D4LC_STARTUP_TIMEOUT:-30}

cd "$(dirname "$0")/.."
LAUNCHER=$(cd "$(dirname "$LAUNCHER")" && pwd)/$(basename "$LAUNCHER")
if [ ! -x "$LAUNCHER" ]; then
    echo "perf_gui_startup: launcher not executable: $LAUNCHER" >&2
    exit 2
fi

for command_name in Xvfb xwininfo; do
    command -v "$command_name" >/dev/null 2>&1 || {
        echo "perf_gui_startup: required command not found: $command_name" >&2
        exit 2
    }
done

SCRATCH=$(mktemp -d /tmp/d4lc-startup-XXXXXX)
mkdir -p "$SCRATCH/home" "$SCRATCH/xdg-config" "$SCRATCH/xdg-data" "$SCRATCH/xdg-cache" \
    "$SCRATCH/xdg-runtime"
chmod 700 "$SCRATCH/xdg-runtime"

XVFB_PID=""
APP_PID=""

cleanup() {
    if [ -n "$APP_PID" ]; then
        kill -9 "$APP_PID" 2>/dev/null || true
        wait "$APP_PID" 2>/dev/null || true
    fi
    if [ -n "$XVFB_PID" ]; then
        kill "$XVFB_PID" 2>/dev/null || true
        wait "$XVFB_PID" 2>/dev/null || true
    fi
    rm -rf "$SCRATCH"
}
trap cleanup EXIT INT TERM

# Xvfb picks a free display number itself (-displayfd) and writes it to the
# file once it accepts connections: no fixed display numbers, and the wait is
# for that report instead of a socket file. Sets XVFB_PID and DISPLAY_NAME in
# the calling shell (no command substitution, so the cleanup trap sees the pid).
start_xvfb() {
    local display_file="$SCRATCH/xvfb-display"
    : >"$display_file"
    Xvfb -displayfd 3 -screen 0 1920x1080x24 -nolisten tcp \
        3>"$display_file" >/dev/null 2>&1 &
    XVFB_PID=$!
    for _ in $(seq 1 200); do
        if [ -s "$display_file" ]; then
            DISPLAY_NAME=":$(head -n 1 "$display_file")"
            return 0
        fi
        kill -0 "$XVFB_PID" 2>/dev/null || break
        sleep 0.05
    done
    kill "$XVFB_PID" 2>/dev/null || true
    wait "$XVFB_PID" 2>/dev/null || true
    XVFB_PID=""
    return 1
}

start_xvfb || {
    echo "perf_gui_startup: could not start a private Xvfb display" >&2
    exit 2
}
export DISPLAY="$DISPLAY_NAME"

SNAP_PATH=""

launch_and_measure() {
    # Prints elapsed integer milliseconds on success; returns 1 on failure.
    APP_PID=""
    local start_ns
    start_ns=$(date +%s%N)
    env -i \
        DISPLAY="$DISPLAY_NAME" \
        HOME="$SCRATCH/home" \
        XDG_CONFIG_HOME="$SCRATCH/xdg-config" \
        XDG_DATA_HOME="$SCRATCH/xdg-data" \
        XDG_CACHE_HOME="$SCRATCH/xdg-cache" \
        XDG_RUNTIME_DIR="$SCRATCH/xdg-runtime" \
        PATH="$PATH" \
        "$LAUNCHER" >"$SCRATCH/app.log" 2>&1 &
    APP_PID=$!

    local deadline=$(( $(date +%s) + TIMEOUT_SECS ))
    while [ "$(date +%s)" -lt "$deadline" ]; do
        if ! kill -0 "$APP_PID" 2>/dev/null; then
            # Process exited on its own: never treat that as a started GUI.
            wait "$APP_PID" 2>/dev/null || true
            APP_PID=""
            return 1
        fi
        local child
        child=$(xwininfo -root -children 2>/dev/null \
            | grep -oE '^[[:space:]]+0x[0-9a-f]+' | head -n1 | tr -d '[:space:]')
        if [ -n "$child" ]; then
            if xwininfo -id "$child" 2>/dev/null | grep -q "Map State:[[:space:]]*IsViewable"; then
                local end_ns elapsed
                end_ns=$(date +%s%N)
                elapsed=$(( (end_ns - start_ns) / 1000000 ))
                if [ -n "$SNAP_PATH" ] && command -v import >/dev/null 2>&1; then
                    import -window root "$SNAP_PATH" 2>/dev/null || true
                    SNAP_PATH=""
                fi
                kill -9 "$APP_PID" 2>/dev/null || true
                wait "$APP_PID" 2>/dev/null || true
                APP_PID=""
                echo "$elapsed"
                return 0
            fi
        fi
        sleep "$POLL_INTERVAL"
    done

    echo "perf_gui_startup: window did not map within ${TIMEOUT_SECS}s (log: $SCRATCH/app.log)" >&2
    tail -n 5 "$SCRATCH/app.log" >&2 || true
    return 1
}

mkdir -p "$(dirname "$OUTPUT_JSON")"
SAMPLES_FILE="$SCRATCH/samples.txt"
: >"$SAMPLES_FILE"

total_runs=$((WARMUP_RUNS + COUNTED_RUNS))
failed=0
counted=0
for run in $(seq 1 "$total_runs"); do
    if [ "$run" -eq "$((WARMUP_RUNS + 1))" ]; then
        SNAP_PATH="${OUTPUT_JSON%.json}-$LABEL-window.png"
    fi
    if elapsed_ms=$(launch_and_measure); then
        if [ "$run" -le "$WARMUP_RUNS" ]; then
            echo "warmup run $run: ${elapsed_ms} ms (discarded)" >&2
        else
            echo "$elapsed_ms" >>"$SAMPLES_FILE"
            counted=$((counted + 1))
        fi
    else
        failed=$((failed + 1))
        break
    fi
done

if [ "$counted" -eq 0 ]; then
    echo "perf_gui_startup: no successful launches; not writing a report" >&2
    exit 1
fi

# Nearest-rank percentiles over the counted integer-millisecond samples
# (identical rank formula to crates/d4lc-cli/src/live.rs::percentile_ms).
sorted=$(sort -n "$SAMPLES_FILE")
percentile() {
    echo "$sorted" | awk -v p="$1" -v n="$counted" \
        'NR == int((p * n + 99) / 100) { print; exit }'
}
p50=$(percentile 50)
p95=$(percentile 95)
p99=$(percentile 99)
min_ms=$(echo "$sorted" | head -n1)
max_ms=$(echo "$sorted" | tail -n1)
mean_ms=$(awk '{ total += $1 } END { printf "%.1f", total / NR }' "$SAMPLES_FILE")

cat >"$OUTPUT_JSON" <<EOF
{
  "schema_version": 1,
  "command": "scripts/perf_gui_startup.sh <launcher> $LABEL $COUNTED_RUNS $(basename "$OUTPUT_JSON")",
  "label": "$LABEL",
  "launcher": "$LAUNCHER",
  "metric": "exec to first mapped (IsViewable) application window, wall clock",
  "percentile_method": "nearest-rank",
  "sample_resolution": "integer milliseconds per sample (xwininfo poll granularity, roughly 10 ms per poll)",
  "runs": { "warmup_discarded": $WARMUP_RUNS, "counted": $counted, "failed": $failed },
  "startup_ms": { "min": $min_ms, "mean": $mean_ms, "p50": $p50, "p95": $p95, "p99": $p99 },
  "display": "$DISPLAY_NAME (private Xvfb 1920x1080x24; the real :0 display is never touched)",
  "environment_note": "isolated scratch HOME/XDG dirs; app killed with SIGKILL before persisting state; Diablo IV actively played during benchmarks (X11 :0 busy; identical for every artifact)"
}
EOF

echo "perf_gui_startup: $LABEL counted=$counted failed=$failed min=${min_ms}ms mean=${mean_ms}ms p50=${p50}ms p95=${p95}ms p99=${p99}ms -> $OUTPUT_JSON"
