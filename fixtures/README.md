# fixtures/

- `expected/` — committed truth catalogs, generator params, comparison reports, and `MANIFEST.sha256`.
- `generated/` — regenerable fixture images (gitignored). Regenerate with:
  `cargo run -p starkit-fixtures -- gen --suite <name> --seed <n> --out fixtures/generated`
  (available after task T0-1).

Spec: ../docs/FIXTURES.md
