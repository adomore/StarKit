# docs/qa — real-corpus QA sheets (T1-10)

One QA sheet per corpus image, scoring StarKit's output on the photographer's own
work. This is the "structured visual-QA checklist" FR-2 and FR-4 require, and the
T1-10 deliverable.

- Corpus spec and manifest: [CORPUS.md](../CORPUS.md).
- Images themselves are gitignored (`corpus/`, D-041) — these sheets carry
  **scores and notes only, no pixels**.
- Copy [TEMPLATE.md](TEMPLATE.md) to `<image-id>.md` and fill it in.

## How a sheet is produced

1. Run the tool on the image (and its sky mask, for a nightscape):
   ```
   starkit process --input corpus/images/<id>.tiff \
     [--sky corpus/sky_masks/<id>.png] \
     --out out/<id>.starkit.tiff --mask out/<id>.mask.tiff --report out/<id>.json
   ```
2. Cross-check detection against the oracle (FR-2 real):
   ```
   starkit inspect --input corpus/images/<id>.tiff --out out/<id>.cat.json
   oracle/.venv/bin/python oracle/measure.py --image corpus/images/<id>.tiff --out out/<id>.oracle.json
   oracle/.venv/bin/python oracle/compare.py --truth ... # or a detection-vs-detection Δ where there is no truth
   ```
   A real image has **no truth catalog**, so the oracle cross-check is
   detection-vs-detection (Δrecall / Δcentroid between StarKit and photutils),
   not against ground truth. Record the agreed tolerance and the measured Δ.
3. Score each axis in the template, 1–5, with a one-line reason. A score is
   worthless without the reason.

## Pass criteria (per phase)

- **FR-2 real:** every graded image's oracle cross-check is within the agreed
  tolerance, and the visual checklist has no score below 3 on detection.
- **FR-4:** the photographer rates the exported mask usable as-is on ≥ 5 images
  — **blocked** until she supplies sign-off (CORPUS.md §4).
- **Manual-vs-tool benchmark:** on the ≥ 5 images with a manual result, she rates
  the tool equal-or-better on the time/quality trade for ≥ 4/5 — **blocked** on
  the manual results (CORPUS.md §4).
