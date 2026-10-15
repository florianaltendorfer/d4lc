#!/usr/bin/env bash
# scripts/smoke_deb_install.sh — install the .deb into a clean Debian/Ubuntu
# system through apt and prove the packaged binaries start there.
#
# Usage (as root, inside a fresh container): scripts/smoke_deb_install.sh <path-to.deb>
#
# apt resolves the package's Depends field for real, so a dependency the
# package forgot to declare (or declared but cannot be installed) fails here
# instead of on a user's machine. Test tooling (xvfb, a font) is installed
# after the package so it cannot mask a missing dependency.
set -euo pipefail

if [ $# -ne 1 ]; then
    echo "usage: scripts/smoke_deb_install.sh <path-to.deb>" >&2
    exit 2
fi

deb=$(realpath "$1")
bundle_root=/opt/d4lc

fail() {
    echo "smoke_deb_install: $*" >&2
    exit 1
}

export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y "$deb"
dpkg-query -W d4lc

# Every library the payloads link must resolve with the wrapper's search path.
for binary in "$bundle_root"/bin/*-real; do
    unresolved=$(LD_LIBRARY_PATH="$bundle_root/lib" ldd "$binary" | grep 'not found' || true)
    [ -z "$unresolved" ] || fail "$binary has unresolved libraries:
$unresolved"
done

# Libraries tied to the host (see bundle_shared_libraries) must not be bundled.
if [ -d "$bundle_root/lib" ]; then
    host_tied=$(ls -1 "$bundle_root/lib" \
        | grep -E '^(libpipewire|libspa|libstdc\+\+|libgcc_s|libdbus|libsystemd|libudev|libGL|libEGL|libvulkan|libwayland|libX11)' || true)
    [ -z "$host_tied" ] || fail "host-tied libraries are bundled:
$host_tied"
fi

# The debug CLI is headless: it must run without a display.
env -u DISPLAY -u WAYLAND_DISPLAY d4lc-debug help >/dev/null

# The debug CLI as an ordinary user, started from that user's home: it reads
# the installed bundle, which stays untouched, and writes below its working
# directory only.
smoke_user=d4lc-smoke
id "$smoke_user" >/dev/null 2>&1 || useradd --create-home --shell /bin/bash "$smoke_user"
smoke_home=$(getent passwd "$smoke_user" | cut -d: -f6)
bundle_digest() {
    find "$bundle_root" -printf '%P %y %s %T@ %m %U\n' | LC_ALL=C sort | sha256sum
}
digest_before=$(bundle_digest)
as_smoke_user() {
    (cd "$smoke_home" && runuser -u "$smoke_user" -- env -u DISPLAY -u WAYLAND_DISPLAY \
        -u D4LC_CONFIG_ROOT -u D4LC_ASSET_ROOT -u XDG_CONFIG_HOME \
        HOME="$smoke_home" "$@")
}
summary=$(as_smoke_user d4lc-debug manual-diablo-validation-bundle --dry-run) \
    || fail "d4lc-debug manual-diablo-validation-bundle --dry-run failed as $smoke_user"
bundle_dir=$(printf '%s\n' "$summary" | sed -nE 's/^ *"bundle_dir": "([^"]*)",?$/\1/p')
case "$bundle_dir" in
    "$smoke_home"/debug-output/manual-diablo-iv/run-*) ;;
    *) fail "the dry-run bundle is not below $smoke_home/debug-output: '$bundle_dir'
$summary" ;;
esac
[ -f "$bundle_dir/bundle-summary.json" ] || fail "$bundle_dir/bundle-summary.json missing"
[ "$(stat -c %U "$bundle_dir/bundle-summary.json")" = "$smoke_user" ] \
    || fail "the dry-run bundle is not owned by $smoke_user"
[ ! -e "$bundle_root/debug-output" ] || fail "the debug CLI wrote into $bundle_root/debug-output"
as_smoke_user d4lc-debug asset-presets | grep -q '"name": "1080p_SMF"' \
    || fail "d4lc-debug asset-presets did not list the bundled presets as $smoke_user"
[ "$(bundle_digest)" = "$digest_before" ] || fail "running the debug CLI changed $bundle_root"

# The desktop app has no --help/--version, so start it under Xvfb through the
# installed launcher. Containers have no D-Bus session bus, so this also
# proves the app degrades instead of crashing without one, and the expected
# degradations are asserted explicitly: no tray icon (StatusNotifierItem
# registration needs the bus) and the single-instance fallback without
# XDG_RUNTIME_DIR: a private per-user directory below the temporary directory
# (d4lc-<uid>, mode 0700, owned by the user), checked on disk as well.
apt-get install -y --no-install-recommends xvfb xauth fonts-dejavu-core

unset DBUS_SESSION_BUS_ADDRESS XDG_RUNTIME_DIR WAYLAND_DISPLAY
[ ! -S "/run/user/$(id -u)/bus" ] || fail "a session bus socket exists; this test needs a bus-less host"

# shellcheck source=scripts/startup_markers.sh
source "$(dirname "$0")/startup_markers.sh"

log=$(mktemp)
status=0
xvfb-run -a timeout --kill-after=2s 8s d4lc >"$log" 2>&1 || status=$?
if [ "$status" -ne 124 ]; then
    cat "$log" >&2
    fail "d4lc exited early with status $status under Xvfb (124 means still running at the timeout)"
fi
problems=$(check_startup_log "$log" || true)
fallback_dir="${TMPDIR:-/tmp}/d4lc-$(id -u)"
grep -Fq "[d4lc] single-instance: XDG_RUNTIME_DIR is not set; using the private fallback $fallback_dir" "$log" \
    || problems="${problems:+$problems
}missing expected bus-less line: single-instance private fallback $fallback_dir"
if [ -L "$fallback_dir" ] || [ ! -d "$fallback_dir" ]; then
    problems="${problems:+$problems
}single-instance fallback $fallback_dir is not a real directory"
elif [ "$(stat -c '%u %a' "$fallback_dir")" != "$(id -u) 700" ]; then
    problems="${problems:+$problems
}single-instance fallback $fallback_dir is $(stat -c 'uid %u mode %a' "$fallback_dir"), expected uid $(id -u) mode 700"
fi
expected='\[d4lc\] tray: failed to register StatusNotifierItem .*No system tray icon will be shown'
grep -Eq "$expected" "$log" || problems="${problems:+$problems
}missing expected bus-less line: $expected"
if [ -n "$problems" ]; then
    cat "$log" >&2
    fail "startup log check failed:
$problems"
fi

echo "smoke_deb_install: ok"
