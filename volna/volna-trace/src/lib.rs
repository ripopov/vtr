//! Standalone trace access: immutable recordings, local readers and bounded remote loading.
//! No viewer state, presentation, GUI toolkit or server process is required.
pub mod data;
pub mod remote;
pub mod session;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub use session::{OpenSpec, Session};
