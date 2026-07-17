# fixtures/

- `expected/` — committed truth catalogs, generator params, comparison reports, and `MANIFEST.sha256`.
- `generated/` — regenerable fixture images (gitignored). Regenerate all five suites with:
  `cargo run --release -p starkit-fixtures -- gen --suite all --out fixtures/generated`
  (~6 min; `--release` matters — a debug build takes far longer.)

`--seed` is optional and defaults to the suite's canonical seed (D-007). It may only be
given for a single suite: `--suite all` means "the five committed suites at their canonical
seeds", and an override there would produce artifacts that silently miss the manifest.

`MANIFEST.sha256` paths are relative to the output root, so `fixtures/expected/<suite>/…`
and `fixtures/generated/<suite>/…` share one set of hashes. Verify a regeneration with:
`cargo test --release -- --ignored`

Committed vs regenerable follows ../docs/FIXTURES.md: `truth.json` and `params.json` are
committed per suite; the TIFF images and `nightscape-fg/sky_mask.png` are images, so they
are gitignored and reproduced from the seed. All of them are hash-pinned in the manifest.

Spec: ../docs/FIXTURES.md
