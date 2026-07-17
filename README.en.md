# StarKit

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Phase](https://img.shields.io/badge/phase-0%20%C2%B7%20measurement%20apparatus-blue)](ROADMAP.md)
[![Rust](https://img.shields.io/badge/rust-2021%20edition-orange.svg)](Cargo.toml)
[![Tests](https://img.shields.io/badge/tests-81%20passing-brightgreen)](#testing)

**English** · [中文](README.md)

**Automated star processing for astrophotography.** Star detection → sky/foreground gating → tiered star masks → parametric star reduction (缩星) and enhancement (提亮星点), with deterministic batch output that round-trips losslessly into Photoshop.

![A synthetic nightscape fixture beside its truth sky mask](docs/images/nightscape.png)

<sup>The `nightscape-fg` golden fixture (left, asinh-stretched for display) and its truth sky mask (right). Star operations are confined to the white region — structurally, not by convention. Both are generated from one u64 seed.</sup>

---

## The problem

Two operations dominate nightscape/deep-sky post-processing time, and both have the same root cause.

**Star reduction (缩星).** After stacking and stretching, stars look bloated against the nebulosity. The manual route — Photoshop Color Range → hand-refine mask → Minimum filter → repair dark halos and broken stars — costs **15–40 minutes per image** and is near-impossible to reproduce consistently across a series.

**Star enhancement (提亮星点).** Making principal stars pop: duplicate layer → Gaussian blur → Screen → hand-paint a mask → recover colour on clipped cores. Equally tedious, equally unrepeatable.

The bottleneck in both is the same thing: **producing an accurate, artifact-free star mask.** Once that exists, reduction and enhancement are cheap parametric operations. StarKit automates the mask — and everything downstream of it.

A third problem is specific to nightscape (星野) work: **an automatic star operation must never touch the foreground.** Trees, ridgelines and buildings have highlights that naive tools happily mistake for stars. In StarKit that is not a best-effort heuristic but a structural invariant (INV-1, below).

## Project status

**Phase 0 — building the measurement apparatus. There is no product code yet, and that is deliberate.**

| Task | What | State |
|---|---|---|
| **T0-1** | Synthetic golden fixture generator | ✅ **done** |
| **T0-2** | Python oracle (photutils) — independent measurement | ✅ **done** |
| **T0-3** | Catalog schema v1 freeze | ✅ **done** |
| **T0-4** | Local CI script (`ci.sh`) | ✅ **done** |

The rule (INV-5) is that **no algorithm ships before the instrument that can prove it works**. Quality claims like "98 % recall" are meaningless without golden data with exact known truth and an independent second measurement. So Phase 0 builds both, and `starkit-core` / `starkit-io` / `starkit-cli` stay empty until gate **G0** passes.

Full plan and task IDs: [ROADMAP.md](ROADMAP.md). Every non-trivial choice is logged in [docs/DECISIONS.md](docs/DECISIONS.md).

**The fixtures are proven solvable.** The independent photutils oracle, measured on `basic-5k`:

| Metric | Bar | Oracle achieves |
|---|---|---|
| Recall @ SNR ≥ 5 | ≥ 98 % | **99.21 %** |
| Precision | ≥ 99 % | **99.87 %** |
| Median centroid error | — | **0.056 px** |

That is the point of the exercise: if the reference instrument cannot find the stars, no claim about `starkit-core` finding them would mean anything. This is also the bar Phase 1 must meet.

> Read the units before quoting the number: truth `snr` is **per-channel** peak SNR, while the mean-of-RGB plane the oracle measures has √3 better SNR — a star labelled `snr = 5` sits at ≈ 8.7 σ in the data actually measured. **"Recall ≥ 98 % at SNR ≥ 5" is an easier achievement than it reads.** See [D-017](docs/DECISIONS.md).

## What exists today: golden fixtures

`starkit-fixtures` renders synthetic star fields whose truth is *exact by construction* — the truth catalog is the generator's input, not a measurement of its output. It deliberately shares no code with the algorithms it will judge (see [docs/FIXTURES.md](docs/FIXTURES.md)).

![100% crops of the four star-field suites](docs/images/suites.png)

<sup>100 % crops, asinh-stretched. Clockwise from top left: a clean field; Milky-Way-core crowding; a clipped star with its halo and bleed column; close pairs at 0.5–2.0 × FWHM.</sup>

| Suite | Size | Stars | Purpose |
|---|---|---|---|
| `basic-5k` | 4096² | 5,000 | clean field, peak SNR 3–200 · primary metrics suite |
| `dense-core` | 4096² | 25,000 | Milky-Way-core density · deblending stress test |
| `saturated` | 2048² | 500 | ~10 % clipped, with halo + bleed structure |
| `pairs` | 2048² | 2,000 | separations 0.5–2.0 × FWHM · deblending limit |
| `nightscape-fg` | 6144×4096 | 8,000 | procedural ridgeline + tree silhouettes, truth sky mask |

Rendering model: Moffat + elliptical-Gaussian PSFs integrated by ×8 supersampling · power-law flux distribution · per-star colour tint · Poisson shot noise + Gaussian read noise · 16-bit quantization with saturation.

Truth catalogs and generator params are committed under [`fixtures/expected/`](fixtures/expected); the images themselves are regenerable and gitignored, pinned by `MANIFEST.sha256`.

## Quick start

```bash
./ci.sh          # everything: fmt, clippy, 52 Rust tests, 29 oracle tests,
                 # fixture determinism smoke. ~25 s, non-zero on any failure.
./ci.sh --full   # adds the ~6 min full-scale acceptance tests
```

Regenerate the fixture images (~400 MB, ~6 min in release; `--seed` defaults to each suite's canonical seed):

```bash
cargo run --release -p starkit-fixtures -- gen --suite all --out fixtures/generated
```

Verify the full acceptance criteria — regenerates all five suites twice and checks byte-identity plus the committed manifest:

```bash
cargo test --release -- --ignored
```

## Repository layout

| Path | Role |
|---|---|
| `crates/starkit-core` | pure algorithms: background, detect, mask, gate, reduce, enhance. I/O-free. |
| `crates/starkit-io` | the only crate that touches files/codecs: TIFF/PNG/JPEG, ICC/EXIF, atomic writes. |
| `crates/starkit-cli` | CLI (`starkit`), presets, batch, per-image JSON reports. |
| `crates/starkit-fixtures` | golden star-field generator — independent code path, must not depend on `starkit-core`. |
| `oracle/` | Python (photutils) independent measurement. Never shares code with the Rust side. |
| `fixtures/expected/` | committed truth catalogs, params, reports, `MANIFEST.sha256`. |
| `tools/` | documentation tooling — e.g. `make_previews.py`, which renders the images above. |

## Invariants

These are release blockers, each covered by tests as soon as the relevant code exists.

- **INV-1 Mask gating** — all star operations are confined to `sky_mask ∧ dilate(star_mask)`. Outside pixels are **bit-identical** to input, enforced structurally by a single compositor and verified with zero-tolerance golden diffs.
- **INV-2 Determinism** — same input + same params ⇒ bit-identical output. No wall clock, no thread-order dependence; RNG only via explicit seeds.
- **INV-3 Input safety** — input files are never modified; all writes go through one atomic temp→fsync→rename path. A crash never leaves a partial output.
- **INV-4 Linear light** — photometric math runs in linear space; gamma/ICC conversion only at I/O boundaries.
- **INV-5 Fixtures first** — no product code before fixtures and oracle pass Phase 0 acceptance.
- **INV-6 Licensing** — permissive dependencies only (MIT / Apache-2.0 / BSD / Zlib / ISC). No StarNet weights or derivatives.
- **INV-7 Core purity** — `starkit-core/src` uses no filesystem, network, clock, or env.

## Testing

`cargo test --workspace` runs 43 Rust tests in about four seconds: PSF/photometry unit tests, byte-identity of every emitted artifact type, and schema + population validation of the committed truth catalogs for all five suites.

`pytest oracle` runs 23 oracle tests in about a second: the T0-2 acceptance criteria, the matching rule from `docs/FIXTURES.md`, and the metric semantics. They assert the **committed reports**, so they need neither the 400 MB of images nor a six-minute regeneration; `python oracle/run_suites.py` rebuilds those reports from the images.

The two full-scale acceptance tests regenerate all five real suites (~10⁸ Poisson draws, ~6 min in release) and are `#[ignore]`d so the default run stays fast enough that people actually run it — see [D-011](docs/DECISIONS.md). They are gated, not skipped: `cargo test --release -- --ignored`.

**Determinism scope:** byte-identity is guaranteed for the same platform and toolchain, which is what INV-2 requires. Cross-*platform* identity is not guaranteed — `rand_distr`'s samplers route transcendentals through the platform libm — so the committed manifest pins this platform's output. See [D-012](docs/DECISIONS.md); how CI handles it is decided at T0-4.

## Roadmap

| Phase | Scope | Gate |
|---|---|---|
| **0** | Golden fixtures + Python oracle + local CI | G0 |
| 1 | MVP CLI: I/O, detection, manual sky mask, star masks, reduction, batch | G1 |
| 2 | Auto sky segmentation, enhancement, starless/stars-only, GUI | G2 |
| 3 | Plate-solve constellation mode, ML starless, wgpu, PS plugin bridge | G3 |

Phases are gate-locked: a phase cannot start before the previous gate checklist is complete.

## Documentation

- [PRD.md](PRD.md) — authoritative functional specification (v1.0, approved) · [中文版](docs/PRD-zh.md)
- [ROADMAP.md](ROADMAP.md) — phases, task IDs, acceptance criteria
- [docs/FIXTURES.md](docs/FIXTURES.md) — fixture spec + catalog schema v1
- [docs/DECISIONS.md](docs/DECISIONS.md) — append-only decision log
- [CLAUDE.md](CLAUDE.md) — working rules for the implementation agent

## License

MIT — see [LICENSE](LICENSE).
