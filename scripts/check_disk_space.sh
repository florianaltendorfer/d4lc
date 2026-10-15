#!/usr/bin/env bash
# Fail-closed disk-space guard for every build/validation entrypoint.
# Build artifacts of this workspace can grow to hundreds of gigabytes; on a
# nearly full disk that takes the whole desktop down. This script makes every
# entrypoint abort BEFORE building when free space is below the
# threshold, and it reports the cargo target size after runs so an
# abnormal writer becomes visible immediately.
set -euo pipefail

# Minimum free space in GB on the filesystem holding the repo.
# Override with D4LC_MIN_FREE_GB. This workspace legitimately needs
# tens of GB for multi-feature debug+release graphs; 80 GB is the
# safety margin below which we refuse to build.
MIN_FREE_GB="${D4LC_MIN_FREE_GB:-80}"

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

free_kb="$(df -Pk "$REPO_ROOT" | awk 'NR==2 {print $4}')"
free_gb=$((free_kb / 1024 / 1024))

if (( free_gb < MIN_FREE_GB )); then
  echo "[disk-guard] FAIL: only ${free_gb} GB free on $(df -P "$REPO_ROOT" | awk 'NR==2 {print $1}')" >&2
  echo "[disk-guard] Refusing to build: minimum is ${MIN_FREE_GB} GB (override: D4LC_MIN_FREE_GB=<gb>)." >&2
  echo "[disk-guard] Free space first, e.g.: cargo clean, rm -rf target/debug target/release." >&2
  exit 1
fi

echo "[disk-guard] ok: ${free_gb} GB free (minimum ${MIN_FREE_GB} GB)"
