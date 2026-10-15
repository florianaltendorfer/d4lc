#!/usr/bin/env bash
# Stage real bundles with a stand-in payload; no build, installation or publishing.
set -euo pipefail
cd "$(dirname "$0")/.."

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fail() { echo "test_package_linux_release: $*" >&2; exit 1; }

(
    source packaging/arch/PKGBUILD
    crate_version=$(awk -F'"' '/^version = / { print $2; exit }' crates/d4lc-cli/Cargo.toml)
    [ "$_tag" = "$crate_version" ] || fail "Arch source tag differs from the release version"
    [[ "$pkgver" != *-* ]] || fail "Arch pkgver cannot contain a hyphen"
    [[ "${source[0]}" == *"/v$_tag.tar.gz" ]] || fail "Arch source URL lost the SemVer tag"
)

for version in 1.0.0-rc.1 1.0.0 1.0.0-rc.1+build-test 1.0.0+build-test; do
    case "$version" in
        1.0.0-rc.1*) deb_version="${version/1.0.0-rc.1/1.0.0~rc.1}" ;;
        *) deb_version="$version" ;;
    esac
    ./scripts/package_linux_release.sh --skip-build --binary "$(type -P true)" \
        --version "$version" --output-dir "$work/packages" >"$work/summary.txt" 2>"$work/package.log" \
        || fail "staging $version failed: $(cat "$work/package.log")"
    staged="$work/packages/d4lc-$version"
    grep -Fxq "Version: $deb_version" "$staged/deb-root/DEBIAN/control" \
        || fail "$version: incorrect Debian prerelease ordering"
    grep -Fxq "version=$version" "$work/summary.txt" || fail "$version: summary lost SemVer identity"
    grep -Fxq "package_version=$version" "$staged/common/d4lc/docs/build-metadata.txt" \
        || fail "$version: build metadata lost SemVer identity"
    tarball=$(awk -F= '/^tarball=/{print $2}' "$work/summary.txt")
    ./scripts/verify_release_checksums.sh "$staged/artifacts/SHA256SUMS" "$tarball" >/dev/null
    [ -x "$staged/deb-root/opt/d4lc/bin/d4lc-real" ] || fail "$version: payload missing"
    "$staged/common/d4lc/bin/d4lc" || fail "$version: launcher failed"
    rm -rf "$staged"
done

echo "test_package_linux_release: ok"
