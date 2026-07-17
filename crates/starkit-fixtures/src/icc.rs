//! A linear-sRGB ICC profile, built byte-by-byte (D-032).
//!
//! The fixtures are linear by construction, but until now they said so only in
//! their truth catalog — the TIFF itself carried no profile. Any honest decoder
//! must then guess, and `starkit-io` guesses sRGB (its documented rule for
//! untagged input), applying a curve this data never had. Neither side was
//! wrong; the file simply did not state its own colour space. Now it does.
//!
//! The profile is a standard **matrix-shaper RGB** profile: sRGB primaries
//! (Bradford-adapted to the D50 PCS, exactly as the reference sRGB profile does)
//! with an **identity tone curve**. "Linear sRGB" is a real, ordinary colour
//! space, not an invention.
//!
//! # Determinism (INV-2)
//!
//! Every byte is a constant. In particular the header's creation-date field
//! (offset 24) is fixed at zero rather than filled from the clock — a
//! wall-clock read here would make every regenerated fixture a different file
//! and break the committed manifest on every run. The profile ID (offset 84) is
//! likewise left zero, which the spec permits.
//!
//! Hand-built rather than pulled from a crate: the profile is ~500 bytes of
//! fixed data, and a dependency would put a version bump between us and the
//! bytes the manifest pins.

/// s15Fixed16Number: the ICC fixed-point encoding, value × 65536 as a big-endian
/// i32.
fn s15f16(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

fn xyz_tag(x: f64, y: f64, z: f64) -> Vec<u8> {
    let mut t = Vec::with_capacity(20);
    t.extend_from_slice(b"XYZ ");
    t.extend_from_slice(&0u32.to_be_bytes()); // reserved
    t.extend_from_slice(&s15f16(x));
    t.extend_from_slice(&s15f16(y));
    t.extend_from_slice(&s15f16(z));
    t
}

/// `curv` with a zero-length table — the ICC spec's own spelling of "identity".
///
/// Deliberately not `curv` with gamma 1.0: a count-0 curve is unambiguous, while
/// a gamma value invites a decoder to run `powf(v, 1.0)` and hand back something
/// a hair off v.
fn linear_trc_tag() -> Vec<u8> {
    let mut t = Vec::with_capacity(12);
    t.extend_from_slice(b"curv");
    t.extend_from_slice(&0u32.to_be_bytes()); // reserved
    t.extend_from_slice(&0u32.to_be_bytes()); // count = 0 => identity
    t
}

/// ICC v2 `textDescriptionType`. Clunky by modern standards, but it is what a v2
/// profile requires and what every reader understands.
fn desc_tag(text: &str) -> Vec<u8> {
    let ascii = format!("{text}\0");
    let mut t = Vec::new();
    t.extend_from_slice(b"desc");
    t.extend_from_slice(&0u32.to_be_bytes()); // reserved
    t.extend_from_slice(&(ascii.len() as u32).to_be_bytes());
    t.extend_from_slice(ascii.as_bytes());
    t.extend_from_slice(&0u32.to_be_bytes()); // unicode language code
    t.extend_from_slice(&0u32.to_be_bytes()); // unicode count
    t.extend_from_slice(&0u16.to_be_bytes()); // scriptcode code
    t.push(0); // scriptcode count
    t.extend_from_slice(&[0u8; 67]); // scriptcode text
    t
}

fn text_tag(text: &str) -> Vec<u8> {
    let mut t = Vec::new();
    t.extend_from_slice(b"text");
    t.extend_from_slice(&0u32.to_be_bytes());
    t.extend_from_slice(text.as_bytes());
    t.push(0);
    t
}

/// Build the linear-sRGB profile the fixtures embed.
pub fn linear_srgb() -> Vec<u8> {
    // sRGB primaries chromatically adapted to D50, as in the reference sRGB
    // profile. Only the TRC differs from sRGB — these are the same primaries.
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", desc_tag("StarKit linear sRGB")),
        (b"wtpt", xyz_tag(0.964_202_881, 1.0, 0.824_905_395)), // D50
        (
            b"rXYZ",
            xyz_tag(0.436_065_674, 0.222_488_403, 0.013_916_016),
        ),
        (
            b"gXYZ",
            xyz_tag(0.385_147_095, 0.716_873_169, 0.097_076_416),
        ),
        (
            b"bXYZ",
            xyz_tag(0.143_066_406, 0.060_607_910, 0.714_096_069),
        ),
        (b"rTRC", linear_trc_tag()),
        (b"gTRC", linear_trc_tag()),
        (b"bTRC", linear_trc_tag()),
        (b"cprt", text_tag("Public Domain")),
    ];

    let header_len = 128usize;
    let table_len = 4 + tags.len() * 12;
    let mut data_at = header_len + table_len;

    // Tag data must start on a 4-byte boundary.
    let mut table = Vec::with_capacity(table_len);
    let mut blob = Vec::new();
    table.extend_from_slice(&(tags.len() as u32).to_be_bytes());
    for (sig, body) in &tags {
        let pad = (4 - (data_at % 4)) % 4;
        blob.resize(blob.len() + pad, 0);
        data_at += pad;
        table.extend_from_slice(*sig);
        table.extend_from_slice(&(data_at as u32).to_be_bytes());
        table.extend_from_slice(&(body.len() as u32).to_be_bytes());
        blob.extend_from_slice(body);
        data_at += body.len();
    }

    let total = header_len + table.len() + blob.len();
    let mut p = vec![0u8; header_len];
    p[0..4].copy_from_slice(&(total as u32).to_be_bytes()); // profile size
    p[4..8].copy_from_slice(&0u32.to_be_bytes()); // preferred CMM: none
    p[8..12].copy_from_slice(&0x0210_0000u32.to_be_bytes()); // version 2.1
    p[12..16].copy_from_slice(b"mntr"); // device class: display
    p[16..20].copy_from_slice(b"RGB "); // data colour space
    p[20..24].copy_from_slice(b"XYZ "); // PCS
                                        // p[24..36] creation date: left zero on purpose — see the module docs.
    p[36..40].copy_from_slice(b"acsp"); // file signature
                                        // p[40..68]: platform, flags, manufacturer, model, attributes — all zero.
    p[64..68].copy_from_slice(&0u32.to_be_bytes()); // rendering intent: perceptual
    p[68..72].copy_from_slice(&s15f16(0.964_202_881)); // PCS illuminant X (D50)
    p[72..76].copy_from_slice(&s15f16(1.0)); // Y
    p[76..80].copy_from_slice(&s15f16(0.824_905_395)); // Z
                                                       // p[80..128]: creator, profile ID, reserved — all zero.

    p.extend_from_slice(&table);
    p.extend_from_slice(&blob);
    debug_assert_eq!(p.len(), total, "declared profile size must match the bytes");
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be_u32(b: &[u8], at: usize) -> u32 {
        u32::from_be_bytes(b[at..at + 4].try_into().expect("4 bytes"))
    }

    #[test]
    fn declared_size_matches_the_actual_bytes() {
        let p = linear_srgb();
        assert_eq!(be_u32(&p, 0) as usize, p.len());
    }

    #[test]
    fn header_declares_an_rgb_display_profile() {
        let p = linear_srgb();
        assert_eq!(&p[12..16], b"mntr");
        assert_eq!(&p[16..20], b"RGB ");
        assert_eq!(&p[20..24], b"XYZ ");
        assert_eq!(&p[36..40], b"acsp", "missing the ICC file signature");
    }

    /// INV-2: the one field that would otherwise be filled from the clock.
    #[test]
    fn the_creation_date_is_zero_not_the_wall_clock() {
        let p = linear_srgb();
        assert!(
            p[24..36].iter().all(|&b| b == 0),
            "a timestamp in the profile would make every regeneration a new file"
        );
    }

    #[test]
    fn is_byte_identical_across_calls() {
        assert_eq!(linear_srgb(), linear_srgb());
    }

    #[test]
    fn carries_all_three_trc_tags_and_they_are_identity() {
        let p = linear_srgb();
        let n = be_u32(&p, 128) as usize;
        let mut found = 0;
        for i in 0..n {
            let at = 132 + i * 12;
            let sig = &p[at..at + 4];
            if sig == b"rTRC" || sig == b"gTRC" || sig == b"bTRC" {
                let off = be_u32(&p, at + 4) as usize;
                let size = be_u32(&p, at + 8) as usize;
                let body = &p[off..off + size];
                assert_eq!(&body[0..4], b"curv");
                assert_eq!(be_u32(body, 8), 0, "TRC must be a count-0 identity curve");
                found += 1;
            }
        }
        assert_eq!(found, 3, "profile must define rTRC, gTRC and bTRC");
    }

    #[test]
    fn every_tag_starts_on_a_four_byte_boundary_and_lies_inside_the_profile() {
        let p = linear_srgb();
        let n = be_u32(&p, 128) as usize;
        for i in 0..n {
            let at = 132 + i * 12;
            let off = be_u32(&p, at + 4) as usize;
            let size = be_u32(&p, at + 8) as usize;
            assert_eq!(off % 4, 0, "tag {i} is not 4-byte aligned");
            assert!(
                off + size <= p.len(),
                "tag {i} runs past the end of the profile"
            );
        }
    }

    #[test]
    fn carries_the_matrix_columns_a_shaper_profile_needs() {
        let p = linear_srgb();
        let n = be_u32(&p, 128) as usize;
        let sigs: Vec<&[u8]> = (0..n).map(|i| &p[132 + i * 12..132 + i * 12 + 4]).collect();
        for want in [b"rXYZ", b"gXYZ", b"bXYZ", b"wtpt", b"desc", b"cprt"] {
            assert!(
                sigs.contains(&&want[..]),
                "missing required tag {:?}",
                std::str::from_utf8(want)
            );
        }
    }
}
