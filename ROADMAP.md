# StarKit — Roadmap

Spec: `PRD.md` v1.0. Phases are gate-locked: tasks of a phase may not start until the previous gate checklist is complete. Task IDs are stable — reference them in commits and DECISIONS entries.

**Current phase: 1** — gate **G0 closed 2026-07-16** (T0-1…T0-4 AC green via `./ci.sh`; Q1 answered D-026; Q2 deferred D-027; Q3/Q4/Q6 deferred D-028). INV-5 is discharged: product code may now be written.

---

## Phase 0 — Measurement apparatus (gate G0)

Goal: before any product code exists, build the instruments that make quality claims checkable — synthetic golden fixtures with exact known truth, and an independent Python oracle (INV-5).

- [x] **T0-1 Fixture generator** (`crates/starkit-fixtures`). Implement per `docs/FIXTURES.md`: Moffat + elliptical-Gaussian PSFs with ×8 supersampling, Poisson + Gaussian read noise, 16-bit quantization with saturation, close pairs, procedural foreground silhouettes with truth sky masks. Emits per suite: TIFF16 image, truth catalog JSON (schema v1), params JSON, SHA-256 manifest lines.
  **AC:** byte-identical output across two runs with the same seed; all five suites generate (`basic-5k`, `dense-core`, `saturated`, `pairs`, `nightscape-fg`); manifests committed under `fixtures/expected/`.
  **Done** 2026-07-16 — AC tests in `crates/starkit-fixtures/tests/acceptance.rs`: `two_runs_with_the_same_seed_are_byte_identical` + `full_suites_are_byte_identical_across_two_runs` (a); `committed_truth_catalogs_validate_against_schema_v1` (b); `regenerating_all_suites_matches_the_committed_manifest` (c). Full-scale variants are `#[ignore]`d by cost (D-011) — `cargo test --release -- --ignored`, 5m39s. Decisions: D-006 (local schema mirror) · D-007 (canonical seeds) · D-008 (deps + exact pins) · D-009 (serialization policy) · D-010 (photometric conventions) · D-011 (test split) · **D-012 (cross-platform determinism is NOT guaranteed — shapes T0-4's manifest check)**.
- [x] **T0-2 Oracle** (`oracle/measure.py`, `oracle/compare.py`). photutils-based detection/measurement on fixture images; comparison vs truth producing recall / precision / centroid-error / FWHM-error statistics using the matching rule in `docs/FIXTURES.md`.
  **AC:** runs on all suites; on `basic-5k` the oracle itself achieves recall ≥ 98 % @ peak SNR ≥ 5 and precision ≥ 99 % — this proves the fixtures are solvable and calibrates the bar `starkit-core` must meet in Phase 1; report JSONs committed under `fixtures/expected/reports/`.
  **Done** 2026-07-16. All five suites measured; reports committed. `basic-5k`: **recall 99.21 %** @ snr ≥ 5, **precision 99.87 %**, centroid median 0.056 px — fixtures are solvable, and this is the bar Phase 1 must meet. AC encoded in `oracle/test_oracle.py` (23 tests, asserts the committed reports; run `oracle/.venv/bin/python -m pytest oracle -q`). Reproduce reports: `python oracle/run_suites.py`. Decisions: D-017 (truth `snr` is per-channel — the measured mean-of-RGB plane has √3 better SNR, so this bar is weaker than it reads), D-018 (FWHM by Gaussian2D fit, +2.3 % / +14.2 % characterised bias), D-019 (silhouette exclusion — DAOFIND's zero-sum kernel read sky beside trees as stars, 59 % → 99.93 % precision on `nightscape-fg`), D-020 (precision definition). Other suites, for reference and by design not AC: `dense-core` 88.10 % recall (crowding), `pairs` 65.84 % (the deblending limit is the point), `saturated` 96.60 %, `nightscape-fg` 91.63 %.
- [x] **T0-3 Catalog schema v1 freeze.** `starkit_core::types` serde types ⇄ `docs/FIXTURES.md` schema; JSON round-trip test; fixtures and oracle both emit/consume the same schema.
  **AC:** round-trip test green; schema section in FIXTURES.md flipped from DRAFT to FROZEN; any change from the draft logged in DECISIONS.md.
  **Done** 2026-07-16. Round-trip is **byte-identical** across all five committed truth catalogs (`crates/starkit-core/tests/schema_v1.rs`, 9 tests) — which also discharges the D-006 debt: those bytes were written by `starkit-fixtures`' independent mirror, so reproducing them exactly through `starkit-core::types` proves the two definitions agree without the crates sharing a type. Verified to have teeth by mutation (renaming one field fails the test). Oracle side checked from Python (`oracle/test_oracle.py`, 6 schema tests → 29 total). FIXTURES.md flipped to FROZEN. Changes from draft: **one** — optional `measurement` block (D-023, owner-approved). Also uncovered D-022: serde_json's default float parser is off by 1 ULP on ~5.7 % of committed values, silently breaking D-009's round-trip claim; fixed via the `float_roundtrip` + `preserve_order` features.
- [x] **T0-4 Local CI script** `ci.sh`: fmt check, clippy `-D warnings`, workspace tests, fixture determinism smoke (regenerate one suite, verify manifest hashes).
  **AC:** single command; non-zero exit on any failure.
  **Done** 2026-07-16. `./ci.sh` runs fmt · clippy · 52 Rust tests · 29 oracle tests · fixture smoke (regenerates `basic-5k`, verifies its 3 artifacts against the committed manifest) in ~25 s. `--full` adds the D-011 `--ignored` full-scale AC tests (~6 min); `--quick` skips the smoke. **AC verified by failure injection, not just the happy path:** fmt violation, failing test, and a corrupted manifest each exit 1 with the offending output; restoring exits 0. Output is captured and shown only on failure. Decisions: D-024 (manifest mismatch fails loudly on every host and names D-012 rather than skipping off-platform — the cross-platform mechanism stays unchosen until a second host produces evidence), D-025 (a missing oracle venv is a failure, never a silent skip).

**Gate G0 checklist**
- [x] T0-1 … T0-4 all AC green — verified by `./ci.sh` (2026-07-16)
- [x] Q1 answered: **Windows + standalone GUI** → D-026
- [x] Q2 corpus: **explicitly deferred** → D-027. Phase 1 proceeds on synthetic fixtures; **G1 cannot close without the corpus** (FR-2 real-image AC, FR-4 sign-off, T1-10 all blocked).
- [x] Q3–Q6: Q5 answered (D-014); Q3/Q4/Q6 deferred to the phase that needs them (D-028)

---

## Phase 1 — MVP CLI (gate G1) — FR-1, FR-2, FR-3 (manual path), FR-4, FR-5 core, FR-8 minimal

- [x] **T1-1 `starkit-io`:** decode TIFF 8/16 + PNG + JPEG → linear f32 RGB; capture ICC bytes + EXIF; encode TIFF16 restoring both; atomic write path (the only write path). ICC backend decision → D-005.
  **AC = FR-1:** no-op run pixel-identical for TIFF16; truncated/corrupt input fails cleanly with no partial file; atomic-semantics test (kill between temp write and rename leaves target untouched).
  **Done** 2026-07-16. 46 tests (33 unit + 13 FR-1 acceptance). **No-op round-trip is pixel-identical on the real 4096² `basic-5k` fixture** — all 50.3M samples — not only on a toy image. Corrupt/truncated/non-image inputs each fail with the right typed variant and create no output file. Atomic path: temp beside target → fsync → rename, with failure-injection tests proving a failed write leaves the previous target byte-identical and leaves no debris; INV-3 checked by hashing the input before and after. Decisions: **D-029 resolves D-005 — no ICC backend at all** (working space is linear light in the source's *own* primaries; converting to a canonical space would clip wide-gamut data and break pixel-identity), curve round-trip proven exhaustively over all 65536 codes; **D-030** EXIF is preserved TIFF→TIFF (sub-IFD resolved and rewritten with correct offsets) but **dropped for JPEG/PNG→TIFF**, and GPS/Interop/MakerNote are never copied.
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
