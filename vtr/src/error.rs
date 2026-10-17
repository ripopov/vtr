//! Error type shared by the writer and the reader.

use std::fmt;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("corrupt VTR file: {0}")]
    Corrupt(&'static str),
    #[error("unsupported VTR version {major}.{minor} (this library requires major {supported})")]
    UnsupportedVersion {
        major: u16,
        minor: u16,
        supported: u16,
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
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub(crate) fn invalid(msg: impl fmt::Display) -> Self {
        Error::Invalid(msg.to_string())
    }
}
