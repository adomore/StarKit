//! ICC handling: extract the tone-response curve, and nothing else.
//!
//! # D-005 resolution: no ICC transform backend (D-029)
//!
//! The obvious reading of "apply the embedded ICC transform" is to convert every
//! image into one canonical linear space. This crate does **not** do that, for
//! two reasons that are not stylistic:
//!
//! 1. **It would destroy data.** ProPhoto RGB is far wider than sRGB. Converting
//!    a ProPhoto frame into a canonical linear-sRGB space clips every colour
//!    outside sRGB's gamut, and no later step can get it back. The photographer
//!    works in wide-gamut space precisely because those colours matter.
//! 2. **It would break FR-1.** The no-op round-trip must be *pixel-identical*.
//!    Convert-and-convert-back is only approximately invertible in f32.
//!
//! So the working space is **linear light in the source's own primaries**: this
//! crate inverts the transfer curve and leaves the primaries alone, and the ICC
//! profile is carried through byte-for-byte and re-attached on encode. That
//! satisfies INV-4 (photometric math runs in linear light, conversion happens
//! only at the I/O boundary) and starkit-io/CLAUDE.md's "de-linearize through the
//! same transform used at decode" — literally the same curve, inverted.
//!
//! It is also enough: star reduction and enhancement are per-channel photometry,
//! not colorimetry. Nothing downstream needs a common colour space, and
//! Photoshop reads the preserved profile and sees the right colours.
//!
//! Consequence: **no lcms2, no qcms, no FFI, no new dependency** — only a small
//! parser for the `rTRC`/`gTRC`/`bTRC` tags. Should a future feature genuinely
//! need cross-profile conversion (it would have to justify the gamut loss),
//! revisit D-029 then.

use crate::error::{IoError, Result};
use std::path::Path;

/// A per-channel tone response curve, as a function display-value → linear light.
///
/// Both directions must round-trip: `from_linear(to_linear(v)) == v` for every
/// v that a 16-bit file can hold. That is what FR-1's pixel-identical no-op
/// rests on, and `curve_round_trips_every_16_bit_value` checks it exhaustively.
#[derive(Debug, Clone, PartialEq)]
pub enum Curve {
    /// Already linear (ICC `curv` with count 0, or gamma 1.0).
    Linear,
    /// Pure power law: linear = v^gamma.
    Gamma(f64),
    /// ICC `para` type 0-4, reduced to the type-4 general form:
    /// `linear = (a*v + b)^g + e` for `v >= d`, else `c*v + f`.
    Parametric {
        g: f64,
        a: f64,
        b: f64,
        c: f64,
        d: f64,
        e: f64,
        f: f64,
    },
    /// ICC `curv` sampled table, uniformly spaced in v, values in [0, 1].
    Table(Vec<f64>),
    /// The standard sRGB piecewise curve. Used when a file carries no profile.
    Srgb,
}

impl Curve {
    pub fn to_linear(&self, v: f64) -> f64 {
        match self {
            Curve::Linear => v,
            Curve::Gamma(g) => powf_clamped(v, *g),
            Curve::Parametric {
                g,
                a,
                b,
                c,
                d,
                e,
                f,
            } => {
                if v >= *d {
                    powf_clamped(a * v + b, *g) + e
                } else {
                    c * v + f
                }
            }
            Curve::Table(t) => interp(t, v),
            Curve::Srgb => {
                if v <= 0.040_448_236_277_105_08 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            }
        }
    }

    pub fn from_linear(&self, x: f64) -> f64 {
        match self {
            Curve::Linear => x,
            Curve::Gamma(g) => powf_clamped(x, 1.0 / g),
            Curve::Parametric {
                g,
                a,
                b,
                c,
                d,
                e,
                f,
            } => {
                // Invert each branch on its own domain. The split is at v = d,
                // which in linear terms is wherever the linear branch ends.
                let x_at_d = c * d + f;
                if x >= x_at_d {
                    (powf_clamped(x - e, 1.0 / g) - b) / a
                } else if *c != 0.0 {
                    (x - f) / c
                } else {
                    0.0
                }
            }
            Curve::Table(t) => invert_interp(t, x),
            Curve::Srgb => {
                if x <= 0.003_130_668_442_500_54 {
                    x * 12.92
                } else {
                    1.055 * x.powf(1.0 / 2.4) - 0.055
                }
            }
        }
    }
}

/// `powf` is only defined for a negative base with an integral exponent; a
/// negative input here means an out-of-range sample, and NaN would poison the
/// whole pipeline silently.
fn powf_clamped(v: f64, g: f64) -> f64 {
    if v <= 0.0 {
        0.0
    } else {
        v.powf(g)
    }
}

fn interp(table: &[f64], v: f64) -> f64 {
    if table.is_empty() {
        return v;
    }
    if table.len() == 1 {
        return powf_clamped(v, table[0]);
    }
    let v = v.clamp(0.0, 1.0);
    let pos = v * (table.len() - 1) as f64;
    let i = (pos.floor() as usize).min(table.len() - 2);
    let frac = pos - i as f64;
    table[i] * (1.0 - frac) + table[i + 1] * frac
}

/// Invert a monotonic sampled curve by binary search plus linear interpolation.
fn invert_interp(table: &[f64], x: f64) -> f64 {
    if table.len() < 2 {
        return x;
    }
    let last = table.len() - 1;
    if x <= table[0] {
        return 0.0;
    }
    if x >= table[last] {
        return 1.0;
    }
    let (mut lo, mut hi) = (0usize, last);
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if table[mid] <= x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let span = table[hi] - table[lo];
    let frac = if span.abs() < f64::EPSILON {
        0.0
    } else {
        (x - table[lo]) / span
    };
    (lo as f64 + frac) / last as f64
}

/// The three per-channel curves of a profile.
#[derive(Debug, Clone, PartialEq)]
pub struct Trc {
    pub r: Curve,
    pub g: Curve,
    pub b: Curve,
}

impl Trc {
    /// What an untagged file gets: sRGB, per starkit-io/CLAUDE.md.
    pub fn srgb() -> Self {
        Trc {
            r: Curve::Srgb,
            g: Curve::Srgb,
            b: Curve::Srgb,
        }
    }

    pub fn channel(&self, c: usize) -> &Curve {
        match c {
            0 => &self.r,
            1 => &self.g,
            _ => &self.b,
        }
    }
}

fn be_u32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn be_u16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

/// s15Fixed16 → f64.
fn s15f16(b: &[u8], at: usize) -> Option<f64> {
    Some(i32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?) as f64 / 65536.0)
}

const ICC_HEADER_LEN: usize = 128;

/// Extract the tone response curves from a raw ICC profile.
///
/// Returns `None` when the profile parses but carries no TRC tags — a LUT-based
/// (`A2B0`) profile, for instance. The caller decides what to do about it;
/// silently substituting sRGB would be a lie about the file.
pub fn parse_trc(icc: &[u8], path: &Path) -> Result<Option<Trc>> {
    let bad = |d: &str| IoError::icc(path, d.to_string());

    if icc.len() < ICC_HEADER_LEN + 4 {
        return Err(bad("profile is shorter than an ICC header"));
    }
    let declared = be_u32(icc, 0).ok_or_else(|| bad("truncated header"))? as usize;
    if declared > icc.len() {
        return Err(bad(&format!(
            "profile declares {declared} bytes but only {} are present",
            icc.len()
        )));
    }

    let count = be_u32(icc, ICC_HEADER_LEN).ok_or_else(|| bad("truncated tag count"))? as usize;
    // 12 bytes per tag-table entry; a bogus count must not make us scan the heap.
    if count > (icc.len() - ICC_HEADER_LEN - 4) / 12 {
        return Err(bad(&format!("tag count {count} exceeds the profile size")));
    }

    let mut found: [Option<Curve>; 3] = [None, None, None];
    for i in 0..count {
        let at = ICC_HEADER_LEN + 4 + i * 12;
        let sig = be_u32(icc, at).ok_or_else(|| bad("truncated tag table"))?;
        let off = be_u32(icc, at + 4).ok_or_else(|| bad("truncated tag table"))? as usize;
        let size = be_u32(icc, at + 8).ok_or_else(|| bad("truncated tag table"))? as usize;

        let slot = match &sig.to_be_bytes() {
            b"rTRC" => 0,
            b"gTRC" => 1,
            b"bTRC" => 2,
            _ => continue,
        };
        let data = icc
            .get(
                off..off
                    .checked_add(size)
                    .ok_or_else(|| bad("tag offset overflows"))?,
            )
            .ok_or_else(|| bad("tag data lies outside the profile"))?;
        found[slot] = Some(parse_curve(data, path)?);
    }

    match (found[0].take(), found[1].take(), found[2].take()) {
        (Some(r), Some(g), Some(b)) => Ok(Some(Trc { r, g, b })),
        (None, None, None) => Ok(None),
        _ => Err(bad("profile defines some but not all of rTRC/gTRC/bTRC")),
    }
}

fn parse_curve(data: &[u8], path: &Path) -> Result<Curve> {
    let bad = |d: &str| IoError::icc(path, d.to_string());
    let sig = be_u32(data, 0).ok_or_else(|| bad("truncated curve tag"))?;

    match &sig.to_be_bytes() {
        b"curv" => {
            let n = be_u32(data, 8).ok_or_else(|| bad("truncated curv"))? as usize;
            match n {
                // Count 0 is the ICC spec's way of writing "identity".
                0 => Ok(Curve::Linear),
                // Count 1: a single u8Fixed8 gamma.
                1 => {
                    let g =
                        be_u16(data, 12).ok_or_else(|| bad("truncated curv gamma"))? as f64 / 256.0;
                    Ok(if (g - 1.0).abs() < 1e-9 {
                        Curve::Linear
                    } else {
                        Curve::Gamma(g)
                    })
                }
                _ => {
                    let mut t = Vec::with_capacity(n);
                    for i in 0..n {
                        let v = be_u16(data, 12 + i * 2)
                            .ok_or_else(|| bad("curv table is shorter than its count"))?;
                        t.push(v as f64 / 65535.0);
                    }
                    Ok(Curve::Table(t))
                }
            }
        }
        b"para" => {
            let kind = be_u16(data, 8).ok_or_else(|| bad("truncated para"))?;
            let p = |i: usize| s15f16(data, 12 + i * 4).ok_or_else(|| bad("truncated para params"));
            // Types 0-4 are all special cases of the type-4 general form; folding
            // them here means `Curve::Parametric` has exactly one shape to invert.
            Ok(match kind {
                0 => Curve::Gamma(p(0)?),
                1 => {
                    let (g, a, b) = (p(0)?, p(1)?, p(2)?);
                    Curve::Parametric {
                        g,
                        a,
                        b,
                        c: 0.0,
                        d: -b / a,
                        e: 0.0,
                        f: 0.0,
                    }
                }
                2 => {
                    let (g, a, b, c) = (p(0)?, p(1)?, p(2)?, p(3)?);
                    Curve::Parametric {
                        g,
                        a,
                        b,
                        c: 0.0,
                        d: -b / a,
                        e: c,
                        f: c,
                    }
                }
                3 => {
                    let (g, a, b, c, d) = (p(0)?, p(1)?, p(2)?, p(3)?, p(4)?);
                    Curve::Parametric {
                        g,
                        a,
                        b,
                        c,
                        d,
                        e: 0.0,
                        f: 0.0,
                    }
                }
                4 => Curve::Parametric {
                    g: p(0)?,
                    a: p(1)?,
                    b: p(2)?,
                    c: p(3)?,
                    d: p(4)?,
                    e: p(5)?,
                    f: p(6)?,
                },
                other => return Err(bad(&format!("unsupported para curve type {other}"))),
            })
        }
        other => Err(bad(&format!(
            "unsupported curve type {:?}",
            String::from_utf8_lossy(other)
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> &'static Path {
        Path::new("test.icc")
    }

    /// The property FR-1 rests on. Exhaustive over all 65536 16-bit codes: a
    /// sampled check would miss exactly the boundary values that break.
    fn assert_round_trips(curve: &Curve, name: &str) {
        for code in 0u32..=65535 {
            let v = code as f64 / 65535.0;
            let back = curve.from_linear(curve.to_linear(v)) * 65535.0;
            let recovered = back.round() as i64;
            assert_eq!(
                recovered, code as i64,
                "{name}: code {code} came back as {recovered} ({back})"
            );
        }
    }

    #[test]
    fn srgb_curve_round_trips_every_16_bit_value() {
        assert_round_trips(&Curve::Srgb, "sRGB");
    }

    #[test]
    fn gamma_curves_round_trip_every_16_bit_value() {
        // 2.19921875 is AdobeRGB's gamma; 1.8 is ProPhoto's.
        for g in [2.2, 2.199_218_75, 1.8, 1.0] {
            assert_round_trips(&Curve::Gamma(g), &format!("gamma {g}"));
        }
    }

    #[test]
    fn linear_curve_round_trips_every_16_bit_value() {
        assert_round_trips(&Curve::Linear, "linear");
    }

    #[test]
    fn parametric_srgb_round_trips_every_16_bit_value() {
        // The sRGB curve expressed as an ICC para type 3 — what a real sRGB
        // profile actually stores.
        assert_round_trips(
            &Curve::Parametric {
                g: 2.4,
                a: 1.0 / 1.055,
                b: 0.055 / 1.055,
                c: 1.0 / 12.92,
                d: 0.04045,
                e: 0.0,
                f: 0.0,
            },
            "para sRGB",
        );
    }

    #[test]
    fn srgb_matches_the_standard_at_known_points() {
        // Tolerance: the constants are the standard's own rounded values.
        assert!((Curve::Srgb.to_linear(0.0) - 0.0).abs() < 1e-12);
        assert!((Curve::Srgb.to_linear(1.0) - 1.0).abs() < 1e-12);
        // Mid-grey 0.5 is ~0.2140 in linear light — the classic check.
        assert!((Curve::Srgb.to_linear(0.5) - 0.214_041_14).abs() < 1e-6);
    }

    #[test]
    fn negative_input_cannot_produce_nan() {
        for c in [Curve::Gamma(2.2), Curve::Srgb, Curve::Linear] {
            assert!(
                c.to_linear(-0.5).is_finite(),
                "{c:?} produced a non-finite value"
            );
            assert!(
                c.from_linear(-0.5).is_finite(),
                "{c:?} produced a non-finite value"
            );
        }
    }

    // -- parser -------------------------------------------------------------

    /// Build a minimal but structurally valid ICC carrying three `curv` tags.
    fn icc_with_curv(payload: &[u8]) -> Vec<u8> {
        let tags: [&[u8; 4]; 3] = [b"rTRC", b"gTRC", b"bTRC"];
        let table_at = ICC_HEADER_LEN + 4;
        let data_at = table_at + 12 * tags.len();
        let mut v = vec![0u8; data_at];
        v.extend_from_slice(payload);
        let total = v.len() as u32;
        v[0..4].copy_from_slice(&total.to_be_bytes());
        v[ICC_HEADER_LEN..ICC_HEADER_LEN + 4].copy_from_slice(&(tags.len() as u32).to_be_bytes());
        for (i, sig) in tags.iter().enumerate() {
            let at = table_at + i * 12;
            v[at..at + 4].copy_from_slice(*sig);
            v[at + 4..at + 8].copy_from_slice(&(data_at as u32).to_be_bytes());
            v[at + 8..at + 12].copy_from_slice(&(payload.len() as u32).to_be_bytes());
        }
        v
    }

    fn curv_gamma(g: f64) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(b"curv");
        p.extend_from_slice(&0u32.to_be_bytes());
        p.extend_from_slice(&1u32.to_be_bytes());
        p.extend_from_slice(&(((g * 256.0).round()) as u16).to_be_bytes());
        p
    }

    /// `curv` stores gamma as u8Fixed8, which cannot represent 2.2: it lands on
    /// 563/256 = 2.19921875. That is not a rounding wart to paper over — it is
    /// exactly why AdobeRGB's gamma is 2.19921875 rather than 2.2. The parser
    /// must report what the file says, not what the author meant.
    #[test]
    fn parses_a_gamma_curv_tag_at_the_precision_the_format_allows() {
        let icc = icc_with_curv(&curv_gamma(2.2));
        let trc = parse_trc(&icc, p()).expect("parse").expect("has TRC");
        assert_eq!(trc.r, Curve::Gamma(2.199_218_75));
        assert_eq!(trc.r, trc.g, "all three channels carry the same curve here");
    }

    #[test]
    fn a_gamma_of_one_is_recognised_as_linear() {
        let icc = icc_with_curv(&curv_gamma(1.0));
        let trc = parse_trc(&icc, p()).expect("parse").expect("has TRC");
        assert_eq!(trc.r, Curve::Linear);
    }

    #[test]
    fn curv_with_count_zero_is_the_identity() {
        let mut payload = Vec::new();
        payload.extend_from_slice(b"curv");
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(&0u32.to_be_bytes());
        let trc = parse_trc(&icc_with_curv(&payload), p())
            .expect("parse")
            .expect("has TRC");
        assert_eq!(trc.r, Curve::Linear);
    }

    #[test]
    fn parses_a_curv_table() {
        let mut payload = Vec::new();
        payload.extend_from_slice(b"curv");
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(&3u32.to_be_bytes());
        for v in [0u16, 16384, 65535] {
            payload.extend_from_slice(&v.to_be_bytes());
        }
        let trc = parse_trc(&icc_with_curv(&payload), p())
            .expect("parse")
            .expect("has TRC");
        match &trc.r {
            Curve::Table(t) => {
                assert_eq!(t.len(), 3);
                assert!((t[2] - 1.0).abs() < 1e-9);
            }
            other => panic!("expected a table, got {other:?}"),
        }
    }

    #[test]
    fn a_profile_with_no_trc_tags_reports_none_rather_than_guessing() {
        let mut v = vec![0u8; ICC_HEADER_LEN + 4];
        let total = v.len() as u32;
        v[0..4].copy_from_slice(&total.to_be_bytes());
        assert_eq!(parse_trc(&v, p()).expect("parse"), None);
    }

    #[test]
    fn a_truncated_profile_is_an_error_not_a_panic() {
        assert!(parse_trc(&[0u8; 8], p()).is_err());
    }

    #[test]
    fn a_profile_declaring_more_bytes_than_it_has_is_rejected() {
        let mut icc = icc_with_curv(&curv_gamma(2.2));
        icc[0..4].copy_from_slice(&999_999u32.to_be_bytes());
        assert!(parse_trc(&icc, p()).is_err());
    }

    #[test]
    fn an_absurd_tag_count_is_rejected_without_scanning_memory() {
        let mut icc = icc_with_curv(&curv_gamma(2.2));
        icc[ICC_HEADER_LEN..ICC_HEADER_LEN + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(parse_trc(&icc, p()).is_err());
    }

    #[test]
    fn a_tag_pointing_outside_the_profile_is_rejected() {
        let mut icc = icc_with_curv(&curv_gamma(2.2));
        let at = ICC_HEADER_LEN + 4;
        icc[at + 4..at + 8].copy_from_slice(&900_000u32.to_be_bytes());
        assert!(parse_trc(&icc, p()).is_err());
    }

    #[test]
    fn a_partial_trc_set_is_rejected_rather_than_half_applied() {
        let mut icc = icc_with_curv(&curv_gamma(2.2));
        // Rename gTRC to something we ignore, leaving r and b defined.
        let at = ICC_HEADER_LEN + 4 + 12;
        icc[at..at + 4].copy_from_slice(b"wtpt");
        assert!(parse_trc(&icc, p()).is_err());
    }
}
