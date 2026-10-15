# UI translations

The UI language follows upstream: the strings come from the WPF app's
resource files (`D4Companion.Localization/Resources*.resx`). The port adds no
translations of its own, with one exception: Simplified Chinese (see below).

## How it works

- Slint text is marked `@tr("...")`. The msgid is the upstream English value
  of the resource key the control binds in XAML (`{loc:LocExtension rsCap...}`),
  so a control that corresponds to an upstream control carries upstream's
  exact English text.
- `cargo xtask import-upstream-locales` builds
  `<culture>/LC_MESSAGES/d4lc-app.po` from the resx files, limited to the keys
  listed in `upstream-keys.txt`. Slint bundles the catalogues at build time
  (`build.rs`), and `src/i18n.rs` reads the same files for strings Rust builds
  itself (`i18n::tr`). Each catalogue header records the resx it came from, its
  SHA-256 and the upstream commit.
- `cargo xtask import-upstream-locales --check` fails when a generated file is
  stale. The xtask tests also check that every key in `upstream-keys.txt` is
  bound somewhere in upstream's `.xaml` or `.cs`, and that every `@tr` string
  is either upstream-owned or listed in `port-only.txt`. The check that every
  `i18n::tr` lookup in Rust is an upstream string lives in the `d4lc-app` tests
  (`src/i18n.rs`).
- To localize another control, bind it to its upstream key: use the upstream
  English value as the msgid, add the key to `upstream-keys.txt` and
  re-run the importer.

## Languages

The settings selector offers the cultures upstream offers
(`SettingsViewModel.cs:399-415`): `de-DE`, `en-US`, `es-ES`, `fr-FR` and
`zh-CN`. Upstream lists the remaining cultures of its table commented out and
ships no resources for them. The choice is persisted as `SelectedAppLanguage`
and is separate from the game-data language (`SelectedAffixLanguage`). A stored
culture the selector does not offer follows the desktop locale where a resource
file applies (upstream leaves WPF on the OS UI culture) and is English
otherwise; the app logs a warning naming the stored value. Numbers keep the
en-US format in every language.

Upstream's satellite resources are incomplete. A key without a value in the
selected language renders in English, as upstream does.

| culture | rows in the catalogue | translated |
|---------|-----------------------|------------|
| de-DE   | 191                   | 70         |
| en-US   | 191                   | 191        |
| es-ES   | 191                   | 179        |
| fr-FR   | 191                   | 104        |
| zh-CN   | 515                   | 515        |

The importer prints the current counts.

## Simplified Chinese

zh-CN is translated in full. Upstream's zh-CN resources cover part of the
strings the port uses; the project owner translated the rest, in
`port/zh-CN.po` (hand-written, merged by the importer). Four title-bar labels
(`d4lc on GitHub` and the three `Diablo4Companion` link labels) were written
without the owner and are pending owner review, as are the eleven build
import strings of the unified URL row and the import progress panel (the
last block of `port/zh-CN.po`). The file covers:

- the upstream keys that have no zh-CN value, and
- every port-only string.

The generated zh-CN catalogue marks each such row `#. d4lc translation, not
from upstream`, so its provenance stays visible. The importer fails when
`port/zh-CN.po` shadows an upstream translation, misses a string, or keeps an
entry the UI no longer uses. Terms follow upstream's zh-CN resources and the
zh-CN game data under `assets/original/data` (词缀 affix, 威能 aspect, 符印
sigil, 符文 rune, 暗金 unique, 巅峰 paragon, 覆盖 overlay, 淬炼 tempered; "Build" stays
English, capitalised).

The app bundles no UI font, so Chinese text needs a system CJK font (Noto Sans
CJK or similar); without one Slint draws empty boxes. `cargo test -p d4lc-app
--test cjk_font -- --ignored` checks the machine.

## What stays English (all languages but zh-CN)

`port-only.txt` lists every `@tr` string without an upstream key (generated,
sorted). They are marked so a catalogue can pick them up, but only zh-CN
translates them. They fall into these groups:

- Text upstream hard-codes in English: most dialog titles, the "Update" /
  "Download" button caption, the "Done" button on the Local and Merge import
  tabs.
- Controls the port added: the display backend selector, diagnostics export,
  the update banner, descriptive hints, and the accessible names (screen-reader
  and test lookups) of controls.
- Strings built from data (`Remove {}`, `Add {}`), whose parts are not upstream
  resources.

A port-only string whose English equals an upstream value, but whose control
upstream does not bind to that key (the Cancel button of a config dialog, the
Debug status grid), carries the message context `d4lc`:
`@tr("d4lc" => "Cancel")`. Without it the lookup would hand it the upstream
translation of a different control. The importer refuses a plain `@tr` that
equals an upstream value without being in `upstream-keys.txt`.

Other text is not marked at all:

- State tokens that double as values (display backend names, marker mode,
  sigil display mode, import provider names): translating the label would
  change the stored value.
- Rust-side status lines, log text, validation errors and the overlay's own
  text. Upstream's overlay notifications (`rsFormatPresetActivated`,
  `rsCapOverlayEnabled`, ...) are not ported.
