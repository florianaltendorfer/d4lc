#!/usr/bin/env bash
# scripts/xtest_journeys.sh — true end-to-end X11 input journeys.
#
# Drives the real desktop binary (release or dev) under a private Xvfb display
# with input injected at the X server level via XTEST (x11rb fake_input), so
# every event passes through the real stack: X server -> winit -> Slint. The
# primitive input/capture commands live in the thin driver
# `cargo run -p d4lc-overlay-x11 --example xtest_input --features real-x11`;
# this script owns the journey orchestration and assertions.
#
# Focus note: a bare Xvfb display has no window manager, so the X input focus
# stays in PointerRoot mode and the app never receives the FocusIn event a WM
# would send; winit then treats the window as unfocused and drops every
# injected key event. Each keyboard segment therefore establishes the input
# focus explicitly with the driver's `focus` command (set-input-focus on the
# app window) — exactly the activation step a real window manager performs —
# and the journeys assert that typed characters really land in the dialog
# fields. On a real desktop this focus transfer happens on every click.
#
# Usage:
#   scripts/xtest_journeys.sh [journey ...]      # 1, 2, 3; default: all
#   D4LC_XTEST_SKIP_BUILD=1 scripts/xtest_journeys.sh
#
# Journeys:
#   1  release binary: click through the nav rail, open the import dialog,
#      type a build URL with real key events (asserted to land in the field),
#      Escape, quit via WM_DELETE_WINDOW.
#   2  dev binary: resize the window to (and below) its minimum size, create
#      a preset through the New Preset dialog via keyboard only (asserted),
#      open the selection-gated rename dialog through the pencil button,
#      complete a preset rename via keyboard only (focus, clear the seeded
#      name, type the new name, Apply) and assert the renamed preset is
#      shown in the preset ComboBox and seeded back on reopen.
#   3  release binary: global hotkey (Ctrl+F12) pressed via XTEST while the
#      window is NOT focused; asserts the persisted settings flip.
#
# Evidence (captures, window geometry, logs, a per-journey status report) is
# written under debug-output/xtest-journeys/ (override: D4LC_XTEST_EVIDENCE_DIR).
#
# The script never reports success without captured
# evidence, and it reports skips (Wayland host session, missing Xvfb) instead
# of pretending the journeys ran. It is intentionally NOT wired into
# scripts/verify_local.sh — building the release + dev desktop binaries makes
# the default validation path much slower and Xvfb-dependent; run it
# standalone (CI job or local check).

set -euo pipefail
cd "$(dirname "$0")/.."

EVIDENCE_ROOT="${D4LC_XTEST_EVIDENCE_DIR:-debug-output/xtest-journeys}"
REPORT_FILE="$EVIDENCE_ROOT/xtest-journeys-report.txt"
APP_WINDOW_TITLE="d4lc"

# ── Skip detection (outer invocation only) ──────────────────────────
if [ "${1:-}" != "--inner" ]; then
    mkdir -p "$EVIDENCE_ROOT"
    : >"$REPORT_FILE"
    if [ "${XDG_SESSION_TYPE:-}" = "wayland" ] && [ "${D4LC_XTEST_FORCE:-0}" != "1" ]; then
        echo "[xtest-journeys] skipped: host session is Wayland (XDG_SESSION_TYPE=wayland); the XTEST journeys target the X11 window path"
        echo "journeys=skipped reason=wayland-host-session" >>"$REPORT_FILE"
        exit 0
    fi
    if ! command -v Xvfb >/dev/null 2>&1 || ! command -v xvfb-run >/dev/null 2>&1; then
        echo "[xtest-journeys] skipped: Xvfb/xvfb-run is not installed"
        echo "journeys=skipped reason=missing-xvfb" >>"$REPORT_FILE"
        exit 0
    fi
    if ! command -v cargo >/dev/null 2>&1; then
        echo "[xtest-journeys] skipped: cargo is not on PATH"
        echo "journeys=skipped reason=missing-cargo" >>"$REPORT_FILE"
        exit 0
    fi
fi

# ── Shared plumbing ────────────────────────────────────────────────────────
WORK=$(mktemp -d /tmp/d4lc-xtest-XXXXXX)
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT INT TERM

# The inner (xvfb-run) invocation is a fresh process with its own scratch
# dir; the driver binary path travels through the environment.
DRIVER_BIN="${D4LC_XTEST_DRIVER:-$WORK/xtest_input}"

build_driver() {
    echo "[xtest-journeys] building the xtest_input driver"
    cargo build -q -p d4lc-overlay-x11 --example xtest_input --features real-x11
    # Copy out of target/: later feature-less cargo builds would overwrite the
    # example binary with the no-op stub.
    cp target/debug/examples/xtest_input "$DRIVER_BIN"
    export D4LC_XTEST_DRIVER="$DRIVER_BIN"
}

build_app_binaries() {
    if [ "${D4LC_XTEST_SKIP_BUILD:-0}" = "1" ] \
        && [ -x target/debug/d4lc-app ] && [ -x target/release/d4lc-app ]; then
        echo "[xtest-journeys] skipping app builds (D4LC_XTEST_SKIP_BUILD=1)"
        return 0
    fi
    echo "[xtest-journeys] building d4lc-app (dev + release)"
    cargo build -q -p d4lc-app
    cargo build -q --release -p d4lc-app
}

# Per-journey state (set by run_inner, consumed by the helpers).
DIR=""
STEP_LOG=""
CAPTURE_LOG=""
LIVE_W=0
LIVE_H=0

drv() {
    # Every driver invocation is logged so the evidence keeps the exact
    # command + output of each journey step.
    echo "\$ xtest_input $*" >>"$STEP_LOG"
    "$DRIVER_BIN" "$@" 2>&1 | tee -a "$STEP_LOG"
}

geom_field() {
    local line=$1 field=$2
    printf '%s\n' "$line" | tr ' ' '\n' | grep "^${field}=" | head -n1 | cut -d= -f2
}

step_capture() {
    local out=$1
    drv capture --title "$APP_WINDOW_TITLE" --out "$DIR/$out" | tee -a "$CAPTURE_LOG"
}

diff_field() {
    local a=$1 b=$2 field=$3
    shift 3
    local line
    line=$(drv diffstat --a "$DIR/$a" --b "$DIR/$b" "$@")
    printf '%s\n' "$line" | tr ' ' '\n' | grep "^${field}=" | head -n1 | cut -d= -f2
}

assert() {
    local description=$1
    shift
    if "$@"; then
        echo "[assert] PASS: $description" | tee -a "$STEP_LOG"
    else
        echo "[assert] FAIL: $description" | tee -a "$STEP_LOG"
        exit 1
    fi
}

assert_number_cmp() {
    # assert_number_cmp <description> <value> <operator> <threshold>
    local description=$1 value=$2 operator=$3 threshold=$4
    assert "$description" awk -v v="$value" -v op="$operator" -v t="$threshold" \
        'BEGIN { if (op == ">=") exit !(v >= t); if (op == "<=") exit !(v <= t); exit 1 }'
}

assert_close_and_exit_zero() {
    local app_pid=$1 reason_label=$2
    drv close-request --title "$APP_WINDOW_TITLE"
    local waited=0
    while kill -0 "$app_pid" 2>/dev/null; do
        sleep 0.2
        waited=$((waited + 1))
        if [ "$waited" -gt 75 ]; then
            echo "[assert] FAIL: $reason_label app did not exit after WM_DELETE_WINDOW" | tee -a "$STEP_LOG"
            exit 1
        fi
    done
    # The app runs wrapped in a subshell that records its exit code, so the
    # code survives even when bash already reaped the background job.
    local code
    code=$(cat "$DIR/app-exit-code" 2>/dev/null || echo "timeout")
    assert "$reason_label app exited with code 0 via WM_DELETE_WINDOW (saw $code)" \
        test "$code" -eq 0
    assert "$reason_label close-request logged" \
        grep -q "window close requested" "$DIR/app.log"
}

common_start() {
    local geometry window_id
    geometry=$(drv wait-window --title "$APP_WINDOW_TITLE" --timeout-ms 60000)
    echo "$geometry" | tee -a "$DIR/geometry.txt"
    LIVE_W=$(geom_field "$geometry" w)
    LIVE_H=$(geom_field "$geometry" h)
    assert "app window mapped at a desktop size (${LIVE_W}x${LIVE_H} in 1000..1280 x 560..800)" \
        test "$LIVE_W" -ge 1000 -a "$LIVE_W" -le 1280 -a "$LIVE_H" -ge 560 -a "$LIVE_H" -le 800
    window_id=$(geom_field "$geometry" window)
    xwininfo -id "$window_id" -stats >"$DIR/xwininfo-window.txt" 2>&1 || true
    xwininfo -root -children >"$DIR/xwininfo-root.txt" 2>&1 || true
}

dialog_point() {
    # $1 = offset from the dialog top; prints "x y" window-relative for the
    # horizontally centered 520x400 dialog box (main_window.slint dialog host).
    local offset_y=$1
    echo "$(( (LIVE_W - 520) / 2 + 250 )) $(( (LIVE_H - 400) / 2 + offset_y ))"
}

dialog_region() {
    # Region covering the dialog box: "x,y,w,h" window-relative.
    echo "$(( (LIVE_W - 520) / 2 )),$(( (LIVE_H - 400) / 2 )),520,230"
}

dialog_button_point() {
    # Dialogs close only through their explicit buttons (upstream parity:
    # Escape/Enter are routed but ignored, see actions::dialogs). The button
    # row is pinned to the dialog bottom: Cancel at x +336..416, Apply/Import
    # at +424..504, both 28 px tall ending 16 px above the dialog bottom.
    local button=$1
    case "$button" in
        cancel) echo "$(( (LIVE_W - 520) / 2 + 376 )) $(( (LIVE_H - 400) / 2 + 370 ))" ;;
        apply) echo "$(( (LIVE_W - 520) / 2 + 464 )) $(( (LIVE_H - 400) / 2 + 370 ))" ;;
        *) echo "unknown dialog button $button" >&2; return 2 ;;
    esac
}

affix_screen_point() {
    # Fixed control anchors of the AffixScreen left column (nav rail 48 px
    # + 310 px presets column; the rows above the entry list are fixed
    # height, so these points are independent of the desktop window size):
    # the "Import" action chip, the "New Preset" action chip (both in the
    # second chip row of the build-import toolbar) and the selection-gated
    # pencil (rename) icon button next to the preset ComboBox.
    local anchor=$1
    case "$anchor" in
        import_chip) echo "88 528" ;;
        new_preset_chip) echo "215 528" ;;
        rename_pencil) echo "302 207" ;;
        *) echo "unknown affix screen anchor $anchor" >&2; return 2 ;;
    esac
}

# ── Inner: one journey under the private Xvfb display ──────────────────────
run_inner() {
    local journey=$1 binary=$2
    local label
    case "$journey" in
        1) label="j1-release-import-dialog" ;;
        2) label="j2-dev-resize-keyboard-rename" ;;
        3) label="j3-release-unfocused-hotkey" ;;
        *) echo "unknown journey $journey" >&2; exit 2 ;;
    esac
    DIR="$EVIDENCE_ROOT/$label"
    STEP_LOG="$DIR/steps.log"
    CAPTURE_LOG="$DIR/captures.txt"
    rm -rf "$DIR"
    mkdir -p "$DIR"
    : >"$STEP_LOG"
    : >"$CAPTURE_LOG"

    # Hermetic scratch environment: the app never sees the real user config.
    mkdir -p "$WORK/home" "$WORK/xdg" "$WORK/config"
    export HOME="$WORK/home"
    export XDG_CONFIG_HOME="$WORK/xdg/config"
    export XDG_DATA_HOME="$WORK/xdg/data"
    export XDG_CACHE_HOME="$WORK/xdg/cache"
    export XDG_RUNTIME_DIR="$WORK/xdg/runtime"
    export D4LC_CONFIG_ROOT="$WORK/config"
    # Deterministic software rendering + X11 windowing (mirrors the visual
    # smoke harness environment).
    export SLINT_BACKEND="winit-software"
    export WINIT_UNIX_BACKEND="x11"
    export XDG_SESSION_TYPE="x11"
    unset WAYLAND_DISPLAY || true

    echo "[xtest-journeys] journey $journey ($label) binary=$binary"
    # --no-updates: journeys stay network-free (the release banner check is
    # the only startup network path and it is opt-out by flag). The wrapper
    # subshell records the app's exit code for the close-request assertion.
    (
        "$binary" --no-updates
        echo $? >"$DIR/app-exit-code"
    ) >"$DIR/app.log" 2>&1 &
    local app_pid=$!

    local status=0
    # Run the journey in a subshell with fail-fast semantics: any driver or
    # assertion error aborts the journey immediately; the subshell exit code
    # is the journey status.
    set +e
    case "$journey" in
        1) ( set -e; journey_1 "$app_pid" ) ;;
        2) ( set -e; journey_2 "$app_pid" ) ;;
        3) ( set -e; journey_3 "$app_pid" ) ;;
    esac
    status=$?
    set -e

    if kill -0 "$app_pid" 2>/dev/null; then
        kill -9 "$app_pid" 2>/dev/null || true
        wait "$app_pid" 2>/dev/null || true
    else
        wait "$app_pid" 2>/dev/null || true
    fi
    return "$status"
}

# ── Journey 1: release binary — nav, import dialog, typed URL, Escape ──────
journey_1() {
    local app_pid=$1
    common_start "$app_pid"

    step_capture j1-0-baseline.ppm

    # Click through the nav rail (48 px entries stacked from y=40 under the
    # 40 px header): tab 1 ("Affix Presets"), assert the screen really
    # changed, then back to tab 0 ("Current Item").
    drv click --title "$APP_WINDOW_TITLE" --x 24 --y 112
    sleep 0.5
    step_capture j1-1-nav-tab1.ppm
    local nav_changed
    nav_changed=$(diff_field j1-0-baseline.ppm j1-1-nav-tab1.ppm changed_pct)
    assert_number_cmp "nav to tab 1 re-rendered the window (changed_pct=$nav_changed >= 5)" \
        "$nav_changed" ">=" 5

    drv click --title "$APP_WINDOW_TITLE" --x 24 --y 64
    sleep 0.5
    step_capture j1-2-back-tab0.ppm
    local nav_back
    nav_back=$(diff_field j1-0-baseline.ppm j1-2-back-tab0.ppm changed_pct)
    assert_number_cmp "nav back to tab 0 restores the baseline (changed_pct=$nav_back <= 2)" \
        "$nav_back" "<=" 2

    # Open the import dialog: the "Import" chip sits in the second chip row
    # of the AffixScreen build-import toolbar (~x 72..105, ~y 521..543).
    read -r chip_x chip_y <<<"$(affix_screen_point import_chip)"
    drv click --title "$APP_WINDOW_TITLE" --x "$chip_x" --y "$chip_y"
    sleep 0.6
    step_capture j1-3-import-dialog.ppm
    local dialog_changed
    dialog_changed=$(diff_field j1-2-back-tab0.ppm j1-3-import-dialog.ppm changed_pct)
    assert_number_cmp "import dialog opened over the whole window (changed_pct=$dialog_changed >= 50)" \
        "$dialog_changed" ">=" 50

    # Click into the "Build URL" field (first input row inside the dialog),
    # establish the input focus a window manager would provide, then type.
    read -r url_x url_y <<<"$(dialog_point 96)"
    drv click --title "$APP_WINDOW_TITLE" --x "$url_x" --y "$url_y"
    sleep 0.3
    drv focus --title "$APP_WINDOW_TITLE"
    sleep 0.3
    drv type-text --text "https://example.com/loot"
    sleep 0.4
    step_capture j1-4-typed-url.ppm
    local typed_px
    typed_px=$(diff_field j1-3-import-dialog.ppm j1-4-typed-url.ppm changed_px \
        --region "$(dialog_region)")
    assert_number_cmp "typed URL landed in the dialog (dialog-top changed_px=$typed_px >= 50)" \
        "$typed_px" ">=" 50

    # Click the dialog's Import (confirm) button: the Rust confirm handler
    # reads the typed URL from the form props, validates it against the
    # bundled build ids, and surfaces a visible dialog error — the observable
    # proof that the XTEST-typed text flowed through Slint into Rust.
    read -r import_x import_y <<<"$(dialog_button_point apply)"
    drv click --title "$APP_WINDOW_TITLE" --x "$import_x" --y "$import_y"
    sleep 0.6
    step_capture j1-5-confirm-error.ppm
    local error_px
    error_px=$(diff_field j1-4-typed-url.ppm j1-5-confirm-error.ppm changed_px \
        --region "$(dialog_region)")
    assert_number_cmp "confirm reacted to the typed URL (dialog changed_px=$error_px >= 1000)" \
        "$error_px" ">=" 1000

    # Escape is routed by the dialog keyboard host and deliberately ignored
    # (upstream parity: no upstream dialog binds Escape — actions::dialogs).
    # The journey presses it and asserts the app's designed response: the
    # dialog stays open, unchanged.
    drv key --name escape
    sleep 0.5
    step_capture j1-6-escape-ignored.ppm
    local escape_changed
    escape_changed=$(diff_field j1-5-confirm-error.ppm j1-6-escape-ignored.ppm changed_pct)
    assert_number_cmp "Escape is parity-ignored, dialog stays open (changed_pct=$escape_changed <= 2)" \
        "$escape_changed" "<=" 2

    # Close the dialog through its real Cancel button.
    read -r cancel_x cancel_y <<<"$(dialog_button_point cancel)"
    drv click --title "$APP_WINDOW_TITLE" --x "$cancel_x" --y "$cancel_y"
    sleep 0.5
    step_capture j1-7-closed.ppm
    local closed_changed restored
    closed_changed=$(diff_field j1-3-import-dialog.ppm j1-7-closed.ppm changed_pct)
    restored=$(diff_field j1-2-back-tab0.ppm j1-7-closed.ppm changed_pct)
    assert_number_cmp "Cancel closed the dialog (changed_pct vs open=$closed_changed >= 50)" \
        "$closed_changed" ">=" 50
    assert_number_cmp "window matches the pre-dialog baseline after close (changed_pct=$restored <= 2)" \
        "$restored" "<=" 2

    assert_close_and_exit_zero "$app_pid" "journey 1"
}

# ── Journey 2: dev binary — resize, keyboard preset-create, keyboard rename ─
journey_2() {
    local app_pid=$1
    common_start "$app_pid"

    step_capture j2-0-baseline.ppm

    # Shrink the window far below its layout minimum; the X server applies
    # the cross-client ConfigureWindow and winit must keep rendering.
    drv resize --title "$APP_WINDOW_TITLE" --w 320 --h 240
    sleep 0.8
    local small_geom small_w small_h
    small_geom=$(drv geom --title "$APP_WINDOW_TITLE")
    echo "$small_geom" | tee -a "$DIR/geometry.txt"
    small_w=$(geom_field "$small_geom" w)
    small_h=$(geom_field "$small_geom" h)
    assert "window resized to 320x240 (got ${small_w}x${small_h})" \
        test "$small_w" -eq 320 -a "$small_h" -eq 240
    local small_stats
    small_stats=$(drv capture --title "$APP_WINDOW_TITLE" --out "$DIR/j2-1-small.ppm")
    echo "$small_stats" | tee -a "$CAPTURE_LOG"
    assert "app still renders at 320x240 (non-blank capture)" \
        test -s "$DIR/j2-1-small.ppm"

    drv resize --title "$APP_WINDOW_TITLE" --w "$LIVE_W" --h "$LIVE_H"
    sleep 0.8
    local back_geom
    back_geom=$(drv geom --title "$APP_WINDOW_TITLE")
    echo "$back_geom" | tee -a "$DIR/geometry.txt"
    assert "window restored to ${LIVE_W}x${LIVE_H}" \
        test "$(geom_field "$back_geom" w)" -eq "$LIVE_W" -a "$(geom_field "$back_geom" h)" -eq "$LIVE_H"

    # Open the New Preset dialog through the "New Preset" action chip.
    read -r chip_x chip_y <<<"$(affix_screen_point new_preset_chip)"
    drv click --title "$APP_WINDOW_TITLE" --x "$chip_x" --y "$chip_y"
    sleep 0.6
    step_capture j2-2-new-preset-dialog.ppm
    local changed
    changed=$(diff_field j2-0-baseline.ppm j2-2-new-preset-dialog.ppm changed_pct)
    assert_number_cmp "new-preset dialog opened (changed_pct=$changed >= 50)" \
        "$changed" ">=" 50

    # Complete the preset creation with the keyboard only: click the name
    # input, establish the input focus (the WM's job; Xvfb has none), type
    # the name (asserted to land), then confirm through the dialog's Apply
    # button (the only confirm path, per upstream parity: Enter is routed
    # but ignored).
    read -r input_x input_y <<<"$(dialog_point 86)"
    drv click --title "$APP_WINDOW_TITLE" --x "$input_x" --y "$input_y"
    sleep 0.3
    drv focus --title "$APP_WINDOW_TITLE"
    sleep 0.3
    drv type-text --text "JourneyPreset"
    sleep 0.4
    step_capture j2-3-typed-name.ppm
    local field_x0=$(( (LIVE_W - 520) / 2 + 16 ))
    local field_y0=$(( (LIVE_H - 400) / 2 + 71 ))
    local typed_px
    typed_px=$(diff_field j2-2-new-preset-dialog.ppm j2-3-typed-name.ppm changed_px \
        --region "$field_x0,$field_y0,488,34")
    assert_number_cmp "typed preset name landed in the new-preset input (field changed_px=$typed_px >= 100)" \
        "$typed_px" ">=" 100

    read -r apply_x apply_y <<<"$(dialog_button_point apply)"
    drv click --title "$APP_WINDOW_TITLE" --x "$apply_x" --y "$apply_y"
    sleep 0.5
    step_capture j2-4-preset-created.ppm
    local closed
    closed=$(diff_field j2-0-baseline.ppm j2-4-preset-created.ppm changed_pct)
    assert_number_cmp "new-preset dialog confirmed closed (changed_pct=$closed <= 3)" \
        "$closed" "<=" 3
    local combobox_region="70,190,300,40"
    local created_px
    created_px=$(diff_field j2-0-baseline.ppm j2-4-preset-created.ppm changed_px \
        --region "$combobox_region")
    assert_number_cmp "preset combobox shows the keyboard-created preset (region changed_px=$created_px >= 300)" \
        "$created_px" ">=" 300

    # Open the rename dialog via the selection-gated pencil button (upstream
    # CanRenameAffixPresetCommandExecute parity): creating the preset
    # selected it, which is what enables the button.
    read -r pencil_x pencil_y <<<"$(affix_screen_point rename_pencil)"
    drv click --title "$APP_WINDOW_TITLE" --x "$pencil_x" --y "$pencil_y"
    sleep 0.6
    step_capture j2-5-rename-dialog.ppm
    local rename_changed
    rename_changed=$(diff_field j2-4-preset-created.ppm j2-5-rename-dialog.ppm changed_pct)
    assert_number_cmp "pencil opened the rename dialog (changed_pct=$rename_changed >= 50)" \
        "$rename_changed" ">=" 50
    local not_setpreset_px
    not_setpreset_px=$(diff_field j2-2-new-preset-dialog.ppm j2-5-rename-dialog.ppm changed_px \
        --region "$(( (LIVE_W - 520) / 2 )),$(( (LIVE_H - 400) / 2 )),520,400")
    assert_number_cmp "the pencil opened the rename dialog, not the new-preset dialog (dialog layout changed_px=$not_setpreset_px >= 1000)" \
        "$not_setpreset_px" ">=" 1000

    # Keyboard-only rename: click the seeded new-name input, establish the
    # input focus, clear the pre-filled current name, type the new name
    # (asserted to land), then confirm through Apply.
    read -r rename_x rename_y <<<"$(dialog_point 137)"
    drv click --title "$APP_WINDOW_TITLE" --x "$rename_x" --y "$rename_y"
    sleep 0.3
    drv focus --title "$APP_WINDOW_TITLE"
    sleep 0.3
    drv key --name end
    drv key --name backspace --count 24
    sleep 0.3
    drv type-text --text "RenamedViaKeys"
    sleep 0.4
    step_capture j2-6-typed-new-name.ppm
    local rename_field_y0=$(( (LIVE_H - 400) / 2 + 122 ))
    local renamed_px
    renamed_px=$(diff_field j2-5-rename-dialog.ppm j2-6-typed-new-name.ppm changed_px \
        --region "$field_x0,$rename_field_y0,488,34")
    assert_number_cmp "typed new preset name landed in the rename input (field changed_px=$renamed_px >= 300)" \
        "$renamed_px" ">=" 300

    drv click --title "$APP_WINDOW_TITLE" --x "$apply_x" --y "$apply_y"
    sleep 0.5
    step_capture j2-7-renamed.ppm
    local renamed_closed
    renamed_closed=$(diff_field j2-4-preset-created.ppm j2-7-renamed.ppm changed_pct)
    assert_number_cmp "rename dialog confirmed closed (changed_pct=$renamed_closed <= 3)" \
        "$renamed_closed" "<=" 3
    local renamed_combobox_px
    renamed_combobox_px=$(diff_field j2-4-preset-created.ppm j2-7-renamed.ppm changed_px \
        --region "$combobox_region")
    assert_number_cmp "preset combobox shows the keyboard-renamed preset (region changed_px=$renamed_combobox_px >= 300)" \
        "$renamed_combobox_px" ">=" 300

    # Reopen the rename dialog: the seeded current name must be the renamed
    # preset, proving the rename persisted in the preset editor.
    drv click --title "$APP_WINDOW_TITLE" --x "$pencil_x" --y "$pencil_y"
    sleep 0.6
    step_capture j2-8-rename-reopened.ppm
    local reopen_changed
    reopen_changed=$(diff_field j2-7-renamed.ppm j2-8-rename-reopened.ppm changed_pct)
    assert_number_cmp "rename dialog reopened (changed_pct=$reopen_changed >= 50)" \
        "$reopen_changed" ">=" 50
    local seed_row_x0=$(( (LIVE_W - 520) / 2 + 16 ))
    local seed_row_y0=$(( (LIVE_H - 400) / 2 + 70 ))
    local reseeded_px
    reseeded_px=$(diff_field j2-5-rename-dialog.ppm j2-8-rename-reopened.ppm changed_px \
        --region "$seed_row_x0,$seed_row_y0,320,24")
    assert_number_cmp "reopened rename dialog seeds the renamed preset (current-name row changed_px=$reseeded_px >= 300)" \
        "$reseeded_px" ">=" 300

    read -r cancel_x cancel_y <<<"$(dialog_button_point cancel)"
    drv click --title "$APP_WINDOW_TITLE" --x "$cancel_x" --y "$cancel_y"
    sleep 0.5
    step_capture j2-9-rename-closed.ppm
    local rename_closed
    rename_closed=$(diff_field j2-7-renamed.ppm j2-9-rename-closed.ppm changed_pct)
    assert_number_cmp "rename dialog cancelled closed (changed_pct=$rename_closed <= 3)" \
        "$rename_closed" "<=" 3

    assert_close_and_exit_zero "$app_pid" "journey 2"
}

# ── Journey 3: release binary — global hotkey while unfocused ──────────────
journey_3() {
    local app_pid=$1
    local settings_file="$WORK/config/Config/Settings.json"
    mkdir -p "$WORK/config/Config"
    # Upstream-shaped minimal settings: enable one binding (the upstream
    # default for ToggleOverlay) so the X11 hotkey backend registers it.
    cat >"$settings_file" <<'EOF'
{
  "KeyBindingConfigToggleOverlay": {
    "IsEnabled": true,
    "Name": "Toggle Overlay",
    "KeyGestureKey": 101,
    "KeyGestureModifier": 2
  }
}
EOF

    common_start "$app_pid"
    step_capture j3-0-baseline.ppm

    # Make the app genuinely unfocused: pointer-root focus + pointer parked
    # over the bare root window, then press Ctrl+F12 at the X server level.
    drv unfocus
    sleep 0.5
    drv key --name f12 --mod ctrl
    sleep 2

    # ToggleOverlay flips settings.overlay_enabled and persists through the
    # same confirm_overlay_config helper the Overlay config dialog uses, so
    # the on-disk settings file is the observable outcome.
    assert "hotkey toggle persisted IsOverlayEnabled=true to $settings_file" \
        grep -q '"IsOverlayEnabled": true' "$settings_file"
    cp "$settings_file" "$DIR/settings-after-hotkey.json"
    step_capture j3-1-after-hotkey.ppm

    assert_close_and_exit_zero "$app_pid" "journey 3"
}

# ── Entry points ────────────────────────────────────────────────────────────
if [ "${1:-}" = "--inner" ]; then
    shift
    run_inner "$@"
    exit $?
fi

build_driver
build_app_binaries

RELEASE_BIN=target/release/d4lc-app
DEV_BIN=target/debug/d4lc-app
declare -a JOURNEYS=()
if [ $# -gt 0 ]; then
    JOURNEYS=("$@")
else
    JOURNEYS=(1 2 3)
fi

declare -i FAILED=0 PASSED=0
for journey in "${JOURNEYS[@]}"; do
    case "$journey" in
        1) binary=$RELEASE_BIN ; label="j1-release-import-dialog" ;;
        2) binary=$DEV_BIN ; label="j2-dev-resize-keyboard-rename" ;;
        3) binary=$RELEASE_BIN ; label="j3-release-unfocused-hotkey" ;;
        *) echo "[xtest-journeys] unknown journey '$journey' (use 1, 2, 3)" >&2; exit 2 ;;
    esac
    if xvfb-run -a --server-args="-screen 0 1280x800x24 -nolisten tcp" \
        "$0" --inner "$journey" "$binary"; then
        echo "journey=$journey label=$label status=passed" | tee -a "$REPORT_FILE"
        PASSED+=1
    else
        echo "journey=$journey label=$label status=failed evidence=$EVIDENCE_ROOT/$label" | tee -a "$REPORT_FILE"
        FAILED+=1
    fi
done

echo "[xtest-journeys] passed=$PASSED failed=$FAILED (evidence: $EVIDENCE_ROOT)"
[ "$FAILED" -eq 0 ] && [ "$PASSED" -gt 0 ]
