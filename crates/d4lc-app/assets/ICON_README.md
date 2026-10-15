# Application Icon

The d4lc application icon is a real bundled asset: `assets/d4lc.png`
(256x256 RGBA PNG with transparency). It is referenced by the `.desktop`
file and the tray icon.

## Bundled asset

- **File:** `assets/d4lc.png` — 256x256, 8-bit RGBA PNG, alpha preserved.
- **Source:** `upstream/Diablo4Companion/D4Companion/Images/app.ico`
  (MIT licensed; upstream commit
  `d508a473515fb6239a93a7a1a146b7ad007f01b1`, release v5.4.0.0). The
  `.ico` carries exactly one frame: 128x128, 32 bits per pixel.
- **Derivation:** the single 128x128 RGBA frame was extracted and
  Lanczos-upscaled to 256x256 with Python PIL:

  ```bash
  python3 - <<'EOF'
  from PIL import Image
  img = Image.open("upstream/Diablo4Companion/D4Companion/Images/app.ico")
  img.size = max(img.ico.sizes())          # (128, 128) — the only frame
  frame = img.convert("RGBA")
  frame.resize((256, 256), Image.LANCZOS).save(
      "crates/d4lc-app/assets/d4lc.png", "PNG", optimize=True)
  EOF
  ```

- **sha256:** `7feef008a29ae576289cd21021b5a440449f284a72f0a45d92309390f030065b`
- **Provenance:** recorded as a *derived* asset (source + derivation +
  sha256) in `assets/provenance/asset-manifest.json` and
  `licenses/license-provenance.json` under `derived_files`. It is
  intentionally **not** listed in `items`/`copied_files`: those sections are
  byte-copy parity gates (`cargo run -p d4lc-cli -- upstream-asset-parity`)
  that require the destination sha256 to equal the upstream source sha256,
  which a converted PNG cannot satisfy. Note that
  `scripts/update_asset_manifest.py` and `d4lc-cli upstream-provenance
  --write` regenerate only the copied-asset sections; if they are re-run,
  re-add the `derived_files` entry.

## Usage

- **Desktop entry** (`assets/d4lc.desktop`): references `Icon=d4lc`, the
  freedesktop icon-theme name. The PNG is resolved through the hicolor icon
  theme — install it as
  `~/.local/share/icons/hicolor/256x256/apps/d4lc.png` (user) or
  `/usr/share/icons/hicolor/256x256/apps/d4lc.png` (system). The release
  packaging (`scripts/package_linux_release.sh`) performs this install for
  the `.deb` and stages the icon inside the AppImage and the tarball.
- **Tray icon**: the `d4lc-app` tray feature (gated behind
  `--features tray-icon`) publishes the StatusNotifierItem icon by name
  (`d4lc`), which resolves through the same icon-theme lookup once the PNG
  is installed in the hicolor theme.

## Status

**Bundled.** The icon is derived from the MIT-licensed upstream application
icon with recorded provenance. No placeholder remains.
