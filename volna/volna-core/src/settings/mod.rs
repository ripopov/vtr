//! User settings: one registry declares every setting; the store resolves a
//! JSONC `settings.json` plus host overrides into a typed [`Settings`] value
//! with diagnostics, computes surgical text edits for GUI changes, ranks
//! settings for a search query, and emits the JSON Schema and the VS Code
//! contribution block. Hosts move bytes; nothing here touches a file.
//!
//! - `registry`: the static table of [`Spec`]s, pages and value kinds
//! - `jsonc`: tolerant parser with spans, and comment-preserving edits
//! - `store`: layers, resolution, diagnostics, the write queue
//! - `search`: the ranked fuzzy matcher with `@modified`, `@page:`, `@id:`
//! - `schema`: JSON Schema, `contributes.configuration`, default document
//! - `migrate`: `preferences.json` version 1 → `settings.json` + `state.json`

mod controller;
pub mod jsonc;
pub mod migrate;
pub mod registry;
pub mod schema;
pub mod search;
pub mod store;

pub use registry::{Apply, Choice, Host, Hosts, Kind, Page, REGISTRY, Spec, spec};
pub use search::{Hit, Matched, search};
pub use store::{Diagnostic, Severity, Store, WRITE_IDLE};

/// A setting value. Integers and numbers are distinct kinds in the registry.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Integer(i64),
    Number(f64),
    Text(String),
}

impl Value {
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Value::Bool(b) => serde_json::Value::Bool(*b),
            Value::Integer(i) => serde_json::Value::from(*i),
            Value::Number(n) => serde_json::Value::from(*n),
            Value::Text(s) => serde_json::Value::String(s.clone()),
        }
    }

    /// The JSON text of the value, as the editor writes it into the document.
    pub fn to_json_text(&self) -> String {
        self.to_json().to_string()
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Integer(i) => Some(*i),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            Value::Integer(i) => Some(*i as f64),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}
impl From<i64> for Value {
    fn from(i: i64) -> Self {
        Value::Integer(i)
    }
}
impl From<f64> for Value {
    fn from(n: f64) -> Self {
        Value::Number(n)
    }
}
impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Text(s.to_owned())
    }
}
impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Text(s)
    }
}

/// Viewport animation for keyboard and menu navigation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Animation {
    #[default]
    On,
    Reduced,
    Off,
}

impl Animation {
    /// Duration of one navigation animation; `None` jumps.
    pub fn duration(self) -> Option<std::time::Duration> {
        match self {
            Animation::On => Some(std::time::Duration::from_millis(140)),
            Animation::Reduced => Some(std::time::Duration::from_millis(70)),
            Animation::Off => None,
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "reduced" => Animation::Reduced,
            "off" => Animation::Off,
            _ => Animation::On,
        }
    }
}

/// Where a trace's workspace is saved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Autosave {
    #[default]
    Sidecar,
    /// VS Code's per-workspace storage; only offered inside VS Code.
    Vscode,
    Off,
}

impl Autosave {
    fn parse(s: &str) -> Self {
        match s {
            "vscode" => Autosave::Vscode,
            "off" => Autosave::Off,
            _ => Autosave::Sidecar,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppearanceSettings {
    /// `one-dark` or the stem of a palette file in the host's theme directory.
    pub theme: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelSettings {
    pub link_by_default: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaveSettings {
    pub animation: Animation,
    pub snap_pixels: u32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceSettings {
    pub autosave: Autosave,
    pub recent_limit: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteSettings {
    pub memory_mib: u64,
    pub object_mib: u64,
    pub server_path: String,
}

/// The resolved settings the viewer reads. Every field has a registry entry;
/// invalid or missing document values fall back to that entry's default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub appearance: AppearanceSettings,
    pub panels: PanelSettings,
    pub waves: WaveSettings,
    pub workspace: WorkspaceSettings,
    pub remote: RemoteSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self::from_values(&|id| spec(id).expect("registered id").default.value())
    }
}

impl Settings {
    /// Build the typed struct from a resolved generic value per id.
    pub fn from_values(value: &dyn Fn(&str) -> Value) -> Self {
        let text = |id: &str| value(id).as_str().unwrap_or_default().to_owned();
        let int = |id: &str| value(id).as_i64().unwrap_or_default();
        Settings {
            appearance: AppearanceSettings {
                theme: text("appearance.theme"),
            },
            panels: PanelSettings {
                link_by_default: value("panels.linkByDefault").as_bool().unwrap_or(true),
            },
            waves: WaveSettings {
                animation: Animation::parse(&text("waves.animation")),
                snap_pixels: int("waves.snapPixels").max(0) as u32,
            },
            workspace: WorkspaceSettings {
                autosave: Autosave::parse(&text("workspace.autosave")),
                recent_limit: int("workspace.recentLimit").max(1) as usize,
            },
            remote: RemoteSettings {
                memory_mib: int("remote.memoryMiB").max(1) as u64,
                object_mib: int("remote.objectMiB").max(1) as u64,
                server_path: text("remote.serverPath"),
            },
        }
    }

    /// The link flags new panels start with.
    pub fn link_by_default(&self) -> crate::wave::model::Link {
        crate::wave::model::Link {
            viewport: self.panels.link_by_default,
            cursor: self.panels.link_by_default,
        }
    }

    pub fn remote_limits(&self) -> crate::remote::limits::Limits {
        crate::remote::limits::Limits {
            memory_mib: self.remote.memory_mib,
            object_mib: self.remote.object_mib,
        }
    }
}
