# starkit-core — crate rules

Pure algorithms. This crate is where INV-2 (determinism), INV-4 (linear light), and INV-7 (purity) bind hardest. Root `CLAUDE.md` applies on top of everything here.

- **Purity:** in `src/`, the following are forbidden: `std::fs`, `std::net`, `std::time`, `std::env`, `println!`/`eprintln!`. If logging is ever needed, raise a DECISIONS entry first. Integration tests under `tests/` may read `fixtures/`.
- **RNG:** none in production paths unless a seed is an explicit function parameter; `rand_chacha` only.
- **Parallelism:** rayon is allowed; results must be order-independent. Approved patterns: `par_iter().map(...).collect()` into index-keyed storage; per-tile buffers merged in a fixed order. Never fold floating-point values in nondeterministic order.
- **Numerics:** pixel buffers `f32`; statistics and accumulators `f64`. Sigma-clip and any statistical estimator documents its exact algorithm at the definition site.
- **Errors:** `thiserror`. No panics on data-driven paths; internal invariant breaches use `debug_assert!` plus a typed error.
- **API shape:** operations take explicit mask arguments and never composite themselves — one compositor (module `gate`, T1-5) applies `sky_mask ∧ dilate(star_mask)` so INV-1 is enforced in exactly one place.
- **Module plan** (create each in its ROADMAP task, not before): `types` (T0-3, exists) · `background` (T1-2) · `detect` (T1-3) · `mask` (T1-4) · `gate` (T1-5) · `reduce` (T1-6/T1-7) · `enhance` (Phase 2) · `decompose` (Phase 2).
- **Golden tests:** zero tolerance outside masks, always. Any nonzero tolerance inside masks must be justified in the test's doc comment.
