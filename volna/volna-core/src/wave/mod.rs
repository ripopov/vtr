//! The waveform panel: viewport math, timeline, the displayed-signal model,
//! its pixel layout, and the painter that turns it into a display list.

pub mod bounded;
pub mod demand;
pub mod exact;
pub mod layout;
pub mod model;
pub mod paint;
pub mod snapshot;
pub mod timeline;
pub mod viewport;

pub use layout::WaveLayout;
pub use model::{DisplayedSignal, Drag, FormatMenu, MenuItem, PointerEvent, WaveModel};
pub use viewport::Viewport;
