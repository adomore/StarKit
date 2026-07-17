#!/usr/bin/env python3
"""Independent star measurement on a StarKit fixture image (task T0-2).

    python measure.py --image PATH --out catalog.json [--fwhm-guess 3.0]

Emits catalog schema v1 in measured form (no `generator` block). Method, all of
it stock photutils/astropy — see oracle/CLAUDE.md, the point is independence
from the Rust side, not agreement with it:

  1. mean-of-RGB plane (D-010: the frame the truth catalog is written against)
  2. Background2D + MedianBackground for a tiled background and RMS map
  3. DAOStarFinder for detection and centroids
  4. an elliptical Gaussian2D fit per source for FWHM / ellipticity / theta
  5. circular aperture photometry for flux

Every step is deterministic: nothing here samples, so no seed is needed.

Known systematic on FWHM (D-018), measured against truth on basic-5k at
snr >= 10 and stable across fit-box sizes:

  * elliptical-Gaussian stars: +2.3 %  (pixel integration — the fixture
    integrates the profile over each pixel, the fitted model is evaluated at
    pixel centres)
  * circular-Moffat stars:    +14.2 %  (model mismatch — a Gaussian fitted to a
    Moffat's sharper core and heavier wings settles wider)

The suites mix both (~60 % Moffat), so the reported FWHM error is dominated by
the Moffat term. This is characterised rather than tuned away: FWHM is not part
of any T0-2 acceptance criterion, and Phase 1's oracle cross-check is recall
only (T1-3). Image second moments were tried first and are worse and less
explainable (+36 %): a Moffat's second moment depends on its beta, which varies
per star, so moments measure the wings rather than the half-max width.
"""

from __future__ import annotations

import argparse
import pathlib
import sys

import numpy as np
from astropy.modeling import fitting, models
from astropy.stats import SigmaClip
from photutils.aperture import CircularAperture, aperture_photometry
from photutils.background import Background2D, MedianBackground
from photutils.detection import DAOStarFinder
from scipy import ndimage

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from _common import SATURATION_ADU, SCHEMA_V1, load_luminance, versions, write_json  # noqa: E402

# Gaussian FWHM per sigma: 2*sqrt(2*ln2).
FWHM_PER_SIGMA = 2.3548200450309493

# DAOStarFinder's `sigma_radius` default: its kernel extends this many sigma.
# Used to size the silhouette exclusion so a kernel footprint cannot straddle the
# sky/foreground edge.
DAO_SIGMA_RADIUS = 1.5

# Detection threshold in units of the background RMS. 5.0 is calibrated on
# basic-5k, where it is the knee of the recall/precision tradeoff: lower floods
# the catalog with noise peaks (3.0 gives ~18k detections for 5k stars), higher
# starts dropping real faint stars. Recorded in every report.
DEFAULT_NSIGMA = 5.0

# Background2D tiling. 64 px is small enough to track the fixtures' linear
# gradient and the nightscape foreground edge, large enough that a 3.2 px star
# cannot pull its own tile's median.
BKG_BOX = 64
BKG_FILTER = 3


def measure(
    image_path: str,
    fwhm_guess: float,
    nsigma: float,
    sky_mask_path: str | None = None,
) -> dict:
    data = load_luminance(image_path)
    height, width = data.shape

    sky = None
    if sky_mask_path:
        from PIL import Image

        sky = np.array(Image.open(sky_mask_path).convert("L")) == 255
        if sky.shape != data.shape:
            raise SystemExit(f"sky mask {sky.shape} does not match image {data.shape}")

    # Keep the foreground out of the background tiles. Cheap and correct, though
    # it is not what fixes the silhouette false positives — see below.
    bkg = Background2D(
        data,
        box_size=BKG_BOX,
        filter_size=BKG_FILTER,
        sigma_clip=SigmaClip(sigma=3.0),
        bkg_estimator=MedianBackground(),
        mask=None if sky is None else ~sky,
    )
    data_sub = data - bkg.background
    rms = float(np.median(bkg.background_rms if sky is None else bkg.background_rms[sky]))

    # Exclude the silhouette *and a kernel radius around it* from detection.
    #
    # DAOFIND correlates a zero-sum Gaussian kernel: it responds to how much
    # brighter a pixel is than its neighbourhood, not to its absolute value. Next
    # to a dark silhouette that contrast is enormous, so ordinary sky reads as a
    # star. This is not a background error — measured at one such false positive
    # on nightscape-fg, the background estimate was 300.1, the data 298.3, a
    # residual of -1.8 against a threshold of 55, and DAOFIND still fired. Only
    # the convolved image explains it (D-019).
    #
    # Pixels *near* the edge are affected too, since the kernel footprint reaches
    # the silhouette, so the mask is dilated by the kernel radius. Untreated this
    # cost nightscape-fg 4175 false positives (99.9 % within 20 px of foreground,
    # median 2.8 px) and put precision at 59 %.
    detect_mask = None
    dilate_px = 0
    if sky is not None:
        dilate_px = int(np.ceil(DAO_SIGMA_RADIUS * fwhm_guess / FWHM_PER_SIGMA)) + 1
        size = 2 * dilate_px + 1
        detect_mask = ndimage.binary_dilation(~sky, structure=np.ones((size, size), dtype=bool))

    finder = DAOStarFinder(threshold=nsigma * rms, fwhm=fwhm_guess, exclude_border=False)
    sources = finder(data_sub, mask=detect_mask)
    if sources is None:
        sources = []

    xs = np.asarray(sources["xcentroid"], dtype=float) if len(sources) else np.array([])
    ys = np.asarray(sources["ycentroid"], dtype=float) if len(sources) else np.array([])

    # Fit box: 2 x FWHM each side. Wide enough to constrain the wings, tight
    # enough to keep neighbours out in dense fields. The measured FWHM bias is
    # flat across box sizes (see the module docstring), so this is a
    # crowding/cost choice, not an accuracy one.
    half = max(3, int(round(2.0 * fwhm_guess)))
    ap_r = max(2.0, 1.5 * fwhm_guess)

    fluxes = np.zeros(len(xs))
    if len(xs):
        phot = aperture_photometry(data_sub, CircularAperture(np.column_stack([xs, ys]), r=ap_r))
        fluxes = np.asarray(phot["aperture_sum"], dtype=float)

    stars = []
    for i in range(len(xs)):
        x, y = xs[i], ys[i]
        x0, x1 = int(max(0, round(x) - half)), int(min(width, round(x) + half + 1))
        y0, y1 = int(max(0, round(y) - half)), int(min(height, round(y) + half + 1))
        cut = data_sub[y0:y1, x0:x1]
        if cut.size == 0:
            continue

        fwhm, ellipticity, theta = _fit_shape(cut, x - x0, y - y0, fwhm_guess)

        peak = float(np.max(data[y0:y1, x0:x1]))
        stars.append(
            {
                "id": i + 1,
                "x": float(x),
                "y": float(y),
                "flux": float(fluxes[i]),
                "peak": peak,
                "fwhm": fwhm if np.isfinite(fwhm) else 0.0,
                "ellipticity": ellipticity,
                "theta": theta,
                "saturated": bool(peak >= SATURATION_ADU),
                "tier": "small",  # replaced below, once every flux is known
            }
        )

    _assign_tiers(stars)

    return {
        "schema": SCHEMA_V1,
        "image": {
            "width": int(width),
            "height": int(height),
            "bit_depth": 16,
            "color_space": "linear-rgb",
        },
        "stars": stars,
        "measurement": {
            "instrument": "oracle/measure.py",
            "method": "Background2D+MedianBackground -> DAOStarFinder -> Gaussian2D fit",
            "plane": "mean-of-RGB",
            "fwhm_guess_px": fwhm_guess,
            "nsigma": nsigma,
            "background_rms": rms,
            "background_box_size": BKG_BOX,
            "sky_mask": sky_mask_path,
            "foreground_exclusion_px": dilate_px,
            "aperture_radius_px": ap_r,
            "versions": versions(),
        },
    }


def _fit_shape(cut: np.ndarray, cx: float, cy: float, fwhm_guess: float):
    """Fit an elliptical Gaussian to a cutout -> (fwhm, ellipticity, theta).

    Returns `(nan, 0.0, 0.0)` when the fit degenerates (a source on the frame
    edge, or an unrecoverable blend). The caller keeps such a detection with its
    position and drops only the shape: recall is the headline metric, and
    throwing away a real star to avoid an awkward number would be the worse lie.
    """
    try:
        yy, xx = np.mgrid[0 : cut.shape[0], 0 : cut.shape[1]]
        sigma_guess = fwhm_guess / FWHM_PER_SIGMA
        init = models.Gaussian2D(
            amplitude=float(np.max(cut)),
            x_mean=cx,
            y_mean=cy,
            x_stddev=sigma_guess,
            y_stddev=sigma_guess,
        )
        fit = fitting.TRFLSQFitter()(init, xx, yy, cut)
        sx = abs(float(fit.x_stddev.value))
        sy = abs(float(fit.y_stddev.value))
        if not (np.isfinite(sx) and np.isfinite(sy)) or sx <= 0 or sy <= 0:
            return float("nan"), 0.0, 0.0

        # Geometric mean, matching the truth catalog's definition of `fwhm`
        # (docs/FIXTURES.md). photutils'/astropy's own convenience properties use
        # the quadratic mean — close for round stars, but a different number, and
        # we are graded against truth.
        fwhm = FWHM_PER_SIGMA * float(np.sqrt(sx * sy))
        major, minor = max(sx, sy), min(sx, sy)
        ellipticity = float(1.0 - minor / major)
        theta = float(fit.theta.value)
        if sy > sx:
            # astropy's theta is the angle of the x_stddev axis; when y is the
            # major axis the position angle is a quarter turn away.
            theta += np.pi / 2.0
        # Truth reports theta in [0, pi): an ellipse is symmetric under a half
        # turn, so fold rather than report an equivalent-but-different angle.
        theta = float(theta % np.pi)
        return fwhm, ellipticity, theta
    except Exception:
        return float("nan"), 0.0, 0.0


def _assign_tiers(stars: list[dict]) -> None:
    """Flux tertiles, matching how the truth catalog tiers its population.

    Tiers never enter the match: evaluation pairs stars by position, never by
    tier equality (docs/FIXTURES.md). This exists so the measured catalog is
    schema-valid, not so it can be compared.
    """
    if not stars:
        return
    order = sorted(range(len(stars)), key=lambda i: stars[i]["flux"])
    n = len(order)
    for rank, i in enumerate(order):
        if rank >= (2 * n) // 3:
            stars[i]["tier"] = "large"
        elif rank >= n // 3:
            stars[i]["tier"] = "medium"
        else:
            stars[i]["tier"] = "small"


def main() -> None:
    ap = argparse.ArgumentParser(description="Measure stars in a StarKit fixture image")
    ap.add_argument("--image", required=True, help="path to the fixture TIFF")
    ap.add_argument("--out", required=True, help="path to write the measured catalog JSON")
    ap.add_argument("--fwhm-guess", type=float, default=3.0, help="DAOStarFinder FWHM prior, px")
    ap.add_argument("--nsigma", type=float, default=DEFAULT_NSIGMA, help="threshold in background RMS")
    ap.add_argument(
        "--sky-mask",
        default=None,
        help="optional 8-bit sky mask (255=sky). Required for a nightscape: it keeps the "
        "foreground out of the background estimate and out of the catalog.",
    )
    args = ap.parse_args()

    catalog = measure(args.image, args.fwhm_guess, args.nsigma, args.sky_mask)
    write_json(args.out, catalog)
    print(f"{args.image}: {len(catalog['stars'])} sources -> {args.out}")


if __name__ == "__main__":
    main()
