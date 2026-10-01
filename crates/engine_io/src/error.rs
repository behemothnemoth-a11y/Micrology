//! Save/load failures.

use engine_core::{RegionPos, VolumePos};
use engine_destruction::{FragmentId, FragmentSpatialError};
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
    /// A volume's support data was internally inconsistent.
    ///
    /// Kept apart from `IoError::BadChunk` because the two mean different
    /// things to whoever is looking at the shard: bad cells lose what was
    /// built, bad support loses what holds it up.
    BadAnchors {
        volume: VolumePos,
        source: engine_volume::AnchorError,
    },
    /// The manifest lists a region whose file is not there.
    MissingRegion { region: RegionPos, path: PathBuf },
    /// A region file does not describe the region it was loaded as.
    RegionMismatch {
        expected: RegionPos,
        found: RegionPos,
    },
    /// A volume in a region file is outside that region.
    VolumeOutsideRegion {
        region: RegionPos,
        volume: VolumePos,
    },
    /// A fragment payload file is missing.
    MissingFragment { fragment: FragmentId, path: PathBuf },
    /// A fragment file identified a different ID than the path requested.
    FragmentMismatch {
        expected: FragmentId,
        found: FragmentId,
    },
    /// A fragment payload is internally invalid.
    BadFragment {
        fragment: FragmentId,
        reason: String,
    },
    /// A fragment-local volume could not be decoded.
    BadFragmentVolume {
        fragment: FragmentId,
        volume: VolumePos,
        source: VolumeError,
    },
    /// A fragment/index file used the wrong tag.
    WrongFragmentFormat { found: String },
    /// A region index references an ID not listed as a fragment payload.
    FragmentIndexUnknownId {
        region: RegionPos,
        fragment: FragmentId,
    },
    /// Persisted region membership disagrees with fragment payload positions.
    FragmentIndexMismatch,
    /// A fragment pose cannot map into the current region address space.
    FragmentSpatial { source: FragmentSpatialError },
    /// A path that should be a world directory is not usable as one.
    NotAWorldDirectory { path: PathBuf },
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
            IoError::BadAnchors { volume, source } => {
                write!(f, "volume {volume:?} holds invalid support data: {source}")
            }
            IoError::BadChunk { chunk, source } => {
                write!(f, "chunk {chunk:?} holds invalid cell data: {source}")
            }
            IoError::MissingRegion { region, path } => write!(
                f,
                "the manifest lists region {region:?} but {} is missing",
                path.display()
            ),
            IoError::RegionMismatch { expected, found } => write!(
                f,
                "region file claims to be {found:?} but was loaded as {expected:?}"
            ),
            IoError::VolumeOutsideRegion { region, volume } => {
                write!(f, "volume {volume:?} does not belong to region {region:?}")
            }
            IoError::MissingFragment { fragment, path } => write!(
                f,
                "fragment {fragment} is listed but {} is missing",
                path.display()
            ),
            IoError::FragmentMismatch { expected, found } => write!(
                f,
                "fragment file claims to be {found} but was loaded as {expected}"
            ),
            IoError::BadFragment { fragment, reason } => {
                write!(f, "fragment {fragment} is invalid: {reason}")
            }
            IoError::BadFragmentVolume {
                fragment,
                volume,
                source,
            } => write!(
                f,
                "fragment {fragment} local volume {volume:?} is invalid: {source}"
            ),
            IoError::WrongFragmentFormat { found } => {
                write!(f, "expected a Micrology fragment file but found `{found}`")
            }
            IoError::FragmentIndexUnknownId { region, fragment } => write!(
                f,
                "fragment index region {region:?} references unknown fragment {fragment}"
            ),
            IoError::FragmentIndexMismatch => write!(
                f,
                "fragment spatial index does not match the persisted fragment payloads"
            ),
            IoError::FragmentSpatial { source } => {
                write!(f, "fragment spatial index failed: {source}")
            }
            IoError::NotAWorldDirectory { path } => {
                write!(f, "{} is not a world directory", path.display())
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
            IoError::BadAnchors { source, .. } => Some(source),
            IoError::BadFragmentVolume { source, .. } => Some(source),
            IoError::FragmentSpatial { source } => Some(source),
            IoError::WrongFormat { .. }
            | IoError::UnsupportedVersion { .. }
            | IoError::MissingRegion { .. }
            | IoError::RegionMismatch { .. }
            | IoError::VolumeOutsideRegion { .. }
            | IoError::MissingFragment { .. }
            | IoError::FragmentMismatch { .. }
            | IoError::BadFragment { .. }
            | IoError::WrongFragmentFormat { .. }
            | IoError::FragmentIndexUnknownId { .. }
            | IoError::FragmentIndexMismatch
            | IoError::NotAWorldDirectory { .. } => None,
        }
    }
}

impl From<serde_json::Error> for IoError {
    fn from(source: serde_json::Error) -> Self {
        IoError::Parse(source)
    }
}
