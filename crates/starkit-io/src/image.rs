//! The decoded image and its metadata.

use crate::icc::Trc;

/// A decoded image in **linear light, in the source's own primaries** (D-029).
///
/// `pixels` is interleaved RGB, `width * height * 3`, f32 per INV-4 and the
/// workspace numerics rule (pixel buffers f32, accumulators f64).
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// Interleaved RGB, linear light, nominally in `[0, 1]`.
    pub pixels: Vec<f32>,
    pub meta: Meta,
}

/// Everything needed to write the image back out as it came in.
#[derive(Debug, Clone)]
pub struct Meta {
    /// The embedded ICC profile, byte-for-byte. Re-attached verbatim on encode:
    /// this crate never rewrites a profile it did not author.
    pub icc: Option<Vec<u8>>,
    /// EXIF payload, captured verbatim.
    pub exif: Option<Exif>,
    /// The curve used to linearize, and the exact curve encode must invert.
    /// Storing it rather than re-deriving it is what makes the round-trip
    /// symmetric by construction instead of by coincidence.
    pub trc: Trc,
    /// Bits per sample in the source (8 or 16). Output is always 16.
    pub source_bit_depth: u8,
    /// Non-fatal observations worth surfacing — an untagged file assumed sRGB,
    /// a profile we could not read a curve from. The CLI reports these per image
    /// (FR-8); they are not errors, but they are not nothing either.
    pub warnings: Vec<String>,
}

/// EXIF as captured from the source.
///
/// TIFF and JPEG disagree about what "the EXIF" is: in a JPEG it is a
/// self-contained APP1 blob with its own TIFF header, while in a TIFF it is a
/// sub-IFD stitched into the file's own directory structure with absolute file
/// offsets. Keeping the distinction explicit stops us from writing one where the
/// other belongs.
#[derive(Debug, Clone, PartialEq)]
pub enum Exif {
    /// A complete TIFF-structured EXIF block (JPEG APP1 payload, minus the
    /// "Exif\0\0" marker).
    Tiff(Vec<u8>),
    /// Individual tags lifted from a TIFF's Exif sub-IFD, already resolved to
    /// their values so no offset needs rewriting.
    Tags(Vec<ExifTag>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExifTag {
    pub tag: u16,
    pub value: ExifValue,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExifValue {
    Ascii(String),
    Short(Vec<u16>),
    Long(Vec<u32>),
    Rational(Vec<(u32, u32)>),
    SRational(Vec<(i32, i32)>),
    Byte(Vec<u8>),
    Undefined(Vec<u8>),
}

impl Meta {
    #[allow(dead_code)]
    pub(crate) fn warn(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }
}

/// Hand-written rather than derived: a derived `Debug` would dump 50 million
/// floats into a test failure message and bury the assertion that mattered.
impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("pixels", &format_args!("[{} f32]", self.pixels.len()))
            .field("meta", &self.meta)
            .finish()
    }
}
