#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

# The smoke test honors D4LC_VISUAL_SMOKE_DIR for its artifact tree; the
# report check must read the same directory (default unchanged).
artifact_dir="${D4LC_VISUAL_SMOKE_DIR:-debug-output/visual-smoke}"
report_file="$artifact_dir/visual-smoke-report.txt"
skip_file="$artifact_dir/skip-report.txt"

# D4LC_VISUAL_SMOKE_EVALUATE_ONLY=1 skips the run and only evaluates the
# artifact directory (scripts/test_visual_smoke_status.sh).
if [[ "${D4LC_VISUAL_SMOKE_EVALUATE_ONLY:-0}" != "1" ]]; then
  echo "[visual-smoke] cargo test -p d4lc-app visual_smoke --features \"real-runtime leptess-backend\""
  D4LC_VISUAL_SMOKE=1 cargo test -p d4lc-app visual_smoke --features "real-runtime leptess-backend" -- --ignored --nocapture
fi

if [[ -f "$report_file" ]]; then
  grep -q '^status=passed$' "$report_file"
  grep -q '^placeholder_check=passed$' "$report_file"
  capture_count="$(grep -c '^capture=' "$report_file")"
  if [[ "$capture_count" -lt 6 ]]; then
    echo "[visual-smoke] expected 6 captures but saw $capture_count"
    exit 1
  fi
  echo "[visual-smoke] passed ($capture_count captures)"
  exit 0
fi

# A skipped smoke proved nothing: report it as SKIPPED and fail with its own
# exit status, so the full gate cannot pass without a screenshot.
if [[ -f "$skip_file" ]]; then
  echo "[visual-smoke] SKIPPED (no screenshots were taken): $(tr '\n' ' ' < "$skip_file")"
  exit 3
fi

echo "[visual-smoke] missing $report_file or $skip_file"
exit 1
