# StarKit — Real-Image Corpus Spec

The photographer's real 16-bit TIFFs, against which Phase 1's real-image
acceptance is judged (FR-2 real, FR-4 sign-off, T1-10). This file is the
**authoritative selection rule** and the **committed manifest** for that corpus.

It exists because synthetic fixtures prove an algorithm is *correct*; only real
work proves it is *useful*, and the two are not substitutes (D-027). Gate **G1
cannot close until this corpus exists and passes QA.**

## 1. Privacy and storage — read first (D-041)

The photographs are the photographer's **copyrighted work**, and this repository
is **public**. Therefore:

- **Raw images are never committed.** They live in a **gitignored** `corpus/`
  directory, local only. `/corpus/` is in the root `.gitignore`.
- The repository commits **only text**: this manifest (no pixels), and the QA
  sheets under `docs/qa/` (scores and notes, no image data).
- Publishing any corpus image — even a crop — requires the photographer's
  explicit written consent, and is off by default.

### Local layout (each user creates `corpus/` themselves)

```
corpus/
  images/      the raw 16-bit TIFFs
  sky_masks/   hand-painted sky masks (PNG/TIFF), one per nightscape, same name
  manual/      the photographer's own manual-PS reduced results (when available)
```

## 2. Technical rules (what `starkit-io` can ingest)

- **Format:** 16-bit TIFF, RGB, **single layer** (flattened — no layer stacks,
  no alpha).
- **State:** stacked and stretched, i.e. the file she would hand to Photoshop —
  **not** RAW, not a linear master. `starkit-io` linearizes through the embedded
  tone curve at decode (INV-4), so a display-referred stretched TIFF is exactly
  right.
- **Colour profile:** an embedded ICC (sRGB / AdobeRGB / ProPhoto) is preferred.
  **Untagged files are accepted** and treated as sRGB (D-029), which records a
  warning; the manifest below marks which images are untagged. (An untagged file
  that is really AdobeRGB is linearized through a slightly wrong curve, which is
  acceptable for per-channel star photometry — StarKit does no colorimetry, D-029
  — but note it.)
- **Size:** ≤ 100 MP (D-042 raised the tested ceiling from 61 MP to validate the
  photographer's 100 MP Hasselblad frames; full frames are accepted, no crop
  needed). At 100 MP the pipeline runs at ~12 s (under the 20 s hard cap) and
  ~4.7 GB peak — see the memory note in PRD §6.
- **No JPEG in the primary corpus:** its blocking artifacts would confound a
  reduction-quality judgement. JPEG decodes fine for a one-off, but does not
  belong in the graded set.

## 3. Coverage — the corpus must exercise the hard cases

At least **20 images**, with a **nightscape : deep-sky ratio of ≈ 70 : 30**
(D-041), covering these categories. The minimums sum to 16; the remaining images
fill out the 70:30 mix and add variety.

| Category | What it proves | Min |
|---|---|---|
| Complex horizon (branches against sky) | FR-3 sky mask + INV-1 foreground protection — the real-world D-019/D-034 case | 4 |
| Clean horizon (ridgeline / mountain) | sky/foreground separation baseline | 2 |
| Dense Milky-Way core | deblending on real crowding | 3 |
| Strong nebulosity (deep-sky) | the "nebulosity mistaken for stars" risk (PRD §11) | 3 |
| Heavily saturated bright stars | saturation / halo / bleed on real data | 2 |
| Clean wide-field (easy baseline) | separates tool artifacts from case difficulty | 2 |

## 4. Companions required for QA

- **Every nightscape** needs a **hand-painted sky mask** (`corpus/sky_masks/`,
  PNG or TIFF, pixel-aligned, same basename as the image). Phase 1 has no auto
  segmentation; the manual path is the guarantee (FR-3), so it is what gets
  tested. White = sky (star ops allowed), black = foreground.
- **FR-4 sign-off + the 5-image manual-vs-tool benchmark** needs the
  photographer's **own manual-PS star-reduced result** for ≥ 5 images
  (`corpus/manual/`). **Deferred (D-041):** the corpus ships with originals only
  for now, so this benchmark and the FR-4 sign-off wait until those results
  arrive. T1-10's structured visual QA (detection / mask / reduction quality)
  proceeds without them.

## 5. Manifest

One row per corpus image. Filled as images arrive; **text only, no pixels.**
`icc` = embedded profile or `untagged`. `sky_mask` / `manual` = whether the
companion exists yet.

| id | category | megapixels | icc | sky_mask | manual | notes |
|---|---|---|---|---|---|---|
| _(awaiting the first images)_ | | | | | | |

## 6. What each corpus image feeds

- **FR-2 real:** structured visual-QA checklist + oracle (photutils) cross-check
  within agreed tolerances — `docs/qa/`.
- **FR-3:** the invariant test proves foreground pixels are bit-identical to
  input, now on *her* nightscapes, not only `nightscape-fg`.
- **FR-4:** she confirms the exported mask is usable as-is in her PS workflow on
  ≥ 5 images — **blocked on her sign-off** (§4).
- **T1-10:** masks + reduction QA sheet under `docs/qa/`.

Naming a file into this spec does not make it part of the graded set — it must
also satisfy §2 and land in a §3 category. An image that fits no category is
extra, not corpus.
