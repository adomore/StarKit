"""Acceptance and unit tests for the oracle (task T0-2).

    oracle/.venv/Scripts/python -m pytest oracle -q      # Windows
    oracle/.venv/bin/python -m pytest oracle -q          # POSIX

The T0-2 acceptance criteria are asserted against the *committed* reports under
`fixtures/expected/reports/`, so they run in seconds and need neither the 400 MB
of fixture images nor a six-minute regeneration. Reproducing those reports from
the images is `python oracle/run_suites.py`, wired into ci.sh at T0-4.
"""

from __future__ import annotations

import pathlib

import numpy as np
import pytest

from _common import REPORT_SCHEMA_V1, SCHEMA_V1, read_json
from compare import compare, match

ROOT = pathlib.Path(__file__).resolve().parent.parent
REPORTS = ROOT / "fixtures" / "expected" / "reports"

SUITES = ["basic-5k", "dense-core", "saturated", "pairs", "nightscape-fg"]


def _report(suite: str) -> dict:
    path = REPORTS / f"{suite}.json"
    if not path.exists():
        pytest.fail(f"missing committed report {path}; run `python oracle/run_suites.py`")
    return read_json(path)


# --------------------------------------------------------------------------
# T0-2 acceptance criteria
# --------------------------------------------------------------------------


@pytest.mark.parametrize("suite", SUITES)
def test_ac_oracle_runs_on_every_suite(suite: str) -> None:
    """AC: "runs on all suites"."""
    r = _report(suite)
    assert r["schema"] == REPORT_SCHEMA_V1
    assert r["suite"] == suite
    assert r["counts"]["detections"] > 0
    assert r["metrics"]["recall_at_snr_floor"] is not None


def test_ac_basic5k_recall_at_least_98_percent() -> None:
    """AC: on basic-5k the oracle itself achieves recall >= 98 % at peak SNR >= 5.

    This is the criterion that proves the fixtures are *solvable*: if the
    reference instrument cannot find the stars, no claim about starkit-core
    finding them would mean anything.
    """
    m = _report("basic-5k")["metrics"]
    assert m["snr_floor"] == 5.0
    assert m["recall_at_snr_floor"] >= 0.98, f"recall {m['recall_at_snr_floor']:.4f} < 0.98"


def test_ac_basic5k_precision_at_least_99_percent() -> None:
    """AC: on basic-5k the oracle itself achieves precision >= 99 %."""
    m = _report("basic-5k")["metrics"]
    assert m["precision"] >= 0.99, f"precision {m['precision']:.4f} < 0.99"


def test_ac_reports_record_their_toolchain() -> None:
    """oracle/CLAUDE.md: every report records the versions used.

    A quality number that cannot be traced to a toolchain is not evidence.
    """
    for suite in SUITES:
        v = _report(suite)["versions"]
        for lib in ("numpy", "scipy", "astropy", "photutils"):
            assert v.get(lib), f"{suite}: report does not pin {lib}"


def test_reports_define_every_metric_they_publish() -> None:
    """No bare number: each metric ships its definition, so it cannot be misread."""
    r = _report("basic-5k")
    defs = r["metric_definitions"]
    for key in ("recall_at_snr_floor", "precision", "centroid_error_px", "fwhm_rel_error"):
        assert key in defs and defs[key], f"metric {key} published without a definition"
    assert "PER-CHANNEL" in defs["snr"], "the per-channel SNR caveat (D-017) must travel with the report"


def test_nightscape_precision_survives_the_silhouette() -> None:
    """Regression guard for D-019.

    DAOFIND's zero-sum kernel reads sky beside a tree silhouette as a star; left
    untreated nightscape-fg scored 59 % precision on 4175 false positives. If
    this drops back, the foreground exclusion has regressed.
    """
    r = _report("nightscape-fg")
    assert r["measurement"]["sky_mask"], "nightscape-fg must be measured with its sky mask"
    assert r["measurement"]["foreground_exclusion_px"] >= 1
    assert r["metrics"]["precision"] >= 0.98, f"precision {r['metrics']['precision']:.4f}"


# --------------------------------------------------------------------------
# The matching rule (docs/FIXTURES.md) — the one piece of logic in compare.py
# that a wrong answer would silently flatter every metric.
# --------------------------------------------------------------------------


def _star(i, x, y, flux=100.0, fwhm=3.0, snr=10.0):
    return {
        "id": i, "x": x, "y": y, "flux": flux, "peak": 50.0, "fwhm": fwhm,
        "ellipticity": 0.0, "theta": 0.0, "saturated": False, "tier": "small", "snr": snr,
    }


def test_match_pairs_a_detection_within_the_radius() -> None:
    truth = [_star(1, 10.0, 10.0)]
    measured = [_star(1, 10.4, 10.3)]
    assert match(truth, measured)[0] == 0


def test_match_rejects_a_detection_outside_the_radius() -> None:
    # radius = max(1.5, 0.5 * 3.0) = 1.5 px; 3 px away must not match.
    truth = [_star(1, 10.0, 10.0, fwhm=3.0)]
    measured = [_star(1, 13.0, 10.0)]
    assert match(truth, measured)[0] == -1


def test_match_radius_follows_fwhm_when_the_star_is_broad() -> None:
    # radius = max(1.5, 0.5 * 10.0) = 5.0 px, so 4 px away matches.
    truth = [_star(1, 10.0, 10.0, fwhm=10.0)]
    measured = [_star(1, 14.0, 10.0)]
    assert match(truth, measured)[0] == 0


def test_each_detection_matches_at_most_one_truth_star() -> None:
    """Two truth stars, one detection: only one may claim it."""
    truth = [_star(1, 10.0, 10.0, flux=100.0), _star(2, 10.8, 10.0, flux=999.0)]
    measured = [_star(1, 10.4, 10.0)]
    m = match(truth, measured)
    assert sorted(m.tolist()) == [-1, 0], f"a detection was double-counted: {m}"


def test_brighter_truth_star_wins_the_contested_detection() -> None:
    """Greedy in descending truth flux (docs/FIXTURES.md)."""
    faint, bright = _star(1, 10.0, 10.0, flux=1.0), _star(2, 10.9, 10.0, flux=999.0)
    m = match([faint, bright], [_star(1, 10.5, 10.0)])
    assert m[1] == 0, "the brighter truth star must take the detection"
    assert m[0] == -1


def test_match_is_empty_when_there_are_no_detections() -> None:
    assert match([_star(1, 1.0, 1.0)], []).tolist() == [-1]


# --------------------------------------------------------------------------
# Metric semantics
# --------------------------------------------------------------------------


def _cat(stars, generator=None):
    c = {"schema": SCHEMA_V1, "image": {"width": 64, "height": 64, "bit_depth": 16,
         "color_space": "linear-rgb"}, "stars": stars}
    if generator is not None:
        c["generator"] = generator
    return c


def test_recall_counts_only_stars_above_the_snr_floor() -> None:
    truth = [_star(1, 10.0, 10.0, snr=50.0), _star(2, 30.0, 30.0, snr=1.0)]
    # Only the bright one is found; the faint one is below the floor and must not
    # count against recall.
    rep = compare(_cat(truth), _cat([_star(1, 10.0, 10.0)]))
    assert rep["metrics"]["recall_at_snr_floor"] == 1.0
    assert rep["counts"]["truth_at_snr_floor"] == 1


def test_detecting_a_faint_star_is_not_a_false_positive() -> None:
    """A detection on a real sub-floor star is a success, not an error.

    The stricter reading punishes the detector for finding a real star; both are
    reported so the choice stays visible.
    """
    truth = [_star(1, 10.0, 10.0, snr=50.0), _star(2, 30.0, 30.0, snr=1.0)]
    measured = [_star(1, 10.0, 10.0), _star(2, 30.0, 30.0)]
    m = compare(_cat(truth), _cat(measured))["metrics"]
    assert m["precision"] == 1.0
    assert m["precision_strict"] == 0.5


def test_a_detection_on_nothing_is_a_false_positive() -> None:
    truth = [_star(1, 10.0, 10.0, snr=50.0)]
    measured = [_star(1, 10.0, 10.0), _star(2, 50.0, 50.0)]
    rep = compare(_cat(truth), _cat(measured))
    assert rep["counts"]["false_positives"] == 1
    assert rep["metrics"]["precision"] == 0.5


def test_centroid_error_is_the_distance_to_truth() -> None:
    truth = [_star(1, 10.0, 10.0, snr=50.0)]
    measured = [_star(1, 10.3, 10.4)]
    m = compare(_cat(truth), _cat(measured))["metrics"]
    assert m["centroid_error_px"]["median"] == pytest.approx(np.hypot(0.3, 0.4))


def test_unusable_fwhm_is_excluded_and_counted_not_silently_zero() -> None:
    """A degenerate fit reports fwhm 0.0; averaging it in would fake a good score."""
    truth = [_star(1, 10.0, 10.0, snr=50.0, fwhm=3.0), _star(2, 30.0, 30.0, snr=50.0, fwhm=3.0)]
    good = _star(1, 10.0, 10.0, fwhm=3.3)
    broken = _star(2, 30.0, 30.0, fwhm=0.0)
    rep = compare(_cat(truth), _cat([good, broken]))
    assert rep["counts"]["fwhm_unusable"] == 1
    assert rep["metrics"]["fwhm_rel_error"]["n"] == 1
    assert rep["metrics"]["fwhm_rel_error"]["median"] == pytest.approx(0.1)


def test_per_snr_bins_partition_the_whole_truth_catalog() -> None:
    truth = [_star(i, i * 5.0, 5.0, snr=s) for i, s in enumerate([0.5, 4.0, 7.0, 15.0, 30.0, 80.0, 500.0])]
    rep = compare(_cat(truth), _cat([]))
    assert sum(b["n_truth"] for b in rep["per_snr_bin"]) == len(truth)


def test_report_carries_the_suite_and_seed_from_truth() -> None:
    rep = compare(_cat([_star(1, 1.0, 1.0)], generator={"suite": "basic-5k", "seed": 1000001}), _cat([]))
    assert rep["suite"] == "basic-5k"
    assert rep["seed"] == 1000001
