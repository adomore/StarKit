# oracle — rules

Independent measurement instrument. Purpose: validate the fixtures (Phase 0) and cross-check `starkit-core` detection (Phase 1 onward). Root `CLAUDE.md` applies on top.

- **Independence is the point.** Use photutils/astropy primitives (DAOStarFinder etc.). Never port Rust algorithms here and never port oracle code into Rust — if the two sides share math, the cross-check is worthless.
- **Interface contract:**
  - `python measure.py --image PATH --out catalog.json [--fwhm-guess 3.0]` → catalog in schema v1 (measured form: no `generator` block; `snr` optional). Schema: `docs/FIXTURES.md`.
  - `python compare.py --truth T.json --measured M.json --out report.json` → recall, precision, centroid-error stats (median/p95), FWHM-error stats, and a per-SNR-bin table, using the matching rule in `docs/FIXTURES.md`.
- **Determinism:** fix seeds for any stochastic step; every report records the photutils/astropy/numpy versions used.
- **Environment:** `python3 -m venv .venv && .venv/bin/pip install -r requirements.txt`. Loose pins in `requirements.txt`; exact resolved versions belong in the reports, not the pins.
- **Outputs:** comparison reports for the committed suites live under `fixtures/expected/reports/`.
