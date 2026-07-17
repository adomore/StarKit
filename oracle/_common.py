"""Shared helpers for the oracle scripts.

Deliberately thin. The oracle's value is that it is an *independent* instrument
(see oracle/CLAUDE.md): it measures the fixtures with photutils/astropy
primitives and shares no math with the Rust side. This module holds only I/O,
schema constants and provenance — nothing that could be mistaken for an
algorithm.
"""

from __future__ import annotations

import json
import pathlib
from typing import Any

# Catalog schema v1. Canonical definition: docs/FIXTURES.md.
SCHEMA_V1 = "starkit-catalog/1"
# Report schema emitted by compare.py.
REPORT_SCHEMA_V1 = "starkit-oracle-report/1"

# Saturation point of the 16-bit fixtures.
SATURATION_ADU = 65535


def versions() -> dict[str, str]:
    """Exact resolved versions of every library that can move a number.

    Required by oracle/CLAUDE.md: reports pin these, requirements.txt stays
    loose. A report whose numbers cannot be traced to a toolchain is not
    evidence.
    """
    import astropy
    import numpy
    import photutils
    import scipy
    import tifffile

    return {
        "numpy": numpy.__version__,
        "scipy": scipy.__version__,
        "astropy": astropy.__version__,
        "photutils": photutils.__version__,
        "tifffile": tifffile.__version__,
    }


def load_luminance(path: str | pathlib.Path):
    """Load a fixture image as the mean-of-RGB plane, float64.

    This is the contract the truth catalog is written against (D-010): `flux`
    and `peak` describe the noiseless mean of the three channels, with per-star
    tint normalised to mean 1.0. Measuring any other combination would compare
    against numbers that were never claimed.
    """
    import numpy as np
    import tifffile

    img = tifffile.imread(str(path))
    if img.ndim != 3 or img.shape[2] != 3:
        raise SystemExit(f"expected an RGB image, got shape {img.shape} from {path}")
    return img.astype(np.float64).mean(axis=2)


def write_json(path: str | pathlib.Path, payload: dict[str, Any]) -> None:
    """Write UTF-8 JSON with LF endings and a trailing newline.

    Mirrors the Rust side's serialization policy (D-009) so committed oracle
    reports are as diff-stable as the fixtures they describe.
    """
    p = pathlib.Path(path)
    p.parent.mkdir(parents=True, exist_ok=True)
    text = json.dumps(payload, indent=2, ensure_ascii=False, sort_keys=False)
    with open(p, "w", encoding="utf-8", newline="\n") as f:
        f.write(text + "\n")


def read_json(path: str | pathlib.Path) -> dict[str, Any]:
    with open(path, encoding="utf-8") as f:
        return json.load(f)
