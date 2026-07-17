#!/usr/bin/env python3
"""Compare a measured catalog against truth (task T0-2).

    python compare.py --truth T.json --measured M.json --out report.json

Implements the matching rule frozen in docs/FIXTURES.md:

    A detected star matches a truth star if within radius max(1.5 px,
    0.5 x truth_fwhm); greedy matching in descending truth flux; each truth
    star matches at most once.

Reported metrics and their exact definitions live in `metric_definitions` in
the emitted report, so a number can never be read without its meaning.
"""

from __future__ import annotations

import argparse
import pathlib
import sys

import numpy as np
from scipy.spatial import cKDTree

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from _common import REPORT_SCHEMA_V1, read_json, versions, write_json  # noqa: E402

# The SNR floor the quality bar is conditioned on (docs/FIXTURES.md, FR-2).
# NOTE: truth `snr` is *per-channel* peak SNR (D-017). The mean-of-RGB plane the
# oracle measures has sqrt(3) better SNR, so a star labelled snr=5 sits at ~8.7
# sigma in the measured image. The bar is honest only when read with that in
# mind.
SNR_FLOOR = 5.0

# Upper edges of the per-SNR-bin table.
SNR_BINS = [3.0, 5.0, 10.0, 20.0, 50.0, 100.0, float("inf")]


def match(truth: list[dict], measured: list[dict]) -> np.ndarray:
    """Return, per truth star, the index of its matched detection or -1.

    Greedy in descending truth flux so that when two truth stars contend for one
    detection, the brighter one wins — which is what a human would call correct
    and, more importantly, is what docs/FIXTURES.md specifies.
    """
    n_t = len(truth)
    matched = np.full(n_t, -1, dtype=int)
    if not measured or n_t == 0:
        return matched

    mx = np.array([s["x"] for s in measured], dtype=float)
    my = np.array([s["y"] for s in measured], dtype=float)
    tree = cKDTree(np.column_stack([mx, my]))
    taken = np.zeros(len(measured), dtype=bool)

    tx = np.array([s["x"] for s in truth], dtype=float)
    ty = np.array([s["y"] for s in truth], dtype=float)
    tfwhm = np.array([s["fwhm"] for s in truth], dtype=float)
    tflux = np.array([s["flux"] for s in truth], dtype=float)

    for i in np.argsort(-tflux):
        radius = max(1.5, 0.5 * tfwhm[i])
        best, best_d2 = -1, np.inf
        for c in tree.query_ball_point([tx[i], ty[i]], radius):
            if taken[c]:
                continue
            d2 = (mx[c] - tx[i]) ** 2 + (my[c] - ty[i]) ** 2
            if d2 < best_d2:
                best, best_d2 = c, d2
        if best >= 0:
            taken[best] = True
            matched[i] = best
    return matched


def _stats(values: np.ndarray) -> dict:
    if values.size == 0:
        return {"n": 0, "median": None, "p95": None, "mean": None}
    return {
        "n": int(values.size),
        "median": float(np.median(values)),
        "p95": float(np.percentile(values, 95)),
        "mean": float(np.mean(values)),
    }


def compare(truth_cat: dict, measured_cat: dict) -> dict:
    truth = truth_cat["stars"]
    measured = measured_cat["stars"]
    matched = match(truth, measured)

    tsnr = np.array([s.get("snr") or 0.0 for s in truth], dtype=float)
    bright = tsnr >= SNR_FLOOR
    found = matched >= 0

    n_bright = int(bright.sum())
    recall = float(found[bright].mean()) if n_bright else None

    # Precision: a detection that lands on *any* truth star is a real star, not
    # a false positive — even if that star is below the SNR floor. Counting such
    # a detection as an error would penalise the detector for succeeding. The
    # stricter reading (only SNR>=5 matches count) is reported alongside so the
    # choice is visible rather than baked in; on basic-5k it differs by ~18 pp.
    n_det = len(measured)
    n_matched_any = int(found.sum())
    n_matched_bright = int(found[bright].sum())
    precision = float(n_matched_any / n_det) if n_det else None
    precision_strict = float(n_matched_bright / n_det) if n_det else None

    idx = np.where(found)[0]
    cerr = np.array(
        [
            np.hypot(measured[matched[i]]["x"] - truth[i]["x"], measured[matched[i]]["y"] - truth[i]["y"])
            for i in idx
        ]
    )

    # FWHM error over matched pairs that carry a usable measurement. A measured
    # fwhm of 0.0 means the moment fit degenerated (edge/blend); those are
    # excluded from the FWHM statistic and counted, rather than silently
    # dragging the median toward zero.
    fw_rel, n_fw_bad = [], 0
    for i in idx:
        mf = measured[matched[i]]["fwhm"]
        tf = truth[i]["fwhm"]
        if mf and mf > 0 and tf > 0:
            fw_rel.append((mf - tf) / tf)
        else:
            n_fw_bad += 1
    fw_rel = np.array(fw_rel)

    bins = []
    lo = 0.0
    for hi in SNR_BINS:
        sel = (tsnr >= lo) & (tsnr < hi)
        n = int(sel.sum())
        bins.append(
            {
                "snr_min": lo,
                "snr_max": None if np.isinf(hi) else hi,
                "n_truth": n,
                "recall": float(found[sel].mean()) if n else None,
            }
        )
        lo = hi

    return {
        "schema": REPORT_SCHEMA_V1,
        "suite": truth_cat.get("generator", {}).get("suite"),
        "seed": truth_cat.get("generator", {}).get("seed"),
        "counts": {
            "truth_total": len(truth),
            "truth_at_snr_floor": n_bright,
            "detections": n_det,
            "matched_any": n_matched_any,
            "matched_at_snr_floor": n_matched_bright,
            "false_positives": n_det - n_matched_any,
            "fwhm_unusable": n_fw_bad,
        },
        "metrics": {
            "snr_floor": SNR_FLOOR,
            "recall_at_snr_floor": recall,
            "precision": precision,
            "precision_strict": precision_strict,
            "centroid_error_px": _stats(cerr),
            "fwhm_rel_error": _stats(np.abs(fw_rel)),
            "fwhm_rel_bias": float(np.median(fw_rel)) if fw_rel.size else None,
        },
        "per_snr_bin": bins,
        "metric_definitions": {
            "matching": "detection within max(1.5 px, 0.5 x truth_fwhm); greedy in descending truth flux; each truth star matches at most once (docs/FIXTURES.md)",
            "snr": "truth peak SNR is PER-CHANNEL (D-017); the measured mean-of-RGB plane has sqrt(3) better SNR, so snr=5 is ~8.7 sigma in the data actually measured",
            "recall_at_snr_floor": "matched truth stars with snr >= floor / all truth stars with snr >= floor",
            "precision": "detections matched to any truth star / all detections",
            "precision_strict": "detections matched to a truth star with snr >= floor / all detections",
            "centroid_error_px": "euclidean distance between matched detection and truth position",
            "fwhm_rel_error": "|measured_fwhm - truth_fwhm| / truth_fwhm over matched pairs with a usable fit",
            "fwhm_rel_bias": "median signed (measured - truth)/truth: sign shows whether moments over- or under-estimate",
        },
        "measurement": measured_cat.get("measurement"),
        "versions": versions(),
    }


def main() -> None:
    ap = argparse.ArgumentParser(description="Compare a measured catalog against truth")
    ap.add_argument("--truth", required=True)
    ap.add_argument("--measured", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    report = compare(read_json(args.truth), read_json(args.measured))
    write_json(args.out, report)

    m = report["metrics"]
    print(
        f"{report['suite']}: recall@snr>={m['snr_floor']} = {_pct(m['recall_at_snr_floor'])}  "
        f"precision = {_pct(m['precision'])}  "
        f"centroid median = {m['centroid_error_px']['median']:.3f} px  "
        f"fwhm median err = {_pct(m['fwhm_rel_error']['median'])}"
    )


def _pct(v: float | None) -> str:
    return "n/a" if v is None else f"{v * 100:.2f}%"


if __name__ == "__main__":
    main()
