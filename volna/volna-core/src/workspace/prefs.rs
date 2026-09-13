//! User preferences are separate from a saved trace workspace. Hosts own I/O.
use crate::wave::model::Link;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub version: u32,
    pub theme: String,
    pub link_by_default: Link,
    pub autosave: bool,
    pub recent_traces: Vec<String>,
    pub recent_workspaces: Vec<String>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            theme: "one-dark".into(),
            link_by_default: Link::default(),
            autosave: true,
            recent_traces: vec![],
            recent_workspaces: vec![],
        }
    }
}
impl Preferences {
    pub fn parse(bytes: &[u8]) -> anyhow::Result<Self> {
        anyhow::ensure!(bytes.len() <= 1024 * 1024, "preferences file is too large");
        let prefs: Self = serde_json::from_slice(bytes)?;
        anyhow::ensure!(prefs.version == 1, "unsupported preferences version");
        Ok(prefs)
    }
    pub fn remember(list: &mut Vec<String>, uri: String) {
        list.retain(|entry| entry != &uri);
        list.insert(0, uri);
        list.truncate(20);
    }
}
