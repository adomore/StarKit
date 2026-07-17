# StarKit Fixtures — Generator Spec and Catalog Schema

Purpose: golden synthetic star fields with exact known truth, so detection and reduction quality is **measured, not eyeballed**. Ground truth = the generation parameters themselves; the Python oracle provides an *independent* measurement of the same images.

**Non-circularity rule:** `starkit-fixtures` must not depend on `starkit-core`, and writes TIFF via the `tiff` crate directly (not `starkit-io`). Truth comes from generator inputs; the oracle (photutils) is a second, algorithm-independent instrument. `starkit-core` is never validated against its own math.

## Rendering model

- Working space: linear flux units, rendered per-channel RGB. Each star carries a subtle blackbody-ish tint parameter (star color realism for the color-recovery features).
- PSF: Moffat(fwhm, β) with β ∈ [2.5, 4.5] (default) and elliptical Gaussian (ellipticity ≤ 0.15, angle θ); per-suite mix.
- Sampling: analytic profile integrated by ×8 supersampling per axis, then box-downsample; star centers at true subpixel positions.
- Background: constant level plus an optional small linear gradient.
- Noise: Poisson (shot) on signal + background, plus Gaussian read noise σ_r. Seeded `rand_chacha` only.
- Quantization: gain scale → clamp to [0, 65535] (saturation) → u16. Output written as 16-bit RGB TIFF.

## Star population

- Flux: power-law distribution (bright tail), range per suite; **peak SNR is computed and stored per star** — quality metrics are conditioned on it.
  - **`snr` is PER-CHANNEL peak SNR** (D-017): `peak / sqrt(peak + background + read_noise²)`, in electrons. The mean-of-RGB plane that consumers actually measure (D-010) averages three channels and so has **√3 better SNR**: a star stored as `snr = 5` sits at ≈ 8.7 σ in that plane. Every SNR-conditioned bar in this repo — including FR-2's "recall ≥ 98 % at SNR ≥ 5" — is stated in these per-channel units and is therefore a weaker claim than it first reads. Do not silently switch definitions: `snr ≥ 5` per-channel and `snr ≥ 8.66` mean-of-RGB select exactly the same stars, but conditioning the existing bar on the mean-of-RGB value makes it unachievable (measured: the oracle tops out at 97.19 % recall, at 25.5 % precision).
- FWHM: normal around a per-suite mean (e.g. 3.2 px ± 0.5), floor 1.6 px.
- Positions: uniform with a minimum-separation constraint, except in `pairs`.
- Tier (truth): flux tertiles within the suite → large / medium / small. Detection may use its own tier thresholds; evaluation matches stars by position, never by tier equality.

## Suites (v1)

| suite | size | stars | notes |
|---|---|---|---|
| `basic-5k` | 4096² | 5,000 | clean field, peak SNR 3–200; primary metrics suite |
| `dense-core` | 4096² | 25,000 | Milky-Way-core density; deblending stress test |
| `saturated` | 2048² | 500 | ~10 % saturated, with halo/bleed approximation |
| `pairs` | 2048² | 2,000 | pair separations 0.5–2.0 × FWHM |
| `nightscape-fg` | 6144×4096 | 8,000 (sky region only) | procedural ridgeline + branch silhouettes at near-zero luminance with texture; truth sky mask emitted |

A 61 MP performance variant `basic-61mp` (9568×6376, `basic-5k` parameters scaled) exists for the T1-9 bench only; it is generated on demand and never hash-pinned.

## Outputs per fixture

`fixtures/generated/{suite}/image.tiff` (16-bit RGB) · `fixtures/generated/{suite}/truth.json` (catalog schema v1 including a `generator` params block + seed) · `fixtures/generated/nightscape-fg/sky_mask.png` (8-bit, that suite only) · one line per artifact in `fixtures/expected/MANIFEST.sha256`.

Images are regenerable and gitignored; truth catalogs, params, comparison reports, and the manifest are **committed** under `fixtures/expected/`. `ci.sh` regenerates at least one suite and verifies hashes against the manifest.

## Determinism

A single u64 seed per suite fully determines the output bytes. Two runs must be byte-identical (T0-1 AC). No `HashMap` iteration order may reach any output path — use `BTreeMap` or sorted vectors throughout.

## Matching rule (used by oracle and by core evaluation)

A detected star matches a truth star if within radius `max(1.5 px, 0.5 × truth_fwhm)`; greedy matching in descending truth flux; each truth star matches at most once. Recall and precision are computed over truth stars with peak SNR ≥ 5 unless a table states otherwise. Centroid/FWHM statistics are computed over matched pairs only.

## Catalog schema v1 — **DRAFT** (frozen at task T0-3)

```json
{
  "schema": "starkit-catalog/1",
  "image": { "width": 4096, "height": 4096, "bit_depth": 16, "color_space": "linear-rgb" },
  "stars": [
    {
      "id": 1, "x": 123.45, "y": 67.89,
      "flux": 15000.0, "peak": 3200.0,
      "fwhm": 3.1, "ellipticity": 0.04, "theta": 0.6,
      "saturated": false, "tier": "small", "snr": 41.2
    }
  ],
  "generator": { "note": "present only in truth catalogs: full params + seed" }
}
```

- Coordinates (frozen project-wide): `(0.0, 0.0)` = center of the top-left pixel; x → right, y → down. `theta` in radians, CCW from +x.
- `flux` = total star flux, `peak` = peak pixel value, both in linear units. `fwhm` = geometric mean of the two axes, pixels. `ellipticity` = 1 − b/a.
- `snr` (peak SNR) is required in truth catalogs, optional in measured/detected catalogs. **Per-channel** — see "Star population" above and D-017.
- `generator` is present only in truth catalogs written by `starkit-fixtures`.
- Rust source of truth for the types: `crates/starkit-core/src/types.rs` (kept in lockstep with this section; divergence is a bug).
