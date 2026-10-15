#!/usr/bin/env bash
# scripts/smoke_release_artifacts.sh — Xvfb-based startup smoke for the
# tarball, .deb, and .AppImage artifacts produced by
# scripts/package_linux_release.sh.
#
# Usage: scripts/smoke_release_artifacts.sh <artifacts-dir> [smoke-output-dir]
#
# Exit status:
#   0 — all detected artifacts started under Xvfb and logged every startup marker,
#       and the debug CLI checks passed
#   1 — any required artifact missing OR any launch or CLI check failed OR no
#       artifact was detected
#
# Debug CLI: the tarball's d4lc-debug runs from a read-only copy of the bundle
# (chmod -R a-w) and the AppImage's through `--debug-cli`, each from a scratch
# working directory with a scratch HOME, XDG_CONFIG_HOME and D4LC_CONFIG_ROOT,
# as an unprivileged user (uid 65534 when this script runs as root). The
# manual-validation dry run must succeed and write below that directory only,
# asset-presets must list the bundled presets, and settings-report must show
# the settings of D4LC_CONFIG_ROOT. The tarball bundle must be unchanged.
#
# Each artifact is started through its packaged launcher wrapper (the one
# that sets the bundled library path and starts in the bundle root). A launch
# passes only when the app was still running when the timeout fired (exit
# status 124) and its log carries every startup line listed in
# scripts/startup_markers.sh. The script never reports success when
# artifacts are missing, when Xvfb is unavailable, or when the app exited
# early or never reached startup. It produces a per-artifact log + screenshot
# under <smoke-output-dir>/<artifact>/.
set -euo pipefail

if [ $# -lt 1 ] || [ $# -gt 2 ]; then
    echo "usage: scripts/smoke_release_artifacts.sh <artifacts-dir> [smoke-output-dir]" >&2
    exit 2
fi

ARTIFACTS_DIR=$1
SMOKE_OUTPUT_DIR=${2:-debug-output/release-smoke}

cd "$(dirname "$0")/.."

# shellcheck source=scripts/startup_markers.sh
source scripts/startup_markers.sh

# Resolve artifacts-dir to an absolute path so the per-artifact extract
# pushd/popd blocks can still reference the source .deb by absolute path.
ARTIFACTS_DIR=$(cd "$ARTIFACTS_DIR" && pwd)
SMOKE_OUTPUT_DIR=$(mkdir -p "$SMOKE_OUTPUT_DIR" && cd "$SMOKE_OUTPUT_DIR" && pwd)

if [ ! -d "$ARTIFACTS_DIR" ]; then
    echo "smoke_release_artifacts: artifacts dir not found: $ARTIFACTS_DIR" >&2
    exit 1
fi

# Prove the tarball, .deb and AppImage can start the
# desktop app under Xvfb. We detect each artifact by extension and skip the
# ones that are absent, but we require AT LEAST ONE to be present and to
# launch successfully.
shopt -s nullglob

tarball=()
deb=()
appimage=()

for candidate in "$ARTIFACTS_DIR"/*.tar.gz "$ARTIFACTS_DIR"/*.tgz; do
    [ -e "$candidate" ] && tarball+=("$candidate")
done
for candidate in "$ARTIFACTS_DIR"/*.deb; do
    [ -e "$candidate" ] && deb+=("$candidate")
done
for candidate in "$ARTIFACTS_DIR"/*.AppImage "$ARTIFACTS_DIR"/*.appimage; do
    [ -e "$candidate" ] && appimage+=("$candidate")
done

total_artifacts=$((${#tarball[@]} + ${#deb[@]} + ${#appimage[@]}))

if [ "$total_artifacts" -eq 0 ]; then
    echo "smoke_release_artifacts: no tarball/.deb/.AppImage artifacts found under $ARTIFACTS_DIR" >&2
    exit 1
fi

echo "smoke_release_artifacts: detected ${#tarball[@]} tarball(s), ${#deb[@]} .deb(s), ${#appimage[@]} AppImage(s) under $ARTIFACTS_DIR"

require_command() {
    local command_name=$1
    if ! command -v "$command_name" >/dev/null 2>&1; then
        echo "smoke_release_artifacts: required command not found: $command_name" >&2
        return 1
    fi
}

require_command xvfb-run || {
    echo "smoke_release_artifacts: xvfb-run is required for the startup smoke" >&2
    exit 1
}

SMOKE_BIN_TIMEOUT=${D4LC_SMOKE_TIMEOUT_SECS:-8}
SMOKE_RESULTS_FILE="$SMOKE_OUTPUT_DIR/smoke-results.txt"
: >"$SMOKE_RESULTS_FILE"

declare -i FAILED=0
declare -i SKIPPED=0
declare -i PASSED=0
declare -i CLI_PASSED=0

# Launch helper: run a launcher under Xvfb with a timeout, log stdout/stderr,
# attempt a screenshot of the X display, and write a status line.
# Args:
#   $1 = artifact_name  (e.g. "d4lc-0.1.0-x86_64.tar.gz")
#   $2 = artifact_kind  ("tarball" | "deb" | "appimage")
#   $3 = launcher_path  (absolute path to the packaged launcher wrapper)
#   $4 = work_dir       (where to place log + screenshot)
launch_under_xvfb() {
    local artifact_name=$1
    local artifact_kind=$2
    local launcher_path=$3
    local work_dir=$4
    local log_path="$work_dir/launch.log"
    local screenshot_path="$work_dir/screenshot.png"
    local xwd_path="$work_dir/screenshot.xwd"
    local status_line="artifact=$artifact_name kind=$artifact_kind"

    mkdir -p "$work_dir"

    if [ ! -x "$launcher_path" ]; then
        echo "$status_line status=failed reason=launcher-not-executable path=$launcher_path" | tee -a "$SMOKE_RESULTS_FILE"
        FAILED+=1
        return
    fi

    # The app keeps its config, its single-instance lock and its show-request
    # socket in per-run directories, so a smoke run never touches (or hands
    # its launch over to) an app instance running on this machine. The
    # session backend is X11 under Xvfb, so a Wayland session is hidden.
    local runtime_dir="$work_dir/runtime"
    local config_dir="$work_dir/config"
    rm -rf "$runtime_dir" "$config_dir"
    mkdir -p "$runtime_dir" "$config_dir"
    chmod 0700 "$runtime_dir"

    # Slint GUI apps do not exit on their own: `timeout` bounds the run and
    # exit status 124 means the app was still running when it fired. Any
    # other status is an early exit or a crash.
    set +e
    # xvfb-run -a picks a free display itself; the wrapper records the one it
    # exported so the screenshot below targets exactly this server instead of
    # guessing from the lock files other X servers leave behind.
    local display_path="$work_dir/display"
    rm -f "$display_path"
    DISPLAY= xvfb-run -a \
        sh -c 'printf "%s\n" "$DISPLAY" >"$1"; shift; exec "$@"' sh "$display_path" \
        env -u WAYLAND_DISPLAY \
            XDG_RUNTIME_DIR="$runtime_dir" \
            D4LC_CONFIG_ROOT="$config_dir" \
            APPIMAGE_EXTRACT_AND_RUN=1 \
        timeout --kill-after=2s "${SMOKE_BIN_TIMEOUT}s" \
        "$launcher_path" \
        >"$log_path" 2>&1 &
    local pid=$!
    # Best-effort screenshot: try ImageMagick `import`, then xwd. Wait for the
    # wrapper to report the display, then give the app a moment to map.
    local display_num=""
    for _ in $(seq 1 100); do
        if [ -s "$display_path" ]; then
            display_num="$(sed -E 's/^[^[:digit:]]*([[:digit:]]+).*$/\1/' "$display_path" | head -n 1)"
            break
        fi
        sleep 0.05
    done
    sleep 1
    if [ -n "$display_num" ] && [ -S "/tmp/.X11-unix/X$display_num" ]; then
        if command -v import >/dev/null 2>&1; then
            DISPLAY=":$display_num" import -window root "$screenshot_path" 2>/dev/null || true
        elif command -v xwd >/dev/null 2>&1; then
            DISPLAY=":$display_num" xwd -root -silent >"$xwd_path" 2>/dev/null || true
            if [ -s "$xwd_path" ] && command -v convert >/dev/null 2>&1; then
                convert "$xwd_path" "$screenshot_path" 2>/dev/null || true
            fi
        fi
    fi
    local run_status=0
    wait "$pid" 2>/dev/null
    run_status=$?
    set -e

    # Verdict: still running at the timeout AND every startup marker logged.
    local problems=""
    if [ "$run_status" -ne 124 ]; then
        problems="exited-with-status-$run_status-before-the-${SMOKE_BIN_TIMEOUT}s-timeout"
    fi
    local marker_problems
    marker_problems=$(check_startup_log "$log_path" || true)
    if [ -n "$marker_problems" ]; then
        problems="${problems:+$problems; }$(echo "$marker_problems" | tr '\n' ';')"
    fi

    if [ -z "$problems" ]; then
        echo "$status_line status=passed log=$log_path screenshot=$screenshot_path" | tee -a "$SMOKE_RESULTS_FILE"
        PASSED+=1
        return
    fi

    echo "$status_line status=failed reason=$problems log=$log_path" | tee -a "$SMOKE_RESULTS_FILE"
    FAILED+=1
}

# Assert on the staged bundle itself: libraries tied to the host must not be
# bundled (a bundled copy would shadow the host's through the wrappers'
# LD_LIBRARY_PATH; keep this list in step with bundle_shared_libraries in
# scripts/package_linux_release.sh), and every bundled library needs a
# copyright file under docs/third-party/.
HOST_TIED_LIB_RE='^(libpipewire|libspa|libstdc\+\+|libgcc_s|libdbus|libsystemd|libudev|libGL|libEGL|libvulkan|libwayland|libX11|libxcb)'

# Args: $1 = artifact_name, $2 = artifact_kind, $3 = bundle root (the folder
# holding bin/, lib/ and docs/). Returns 1 and counts a failure on a violation.
check_bundle_contents() {
    local artifact_name=$1
    local artifact_kind=$2
    local bundle_root=$3
    local status_line="artifact=$artifact_name kind=$artifact_kind"
    local lib_dir="$bundle_root/lib"
    local host_tied

    [ -d "$lib_dir" ] || return 0
    host_tied=$(ls -1 "$lib_dir" | grep -E "$HOST_TIED_LIB_RE" | tr '\n' ',' || true)
    if [ -n "$host_tied" ]; then
        echo "$status_line status=failed reason=host-tied-libraries-bundled libs=$host_tied" | tee -a "$SMOKE_RESULTS_FILE"
        FAILED+=1
        return 1
    fi
    if [ -n "$(ls -A "$lib_dir")" ] && ! ls "$bundle_root"/docs/third-party/*.copyright >/dev/null 2>&1; then
        echo "$status_line status=failed reason=bundled-libraries-without-copyright-files" | tee -a "$SMOKE_RESULTS_FILE"
        FAILED+=1
        return 1
    fi
    return 0
}

# Listing of a tree that changes when anything below it is written.
tree_digest() {
    find "$1" -printf '%P %y %s %T@\n' | LC_ALL=C sort | sha256sum
}

# Run the debug CLI the way a user does and check what it reads and writes.
# Args: $1 = artifact_name, $2 = artifact_kind, $3 = work_dir, $4 = bundle
# root that must stay unchanged (empty: not checked), $5... = the command
# that starts the packaged debug CLI.
smoke_debug_cli() {
    local artifact_name=$1
    local artifact_kind=$2
    local cli_dir="$3/debug-cli"
    local bundle_root=$4
    shift 4
    local status_line="artifact=$artifact_name kind=$artifact_kind check=debug-cli"
    local cwd="$cli_dir/cwd" home="$cli_dir/home" config="$cli_dir/config" tmp="$cli_dir/tmp"
    local problems="" digest_before="" output
    local as_user=()

    rm -rf "$cli_dir"
    mkdir -p "$cwd" "$home" "$config/Config" "$tmp"
    # Root writes through read-only permissions; the CLI runs as nobody then.
    if [ "$(id -u)" -eq 0 ]; then
        chown -R 65534:65534 "$cli_dir"
        as_user=(setpriv --reuid=65534 --regid=65534 --clear-groups)
    fi
    [ -z "$bundle_root" ] || digest_before=$(tree_digest "$bundle_root")

    # Prints the CLI's stdout; stderr goes to the log.
    run_cli() {
        (cd "$cwd" && "${as_user[@]}" env -u DISPLAY -u WAYLAND_DISPLAY -u D4LC_ASSET_ROOT \
            HOME="$home" XDG_CONFIG_HOME="$home/.config" D4LC_CONFIG_ROOT="$config" \
            TMPDIR="$tmp" APPIMAGE_EXTRACT_AND_RUN=1 "$@") 2>>"$cli_dir/cli.log"
    }

    if ! output=$(run_cli "$@" manual-diablo-validation-bundle --dry-run); then
        problems="dry-run-failed"
    elif ! grep -q '"status": "dry_run_ready"' <<<"$output"; then
        problems="dry-run-not-ready"
    elif ! ls "$cwd"/debug-output/manual-diablo-iv/run-*/bundle-summary.json >/dev/null 2>&1; then
        problems="dry-run-bundle-not-below-the-working-directory"
    fi

    if ! output=$(run_cli "$@" asset-presets) || ! grep -q '"name": "1080p_SMF"' <<<"$output"; then
        problems="${problems:+$problems;}asset-presets-did-not-list-the-bundled-presets"
    fi

    printf '{ "SelectedSystemPreset": "2160p_SSF", "SelectedAffixLanguage": "deDE" }\n' \
        >"$config/Config/Settings.json"
    if ! output=$(run_cli "$@" settings-report) \
        || ! grep -q '"selected_system_preset": "2160p_SSF"' <<<"$output" \
        || ! grep -q '"selected_affix_language": "deDE"' <<<"$output" \
        || ! grep -q '"config_root_source": "D4LC_CONFIG_ROOT"' <<<"$output"; then
        problems="${problems:+$problems;}settings-report-ignored-D4LC_CONFIG_ROOT"
    fi

    if [ -n "$bundle_root" ] && [ "$(tree_digest "$bundle_root")" != "$digest_before" ]; then
        problems="${problems:+$problems;}the-bundle-changed"
    fi

    if [ -z "$problems" ]; then
        echo "$status_line status=passed log=$cli_dir/cli.log" | tee -a "$SMOKE_RESULTS_FILE"
        CLI_PASSED+=1
        return
    fi
    echo "$status_line status=failed reason=$problems log=$cli_dir/cli.log" | tee -a "$SMOKE_RESULTS_FILE"
    FAILED+=1
}

# ── Smoke the tarball ──────────────────────────────────────────────────
for tar in "${tarball[@]}"; do
    name=$(basename "$tar")
    work_dir="$SMOKE_OUTPUT_DIR/${name}.smoke"
    extract_dir="$work_dir/extract"
    mkdir -p "$extract_dir"
    if ! tar -C "$extract_dir" -xzf "$tar"; then
        echo "artifact=$name kind=tarball status=failed reason=extract-failed" | tee -a "$SMOKE_RESULTS_FILE"
        FAILED+=1
        continue
    fi
    check_bundle_contents "$name" "tarball" "$extract_dir/d4lc" || continue
    # The packaged launcher wrapper, not the -real payload: it is what users
    # run and it sets the bundled library path and the working directory.
    launcher="$extract_dir/d4lc/bin/d4lc"
    if [ ! -x "$launcher" ]; then
        echo "artifact=$name kind=tarball status=failed reason=no-launcher-wrapper path=$launcher" | tee -a "$SMOKE_RESULTS_FILE"
        FAILED+=1
        continue
    fi
    launch_under_xvfb "$name" "tarball" "$launcher" "$work_dir"
    # The debug CLI from a read-only bundle, as installed for another user.
    bundle="$extract_dir/d4lc"
    chmod -R a-w "$bundle"
    smoke_debug_cli "$name" "tarball" "$work_dir" "$bundle" "$bundle/bin/d4lc-debug"
    chmod -R u+w "$bundle"
done

# ── Smoke the .deb ────────────────────────────────────────────────────
for d in "${deb[@]}"; do
    name=$(basename "$d")
    work_dir="$SMOKE_OUTPUT_DIR/${name}.smoke"
    extract_dir="$work_dir/extract"
    mkdir -p "$extract_dir"
    # We prefer `dpkg-deb -x` for a clean extraction; fall back to `ar`+`tar`
    # if dpkg-deb is not available (Arch-based local dev hosts).
    if command -v dpkg-deb >/dev/null 2>&1; then
        if ! dpkg-deb -x "$d" "$extract_dir"; then
            echo "artifact=$name kind=deb status=failed reason=dpkg-deb-extract-failed" | tee -a "$SMOKE_RESULTS_FILE"
            FAILED+=1
            continue
        fi
    elif command -v ar >/dev/null 2>&1; then
        pushd "$extract_dir" >/dev/null
        if ! ar x "$d"; then
            popd >/dev/null
            echo "artifact=$name kind=deb status=failed reason=ar-extract-failed" | tee -a "$SMOKE_RESULTS_FILE"
            FAILED+=1
            continue
        fi
        popd >/dev/null
        if ! tar -xf "$extract_dir/data.tar.zst" -C "$extract_dir" 2>/dev/null \
            && ! tar -xf "$extract_dir/data.tar.xz" -C "$extract_dir" 2>/dev/null \
            && ! tar -xf "$extract_dir/data.tar.gz" -C "$extract_dir" 2>/dev/null \
            && ! tar -xf "$extract_dir/data.tar" -C "$extract_dir" 2>/dev/null; then
            echo "artifact=$name kind=deb status=failed reason=data-tar-extract-failed" | tee -a "$SMOKE_RESULTS_FILE"
            FAILED+=1
            continue
        fi
    else
        echo "artifact=$name kind=deb status=skipped reason=neither-dpkg-deb-nor-ar-available" | tee -a "$SMOKE_RESULTS_FILE"
        SKIPPED+=1
        continue
    fi
    check_bundle_contents "$name" "deb" "$extract_dir/opt/d4lc" || continue
    # /usr/bin/d4lc is hard-wired to /opt/d4lc, which does not exist in the
    # extracted tree; the bundle's own relocatable launcher is the same
    # wrapper logic and runs from where it was extracted. The installed
    # /usr/bin/d4lc is exercised by scripts/smoke_deb_install.sh.
    launcher="$extract_dir/opt/d4lc/bin/d4lc"
    if [ ! -x "$launcher" ]; then
        echo "artifact=$name kind=deb status=failed reason=no-launcher-wrapper path=$launcher" | tee -a "$SMOKE_RESULTS_FILE"
        FAILED+=1
        continue
    fi
    launch_under_xvfb "$name" "deb" "$launcher" "$work_dir"
done

# ── Smoke the AppImage ────────────────────────────────────────────────
for ai in "${appimage[@]}"; do
    name=$(basename "$ai")
    work_dir="$SMOKE_OUTPUT_DIR/${name}.smoke"
    mkdir -p "$work_dir"
    launch_under_xvfb "$name" "appimage" "$ai" "$work_dir"
    smoke_debug_cli "$name" "appimage" "$work_dir" "" "$ai" --debug-cli
done

echo "summary: passed=$PASSED failed=$FAILED skipped=$SKIPPED total=$total_artifacts debug_cli_passed=$CLI_PASSED" | tee -a "$SMOKE_RESULTS_FILE"

# Any failed launch, or no launch at all, fails the release workflow.
if [ "$FAILED" -gt 0 ] || [ "$PASSED" -eq 0 ]; then
    exit 1
fi

exit 0
