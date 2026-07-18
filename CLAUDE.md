# StarKit — Project Instructions for Claude Code

Automated star processing for astrophotography: detection → sky/foreground gating → tiered star masks → parametric star reduction (缩星) and enhancement (提亮星点), with deterministic batch output that round-trips losslessly into Photoshop.

- Authoritative spec: `PRD.md` (v1.0, approved 2026-07-16). `docs/PRD-zh.md` is the Chinese mirror; English wins on conflict.
- Plan and task IDs: `ROADMAP.md`. Decision log: `docs/DECISIONS.md`. Fixture spec + catalog schema: `docs/FIXTURES.md`.

## Current state

**Phase 1. Gate G0 closed 2026-07-16** — T0-1…T0-4 AC green (`./ci.sh`), Q1 answered (D-026: Windows + standalone GUI), Q2 deferred (D-027), Q3/Q4/Q6 deferred (D-028). **INV-5 is discharged: product code may now be written.** Next task: **T1-10** (real-corpus QA) — **blocked on the real corpus (D-027)**. T1-1…T1-9 done: `starkit-io` round-trips TIFF16 pixel-identically; `background` 0.19 ADU RMS; `detect` meets all four FR-2 bars on `basic-5k` (99.94 % / 99.92 % / 0.098 px / 5.77 %); `mask` exports 16-bit tiered masks with pinned golden hashes; `gate` is INV-1's single enforcement point — proved on `nightscape-fg` at zero tolerance, D-034's debt paid (precision 35.7 % → 99.91 %); `reduce` has both methods — B (morphological) and A (resynthesis, the default): FR-5's five bars all met on `basic-5k` (FWHM +7.4 % at r=0.5, star count preserved, no dark halo, outside-gate bit-identity on `basic-5k` and `nightscape-fg`, deterministic). `starkit-cli` wires it all into `process`/`inspect`/`batch` (FR-8 met on the compiled binary: 100-image batch, hash-identical re-run, skip-on-error at exit 7); the 61 MP bench meets budget at 8.3 s compute single-threaded (T1-9, D-040). **Every star operation composites only through `gate::composite`** — a direct write into an output plane anywhere else is a review blocker. Standing debt: **G1 cannot close without the real corpus** (D-027) — FR-2's real-image AC, FR-4's photographer sign-off and T1-10 are all blocked on it. Corpus selection rules are now agreed and specified in `docs/CORPUS.md` (D-041): nightscape-heavy 70:30, ≥20 images, gitignored `corpus/` (copyrighted work, public repo), 16-bit TIFF. **The images themselves are still pending**, and FR-4 sign-off + the manual-vs-tool benchmark are deferred until the photographer supplies her own manual-PS results.

## Repository map

| path | role |
|---|---|
| `crates/starkit-core` | pure algorithms: background, detect, mask, gate, reduce, enhance, decompose. I/O-free. |
| `crates/starkit-io` | the only crate that touches files/codecs: TIFF/PNG/JPEG, ICC/EXIF, linearization, atomic writes. |
| `crates/starkit-cli` | CLI (`starkit`), presets, batch, per-image JSON reports, stable exit codes. |
| `crates/starkit-fixtures` | synthetic golden star-field generator. Independent code path — must **not** depend on `starkit-core` (see `docs/FIXTURES.md`). |
| `oracle/` | Python (photutils) independent measurement + comparison reports. Never shares code with the Rust side. |
| `fixtures/expected/` | committed truth catalogs, params, reports, `MANIFEST.sha256`. |
| `fixtures/generated/` | regenerable fixture images (gitignored). |

## Hard invariants

Every invariant is a release blocker. Each must be covered by at least one test as soon as the relevant code exists.

- **INV-1 Mask gating.** All star operations are confined to `sky_mask ∧ dilate(star_mask)`. Pixels outside are **bit-identical** to input. Enforce structurally (operations receive masks; a single compositor applies them) and verify with golden diffs at zero tolerance.
- **INV-2 Determinism.** Same input + same params ⇒ bit-identical output. No wall clock, no thread-order dependence; RNG only via explicit seeds (`rand_chacha`). Parallel code must be order-independent (indexed writes / map-collect; never fold floats in nondeterministic order).
- **INV-3 Input safety.** Input files are never modified. All writes go through `starkit-io`'s single atomic path: temp file in the target directory → fsync → rename. A crash never leaves a partial output.
- **INV-4 Linear light.** Photometric math runs in linear working space; gamma/ICC conversion happens only at `starkit-io` boundaries.
- **INV-5 Fixtures first.** No product code before the fixture generator and oracle pass their Phase 0 acceptance (ROADMAP gate G0).
- **INV-6 Licensing.** Permissive dependencies only (MIT / Apache-2.0 / BSD / Zlib / ISC). No StarNet weights or derivatives. Check the license every time a dependency is added.
- **INV-7 Core purity.** `starkit-core/src` uses no filesystem, network, clock, or env. Integration tests under `tests/` may read `fixtures/`.

## Workflow discipline

1. Work strictly by ROADMAP task IDs (T0-1, T1-3, …). Do not start a task from a later phase before the current gate checklist is fully checked.
2. Every task lands with its acceptance criteria encoded as tests where testable; anything not testable gets an explicit completion note next to the ROADMAP checkbox.
3. Tick the ROADMAP checkbox in the same change that completes the task. Record every nontrivial choice in `docs/DECISIONS.md` (next D-number, context + decision + alternatives).
4. Detection-quality claims are only valid with an oracle cross-check report attached (Phase 1 onward).
5. **No green test, no done.**

## Commands

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
# full-scale fixture AC tests: regenerate all five suites, check two-run
# byte-identity and the committed manifest. Slow (~6 min) — gated (D-011).
cargo test --release -- --ignored
# regenerate fixtures (--seed defaults to the suite's canonical seed, D-007;
# `--suite all` = the five committed suites, and refuses a --seed override):
cargo run --release -p starkit-fixtures -- gen --suite all --out fixtures/generated
# oracle setup (T0-2). Interpreter path is POSIX here; on Windows it is
# oracle/.venv/Scripts/python.exe (D-021).
python3 -m venv oracle/.venv
oracle/.venv/bin/python -m pip install -r oracle/requirements.txt
# oracle tests (fast: asserts the committed reports, needs no fixture images):
oracle/.venv/bin/python -m pytest oracle -q
# regenerate the committed oracle reports (needs fixtures/generated, ~2 min):
oracle/.venv/bin/python oracle/run_suites.py
# single-command local CI: fmt, clippy, rust tests, oracle tests, fixture
# smoke (~25 s). --full adds the ~6 min --ignored AC tests; --quick skips the
# smoke. Non-zero exit on any failure.
./ci.sh
```

## Code conventions

- Rust edition per workspace `Cargo.toml` (see D-004); rustfmt defaults; clippy clean at `-D warnings`.
- Errors: `thiserror` in library crates, `anyhow` only in binaries. No `unwrap`/`expect` on data-driven paths in `starkit-core`/`starkit-io` (tests exempt). Internal invariant breaches: `debug_assert!` plus a typed error.
- Numerics: pixel buffers `f32`; statistics and accumulators `f64`. Every nonzero test tolerance is documented at the test site.
- Coordinates (frozen): `(0.0, 0.0)` is the **center of the top-left pixel**; x → right, y → down. See `docs/FIXTURES.md`.
- All code, comments, commits, and docs in English. **Exception (D-016): reader-facing docs may ship a Chinese mirror** — `README.md` (中文, the GitHub landing page) ⇄ `README.en.md`, and `PRD.md` ⇄ `docs/PRD-zh.md`. English remains authoritative on conflict. Everything else — code, comments, commit messages, ROADMAP, DECISIONS, FIXTURES — stays English-only.

## Definition of Done — every task

tests green (including new AC tests) · clippy + fmt clean · invariants unaffected (or their tests deliberately updated) · ROADMAP checkbox ticked · DECISIONS.md updated if a choice was made.
