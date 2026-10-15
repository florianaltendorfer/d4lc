# Security policy

## Supported scope

Security fixes target the latest release on Linux/X11; older releases are not
patched. No release has been published yet, so until then the supported
build is a source build of the `main` branch. Wayland support is
experimental; reports that only reproduce on Wayland are accepted and handled
best-effort.

Context that frames what is and is not a vulnerability here: d4lc is a
read-only companion. It captures and reads tooltips, never sends input to the
game, and does not talk to game servers. Its network inputs are listed under
"Network and privacy" in the [README](README.md).

## What to report

- Memory-safety issues or crashes triggered by untrusted input: upstream data
  files, settings and presets, assets, or window content picked up by
  capture/OCR.
- Problems with downloaded content: system preset archives (path traversal,
  archive bombs, writes through symlinks, a failed download damaging the
  installed presets), build-import responses from provider sites, and the
  update check.
- The headless browser used for build imports running without Chromium's
  sandbox, with the user's normal browser profile, or against a host other
  than the chosen provider.
- File writes landing outside the directories the app owns: the config root
  (`~/.config/d4lc` or `D4LC_CONFIG_ROOT`, with `Config/`, `Images/`,
  `Builds/`, `Profiles/` and `logs/`), the crash-log directory
  (`~/.local/share/d4lc/crashes`), `debug-output/`, and paths you configured.
- The release artifacts, the release workflow or
  `scripts/package_linux_release.sh` doing something they should not
  (unexpected network access, arbitrary code execution, artifacts that were
  not built from the tagged commit).
- Any path that lets the app send input to the game. That must not exist.

## How to report

Do not open a public issue with details of a security problem.

Use GitHub private vulnerability reporting:

https://github.com/florianaltendorfer/d4lc/security/advisories/new

If that form is unavailable, open an issue titled "Security contact request"
that names no component, no version and no symptom, and attach nothing. The
maintainer replies on GitHub to arrange a private channel.

Include:

- The release or commit you run, and whether you used a release artifact or
  built from source.
- Your environment: distribution, X11 session details.
- What you observed, and what an attacker could do with it.
- Reproduction steps and any relevant files (config, crafted input,
  `debug-output/` artifacts). Remove personal data first: captures can show
  your BattleTag, character names and other windows, and logs and crash logs
  can contain file paths with your user name.

## Response expectations

This is a solo-maintained project; there is no on-call rotation and no
response-time guarantee. Reports are triaged on a best-effort basis, you will
get a reply once the report has been triaged, and fixes land in the next
release. Please coordinate disclosure and allow a reasonable window to prepare
and publish a fix before public details.
