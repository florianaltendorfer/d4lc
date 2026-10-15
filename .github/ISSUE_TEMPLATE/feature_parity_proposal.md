---
name: Feature / parity proposal
about: Propose porting a feature or behavior from Diablo4Companion, with upstream source evidence
title: "Parity proposal: <short summary>"
labels: ["enhancement", "parity"]
assignees: []
---

## Proposal

What d4lc should do after this port, in one or two sentences.

## Upstream reference

d4lc ports behavior, it does not invent it. Every claim needs a source-level
reference that can be checked against the public upstream sources
(https://github.com/josdemmers/Diablo4Companion), which are pinned by
`upstream.lock` and available locally in the `upstream/Diablo4Companion`
submodule.

- Upstream release/commit:
- Change source: release / branch head / wiki page

## Behavior evidence

List each behavior claim with the upstream file or method that implements it.
Use `UNKNOWN` only when the source could not be inspected — do not guess.

| Claim | Upstream path/method | Notes |
|---|---|---|
|  |  |  |

## Linux/X11 considerations

- Upstream Windows dependencies (WPF/Win32/DirectX) that need a native
  replacement here:
- Interactions with capture, tooltip detection, OCR, or the overlay:

## Why it matters

What this unlocks for the Linux port, and which parts of the pipeline it
touches.

## Alternatives considered

Other ways to get the same result, and why the upstream behavior wins.
