# Live Mobalytics captures (2026-10-15)

Real provider answers, reduced to the fields the d4lc parsers read. They pin
the site shapes that broke the import in October 2026.

| File | Source | Shape it pins |
|---|---|---|
| `build-by-id.graphql.json` | `api/diablo4` GraphQL body of `https://mobalytics.gg/diablo-4/profile/littlegrief-ic8lxh/builds/a682e939-08d1-41b3-a90d-bb2f51bf8e0d` (captured by headless Chromium) | `implicitStats: null`, `null` entries inside `gearStats` |
| `build-by-slug.diablo4state.json` | `window.__PRELOADED_STATE__.diablo4State` of `https://mobalytics.gg/diablo-4/builds/warlock-blazing-scream` | an `ngf-comments` query whose `state.data` is an object, a `null` query entry, the empty `no-variant-id` placeholder variant |
| `profile-builds.diablo4state.json` | `diablo4State` of `https://mobalytics.gg/diablo-4/profile/mrronit/builds` | the profile queries (`/profile/<user>` itself is an empty client shell) |

Reduction (scripted, structure unchanged): only `id`, names, dates, slugs,
variant ids/titles, gear slots (slot, entity slug/title/type, modifier ids),
socket and paragon slugs are kept. Icons, colors, guide text, comments,
author/account data and the document-type catalogue were removed; the
comment query keeps its object shape with `REDACTED` ids and no comments.
