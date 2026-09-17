//! Machine state that is not a setting: the recent traces and workspaces.
//! Plain versioned JSON written only by the app; never synced or hand-edited.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
pub const MAX_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct State {
    pub version: u32,
    /// Newest first.
    pub recent_traces: Vec<String>,
    pub recent_workspaces: Vec<String>,
}

impl State {
    pub fn new() -> Self {
        Self {
            version: VERSION,
            ..Default::default()
        }
    }

    /// An unreadable or wrong-version file yields an error; callers fall back
    /// to empty lists and must not overwrite the file.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() <= MAX_BYTES, "state file is too large");
        let state: Self = serde_json::from_slice(bytes)?;
        ensure!(state.version == VERSION, "unsupported state version");
        Ok(state)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).expect("state serializes")
    }

    /// Move `uri` to the front of `list`, keeping at most `limit` entries.
    pub fn remember(list: &mut Vec<String>, uri: String, limit: usize) {
        list.retain(|entry| entry != &uri);
        list.insert(0, uri);
        list.truncate(limit.max(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_rejects_other_versions() {
        let mut state = State::new();
        for i in 0..5 {
            State::remember(&mut state.recent_traces, format!("file:///{i}.vtr"), 3);
        }
        assert_eq!(
            state.recent_traces,
            ["file:///4.vtr", "file:///3.vtr", "file:///2.vtr"]
        );
        State::remember(&mut state.recent_traces, "file:///2.vtr".into(), 3);
        assert_eq!(state.recent_traces[0], "file:///2.vtr");
        assert_eq!(State::parse(&state.to_bytes()).unwrap(), state);
        assert!(State::parse(b"{\"version\":2}").is_err());
        assert!(State::parse(b"nope").is_err());
        assert_eq!(State::parse(b"{\"version\":1}").unwrap(), State::new());
    }
}
