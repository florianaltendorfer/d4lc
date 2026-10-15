#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
zip_path="${1:-}"
if [ -z "$zip_path" ]; then
  echo "usage: $0 /path/to/Diablo4Companion-master.zip" >&2
  exit 2
fi
rm -rf upstream/Diablo4Companion
mkdir -p upstream
unzip -q "$zip_path" -d upstream/.tmp-diablo4companion
inner="$(find upstream/.tmp-diablo4companion -maxdepth 1 -type d -name 'Diablo4Companion-*' | head -1)"
if [ -z "$inner" ]; then
  echo "Could not find extracted Diablo4Companion directory" >&2
  exit 3
fi
mv "$inner" upstream/Diablo4Companion
rm -rf upstream/.tmp-diablo4companion
echo "Imported upstream source into upstream/Diablo4Companion"
