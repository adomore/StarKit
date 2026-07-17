# StarKit — Roadmap

Spec: `PRD.md` v1.0. Phases are gate-locked: tasks of a phase may not start until the previous gate checklist is complete. Task IDs are stable — reference them in commits and DECISIONS entries.

**Current phase: 0**

---

## Phase 0 — Measurement apparatus (gate G0)

Goal: before any product code exists, build the instruments that make quality claims checkable — synthetic golden fixtures with exact known truth, and an independent Python oracle (INV-5).

- [x] **T0-1 Fixture generator** (`crates/starkit-fixtures`). Implement per `docs/FIXTURES.md`: Moffat + elliptical-Gaussian PSFs with ×8 supersampling, Poisson + Gaussian read noise, 16-bit quantization with saturation, close pairs, procedural foreground silhouettes with truth sky masks. Emits per suite: TIFF16 image, truth catalog JSON (schema v1), params JSON, SHA-256 manifest lines.
  **AC:** byte-identical output across two runs with the same seed; all five suites generate (`basic-5k`, `dense-core`, `saturated`, `pairs`, `nightscape-fg`); manifests committed under `fixtures/expected/`.
  **Done** 2026-07-16 — AC tests in `crates/starkit-fixtures/tests/acceptance.rs`: `two_runs_with_the_same_seed_are_byte_identical` + `full_suites_are_byte_identical_across_two_runs` (a); `committed_truth_catalogs_validate_against_schema_v1` (b); `regenerating_all_suites_matches_the_committed_manifest` (c). Full-scale variants are `#[ignore]`d by cost (D-011) — `cargo test --release -- --ignored`, 5m39s. Decisions: D-006 (local schema mirror) · D-007 (canonical seeds) · D-008 (deps + exact pins) · D-009 (serialization policy) · D-010 (photometric conventions) · D-011 (test split) · **D-012 (cross-platform determinism is NOT guaranteed — shapes T0-4's manifest check)**.
- [ ] **T0-2 Oracle** (`oracle/measure.py`, `oracle/compare.py`). photutils-based detection/measurement on fixture images; comparison vs truth producing recall / precision / centroid-error / FWHM-error statistics using the matching rule in `docs/FIXTURES.md`.
  **AC:** runs on all suites; on `basic-5k` the oracle itself achieves recall ≥ 98 % @ peak SNR ≥ 5 and precision ≥ 99 % — this proves the fixtures are solvable and calibrates the bar `starkit-core` must meet in Phase 1; report JSONs committed under `fixtures/expected/reports/`.
- [ ] **T0-3 Catalog schema v1 freeze.** `starkit_core::types` serde types ⇄ `docs/FIXTURES.md` schema; JSON round-trip test; fixtures and oracle both emit/consume the same schema.
  **AC:** round-trip test green; schema section in FIXTURES.md flipped from DRAFT to FROZEN; any change from the draft logged in DECISIONS.md.
- [ ] **T0-4 Local CI script** `ci.sh`: fmt check, clippy `-D warnings`, workspace tests, fixture determinism smoke (regenerate one suite, verify manifest hashes).
  **AC:** single command; non-zero exit on any failure.

**Gate G0 checklist**
- [ ] T0-1 … T0-4 all AC green
- [ ] Q1 answered (photographer's OS + form factor) → DECISIONS.md
- [ ] Q2 corpus: ≥ 20 real 16-bit TIFFs collected incl. hard cases (or explicitly deferred with a decision note — Phase 1 real-image AC will block on it)
- [ ] Q3–Q6 answered or deferred with decision notes

---

## Phase 1 — MVP CLI (gate G1) — FR-1, FR-2, FR-3 (manual path), FR-4, FR-5 core, FR-8 minimal

- [ ] **T1-1 `starkit-io`:** decode TIFF 8/16 + PNG + JPEG → linear f32 RGB; capture ICC bytes + EXIF; encode TIFF16 restoring both; atomic write path (the only write path). ICC backend decision → D-005.
  **AC = FR-1:** no-op run pixel-identical for TIFF16; truncated/corrupt input fails cleanly with no partial file; atomic-semantics test (kill between temp write and rename leaves target untouched).
- [ ] **T1-2 `starkit-core::background`:** tiled sigma-clipped background + noise map.
  **AC:** background RMS error within the tolerance documented at the test site on `basic-5k`; deterministic.
- [ ] **T1-3 `starkit-core::detect`:** threshold + local maxima, subpixel centroid, elliptical FWHM, deblending, saturation flags, tiering.
  **AC = FR-2 synthetic:** recall ≥ 98 % @ SNR ≥ 5, precision ≥ 99 %, median centroid error ≤ 0.3 px, median FWHM error ≤ 10 % on all applicable suites; **plus** oracle cross-check report (Δrecall ≤ 0.5 pp vs oracle on `basic-5k`; deviations beyond that require a DECISIONS entry).
- [ ] **T1-4 `starkit-core::mask` + export:** tiered feathered masks (radius = k × FWHM), saturated-halo extension; 16-bit grayscale export via `starkit-io`.
  **AC = FR-4:** golden mask hashes stable; pixel-alignment test vs source dimensions.
- [ ] **T1-5 Sky-mask ingestion + gating:** import external grayscale mask; enforce INV-1 in the single compositor.
  **AC = FR-3 manual path:** invariant test proves pixels outside the sky mask are bit-identical, including on `nightscape-fg`.
- [ ] **T1-6 Reduction B (morphological):** masked erosion, N iterations.
  **AC:** outside-mask bit-identity; determinism; golden visual fixture.
- [ ] **T1-7 Reduction A (resynthesis, default):** per star — local background (annulus + inpaint), PSF-scaled re-add at factor r, color from the unclipped core annulus, saturated-star protection.
  **AC = FR-5 full:** achieved median FWHM reduction within ±10 % of requested; star count preserved for r > 0; no ringing/dark halos on golden diffs; outside-mask bit-identity; deterministic.
- [ ] **T1-8 `starkit-cli`:** `process` (versioned preset JSON), `inspect` (catalog dump), folder batch with per-image JSON report, stable exit-code table, skip-on-error.
  **AC = FR-8:** 100-image batch unattended; re-run hash-identical; one poisoned file is reported and skipped without aborting the batch.
- [ ] **T1-9 Bench harness + baselines:** synthetic 61 MP variant; record full-pipeline timing (target ≤ 10 s, hard cap 20 s on 8 cores).
  **AC:** baseline JSON committed; regression check wired into `ci.sh`.
- [ ] **T1-10 Real-corpus QA round:** masks + reduction on the photographer's images; structured QA sheet stored under `docs/qa/` (create on first use).

**Gate G1 checklist:** all Phase 1 AC green · bench within budget · FR-4 photographer sign-off (exported masks usable as-is in her PS workflow on ≥ 5 of her images) · QA sheet filed.

---

## Phase 2 — Full pipeline + GUI (gate G2)

FR-3 auto sky segmentation · FR-5 complete (tier-selective reduction, nebulosity protection tuned on the real corpus) · FR-6 enhancement (glow, soft-knee lift, color recovery, 柔焦 preset, optional spikes) · FR-7 starless/stars-only decomposition · FR-9 GUI. GUI framework (egui vs Tauri) is decided at G1 exit on three criteria: proxy-preview latency ≤ 200 ms on a 4K proxy, packaging weight, CJK rendering — record as a DECISIONS entry. Detailed task breakdown is authored at G1 exit, not before.

## Phase 3 — Optional / advanced (gate G3)

Plate-solve constellation mode · ML starless (self-trained or license-clean) · wgpu acceleration · star-shape repair (edge coma) · PS plugin bridge. Each item requires its own mini-PRD before work starts.
