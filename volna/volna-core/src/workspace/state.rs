//! Machine state that is not a setting: the recent traces and workspaces.
//! Plain versioned JSON written only by the app; never synced or hand-edited.
use super::recent::Recent;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 3;
pub const MAX_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct State {
    pub version: u32,
    /// Traces and workspaces, newest first.
    pub recent: Vec<Recent>,
}

impl State {
    pub fn new() -> Self {
        Self {
            version: VERSION,
            ..Default::default()
        }
    }

    /// An older version starts empty and is replaced on the next save. An
    /// unreadable or newer file yields an error; callers fall back to an
    /// empty list and must not overwrite the file.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        #[derive(Deserialize)]
        struct Version {
            version: u32,
        }
        ensure!(bytes.len() <= MAX_BYTES, "state file is too large");
        let Version { version } = serde_json::from_slice(bytes)?;
        ensure!(version <= VERSION, "unsupported state version");
        if version < VERSION {
            return Ok(Self::new());
        }
        Ok(serde_json::from_slice(bytes)?)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).expect("state serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::recent::RecentKind;

    #[test]
    fn round_trips_and_rejects_other_versions() {
        let mut state = State::new();
        state.recent.push(Recent {
            uri: "file:///w.volna.json".into(),
            kind: RecentKind::Workspace,
            trace: Some("file:///t.vtr".into()),
            opened: 7,
        });
        state.recent.push(Recent {
            uri: "file:///t.vtr".into(),
            kind: RecentKind::Trace,
            trace: None,
            opened: 5,
        });
        assert_eq!(State::parse(&state.to_bytes()).unwrap(), state);
        let old = br#"{"version":2,"recentTraces":["file:///t.vtr"]}"#;
        assert_eq!(State::parse(old).unwrap(), State::new());
        assert!(State::parse(b"{\"version\":4}").is_err());
        assert!(State::parse(b"nope").is_err());
        assert_eq!(State::parse(b"{\"version\":3}").unwrap(), State::new());
    }
}
