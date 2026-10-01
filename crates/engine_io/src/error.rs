//! Save/load failures.

use engine_core::VolumePos;
use engine_volume::VolumeError;
use std::fmt;
use std::path::PathBuf;

/// Anything that can go wrong reading or writing a world.
#[derive(Debug)]
pub enum IoError {
    /// The underlying file could not be read or written.
    File {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The bytes were not valid JSON, or did not match the schema.
    Parse(serde_json::Error),
    /// The file identified itself as something other than a world.
    WrongFormat { found: String },
    /// The file is a world, but from a version this build cannot read.
    UnsupportedVersion { found: u32, supported: u32 },
    /// A chunk's cell data was internally inconsistent.
    BadChunk {
        chunk: VolumePos,
        source: VolumeError,
    },
}

impl fmt::Display for IoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IoError::File { path, source } => {
                write!(f, "could not access {}: {source}", path.display())
            }
            IoError::Parse(source) => write!(f, "malformed world data: {source}"),
            IoError::WrongFormat { found } => write!(
                f,
                "expected a `{}` file but found `{found}`",
                crate::FORMAT_TAG
            ),
            IoError::UnsupportedVersion { found, supported } => write!(
                f,
                "world format version {found} is not supported by this build \
                 (which reads version {supported})"
            ),
            IoError::BadChunk { chunk, source } => {
                write!(f, "chunk {chunk:?} holds invalid cell data: {source}")
            }
        }
    }
}

impl std::error::Error for IoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            IoError::File { source, .. } => Some(source),
            IoError::Parse(source) => Some(source),
            IoError::BadChunk { source, .. } => Some(source),
            IoError::WrongFormat { .. } | IoError::UnsupportedVersion { .. } => None,
        }
    }
}

impl From<serde_json::Error> for IoError {
    fn from(source: serde_json::Error) -> Self {
        IoError::Parse(source)
    }
}
