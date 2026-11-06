# Manifest

Every file in the pack, checked against the file contract: 55 files plus this manifest. All checks pass.

| File | Format | Canvas / viewBox | Alpha | Palette | Bytes | Check |
| --- | --- | --- | --- | --- | ---: | --- |
| `masters/mark.svg` | SVG | viewBox 0 0 1305 1000 | transparent | currentColor | 550 | ✓ |
| `masters/mark-color-on-light.svg` | SVG | viewBox 0 0 1305 1000 | transparent | on-light | 525 | ✓ |
| `masters/mark-color-on-dark.svg` | SVG | viewBox 0 0 1305 1000 | transparent | on-dark | 525 | ✓ |
| `masters/mark-color-transparent.svg` | SVG | viewBox 0 0 1305 1000 | transparent | fallback | 525 | ✓ |
| `masters/mark-black.svg` | SVG | viewBox 0 0 1305 1000 | transparent | black | 525 | ✓ |
| `masters/mark-white.svg` | SVG | viewBox 0 0 1305 1000 | transparent | white | 525 | ✓ |
| `masters/wordmark.svg` | SVG | viewBox 0 -1000 4614.38 1010.5 | transparent | currentColor | 1,148 | ✓ |
| `masters/wordmark-black.svg` | SVG | viewBox 0 -1000 4614.38 1010.5 | transparent | black | 1,143 | ✓ |
| `masters/wordmark-white.svg` | SVG | viewBox 0 -1000 4614.38 1010.5 | transparent | white | 1,143 | ✓ |
| `masters/lockup.svg` | SVG | viewBox 0 -820 4546.17 827.35 | transparent | currentColor | 1,745 | ✓ |
| `masters/lockup-color-on-light.svg` | SVG | viewBox 0 -820 4546.17 827.35 | transparent | on-light | 1,715 | ✓ |
| `masters/lockup-color-on-dark.svg` | SVG | viewBox 0 -820 4546.17 827.35 | transparent | on-dark | 1,715 | ✓ |
| `masters/lockup-black.svg` | SVG | viewBox 0 -820 4546.17 827.35 | transparent | black | 1,715 | ✓ |
| `masters/lockup-white.svg` | SVG | viewBox 0 -820 4546.17 827.35 | transparent | white | 1,715 | ✓ |
| `masters/mark-tile-light.svg` | SVG | viewBox 0 0 1024 1024 | opaque | light tile · on-light | 626 | ✓ |
| `masters/mark-tile-dark.svg` | SVG | viewBox 0 0 1024 1024 | opaque | dark tile · on-dark | 626 | ✓ |
| `masters/mark-small-color-transparent.svg` | SVG | viewBox 0 0 1024 1024 | transparent | fallback | 527 | ✓ extra, not in the contract: source of favicon.ico and icon.png |
| `app/icon.svg` | SVG | viewBox 0 0 1024 1024 | transparent | on-light · on-dark (prefers-color-scheme) | 608 | ✓ |
| `app/favicon.ico` | ICO | 16, 32, 48 | yes | on-light | 2,452 | ✓ |
| `app/icon.png` | PNG | 32×32 | yes | on-light | 573 | ✓ |
| `app/icon-192.png` | PNG | 192×192 | no | light tile · on-light | 2,099 | ✓ |
| `app/icon-512.png` | PNG | 512×512 | no | light tile · on-light | 5,679 | ✓ |
| `app/icon-192-maskable.png` | PNG | 192×192 | no | light tile · on-light · safe-zone padded | 1,979 | ✓ |
| `app/icon-512-maskable.png` | PNG | 512×512 | no | light tile · on-light · safe-zone padded | 5,349 | ✓ |
| `app/apple-icon.png` | PNG | 180×180 | no | light tile · on-light | 1,912 | ✓ |
| `app/opengraph-image.png` | PNG | 1200×630 | no | light · on-light | 24,752 | ✓ |
| `app/twitter-image.png` | PNG | 1200×600 | no | light · on-light | 24,548 | ✓ |
| `web/app/icon.svg` | SVG | viewBox 0 0 1024 1024 | transparent | on-light · on-dark (prefers-color-scheme) | 608 | ✓ |
| `web/app/favicon.ico` | ICO | 16, 32, 48 | yes | on-light | 2,452 | ✓ |
| `web/app/icon.png` | PNG | 32×32 | yes | on-light | 573 | ✓ |
| `web/app/icon-192.png` | PNG | 192×192 | no | light tile · on-light | 2,099 | ✓ |
| `web/app/icon-512.png` | PNG | 512×512 | no | light tile · on-light | 5,679 | ✓ |
| `web/app/icon-192-maskable.png` | PNG | 192×192 | no | light tile · on-light · safe-zone padded | 1,979 | ✓ |
| `web/app/icon-512-maskable.png` | PNG | 512×512 | no | light tile · on-light · safe-zone padded | 5,349 | ✓ |
| `web/app/apple-icon.png` | PNG | 180×180 | no | light tile · on-light | 1,912 | ✓ |
| `web/app/opengraph-image.png` | PNG | 1200×630 | no | light · on-light | 16,910 | ✓ |
| `web/app/twitter-image.png` | PNG | 1200×600 | no | light · on-light | 16,761 | ✓ |
| `uploads/github-avatar-1024.png` | PNG | 1024×1024 | yes | on-light | 12,002 | ✓ |
| `uploads/slack-icon-1024.png` | PNG | 1024×1024 | yes | fallback | 9,311 | ✓ ink within 77.1% circle |
| `uploads/stripe-icon-512.png` | PNG | 512×512 | yes | fallback | 3,722 | ✓ ink within 76.9% circle |
| `uploads/workos-logo-icon-512.png` | PNG | 512×512 | yes | fallback | 3,722 | ✓ ink within 76.9% circle |
| `uploads/stripe-logo-1024.png` | PNG | 1024×186 | yes | on-light | 9,532 | ✓ ink touches 0..1023 × 0..185 |
| `uploads/workos-logo-1024.png` | PNG | 1024×186 | yes | on-light | 9,532 | ✓ ink touches 0..1023 × 0..185 |
| `uploads/stripe-logo-1024-on-dark.png` | PNG | 1024×186 | yes | on-dark | 9,533 | ✓ ink touches 0..1023 × 0..185 |
| `uploads/x-profile-400.png` | PNG | 400×400 | yes | fallback | 2,987 | ✓ ink within 77.1% circle |
| `uploads/x-header-1500x500.png` | PNG | 1500×500 | no | light · on-light | 22,953 | ✓ |
| `uploads/linkedin-logo-300.png` | PNG | 300×300 | yes | fallback | 2,154 | ✓ ink within 77.0% circle |
| `uploads/linkedin-cover-1128x191.png` | PNG | 1128×191 | no | dark · on-dark | 5,732 | ✓ |
| `uploads/github-social-1280x640.png` | PNG | 1280×640 | no | light · on-light | 26,112 | ✓ |
| `uploads/workos-logo.svg` | SVG | viewBox 0 -820 4546.17 827.35 | transparent | on-light | 1,715 | ✓ |
| `uploads/workos-favicon.svg` | SVG | viewBox 0 0 1024 1024 | transparent | on-light · on-dark (prefers-color-scheme) | 608 | ✓ |
| `README.md` | Markdown | — | — | — | 8,384 | ✓ spec |
| `proof/contact-sheet.png` | PNG | 1500×2475 | no | — | 299,434 | ✓ proof |
| `proof/favicon-tabs.png` | PNG | 1824×876 | no | — | 78,358 | ✓ proof |
| `proof/sizes.png` | PNG | 1472×736 | no | — | 55,404 | ✓ proof |
