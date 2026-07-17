//! `starkit` — the command-line interface (task T1-8, FR-8).
//!
//! Three subcommands: `process` one image, `inspect` its star catalog, or `batch`
//! a folder. A batch never aborts on a bad file — it reports and skips (FR-8) —
//! and exit codes come from one stable table ([`exit`]) so scripts can branch on
//! them.

mod exit;
mod pipeline;
mod preset;
mod report;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use pipeline::{process_file, Outputs};
use preset::Preset;
use report::Report;

#[derive(Parser)]
#[command(
    name = "starkit",
    about = "Automated star processing for astrophotography",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Reduce the stars in one image.
    Process(ProcessArgs),
    /// Detect stars and print the catalog as JSON (no image is written).
    Inspect(InspectArgs),
    /// Process every image in a folder, one report per image, skipping failures.
    Batch(BatchArgs),
}

#[derive(Parser)]
struct ProcessArgs {
    /// Input image (TIFF/PNG/JPEG).
    #[arg(long)]
    input: PathBuf,
    /// Output reduced image (16-bit TIFF). Defaults to `<input>.starkit.tiff`.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Preset JSON. Omit for defaults.
    #[arg(long)]
    preset: Option<PathBuf>,
    /// External sky mask (the manual FR-3 path): star ops stay inside it.
    #[arg(long)]
    sky: Option<PathBuf>,
    /// Also export the combined star mask here (16-bit grayscale).
    #[arg(long)]
    mask: Option<PathBuf>,
    /// Write the per-image JSON report here (default: stdout).
    #[arg(long)]
    report: Option<PathBuf>,
}

#[derive(Parser)]
struct InspectArgs {
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    preset: Option<PathBuf>,
    /// Write the catalog JSON here (default: stdout).
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Parser)]
struct BatchArgs {
    /// Folder of input images.
    #[arg(long)]
    input_dir: PathBuf,
    /// Folder for outputs (created if absent). Reports land beside each output.
    #[arg(long)]
    out_dir: PathBuf,
    #[arg(long)]
    preset: Option<PathBuf>,
    /// Also export each combined star mask.
    #[arg(long)]
    masks: bool,
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Command::Process(a) => run_process(a),
        Command::Inspect(a) => run_inspect(a),
        Command::Batch(a) => run_batch(a),
    };
    std::process::exit(code);
}

fn load_preset(path: Option<&Path>) -> Result<Preset> {
    match path {
        None => Ok(Preset::default()),
        Some(p) => {
            let text = std::fs::read_to_string(p)
                .with_context(|| format!("reading preset {}", p.display()))?;
            Preset::from_json(&text).with_context(|| format!("parsing preset {}", p.display()))
        }
    }
}

/// Print an error to stderr and return the exit code, so `main` stays a table.
fn fail(code: i32, msg: impl std::fmt::Display) -> i32 {
    eprintln!("starkit: {msg}");
    code
}

fn run_process(args: ProcessArgs) -> i32 {
    let preset = match load_preset(args.preset.as_deref()) {
        Ok(p) => p,
        Err(e) => return fail(exit::USAGE, e),
    };
    let out_path = args.out.unwrap_or_else(|| default_output(&args.input));
    let outputs = Outputs {
        reduced: preset.outputs.reduced.then_some(out_path),
        mask: args.mask,
    };

    match process_file(&args.input, args.sky.as_deref(), &preset, &outputs) {
        Ok(report) => {
            if let Err(e) = emit_report(&report, args.report.as_deref()) {
                return fail(exit::IO, e);
            }
            exit::OK
        }
        Err(e) => fail(e.exit_code(), &e),
    }
}

fn run_inspect(args: InspectArgs) -> i32 {
    let preset = match load_preset(args.preset.as_deref()) {
        Ok(p) => p,
        Err(e) => return fail(exit::USAGE, e),
    };
    let image = match starkit_io::decode(&args.input) {
        Ok(i) => i,
        Err(e) => return fail(exit::code_for_io(&e), e),
    };
    let (w, h) = (image.width as usize, image.height as usize);
    let mean: Vec<f32> = image
        .pixels
        .chunks_exact(3)
        .map(|c| (c[0] + c[1] + c[2]) / 3.0)
        .collect();
    let det = match starkit_core::detect::detect(&mean, w, h, &preset.detect.to_params()) {
        Ok(d) => d,
        Err(e) => return fail(exit::code_for_core(&e), e),
    };

    let catalog = catalog_json(&image, &det, &preset);
    match args.out {
        Some(p) => {
            if let Err(e) = std::fs::write(&p, catalog.as_bytes()) {
                return fail(exit::IO, format!("writing {}: {e}", p.display()));
            }
        }
        None => print!("{catalog}"),
    }
    exit::OK
}

fn run_batch(args: BatchArgs) -> i32 {
    let preset = match load_preset(args.preset.as_deref()) {
        Ok(p) => p,
        Err(e) => return fail(exit::USAGE, e),
    };
    if let Err(e) = std::fs::create_dir_all(&args.out_dir) {
        return fail(
            exit::IO,
            format!("creating {}: {e}", args.out_dir.display()),
        );
    }

    let inputs = match collect_inputs(&args.input_dir) {
        Ok(v) => v,
        Err(e) => return fail(exit::IO, e),
    };
    if inputs.is_empty() {
        return fail(
            exit::USAGE,
            format!("no images in {}", args.input_dir.display()),
        );
    }

    let mut skipped = 0usize;
    for input in &inputs {
        let stem = input
            .file_stem()
            .map(|s| s.to_os_string())
            .unwrap_or_default();
        let out = args.out_dir.join(&stem).with_extension("starkit.tiff");
        let mask = args
            .masks
            .then(|| args.out_dir.join(&stem).with_extension("mask.tiff"));
        let report_path = args.out_dir.join(&stem).with_extension("report.json");
        let outputs = Outputs {
            reduced: preset.outputs.reduced.then_some(out),
            mask,
        };

        // FR-8: a failure is reported and skipped; the batch continues.
        match process_file(input, None, &preset, &outputs) {
            Ok(report) => {
                let _ = std::fs::write(&report_path, report.to_json().as_bytes());
                println!(
                    "ok    {} ({} stars)",
                    input.display(),
                    report.stars_detected
                );
            }
            Err(e) => {
                skipped += 1;
                let report = Report::skipped(&input.display().to_string(), &e.to_string());
                let _ = std::fs::write(&report_path, report.to_json().as_bytes());
                eprintln!("skip  {} — {e}", input.display());
            }
        }
    }

    println!(
        "batch: {} processed, {skipped} skipped",
        inputs.len() - skipped
    );
    if skipped > 0 {
        exit::BATCH_PARTIAL
    } else {
        exit::OK
    }
}

/// Images in a folder, in a **sorted** order so a batch is deterministic
/// regardless of how the filesystem enumerates entries (INV-2).
fn collect_inputs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.is_file() && is_image_ext(&path) {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn is_image_ext(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("tif" | "tiff" | "png" | "jpg" | "jpeg")
    )
}

fn default_output(input: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    input.with_file_name(stem).with_extension("starkit.tiff")
}

fn emit_report(report: &Report, path: Option<&Path>) -> Result<()> {
    match path {
        Some(p) => std::fs::write(p, report.to_json().as_bytes())
            .with_context(|| format!("writing report {}", p.display())),
        None => {
            print!("{}", report.to_json());
            Ok(())
        }
    }
}

/// Build a measured-form catalog (schema v1) from detections, for `inspect`.
fn catalog_json(
    image: &starkit_io::Image,
    det: &[starkit_core::detect::Detection],
    preset: &Preset,
) -> String {
    use starkit_core::types::{Catalog, ImageMeta, Star, SCHEMA_V1};

    let stars: Vec<Star> = det
        .iter()
        .enumerate()
        .map(|(i, d)| Star {
            id: i as u32 + 1,
            x: d.x,
            y: d.y,
            flux: d.flux,
            peak: d.peak,
            fwhm: d.fwhm,
            ellipticity: d.ellipticity,
            theta: d.theta,
            saturated: d.saturated,
            tier: d.tier,
            snr: Some(d.snr),
        })
        .collect();

    let catalog = Catalog {
        schema: SCHEMA_V1.to_string(),
        image: ImageMeta {
            width: image.width,
            height: image.height,
            bit_depth: 16,
            color_space: "linear-rgb".to_string(),
        },
        stars,
        generator: None,
        measurement: Some(serde_json::json!({
            "instrument": "starkit-cli inspect",
            "nsigma": preset.detect.nsigma,
            "fwhm_guess": preset.detect.fwhm_guess,
        })),
    };
    let mut s = serde_json::to_string_pretty(&catalog).expect("a catalog always serializes");
    s.push('\n');
    s
}
