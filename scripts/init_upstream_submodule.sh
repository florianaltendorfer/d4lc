#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

url="$(grep '^url *= *' config/upstream-files.toml | head -1 | sed 's/.*= *//; s/"//g')"
branch="$(grep '^branch *= *' config/upstream-files.toml | head -1 | sed 's/.*= *//; s/"//g')"
local_path="$(grep '^local_path *= *' config/upstream-files.toml | head -1 | sed 's/.*= *//; s/"//g')"

if [ -z "$url" ] || [ -z "$branch" ] || [ -z "$local_path" ]; then
  echo "[upstream] failed to read url/branch/local_path from config/upstream-files.toml" >&2
  exit 2
fi

mkdir -p "$(dirname "$local_path")"

if [ -d "$local_path/.git" ] || git -C "$local_path" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "[upstream] already initialized at $local_path"
  git -C "$local_path" remote get-url origin >/dev/null 2>&1 || true
  exit 0
fi

if [ -e "$local_path" ] && [ "$(find "$local_path" -mindepth 1 -maxdepth 1 | wc -l)" -gt 0 ]; then
  echo "[upstream] $local_path exists but is not a git checkout; leaving files untouched" >&2
  echo "[upstream] offline/source-bundle mode detected; remove the folder to clone a fresh submodule" >&2
  exit 0
fi

if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  git submodule add -b "$branch" "$url" "$local_path"
else
  git clone --branch "$branch" "$url" "$local_path"
fi

echo "[upstream] initialized $url#$branch at $local_path"
