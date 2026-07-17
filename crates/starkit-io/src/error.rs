//! Error taxonomy (starkit-io/CLAUDE.md).
//!
//! Deliberately coarse and stable: `starkit-cli` maps these variants onto its
//! exit-code table at T1-8, so the set of variants is closer to a public
//! interface than to an implementation detail. Add a variant rather than
//! overloading an existing one.

use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, IoError>;

#[derive(Debug, thiserror::Error)]
pub enum IoError {
    /// The file is not decodable as the format it claims to be — truncated,
    /// corrupt, or simply not that format. Carries the format so the CLI can
    /// tell "this JPEG is broken" from "this isn't a JPEG".
    #[error("cannot decode {path} as {format}: {source}")]
    Decode {
        format: &'static str,
        path: PathBuf,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Decodable, but not a shape this tool works on — a 1-bit fax, a 5-channel
    /// separation, a 32-bit float TIFF. Distinct from `Decode` because the file
    /// is fine; our support for it is not.
    #[error("{path}: unsupported layout — {detail}")]
    UnsupportedLayout { path: PathBuf, detail: String },

    /// The embedded ICC profile is present but unusable.
    #[error("{path}: ICC profile is unusable — {detail}")]
    IccTransform { path: PathBuf, detail: String },

    /// Filesystem failure. Carries the path because "No such file or directory"
    /// without one is a bug report nobody can act on.
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl IoError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        IoError::Io {
            path: path.into(),
            source,
        }
    }

    pub(crate) fn decode<E>(format: &'static str, path: impl Into<PathBuf>, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        IoError::Decode {
            format,
            path: path.into(),
            source: Box::new(source),
        }
    }

    pub(crate) fn layout(path: impl Into<PathBuf>, detail: impl Into<String>) -> Self {
        IoError::UnsupportedLayout {
            path: path.into(),
            detail: detail.into(),
        }
    }

    pub(crate) fn icc(path: impl Into<PathBuf>, detail: impl Into<String>) -> Self {
        IoError::IccTransform {
            path: path.into(),
            detail: detail.into(),
        }
    }
}
