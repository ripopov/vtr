//! Save destinations, restore precedence and an explicit-clock save scheduler.
//! Hosts move opaque bytes and echo tickets. No filesystem or GUI lives here.

use super::Workspace;
use crate::Instant;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;

pub const IDLE: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Target {
    File { uri: String },
    FallbackFile { uri: String },
    Storage { key: String },
}
impl Target {
    pub fn location(&self) -> &str {
        match self {
            Self::File { uri } | Self::FallbackFile { uri } => uri,
            Self::Storage { .. } => "storage:workspace",
        }
    }
    pub(crate) fn trace_reference(&self, trace_uri: &str) -> Result<String> {
        let trace = url::Url::parse(trace_uri).context("invalid trace URI")?;
        if let Self::File { uri } = self {
            let directory = url::Url::parse(uri)?.join(".")?;
            if let Some(relative) = directory.make_relative(&trace) {
                return Ok(relative);
            }
        }
        Ok(trace.into())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Persistence {
    #[default]
    Disabled,
    Auto,
    Explicit(Target),
    /// VS Code's per-workspace storage preference.
    Storage,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "lowercase")]
pub enum Content {
    Missing,
    Bytes(Vec<u8>),
    Error(String),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    pub target: Target,
    pub content: Content,
    pub writable: bool,
}

pub struct Selection {
    pub workspace: Option<Workspace>,
    pub origin: Target,
    pub target: Target,
    pub supersedes: Option<String>,
    pub notices: Vec<String>,
}

pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Read a restore candidate. One saved by an older version counts as missing,
/// so the next save overwrites it; a newer or malformed one is an error.
pub(crate) fn read(content: &Content, notices: &mut Vec<String>) -> Result<Option<Workspace>> {
    match content {
        Content::Missing => Ok(None),
        Content::Error(error) => anyhow::bail!("cannot read workspace: {error}"),
        Content::Bytes(bytes) if Workspace::is_outdated(bytes) => {
            notices.push("Discarded a workspace saved by an older Volna version".into());
            Ok(None)
        }
        Content::Bytes(bytes) => Workspace::parse(bytes).map(Some),
    }
}

/// Fallbacks identify the exact sidecar they were based on, not its timestamp.
/// The chosen target remains authoritative until a successful explicit change.
pub fn select(sidecar: Candidate, fallback: Candidate, storage: bool) -> Result<Selection> {
    let mut notices = Vec::new();
    let side = read(&sidecar.content, &mut notices)?;
    let back = read(&fallback.content, &mut notices)?;
    let base = match &sidecar.content {
        Content::Bytes(bytes) => Some(hash(bytes)),
        _ => None,
    };
    if let Some(back) = back {
        if side.is_none() || back.supersedes == base {
            return Ok(Selection {
                supersedes: back.supersedes.clone(),
                workspace: Some(back),
                origin: fallback.target.clone(),
                target: fallback.target,
                notices,
            });
        }
        notices.push("Saved fallback was set aside because the sidecar changed".into());
    }
    let use_fallback = storage || !sidecar.writable;
    Ok(Selection {
        workspace: side,
        origin: sidecar.target.clone(),
        target: if use_fallback {
            fallback.target
        } else {
            sidecar.target
        },
        supersedes: use_fallback.then_some(base).flatten(),
        notices,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveTicket {
    pub target: Target,
    #[serde(with = "decimal")]
    pub epoch: u64,
    #[serde(with = "decimal")]
    pub revision: u64,
}
mod decimal {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(value: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&value.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug)]
struct Pending {
    ticket: SaveTicket,
    switch: bool,
}

/// One outstanding write per editor also serializes Save As with idle saves.
/// Acknowledgements can advance only the exact snapshot they identify.
#[derive(Debug, Default)]
pub struct Scheduler {
    pub policy: Persistence,
    target: Option<Target>,
    supersedes: Option<String>,
    epoch: u64,
    revision: u64,
    acked: u64,
    changed_at: Option<Instant>,
    outstanding: Option<Pending>,
    destination: Option<Target>,
    explicit: bool,
    suspended: bool,
    last_error: Option<String>,
}

impl Scheduler {
    pub fn target(&self) -> Option<&Target> {
        self.target.as_ref()
    }
    pub(crate) fn supersedes(&self) -> Option<&str> {
        self.supersedes.as_deref()
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn dirty(&self) -> bool {
        self.revision > self.acked
    }
    pub fn suspended(&self) -> bool {
        self.suspended
    }
    pub fn error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }
    pub fn outstanding(&self) -> Option<&SaveTicket> {
        self.outstanding.as_ref().map(|p| &p.ticket)
    }
    pub fn enabled(&self) -> bool {
        self.policy != Persistence::Disabled
    }
    /// Unsaved changes exist and a write to the target is allowed.
    pub(crate) fn wants_write(&self) -> bool {
        self.enabled() && self.dirty() && !self.suspended && self.target.is_some()
    }

    /// Begin a fully loaded workspace, never a partially opened trace.
    pub fn begin(&mut self, target: Option<Target>, supersedes: Option<String>) -> Result<()> {
        ensure!(
            self.outstanding.is_none(),
            "workspace write still outstanding"
        );
        self.epoch = self
            .epoch
            .checked_add(1)
            .context("workspace epochs exhausted")?;
        self.target = target;
        self.supersedes = supersedes;
        self.acked = self.revision;
        self.changed_at = None;
        self.destination = None;
        self.explicit = false;
        self.suspended = false;
        self.last_error = None;
        Ok(())
    }

    pub fn suspend(&mut self, error: String) {
        self.suspended = true;
        self.last_error = Some(error);
    }

    pub fn changed(&mut self, now: Instant) {
        if !self.enabled() {
            return;
        }
        if let Some(revision) = self.revision.checked_add(1) {
            self.revision = revision;
        } else {
            self.suspend("workspace revisions exhausted".into());
        }
        self.changed_at = Some(now);
    }

    pub fn save(&mut self) {
        self.explicit = true;
    }
    pub fn save_as(&mut self, target: Target) {
        self.destination = Some(target);
        self.explicit = true;
    }

    /// Request a snapshot only when idle or forced. Animation extends the idle
    /// deadline but does not create revisions. Disabled policy forbids all writes.
    pub fn next(&mut self, now: Instant, animating: bool, force: bool) -> Option<SaveTicket> {
        if !self.enabled() || self.outstanding.is_some() {
            return None;
        }
        if animating {
            self.changed_at = Some(now);
        }
        if self.suspended && !self.explicit {
            return None;
        }
        if !self.explicit
            && (!self.dirty()
                || (!force
                    && self
                        .changed_at
                        .is_none_or(|at| now.saturating_duration_since(at) < IDLE)))
        {
            return None;
        }
        let switch = self.destination.is_some();
        let target = self.destination.take().or_else(|| self.target.clone())?;
        let ticket = SaveTicket {
            target,
            epoch: self.epoch,
            revision: self.revision,
        };
        self.outstanding = Some(Pending {
            ticket: ticket.clone(),
            switch,
        });
        self.explicit = false;
        Some(ticket)
    }

    /// Returns false for stale, duplicate, or foreign acknowledgements.
    pub fn acknowledge(
        &mut self,
        ticket: &SaveTicket,
        error: Option<String>,
        now: Instant,
    ) -> bool {
        let Some(pending) = &self.outstanding else {
            return false;
        };
        if pending.ticket != *ticket || ticket.epoch != self.epoch {
            return false;
        }
        let pending = self.outstanding.take().unwrap();
        if let Some(error) = error {
            self.last_error = Some(error);
            self.changed_at = Some(now);
        } else {
            self.acked = ticket.revision;
            self.last_error = None;
            self.suspended = false;
            if pending.switch {
                self.target = Some(ticket.target.clone());
                self.supersedes = None;
            }
        }
        true
    }
}
