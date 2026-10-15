# Bot-protection fixtures

`mobalytics-cloudflare-challenge.dom.html`: the DOM headless Chromium 153
rendered for `https://mobalytics.gg/diablo-4/builds` on 2026-10-12T19:54Z
(HTTP 403, Cloudflare managed challenge "Just a moment..."). The inline
`<style>` was dropped and the per-request challenge tokens, nonces and the ray
id were replaced with `REDACTED`; the structure and the markers are unchanged.
Used by the build-download challenge detection tests.
