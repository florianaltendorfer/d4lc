# d4lc supplement assets

Files under `assets/d4lc/` are authored by d4lc, not copied from upstream
Diablo4Companion. They fill gaps in the upstream system presets and follow
upstream file-name conventions, so upstream code paths classify them. An
upstream preset file with the same name always takes precedence.

## `systempresets/1080p_SMF/dot-affixes_greater_tempered.png`

- Purpose: affix marker of a tempered affix that became a greater affix.
  Upstream evaluates such an affix as Greater ("Since season 11 a tempered
  affix can become a greater affix", `AffixManager.cs:547-549`), but its
  1080p_SMF preset (also the latest upstream `1080p_SMF.zip`) has no matching
  template. The in-game star carries an orange rim, and after the upstream
  binarization (`ThresholdBinaryInv` 70/255) `dot-affixes_greater.png`
  scores 0.092 SQDIFF_NORMED against it, above the 0.05
  `ThresholdSimilarityAffixLocation` default. Without a marker, the affix
  merges into the previous affix area and gets no overlay dot.
- Classification: the `dot-affixes_greater` prefix makes it a Greater affix
  marker (`ScreenProcessHandler.cs:961`).
- Source: a 16x16 crop of a native 1920x1080 Diablo IV frame (2026-10-02,
  "+12.0% Attack Speed" on a tempered greater roll), RGB, metadata stripped.
- Check against that frame: 0.000 on the tempered greater star, 0.092 on a
  plain greater star, and no other marker below 0.09.
- SHA256: `2207b91f5b5689513a4f163ae3c10eecbc479a6de4c6528f03baaa45f945c39a`

Other presets do not have a supplement yet. Each one needs a crop from a
native frame at that preset's resolution and font size.
