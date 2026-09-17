//! Workspace lifecycle over the core's command/event loop. Hosts only read,
//! write and choose locations; interpretation and save ordering stay here.
use super::{
    RestorePlan, Workspace,
    persistence::{self, Candidate, Content, Persistence, SaveTicket, Scheduler, Target},
};
use crate::session::OpenSpec;
use crate::{App, Event, Instant};
use anyhow::{Context, Result, ensure};

#[derive(Default)]
pub struct State {
    pub scheduler: Scheduler,
    pub state: super::state::State,
    pub trace_uri: Option<String>,
    pub notices: Vec<String>,
    pub(crate) loading: bool,
    pub(crate) opening_uri: Option<String>,
    pending: Option<Transition>,
}

enum Transition {
    Open {
        spec: OpenSpec,
        show_all: bool,
        uri: Option<String>,
    },
    Close,
    Restore {
        plan: Box<RestorePlan>,
        target: Target,
        supersedes: Option<String>,
    },
    Quit,
}

impl App {
    pub fn configure_persistence(&mut self, policy: Persistence) {
        self.workspace.scheduler.policy = policy;
    }

    /// Native and web hosts supply a canonical, durable resource identity.
    pub fn open_resource(&mut self, spec: OpenSpec, trace_uri: String) {
        self.transition(Transition::Open {
            spec,
            show_all: false,
            uri: Some(trace_uri),
        });
    }

    pub(crate) fn open_with_workspace(&mut self, spec: OpenSpec, show_all: bool) {
        self.transition(Transition::Open {
            spec,
            show_all,
            uri: None,
        });
    }
    pub(crate) fn close_with_workspace(&mut self) {
        self.transition(Transition::Close);
    }
    pub fn request_quit(&mut self) {
        self.transition(Transition::Quit);
    }

    pub fn report_workspace_error(&mut self, text: String) {
        self.notice(text);
    }

    fn notice(&mut self, text: String) {
        self.workspace.notices.push(text.clone());
        self.events.push(Event::Notice(text));
        self.changed();
    }

    fn transition(&mut self, transition: Transition) {
        self.workspace.pending = Some(transition);
        self.advance_transition();
    }

    fn advance_transition(&mut self) {
        if self.workspace.pending.is_none() {
            return;
        }
        let scheduler = &self.workspace.scheduler;
        if scheduler.outstanding().is_some() {
            return;
        }
        if scheduler.enabled()
            && scheduler.dirty()
            && !scheduler.suspended()
            && scheduler.target().is_some()
        {
            self.persist_workspace(Instant::now(), true);
            return;
        }
        let transition = self.workspace.pending.take().unwrap();
        match transition {
            Transition::Open {
                spec,
                show_all,
                uri,
            } => {
                if let Err(error) = self.workspace.scheduler.begin(None, None) {
                    self.notice(error.to_string());
                    return;
                }
                self.workspace.opening_uri = uri;
                self.workspace.trace_uri = None;
                self.workspace.loading = true;
                self.workspace.notices.clear();
                self.open_now(spec, show_all);
            }
            Transition::Close => {
                if let Err(error) = self.workspace.scheduler.begin(None, None) {
                    self.notice(error.to_string());
                    return;
                }
                let trace_uri = self.workspace.trace_uri.take();
                self.workspace.opening_uri = None;
                self.workspace.loading = false;
                self.close_now();
                if let Some(trace_uri) = trace_uri {
                    self.events.push(Event::TraceClosed { trace_uri });
                }
            }
            Transition::Restore {
                plan,
                target,
                supersedes,
            } => match plan.commit(self) {
                Ok(report) => {
                    if let Err(error) = self.workspace.scheduler.begin(Some(target), supersedes) {
                        self.notice(error.to_string());
                    }
                    self.workspace.loading = false;
                    for text in report.notices {
                        self.notice(text);
                    }
                }
                Err(error) => self.notice(error.to_string()),
            },
            Transition::Quit => self.events.push(Event::Quit),
        }
    }

    pub(crate) fn session_ready_for_workspace(&mut self) {
        self.workspace.trace_uri = self.workspace.opening_uri.take();
        if self.workspace.scheduler.enabled()
            && let Some(uri) = &self.workspace.trace_uri
        {
            self.events.push(Event::LoadWorkspace {
                trace_uri: uri.clone(),
            });
        } else {
            self.workspace.loading = false;
        }
    }

    /// Automatic candidates are read together, including read failures. A
    /// malformed candidate suspends autosave so it cannot be overwritten.
    pub fn restore_candidates(&mut self, trace_uri: &str, sidecar: Candidate, fallback: Candidate) {
        if !self.workspace.loading || self.workspace.trace_uri.as_deref() != Some(trace_uri) {
            return;
        }
        let selection = match &self.workspace.scheduler.policy {
            Persistence::Disabled => {
                self.workspace.loading = false;
                return;
            }
            Persistence::Explicit(_) => match &sidecar.content {
                Content::Bytes(bytes) => {
                    Workspace::parse(bytes).map(|workspace| persistence::Selection {
                        workspace: Some(workspace),
                        origin: sidecar.target.clone(),
                        target: sidecar.target.clone(),
                        supersedes: None,
                        notices: vec![],
                    })
                }
                Content::Missing => Ok(persistence::Selection {
                    workspace: None,
                    origin: sidecar.target.clone(),
                    target: sidecar.target.clone(),
                    supersedes: None,
                    notices: vec![],
                }),
                Content::Error(error) => Err(anyhow::anyhow!(error.clone())),
            },
            policy => {
                persistence::select(sidecar.clone(), fallback, *policy == Persistence::Storage)
            }
        };
        let result = selection.and_then(|selected| {
            let plan = selected
                .workspace
                .map(|w| w.prepare(self, trace_uri, selected.origin.location()))
                .transpose()?;
            if let Some(plan) = plan {
                let report = plan.commit(self)?;
                for text in report.notices {
                    self.notice(text);
                }
            }
            self.workspace
                .scheduler
                .begin(Some(selected.target), selected.supersedes)?;
            for text in selected.notices {
                self.notice(text);
            }
            Ok(())
        });
        self.workspace.loading = false;
        if let Err(error) = result {
            // Retain the intended destination for an explicit Save/Save As.
            let _ = self.workspace.scheduler.begin(Some(sidecar.target), None);
            let text = format!("Workspace restore failed; autosave paused: {error:#}");
            self.workspace.scheduler.suspend(text.clone());
            self.notice(text);
        }
        self.changed();
    }

    /// Prepare first, then flush the previous workspace before the atomic commit.
    pub fn open_workspace(&mut self, target: Target, bytes: &[u8]) -> Result<()> {
        ensure!(
            self.workspace.scheduler.enabled(),
            "workspace persistence is disabled"
        );
        let uri = self
            .workspace
            .trace_uri
            .as_deref()
            .context("open the referenced trace first")?;
        let plan = Workspace::parse(bytes)?.prepare(self, uri, target.location())?;
        self.transition(Transition::Restore {
            plan: Box::new(plan),
            target,
            supersedes: None,
        });
        Ok(())
    }

    pub fn save_workspace(&mut self, destination: Option<Target>) {
        if let Some(target) = destination {
            self.workspace.scheduler.save_as(target);
        } else {
            self.workspace.scheduler.save();
        }
        self.persist_workspace(Instant::now(), true);
    }

    pub fn persist_workspace(&mut self, now: Instant, force: bool) {
        if self.workspace.loading || !self.doc.is_loaded() {
            return;
        }
        let Some(uri) = self.workspace.trace_uri.clone() else {
            return;
        };
        let animating = self.is_animating();
        let Some(ticket) = self.workspace.scheduler.next(now, animating, force) else {
            return;
        };
        let base = if self.workspace.scheduler.target() == Some(&ticket.target) {
            self.workspace.scheduler.supersedes().map(str::to_owned)
        } else {
            None
        };
        let bytes = ticket
            .target
            .trace_reference(&uri)
            .and_then(|reference| Workspace::capture(self, reference, base)?.to_bytes());
        match bytes {
            Ok(bytes) => self.events.push(Event::PersistWorkspace { ticket, bytes }),
            Err(error) => {
                self.workspace_saved(ticket, Some(error.to_string()), now);
            }
        }
    }

    pub fn workspace_saved(&mut self, ticket: SaveTicket, error: Option<String>, now: Instant) {
        if !self
            .workspace
            .scheduler
            .acknowledge(&ticket, error.clone(), now)
        {
            return;
        }
        if let Some(error) = error {
            // A failed flush cancels the transition and keeps all live state.
            self.workspace.pending = None;
            self.notice(format!("Workspace not saved: {error}"));
        } else {
            self.workspace
                .notices
                .retain(|s| !s.starts_with("Workspace not saved:"));
            self.advance_transition();
            self.persist_workspace(now, false);
            self.changed();
        }
    }

    pub(crate) fn workspace_tick(&mut self, now: Instant) -> bool {
        self.persist_workspace(now, false);
        let s = &self.workspace.scheduler;
        !self.workspace.loading
            && s.enabled()
            && s.dirty()
            && !s.suspended()
            && s.outstanding().is_none()
            && s.target().is_some()
    }
}
