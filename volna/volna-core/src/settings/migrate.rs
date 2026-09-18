//! One-time migration of `preferences.json` version 1 into `settings.json`
//! (only the keys that differ from the defaults) and `state.json` (the recent
//! lists). The host renames the old file afterwards; a second run is a no-op
//! because the old file is gone.

use super::Value;
use super::registry::spec;
use crate::workspace::state::State;
use anyhow::{Result, ensure};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(default)]
struct Preferences {
    version: u32,
    theme: String,
    link_by_default: Link,
    autosave: bool,
    recent_traces: Vec<String>,
    recent_workspaces: Vec<String>,
}

#[derive(Deserialize)]
struct Link {
    viewport: bool,
    cursor: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            theme: crate::theme::builtin::ONE_DARK.into(),
            link_by_default: Link {
                viewport: true,
                cursor: true,
            },
            autosave: true,
            recent_traces: vec![],
            recent_workspaces: vec![],
        }
    }
}

/// The two documents a version 1 preferences file becomes.
pub struct Migrated {
    pub settings: String,
    pub state: State,
}

/// `schema_uri` is written as the `$schema` line of the new document.
pub fn preferences_v1(bytes: &[u8], schema_uri: Option<&str>) -> Result<Migrated> {
    ensure!(
        bytes.len() <= super::store::MAX_BYTES,
        "preferences file is too large"
    );
    let prefs: Preferences = serde_json::from_slice(bytes)?;
    ensure!(prefs.version == 1, "unsupported preferences version");
    let mut entries: Vec<(&str, Value)> = Vec::new();
    if let Some(uri) = schema_uri {
        entries.push(("$schema", Value::Text(uri.into())));
    }
    let mut keep = |id: &'static str, value: Value| {
        if spec(id).unwrap().default.value() != value {
            entries.push((id, value));
        }
    };
    keep("appearance.theme", Value::Text(prefs.theme));
    keep(
        "panels.linkByDefault",
        Value::Bool(prefs.link_by_default.viewport && prefs.link_by_default.cursor),
    );
    keep(
        "workspace.autosave",
        Value::Text(if prefs.autosave { "sidecar" } else { "off" }.into()),
    );
    let settings = if entries.is_empty() {
        "{}\n".to_owned()
    } else {
        let mut out = String::from("{\n");
        for (ix, (key, value)) in entries.iter().enumerate() {
            let comma = if ix + 1 < entries.len() { "," } else { "" };
            out.push_str(&format!("  \"{key}\": {}{comma}\n", value.to_json_text()));
        }
        out.push_str("}\n");
        out
    };
    Ok(Migrated {
        settings,
        state: State {
            recent_traces: prefs.recent_traces,
            recent_workspaces: prefs.recent_workspaces,
            ..State::new()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_1_preferences_become_settings_and_state() {
        let migrated = preferences_v1(
            br#"{"version":1,"theme":"one-dark","link_by_default":{"viewport":false,"cursor":false},"autosave":false,"recent_traces":["file:///a.vtr"],"recent_workspaces":[]}"#,
            Some("./settings.schema.json"),
        )
        .unwrap();
        assert_eq!(
            migrated.settings,
            "{\n  \"$schema\": \"./settings.schema.json\",\n  \"panels.linkByDefault\": false,\n  \"workspace.autosave\": \"off\"\n}\n"
        );
        assert_eq!(migrated.state.recent_traces, vec!["file:///a.vtr"]);
        let mut store = super::super::Store::new(super::super::Host::Native);
        store.load(&migrated.settings);
        assert!(store.diagnostics().is_empty());
        assert!(!store.resolved().panels.link_by_default);
        let untouched = preferences_v1(b"{\"version\":1}", None).unwrap();
        assert_eq!(untouched.settings, "{}\n");
        assert!(preferences_v1(b"{\"version\":2}", None).is_err());
    }
}
