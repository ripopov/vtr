//! The Transaction panel: one recorded transaction, prepared for reading.
//! The view is a pure function of resident data ([`view`]); the model owns
//! which record a panel shows, its history and the reader's preferences.

pub mod model;
pub mod view;

pub use model::{
    MAX_HISTORY, PANEL_BYTES, ShownRecord, TransactionCommand, TransactionModel, TxPanelState,
};
pub use view::{
    AttrRow, Chip, EventRow, EventTick, Identity, LaneCell, LaneRow, RefRole, RefRow, Section,
    SectionKey, StageRow, Timing, TxView, ViewPrefs, view,
};
