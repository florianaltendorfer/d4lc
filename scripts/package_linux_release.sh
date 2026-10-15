#!/usr/bin/env bash
# scripts/package_linux_release.sh — build the release bundle (tarball, .deb
# root, AppDir) and optionally the .deb and the AppImage.
#
# Launchers: the desktop app (d4lc) starts in the bundle root, which it reads
# its game data from. The debug CLI (d4lc-debug) gets the bundle root as
# D4LC_ASSET_ROOT and keeps the caller's working directory, so its relative
# paths and default debug-output/ land where the user runs it; nothing is
# written into the installed bundle.
#
# AppImage: AppRun starts the desktop app. The bundled debug CLI is started
# through the same AppImage with --debug-cli as the first argument:
#   ./d4lc-<version>-x86_64.AppImage --debug-cli <command> [options]
# (without FUSE add --appimage-extract-and-run, or set APPIMAGE_EXTRACT_AND_RUN=1).
set -euo pipefail

cd "$(dirname "$0")/.."

PACKAGE_NAME="d4lc"
APP_PACKAGE="d4lc-app"
CLI_PACKAGE="d4lc-cli"
CLI_BINARY="d4lc"
# The shipped desktop app needs the live runtime, native OCR and the
# Wayland ScreenCast capture; the debug CLI only needs native OCR.
APP_FEATURES="real-runtime leptess-backend wayland-portal"
CLI_FEATURES="leptess-backend"
DEFAULT_OUTPUT_DIR="target/package/linux-release"
DEFAULT_BUILD_BINARY="target/release/d4lc-app"
DEFAULT_DEBUG_BINARY="target/release/d4lc"
# Tesseract, Leptonica and their codec chain are bundled; what is left is
# the host stack the bundle deliberately does not carry (see
# bundle_shared_libraries). The glibc floor is the release build baseline.
DEFAULT_DEB_DEPENDS="libc6 (>= 2.35), libstdc++6, libgcc-s1, libx11-6, libxcursor1, libxext6, libxi6, libxkbcommon-x11-0, libfontconfig1, libfreetype6, libcurl4, libssl3, zlib1g, libpipewire-0.3-0"
# xdg-open serves the title-bar links and "open folder"; a Chromium binary is
# needed only for the D2Core/D4Builds/Mobalytics build imports.
DEFAULT_DEB_RECOMMENDS="tesseract-ocr-eng, xdg-utils"
DEFAULT_DEB_SUGGESTS="chromium | chromium-browser"
DEFAULT_MAINTAINER="Florian Josef Altendorfer <info@altendorfer.dev>"
CHECKSUM_FILENAME="SHA256SUMS"
SUMMARY_FILENAME="package-summary.txt"

OUTPUT_DIR="$DEFAULT_OUTPUT_DIR"
VERSION=""
BINARY_PATH="$DEFAULT_BUILD_BINARY"
DEBUG_BINARY_PATH=""
PRIMARY_NAME="d4lc"
DEBUG_NAME="d4lc-debug"
TESSDATA_DIR=""
BUILD_BINARY=1
BUILD_APPIMAGE=0
BUILD_DEB=0
DEB_DEPENDS="${D4LC_DEB_DEPENDS:-$DEFAULT_DEB_DEPENDS}"
DEB_RECOMMENDS="${D4LC_DEB_RECOMMENDS:-$DEFAULT_DEB_RECOMMENDS}"
DEB_SUGGESTS="${D4LC_DEB_SUGGESTS:-$DEFAULT_DEB_SUGGESTS}"
MAINTAINER="${D4LC_MAINTAINER:-$DEFAULT_MAINTAINER}"
DESKTOP_FILE="${D4LC_DESKTOP_FILE:-crates/d4lc-app/assets/d4lc.desktop}"
APP_ICON_PNG="${D4LC_APP_ICON:-crates/d4lc-app/assets/d4lc.png}"
APPIMAGE_RUNTIME_FILE="${D4LC_APPIMAGE_RUNTIME_FILE:-}"
# Extra documents installed into the bundle's docs/ (name taken from the
# file's basename); the release workflow passes the generated Rust
# dependency notices here.
DOC_FILES=()

usage() {
    cat <<'EOF'
Usage: ./scripts/package_linux_release.sh [options]

Build a release-shaped Linux bundle for d4lc, then stage AppDir and deb roots.

Options:
  --output-dir PATH         Packaging output directory. Default: target/package/linux-release
  --version VERSION         Override the package version from crates/d4lc-cli/Cargo.toml
  --binary PATH             Primary payload binary. Default: target/release/d4lc-app (the desktop app)
  --debug-binary PATH       Debug CLI binary bundled as bin/d4lc-debug. Default when building: target/release/d4lc
  --primary-name NAME       User-facing launcher name installed in /usr/bin. Default: d4lc
  --debug-name NAME         Debug CLI launcher name installed in /opt/d4lc/bin. Default: d4lc-debug
  --skip-build              Skip cargo build and trust --binary / --debug-binary (build them with
                            d4lc-app --features "real-runtime leptess-backend wayland-portal" and
                            d4lc-cli --features leptess-backend)
  --tessdata-dir PATH       Copy a tessdata directory into the bundle and export TESSDATA_PREFIX
  --doc-file PATH           Install an additional document into the bundle's docs/
  --appimage                Build an AppImage with appimagetool after staging AppDir
  --deb                     Build a .deb with dpkg-deb after staging deb-root
  --help                    Show this help

Environment overrides:
  D4LC_DEB_DEPENDS      Debian dependency string. Default: libc6 (>= 2.35), libstdc++6, libgcc-s1, libx11-6, libxcursor1, libxext6, libxi6, libxkbcommon-x11-0, libfontconfig1, libfreetype6, libcurl4, libssl3, zlib1g, libpipewire-0.3-0
  D4LC_DEB_RECOMMENDS   Debian recommends string. Default: tesseract-ocr-eng, xdg-utils
  D4LC_DEB_SUGGESTS     Debian suggests string. Default: chromium | chromium-browser
  D4LC_MAINTAINER       Debian Maintainer field. Default: Florian Josef Altendorfer <info@altendorfer.dev>
  D4LC_DESKTOP_FILE     Override the source .desktop file path. Default: crates/d4lc-app/assets/d4lc.desktop
  D4LC_APP_ICON         Override the source app icon path. Default: crates/d4lc-app/assets/d4lc.png
  D4LC_APPIMAGE_RUNTIME_FILE  AppImage runtime passed to appimagetool --runtime-file. Default: appimagetool downloads the unpinned continuous runtime
EOF
}

fail() {
    echo "package_linux_release.sh: $*" >&2
    exit 1
}

require_file() {
    local path=$1
    [ -e "$path" ] || fail "required path missing: $path"
}

require_command() {
    local command_name=$1
    command -v "$command_name" >/dev/null 2>&1 || fail "required command not found: $command_name"
}

# The version and launcher names become file names, wrapper script text and
# deb control fields; reject anything that could escape a path or inject a
# newline before the first file is touched.
validate_package_version() {
    [[ "$1" =~ ^[0-9A-Za-z][0-9A-Za-z.+~_-]*$ ]] || fail "invalid package version: $1"
}

validate_name() {
    [[ "$1" =~ ^[0-9A-Za-z][0-9A-Za-z._-]*$ ]] || fail "invalid name: $1"
}

package_version() {
    awk -F'"' '/^version = / { print $2; exit }' crates/d4lc-cli/Cargo.toml
}

git_revision() {
    # Full commit id so every artifact maps to exactly one build commit
    # (a short id is unique only with high probability); a dirty tree is
    # flagged because its artifacts no longer match any commit.
    local revision
    revision="$(git rev-parse HEAD 2>/dev/null)" || { echo "unknown"; return; }
    if [ -n "$(git status --porcelain 2>/dev/null)" ]; then
        revision="${revision}-dirty"
    fi
    echo "$revision"
}

map_deb_arch() {
    case "$(uname -m)" in
        x86_64) echo "amd64" ;;
        aarch64) echo "arm64" ;;
        *) uname -m ;;
    esac
}

map_appimage_arch() {
    case "$(uname -m)" in
        x86_64) echo "x86_64" ;;
        aarch64) echo "aarch64" ;;
        *) uname -m ;;
    esac
}

copy_tree() {
    local source=$1
    local destination=$2
    mkdir -p "$(dirname "$destination")"
    cp -a "$source" "$destination"
}

write_runtime_requirements() {
    local output_path=$1

    local tessdata_requirement
    if [ -n "$TESSDATA_DIR" ]; then
        tessdata_requirement="- OCR language packs are bundled under tessdata/ and exported through TESSDATA_PREFIX by the launcher wrappers (upstream shape: traineddata inside the application folder). A user-set TESSDATA_PREFIX, or a bundle built without --tessdata-dir, falls back to the host's system tessdata."
    else
        tessdata_requirement="- Tessdata language files for the OCR languages you enable. This bundle was packaged without --tessdata-dir, so OCR uses the host's system tessdata."
    fi

    cat >"$output_path" <<EOF
d4lc Linux release requirements
================================

Build-time requirements
- Rust toolchain with cargo
- pkg-config
- libtesseract-dev and libleptonica-dev (or distro-equivalent header packages)
- libpipewire-0.3-dev and clang/libclang (Wayland ScreenCast capture)
- An X11-capable build machine if you want to run the packaged smoke test locally

Runtime requirements
- X11 session with DISPLAY set (the primary release target), or a Wayland session (see below).
- This file states requirements, not validation: the checks that ran on this build are listed in its release notes, and a check in a live X11 game session is recorded there per release, never implied.
- No helper programs: game-window discovery, capture and the overlay talk to the X server directly. Only the debug CLI's optional x11-command capture backend runs xwd (Debian/Ubuntu: x11-apps; Arch: xorg-xwd).
- X11 server support for the capture/overlay path:
  - MIT-SHM enables the fast capture path; XGetImage is the fallback
  - SHAPE provides click-through input for the native overlay
- Tesseract, Leptonica and their image-codec libraries are bundled under lib/. The host provides glibc, libstdc++/libgcc_s, the X11 client libraries, fontconfig/freetype, libcurl/OpenSSL, zlib and PipeWire (libpipewire-0.3); the .deb declares these as dependencies.
$tessdata_requirement

Wayland
- Capture uses the ScreenCast portal: xdg-desktop-portal with a PipeWire backend and libpipewire-0.3 on the host.
- The overlay uses wlr-layer-shell and game-window focus uses KDE's plasma window management; other compositors get an explicit capability report instead.
- Wayland support is covered by automated protocol tests only and has not been validated on a live compositor.

Provenance documents included in this bundle
- docs/license-provenance.json
- docs/upstream-file-manifest.json
- docs/asset-manifest.json
- docs/LICENSE (d4lc, MIT)
- docs/upstream-LICENSE
- docs/slint-LICENSE (royalty-free Slint desktop license elected for the UI stack)
- docs/tessdata-LICENSE and docs/tessdata-provenance.md (Apache-2.0 OCR language packs)
- docs/rust-dependency-notices.html (generated license notices for the linked Rust crates)
- docs/LICENSE-PROVENANCE-NOTICE.md
- docs/Inconsolata-OFL.txt (licence of the embedded overlay font)
- docs/bundled-fonts.json
- docs/third-party/<package>.copyright (licence texts of the bundled shared libraries, listed in docs/bundled-libs.txt)
- upstream.lock (the reviewed upstream baseline; d4lc-debug upstream-changed reads it)

Debug CLI (d4lc-debug)
- The launcher exports the bundle root as D4LC_ASSET_ROOT and keeps your working directory: relative paths and the default debug-output/ directories resolve against it, never against the bundle.
- Settings come from the desktop app's config root: --config-root, else D4LC_CONFIG_ROOT, else ~/.config/d4lc.
- AppImage: run the AppImage with --debug-cli as the first argument.
- upstream-report, upstream-manifest, upstream-provenance, upstream-asset-parity, and perf-detect-offline without --input need a developer checkout; they refuse to run from this bundle.
EOF
}

# Identity of the AppImage runtime for build-metadata.txt: its digest, not
# its path (paths are host-specific and may name the builder's home).
appimage_runtime_identity() {
    if [ "$BUILD_APPIMAGE" -ne 1 ]; then
        echo "NONE (no AppImage built)"
    elif [ -n "$APPIMAGE_RUNTIME_FILE" ]; then
        [ -f "$APPIMAGE_RUNTIME_FILE" ] || fail "D4LC_APPIMAGE_RUNTIME_FILE does not exist: $APPIMAGE_RUNTIME_FILE"
        require_command sha256sum
        echo "sha256:$(sha256sum "$APPIMAGE_RUNTIME_FILE" | cut -d' ' -f1)"
    else
        echo "UNPINNED (downloaded by appimagetool)"
    fi
}

write_build_metadata() {
    local output_path=$1
    local package_version_value=$2
    local appimage_runtime
    appimage_runtime="$(appimage_runtime_identity)"

    cat >"$output_path" <<EOF
package_name=$PACKAGE_NAME
package_version=$package_version_value
git_revision=$(git_revision)
binary_path=$BINARY_PATH
debug_binary_path=${DEBUG_BINARY_PATH:-NONE}
primary_name=$PRIMARY_NAME
debug_name=$DEBUG_NAME
build_binary=$BUILD_BINARY
app_features=$APP_FEATURES
cli_features=$CLI_FEATURES
app_tray_feature=enabled (tray-icon is a d4lc-app default feature)
tessdata_dir=${TESSDATA_DIR:-SYSTEM}
deb_depends=$DEB_DEPENDS
deb_recommends=$DEB_RECOMMENDS
deb_suggests=$DEB_SUGGESTS
maintainer=$MAINTAINER
desktop_file=$DESKTOP_FILE
appimage_runtime=$appimage_runtime
EOF
}

# The line a launcher runs before exec: the desktop app starts in the bundle
# root; the debug CLI reads the bundle through D4LC_ASSET_ROOT and keeps the
# caller's working directory for its input and output paths.
launcher_root_setup() {
    local launcher_name=$1
    if [ "$launcher_name" = "$DEBUG_NAME" ]; then
        # shellcheck disable=SC2016 # expanded by the generated launcher
        echo 'export D4LC_ASSET_ROOT="$bundle_root"'
    else
        # shellcheck disable=SC2016 # expanded by the generated launcher
        echo 'cd "$bundle_root"'
    fi
}

write_bundle_wrapper() {
    # Args: $1=output_path $2=launcher_name (e.g. d4lc or d4lc-debug)
    local output_path=$1
    local launcher_name=$2

    cat >"$output_path" <<EOF
#!/usr/bin/env bash
# Auto-generated by scripts/package_linux_release.sh
# Wraps the $launcher_name payload with the relocatable bundle environment.
set -euo pipefail

bundle_root="\$(cd "\$(dirname "\${BASH_SOURCE[0]}")/.." && pwd)"
if [ -d "\$bundle_root/lib" ]; then
    export LD_LIBRARY_PATH="\$bundle_root/lib\${LD_LIBRARY_PATH:+:\$LD_LIBRARY_PATH}"
fi
if [ -d "\$bundle_root/tessdata" ] && [ -z "\${TESSDATA_PREFIX:-}" ]; then
    export TESSDATA_PREFIX="\$bundle_root/tessdata"
fi

$(launcher_root_setup "$launcher_name")
exec "\$bundle_root/bin/$launcher_name-real" "\$@"
EOF
    chmod 0755 "$output_path"
}

write_deb_wrapper() {
    # /usr/bin/<primary> wrapper that execs the bundle's primary payload.
    local output_path=$1
    local launcher_name=$2

    cat >"$output_path" <<EOF
#!/usr/bin/env bash
# Auto-generated by scripts/package_linux_release.sh
set -euo pipefail

bundle_root="/opt/d4lc"
if [ -d "\$bundle_root/lib" ]; then
    export LD_LIBRARY_PATH="\$bundle_root/lib\${LD_LIBRARY_PATH:+:\$LD_LIBRARY_PATH}"
fi
if [ -d "\$bundle_root/tessdata" ] && [ -z "\${TESSDATA_PREFIX:-}" ]; then
    export TESSDATA_PREFIX="\$bundle_root/tessdata"
fi

$(launcher_root_setup "$launcher_name")
exec "\$bundle_root/bin/$launcher_name-real" "\$@"
EOF
    chmod 0755 "$output_path"
}

write_appimage_apprun() {
    # AppRun inside the AppDir root; execs the user-facing primary launcher,
    # or the debug CLI launcher ($3, empty when none is bundled) when the
    # first argument is --debug-cli.
    local output_path=$1
    local launcher_name=$2
    local debug_launcher_name=$3
    local debug_dispatch=""
    if [ -n "$debug_launcher_name" ]; then
        debug_dispatch="if [ \"\${1:-}\" = \"--debug-cli\" ]; then
    shift
    exec \"\$appdir/usr/bin/$debug_launcher_name\" \"\$@\"
fi"
    fi

    cat >"$output_path" <<EOF
#!/usr/bin/env bash
# Auto-generated by scripts/package_linux_release.sh
# Starts the desktop app; --debug-cli as the first argument starts the
# bundled debug CLI with the remaining arguments instead.
set -euo pipefail

appdir="\$(cd "\$(dirname "\${BASH_SOURCE[0]}")" && pwd)"
$debug_dispatch
exec "\$appdir/usr/bin/$launcher_name" "\$@"
EOF
    chmod 0755 "$output_path"
}

write_appimage_wrapper() {
    local output_path=$1
    local launcher_name=$2

    cat >"$output_path" <<EOF
#!/usr/bin/env bash
# Auto-generated by scripts/package_linux_release.sh
set -euo pipefail

appdir="\$(cd "\$(dirname "\${BASH_SOURCE[0]}")/../.." && pwd)"
bundle_root="\$appdir/usr/lib/d4lc"
if [ -d "\$bundle_root/lib" ]; then
    export LD_LIBRARY_PATH="\$bundle_root/lib\${LD_LIBRARY_PATH:+:\$LD_LIBRARY_PATH}"
fi
if [ -d "\$bundle_root/tessdata" ] && [ -z "\${TESSDATA_PREFIX:-}" ]; then
    export TESSDATA_PREFIX="\$bundle_root/tessdata"
fi

$(launcher_root_setup "$launcher_name")
exec "\$bundle_root/bin/$launcher_name-real" "\$@"
EOF
    chmod 0755 "$output_path"
}

copy_desktop_file() {
    # Copy the canonical .desktop file from the repo and rewrite Exec= to the
    # primary launcher name. Terminal stays false because the desktop app is a
    # GUI, not a CLI.
    local output_path=$1
    local launcher_name=$2

    require_file "$DESKTOP_FILE"
    sed -E "s|^Exec=.*|Exec=$launcher_name|" "$DESKTOP_FILE" >"$output_path"
    # Validate the file that actually ships. desktop-file-validate is part
    # of desktop-file-utils; treat its absence as a packaging error so the
    # gate cannot silently skip the check.
    if command -v desktop-file-validate >/dev/null; then
        desktop-file-validate "$output_path"
    else
        fail "desktop-file-validate not found; install desktop-file-utils"
    fi
}

write_appimage_icon() {
    # Install the real bundled PNG as the AppImage icon (AppDir root +
    # hicolor theme entry); .DirIcon points at it per AppImage convention.
    local output_path=$1

    install -m 0644 "$APP_ICON_PNG" "$output_path"
}

prepare_common_bundle() {
    local bundle_root=$1
    local package_version_value=$2

    rm -rf "$bundle_root"
    mkdir -p "$bundle_root/bin" "$bundle_root/assets/provenance" "$bundle_root/docs" \
        "$bundle_root/share/icons"

    install -m 0755 "$BINARY_PATH" "$bundle_root/bin/${PRIMARY_NAME}-real"
    if [ -n "$DEBUG_BINARY_PATH" ]; then
        require_file "$DEBUG_BINARY_PATH"
        install -m 0755 "$DEBUG_BINARY_PATH" "$bundle_root/bin/${DEBUG_NAME}-real"
    fi
    # Relocatable launchers for the tarball: they put bundle_root/lib on the
    # library path, export the bundled tessdata, and start the app in the
    # bundle root (the debug CLI gets it as D4LC_ASSET_ROOT instead).
    write_bundle_wrapper "$bundle_root/bin/${PRIMARY_NAME}" "$PRIMARY_NAME"
    if [ -n "$DEBUG_BINARY_PATH" ]; then
        write_bundle_wrapper "$bundle_root/bin/${DEBUG_NAME}" "$DEBUG_NAME"
    fi
    copy_tree "assets/original" "$bundle_root/assets/original"
    copy_tree "assets/d4lc" "$bundle_root/assets/d4lc"
    install -m 0644 "$APP_ICON_PNG" "$bundle_root/share/icons/${PRIMARY_NAME}.png"
    install -m 0644 "assets/provenance/asset-manifest.json" "$bundle_root/assets/provenance/asset-manifest.json"
    install -m 0644 "assets/provenance/asset-manifest.json" "$bundle_root/docs/asset-manifest.json"
    install -m 0644 "licenses/license-provenance.json" "$bundle_root/docs/license-provenance.json"
    install -m 0644 "licenses/upstream-file-manifest.json" "$bundle_root/docs/upstream-file-manifest.json"
    install -m 0644 "licenses/Inconsolata-OFL.txt" "$bundle_root/docs/Inconsolata-OFL.txt"
    install -m 0644 "licenses/bundled-fonts.json" "$bundle_root/docs/bundled-fonts.json"
    install -m 0644 "LICENSE" "$bundle_root/docs/LICENSE"
    install -m 0644 "upstream/Diablo4Companion/LICENSE" "$bundle_root/docs/upstream-LICENSE"
    install -m 0644 "licenses/slint-LICENSE" "$bundle_root/docs/slint-LICENSE"
    install -m 0644 "packaging/LICENSE-PROVENANCE-NOTICE.md" "$bundle_root/docs/LICENSE-PROVENANCE-NOTICE.md"
    install -m 0644 "packaging/linux-packaging.md" "$bundle_root/docs/linux-packaging.md"
    install -m 0644 "README.md" "$bundle_root/docs/README.md"
    # The reviewed upstream baseline of this build; d4lc-debug upstream-changed
    # compares a remote head against it.
    install -m 0644 "upstream.lock" "$bundle_root/upstream.lock"

    if [ -n "$TESSDATA_DIR" ]; then
        mkdir -p "$bundle_root/tessdata"
        cp -a "$TESSDATA_DIR"/. "$bundle_root/tessdata/"
        # The traineddata packs are Apache-2.0 files from the tesseract
        # project, not upstream companion assets; the bundle carries their
        # license and origin record alongside.
        install -m 0644 licenses/tessdata-LICENSE "$bundle_root/docs/tessdata-LICENSE"
        install -m 0644 licenses/tessdata-provenance.md "$bundle_root/docs/tessdata-provenance.md"
    fi

    local doc
    for doc in "${DOC_FILES[@]}"; do
        install -m 0644 "$doc" "$bundle_root/docs/$(basename "$doc")"
    done

    write_runtime_requirements "$bundle_root/docs/runtime-requirements.txt"
    write_build_metadata "$bundle_root/docs/build-metadata.txt" "$package_version_value"
    if command -v ldd >/dev/null 2>&1; then
        ldd "$BINARY_PATH" >"$bundle_root/docs/runtime-libs.txt" || true
        if [ -n "$DEBUG_BINARY_PATH" ]; then
            echo "--- $DEBUG_BINARY_PATH ---" >>"$bundle_root/docs/runtime-libs.txt"
            ldd "$DEBUG_BINARY_PATH" >>"$bundle_root/docs/runtime-libs.txt" || true
        fi
    fi

    bundle_shared_libraries "$bundle_root"
}

library_package_name() {
    # Print the distro package that owns the library file $1 (dpkg or pacman).
    # $2 is the symlink-resolved path. dpkg on Ubuntu 22.04 records the
    # libraries of its essential packages under /lib even though /lib is
    # /usr/lib there, so it only knows the path the loader printed ($1), not
    # the resolved /usr/lib one.
    local loader_path=$1
    local real=$2
    local pkg="" candidate
    for candidate in "$loader_path" "$real"; do
        if command -v dpkg >/dev/null 2>&1; then
            pkg="$(dpkg -S "$candidate" 2>/dev/null | head -n1 | sed -E 's/[:,].*//')" || pkg=""
        fi
        if [ -z "$pkg" ] && command -v pacman >/dev/null 2>&1; then
            pkg="$(pacman -Qoq "$candidate" 2>/dev/null | head -n1)" || pkg=""
        fi
        [ -z "$pkg" ] || break
    done
    [ -n "$pkg" ] || return 1
    echo "$pkg"
}

write_library_copyright() {
    # Write the licence text of distro package $1 to $2. Debian-style hosts
    # ship /usr/share/doc/<pkg>/copyright (a missing file is an error); Arch
    # ships /usr/share/licenses/<pkg>/ plus shared SPDX texts under
    # /usr/share/licenses/spdx/.
    local pkg=$1
    local output_path=$2

    if command -v dpkg >/dev/null 2>&1 && dpkg -s "$pkg" >/dev/null 2>&1; then
        [ -s "/usr/share/doc/$pkg/copyright" ] || return 1
        cp -L "/usr/share/doc/$pkg/copyright" "$output_path"
        return 0
    fi

    command -v pacman >/dev/null 2>&1 || return 1
    pacman -Qi "$pkg" >/dev/null 2>&1 || return 1
    local license_ids license_id license_file found=0
    license_ids="$(pacman -Qi "$pkg" | awk -F' *: *' '/^Licenses/ { print $2; exit }')"
    {
        echo "Package: $pkg $(pacman -Q "$pkg" | awk '{print $2}')"
        echo "Licenses: $license_ids"
        echo "URL: $(pacman -Qi "$pkg" | awk -F' *: *' '/^URL/ { print $2; exit }')"
    } >"$output_path"
    if [ -d "/usr/share/licenses/$pkg" ]; then
        while IFS= read -r license_file; do
            printf '\n==> %s <==\n' "$license_file" >>"$output_path"
            cat "$license_file" >>"$output_path"
            found=1
        done < <(find -L "/usr/share/licenses/$pkg" -type f | sort)
    fi
    for license_id in $license_ids; do
        license_file="/usr/share/licenses/spdx/${license_id}.txt"
        if [ -f "$license_file" ]; then
            printf '\n==> %s <==\n' "$license_file" >>"$output_path"
            cat "$license_file" >>"$output_path"
            found=1
        fi
    done
    # Arch ships no text for licences it treats as common (and the legacy
    # GPL/LGPL/MIT ids have none); the recorded metadata is all there is.
    if [ "$found" -eq 0 ]; then
        echo "package_linux_release.sh: warning: $pkg ships no licence text; recorded its pacman licence metadata only" >&2
    fi
}

bundle_shared_libraries() {
    # Bundle non-core shared libraries next to the payload so the artifacts
    # run on hosts whose OCR/codec library versions differ from the build
    # host (e.g. Ubuntu liblept.so.5 vs Arch libleptonica.so.6). Everything
    # that is tied to the host stays on the host (skip list below). The
    # wrappers prepend bundle_root/lib to LD_LIBRARY_PATH, so a bundled copy
    # of a host-tied library would win over the host's and break it. Every
    # bundled library's distro copyright file is copied to docs/third-party/.
    local bundle_root=$1
    # Without ldd nothing would be bundled and the launchers would silently
    # run on the host's OCR libraries.
    require_command ldd

    local skip_patterns=(
        # glibc and its loader: must match the host kernel/loader.
        'ld-linux' 'libc\.so' 'libm\.so' 'libdl\.' 'libpthread' 'librt\.' 'libresolv'
        # C++/gcc runtimes: the build baseline is the oldest supported distro,
        # so the host's copy is at least as new; an older bundled copy would
        # break host plugins (Mesa, PipeWire/SPA) built against newer symbols.
        'libstdc\+\+' 'libgcc_s'
        # X11 client stack (libX11, libXext, libXi, ..., every libxcb*):
        # universal and ABI-stable, and Mesa/GLX interop needs the host's
        # copies. libxkbcommon follows the host keymap configuration.
        'libX' 'libxcb' 'libxshmfence' 'libxkbcommon' 'libxkbregistry'
        # Wayland client libraries: the host's compositor and EGL/Mesa
        # Wayland platform code link against the host copy.
        'libwayland'
        # OpenGL/EGL/Vulkan/GBM/DRM: user-space half of the host GPU driver
        # (Mesa or NVIDIA); the vendor ICD must match the installed driver.
        'libGL' 'libEGL' 'libOpenGL' 'libvulkan' 'libgbm' 'libdrm' 'libglapi'
        # PipeWire and SPA: the client library has to match the host daemon,
        # and SPA/PipeWire modules are loaded from the distro's plugin
        # directories, which are built against the host's libpipewire/libspa.
        'libpipewire' 'libspa'
        # D-Bus, systemd and udev client libraries: they talk to the host's
        # bus, logind and device manager, and libpipewire links against them.
        'libdbus' 'libsystemd' 'libudev'
        # fontconfig/freetype must match the host font configuration.
        'libfontconfig\.' 'libfreetype\.'
        # TLS/HTTP stack must match the host CA store and proxy setup.
        'libcurl\.' 'libssl\.' 'libcrypto\.' 'libbrotli' 'libnghttp2' 'libnghttp3'
        'libpsl' 'libssh2' 'librtmp'
        # Compression and glib: universal, ABI-stable core libraries.
        'libz\.' 'libbz2\.' 'liblzma\.' 'libzstd\.'
        'libglib-2\.0' 'libgio-2\.0' 'libgobject-2\.0'
    )
    local skip_re
    skip_re="^($(IFS='|'; echo "${skip_patterns[*]}"))"

    mkdir -p "$bundle_root/lib" "$bundle_root/docs/third-party"
    local binaries=("$bundle_root/bin/${PRIMARY_NAME}-real")
    if [ -n "$DEBUG_BINARY_PATH" ] && [ -f "$bundle_root/bin/${DEBUG_NAME}-real" ]; then
        binaries+=("$bundle_root/bin/${DEBUG_NAME}-real")
    fi

    local lib base pkg binary ldd_output
    local -A packages_seen=()
    for binary in "${binaries[@]}"; do
        # Resolved before the loop: inside a process substitution an ldd
        # failure, or a library the loader cannot find, would go unnoticed.
        ldd_output="$(ldd "$binary")" || fail "ldd failed for $binary"
        if grep -q 'not found' <<<"$ldd_output"; then
            fail "unresolved libraries for $binary:
$(grep 'not found' <<<"$ldd_output")"
        fi
        while read -r lib; do
            base="$(basename "$lib")"
            if echo "$base" | grep -qE "$skip_re"; then
                continue
            fi
            if [ -f "$lib" ] && [ ! -f "$bundle_root/lib/$base" ]; then
                # A missing bundled library would silently break the
                # bundle's LD_LIBRARY_PATH story; fail instead.
                cp -L "$lib" "$bundle_root/lib/$base" \
                    || fail "failed to bundle library $lib"
                pkg="$(library_package_name "$lib" "$(readlink -f "$lib")")" \
                    || fail "cannot resolve the distro package of bundled library $lib"
                if [ -z "${packages_seen[$pkg]:-}" ]; then
                    write_library_copyright "$pkg" "$bundle_root/docs/third-party/$pkg.copyright" \
                        || fail "no copyright file found for package $pkg (bundled library $lib)"
                    packages_seen[$pkg]=1
                fi
            fi
        done < <(awk '{if ($3 ~ /^\//) print $3}' <<<"$ldd_output" | sort -u)
    done

    if [ -d "$bundle_root/lib" ] && [ -n "$(ls -A "$bundle_root/lib" 2>/dev/null)" ]; then
        ls -1 "$bundle_root/lib" >"$bundle_root/docs/bundled-libs.txt"
    fi
}

stage_deb_root() {
    local deb_root=$1
    local common_bundle_root=$2
    local package_version_value=$3
    local deb_arch=$4
    # Debian needs '~rc.1' to sort before stable; keep SemVer in artifact identities.
    local version_core=${package_version_value%%+*}
    local deb_version="${version_core/-/\~}${package_version_value#"$version_core"}"

    rm -rf "$deb_root"
    mkdir -p "$deb_root/DEBIAN" "$deb_root/usr/bin" "$deb_root/usr/share/doc" \
        "$deb_root/usr/share/applications" "$deb_root/usr/share/icons/hicolor/256x256/apps"
    copy_tree "$common_bundle_root" "$deb_root/opt/d4lc"
    ln -s ../../../opt/d4lc/docs "$deb_root/usr/share/doc/d4lc"
    write_deb_wrapper "$deb_root/usr/bin/${PRIMARY_NAME}" "$PRIMARY_NAME"
    if [ -n "$DEBUG_BINARY_PATH" ]; then
        write_deb_wrapper "$deb_root/usr/bin/${DEBUG_NAME}" "$DEBUG_NAME"
    fi
    copy_desktop_file "$deb_root/usr/share/applications/${PRIMARY_NAME}.desktop" "$PRIMARY_NAME"
    install -m 0644 "$APP_ICON_PNG" \
        "$deb_root/usr/share/icons/hicolor/256x256/apps/${PRIMARY_NAME}.png"

    cat >"$deb_root/DEBIAN/control" <<EOF
Package: $PACKAGE_NAME
Version: $deb_version
Section: games
Priority: optional
Architecture: $deb_arch
Maintainer: $MAINTAINER
Depends: $DEB_DEPENDS
Recommends: $DEB_RECOMMENDS
Suggests: $DEB_SUGGESTS
Description: Linux-first Rust-native Diablo IV companion toolkit
 X11-first release bundle with upstream data/provenance docs, native OCR, and
 packaged release smoke-test notes. The primary payload is the Slint desktop
 app (${PRIMARY_NAME}). A debug CLI is bundled as ${DEBUG_NAME} when present.
 Native Wayland capture/overlay parity is not supported by this package.
EOF
}

stage_appdir() {
    local appdir=$1
    local common_bundle_root=$2

    rm -rf "$appdir"
    mkdir -p \
        "$appdir/usr/bin" \
        "$appdir/usr/lib" \
        "$appdir/usr/share/applications" \
        "$appdir/usr/share/doc" \
        "$appdir/usr/share/icons/hicolor/256x256/apps"
    copy_tree "$common_bundle_root" "$appdir/usr/lib/d4lc"
    ln -s ../../lib/d4lc/docs "$appdir/usr/share/doc/d4lc"
    write_appimage_apprun "$appdir/AppRun" "$PRIMARY_NAME" "${DEBUG_BINARY_PATH:+$DEBUG_NAME}"
    write_appimage_wrapper "$appdir/usr/bin/${PRIMARY_NAME}" "$PRIMARY_NAME"
    if [ -n "$DEBUG_BINARY_PATH" ]; then
        write_appimage_wrapper "$appdir/usr/bin/${DEBUG_NAME}" "$DEBUG_NAME"
    fi
    copy_desktop_file "$appdir/${PRIMARY_NAME}.desktop" "$PRIMARY_NAME"
    cp "$appdir/${PRIMARY_NAME}.desktop" "$appdir/usr/share/applications/${PRIMARY_NAME}.desktop"
    write_appimage_icon "$appdir/${PRIMARY_NAME}.png"
    cp "$appdir/${PRIMARY_NAME}.png" "$appdir/usr/share/icons/hicolor/256x256/apps/${PRIMARY_NAME}.png"
    ln -s "${PRIMARY_NAME}.png" "$appdir/.DirIcon"
}

create_tarball() {
    local common_root=$1
    local artifacts_dir=$2
    local package_version_value=$3
    local appimage_arch=$4
    local tarball_path="$artifacts_dir/${PACKAGE_NAME}-${package_version_value}-linux-${appimage_arch}.tar.gz"

    mkdir -p "$artifacts_dir"
    tar -C "$common_root" -czf "$tarball_path" "$PACKAGE_NAME"
    echo "$tarball_path"
}

write_artifact_checksums() {
    local output_path=$1
    shift

    require_command sha256sum
    : >"$output_path"

    local artifact_path
    local artifact_dir
    local artifact_name
    for artifact_path in "$@"; do
        require_file "$artifact_path"
        artifact_dir=$(dirname "$artifact_path")
        artifact_name=$(basename "$artifact_path")
        (
            cd "$artifact_dir"
            sha256sum "$artifact_name"
        ) >>"$output_path"
    done
}

write_package_summary() {
    local output_path=$1
    local package_version_value=$2
    local common_bundle_root=$3
    local deb_root=$4
    local appdir=$5
    local artifacts_dir=$6
    local tarball_path=$7
    local checksum_path=$8
    local deb_path=$9
    local appimage_path=${10}

    cat >"$output_path" <<EOF
PACKAGE_LINUX_RELEASE
version=$package_version_value
git_revision=$(git_revision)
binary=$BINARY_PATH
debug_binary=${DEBUG_BINARY_PATH:-NONE}
primary_name=$PRIMARY_NAME
debug_name=$DEBUG_NAME
common_bundle=$common_bundle_root
deb_root=$deb_root
appdir=$appdir
artifacts_dir=$artifacts_dir
tarball=$tarball_path
sha256sums=$checksum_path
deb_requested=$BUILD_DEB
deb_artifact=$deb_path
appimage_requested=$BUILD_APPIMAGE
appimage_artifact=$appimage_path
EOF
}

build_deb_artifact() {
    local deb_root=$1
    local artifacts_dir=$2
    local package_version_value=$3
    local deb_arch=$4
    local output_path="$artifacts_dir/${PACKAGE_NAME}_${package_version_value}_${deb_arch}.deb"

    require_command dpkg-deb
    dpkg-deb --build "$deb_root" "$output_path" >/dev/null
    echo "$output_path"
}

build_appimage_artifact() {
    local appdir=$1
    local artifacts_dir=$2
    local package_version_value=$3
    local appimage_arch=$4
    local output_path="$artifacts_dir/${PACKAGE_NAME}-${package_version_value}-${appimage_arch}.AppImage"

    require_command appimagetool
    # Without --runtime-file appimagetool downloads the runtime from the
    # moving `continuous` tag of AppImage/type2-runtime with no integrity
    # check; the release workflow passes a pinned, SHA256-verified file.
    local runtime_args=()
    if [ -n "$APPIMAGE_RUNTIME_FILE" ]; then
        [ -f "$APPIMAGE_RUNTIME_FILE" ] || fail "D4LC_APPIMAGE_RUNTIME_FILE does not exist: $APPIMAGE_RUNTIME_FILE"
        runtime_args=(--runtime-file "$APPIMAGE_RUNTIME_FILE")
    fi
    # CI containers expose no /dev/fuse; APPIMAGE_EXTRACT_AND_RUN makes the
    # appimagetool AppImage unpack itself instead of mounting through FUSE.
    ARCH="$appimage_arch" APPIMAGE_EXTRACT_AND_RUN=1 appimagetool "${runtime_args[@]}" "$appdir" "$output_path" >/dev/null
    echo "$output_path"
}

parse_args() {
    while [ $# -gt 0 ]; do
        case "$1" in
            --output-dir)
                [ $# -ge 2 ] || fail "--output-dir requires a value"
                OUTPUT_DIR=$2
                shift 2
                ;;
            --version)
                [ $# -ge 2 ] || fail "--version requires a value"
                VERSION=$2
                shift 2
                ;;
            --binary)
                [ $# -ge 2 ] || fail "--binary requires a value"
                BINARY_PATH=$2
                shift 2
                ;;
            --debug-binary)
                [ $# -ge 2 ] || fail "--debug-binary requires a value"
                DEBUG_BINARY_PATH=$2
                shift 2
                ;;
            --primary-name)
                [ $# -ge 2 ] || fail "--primary-name requires a value"
                PRIMARY_NAME=$2
                shift 2
                ;;
            --debug-name)
                [ $# -ge 2 ] || fail "--debug-name requires a value"
                DEBUG_NAME=$2
                shift 2
                ;;
            --skip-build)
                BUILD_BINARY=0
                shift
                ;;
            --tessdata-dir)
                [ $# -ge 2 ] || fail "--tessdata-dir requires a value"
                TESSDATA_DIR=$2
                shift 2
                ;;
            --doc-file)
                [ $# -ge 2 ] || fail "--doc-file requires a value"
                [ -f "$2" ] || fail "--doc-file target does not exist: $2"
                DOC_FILES+=("$2")
                shift 2
                ;;
            --appimage)
                BUILD_APPIMAGE=1
                shift
                ;;
            --deb)
                BUILD_DEB=1
                shift
                ;;
            --help)
                usage
                exit 0
                ;;
            *)
                fail "unknown argument: $1"
                ;;
        esac
    done
}

main() {
    parse_args "$@"

    require_file "assets/original"
    require_file "assets/d4lc"
    require_file "assets/provenance/asset-manifest.json"
    require_file "licenses/license-provenance.json"
    require_file "licenses/upstream-file-manifest.json"
    require_file "licenses/Inconsolata-OFL.txt"
    require_file "licenses/bundled-fonts.json"
    require_file "licenses/slint-LICENSE"
    require_file "licenses/tessdata-LICENSE"
    require_file "licenses/tessdata-provenance.md"
    require_file "LICENSE"
    require_file "upstream.lock"
    require_file "upstream/Diablo4Companion/LICENSE"
    require_file "packaging/LICENSE-PROVENANCE-NOTICE.md"
    require_file "packaging/linux-packaging.md"
    require_file "$DESKTOP_FILE"
    require_file "$APP_ICON_PNG"

    VERSION=${VERSION:-$(package_version)}
    [ -n "$VERSION" ] || fail "could not resolve package version"
    validate_package_version "$VERSION"
    validate_name "$PRIMARY_NAME"
    validate_name "$DEBUG_NAME"

    if [ -n "$TESSDATA_DIR" ]; then
        require_file "$TESSDATA_DIR"
        find "$TESSDATA_DIR" -maxdepth 1 -name '*.traineddata' | grep -q . \
            || fail "no .traineddata files found under --tessdata-dir=$TESSDATA_DIR"
    fi

    if [ "$BUILD_BINARY" -eq 1 ]; then
        require_command cargo
        cargo build --release --locked -p "$APP_PACKAGE" --features "$APP_FEATURES"
        cargo build --release --locked -p "$CLI_PACKAGE" --bin "$CLI_BINARY" --features "$CLI_FEATURES"
        DEBUG_BINARY_PATH=${DEBUG_BINARY_PATH:-$DEFAULT_DEBUG_BINARY}
    fi

    require_file "$BINARY_PATH"

    local deb_arch
    local appimage_arch
    deb_arch=$(map_deb_arch)
    appimage_arch=$(map_appimage_arch)

    local work_root="$OUTPUT_DIR/${PACKAGE_NAME}-${VERSION}"
    local common_root="$work_root/common"
    local common_bundle_root="$common_root/$PACKAGE_NAME"
    local deb_root="$work_root/deb-root"
    local appdir="$work_root/AppDir"
    local artifacts_dir="$work_root/artifacts"
    local tarball_path
    local checksum_path="$artifacts_dir/$CHECKSUM_FILENAME"
    local summary_path="$artifacts_dir/$SUMMARY_FILENAME"
    local deb_path="SKIPPED"
    local appimage_path="SKIPPED"
    local checksum_inputs=()

    mkdir -p "$common_root" "$artifacts_dir"
    prepare_common_bundle "$common_bundle_root" "$VERSION"
    stage_deb_root "$deb_root" "$common_bundle_root" "$VERSION" "$deb_arch"
    stage_appdir "$appdir" "$common_bundle_root"
    tarball_path=$(create_tarball "$common_root" "$artifacts_dir" "$VERSION" "$appimage_arch")
    checksum_inputs+=("$tarball_path")

    if [ "$BUILD_DEB" -eq 1 ]; then
        deb_path=$(build_deb_artifact "$deb_root" "$artifacts_dir" "$VERSION" "$deb_arch")
        checksum_inputs+=("$deb_path")
    fi

    if [ "$BUILD_APPIMAGE" -eq 1 ]; then
        appimage_path=$(build_appimage_artifact "$appdir" "$artifacts_dir" "$VERSION" "$appimage_arch")
        checksum_inputs+=("$appimage_path")
    fi

    write_artifact_checksums "$checksum_path" "${checksum_inputs[@]}"
    write_package_summary \
        "$summary_path" \
        "$VERSION" \
        "$common_bundle_root" \
        "$deb_root" \
        "$appdir" \
        "$artifacts_dir" \
        "$tarball_path" \
        "$checksum_path" \
        "$deb_path" \
        "$appimage_path"
    cat "$summary_path"
}

main "$@"
