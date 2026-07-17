#!/usr/bin/env python3
"""Run the oracle over every committed suite and write the reports (task T0-2).

    python run_suites.py [--generated DIR] [--reports DIR]

Thin driver over measure.py + compare.py so the committed reports under
`fixtures/expected/reports/` can be reproduced with one command, and so ci.sh
(T0-4) has a single entry point. All the real work — and all the interface
contract — lives in the two scripts it calls.
"""

from __future__ import annotations

import argparse
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from compare import compare  # noqa: E402
from measure import measure  # noqa: E402
from _common import read_json, write_json  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parent.parent

# Per-suite measurement inputs.
#
# `fwhm_guess` is the suite's own fwhm_mean_px: the oracle is entitled to know
# the seeing it is measuring, exactly as an astronomer reads it off the frame.
# It is not entitled to know where the stars are, and it does not.
#
# `sky_mask` is passed only for the nightscape, where it is not a convenience but
# a correctness requirement (D-019): DAOFIND's zero-sum kernel reads the sky
# beside a tree silhouette as a star.
SUITES = [
    {"name": "basic-5k", "fwhm_guess": 3.2, "sky_mask": False},
    {"name": "dense-core", "fwhm_guess": 3.2, "sky_mask": False},
    {"name": "saturated", "fwhm_guess": 3.2, "sky_mask": False},
    {"name": "pairs", "fwhm_guess": 3.2, "sky_mask": False},
    {"name": "nightscape-fg", "fwhm_guess": 3.2, "sky_mask": True},
]


def main() -> None:
    ap = argparse.ArgumentParser(description="Run the oracle over every committed suite")
    ap.add_argument("--generated", default=str(ROOT / "fixtures" / "generated"))
    ap.add_argument("--reports", default=str(ROOT / "fixtures" / "expected" / "reports"))
    ap.add_argument(
        "--catalogs",
        default=None,
        help="optional dir to keep the measured catalogs (they are large and not committed)",
    )
    args = ap.parse_args()

    generated = pathlib.Path(args.generated)
    reports = pathlib.Path(args.reports)

    failures = []
    for suite in SUITES:
        name = suite["name"]
        image = generated / name / "image.tiff"
        truth_path = generated / name / "truth.json"
        if not image.exists():
            failures.append(f"{name}: missing {image}")
            continue

        mask = str(generated / name / "sky_mask.png") if suite["sky_mask"] else None
        measured = measure(str(image), suite["fwhm_guess"], 5.0, mask)
        if args.catalogs:
            write_json(pathlib.Path(args.catalogs) / f"{name}.json", measured)

        report = compare(read_json(truth_path), measured)
        write_json(reports / f"{name}.json", report)

        m = report["metrics"]
        print(
            f"{name:15s} recall@snr>=5 {_pct(m['recall_at_snr_floor'])}  "
            f"precision {_pct(m['precision'])}  "
            f"centroid {m['centroid_error_px']['median']:.3f} px  "
            f"fwhm {_pct(m['fwhm_rel_error']['median'])}"
        )

    if failures:
        for f in failures:
            print(f"FAILED: {f}", file=sys.stderr)
        raise SystemExit(1)


def _pct(v: float | None) -> str:
    return "n/a" if v is None else f"{v * 100:6.2f}%"


if __name__ == "__main__":
    main()
