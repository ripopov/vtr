//! The waveform panel: viewport math, timeline, the displayed-signal model and
//! the custom `WaveTable` element that paints names, values and waves.

pub mod table;
pub mod timeline;

pub mod view;
pub mod viewport;

pub use table::WaveTable;
pub use view::{DisplayedSignal, WaveView, WaveViewEvent};
pub use viewport::Viewport;
