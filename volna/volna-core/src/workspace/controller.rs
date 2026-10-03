//! Workspace lifecycle over the core's command/event loop. Hosts only read,
//! write and choose locations; interpretation and save ordering stay here.
use super::{
    RestorePlan, Workspace,
    persistence::{self, Candidate, Persistence, SaveTicket, Scheduler, Target},
};
use crate::trace::TraceId;
use crate::{App, Event, Instant};
use anyhow::{Context, Result, ensure};
use volna_trace::session::OpenSpec;

#[derive(Default)]
pub struct State {
    pub scheduler: Scheduler,
    pub state: super::state::State,
    pub recent: super::recent::View,
    /// Trace A's durable identity: it owns the workspace.
    pub trace_uri: Option<String>,
    pub notices: Vec<String>,
    pub(crate) loading: bool,
    opening_uri: Option<String>,
    pending: Option<Transition>,
    /// A workspace waiting for the other traces it names to open.
    waiting: Option<Waiting>,
}

/// A workspace to restore once the traces it names are open.
struct Waiting {
    workspace: Box<Workspace>,
    /// Where it was read from: its trace paths are relative to this.
    origin: Target,
    /// Where it saves to from now on.
    target: Target,
    supersedes: Option<String>,
    notices: Vec<String>,
    /// Chosen with Open Workspace, rather than found beside trace A.
    explicit: bool,
    traces: std::collections::BTreeSet<TraceId>,
}

enum Transition {
    Open {
        spec: OpenSpec,
        uri: Option<String>,
    },
    Close,
    /// Open Workspace, once the open workspace is flushed.
    OpenWorkspace(Box<Waiting>),
    Restore {
        plan: Box<RestorePlan>,
        target: Target,
        supersedes: Option<String>,
    },
    Quit,
}

/// How the core opens a trace a workspace names: a local file by its path.
/// Other locations are the host's to open.
fn spec_for(uri: &str) -> Result<OpenSpec> {
    #[cfg(not(target_family = "wasm"))]
    if let Ok(url) = url::Url::parse(uri)
        && url.scheme() == "file"
        && let Ok(path) = url.to_file_path()
    {
        return Ok(OpenSpec::Path(path));
    }
    anyhow::bail!("{uri} is not a local file")
}

/// The URI of every trace but A that `waiting` names, by letter.
fn resolve_traces(waiting: &Waiting) -> Result<Vec<(TraceId, String)>> {
    let location = waiting.origin.location();
    waiting
        .workspace
        .traces
        .iter()
        .filter(|saved| saved.letter != TraceId::A)
        .map(|saved| Ok((saved.letter, super::resolve_trace(&saved.path, location)?)))
        .collect()
}

impl App {
    pub fn configure_persistence(&mut self, policy: Persistence) {
        self.workspace.scheduler.policy = policy;
    }

    /// Native and web hosts supply a canonical, durable resource identity.
    pub fn open_resource(&mut self, spec: OpenSpec, trace_uri: String) {
        self.transition(Transition::Open {
            spec,
            uri: Some(trace_uri),
        });
    }

    pub(crate) fn open(&mut self, spec: OpenSpec) {
        self.transition(Transition::Open { spec, uri: None });
    }

    pub fn close_trace(&mut self) {
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
        // Quit is final: nothing queued after it replaces it.
        if matches!(self.workspace.pending, Some(Transition::Quit)) {
            return self.advance_transition();
        }
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
        if scheduler.wants_write() {
            self.persist_workspace(Instant::now(), true);
            return;
        }
        let transition = self.workspace.pending.take().unwrap();
        match transition {
            Transition::Open { spec, uri } => {
                if !self.begin_unsaved() {
                    return;
                }
                self.workspace.opening_uri = uri;
                self.workspace.trace_uri = None;
                self.workspace.loading = true;
                self.workspace.notices.clear();
                self.open_now(spec);
            }
            Transition::Close => {
                if !self.begin_unsaved() {
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
            Transition::OpenWorkspace(waiting) => {
                // Detach from the flushed workspace first: the traces the
                // new one opens and closes must never save into the old.
                if !self.begin_unsaved() {
                    return;
                }
                let target = waiting.target.clone();
                if let Err(error) = self.restore_when_open(*waiting) {
                    self.restore_failed(target, error);
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
                    self.notice_all(report.notices);
                }
                Err(error) => self.notice(error.to_string()),
            },
            Transition::Quit => self.events.push(Event::Quit),
        }
    }

    /// Detach the scheduler from its target; false when it refused.
    fn begin_unsaved(&mut self) -> bool {
        match self.workspace.scheduler.begin(None, None) {
            Ok(()) => true,
            Err(error) => {
                self.notice(error.to_string());
                false
            }
        }
    }

    fn notice_all(&mut self, texts: Vec<String>) {
        for text in texts {
            self.notice(text);
        }
    }

    pub(crate) fn session_ready_for_workspace(&mut self) {
        self.workspace.trace_uri = self.workspace.opening_uri.take();
        self.workspace.waiting = None;
        let uri = self.workspace.trace_uri.clone();
        self.doc.set_uri(TraceId::A, uri);
        if let Some(uri) = &self.workspace.trace_uri {
            self.events.push(Event::TraceOpened {
                trace_uri: uri.clone(),
            });
        }
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
            Persistence::Explicit(_) => {
                let mut notices = Vec::new();
                persistence::read(&sidecar.content, &mut notices).map(|workspace| {
                    persistence::Selection {
                        workspace,
                        origin: sidecar.target.clone(),
                        target: sidecar.target.clone(),
                        supersedes: None,
                        notices,
                    }
                })
            }
            policy => {
                persistence::select(sidecar.clone(), fallback, *policy == Persistence::Storage)
            }
        };
        let result = selection.map(|selected| match selected.workspace {
            Some(workspace) => self.restore_when_open(Waiting {
                workspace: Box::new(workspace),
                origin: selected.origin,
                target: selected.target,
                supersedes: selected.supersedes,
                notices: selected.notices,
                explicit: false,
                traces: Default::default(),
            }),
            None => {
                let begun = self
                    .workspace
                    .scheduler
                    .begin(Some(selected.target), selected.supersedes);
                self.notice_all(selected.notices);
                self.workspace.loading = false;
                begun
            }
        });
        if let Err(error) = result.and_then(|r| r) {
            self.restore_failed(sidecar.target, error);
        }
        self.changed();
    }

    /// An automatic restore failed: keep the destination for an explicit
    /// Save, and pause autosave so the file is not overwritten.
    fn restore_failed(&mut self, target: Target, error: anyhow::Error) {
        self.workspace.loading = false;
        let _ = self.workspace.scheduler.begin(Some(target), None);
        let text = format!("Workspace restore failed; autosave paused: {error:#}");
        self.workspace.scheduler.suspend(text.clone());
        self.notice(text);
    }

    /// Restore `waiting` once every trace it names is open. A workspace
    /// chosen with Open Workspace makes the trace set its own: it opens the
    /// traces it names and closes the others. One restored beside trace A
    /// only opens what it names under free letters: traces the user opened
    /// with A (`volna A B`) stay, and its rows resolve against them.
    fn restore_when_open(&mut self, mut waiting: Waiting) -> Result<()> {
        // Resolve every path before changing any trace.
        let traces = resolve_traces(&waiting)?;
        let named: std::collections::BTreeSet<TraceId> =
            waiting.workspace.traces.iter().map(|t| t.letter).collect();
        for (letter, uri) in traces {
            let slot = self.doc.traces().get(letter);
            if slot.is_some_and(|slot| slot.uri.as_deref() == Some(uri.as_str())) {
                continue;
            }
            if slot.is_some() {
                if !waiting.explicit {
                    continue;
                }
                self.drop_trace(letter);
            }
            match spec_for(&uri).and_then(|spec| self.doc.add_trace_as(spec, letter)) {
                Ok(trace) => {
                    self.doc.set_uri(trace, Some(uri));
                    waiting.traces.insert(trace);
                }
                Err(error) => waiting
                    .notices
                    .push(format!("Trace {letter} not opened: {error:#}")),
            }
        }
        let extra: Vec<TraceId> = self
            .doc
            .traces()
            .ids()
            .filter(|id| waiting.explicit && !named.contains(id))
            .collect();
        for trace in extra {
            self.drop_trace(trace);
        }
        // Autosave waits with the restore; an automatic one already waits.
        if !waiting.traces.is_empty() {
            self.workspace.loading = true;
        }
        self.workspace.waiting = Some(waiting);
        self.changed();
        self.restore_if_ready()
    }

    /// Take a trace out without journaling it: a workspace about to be
    /// restored replaces the whole session, history included.
    fn drop_trace(&mut self, trace: TraceId) {
        self.forget_trace(trace);
    }

    fn restore_if_ready(&mut self) -> Result<()> {
        if self
            .workspace
            .waiting
            .as_ref()
            .is_none_or(|w| !w.traces.is_empty())
        {
            return Ok(());
        }
        let waiting = self.workspace.waiting.take().expect("checked above");
        let uri = self
            .workspace
            .trace_uri
            .clone()
            .context("open the referenced trace first")?;
        let plan = waiting
            .workspace
            .prepare(self, &uri, waiting.origin.location())?;
        if waiting.explicit {
            // The open workspace is flushed before the restore replaces it.
            self.workspace.loading = false;
            self.notice_all(waiting.notices);
            self.transition(Transition::Restore {
                plan: Box::new(plan),
                target: waiting.target,
                supersedes: waiting.supersedes,
            });
            return Ok(());
        }
        let report = plan.commit(self)?;
        self.notice_all(report.notices);
        self.workspace
            .scheduler
            .begin(Some(waiting.target), waiting.supersedes)?;
        self.notice_all(waiting.notices);
        self.workspace.loading = false;
        Ok(())
    }

    /// A trace a waiting workspace named finished opening.
    pub(crate) fn workspace_trace_joined(&mut self, trace: TraceId) {
        self.workspace_trace_settled(trace, None);
    }

    /// A trace a waiting workspace named failed to open; its rows restore
    /// unresolved.
    pub(crate) fn workspace_trace_failed(&mut self, trace: TraceId, error: String) {
        self.workspace_trace_settled(trace, Some(error));
    }

    fn workspace_trace_settled(&mut self, trace: TraceId, error: Option<String>) {
        let Some(waiting) = self.workspace.waiting.as_mut() else {
            return;
        };
        if !waiting.traces.remove(&trace) {
            return;
        }
        if let Some(error) = error {
            waiting
                .notices
                .push(format!("Trace {trace} not opened: {error}"));
        }
        let target = waiting.target.clone();
        if let Err(error) = self.restore_if_ready() {
            self.restore_failed(target, error);
        }
        self.changed();
    }

    /// Prepare first, then flush the previous workspace before the atomic commit.
    pub fn open_workspace(&mut self, target: Target, bytes: &[u8]) -> Result<()> {
        ensure!(
            self.workspace.scheduler.enabled(),
            "workspace persistence is disabled"
        );
        ensure!(
            self.workspace.trace_uri.is_some(),
            "open the referenced trace first"
        );
        let workspace = Workspace::parse(bytes)?;
        let waiting = Waiting {
            workspace: Box::new(workspace),
            origin: target.clone(),
            target,
            supersedes: None,
            notices: Vec::new(),
            explicit: true,
            traces: Default::default(),
        };
        resolve_traces(&waiting)?;
        // The open workspace is flushed before any of its traces change.
        self.transition(Transition::OpenWorkspace(Box::new(waiting)));
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
        if self.workspace.trace_uri.is_none() {
            return;
        }
        let animating = self.is_animating();
        let Some(ticket) = self.workspace.scheduler.next(now, animating, force) else {
            return;
        };
        let base = if self.workspace.scheduler.target() == Some(&ticket.target) {
            self.workspace.scheduler.supersedes().map(str::to_owned)
        } else {
            None
        };
        let target = &ticket.target;
        let path_of = |slot: &crate::trace::TraceSlot| {
            slot.uri
                .as_deref()
                .map(|uri| target.trace_reference(uri))
                .transpose()
        };
        let bytes = Workspace::capture(self, path_of, base).and_then(|w| w.to_bytes());
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
            // A retry failing the same way is not news.
            let text = format!("Workspace not saved: {error}");
            if !self.workspace.notices.contains(&text) {
                self.workspace
                    .notices
                    .retain(|s| !s.starts_with("Workspace not saved:"));
                self.notice(text);
            }
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
        !self.workspace.loading && s.wants_write() && s.outstanding().is_none()
    }
}
