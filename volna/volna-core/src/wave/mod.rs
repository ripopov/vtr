//! The waveform panel: viewport math, timeline, the displayed-signal model,
//! its pixel layout, and the painter that turns it into a display list.

pub mod analog;
pub mod group;
pub mod lane;
pub mod layout;
pub mod marks;
pub mod model;
pub mod overlay;
pub mod paint;
pub mod stack;
pub mod timeline;
pub mod tint;
pub mod tree;
pub mod viewport;

pub use layout::WaveLayout;
pub use model::{
    DisplayedSignal, Drag, GroupRow, GroupStyle, MenuEntry, MenuItem, PointerEvent, RowHeight,
    WaveMenu, WaveMenuKind, WaveModel, WaveRow,
};
pub use tint::Tint;
pub use tree::Entry;
pub use viewport::Viewport;
