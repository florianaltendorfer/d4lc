#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

# Two tiers. The inner loop (a single implementation or fix pass) runs
# `--fast`: everything except the desktop and browser gates. The commit and
# integration gate runs the full tier, which adds the real-input pilot suite,
# the X11 visual smoke and the real-browser integration tests. Both tiers run
# the same unit suite, clippy pass and parity gate, so a fast iteration never
# lands code the full tier has not checked in a different way.
fast=0
for argument in "$@"; do
  case "$argument" in
    --fast) fast=1 ;;
    -h | --help)
      echo "usage: verify_local.sh [--fast]"
      echo "  --fast  skip the real-input pilot suite, the desktop visual smoke and the real-browser tests"
      exit 0
      ;;
    *)
      echo "[verify] unknown argument: $argument" >&2
      exit 2
      ;;
  esac
done
if [ "$fast" -eq 1 ]; then
  echo "[verify] tier: fast (real-input pilots, desktop visual smoke and real-browser tests skipped)"
else
  echo "[verify] tier: full"
fi

echo "[verify] disk-space guard"
./scripts/check_disk_space.sh
# Report the artifact footprint after the run so an abnormal writer
# into target/ becomes visible instead of silently filling the disk.
trap 'echo "[disk-guard] target size after verify: $(du -sh target 2>/dev/null | cut -f1)"' EXIT

echo "[verify] dependency direction"
python3 scripts/verify_dep_direction.py

echo "[verify] local scripts"
./scripts/test_install_local.sh
./scripts/test_visual_smoke_status.sh
./scripts/test_publish_draft_release.sh
./scripts/test_package_linux_release.sh
./scripts/test_check_upstream.sh
./scripts/test_release_artifact_checks.sh

echo "[verify] release workflow gating"
python3 scripts/check_release_workflow.py --self-test

echo "[verify] cargo fmt"
cargo fmt --all -- --check

echo "[verify] cargo clippy"
env -u DISPLAY -u WAYLAND_DISPLAY cargo clippy --workspace --all-targets -- -D warnings

# The shipped binaries (d4lc-app, and d4lc-cli as d4lc-debug) build with
# these features; default-feature clippy never sees the code behind them.
release_features="d4lc-app/real-runtime d4lc-app/leptess-backend d4lc-app/wayland-portal d4lc-cli/leptess-backend d4lc-overlay-x11/real-x11"
echo "[verify] cargo clippy (release features)"
env -u DISPLAY -u WAYLAND_DISPLAY cargo clippy -p d4lc-app -p d4lc-cli -p d4lc-overlay-x11 \
  --all-targets --features "$release_features" -- -D warnings

echo "[verify] cargo test"
env -u DISPLAY -u WAYLAND_DISPLAY cargo test --workspace

# Tests behind the release features (live runtime, native OCR, portal
# capture, the real X11 overlay backend under Xvfb) only compile with them;
# the default-feature run never sees them.
echo "[verify] cargo test (release features)"
env -u DISPLAY -u WAYLAND_DISPLAY cargo test -p d4lc-app -p d4lc-runtime -p d4lc-capture \
  -p d4lc-ocr -p d4lc-cli -p d4lc-overlay-x11 --features "$release_features"

if [ "$fast" -eq 0 ]; then
  echo "[verify] real-input pilot suite (one process per pilot, run in parallel)"
  python3 scripts/run_pilot_suite.py

  echo "[verify] desktop visual smoke (a SKIPPED smoke fails the full tier)"
  ./scripts/verify_visual_smoke.sh

  # Real Chromium over CDP against loopback pages; a missing browser fails
  # the full tier. The live provider smokes stay opt-in (network).
  echo "[verify] real-browser integration (loopback)"
  ./scripts/run_browser_integration.sh
fi

echo "[verify] upstream asset parity gate"
cargo run -q -p d4lc-cli -- upstream-asset-parity
