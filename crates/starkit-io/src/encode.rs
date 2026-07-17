//! Encoding: linear f32 RGB → 16-bit TIFF, ICC and EXIF restored.
//!
//! De-linearizes through **the same curve** the decoder used, taken from
//! `Meta::trc` rather than re-derived — that symmetry is what makes FR-1's
//! pixel-identical no-op hold by construction instead of by luck.
//!
//! Output is always 16-bit, including for an 8-bit source: that is FR-1's stated
//! default, and widening never loses information.

use std::io::Cursor;
use std::path::Path;

use crate::error::{IoError, Result};
use crate::icc::Trc;
use crate::image::{Exif, ExifTag, ExifValue, Image};

const TAG_ICC: u16 = 34675;
const TAG_EXIF_IFD: u16 = 34665;

/// Encode to a 16-bit TIFF in memory.
pub fn encode_tiff16(img: &Image, path: &Path) -> Result<Vec<u8>> {
    let samples = delinearize(&img.pixels, &img.meta.trc);
    write_tiff(img, &samples, path)
}

/// Encode and write atomically. The only way an image reaches disk.
pub fn write_tiff16(img: &Image, path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let bytes = encode_tiff16(img, path)?;
    crate::atomic::atomic_write(path, &bytes)
}

/// Linear f32 → 16-bit codes, through the inverse of the decode curve.
///
/// `round()` rather than truncation: truncation would lose a half-LSB on every
/// pixel and break the round-trip outright. Values outside [0, 1] are clamped —
/// a 16-bit file cannot hold them, and wrapping would be worse than clipping.
fn delinearize(pixels: &[f32], trc: &Trc) -> Vec<u16> {
    pixels
        .iter()
        .enumerate()
        .map(|(i, &x)| {
            let v = trc.channel(i % 3).from_linear(x as f64);
            (v * 65535.0).round().clamp(0.0, 65535.0) as u16
        })
        .collect()
}

fn write_tiff(img: &Image, samples: &[u16], path: &Path) -> Result<Vec<u8>> {
    use tiff::encoder::{colortype, TiffEncoder};
    use tiff::tags::Tag;

    let io = |e: tiff::TiffError| IoError::decode("tiff", path, e);

    let mut buf = Cursor::new(Vec::new());
    {
        let mut enc = TiffEncoder::new(&mut buf).map_err(io)?;

        // EXIF first: the sub-IFD must exist before IFD0 can point at it.
        let exif_ptr = match &img.meta.exif {
            Some(Exif::Tags(tags)) if !tags.is_empty() => {
                let mut dir = enc.extra_directory().map_err(io)?;
                for t in tags {
                    write_exif_tag(&mut dir, t).map_err(io)?;
                }
                Some(dir.finish_with_offsets().map_err(io)?)
            }
            // A JPEG's APP1 block is a whole TIFF structure of its own. Splicing
            // it into this file's directory tree would need its internal offsets
            // rebased; we do not, so it is dropped rather than written broken.
            _ => None,
        };

        let mut image = enc
            .new_image::<colortype::RGB16>(img.width, img.height)
            .map_err(io)?;
        {
            let dir = image.encoder();
            if let Some(icc) = &img.meta.icc {
                dir.write_tag(Tag::Unknown(TAG_ICC), icc.as_slice())
                    .map_err(io)?;
            }
            if let Some(ptr) = exif_ptr {
                dir.write_tag(Tag::Unknown(TAG_EXIF_IFD), ptr.offset)
                    .map_err(io)?;
            }
        }
        image.write_data(samples).map_err(io)?;
    }
    Ok(buf.into_inner())
}

fn write_exif_tag<W, K>(
    dir: &mut tiff::encoder::DirectoryEncoder<'_, W, K>,
    t: &ExifTag,
) -> std::result::Result<(), tiff::TiffError>
where
    W: std::io::Write + std::io::Seek,
    K: tiff::encoder::TiffKind,
{
    use tiff::tags::Tag;
    let tag = Tag::Unknown(t.tag);
    match &t.value {
        ExifValue::Ascii(s) => dir.write_tag(tag, s.as_str()),
        ExifValue::Short(v) => dir.write_tag(tag, v.as_slice()),
        ExifValue::Long(v) => dir.write_tag(tag, v.as_slice()),
        ExifValue::Byte(v) | ExifValue::Undefined(v) => dir.write_tag(tag, v.as_slice()),
        ExifValue::Rational(v) => {
            let r: Vec<tiff::encoder::Rational> = v
                .iter()
                .map(|&(n, d)| tiff::encoder::Rational { n, d })
                .collect();
            dir.write_tag(tag, r.as_slice())
        }
        ExifValue::SRational(v) => {
            let r: Vec<tiff::encoder::SRational> = v
                .iter()
                .map(|&(n, d)| tiff::encoder::SRational { n, d })
                .collect();
            dir.write_tag(tag, r.as_slice())
        }
    }
}
