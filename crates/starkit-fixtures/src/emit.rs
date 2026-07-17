//! Writing fixtures to disk: TIFF16 image, sky-mask PNG, truth catalog JSON,
//! params JSON, and SHA-256 manifest lines.
//!
//! Serialization policy (D-009), because every byte here is hash-pinned:
//! - JSON via `serde_json::to_string_pretty` (2-space indent, `\n` newlines,
//!   `ryu` shortest-round-trip floats — platform-independent), plus one trailing
//!   newline. `.gitattributes` marks `fixtures/expected/**` as `-text` so git
//!   never rewrites those newlines on a Windows checkout.
//! - Manifest lines are `sha256sum` format (`<hex>  <path>`), paths **relative to
//!   the output directory** with forward slashes, sorted byte-wise by path. No
//!   absolute path can leak into a committed file.

use std::io::{BufWriter, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::catalog::Catalog;
use crate::params::Params;

/// One manifest entry: the hash of an emitted artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestLine {
    /// Path relative to the output root, forward slashes.
    pub path: String,
    pub sha256: String,
}

impl ManifestLine {
    fn render(&self) -> String {
        format!("{}  {}\n", self.sha256, self.path)
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Serialize any fixture JSON under the one policy above.
pub fn to_json_bytes<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    let mut s = serde_json::to_string_pretty(value)?;
    s.push('\n');
    Ok(s.into_bytes())
}

/// TIFF tag 34675 — the embedded ICC profile.
const TAG_ICC: u16 = 34675;

/// Encode an interleaved RGB16 buffer as a 16-bit TIFF, in memory.
///
/// Embeds the linear-sRGB profile (D-032) so the file states its own colour
/// space instead of leaving every reader to guess. Without it a decoder must
/// assume something — `starkit-io` assumes sRGB, by its own documented rule —
/// and would apply a tone curve this data never had.
pub fn encode_tiff_rgb16(width: u32, height: u32, px: &[u16]) -> Result<Vec<u8>, String> {
    let icc = crate::icc::linear_srgb();
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut enc = tiff::encoder::TiffEncoder::new(&mut buf)
            .map_err(|e| format!("tiff encoder init: {e}"))?;
        let mut image = enc
            .new_image::<tiff::encoder::colortype::RGB16>(width, height)
            .map_err(|e| format!("tiff image: {e}"))?;
        image
            .encoder()
            .write_tag(tiff::tags::Tag::Unknown(TAG_ICC), icc.as_slice())
            .map_err(|e| format!("tiff icc tag: {e}"))?;
        image
            .write_data(px)
            .map_err(|e| format!("tiff write: {e}"))?;
    }
    Ok(buf.into_inner())
}

/// Encode an 8-bit grayscale mask as PNG, in memory.
pub fn encode_png_gray8(width: u32, height: u32, px: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| format!("png header: {e}"))?;
        w.write_image_data(px)
            .map_err(|e| format!("png write: {e}"))?;
        w.finish().map_err(|e| format!("png finish: {e}"))?;
    }
    Ok(out)
}

/// Write bytes and return the manifest line for them.
fn write_artifact(root: &Path, rel: &str, bytes: &[u8]) -> Result<ManifestLine, String> {
    let full = root.join(rel);
    if let Some(dir) = full.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    }
    let f = std::fs::File::create(&full).map_err(|e| format!("create {}: {e}", full.display()))?;
    let mut w = BufWriter::new(f);
    w.write_all(bytes)
        .map_err(|e| format!("write {}: {e}", full.display()))?;
    w.flush()
        .map_err(|e| format!("flush {}: {e}", full.display()))?;
    Ok(ManifestLine {
        path: rel.to_string(),
        sha256: sha256_hex(bytes),
    })
}

/// Write every artifact of one rendered suite under `root/{suite}/`.
pub fn write_suite(
    root: &Path,
    suite: &str,
    catalog: &Catalog,
    params: &Params,
    image: &[u16],
    sky_mask: Option<&[u8]>,
) -> Result<Vec<ManifestLine>, String> {
    let (w, h) = (catalog.image.width, catalog.image.height);
    let mut lines = vec![
        write_artifact(
            root,
            &format!("{suite}/image.tiff"),
            &encode_tiff_rgb16(w, h, image)?,
        )?,
        write_artifact(
            root,
            &format!("{suite}/truth.json"),
            &to_json_bytes(catalog).map_err(|e| format!("truth.json: {e}"))?,
        )?,
        write_artifact(
            root,
            &format!("{suite}/params.json"),
            &to_json_bytes(params).map_err(|e| format!("params.json: {e}"))?,
        )?,
    ];
    if let Some(mask) = sky_mask {
        lines.push(write_artifact(
            root,
            &format!("{suite}/sky_mask.png"),
            &encode_png_gray8(w, h, mask)?,
        )?);
    }
    Ok(lines)
}

/// Render the manifest file body: sorted by path, one line per artifact.
pub fn render_manifest(lines: &mut [ManifestLine]) -> Vec<u8> {
    lines.sort_by(|a, b| a.path.cmp(&b.path));
    lines
        .iter()
        .map(ManifestLine::render)
        .collect::<String>()
        .into_bytes()
}

/// Parse a manifest body back into lines (used by the AC tests and, later, ci.sh).
pub fn parse_manifest(body: &str) -> Result<Vec<ManifestLine>, String> {
    body.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let (hash, path) = l
                .split_once("  ")
                .ok_or_else(|| format!("malformed manifest line: {l:?}"))?;
            Ok(ManifestLine {
                path: path.to_string(),
                sha256: hash.to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_the_known_empty_string_digest() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    /// Pins the float serialization policy: `ryu` shortest-round-trip, no
    /// locale, no platform-dependent `%g`. If this changes, every committed hash
    /// changes with it — which is exactly what we want to be told about.
    #[test]
    fn float_json_formatting_is_pinned() {
        #[derive(serde::Serialize)]
        struct F {
            a: f64,
            b: f64,
            c: f64,
            d: f64,
        }
        let bytes = to_json_bytes(&F {
            // Not exactly 0.3: the full f64 expansion must survive.
            a: 0.1 + 0.2,
            // Integral value: must keep the `.0` so it stays a float on re-read.
            b: 1.0,
            // Large magnitude: exponent form, and `e+21` not `e21`.
            c: 1e21,
            // Non-terminating: shortest repr that still round-trips.
            d: 1.0_f64 / 3.0,
        })
        .expect("serialize");
        let s = String::from_utf8(bytes).expect("utf8");
        assert_eq!(
            s,
            "{\n  \"a\": 0.30000000000000004,\n  \"b\": 1.0,\n  \"c\": 1e+21,\n  \
             \"d\": 0.3333333333333333\n}\n"
        );
    }

    #[test]
    fn json_uses_lf_and_ends_with_exactly_one_newline() {
        let bytes = to_json_bytes(&serde_json::json!({"k": [1, 2]})).expect("serialize");
        let s = String::from_utf8(bytes).expect("utf8");
        assert!(!s.contains('\r'), "CR must never reach a hashed artifact");
        assert!(s.ends_with("}\n"));
        assert!(!s.ends_with("}\n\n"));
    }

    #[test]
    fn manifest_is_sorted_by_path_and_round_trips() {
        let mut lines = vec![
            ManifestLine {
                path: "z/image.tiff".into(),
                sha256: "bb".into(),
            },
            ManifestLine {
                path: "a/truth.json".into(),
                sha256: "aa".into(),
            },
        ];
        let body = render_manifest(&mut lines);
        let s = String::from_utf8(body).expect("utf8");
        assert_eq!(s, "aa  a/truth.json\nbb  z/image.tiff\n");
        assert_eq!(parse_manifest(&s).expect("parse"), lines);
    }

    #[test]
    fn manifest_parse_rejects_a_malformed_line() {
        assert!(parse_manifest("deadbeef only-one-space.json").is_err());
    }

    #[test]
    fn tiff_encoding_is_byte_stable_for_the_same_pixels() {
        let px: Vec<u16> = (0..(4 * 3 * 3)).map(|i| (i * 997) as u16).collect();
        let a = encode_tiff_rgb16(4, 3, &px).expect("encode");
        let b = encode_tiff_rgb16(4, 3, &px).expect("encode");
        assert_eq!(
            a, b,
            "tiff encoder must not embed timestamps or padding noise"
        );
    }

    #[test]
    fn png_encoding_is_byte_stable_for_the_same_pixels() {
        let px: Vec<u8> = (0..48u32).map(|i| (i * 5) as u8).collect();
        let a = encode_png_gray8(8, 6, &px).expect("encode");
        let b = encode_png_gray8(8, 6, &px).expect("encode");
        assert_eq!(a, b);
    }
}
