# Manual validation in a live Diablo IV session (VAL-01)

This is the acceptance procedure for the one check CI cannot run: the release
artifact against a running Diablo IV session on X11. It is a **stable-release
blocker** until recorded for the exact build being published. A release
candidate may be published for public testing with the missing checks listed
explicitly in its release notes; an untested check is not a pass.

The procedure has two parts:

- **Automated diagnostics** (section 3): the packaged debug CLI writes
  machine-checkable artifacts. Everything except the game-dependent capture
  steps can be rehearsed without Diablo IV (section 3.0).
- **Interactive checks** (section 4): the desktop app, the overlay and the
  hotkeys in the running game. Only a person at the game can pass these.

Both parts use the **packaged release artifact**, never a development build,
and run as your normal user. Nothing needs root except installing the `.deb`.

Nothing here is marked passed automatically. A step counts only when its
evidence was produced in the session being recorded.

## 0. Preconditions

- An X11 session (`XDG_SESSION_TYPE=x11`, `DISPLAY` set). Wayland/XWayland is
  out of scope for this gate.
- Diablo IV running, windowed or fullscreen-windowed, on the resolution of the
  preset you will select. Have the character in a state where you can open item
  tooltips (inventory, vendor, stash).
- **No other d4lc instance is running** (`pgrep -a d4lc` prints nothing). d4lc
  allows one instance per session; a second start is refused, which is part of
  what this run checks. Do not override `XDG_RUNTIME_DIR` or other session
  variables: that would create a separate single-instance scope and cut the
  app off from the session bus (tray, portals).
- The release artifacts (tarball, `.deb`, AppImage) and their `SHA256SUMS` and
  `package-summary.txt` for the build under test. The `tessdata` packs ship
  inside the bundle; nothing to install for OCR.

Record once at the start. `D4LC_CONFIG_ROOT` gives this run its own settings, so
the test never touches your real `~/.config/d4lc`; the app and the CLI both read
it. `D4LC_RUN` is a writable working directory: every CLI output lands below it.

```bash
export D4LC_ARTIFACT_DIR=/path/to/the/downloaded/artifacts
export D4LC_VERSION=<version>              # e.g. 1.0.0-rc.1
export D4LC_RUN=$(mktemp -d "$HOME/d4lc-val01-XXXX")
export D4LC_CONFIG_ROOT="$D4LC_RUN/config"
mkdir -p "$D4LC_CONFIG_ROOT"
cd "$D4LC_RUN"
```

## 1. Artifact identity

```bash
(cd "$D4LC_ARTIFACT_DIR" && sha256sum -c SHA256SUMS)   # every listed file: OK
grep -E '^(version|git_revision)=' "$D4LC_ARTIFACT_DIR/package-summary.txt"
```

- `git_revision` must equal the commit the release was built from (the
  `Build commit:` line in the release notes). A value ending in `-dirty` is a
  local build of an uncommitted tree and never a release candidate.
- `version` must equal the tag's version (`v<version>`).

## 2. Choose a package format and set the launchers

Pick one format per run; repeat the run for each format you want to accept.
In all three the CLI reads its game data from the installed bundle (the launcher
sets `D4LC_ASSET_ROOT`) and writes to the current directory, `$D4LC_RUN`.

**Tarball** (no root needed; the bundle may stay read-only):

```bash
tar -C "$D4LC_RUN" -xzf "$D4LC_ARTIFACT_DIR/d4lc-$D4LC_VERSION-linux-x86_64.tar.gz"
export D4LC_APP="$D4LC_RUN/d4lc/bin/d4lc"
export D4LC_DEBUG="$D4LC_RUN/d4lc/bin/d4lc-debug"
```

**`.deb`** (installs to `/opt/d4lc` and `/usr/bin`; this changes the system):

```bash
sudo apt-get install "$D4LC_ARTIFACT_DIR/d4lc_${D4LC_VERSION}_amd64.deb"
export D4LC_APP=/usr/bin/d4lc
export D4LC_DEBUG=/usr/bin/d4lc-debug
```

**AppImage** (the debug CLI is the same file with `--debug-cli` first):

```bash
export D4LC_APPIMAGE="$D4LC_ARTIFACT_DIR/d4lc-$D4LC_VERSION-x86_64.AppImage"
chmod +x "$D4LC_APPIMAGE"
d4lc_app() { "$D4LC_APPIMAGE" "$@"; }
d4lc_debug() { "$D4LC_APPIMAGE" --debug-cli "$@"; }
export -f d4lc_app d4lc_debug 2>/dev/null || true
export D4LC_APP=d4lc_app D4LC_DEBUG=d4lc_debug
```

Without FUSE, also `export APPIMAGE_EXTRACT_AND_RUN=1`. Run the later commands
in this same bash shell, where the two functions are defined.

Smoke and identity checks (no game needed):

```bash
"$D4LC_DEBUG" --version                # the version under test
"$D4LC_DEBUG" asset-presets            # bundled system presets, read from the bundle
"$D4LC_DEBUG" upstream-changed --remote-head <commit>   # optional: reads the bundled upstream.lock
"$D4LC_DEBUG" settings-report          # config_root is $D4LC_CONFIG_ROOT
```

`settings-report` must name `$D4LC_CONFIG_ROOT` as `config.config_root` with
`config_root_source` `D4LC_CONFIG_ROOT`. Before the app has saved anything,
`config.settings.status` is `missing` and `effect` is `defaults`. That is
correct; the report never claims `loaded` without a file.

`upstream-report`, `upstream-manifest`, `upstream-provenance`,
`upstream-asset-parity` and `perf-detect-offline` without `--input` need a source
checkout and fail in a package with "requires a developer checkout"; they are
not part of this procedure.

Start the desktop app. It uses the same `D4LC_CONFIG_ROOT`:

```bash
"$D4LC_APP" &
```

In the app, select the system preset that matches your game resolution and the
game language, and save. Then confirm the CLI sees the app's settings:

```bash
"$D4LC_DEBUG" settings-report
```

Pass when `config.settings.status` is `loaded` and the preset/language are the
ones you just selected in the app.

## 3. Automated diagnostics (packaged debug CLI)

Run these in a second terminal (with the variables from sections 0 and 2) while
the game is in the foreground. Add `--delay-seconds 3` to commands that must
run after you switch focus back to the game. All outputs land below
`$D4LC_RUN/debug-output/`; nothing is written into the installed bundle.

### 3.0 Rehearsal without the game

```bash
"$D4LC_DEBUG" manual-diablo-validation-bundle --dry-run
```

Pass when it exits 0, prints `"dry_run": true` and a `bundle_dir` below
`$D4LC_RUN/debug-output/manual-diablo-iv/`, and that directory holds
`bundle-summary.json` and `settings-snapshot.json`. The snapshot's `status`
(`loaded`, `missing`, `invalid` or `unreadable`) and `config` block show which
settings file was used and which values came from command-line flags
(`config.cli_overrides`). An `invalid` or `unreadable` settings file stops the
command with exit 2; it is never replaced by defaults.

### 3.1 Window discovery (VAL-01 #1)

```bash
"$D4LC_DEBUG" x11-windows --match "Diablo IV"
```

Pass when `selected` is the game window and `selected.matches_query` is
`true`. Repeat with `--class` and `--pid` if you want the stricter query.

### 3.2 Bounded live session bundle (VAL-01 #2, #3, #5, #6)

Open an item tooltip and keep it open for the duration of the run. The preset
and language come from the app's settings (section 2); pass `--preset` or
`--language` only to deliberately override them, and the snapshot then lists
them under `cli_overrides`.

```bash
"$D4LC_DEBUG" manual-diablo-validation-bundle \
    --match "Diablo IV" \
    --delay-seconds 3 \
    --frames 40
```

This writes `debug-output/manual-diablo-iv/run-<timestamp>/` below `$D4LC_RUN`
(or the absolute directory given with `--output-dir`) with:
`bundle-summary.json`, `selected-window.json`, `settings-snapshot.json`,
`live-session.jsonl`, `live-session-summary.json`, `final-frame.json`,
`overlay-frame.json`, `ocr-output.json`, `parsed-item.json`, `rule-result.json`.

Pass when:

- `bundle-summary.json` has `"status": "captured"`, `"final_detection_found": true`
  and `"ocr_status": "ok"` (`captured_without_tooltip` means no tooltip was
  detected; `partial` means OCR did not complete; both fail this step);
- `settings-snapshot.json` has `"status": "loaded"` and names `$D4LC_CONFIG_ROOT`;
- `ocr-output.json` holds the tooltip text you see on screen;
- `parsed-item.json` carries the parsed item type, item power and at least one
  affix row that matches the tooltip.

`rule-result.json` is `unavailable` by design: the CLI has no app-owned
rule/overlay state. Affix/rule evaluation and marker positions are checked in
the app (section 4).

### 3.3 Three item types (VAL-01 #4, #6)

Repeat 3.2 for **three different item types** (for example a legendary ring
with greater affixes, a rare/legendary armour piece, and a unique or a
nightmare sigil). Each run produces its own `run-<timestamp>/` directory; keep
all three as evidence.

### 3.4 Live loop with the overlay (VAL-01 #2, #3, #7)

Quit the desktop app first (one overlay at a time), then:

```bash
"$D4LC_DEBUG" live-loop --match "Diablo IV" --overlay --ocr-worker \
    --delay-seconds 3 --frames 200
```

Pass when the loop starts, its session start line names the expected config
root and preset, it reports capture and detection ticks, draws the overlay, and
stops cleanly at the end or on `Ctrl-C`. Run it twice in a row to confirm start
and stop.

## 4. Interactive app checks

Start the app again (`"$D4LC_APP" &`) and record the outcome of each line
(VAL-01 #6 to #12). The hotkeys are disabled by default as in the original;
enable the ones you test under Settings -> Hotkeys first.

| # | Check | Pass condition |
|---|---|---|
| 6 | Affix/rule evaluation | With the overlay on, hovering an item marks exactly the affixes the preset wants: correct affix rows, greater/tempered shape and colour, "hide unwanted" respected. Compare against the item text on screen. |
| 7 | Overlay marker positions | Markers sit on the matching affix lines of the tooltip, not offset by a line or by the window frame. Check on two different item types. |
| 8 | Preset switch during use | Turn the overlay off, switch the system preset (Settings -> System presets) and the affix preset, turn it back on; the markers follow the new preset. The app must refuse the switch while the overlay is on. |
| 9 | Hotkeys | `Ctrl+F12` toggles the overlay, `Ctrl+F5` switches the preset, `Ctrl+F6` switches the overlay mode, `Ctrl+F10` takes a debug screenshot. Confirm each has the documented effect and appears in the logs. |
| 10 | Window lifecycle | Alt-tab away and back, minimise and restore the game, then close the game: the app must stop marking on focus/minimise loss, resume on return, and report the window loss instead of hanging or crashing. |
| 11 | Restart with existing settings | Quit the app and start it again with the same `D4LC_CONFIG_ROOT`. The preset, language, overlay state and hotkey bindings are restored from `Config/Settings.json`. |
| 12 | Stability / performance | Run the overlay through a 10-15 minute session with normal item checking. No crash, no freeze, no runaway memory, and no growing marker lag. Note the app's reported hotpath/tick timings from the Debug screen. |
| 13 | Single instance | While the app runs, start `"$D4LC_APP"` a second time: it must not open a second window or overlay. |
| 14 | Debug export while the overlay runs | With the overlay on, press "Export debug images" on the Debug screen: the UI stays responsive and the export reports its result. |

Also verify the bundled tessdata is used: with the default environment the
launcher sets `TESSDATA_PREFIX` to the bundle's `tessdata/`, and OCR must
produce the same text as in the CI recorded-frame tests.

If the app reports an interrupted configuration change at startup, it names the
exact `d4lc-debug config-recover --complete|--rollback --record <id>` command.
Record that as a finding; the run itself never needs it.

## 5. Evidence and redaction

Collect:

- the `SHA256SUMS` check output and `package-summary.txt`;
- the `settings-report` output from section 2;
- the three `manual-diablo-iv/run-*/` directories (at least
  `bundle-summary.json`, `settings-snapshot.json`, `ocr-output.json` and
  `parsed-item.json`);
- one or two redacted screenshots of the overlay with markers;
- the app's logs from the session.

Redact before attaching anything: the tooltip captures and logs can show your
BattleTag, character names, other windows and paths with your user name.
The CLI bundle already redacts the window title and class.

## 6. Result

Record, per the table above: commit SHA, artifact name and format, date,
session details (distribution, resolution, game language, preset and character
class), pass/fail/not-tested per row, and any deviation. Also record whether
trading, rune recognition and paragon were tested; this checklist alone does
not establish coverage of every class or feature. A failure blocks a stable
release. For an RC, document missing checks and known failures in the release
notes; never present them as passed.

Clean up afterwards: `rm -rf "$D4LC_RUN"`, and for the `.deb`
`sudo apt-get remove d4lc` if it was installed only for this run.
