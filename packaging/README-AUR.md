# AUR packaging (Arch / CachyOS / paru)

[`arch/PKGBUILD`](arch/PKGBUILD) builds `d4lc` from the GitHub release tag and
installs the repository's own staged layout:

- `/opt/d4lc/` — relocatable bundle (binaries, bundled OCR libraries, assets, docs)
- `/usr/bin/d4lc` and `/usr/bin/d4lc-debug` — wrappers (set `LD_LIBRARY_PATH`
  to the bundled `lib/` and `TESSDATA_PREFIX` when present)
- `/usr/share/applications/d4lc.desktop` + hicolor icon — KDE/GNOME menu entry

The recipe targets RC tag `v1.0.0-rc.1` (Arch version `1.0.0rc1`). It requires
the matching public source tag and is not on the AUR. This is a source-build
recipe, not a promise that every feature has been live-tested.

## Publish to the AUR (owner)

First publish the intended source tag, verify that its archive is publicly
downloadable, and update both `pkgver` (Arch syntax) and `_tag` (SemVer). Do not publish the recipe with
`SKIP`, and do not reuse an old archive's checksum for a new version. The pinned
upstream license checksum is included in the recipe.

`updpkgsums` comes from Arch's `pacman-contrib` package. Generate `.SRCINFO`
**after** updating the version and checksums, then build and test before pushing.

```bash
git clone ssh://aur@aur.archlinux.org/d4lc.git /tmp/aur-d4lc   # creates the package base
cp packaging/arch/PKGBUILD /tmp/aur-d4lc/
cd /tmp/aur-d4lc
# Update pkgver/_tag/pkgrel in PKGBUILD to the intended release first.
updpkgsums
! grep -q "'SKIP'" PKGBUILD
makepkg --printsrcinfo > .SRCINFO
makepkg --cleanbuild --syncdeps
# Review and install/test the built package before committing and pushing.
git add PKGBUILD .SRCINFO
git commit -m "Update d4lc release package"
git push
```

After that, users install with `paru -S d4lc` (or `yay -S d4lc`).

## Local test before publishing

This needs the published tag as well.

```bash
cd packaging/arch
makepkg -f          # full build + package into ./d4lc-*.pkg.tar.zst
pacman -U d4lc-*.pkg.tar.zst
```

## Notes

- The bundle ships its own `libtesseract`/`libleptonica` chain, so the
  package does not depend on the host's tesseract/leptonica versions. It
  carries no tessdata: `tesseract-data-eng` is a required dependency and
  other languages are optional `tesseract-data-*` packages.
- `sha256sums` must be real values for AUR publication. `SKIP` is a release
  blocker, not a verified checksum.
- Building from source needs Tesseract and Leptonica headers/libraries even
  though the resulting bundle carries its own OCR library chain.
- The package intentionally does not `strip` the binaries so panic-hook
  crash logs stay symbolizable.
- Chromium (or Google Chrome) is an optional dependency: only the D2Core,
  D4Builds and Mobalytics build imports drive a headless browser.
  `D4LC_CHROME_BIN` selects a specific browser binary.
