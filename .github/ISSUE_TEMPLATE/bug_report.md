---
name: Bug report
about: Something in d4lc does not work as expected on Linux (X11, or experimental Wayland)
title: "Bug: <short summary>"
labels: ["bug"]
assignees: []
---

## Summary

What happened, and what you expected instead.

## Environment

- d4lc version or commit, and how you run it (release artifact / built from source):
- Distribution and desktop environment:
- X11, XWayland or Wayland session? (`echo $XDG_SESSION_TYPE`, `echo $DISPLAY`)
- Game window resolution and system preset used (e.g. `1440p_SMF`):
- OCR language(s) enabled:

## Reproduction steps

Minimal, numbered steps that trigger the bug from a normal session start:

1.
2.
3.

Actual result:

Expected result:

## Before you attach anything: remove personal data

Issues are public. Screenshots, captures and logs can contain personal data;
check every file before attaching it:

- Crop images to the part that shows the bug (usually the item tooltip or the
  overlay). Do not upload unfiltered full-screen or full-desktop captures:
  root-window and manual-region captures include every other open window.
- Black out your BattleTag, character, clan and friend names, chat, account
  or region details, and anything shown by other windows or notifications.
- Replace your home directory and user name in paths (for example
  `/home/<you>/...` becomes `~/...`) in logs, crash logs and command output.
- Crash logs (`~/.local/share/d4lc/crashes/`) contain the panic message,
  a backtrace and a settings snapshot; read them before attaching and redact
  paths or names.
- Do not attach config, profile or build files you have not read; they may
  contain names or private notes.

If a bug cannot be shown without personal data, say so in the issue and ask
for a private way to share it instead of uploading it.

## Debug output

If you used the app's Debug screen or the CLI's live commands, the files under
`debug-output/` (captures, OCR dumps, screenshots) help with capture, detection
and OCR bugs. A single raw frame of the game window is often enough (the debug
CLI is `d4lc-debug` in release packages and `d4lc` in a source build):

```bash
d4lc-debug capture-window-raw --match "Diablo IV" --output frame.png
```

Its printed JSON describes only the captured game window. Do not paste the
output of `d4lc-debug x11-windows`: it lists the title, class and process id of
every open window on your desktop.

Apply the checklist above to every file and every pasted command output: crop
the frame to the relevant tooltip or overlay and redact names before attaching
it. Link large files instead of pasting them.

## Logs

Relevant stderr output or panic messages, with paths and names redacted:

```text

```

## Additional context

Anything else that narrows it down (worked in an earlier release, only in live
loop, only with overlay enabled, ...).
