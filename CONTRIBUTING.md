# Contributing

d4lc is a solo-maintained, Linux-first Rust port of
[josdemmers/Diablo4Companion](https://github.com/josdemmers/Diablo4Companion).
The rules below keep the codebase reviewable by one person.

## Ground rules

- **Rust-native only.** No CLR/.NET/WPF runtime bridge, and no spawned OCR
  binaries in the live hot path.
- **No invented behavior.** Features are ported with upstream parity; behavior
  claims need an upstream source reference (see the
  [feature/parity proposal template](.github/ISSUE_TEMPLATE/feature_parity_proposal.md)).
- **Linux/X11 first.** Wayland support is experimental.

Build prerequisites and commands are in the [README](README.md).

## Workflow

Work is change-driven: one scoped change at a time, from problem to validated
result.

1. Open or pick an issue. Bugs use the
   [bug report template](.github/ISSUE_TEMPLATE/bug_report.md); behavior that
   exists upstream uses the feature/parity proposal template.
2. Keep the change atomic: one logical change, no drive-by edits.
3. Run the validation gate before the change counts as done.

## Validation

Every change passes the local gate before it is committed or proposed:

```bash
./scripts/verify_local.sh --fast   # inner loop: fmt, clippy, tests, parity
./scripts/verify_local.sh           # full gate: adds the desktop pilot suite,
                                    # the X11 visual smoke and the real-browser
                                    # tests — run exclusively, not alongside
                                    # other builds or tests
```

The gate runs the dependency-direction check, the release-script and
release-workflow checks, `cargo fmt`,
`cargo clippy --workspace --all-targets -D warnings` and `cargo test --workspace`
(default and release-feature configurations), and the upstream asset parity
check; the full tier adds the pilots, the visual smoke and
`scripts/run_browser_integration.sh`. CI runs the full tier plus a build with
the minimum Rust version and `cargo deny check`, which you can run locally as
well.

Tests never skip silently. A test that needs a window runs on Slint's headless
testing backend; a test that needs an X server starts its own Xvfb and fails
when Xvfb is not installed. Tests that need something the gate cannot provide
(a live provider site, a running game) are `#[ignore]`d with the reason and
are reported as not run.

## Releases

`.github/workflows/release.yml` builds a release only from a commit that passed
the full verification, and publishes only to an existing tag that points to
that commit and names the crate version (`v<version>`). It creates or
refreshes a draft and never modifies a published release
(`scripts/publish_draft_release.sh`). Versions such as `1.0.0-rc.1` are
marked **Pre-release**, never **Latest**. After reviewing the checks and notes,
the maintainer can publish the RC draft for public testing without claiming
complete live-game validation. Record untested classes and features, trading,
runes and platforms in the release notes; do not remove those caveats just
because CI is green.

Before a stable release, check the packaged build in a live X11 game session
using `docs/manual-validation-diablo-iv.md` and record the result in the release
description. Check live provider imports with
`scripts/run_browser_integration.sh` and the `D4LC_*_SMOKE_*` variables
documented there.

Capture/OCR/overlay changes should also attach the `debug-output/` evidence
they produce to the issue. Crop captures to the tooltip and remove personal
data (BattleTag, character names, other windows, user names in paths) first.

## Commit style

- One atomic commit per change.
- Subject: short imperative summary in plain English. No ticket IDs, no
  references to tooling or AI assistance.
- Body (optional): the why in two or three sentences when the diff does not
  explain itself.

## Questions

Shared concepts (system presets, affix presets, build imports) are covered by
the [upstream wiki](https://github.com/josdemmers/Diablo4Companion/wiki) and
the README. Anything else: open an issue.
