# QA — `<image-id>`

Copy this file to `<image-id>.md`. Text only; no pixels (D-041).

## Image

| field | value |
|---|---|
| id | `<id>` |
| category | complex-horizon / clean-horizon / dense-core / nebulosity / saturated / clean-widefield |
| megapixels | |
| icc | e.g. AdobeRGB / untagged→sRGB |
| sky mask used | yes / no (path) |
| preset | default / `<path>` |

## Result (from the per-image report)

| field | value |
|---|---|
| stars detected | |
| gate open fraction | |
| decode warnings | |

## Oracle cross-check (FR-2 real)

Real images have no truth catalog, so this is StarKit-vs-photutils, not
vs ground truth. Record the agreed tolerance and the measured delta.

| metric | StarKit | oracle (photutils) | Δ | within tolerance? |
|---|---|---|---|---|
| stars @ SNR≥5 | | | | |
| median centroid Δ (px) | | | | |
| notes | | | | |

## Visual scores (1–5, with a reason — a score without a reason is noise)

| axis | score | reason |
|---|---|---|
| Detection: real stars found, few false positives | | |
| Nebulosity **not** eaten / mistaken for stars | | |
| Sky mask: foreground untouched (INV-1), edges clean | | |
| Reduction: no dark halos, no ringing | | |
| Reduction: stars look natural, series-consistent | | |
| Mask export: usable as-is in PS *(FR-4 — blocked on sign-off)* | | |

## Manual-vs-tool *(only if a manual PS result exists — else N/A, blocked)*

| | verdict |
|---|---|
| Tool equal-or-better on time/quality trade? | yes / no / N/A |
| One-line comparison | |

## Verdict

- [ ] Detection acceptable (no axis < 3)
- [ ] Reduction acceptable
- [ ] INV-1 holds visually (foreground untouched)
- Overall: **pass / needs-work / fail** — one line why.
