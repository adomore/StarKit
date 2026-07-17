//! Decoding: TIFF 8/16, PNG, JPEG → linear f32 RGB (starkit-io/CLAUDE.md).
//!
//! Input files are opened read-only and never written. Format is chosen by
//! sniffing the magic bytes, not by extension: a `.tif` that is really a JPEG is
//! a thing that happens, and trusting the name would produce a confusing
//! `Decode { format: "tiff" }` for a perfectly good JPEG.

use std::io::Cursor;
use std::path::Path;

use crate::error::{IoError, Result};
use crate::icc::{self, Trc};
use crate::image::{Exif, Image, Meta};

/// TIFF tag 34675 — the embedded ICC profile.
const TAG_ICC: u16 = 34675;
/// TIFF tag 34665 — pointer to the Exif sub-IFD.
const TAG_EXIF_IFD: u16 = 34665;

/// Sniffed input format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Tiff,
    Png,
    Jpeg,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Tiff => "tiff",
            Format::Png => "png",
            Format::Jpeg => "jpeg",
        }
    }

    /// Identify by magic bytes.
    pub fn sniff(bytes: &[u8]) -> Option<Format> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Format::Png)
        } else if bytes.starts_with(b"\xff\xd8\xff") {
            Some(Format::Jpeg)
        } else if bytes.starts_with(b"II\x2a\x00") || bytes.starts_with(b"MM\x00\x2a") {
            Some(Format::Tiff)
        } else {
            None
        }
    }
}

/// Decode an image file to linear f32 RGB.
pub fn decode(path: impl AsRef<Path>) -> Result<Image> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|e| IoError::io(path, e))?;
    decode_bytes(&bytes, path)
}

/// Import an external grayscale mask (FR-3's manual path).
///
/// Returns coverage in `[0, 1]`, row-major, plus its dimensions. Accepts 8- or
/// 16-bit grayscale and RGB — a mask painted in Photoshop and saved without
/// flattening to grayscale is the normal case, not an error, so an RGB file is
/// read via its first channel rather than rejected.
///
/// **No tone curve is applied.** A mask is a coverage fraction, not light: 50 %
/// grey means half-selected, and running it through an sRGB curve would silently
/// turn that into 21 %.
pub fn decode_mask(path: impl AsRef<Path>) -> Result<(Vec<f32>, u32, u32)> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|e| IoError::io(path, e))?;
    match Format::sniff(&bytes) {
        Some(Format::Png) => decode_mask_png(&bytes, path),
        Some(Format::Tiff) => decode_mask_tiff(&bytes, path),
        Some(Format::Jpeg) => Err(IoError::layout(
            path,
            "a JPEG mask is lossy: its blocking artifacts would move the gate              boundary. Export the mask as PNG or TIFF.",
        )),
        None => Err(IoError::layout(path, "unrecognised mask file type")),
    }
}

fn decode_mask_png(bytes: &[u8], path: &Path) -> Result<(Vec<f32>, u32, u32)> {
    let mut dec = png::Decoder::new(Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec
        .read_info()
        .map_err(|e| IoError::decode("png", path, e))?;
    let mut buf = vec![0u8; reader.output_buffer_size().unwrap_or(0)];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| IoError::decode("png", path, e))?;

    let channels = match info.color_type {
        png::ColorType::Grayscale => 1usize,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        other => {
            return Err(IoError::layout(
                path,
                format!("unsupported mask colour type {other:?}"),
            ))
        }
    };
    let raw = &buf[..info.buffer_size()];
    let n = info.width as usize * info.height as usize;
    let max = match info.bit_depth {
        png::BitDepth::Eight => 255.0f32,
        png::BitDepth::Sixteen => 65535.0,
        other => {
            return Err(IoError::layout(
                path,
                format!("unsupported mask bit depth {other:?}"),
            ))
        }
    };
    let sixteen = matches!(info.bit_depth, png::BitDepth::Sixteen);
    let data: Vec<f32> = (0..n)
        .map(|i| {
            let k = i * channels;
            let v = if sixteen {
                u16::from_be_bytes([raw[k * 2], raw[k * 2 + 1]]) as f32
            } else {
                raw[k] as f32
            };
            v / max
        })
        .collect();
    Ok((data, info.width, info.height))
}

fn decode_mask_tiff(bytes: &[u8], path: &Path) -> Result<(Vec<f32>, u32, u32)> {
    use tiff::decoder::{Decoder, DecodingResult};
    let mut d = Decoder::new(Cursor::new(bytes))
        .map_err(|e| IoError::decode("tiff", path, e))?
        .with_limits(tiff::decoder::Limits::unlimited());
    let (w, h) = d
        .dimensions()
        .map_err(|e| IoError::decode("tiff", path, e))?;
    let (channels, max) = match d
        .colortype()
        .map_err(|e| IoError::decode("tiff", path, e))?
    {
        tiff::ColorType::Gray(8) => (1usize, 255.0f64),
        tiff::ColorType::Gray(16) => (1, 65535.0),
        tiff::ColorType::RGB(8) => (3, 255.0),
        tiff::ColorType::RGB(16) => (3, 65535.0),
        other => {
            return Err(IoError::layout(
                path,
                format!("unsupported mask layout {other:?}"),
            ))
        }
    };
    let samples: Vec<f64> = match d
        .read_image()
        .map_err(|e| IoError::decode("tiff", path, e))?
    {
        DecodingResult::U8(v) => v.into_iter().map(f64::from).collect(),
        DecodingResult::U16(v) => v.into_iter().map(f64::from).collect(),
        _ => {
            return Err(IoError::layout(
                path,
                "expected 8- or 16-bit integer samples",
            ))
        }
    };
    let n = w as usize * h as usize;
    let data: Vec<f32> = (0..n)
        .map(|i| (samples[i * channels] / max) as f32)
        .collect();
    Ok((data, w, h))
}

/// Decode from memory. `path` is only used for error messages.
pub fn decode_bytes(bytes: &[u8], path: &Path) -> Result<Image> {
    match Format::sniff(bytes) {
        Some(Format::Tiff) => decode_tiff(bytes, path),
        Some(Format::Png) => decode_png(bytes, path),
        Some(Format::Jpeg) => decode_jpeg(bytes, path),
        None => Err(IoError::layout(
            path,
            "unrecognised file type (not TIFF, PNG or JPEG)",
        )),
    }
}

/// Build the metadata, resolving which curve to linearize with.
///
/// An untagged file is assumed sRGB *and says so* in `warnings`: the assumption
/// is usually right and always a guess, so it travels with the image rather than
/// being forgotten at the boundary.
fn meta_for(
    icc_bytes: Option<Vec<u8>>,
    exif: Option<Exif>,
    bit_depth: u8,
    path: &Path,
) -> Result<Meta> {
    let mut warnings = Vec::new();
    let trc = match &icc_bytes {
        None => {
            warnings.push("no ICC profile; assuming sRGB".to_string());
            Trc::srgb()
        }
        Some(raw) => match icc::parse_trc(raw, path)? {
            Some(trc) => trc,
            None => {
                // A LUT-based profile. We keep the bytes and re-attach them, so
                // the colour is not lost — but we cannot read a curve out of it,
                // and guessing sRGB silently would be a lie about the file.
                warnings.push(
                    "ICC profile has no tone curve (rTRC/gTRC/bTRC); assuming sRGB for \
                     linearization. The profile itself is preserved unchanged."
                        .to_string(),
                );
                Trc::srgb()
            }
        },
    };
    Ok(Meta {
        icc: icc_bytes,
        exif,
        trc,
        source_bit_depth: bit_depth,
        warnings,
    })
}

/// Map integer samples to linear f32 through the per-channel curve.
fn linearize(samples: &[u32], max: f64, trc: &Trc) -> Vec<f32> {
    // A 65536-entry LUT per channel: powf() 75 million times is minutes of
    // wasted CPU on a 61 MP frame, and the input domain is tiny.
    let n = max as usize + 1;
    let luts: Vec<Vec<f32>> = (0..3)
        .map(|c| {
            let curve = trc.channel(c);
            (0..=max as u32)
                .map(|v| curve.to_linear(v as f64 / max) as f32)
                .collect::<Vec<f32>>()
        })
        .collect();
    debug_assert!(luts.iter().all(|l| l.len() == n));

    samples
        .iter()
        .enumerate()
        .map(|(i, &v)| luts[i % 3][(v as usize).min(n - 1)])
        .collect()
}

// ---------------------------------------------------------------------------
// TIFF
// ---------------------------------------------------------------------------

fn decode_tiff(bytes: &[u8], path: &Path) -> Result<Image> {
    use tiff::decoder::{Decoder, DecodingResult};

    let mut d = Decoder::new(Cursor::new(bytes))
        .map_err(|e| IoError::decode("tiff", path, e))?
        .with_limits(tiff::decoder::Limits::unlimited());

    let (w, h) = d
        .dimensions()
        .map_err(|e| IoError::decode("tiff", path, e))?;
    let colortype = d
        .colortype()
        .map_err(|e| IoError::decode("tiff", path, e))?;

    let (bit_depth, max) = match colortype {
        tiff::ColorType::RGB(8) => (8u8, 255.0f64),
        tiff::ColorType::RGB(16) => (16u8, 65535.0f64),
        other => {
            return Err(IoError::layout(
                path,
                format!("expected 8- or 16-bit RGB, found {other:?}"),
            ))
        }
    };

    let icc_bytes = read_tag_bytes(&mut d, TAG_ICC);
    let exif = read_tiff_exif(&mut d, bytes);

    let img = d
        .read_image()
        .map_err(|e| IoError::decode("tiff", path, e))?;
    let samples: Vec<u32> = match img {
        DecodingResult::U8(v) => v.into_iter().map(u32::from).collect(),
        DecodingResult::U16(v) => v.into_iter().map(u32::from).collect(),
        _ => {
            return Err(IoError::layout(
                path,
                "expected 8- or 16-bit integer samples",
            ))
        }
    };

    let expected = w as usize * h as usize * 3;
    if samples.len() != expected {
        return Err(IoError::decode(
            "tiff",
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("expected {expected} samples, decoded {}", samples.len()),
            ),
        ));
    }

    let meta = meta_for(icc_bytes, exif, bit_depth, path)?;
    Ok(Image {
        width: w,
        height: h,
        pixels: linearize(&samples, max, &meta.trc),
        meta,
    })
}

fn read_tag_bytes<R: std::io::Read + std::io::Seek>(
    d: &mut tiff::decoder::Decoder<R>,
    tag: u16,
) -> Option<Vec<u8>> {
    d.find_tag(tiff::tags::Tag::Unknown(tag))
        .ok()
        .flatten()
        .and_then(|v| v.into_u8_vec().ok())
        .filter(|v| !v.is_empty())
}

/// Lift the Exif sub-IFD out of a TIFF as resolved tag values.
///
/// Not a verbatim byte copy, deliberately: a TIFF's Exif IFD is stitched into
/// the file with absolute offsets, so copying its bytes into a new file would
/// produce pointers into nothing. Resolving the values here means the encoder
/// can rewrite them with correct offsets. Failure is silent-but-recorded rather
/// than fatal — a broken Exif block should not cost the photographer the image.
fn read_tiff_exif<R: std::io::Read + std::io::Seek>(
    d: &mut tiff::decoder::Decoder<R>,
    _bytes: &[u8],
) -> Option<Exif> {
    // The sub-IFD pointer must exist for there to be anything to read.
    d.find_tag(tiff::tags::Tag::Unknown(TAG_EXIF_IFD))
        .ok()
        .flatten()?;
    // Reading the sub-IFD itself needs offset-level access the `tiff` decoder
    // does not expose; see `exif.rs`.
    crate::exif::read_tiff_exif_ifd(_bytes).map(Exif::Tags)
}

// ---------------------------------------------------------------------------
// PNG
// ---------------------------------------------------------------------------

fn decode_png(bytes: &[u8], path: &Path) -> Result<Image> {
    let mut dec = png::Decoder::new(Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec
        .read_info()
        .map_err(|e| IoError::decode("png", path, e))?;

    let icc_bytes = reader
        .info()
        .icc_profile
        .as_ref()
        .map(|c| c.clone().into_owned());
    let exif = reader
        .info()
        .exif_metadata
        .as_ref()
        .map(|e| Exif::Tiff(e.clone().into_owned()));

    let mut buf = vec![0u8; reader.output_buffer_size().unwrap_or(0)];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| IoError::decode("png", path, e))?;

    let (bit_depth, max) = match info.bit_depth {
        png::BitDepth::Eight => (8u8, 255.0f64),
        png::BitDepth::Sixteen => (16u8, 65535.0f64),
        other => {
            return Err(IoError::layout(
                path,
                format!("expected 8- or 16-bit PNG, found {other:?}"),
            ))
        }
    };

    let channels = match info.color_type {
        png::ColorType::Rgb => 3usize,
        png::ColorType::Rgba => 4,
        other => {
            return Err(IoError::layout(
                path,
                format!("expected RGB or RGBA PNG, found {other:?}"),
            ))
        }
    };

    let raw = &buf[..info.buffer_size()];
    let mut samples = Vec::with_capacity(info.width as usize * info.height as usize * 3);
    let px = |i: usize| -> u32 {
        if bit_depth == 16 {
            u16::from_be_bytes([raw[i * 2], raw[i * 2 + 1]]) as u32
        } else {
            raw[i] as u32
        }
    };
    let n = info.width as usize * info.height as usize;
    for p in 0..n {
        for c in 0..3 {
            // Alpha is dropped, not composited: this tool has no opinion about
            // what a transparent star should look like, and inventing a
            // background would be one.
            samples.push(px(p * channels + c));
        }
    }

    let meta = meta_for(icc_bytes, exif, bit_depth, path)?;
    Ok(Image {
        width: info.width,
        height: info.height,
        pixels: linearize(&samples, max, &meta.trc),
        meta,
    })
}

// ---------------------------------------------------------------------------
// JPEG
// ---------------------------------------------------------------------------

fn decode_jpeg(bytes: &[u8], path: &Path) -> Result<Image> {
    let mut d = jpeg_decoder::Decoder::new(Cursor::new(bytes));
    let pixels = d.decode().map_err(|e| IoError::decode("jpeg", path, e))?;
    let info = d
        .info()
        .ok_or_else(|| IoError::layout(path, "JPEG decoded but reported no frame info"))?;

    let icc_bytes = d.icc_profile();
    let exif = extract_jpeg_exif(bytes).map(Exif::Tiff);

    let samples: Vec<u32> = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => pixels.iter().map(|&v| v as u32).collect(),
        jpeg_decoder::PixelFormat::L8 => pixels.iter().flat_map(|&v| [v as u32; 3]).collect(),
        other => {
            return Err(IoError::layout(
                path,
                format!("unsupported JPEG pixel format {other:?}"),
            ))
        }
    };

    let meta = meta_for(icc_bytes, exif, 8, path)?;
    Ok(Image {
        width: info.width as u32,
        height: info.height as u32,
        pixels: linearize(&samples, 255.0, &meta.trc),
        meta,
    })
}

/// Pull the APP1 "Exif\0\0" payload straight out of the JPEG stream.
///
/// In a JPEG the EXIF block is self-contained — it carries its own TIFF header
/// and its offsets are relative to that header — so a byte copy is genuinely
/// verbatim and genuinely portable.
fn extract_jpeg_exif(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut i = 2; // skip SOI
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xFF {
            return None;
        }
        let marker = bytes[i + 1];
        // Standalone markers carry no length.
        if (0xD0..=0xD9).contains(&marker) {
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if len < 2 || i + 2 + len > bytes.len() {
            return None;
        }
        let seg = &bytes[i + 4..i + 2 + len];
        if marker == 0xE1 && seg.starts_with(b"Exif\0\0") {
            return Some(seg[6..].to_vec());
        }
        // SOS: entropy-coded data follows; no more metadata worth scanning.
        if marker == 0xDA {
            return None;
        }
        i += 2 + len;
    }
    None
}
