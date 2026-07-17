//! Minimal TIFF IFD reader, used only to lift a TIFF's Exif sub-IFD.
//!
//! Why this exists rather than a byte copy: in a TIFF, the Exif block is a
//! sub-IFD woven into the host file, and any value longer than four bytes lives
//! at an **absolute file offset**. Copying those bytes into a new file would
//! copy pointers into unrelated data. So the values are resolved here, and the
//! encoder writes them out fresh with offsets that are correct for the file it
//! is building. (A JPEG's APP1 block has the opposite property — it is
//! self-contained — and is copied verbatim; see `decode::extract_jpeg_exif`.)
//!
//! Scope is deliberately narrow: enough to preserve what a stacked-and-stretched
//! TIFF actually carries. Nested GPS/Interop IFDs and maker notes are not
//! followed — see `NOT_COPIED`.

use crate::image::{ExifTag, ExifValue};

/// Tags that are pointers to further IFDs, or are meaningless once moved.
///
/// Copying a pointer value would produce a dangling offset in the output, which
/// is worse than dropping it: a reader would follow it into whatever happens to
/// live there. Dropping is visible; a dangling pointer is not.
const NOT_COPIED: &[u16] = &[
    34853, // GPSInfoIFD  — pointer
    40965, // InteropIFD  — pointer
    37500, // MakerNote   — vendor-private, frequently offset-dependent
];

struct Reader<'a> {
    b: &'a [u8],
    le: bool,
}

impl<'a> Reader<'a> {
    fn u16(&self, at: usize) -> Option<u16> {
        let s = self.b.get(at..at + 2)?;
        let a = [s[0], s[1]];
        Some(if self.le {
            u16::from_le_bytes(a)
        } else {
            u16::from_be_bytes(a)
        })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let s = self.b.get(at..at + 4)?;
        let a = [s[0], s[1], s[2], s[3]];
        Some(if self.le {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        })
    }
}

fn type_size(t: u16) -> Option<usize> {
    Some(match t {
        1 | 2 | 6 | 7 => 1, // BYTE, ASCII, SBYTE, UNDEFINED
        3 | 8 => 2,         // SHORT, SSHORT
        4 | 9 | 13 => 4,    // LONG, SLONG, IFD
        5 | 10 => 8,        // RATIONAL, SRATIONAL
        11 => 4,            // FLOAT
        12 => 8,            // DOUBLE
        _ => return None,
    })
}

/// Read a TIFF's Exif sub-IFD into resolved tag values.
///
/// Returns `None` when the file has no Exif IFD, or when anything about it does
/// not parse. Metadata is worth preserving but never worth failing an image
/// over: the pixels are the deliverable.
pub(crate) fn read_tiff_exif_ifd(bytes: &[u8]) -> Option<Vec<ExifTag>> {
    let le = match bytes.get(0..4)? {
        b"II\x2a\x00" => true,
        b"MM\x00\x2a" => false,
        _ => return None,
    };
    let r = Reader { b: bytes, le };

    let ifd0 = r.u32(4)? as usize;
    let exif_off = find_tag_value_u32(&r, ifd0, 34665)? as usize;
    read_ifd(&r, exif_off)
}

/// Find a scalar LONG/SHORT tag in an IFD — used for the sub-IFD pointer.
fn find_tag_value_u32(r: &Reader, ifd: usize, want: u16) -> Option<u32> {
    let n = r.u16(ifd)? as usize;
    for i in 0..n {
        let e = ifd + 2 + i * 12;
        if r.u16(e)? == want {
            let t = r.u16(e + 2)?;
            return match t {
                3 => r.u16(e + 8).map(u32::from),
                4 | 13 => r.u32(e + 8),
                _ => None,
            };
        }
    }
    None
}

fn read_ifd(r: &Reader, ifd: usize) -> Option<Vec<ExifTag>> {
    let n = r.u16(ifd)? as usize;
    // An IFD entry is 12 bytes; a corrupt count must not send us off the end.
    if ifd + 2 + n * 12 > r.b.len() {
        return None;
    }

    let mut out = Vec::new();
    for i in 0..n {
        let e = ifd + 2 + i * 12;
        let tag = r.u16(e)?;
        let ty = r.u16(e + 2)?;
        let count = r.u32(e + 4)? as usize;

        if NOT_COPIED.contains(&tag) {
            continue;
        }
        let Some(tsize) = type_size(ty) else { continue };
        let total = tsize.checked_mul(count)?;

        // Values of four bytes or fewer are inline; longer ones are at an offset.
        let data: &[u8] = if total <= 4 {
            r.b.get(e + 8..e + 8 + total)?
        } else {
            let off = r.u32(e + 8)? as usize;
            match r.b.get(off..off.checked_add(total)?) {
                Some(d) => d,
                // A pointer past the end means a truncated or lying file. Skip
                // the tag rather than abandoning the rest of the block.
                None => continue,
            }
        };

        let sub = Reader { b: data, le: r.le };
        let value = match ty {
            2 => ExifValue::Ascii(
                String::from_utf8_lossy(data.split(|&c| c == 0).next().unwrap_or(data))
                    .into_owned(),
            ),
            1 => ExifValue::Byte(data.to_vec()),
            7 => ExifValue::Undefined(data.to_vec()),
            3 => ExifValue::Short((0..count).filter_map(|k| sub.u16(k * 2)).collect()),
            4 => ExifValue::Long((0..count).filter_map(|k| sub.u32(k * 4)).collect()),
            5 => ExifValue::Rational(
                (0..count)
                    .filter_map(|k| Some((sub.u32(k * 8)?, sub.u32(k * 8 + 4)?)))
                    .collect(),
            ),
            10 => ExifValue::SRational(
                (0..count)
                    .filter_map(|k| Some((sub.u32(k * 8)? as i32, sub.u32(k * 8 + 4)? as i32)))
                    .collect(),
            ),
            _ => continue,
        };
        out.push(ExifTag { tag, value });
    }
    // Sorted by tag: the TIFF spec requires ascending tag order in an IFD, and
    // it also makes the output deterministic (INV-2) regardless of input order.
    out.sort_by_key(|t| t.tag);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a little-endian TIFF whose IFD0 points at an Exif sub-IFD.
    fn tiff_with_exif(entries: &[(u16, u16, &[u8])]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"II\x2a\x00");
        b.extend_from_slice(&8u32.to_le_bytes()); // IFD0 at 8

        // IFD0: one entry, the Exif pointer.
        let ifd0_at = b.len();
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&34665u16.to_le_bytes());
        b.extend_from_slice(&4u16.to_le_bytes()); // LONG
        b.extend_from_slice(&1u32.to_le_bytes());
        let exif_ptr_at = b.len();
        b.extend_from_slice(&0u32.to_le_bytes()); // patched below
        b.extend_from_slice(&0u32.to_le_bytes()); // next IFD = none
        let _ = ifd0_at;

        // Exif IFD.
        let exif_at = b.len() as u32;
        b[exif_ptr_at..exif_ptr_at + 4].copy_from_slice(&exif_at.to_le_bytes());
        b.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        let mut patches = Vec::new();
        for (tag, ty, data) in entries {
            b.extend_from_slice(&tag.to_le_bytes());
            b.extend_from_slice(&ty.to_le_bytes());
            let tsize = type_size(*ty).expect("known type");
            b.extend_from_slice(&((data.len() / tsize) as u32).to_le_bytes());
            if data.len() <= 4 {
                let mut v = data.to_vec();
                v.resize(4, 0);
                b.extend_from_slice(&v);
            } else {
                patches.push((b.len(), data.to_vec()));
                b.extend_from_slice(&0u32.to_le_bytes());
            }
        }
        b.extend_from_slice(&0u32.to_le_bytes()); // next IFD
        for (at, data) in patches {
            let off = b.len() as u32;
            b[at..at + 4].copy_from_slice(&off.to_le_bytes());
            b.extend_from_slice(&data);
        }
        b
    }

    #[test]
    fn reads_an_inline_short_value() {
        let t = tiff_with_exif(&[(33434, 3, &100u16.to_le_bytes())]);
        let tags = read_tiff_exif_ifd(&t).expect("exif");
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].tag, 33434);
        assert_eq!(tags[0].value, ExifValue::Short(vec![100]));
    }

    #[test]
    fn reads_an_offset_ascii_value() {
        let t = tiff_with_exif(&[(305, 2, b"StarKit Test Software\0")]);
        let tags = read_tiff_exif_ifd(&t).expect("exif");
        assert_eq!(
            tags[0].value,
            ExifValue::Ascii("StarKit Test Software".into())
        );
    }

    #[test]
    fn reads_a_rational() {
        let mut d = Vec::new();
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&200u32.to_le_bytes());
        let t = tiff_with_exif(&[(33434, 5, &d)]);
        let tags = read_tiff_exif_ifd(&t).expect("exif");
        assert_eq!(tags[0].value, ExifValue::Rational(vec![(1, 200)]));
    }

    #[test]
    fn pointer_tags_are_dropped_rather_than_copied_as_dangling_offsets() {
        let t = tiff_with_exif(&[
            (34853, 4, &1234u32.to_le_bytes()), // GPS IFD pointer
            (33434, 3, &100u16.to_le_bytes()),
        ]);
        let tags = read_tiff_exif_ifd(&t).expect("exif");
        assert_eq!(tags.len(), 1, "a pointer tag was copied: {tags:?}");
        assert_eq!(tags[0].tag, 33434);
    }

    #[test]
    fn tags_come_back_in_ascending_order() {
        let t = tiff_with_exif(&[
            (40962, 3, &10u16.to_le_bytes()),
            (33434, 3, &20u16.to_le_bytes()),
            (34855, 3, &30u16.to_le_bytes()),
        ]);
        let tags = read_tiff_exif_ifd(&t).expect("exif");
        let order: Vec<u16> = tags.iter().map(|t| t.tag).collect();
        assert_eq!(order, vec![33434, 34855, 40962]);
    }

    #[test]
    fn a_tiff_without_an_exif_ifd_yields_none() {
        let mut b = Vec::new();
        b.extend_from_slice(b"II\x2a\x00");
        b.extend_from_slice(&8u32.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(read_tiff_exif_ifd(&b), None);
    }

    #[test]
    fn garbage_is_none_not_a_panic() {
        assert_eq!(read_tiff_exif_ifd(b"not a tiff at all"), None);
        assert_eq!(read_tiff_exif_ifd(&[]), None);
    }

    #[test]
    fn a_value_pointing_past_the_end_skips_that_tag_only() {
        let mut t = tiff_with_exif(&[
            (305, 2, b"a long software name\0"),
            (33434, 3, &7u16.to_le_bytes()),
        ]);
        // Corrupt the ASCII tag's offset; the SHORT tag must still survive.
        // IFD0 layout: [0..4] magic, [4..8] IFD0 offset, [8..10] count,
        // [10..12] tag, [12..14] type, [14..18] count, [18..22] the pointer.
        let exif_at = u32::from_le_bytes(t[18..22].try_into().expect("slice")) as usize;
        let first_entry = exif_at + 2;
        t[first_entry + 8..first_entry + 12].copy_from_slice(&9_000_000u32.to_le_bytes());
        let tags = read_tiff_exif_ifd(&t).expect("exif");
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].tag, 33434);
    }
}
