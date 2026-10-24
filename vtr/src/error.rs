//! Error type shared by the writer and the reader.

use std::fmt;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("corrupt VTR file: {0}")]
    Corrupt(&'static str),
    #[error(
        "unsupported {format} version {major}.{minor} (expected {supported_major}.{supported_minor})"
    )]
    UnsupportedVersion {
        format: &'static str,
        major: u16,
        minor: u16,
        supported_major: u16,
        supported_minor: u16,
    },
    #[error("invalid argument: {0}")]
    Invalid(String),
    #[error("invalid state: {0}")]
    State(&'static str),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("checksum mismatch in section at offset {offset}")]
    Checksum { offset: u64 },
    #[error("codec error: {0}")]
    Codec(String),
    #[error("recording failed: {0}")]
    Recording(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    // Keep enum cleanup out of successful parsing loops across codegen units.
    #[cold]
    pub(crate) fn corrupt(message: &'static str) -> Self {
        Self::Corrupt(message)
    }
    pub(crate) fn invalid(msg: impl fmt::Display) -> Self {
        Error::Invalid(msg.to_string())
    }
}
