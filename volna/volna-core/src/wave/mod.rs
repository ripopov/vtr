//! The waveform panel: viewport math, timeline, the displayed-signal model,
//! its pixel layout, and the painter that turns it into a display list.

pub mod analog;
pub mod group;
pub mod lane;
pub mod layout;
pub mod model;
pub mod overlay;
pub mod paint;
pub mod timeline;
pub mod tree;
pub mod viewport;

pub use layout::WaveLayout;
pub use model::{
    DisplayedSignal, Drag, GroupRow, MenuEntry, MenuItem, PointerEvent, RowHeight, WaveMenu,
    WaveMenuKind, WaveModel, WaveRow,
};
pub use tree::Entry;
pub use viewport::Viewport;
