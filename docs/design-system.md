# Desktop design

The interface is an English/Korean image review and print workspace. Keep the final white
paper prominent and routine controls brief. It is not an image editor.

## Ownership and components

Read this document before changing the UI. [Architecture](architecture.md) defines
the code boundaries: `App.tsx` owns display/zoom; `usePrintWorkflow.ts` owns
operations; `native.ts` owns Tauri IO; `contracts.ts` contains DTOs.

Use the existing HeroUI v3 Button, Input, Label, Checkbox, Select, Accordion,
Tooltip, Spinner, and ProgressBar. Mutable controls receive explicit disabled
states. Sections use headings, space, and separators, without Card/Surface
wrappers or nested cards. Keep one primary action: Open for an empty workspace,
Print after an image or pattern is loaded.

## Tokens

[`src/design-tokens.css`](../src/design-tokens.css) is the source of truth for
color, typography, spacing, radius, and motion; [`src/styles.css`](../src/styles.css)
owns layout. Do not create local palettes.

| Role | Value / use |
|---|---|
| Canvas / foreground | `#010102` / `#f7f8f8` |
| Secondary / muted | `#d0d6e0` / `#8a8f98` |
| Accent / hover / focus | `#5e6ad2` / `#828fff` / `#5e69d1` |
| Surfaces | `#0b0c0f`, `#111216`, `#191b20`, `#23252a` |
| Essential border / decorative separator | `#62666d` / `#23252a` |
| Paper | White, black dots, `#d4d4d8` unprintable stripes |
| Type | System Korean-capable sans; labels 13px, body 14px, headings 15px |
| Spacing / controls | 4px grid, 8px radius, 44px control height |

Lavender is for primary actions, links, and focus. Selected list items and
checkboxes stay neutral. Primary text is white, hover text is canvas, and pressed
text is white. Keep dark styling in portals and the native title bar.
Use opaque 3px focus rings with 3px offsets on the actual interactive control;
avoid duplicate focus outlines on Select wrappers. Body text needs 4.5:1 contrast
and essential boundaries/focus 3:1. Decorative separators and disabled states
are distinct from essential boundaries. IDs/hashes use monospace; settings do not.

## Layout and preview

At 1100×820, use a left preview and a 300px right inspector. Keep a small header
with OpenLabel, Open, the filename, and a compact language selector. Only inspector settings scroll; copies,
Print, Cancel, progress, and result stay in the action area. A 50×80 mm label in
fit view and Print should be visible together.

At widths ≤850px, use one column and page scrolling; the minimum window is
640×600, with a 280px preview and sticky action area. Open must be visible in the
initial empty viewport. Long names and IDs wrap without horizontal page overflow.
Leave focus-ring space and scroll padding so sticky actions do not obscure focus.

Fit centers the whole paper within the bounded viewport without scrollbars.
1×/2×/4× dot views use internal scrolling and nearest-neighbor pixels. Zoom and
disclosures remain usable while a print locks mutable settings. Both paper and
head views display the same Rust PNG; overlays never change transmitted dots.
Never animate the paper or PNG.

Printer, advanced settings, and technical details use three non-nested disclosures.
Closed content stays mounted but is hidden from keyboard and accessibility navigation.
Keep exact device IDs, paths, hashes, dot counts, and full errors in those details.
Essential errors, clipping notices, progress, and next actions remain discoverable
without a tooltip. Open advanced settings when an invalid advanced field needs correction.

## Interaction and state

Language defaults to Korean for a Korean system locale and English otherwise.
An explicit English/한국어 selection is remembered; invalid/unreadable storage
falls back to the system locale, and write failure keeps the current session's
selection with a notice. Language remains available while work is locked.
All app-owned labels, validation, status, errors, dialog titles/filters, and
accessible text use the shared catalogs. The default macOS menus also follow the
selection through a display-only native call, preserving roles and shortcuts.
React Aria receives the same locale;
the document's `lang` and `dir` follow it. Language changes never reread input,
regenerate previews, reconnect, print, cancel, or change settings and hashes.

macOS Bluetooth usage descriptions use English base and English/Korean
`InfoPlist.strings` resources. Permission prompts and native Open/Save/Cancel
buttons follow the OS/app language chosen by macOS, independently of the live
in-app selection; the dialog plugin exposes titles and filters, not those buttons.


Opening or dropping an image loads its own validated settings. Desktop paper
width/height persist across images, reset, printing, and restarts; explicit
**Load settings / 설정 불러오기** adopts that file's paper. Invalid numeric input, including blank
values, remains visible and blocks Print rather than being clamped on blur.
Bad sidecars require explicit reset or another file. Storage failure does not
block the current session, but must tell the user paper could not be remembered.

Artwork scale is distinct from screen zoom. Fit restores scale to 100% and keeps
paper/calibration. Mirror precedes clockwise rotation; paper axes stay fixed.
The calibration pattern uses current settings, one copy, and 100% scale; returning
to the image restores its scale, including invalid input. Pattern selection never
prints or overwrites the source. Save settings from image mode.

Changes to source or raster settings immediately invalidate the preview and Print.
Late preview/dialog/scan/save results must not overwrite newer operations. Runtime
polling distinguishes the current job from the desktop-owned job; a CLI print
locks editing and shows progress without replacing local image/settings/selection.
A terminal result remains visible until a new explicit action. `completed`,
`failed`, and `cancelled` cannot be overwritten by a late cancel/poll error;
mutable controls and the next print stay locked until `finished`.

A candidate selection is not a verified connection. Print requires a current
preview, both hashes, an allowed exact ID, body-model confirmation, valid inputs,
and no active conflicting operation. Native evidence alone controls the connected
status. A poll error removes verified connection display and blocks printing.
No automatic selection, model confirmation, or reprint is introduced.

Use visible labels, keyboard focus, text status/live regions, associated numeric
errors, and useful preview alt text. Show Spinner only during real work and
ProgressBar from actual transmitted bytes. Tooltips supplement existing labels.
Motion uses 150ms feedback, 250ms entry, and 100ms exit; reduced-motion removes
transitions and spinning. Automated DOM checks do not substitute for native
keyboard, window-size, reduced-motion, or physical print verification.
