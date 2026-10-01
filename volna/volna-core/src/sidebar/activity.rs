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
use crate::trace::{Placement, TraceId, TraceSet};
use crate::wave::Viewport;

#[derive(Default)]
pub struct ActivityModel {
    traces: HashMap<TraceId, TraceActivity>,
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
}

enum Counter {
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
    ) -> Vec<LoadRequest> {
        let mut requests = Vec::new();
        self.traces.retain(|&t, a| {
            traces
                .get(t)
                .is_some_and(|slot| slot.generation() == a.generation && slot.session().is_some())
        });
        for slot in traces.iter() {
            let (Some(session), trace) = (slot.session(), slot.id) else {
                continue;
            };
            let Some(index) = session.activity() else {
                continue;
            };
            let a = self.traces.entry(trace).or_insert_with(|| {
                requests.push(LoadRequest::ActivityCounter {
                    trace,
                    generation: slot.generation(),
                    session: Arc::clone(session),
                    budget: budget.clone(),
                });
                TraceActivity {
                    generation: slot.generation(),
                    counter: Counter::Requested,
                    shown: None,
                    classifying: None,
                    resolving: None,
                    failed: None,
                }
            });
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
            LoadResult::ActivityCounter {
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
