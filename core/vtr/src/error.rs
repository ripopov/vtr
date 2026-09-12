//! Error type shared by the writer and the reader.

use std::fmt;

/// All VTR operations return this error type.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file is not a VTR file, or a structural invariant is violated.
    #[error("corrupt VTR file: {0}")]
    Corrupt(&'static str),
    /// The file has a newer major version than this library understands.
    #[error("unsupported VTR version {major}.{minor} (this library reads up to major {supported})")]
    UnsupportedVersion { major: u16, minor: u16, supported: u16 },
    /// A caller-supplied argument was invalid (bad id, non-monotonic time, ...).
    #[error("invalid argument: {0}")]
    Invalid(String),
    /// Operation is not permitted in the writer's current state.
    #[error("invalid state: {0}")]
    State(&'static str),
    /// Underlying I/O failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Checksum mismatch (only reported when verification is enabled).
    #[error("checksum mismatch in section at offset {offset}")]
    Checksum { offset: u64 },
    /// Compression/decompression failure.
    #[error("codec error: {0}")]
    Codec(String),
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub(crate) fn invalid(msg: impl fmt::Display) -> Self {
        Error::Invalid(msg.to_string())
    }
}
