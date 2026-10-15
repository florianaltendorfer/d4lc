# Bundled tessdata provenance

The release bundles (tarball, `.deb`, AppImage) ship the same 13 Tesseract
language packs the upstream companion carries in
`D4Companion/tessdata/`. Those files are **not** Diablo4Companion assets:
the upstream repository vendors them from the Tesseract project, and this
port's packaging copies them from the pinned upstream submodule, which in
turn carries the `tessdata_fast` variants.

- Source repository: <https://github.com/tesseract-ocr/tessdata_fast>
- Source tree the bundled blobs match:
  `87416418657359cb625c412a48b6e1d6d41c29bd` (`main` at the time of
  review). Every bundled `*.traineddata` file's Git blob id equals the one
  recorded in that tree.
- License: Apache License 2.0 — full text in [`tessdata-LICENSE`](tessdata-LICENSE),
  taken verbatim from the source tree above.
- Bundled languages: `chi_sim`, `chi_tra`, `deu`, `eng`, `fra`, `ita`,
  `jpn`, `kor`, `pol`, `por`, `rus`, `spa`, `tur`.

A user-set `TESSDATA_PREFIX` overrides the bundled packs; bundles packaged
without `--tessdata-dir` use the host's system tessdata instead.

## Verification

To re-verify that the files copied from the pinned submodule are the
`tessdata_fast` blobs (and were not modified anywhere in the chain):

```sh
cd upstream/Diablo4Companion
for f in D4Companion/tessdata/*.traineddata; do
    echo "$(git rev-parse "HEAD:$f") $(basename "$f")"
done
# Compare each blob id against the tessdata_fast tree:
curl -fsSL https://api.github.com/repos/tesseract-ocr/tessdata_fast/git/trees/main
```
