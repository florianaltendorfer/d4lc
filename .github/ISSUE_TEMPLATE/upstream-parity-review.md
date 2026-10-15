---
name: Upstream parity review
about: Review Diablo4Companion upstream changes and list scoped d4lc parity follow-ups
title: "Upstream parity review: <upstream commit or release>"
labels: ["upstream", "parity-review"]
assignees: []
---

## Upstream reference

- Upstream URL: https://github.com/josdemmers/Diablo4Companion
- Upstream branch/release:
- Previous reviewed commit from `upstream.lock`:
- New upstream commit:
- Change source: release / branch head / manual inspection

## Required local commands

```bash
./scripts/check_upstream.sh
cargo run -q -p d4lc-cli -- upstream-manifest --write licenses/upstream-file-manifest.json
./scripts/inspect_upstream.sh
```

## Affected upstream areas

Mark every area touched by the upstream diff. Use `config/upstream-files.toml` for path-level mapping.

- [ ] Settings: `D4Companion.Entities/SettingsD4.cs`, `D4Companion.Services/SettingsManager.cs`
- [ ] System presets/controller images: `D4Companion.Services/SystemPresetManager.cs`, `downloads/systempresets/*`
- [ ] Screen capture: `D4Companion.Services/ScreenCaptureHandler.cs`, `D4Companion.Helpers/ScreenCapture.cs`
- [ ] Tooltip processing: `D4Companion.Services/ScreenProcessHandler.cs`
- [ ] OCR/conversion: `D4Companion.Services/OcrHandler.cs`
- [ ] Affix/aspect/unique/rune/sigil data: `D4Companion/Data/*.json`
- [ ] Overlay/paragon: `D4Companion.Services/OverlayHandler.cs`, `ParagonBoards.*.json`, `ParagonGlyphs.*.json`
- [ ] Trade: `D4Companion.Services/TradeItemManager.cs`, `Config/TradeItems.json`
- [ ] Build imports: `BuildsManagerD4Builds.cs`, `BuildsManagerMaxroll.cs`, `BuildsManagerMobalytics.cs`, `BuildsManagerD2Core.cs`
- [ ] UI/localization: `D4Companion/ViewModels/*`, `D4Companion/Views/*`, localization resources
- [ ] Release notes / README / wiki docs

## Manifest/provenance review

- [ ] `licenses/upstream-file-manifest.json` was regenerated.
- [ ] `licenses/license-provenance.json` still records every copied/reused file, if any.
- [ ] New upstream assets/data were not copied without source path, upstream commit, SHA256, and MIT attribution.
- [ ] Missing upstream paths are listed as unresolved/uncertain instead of guessed.

## Behavior claims

Record every original-behavior claim with a path-level upstream reference. Use `UNKNOWN` only when the file could not be inspected.

| Claim | Upstream path/method | d4lc code/test | Parity status |
|---|---|---|---|
|  |  |  | exact / equivalent / partial / unknown |

## Follow-up work

| Scope | Upstream reference | Notes |
|---|---|---|
|  |  |  |

## Validation

```text
./scripts/verify_local.sh
```

Result:

## Notes / uncertainties

- 
