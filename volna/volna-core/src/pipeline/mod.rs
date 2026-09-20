//! The pipeline panel: Konata-style rows of stage cells over any transaction
//! track, sharing the document's time axis with the wave panels and owning a
//! local row axis.

pub mod activity;
pub use activity::{Activity, ActivityCommand, FollowActivity};
pub mod layout;
pub mod model;
pub mod paint;
pub mod palette;
pub mod rows;

pub use layout::PipelineLayout;
pub use model::{Hit, PipelineModel, Rows, TrackSource};
pub use palette::{StagePalette, StageStyle};
pub use rows::RowView;
