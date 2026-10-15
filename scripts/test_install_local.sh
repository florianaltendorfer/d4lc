#!/usr/bin/env bash
set -euo pipefail

# Runs scripts/install_local.sh --skip-build against stand-in binaries in a
# throwaway HOME: the payload must land together with the game data, the
# launcher must start it from there, a reinstall must replace the assets
# wholesale, and a missing build must fail without installing anything.

cd "$(dirname "$0")/.."
repo_root=$(pwd)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fail() {
    echo "test_install_local: $*" >&2
    exit 1
}

home="$work/home"
target="$work/target"
app_dir="$home/.local/share/d4lc"
mkdir -p "$home" "$target/release"

# Missing build output: refuse, install nothing.
if HOME="$home" D4LC_INSTALL_TARGET_DIR="$target" ./scripts/install_local.sh --skip-build \
    >/dev/null 2>"$work/missing.err"; then
    fail "an install without built binaries must fail"
fi
grep -q "required path missing" "$work/missing.err" || fail "the missing build must be named"
[ ! -e "$home/.local/bin/d4lc" ] || fail "nothing may be installed after a failed install"

# Stand-in payloads that report their working directory (the debug CLI also
# its asset root).
printf '#!/usr/bin/env bash\npwd\n' >"$target/release/d4lc-app"
# shellcheck disable=SC2016 # expanded by the stand-in payload
printf '#!/usr/bin/env bash\necho "$(pwd) ${D4LC_ASSET_ROOT:-unset}"\n' >"$target/release/d4lc"
chmod 0755 "$target/release/d4lc-app" "$target/release/d4lc"

HOME="$home" D4LC_INSTALL_TARGET_DIR="$target" ./scripts/install_local.sh --skip-build >/dev/null 2>&1

[ -x "$home/.local/bin/d4lc" ] || fail "desktop launcher missing"
[ -x "$home/.local/bin/d4lc-debug" ] || fail "debug launcher missing"
[ -x "$app_dir/bin/d4lc-real" ] || fail "desktop payload missing"
[ -x "$app_dir/bin/d4lc-debug-real" ] || fail "debug payload missing"
for tree in assets/original/data assets/original/systempresets assets/d4lc; do
    [ -d "$app_dir/$tree" ] || fail "game data missing: $tree"
done
diff -rq "$repo_root/assets/original/data" "$app_dir/assets/original/data" >/dev/null \
    || fail "installed catalog data differs from the checkout"

# The launcher starts the payload from the data directory, whatever the caller's cwd.
started_in=$(cd / && "$home/.local/bin/d4lc")
[ "$started_in" = "$app_dir" ] || fail "launcher started the payload in '$started_in', not '$app_dir'"
# The debug CLI keeps the caller's directory and reads the data through D4LC_ASSET_ROOT.
debug_started=$(cd / && "$home/.local/bin/d4lc-debug")
[ "$debug_started" = "/ $app_dir" ] \
    || fail "debug launcher reported '$debug_started', not '/ $app_dir' (cwd, D4LC_ASSET_ROOT)"

# A reinstall replaces the assets wholesale.
touch "$app_dir/assets/original/stale-file"
HOME="$home" D4LC_INSTALL_TARGET_DIR="$target" ./scripts/install_local.sh --skip-build >/dev/null 2>&1
[ ! -e "$app_dir/assets/original/stale-file" ] || fail "a reinstall must drop stale asset files"

# A failed copy leaves the previous install, assets included, untouched.
mkdir -p "$work/fakebin"
printf '#!/usr/bin/env bash\ncase "${*: -1}" in */assets/d4lc) exit 1 ;; esac\nexec /usr/bin/env -u PATH PATH=/usr/bin:/bin cp "$@"\n' >"$work/fakebin/cp"
chmod 0755 "$work/fakebin/cp"
touch "$app_dir/assets/original/previous-install"
if PATH="$work/fakebin:$PATH" HOME="$home" D4LC_INSTALL_TARGET_DIR="$target" \
    ./scripts/install_local.sh --skip-build >/dev/null 2>&1; then
    fail "an install whose copy fails must fail"
fi
[ -e "$app_dir/assets/original/previous-install" ] || fail "a failed copy must keep the previous assets"
[ -x "$app_dir/bin/d4lc-real" ] || fail "a failed copy must keep the previous payload"
leftovers=$(find "$home/.local/share" -maxdepth 1 -name '.d4lc-install.*' | wc -l)
[ "$leftovers" -eq 0 ] || fail "a failed install must clean up its staging directory"

# Replacing a payload that is running must work (no write in place).
cp "$(command -v sleep)" "$target/release/d4lc-app"
HOME="$home" D4LC_INSTALL_TARGET_DIR="$target" ./scripts/install_local.sh --skip-build >/dev/null 2>&1
"$app_dir/bin/d4lc-real" 30 &
running=$!
trap 'kill "$running" 2>/dev/null || true; rm -rf "$work"' EXIT
HOME="$home" D4LC_INSTALL_TARGET_DIR="$target" ./scripts/install_local.sh --skip-build >/dev/null 2>&1 \
    || fail "reinstalling over a running payload must succeed"
kill "$running" 2>/dev/null || true

# CARGO_TARGET_DIR is honored when no explicit install target is given.
HOME="$home" CARGO_TARGET_DIR="$target" ./scripts/install_local.sh --skip-build >/dev/null 2>&1 \
    || fail "CARGO_TARGET_DIR must locate the built binaries"

echo "test_install_local: ok"
