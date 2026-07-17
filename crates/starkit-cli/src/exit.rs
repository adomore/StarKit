//! The stable exit-code table (FR-8).
//!
//! These numbers are a contract: batch scripts and CI branch on them, so they
//! may be **added to but never renumbered**. clap already exits `2` on argument
//! errors, so this table starts at `3` to avoid colliding with it.
//!
//! The distinction that matters for FR-8 is [`BATCH_PARTIAL`]: a batch that hit a
//! poisoned file must finish the rest and exit non-zero, so a caller can tell
//! "everything processed" from "some were skipped" without parsing logs.

use starkit_core::Error as CoreError;
use starkit_io::IoError;

/// Everything succeeded.
pub const OK: i32 = 0;
/// Bad arguments / usage. Matches clap's own convention.
pub const USAGE: i32 = 2;
/// An input could not be read, or an output could not be written.
pub const IO: i32 = 3;
/// An input was not decodable — truncated, corrupt, or not the format it claims.
pub const DECODE: i32 = 4;
/// The input decoded but is a layout this tool does not handle.
pub const UNSUPPORTED: i32 = 5;
/// A processing step failed on otherwise-valid data (an internal error).
pub const PROCESSING: i32 = 6;
/// A batch ran to completion but skipped at least one image (FR-8). The batch
/// did **not** abort — this is how a caller learns some files failed.
pub const BATCH_PARTIAL: i32 = 7;

/// Map a decode/IO error to its stable exit code, so a single image and a batch
/// classify failures the same way.
pub fn code_for_io(err: &IoError) -> i32 {
    match err {
        IoError::Io { .. } => IO,
        IoError::Decode { .. } => DECODE,
        IoError::UnsupportedLayout { .. } | IoError::IccTransform { .. } => UNSUPPORTED,
    }
}

/// Map a core processing error to its stable exit code.
pub fn code_for_core(_err: &CoreError) -> i32 {
    // Every `starkit_core::Error` variant is a processing failure on data that
    // already decoded (a size mismatch, a bad parameter, an empty image). None
    // is an I/O condition, so they share one code.
    PROCESSING
}
