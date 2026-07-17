# StarKit — Automated Star Processing for Astrophotography

**Functional Specification (PRD) · v1.0 · approved 2026-07-16**
**Status: APPROVED (2026-07-16). Authoritative implementation spec — changes require a docs/DECISIONS.md entry.**
**Audience: product owner (MingJiang), primary user (professional astrophotographer), Claude Code (implementation agent).**

Glossary: 缩星 = star reduction · 提亮星点 = star enhancement · 星野 = nightscape (sky + foreground) · 深空 = deep-sky · 柔焦 = diffusion/soft-glow

---

## 1. Problem statement

Professional astrophotography post-processing spends a disproportionate amount of time on two repetitive operations:

1. **Star reduction (缩星).** After stacking and stretching, stars are bloated relative to nebulosity / the Milky Way. The manual workflow (Photoshop: Color Range selection → refine mask by hand → Minimum filter → repair dark halos and broken star edges; PixInsight: build star mask → MorphologicalTransformation → fix artifacts) takes 15–40 minutes per image and is hard to reproduce consistently across a series.
2. **Star enhancement (提亮星点).** Making the principal stars pop so constellations read clearly: duplicate layer → Gaussian blur → Screen blend → hand-paint a mask restricted to the main stars → recover star color on clipped cores. Equally tedious, equally unrepeatable.

The root cause of both pains is the same: **producing an accurate, artifact-free star mask is the actual bottleneck.** Reduction and enhancement are cheap parametric operations *once a high-quality mask exists*. A second, nightscape-specific problem: any automatic star operation must not touch the foreground (trees, ridgelines, buildings) — naive tools mistake foreground highlights for stars.

**Goal:** a desktop tool that automates *star detection → sky/foreground separation → star masking → parametric reduction / enhancement*, with deterministic batch processing and lossless round-trip into an existing Photoshop workflow. Target: reduce a 20–40 minute manual task to under one minute plus review.

## 2. Target users and workflow position

- **Primary:** professional nightscape / deep-sky photographer. Photoshop-centric. Works on stacked, stretched **16-bit TIFF** files, typically 24–61 MP.
- **Secondary:** MingJiang — CLI batch use, pipeline integration, maintenance.

Workflow position: **after** stacking/stretching (Sequator / Siril / PS), **before or alongside** final grading in Photoshop. Therefore the tool must export layers and masks that drop cleanly back into PS (16-bit, ICC preserved, pixel-aligned).

## 3. Goals and non-goals

**Goals (v1.x)**

- G1 — Automatic star detection with sky/foreground separation.
- G2 — Parametric star reduction with a hard guarantee: pixels outside the star mask are untouched.
- G3 — Selective star enhancement: glow, luminosity lift, star-color recovery, optional diffusion-filter (柔焦/Softon-style) emulation.
- G4 — Deterministic batch processing: same input + same params → bit-identical output.
- G5 — Exportable intermediate assets for PS compositing: star mask(s), starless layer, stars-only layer.

**Non-goals (v1)**

RAW decode/demosaic · stacking/alignment · gradient & light-pollution removal · denoising · general retouching · tracking-mount tooling · mobile platforms. (Some may become v2+ candidates; explicitly out of scope now to keep the phase gates honest.)

## 4. Processing pipeline (overview)

```
Input TIFF ──▶ Linearize (ICC → linear working space)
          ──▶ Sky/foreground segmentation  (auto + imported/painted mask)
          ──▶ Background model (tile-based sigma-clipped estimation)
          ──▶ Star detection → star catalog (x, y, flux, FWHM, peak, saturated, tier)
          ──▶ Star masks (tiered, feathered)
          ──▶ [Reduce] ──▶ [Enhance] ──▶ Recompose
          ──▶ De-linearize ──▶ Output TIFF 16-bit (+ optional: mask / starless / stars-only)
```

All photometric operations run in linear light; gamma/ICC handled at the boundaries. All star operations are gated by `sky_mask ∧ star_mask` — this is a structural invariant, not a convention.

## 5. Functional requirements

### FR-1 Image I/O

- **In:** TIFF 8/16-bit RGB (single layer), PNG, JPEG. Embedded ICC honored (sRGB / AdobeRGB / ProPhoto). Tested up to 61 MP.
- **Out:** TIFF 16-bit default. ICC profile and EXIF preserved. Never overwrites input by default; atomic temp-write + rename.
- **Acceptance:** no-op run (all operations disabled) produces pixel-identical TIFF16 output. Corrupt/truncated input fails with a clean error, never a partial output file.

### FR-2 Star detection and PSF measurement

- Tile-based background + noise estimation (sigma-clipped); thresholded local-maxima detection; subpixel centroid; per-star FWHM (elliptical fit — ellipticity retained for later star-shape repair); saturation flagging; deblending of close pairs; tier classification (large / medium / small) by flux and FWHM.
- Output: star catalog (JSON + in-memory), the substrate for every downstream op.
- **Acceptance (synthetic golden fields, generator committed in repo):** recall ≥ 98 % at SNR ≥ 5; precision ≥ 99 %; median centroid error ≤ 0.3 px; median FWHM error ≤ 10 %.
- **Acceptance (real corpus, ≥ 20 photographer-supplied TIFFs incl. hard cases):** structured visual-QA checklist passes; results cross-checked against Python oracle (photutils DAOStarFinder) within agreed tolerances.

### FR-3 Sky / foreground segmentation

- **Auto mode:** heuristic segmentation exploiting nightscape structure (sky = smooth gradient + point sources; foreground = dark, high-texture, connected-from-bottom). Outputs sky mask + confidence map.
- **Manual override (must ship in Phase 1):** import an external grayscale mask (PNG/TIFF) painted in PS. GUI brush arrives in Phase 2. Auto quality is best-effort; the manual path is the guarantee.
- **Hard invariant:** zero star operations outside the sky mask — enforced structurally and covered by tests.
- **Acceptance:** IoU ≥ 0.95 vs. hand labels on clean-horizon test images; complex horizons (trees) always correctable via imported mask; invariant test proves foreground pixels are bit-identical to input.

### FR-4 Star mask generation (a deliverable in its own right)

- Soft mask from catalog: per-star radius = k × FWHM with configurable feather; halo extension for saturated stars; separate tier masks (large/medium/small) plus combined.
- Export as 16-bit grayscale TIFF/PNG, pixel-aligned to source — directly usable as a PS layer mask. *This alone replaces the most tedious manual step and is the first user-visible win.*
- **Acceptance:** photographer confirms exported mask is usable as-is in her PS workflow on ≥ 5 of her own images.

### FR-5 Star reduction (缩星)

- **Method A — resynthesis (default):** per star: estimate local background (annulus + inpaint), extract the star profile, re-add a PSF-modeled star scaled in size and/or amplitude by reduction factor r ∈ [0, 1]. Star color preserved from the unclipped core annulus. Avoids the classic Minimum-filter artifacts (dark halos, broken star edges, eroded nebulosity).
- **Method B — morphological (fallback):** masked erosion, N iterations. Fast, and matches the look photographers already know.
- Parameters: strength r; size-vs-brightness balance; tier selection (e.g. reduce small/medium only, keep hero stars); protect-saturated toggle; protect-nebulosity option (structure-aware exclusion, tuned on the real corpus).
- **Acceptance:** pixels outside the dilated star mask are bit-identical to input; star count unchanged (no star deleted unless r = 0); achieved median FWHM reduction within ±10 % of requested; golden-diff shows no ringing/dark halos; deterministic across runs.

### FR-6 Star enhancement (提亮星点)

- **Selection:** top-N by flux, flux percentile, or explicit tier. (P3 option: plate-solve + catalog to target named constellation stars — see §9.)
- **Effects (independently toggleable):** soft glow (Gaussian bloom, screen-composite in linear light); core luminosity lift with soft-knee (no new clipping when "protect highlights" is on); star-color recovery on clipped cores (hue taken from the unclipped annulus); diffusion-filter emulation preset (柔焦/Softon-style, radius scaled by star flux); synthetic diffraction spikes (off by default).
- **Acceptance:** with highlight protection on, zero newly clipped pixels; effects confined to selected stars' dilated masks; deterministic.

### FR-7 Starless / stars-only decomposition

- Reduction path at r = 0 yields the starless layer; stars-only = input − starless (linear space). Both exportable as 16-bit TIFF for PS compositing or third-party use.
- **Acceptance:** starless + stars-only recomposition RMS error ≤ 1 LSB at 16-bit.

### FR-8 Batch processing and presets

- CLI: process a folder against a versioned preset (JSON schema); stable exit codes; per-image JSON report (star count, parameters, timings, warnings).
- **Acceptance:** 100-image batch runs unattended; re-run produces hash-identical outputs; a failed image never aborts the batch (reported, skipped).

### FR-9 GUI (Phase 2)

Before/after slider · 100 % zoom + pan · colored mask overlay · parameter sliders with live proxy preview (≤ 200 ms per update on a 4K proxy) · include/exclude brush feeding FR-3 · batch queue · presets UI · 中文 + English.

## 6. Non-functional requirements

- **Performance:** full pipeline (detect + reduce + enhance) on a 61 MP 16-bit TIFF ≤ 10 s target / 20 s hard cap on an 8-core desktop CPU. Baseline measured by the Phase 1 benchmark harness and locked as a regression budget.
- **Memory:** ≤ 2 × decoded image size + O(catalog).
- **Determinism:** no wall-clock or thread-order dependence in output; fixed seeds where randomness exists.
- **Reliability:** input files are never modified; crash mid-run leaves no partial output (temp + atomic rename).
- **Portability:** Windows 10+ and macOS 13+ as first-class targets (pending Q1); Linux kept green in CI.
- **Licensing:** permissive deps only (MIT/Apache/BSD). **No StarNet weights in v1** — their license is non-commercial and provenance-restricted; ML-based starless is deferred to Phase 3 with a self-trained or properly licensed model.

## 7. Architecture and stack (proposal)

- **Rust workspace:** `starkit-core` (pure algorithms, I/O-free where possible) · `starkit-io` (TIFF/ICC/EXIF) · `starkit-cli` · `starkit-gui` (Phase 2; egui vs. Tauri decided at the Phase 1 exit gate on preview latency, packaging weight, and CJK font rendering).
- **Parallelism:** rayon tile pipeline; SIMD hot paths only after profiling; GPU (wgpu) is a Phase 3 question.
- **Python oracle (`oracle/`):** photutils (DAOStarFinder) + scipy reference implementations to cross-validate detection, centroiding, and FWHM on golden fixtures — same pattern as NRS.
- **Golden fixtures:** committed synthetic star-field generator (Moffat/Gaussian PSFs, realistic noise model, saturation, close pairs, synthetic foreground silhouettes) with expected catalogs and expected output hashes.

## 8. Test strategy and definition of done

Unit + property tests per module · golden-image diffs with zero tolerance outside masks · oracle cross-checks · determinism tests (double-run hash equality) · performance harness with recorded baselines · **photographer-in-the-loop QA:** a structured scoring sheet per phase, plus a benchmark of 5 images where she compares tool output against her own manual PS result (pass = tool rated equal-or-better on time-quality tradeoff for ≥ 4 / 5).

Definition of done per phase = all acceptance criteria green + benchmarks within budget + QA sheet signed. No green test, no done.

## 9. Phased roadmap

| Phase | Gate | Scope | Exit criteria |
|---|---|---|---|
| **0 — Spec freeze** | G0 | Answer §10; freeze v1 scope; build fixture generator + Python oracle; collect real corpus | Open questions closed; fixtures + oracle green |
| **1 — MVP (CLI)** | G1 | FR-1, FR-2, FR-3 (manual-mask path), FR-4, FR-5 core, FR-8 minimal | All Phase-1 AC green; photographer gets usable masks + reduction on her own images |
| **2 — Full pipeline + GUI** | G2 | FR-3 auto segmentation, FR-5 complete, FR-6, FR-7, FR-9 | AC green; QA sheet signed; GUI framework decision executed |
| **3 — Optional/advanced** | G3 | Plate-solve constellation mode; ML starless (licensed/self-trained); wgpu acceleration; star-shape repair for edge coma; PS plugin bridge | Individually gated; each item gets its own mini-PRD |

## 10. Open questions — blocking Phase 0 exit

- **Q1 (critical):** Photographer's OS and preferred form factor — standalone GUI, CLI batch, or is a Photoshop-plugin path the real priority? This decides the Phase 2 shape.
- **Q2 (critical):** Can she supply ≥ 20 representative 16-bit TIFFs as the test corpus, including hard cases (trees on the horizon, dense Milky Way core, strong nebulosity, heavily saturated bright stars)? What is her typical nightscape : deep-sky ratio?
- **Q3:** Is a full starless output an artistic deliverable she actually wants, or only an internal step? (Affects FR-7 polish priority.)
- **Q4:** Enhancement taste — does she use diffraction spikes / heavy glow, or subtle-only? One or two reference images of her ideal "after" would anchor FR-6 defaults.
- **Q5:** Distribution — private tool for her, or possible public release later? (Affects licensing rigor, code signing/notarization, UI polish budget.)
- **Q6:** GPU on her machine? (Only matters for Phase 3 planning.)

## 11. Risks and mitigations

- **Auto sky segmentation on complex horizons** (tree branches against sky) is genuinely hard → manual mask import ships in Phase 1 as the guaranteed path; auto mode is a convenience layered on top, never a dependency.
- **Nebulosity mistaken for stars** in deep-sky frames → tier thresholds + protect-nebulosity option, validated against the real corpus before G1 closes.
- **"Good" is partly aesthetic** → photographer-in-the-loop QA sheet each phase and the 5-image manual-vs-tool benchmark keep the target honest.
- **Scope creep toward a general astro editor** → §3 non-goals are contractual; anything new gets its own gate.

## 12. Next steps upon approval

1. Close §10 (Q1/Q2 minimum) → Phase 0 gate.
2. Produce Claude Code handoff: repo scaffold, layered `CLAUDE.md` invariants (mask-gating invariant, determinism, atomic writes), `ROADMAP.md` with per-phase AC, fixture-generator spec.
3. Begin Phase 0 implementation (fixtures + oracle) — no product code before fixtures exist.

---
*v1.0 — approved 2026-07-16. Project name confirmed: StarKit.*
