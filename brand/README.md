# Telmoni brand files

The mark, wordmark, lockup, icons, social cards and platform uploads for Telmoni, built to the file contract. Every raster is rendered from the SVG masters at exactly its canvas. `MANIFEST.md` lists every file with its format, canvas or `viewBox`, alpha, palette and size in bytes, each checked against the contract. `proof/` shows the favicon in browser tabs and the small sizes at real pixels.

## What changed from today's pack

- **Mark:** redrawn as the fused "tm" with the found unit. It now has **two tones** instead of three. `viewBox` is `0 0 1305 1000` (1.305:1), previously `40 125 955 740`.
- **Colours:** pinned to `#0A0A0A` (dark) and `#FAFAFA` (light) everywhere, with the unit in amber, `#F2A93B`. These replace `#09090B`, `#F7F7F8` and the drifted `#0A0A0B`.
- **Wordmark:** `viewBox` is now `0 -1000 4614.38 1010.5`.
- **Lockup:** `viewBox` is now `0 -820 4546.17 827.35`, at **5.49:1**. The Stripe and WorkOS logo images are therefore **1024 × 186**.
- **Favicons:** `icon.svg`, `favicon.ico` and `icon.png` use a small-size cut of the mark, fitted to whole pixels at 16, 32 and 48 px (see below).
- **Optional preview:** `uploads/github-social-1280x640.png` is included.
- **Card text:** both card pairs say exactly the lines in the contract, so no alt text is needed.

## The mark

Two tones:

| Tone | What it is | Shapes, in document order |
| --- | --- | --- |
| **Body** | the fused t and m | 1 `rect` t stem · 2 `rect` crossbar · 3 `path` first arch and its leg · 4 `path` last arch and its leg |
| **Unit** | the found square under the last leg | 5 `rect` |

- Every shape carries exactly one tone; files hold no ids or classes, so map tones by order.
- **The unit is amber:** `#F2A93B` in every colour palette; only the `-black`, `-white` and `currentColor` masters draw it in the body's colour. It's a separate tone so the console can paint it from its own token, for example a live status or the "found" animation. Don't recolour it with chart colours.
- Body shapes overlap inside the letter, so no hairline seams show between them. Paint tones with opaque colours.
- Construction, in `viewBox` units:
  - stem 200 (one fifth of the height); counters 250; crossbar 300–480.
  - arches: radius 325, with the last arch rising 12.5 higher.
  - unit: a 210 square, 70 below the end of the last leg.

## Palettes

| Tone | on-light | on-dark | fallback |
| --- | --- | --- | --- |
| Body | `#0A0A0A` | `#FAFAFA` | `#787878` |
| Unit | `#F2A93B` | `#F2A93B` | `#F2A93B` |
| Wordmark (in lockups) | `#0A0A0A` | `#FAFAFA` | — |

| Tile | Background | Mark |
| --- | --- | --- |
| Light tile | `#FAFAFA` | on-light |
| Dark tile | `#0A0A0A` | on-dark |

- **Dark and light values:** `#0A0A0A` and `#FAFAFA` are shadcn New York neutral-950 and neutral-50. They're the only dark and light used in any file.
- **Amber:** `#F2A93B`, the unit's colour, and the only colour beside the dark and light.
- **Fallback:** `#787878` is the grey that reads equally on white (4.4:1) and on the dark tile (4.5:1).
- **Card text:** the line on the social cards is `#404040`, on the light.
- **Black and white masters:** the `-black` and `-white` files use pure `#000000` and `#FFFFFF`.

## Wordmark

- `viewBox="0 -1000 4614.38 1010.5"`, with the baseline at y = 0.
- Ascender 1000, x-height 700, stem 171.5.
- Round letters overshoot 10.5 below the baseline.

## Lockup

`viewBox="0 -820 4546.17 827.35"`, aspect 5.49:1. The mark stands on the baseline and sets the top. The `viewBox` bottom takes in the letters' overshoot.

```
<g id="mark" transform="translate(0 -820) scale(0.82)">   mark.svg's shapes, unchanged
<g id="word" transform="translate(1316.1 0) scale(0.7)">   wordmark.svg's path, unchanged
```

- **Gap:** 246, which is 1.5 × the stem of the scaled mark.
- **Alignment:** the top of the mark's crossbar sits on the line the i's square dot hangs from (y ≈ −574).

## Small-size cut (favicons)

`icon.svg`, `uploads/workos-favicon.svg`, `favicon.ico` and `icon.png` draw the mark on a 32-px grid. Every edge sits on an even grid line, so the mark lands on whole pixels at 16, 32 and 48 px.

| | At 32 px | At 16 px |
| --- | --- | --- |
| Stems | 4 | 2 |
| Counters | 6 | 3 |
| Unit | 4 | 2 |
| Gap above the unit | 2 | 1 |

- The mark spans 896 × 704 of the 1024 canvas. It is raised one grid step so it looks centred.
- The 16, 32 and 48 px frames are rendered from `masters/mark-small-color-transparent.svg`. That file isn't in the contract; it's kept as the raster source.

## Clear space

One found unit (the square) on every side:
- **Mark:** 210 in the mark's `viewBox` (21% of its height).
- **Lockup:** 172.2 in the lockup's `viewBox`. That's about 4 px at 20 px tall and 5 px at 24 px tall.

## Minimum sizes

- **Lockup:** 20 px tall (110 px wide). The console header and sign-in page.
  - At 20 px the gap above the unit is about 1.4 px: visible, but soft.
  - From 24 px the gap is clean.
- **Mark:** a 20 × 20 px box, where the master is 15.3 px tall.
  - Place the mark 2 px from the top of the box rather than centring it (2.34 px). The gap above the unit then lands on a clear pixel row.
- **Favicon:** 16 px, using the small-size cut.

`proof/sizes.png` shows each of these at real pixels, on light and on dark, together with the 24 px site header and the README's 360 px lockup.

## Layouts

- **Square icons:**
  - The mark is 61.2% of the canvas wide and centred: `translate(198.8 272) scale(0.48)` in a 1024 canvas.
  - The ink stays inside 77% of the canvas width measured as a centred circle. That clears the maskable 80% circle and X's circular crop.
  - Used in the tiles, maskable icons, `apple-icon.png`, `icon-192/512.png` and every square upload.
- **Social cards:**
  - Background: the light, `#FAFAFA`.
  - Lockup: on-light, centred.
  - Line: Inter Regular, `#404040`, centred under the lockup.
- **X header and GitHub's social preview:** the same layout as the cards, with the line "Privacy-first, open-source monitoring for AI agents."
- **LinkedIn cover:** the lockup alone, 330 px wide, centred.

## Files

```
masters/
  mark.svg                      currentColor
  mark-color-on-light.svg       mark-color-on-dark.svg       mark-color-transparent.svg (fallback)
  mark-black.svg                mark-white.svg
  wordmark.svg                  wordmark-black.svg           wordmark-white.svg
  lockup.svg                    lockup-color-on-light.svg    lockup-color-on-dark.svg
  lockup-black.svg              lockup-white.svg
  mark-tile-light.svg           mark-tile-dark.svg           1024, opaque
  mark-small-color-transparent.svg                           favicon source (extra)
app/  and  web/app/             identical icons; the cards differ by their line
  icon.svg                      on-light, on-dark under prefers-color-scheme
  favicon.ico                   16, 32, 48 · on-light
  icon.png                      32 · on-light
  icon-192.png  icon-512.png    light tile, no alpha
  icon-192-maskable.png  icon-512-maskable.png   light tile, no alpha
  apple-icon.png                180, light tile, no alpha
  opengraph-image.png           1200×630, opaque
  twitter-image.png             1200×600, opaque
                                both cards in app/ read "Privacy-first, open-source monitoring for AI agents."
                                both cards in web/app/ read "Console and docs"
uploads/
  github-avatar-1024.png                                      transparent, on-light
  slack-icon-1024.png  stripe-icon-512.png  workos-logo-icon-512.png
  x-profile-400.png  linkedin-logo-300.png                    transparent, fallback
  stripe-logo-1024.png  workos-logo-1024.png                  1024×186, on-light
  stripe-logo-1024-on-dark.png                                1024×186, on-dark
  workos-logo.svg               the on-light lockup
  workos-favicon.svg            a copy of icon.svg
  x-header-1500x500.png  linkedin-cover-1128x191.png  github-social-1280x640.png   opaque
proof/
  favicon-tabs.png              16, 32, 48 px in light and dark tabs
  sizes.png                     the sizes above at real pixels
  contact-sheet.png             every raster in context
MANIFEST.md
```

The card text is set in [Inter](https://rsms.me/inter/) (SIL Open Font License 1.1).
