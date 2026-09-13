//! Raw, bounded trace-query contracts shared by native and remote consumers.
//!
//! This crate has no viewer, GUI or VDB dependency. Its optional native reader
//! dependency is excluded from WASM builds. Native
//! delivery uses typed immutable results; only the remote adapter enables
//! `wire` framing. Times never pass through floating-point arithmetic.

pub mod budget;
pub mod metadata;
#[cfg(all(feature = "native-engine", not(target_family = "wasm")))]
pub mod native;
#[cfg(all(feature = "native-engine", not(target_family = "wasm")))]
pub mod native_metadata;
#[cfg(all(feature = "native-engine", not(target_family = "wasm")))]
pub mod native_session;
pub mod session;
pub mod summary;
pub mod time;
pub mod wave;
#[cfg(feature = "wire")]
pub mod wire;
#[cfg(feature = "wire")]
pub mod wire_encode;
#[cfg(feature = "wire")]
pub mod wire_request;

pub use budget::{Budget, Cancellation, Reservation};
pub use time::{Grid, Interval, TimeBound};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("invalid query: {0}")]
    Invalid(&'static str),
    #[error("resource limit exceeded")]
    ResourceLimit,
    #[error("query cancelled")]
    Cancelled,
    #[error("trace query failed: {0}")]
    Backend(String),
    #[error("invalid frame: {0}")]
    Frame(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;
