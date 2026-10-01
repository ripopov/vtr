//! Scope activity meters (docs/hierarchy-activity.html): how many of each
//! scope's signals change in the wave viewport, for every trace with an
//! activity index.
//!
//! [`ActivityModel::sync`] turns the viewport into each trace's window and
//! asks for at most one classification and one read per trace at a time, so
//! a pan or zoom costs one pass per frame, coalesced, off the UI thread. A
//! classification installs at once; while it leaves signals undecided the
//! rows show a range, and the read that follows replaces it with the exact
//! counts unless the view has moved on, in which case the read is dropped.
//! Meters are derived view state: nothing here enters the undo journal or
//! the workspace.

use std::collections::HashMap;
use std::sync::Arc;

use crate::data::{ActivityCounter, ActivityCounts, ScopeId};
use crate::remote::memory::MemoryBudget;
use crate::session::{LoadRequest, LoadResult};
use crate::settings::ActivityIndexPolicy;
use crate::trace::{Placement, TraceId, TraceSet};
use crate::wave::Viewport;

#[derive(Default)]
pub struct ActivityModel {
    traces: HashMap<TraceId, TraceActivity>,
    policy: Option<ActivityIndexPolicy>,
}

struct TraceActivity {
    generation: u64,
    counter: Counter,
    /// The latest counts, for the window they name.
    shown: Option<Arc<ActivityCounts>>,
    /// The window of the classification in flight.
    classifying: Option<(u64, u64)>,
    /// The window of the read in flight.
    resolving: Option<(u64, u64)>,
    /// A window whose classification or read failed: not asked again.
    failed: Option<(u64, u64)>,
    build: Build,
}

enum Build {
    Offered,
    Queued,
    Running {
        control: Arc<vtr::activity::BuildControl>,
        cancelling: bool,
    },
    Dismissed,
    Failed(String),
}

impl Drop for TraceActivity {
    fn drop(&mut self) {
        if let Build::Running { control, .. } = &self.build {
            control.cancel();
        }
    }
}

/// The build banner's toolkit-independent state for one trace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActivityBuildState {
    Offer {
        estimated_seconds: u64,
    },
    Building {
        progress: vtr::activity::BuildProgress,
        cancelling: bool,
    },
    Failed {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityBuildView {
    pub trace: TraceId,
    pub name: String,
    pub state: ActivityBuildState,
}

impl ActivityBuildView {
    pub fn message(&self) -> String {
        match &self.state {
            ActivityBuildState::Offer { estimated_seconds } => {
                format!("No activity index yet · about {estimated_seconds} s")
            }
            ActivityBuildState::Building {
                progress,
                cancelling: true,
            } => format!(
                "Cancelling · {} of {} blocks",
                progress.completed, progress.total
            ),
            ActivityBuildState::Building { progress, .. } if progress.total == 0 => {
                "Preparing the activity index…".into()
            }
            ActivityBuildState::Building { progress, .. } => format!(
                "Building the activity index · {} of {} blocks",
                progress.completed, progress.total
            ),
            ActivityBuildState::Failed { message } => format!("Activity index failed: {message}"),
        }
    }
}

enum Counter {
    Absent,
    Requested,
    Ready(Arc<ActivityCounter>),
    Failed,
}

/// The trace times a session-time viewport shows: the changes at trace
/// time `t` are drawn at `t × scale`.
pub fn trace_window(viewport: Viewport, placement: Placement) -> (u64, u64) {
    let scale = placement.scale() as f64;
    let t0 = (viewport.start.max(0.0) / scale).ceil();
    let t1 = (viewport.end.max(0.0) / scale).floor();
    (
        t0.min(u64::MAX as f64) as u64,
        t1.min(u64::MAX as f64) as u64,
    )
}

impl ActivityModel {
    /// The requests that bring every indexed trace's counts to `viewport`;
    /// forgets traces that closed or were replaced.
    pub(crate) fn sync(
        &mut self,
        traces: &TraceSet,
        viewport: Viewport,
        budget: &MemoryBudget,
        policy: ActivityIndexPolicy,
        meters_visible: bool,
    ) -> Vec<LoadRequest> {
        let mut requests = Vec::new();
        let policy_changed = self.policy.replace(policy) != Some(policy);
        self.prune(traces);
        for slot in traces.iter() {
            let (Some(session), trace) = (slot.session(), slot.id) else {
                continue;
            };
            if session.activity().is_none() && session.activity_build_info().is_none() {
                continue;
            }
            let a = self.traces.entry(trace).or_insert_with(|| TraceActivity {
                generation: slot.generation(),
                counter: Counter::Absent,
                shown: None,
                classifying: None,
                resolving: None,
                failed: None,
                build: Build::Offered,
            });
            let Some(index) = session.activity() else {
                if session.activity_build_info().is_some() {
                    if policy == ActivityIndexPolicy::Always
                        && (matches!(a.build, Build::Offered)
                            || policy_changed && matches!(a.build, Build::Dismissed))
                    {
                        a.build = Build::Queued;
                    }
                    if matches!(a.build, Build::Queued) {
                        let control = Arc::new(vtr::activity::BuildControl::default());
                        a.build = Build::Running {
                            control: Arc::clone(&control),
                            cancelling: false,
                        };
                        requests.push(LoadRequest::BuildActivity {
                            trace,
                            generation: a.generation,
                            session: Arc::clone(session),
                            options: vtr::activity::BuildOptions {
                                control: Some(control),
                                memory: (512u64 << 20).min(
                                    (budget.limit().saturating_sub(budget.used()) / 2)
                                        .saturating_sub(
                                            (session.info().signal_count as u64).saturating_mul(24),
                                        ),
                                ),
                                ..Default::default()
                            },
                            cache_dir: vtr::activity::default_cache_dir(),
                            budget: budget.clone(),
                        });
                    }
                }
                continue;
            };
            if !meters_visible {
                continue;
            }
            if matches!(a.counter, Counter::Absent) {
                a.counter = Counter::Requested;
                requests.push(LoadRequest::ActivityCounter {
                    trace,
                    generation: a.generation,
                    session: Arc::clone(session),
                    budget: budget.clone(),
                });
            }
            let Counter::Ready(counter) = &a.counter else {
                continue;
            };
            let want = trace_window(viewport, slot.placement());
            if a.failed == Some(want) {
                continue;
            }
            let shown = a.shown.as_ref().filter(|c| c.window == want);
            match shown {
                None if a.classifying.is_none() => {
                    a.classifying = Some(want);
                    requests.push(LoadRequest::Activity {
                        trace,
                        generation: a.generation,
                        window: want,
                        index,
                        counter: Arc::clone(counter),
                        budget: budget.clone(),
                    });
                }
                Some(counts) if !counts.exact() && a.resolving.is_none() => {
                    a.resolving = Some(want);
                    requests.push(LoadRequest::ResolveActivity {
                        trace,
                        generation: a.generation,
                        session: Arc::clone(session),
                        counter: Arc::clone(counter),
                        counts: Arc::clone(counts),
                        budget: budget.clone(),
                    });
                }
                _ => {}
            }
        }
        requests
    }

    /// Takes an activity result; true when what the rows show changed.
    pub(crate) fn deliver(
        &mut self,
        result: LoadResult,
        traces: &TraceSet,
        viewport: Viewport,
    ) -> anyhow::Result<bool> {
        let (trace, generation) = match &result {
            LoadResult::ActivityBuilt {
                trace, generation, ..
            }
            | LoadResult::ActivityCounter {
                trace, generation, ..
            }
            | LoadResult::Activity {
                trace, generation, ..
            }
            | LoadResult::ActivityResolved {
                trace, generation, ..
            } => (*trace, *generation),
            _ => return Ok(false),
        };
        let Some(slot) = traces
            .get(trace)
            .filter(|s| s.generation() == generation && s.session().is_some())
        else {
            return Ok(false);
        };
        let want = trace_window(viewport, slot.placement());
        match result {
            LoadResult::ActivityBuilt {
                trace,
                generation,
                result,
            } => {
                let Some(a) = self.current(trace, generation) else {
                    return Ok(false);
                };
                let Build::Running { control, .. } = &a.build else {
                    return Ok(false);
                };
                if control.is_cancelled() {
                    a.build = Build::Dismissed;
                    return Ok(true);
                }
                match result {
                    Ok(()) => {
                        a.build = Build::Dismissed;
                        Ok(true)
                    }
                    Err(error) => {
                        a.build = Build::Failed(format!("{error:#}"));
                        Err(error.context(format!("Activity index of trace {trace}")))
                    }
                }
            }
            LoadResult::ActivityCounter {
                trace,
                generation,
                result,
            } => {
                let Some(a) = self.current(trace, generation) else {
                    return Ok(false);
                };
                match result {
                    Ok(counter) => {
                        a.counter = Counter::Ready(counter);
                        Ok(false)
                    }
                    Err(error) => {
                        a.counter = Counter::Failed;
                        Err(error.context(format!("Scope activity of trace {trace}")))
                    }
                }
            }
            LoadResult::Activity {
                trace,
                generation,
                window,
                result,
            } => {
                let Some(a) = self.current(trace, generation) else {
                    return Ok(false);
                };
                if a.classifying == Some(window) {
                    a.classifying = None;
                }
                if window != want {
                    return Ok(false);
                }
                match result {
                    Ok(counts) => {
                        a.shown = Some(counts);
                        Ok(true)
                    }
                    Err(error) => {
                        a.failed = Some(window);
                        a.shown = None;
                        Err(error.context(format!("Scope activity of trace {trace}")))
                    }
                }
            }
            LoadResult::ActivityResolved {
                trace,
                generation,
                window,
                result,
            } => {
                let Some(a) = self.current(trace, generation) else {
                    return Ok(false);
                };
                if a.resolving == Some(window) {
                    a.resolving = None;
                }
                // A read for a window the view has left is dropped.
                if window != want || a.shown.as_ref().is_none_or(|c| c.window != window) {
                    return Ok(false);
                }
                match result {
                    Ok(counts) => {
                        a.shown = Some(counts);
                        Ok(true)
                    }
                    Err(error) => {
                        a.failed = Some(window);
                        a.shown = None;
                        Err(error.context(format!("Scope activity of trace {trace}")))
                    }
                }
            }
            _ => Ok(false),
        }
    }

    fn current(&mut self, trace: TraceId, generation: u64) -> Option<&mut TraceActivity> {
        self.traces
            .get_mut(&trace)
            .filter(|a| a.generation == generation)
    }

    /// Queue an explicit build. A running build cannot be queued again.
    pub(crate) fn start_build(&mut self, trace: TraceId) {
        if let Some(a) = self.traces.get_mut(&trace)
            && !matches!(a.build, Build::Running { .. })
        {
            a.build = Build::Queued;
        }
    }

    pub(crate) fn dismiss_build(&mut self, trace: TraceId) {
        if let Some(a) = self.traces.get_mut(&trace)
            && !matches!(a.build, Build::Running { .. })
        {
            a.build = Build::Dismissed;
        }
    }

    pub(crate) fn cancel_build(&mut self, trace: TraceId) {
        if let Some(a) = self.traces.get_mut(&trace) {
            if matches!(a.build, Build::Queued) {
                a.build = Build::Dismissed;
            }
            if let Build::Running {
                control,
                cancelling,
            } = &mut a.build
                && control.cancel()
            {
                *cancelling = true;
            }
        }
    }

    /// Cancels builds and releases derived state of closed/replaced traces.
    pub(crate) fn prune(&mut self, traces: &TraceSet) {
        self.traces.retain(|&t, a| {
            traces
                .get(t)
                .is_some_and(|slot| slot.generation() == a.generation && slot.session().is_some())
        });
    }

    /// Build offers and progress for locally buildable, unindexed traces.
    pub fn builds(&self, traces: &TraceSet, policy: ActivityIndexPolicy) -> Vec<ActivityBuildView> {
        traces
            .iter()
            .filter_map(|slot| {
                let session = slot.session()?;
                if session.activity().is_some() {
                    return None;
                }
                let info = session.activity_build_info()?;
                let a = self
                    .traces
                    .get(&slot.id)
                    .filter(|a| a.generation == slot.generation())?;
                let state = match &a.build {
                    Build::Running {
                        control,
                        cancelling,
                    } => ActivityBuildState::Building {
                        progress: control.progress(),
                        cancelling: *cancelling,
                    },
                    Build::Queued => ActivityBuildState::Building {
                        progress: Default::default(),
                        cancelling: false,
                    },
                    Build::Offered if policy != ActivityIndexPolicy::Never => {
                        ActivityBuildState::Offer {
                            estimated_seconds: info.estimated_seconds(),
                        }
                    }
                    Build::Failed(message) if policy != ActivityIndexPolicy::Never => {
                        ActivityBuildState::Failed {
                            message: message.clone(),
                        }
                    }
                    _ => return None,
                };
                Some(ActivityBuildView {
                    trace: slot.id,
                    name: session.info().name.clone(),
                    state,
                })
            })
            .collect()
    }

    /// The counts a trace shows, for the window they name.
    pub fn counts(&self, trace: TraceId) -> Option<&Arc<ActivityCounts>> {
        self.traces.get(&trace)?.shown.as_ref()
    }

    /// `(changing, upper)` below a scope, only when the trace generation
    /// and requested window still match the shown counts.
    pub fn get(
        &self,
        trace: TraceId,
        generation: u64,
        window: (u64, u64),
        scope: ScopeId,
    ) -> Option<(u32, u32)> {
        let a = self.traces.get(&trace)?;
        let counts = a.shown.as_ref()?;
        (a.generation == generation && counts.window == window)
            .then(|| counts.get(scope))
            .flatten()
    }
}
