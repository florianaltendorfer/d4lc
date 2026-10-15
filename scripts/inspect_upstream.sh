#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

cargo run -q -p d4lc-cli -- upstream-report "$@"
