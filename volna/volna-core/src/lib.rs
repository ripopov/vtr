//! Volna core: everything the viewer decides, with no GUI toolkit attached.
//!
//! A frontend owns the window, paints the [`scene::Scene`] this crate
//! produces, hosts native widgets for the chrome, and runs the loads the core
//! asks for. See `volna/volna/ARCHITECTURE.md` for the contract.
//!
//! Module map:
//! - `app`: the [`app::App`] state machine, its [`app::Command`]s and [`app::Event`]s
//! - `document`: the open trace and the state every view shares (cursor, markers)
//! - `session`: the [`session::Session`] boundary through which all trace data is read
//! - `data`: values, histories, translators, hierarchy
//! - `wave`: viewport math, timeline, the wave panel model, layout and painter
//! - `clock`: declared clocks, their timelines and each panel's rulers and cycle axis
//! - `settings`: the registry, `settings.json` store, search and generated schema
//! - `transaction`: the prepared view of one record and the panel that shows it
//! - `sidebar`: scope tree and variable list models
//! - `scene`, `geometry`, `color`, `theme`, `icons`: the toolkit-neutral presentation types

pub mod app;
pub mod clock;
pub mod color;
pub mod data;
pub mod document;
pub mod geometry;
pub mod icons;
pub mod nav;
pub mod panels;
pub mod pipeline;
pub mod remote;
pub mod scene;
pub mod selection;
pub mod session;
pub mod settings;
pub mod sidebar;
pub mod table;
pub mod theme;
pub mod transaction;
pub mod wave;

pub use app::{Action, App, Command, Event};
pub use color::Color;
pub use document::Document;
pub use scene::{FontRole, Scene, TextMeasure};
pub use session::{LoadRequest, LoadResult, OpenSpec, Session};
pub use theme::Theme;

/// Re-exported clock so frontends and the core agree on `Instant`.
pub use web_time::Instant;

pub mod workspace;
