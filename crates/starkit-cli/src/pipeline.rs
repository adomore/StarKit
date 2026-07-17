//! The processing pipeline: one image in, a reduced image (and report) out.
//!
//! This is where the crates meet — `starkit-io` at the two ends, `starkit-core`
//! in the middle — and where the compositor rule earns its keep: reduction
//! returns a modified plane, and only `gate::composite` writes it back, so INV-1
//! holds for the whole CLI by construction rather than by vigilance.
//!
//! Reduction runs **per channel** so a star's colour survives: detection fixes
//! the geometry once on the mean-of-RGB plane, and each channel is resynthesised
//! and composited with that same geometry, so the channels cannot drift out of
//! registration.

use std::path::{Path, PathBuf};

use starkit_core::detect::{self, Detection};
use starkit_core::gate::{self, Gate};
use starkit_core::mask::{self, MaskStar};
use starkit_core::reduce::{self, ReduceStar};
use starkit_io::Image;

use crate::preset::{Method, Preset};
use crate::report::Report;

/// Outputs a single `process` run may write, resolved to concrete paths.
pub struct Outputs {
    pub reduced: Option<PathBuf>,
    pub mask: Option<PathBuf>,
}

/// Run the full pipeline on one already-decoded image.
///
/// Returns the report and the modified `Image` (ready to encode). Split from the
/// I/O so it is testable without touching disk, and so the batch driver can
/// decide what to do with failures.
pub fn process_image(
    image: &Image,
    sky: Option<&[f32]>,
    preset: &Preset,
) -> Result<(Image, Report), starkit_core::Error> {
    let w = image.width as usize;
    let h = image.height as usize;
    let n = w * h;

    // Mean-of-RGB plane: the frame detection and masking work on (D-010).
    let mean: Vec<f32> = image
        .pixels
        .chunks_exact(3)
        .map(|c| (c[0] + c[1] + c[2]) / 3.0)
        .collect();

    // Detect, then — on a nightscape — drop the silhouette-edge phantoms before
    // anything downstream can act on them (D-034).
    let dparams = preset.detect.to_params();
    let mut det: Vec<Detection> = detect::detect(&mean, w, h, &dparams)?;
    if let Some(sky) = sky {
        det = gate::restrict_to_sky(
            &det,
            sky,
            w,
            h,
            crate::preset::sky_margin_px(&preset.detect),
        )?;
    }

    // Star mask and gate — the one place the invariant is enforced.
    let stars: Vec<MaskStar> = det.iter().map(MaskStar::from).collect();
    let star_mask = mask::build(&stars, w, h, &preset.mask.to_params())?;
    let g: Gate = gate::build(&star_mask, sky, preset.dilate_px())?;

    // Reduce each channel with the shared geometry, then composite through the
    // gate. Channel-major would be simpler to read but cache-hostile; this keeps
    // the three per-channel planes contiguous.
    let reduce_stars: Vec<ReduceStar> = det.iter().map(ReduceStar::from).collect();
    let mut out_pixels = image.pixels.clone();
    for c in 0..3 {
        let channel: Vec<f32> = (0..n).map(|i| image.pixels[i * 3 + c]).collect();
        let modified = match preset.reduce.method {
            Method::Resynthesis => {
                reduce::resynthesis(&channel, w, h, &reduce_stars, &preset.reduce.to_resynth())?
            }
            Method::Morphological => {
                reduce::morphological(&channel, w, h, &preset.reduce.to_morph())?
            }
        };
        let composited = g.composite(&channel, &modified)?;
        for (i, v) in composited.into_iter().enumerate() {
            out_pixels[i * 3 + c] = v;
        }
    }

    let out = Image {
        width: image.width,
        height: image.height,
        pixels: out_pixels,
        meta: image.meta.clone(),
    };

    let report = Report::new(
        det.len(),
        g.open_fraction(),
        preset,
        image.meta.warnings.clone(),
    );
    Ok((out, report))
}

/// Decode, process, and write, mapping every failure to a typed CLI error.
///
/// The input is opened read-only and never written (INV-3); outputs go through
/// `starkit-io`'s atomic path.
pub fn process_file(
    input: &Path,
    sky_path: Option<&Path>,
    preset: &Preset,
    outputs: &Outputs,
) -> Result<Report, ProcessError> {
    let image = starkit_io::decode(input).map_err(ProcessError::Io)?;

    let sky = match sky_path {
        Some(p) => {
            let (data, mw, mh) = starkit_io::decode_mask(p).map_err(ProcessError::Io)?;
            if mw != image.width || mh != image.height {
                return Err(ProcessError::SkyMaskMismatch {
                    image: (image.width, image.height),
                    mask: (mw, mh),
                });
            }
            Some(data)
        }
        None => None,
    };

    let (reduced, mut report) =
        process_image(&image, sky.as_deref(), preset).map_err(ProcessError::Core)?;

    if let Some(path) = &outputs.reduced {
        starkit_io::write_tiff16(&reduced, path).map_err(ProcessError::Io)?;
        report.output = Some(path.display().to_string());
    }
    if let Some(path) = &outputs.mask {
        // Rebuild the combined mask for export (FR-4). Cheap next to the reduce.
        let w = image.width as usize;
        let h = image.height as usize;
        let mean: Vec<f32> = image
            .pixels
            .chunks_exact(3)
            .map(|c| (c[0] + c[1] + c[2]) / 3.0)
            .collect();
        let mut det =
            detect::detect(&mean, w, h, &preset.detect.to_params()).map_err(ProcessError::Core)?;
        if let Some(s) = &sky {
            det =
                gate::restrict_to_sky(&det, s, w, h, crate::preset::sky_margin_px(&preset.detect))
                    .map_err(ProcessError::Core)?;
        }
        let stars: Vec<MaskStar> = det.iter().map(MaskStar::from).collect();
        let m = mask::build(&stars, w, h, &preset.mask.to_params()).map_err(ProcessError::Core)?;
        starkit_io::write_gray16(&m.data, image.width, image.height, path)
            .map_err(ProcessError::Io)?;
        report.mask_output = Some(path.display().to_string());
    }

    report.input = input.display().to_string();
    Ok(report)
}

/// A `process` failure, already classified for the exit-code table.
#[derive(Debug)]
pub enum ProcessError {
    Io(starkit_io::IoError),
    Core(starkit_core::Error),
    SkyMaskMismatch { image: (u32, u32), mask: (u32, u32) },
}

impl ProcessError {
    pub fn exit_code(&self) -> i32 {
        match self {
            ProcessError::Io(e) => crate::exit::code_for_io(e),
            ProcessError::Core(e) => crate::exit::code_for_core(e),
            // A mask for a different image is a usage mistake, not corrupt data.
            ProcessError::SkyMaskMismatch { .. } => crate::exit::UNSUPPORTED,
        }
    }
}

impl std::fmt::Display for ProcessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcessError::Io(e) => write!(f, "{e}"),
            ProcessError::Core(e) => write!(f, "{e}"),
            ProcessError::SkyMaskMismatch { image, mask } => write!(
                f,
                "sky mask is {}x{} but the image is {}x{} — it is a mask for a different image",
                mask.0, mask.1, image.0, image.1
            ),
        }
    }
}

impl std::error::Error for ProcessError {}
