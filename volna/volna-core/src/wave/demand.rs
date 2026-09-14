//! Latest-intent waveform loading, independent of executor and transport.
//! This controller owns its query session. A host polls it outside painting;
//! returned immutable bins/windows feed the toolkit-independent painters.
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use vtr_query::{
    Budget, Error, Grid, Interval, Reservation, Result,
    session::{AsyncSession, Continuation, Query, Reply},
    summary::WaveBin,
    wave::Limits,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Demand {
    Summary { signal: u32, grid: Grid },
    Window { signal: u32, interval: Interval },
}
impl Demand {
    pub fn signal(self) -> u32 {
        match self {
            Self::Summary { signal, .. } | Self::Window { signal, .. } => signal,
        }
    }
    pub fn grid(self) -> Option<Grid> {
        match self {
            Self::Summary { grid, .. } => Some(grid),
            _ => None,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// A page or a lifecycle transition was accepted; poll again outside paint.
    Advanced,
    /// Desired coverage is complete, absent, or failed; no work remains.
    Idle,
    /// The shared session cannot admit work yet. Retry when its host makes
    /// progress or after releasing pinned data; never spin on this result.
    Backpressure,
}
struct Running<T> {
    demand: Demand,
    task: T,
    cancelled: bool,
    cursor: Option<Continuation>,
}

/// State for one row/coverage demand within the document's shared session.
/// Access through `WaveDemands::slot`; mutation goes through the scheduler.
pub struct WaveSlot<T> {
    limits: Limits,
    max_items: u32,
    desired: Option<Demand>,
    displayed: Option<Demand>,
    running: Option<Running<T>>,
    next: Option<(Demand, Continuation)>,
    release: Option<Continuation>,
    bins: Vec<Arc<WaveBin>>,
    exact: Option<Arc<super::exact::ExactWindow>>,
    pending_exact: Option<super::exact::ExactWindow>,
    budget: Budget,
    _charge: Reservation,
    finished: bool,
    received: usize,
    error: Option<Error>,
    revision: u64,
}
impl<T: Future<Output = Result<Arc<vtr_query::session::Delivery>>> + Unpin> WaveSlot<T> {
    fn new(limits: Limits, max_items: u32, budget: &Budget) -> Result<Self> {
        if max_items == 0 || max_items > 4096 {
            return Err(Error::Invalid(
                "waveform demand needs 1..4096 bins or changes",
            ));
        }
        let charge = budget.reserve(max_items as usize * std::mem::size_of::<Arc<WaveBin>>())?;
        let mut bins = Vec::new();
        bins.try_reserve_exact(max_items as usize)
            .map_err(|_| Error::ResourceLimit)?;
        Ok(Self {
            limits,
            max_items,
            desired: None,
            displayed: None,
            running: None,
            next: None,
            release: None,
            bins,
            exact: None,
            pending_exact: None,
            budget: budget.clone(),
            _charge: charge,
            finished: true,
            received: 0,
            error: None,
            revision: 0,
        })
    }
    fn set<S: AsyncSession<Task = T>>(
        &mut self,
        session: &S,
        demand: Option<Demand>,
    ) -> Result<()> {
        if matches!(demand, Some(Demand::Summary { grid, .. }) if grid.count() > self.max_items) {
            return Err(Error::Invalid("summary demand exceeds bin capacity"));
        }
        if demand == self.desired {
            return Ok(());
        }
        if let Some(exact) = self.pending_exact.take()
            && exact
                .coverage()
                .is_some_and(|coverage| !coverage.is_empty())
        {
            self.exact = Some(Arc::new(exact));
        }
        self.changed();
        self.desired = demand;
        self.finished = demand.is_none();
        self.error = None;
        if let Some(running) = &mut self.running
            && !running.cancelled
        {
            session.cancel(&running.task);
            running.cancelled = true;
        }
        if let Some((_, cursor)) = self.next.take() {
            self.release = Some(cursor);
        }
        if demand.is_none()
            || self
                .displayed
                .is_some_and(|old| demand.is_some_and(|new| old.signal() != new.signal()))
        {
            self.bins.clear();
            self.exact = None;
            self.pending_exact = None;
            self.displayed = None;
        }
        Ok(())
    }
    fn changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn bins(&self) -> &[Arc<WaveBin>] {
        &self.bins
    }
    pub fn exact(&self) -> Option<&super::exact::ExactWindow> {
        self.pending_exact
            .as_ref()
            .filter(|exact| {
                exact
                    .coverage()
                    .is_some_and(|coverage| !coverage.is_empty())
            })
            .or(self.exact.as_deref())
    }
    /// A raw held value at a cursor, only when the retained evidence proves it.
    /// Ambiguous aggregate interiors need a separate exact/sample request.
    pub fn sample_at(&self, time: u64) -> Option<vtr_query::wave::Sample> {
        super::snapshot::sample_at(&self.bins, self.exact(), time)
    }
    pub fn value_at(&self, time: u64, max_bytes: usize) -> Result<Option<crate::data::WaveValue>> {
        self.sample_at(time)
            .map(|sample| crate::data::WaveValue::from_query_sample(&sample, max_bytes))
            .transpose()
    }
    /// Render available coverage at the current viewport, including retained
    /// coverage while a replacement runs. This path never submits or polls work.
    pub fn paint(
        &self,
        viewport: Interval,
        row: crate::geometry::Rect,
        theme: &crate::Theme,
        scene: &mut crate::Scene,
    ) {
        if let Some(exact) = self.exact() {
            super::bounded::paint_exact(exact, viewport, row, theme, scene);
        } else {
            super::bounded::paint_summary(&self.bins, viewport, row, theme, scene);
        }
    }
    pub fn displayed(&self) -> Option<Demand> {
        self.displayed
    }
    pub fn error(&self) -> Option<&Error> {
        self.error.as_ref()
    }
    pub fn is_complete(&self) -> bool {
        self.finished && self.error.is_none() && self.desired == self.displayed
    }
    fn retry(&mut self) {
        if self.error.take().is_some() {
            self.changed();
            if let Some(exact) = self.pending_exact.take() {
                self.exact = Some(Arc::new(exact));
            }
            self.finished = false;
        }
    }
    fn holds_operation(&self) -> bool {
        self.running.is_some() || self.next.is_some() || self.release.is_some()
    }
    fn poll<S: AsyncSession<Task = T>>(
        &mut self,
        session: &S,
        cx: &mut Context<'_>,
        allow_start: bool,
    ) -> Poll<Progress> {
        if let Some(cursor) = self.release {
            match session.release(cursor) {
                Ok(()) => self.release = None,
                Err(Error::ResourceLimit) => return Poll::Ready(Progress::Backpressure),
                Err(error) => {
                    self.release = None;
                    self.changed();
                    self.error = Some(error);
                    self.finished = true;
                }
            }
        }
        if let Some(running) = &mut self.running {
            let result = match Pin::new(&mut running.task).poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(result) => result,
            };
            let running = self.running.take().unwrap();
            match result {
                Ok(delivery) => {
                    if running.cancelled || Some(running.demand) != self.desired {
                        self.release = Some(delivery.request);
                    } else if let (Demand::Summary { grid, .. }, Reply::Summary(page)) =
                        (running.demand, &delivery.reply)
                    {
                        let expected_offset = if running.cursor.is_some() {
                            self.received
                        } else {
                            0
                        };
                        if page.grid != grid || page.offset as usize != expected_offset {
                            session.close();
                            self.changed();
                            self.error =
                                Some(Error::Invalid("summary coverage does not match demand"));
                            self.finished = true;
                            return Poll::Ready(Progress::Advanced);
                        }
                        let received = expected_offset + page.bins().len();
                        if received > grid.count() as usize
                            || page.complete != delivery.next.is_none()
                            || (page.complete && received != grid.count() as usize)
                        {
                            self.changed();
                            self.error = Some(Error::Invalid("summary exceeded demand capacity"));
                            self.finished = true;
                            self.release = Some(delivery.request);
                        } else {
                            if !page.bins().is_empty() {
                                if expected_offset == 0 {
                                    self.bins.clear();
                                    self.exact = None;
                                    self.displayed = Some(running.demand);
                                }
                                self.bins.extend(page.bins().iter().cloned());
                            }
                            if !page.bins().is_empty() || delivery.next.is_none() {
                                self.changed();
                            }
                            self.received = received;
                            self.finished = delivery.next.is_none();
                            if let Some(next) = delivery.next {
                                self.next = Some((running.demand, next));
                            } else {
                                self.release = Some(delivery.request);
                            }
                        }
                    } else if let (Demand::Window { interval, .. }, Reply::Window(page)) =
                        (running.demand, &delivery.reply)
                    {
                        let accepted = if page.interval != interval {
                            Err(Error::Invalid("exact coverage does not match demand"))
                        } else {
                            self.pending_exact
                                .as_mut()
                                .expect("admitted exact request")
                                .append(delivery.clone())
                        };
                        match accepted {
                            Ok(_) => {
                                let exact = self.pending_exact.as_ref().unwrap();
                                if exact.is_complete()
                                    || exact
                                        .coverage()
                                        .is_some_and(|coverage| !coverage.is_empty())
                                {
                                    self.bins.clear();
                                    self.exact = None;
                                    self.displayed = Some(running.demand);
                                    self.changed();
                                }
                                self.finished = delivery.next.is_none();
                                if let Some(next) = delivery.next {
                                    self.next = Some((running.demand, next));
                                } else {
                                    self.exact = Some(Arc::new(self.pending_exact.take().unwrap()));
                                    self.release = Some(delivery.request);
                                }
                            }
                            Err(error) => {
                                self.changed();
                                self.error = Some(error);
                                self.finished = true;
                                self.release = Some(delivery.request);
                            }
                        }
                    } else {
                        self.changed();
                        self.error = Some(Error::Invalid("unexpected waveform reply"));
                        self.finished = true;
                        self.release = Some(delivery.request);
                    }
                }
                Err(error) if !running.cancelled && Some(running.demand) == self.desired => {
                    self.release = running.cursor;
                    self.changed();
                    self.error = Some(error);
                    self.finished = true;
                }
                Err(_) => {
                    self.release = running.cursor;
                }
            }
            return Poll::Ready(Progress::Advanced);
        }
        if self.finished {
            return Poll::Ready(Progress::Idle);
        }
        let Some(demand) = self.desired else {
            return Poll::Ready(Progress::Idle);
        };
        if self.next.is_none() && !allow_start {
            return Poll::Ready(Progress::Backpressure);
        }
        let cursor = self.next.map(|(_, cursor)| cursor);
        let task = if let Some((_, cursor)) = self.next {
            session.advance(cursor)
        } else {
            let query = match demand {
                Demand::Summary { signal, grid } => Query::Summary { signal, grid },
                Demand::Window { signal, interval } => {
                    if self.pending_exact.is_none() {
                        match super::exact::ExactWindow::new(self.max_items as usize, &self.budget)
                        {
                            Ok(exact) => self.pending_exact = Some(exact),
                            Err(Error::ResourceLimit) => {
                                return Poll::Ready(Progress::Backpressure);
                            }
                            Err(error) => {
                                self.changed();
                                self.error = Some(error);
                                self.finished = true;
                                return Poll::Ready(Progress::Advanced);
                            }
                        }
                    }
                    Query::Window { signal, interval }
                }
            };
            session.execute(query, self.limits)
        };
        match task {
            Ok(task) => {
                self.next = None;
                self.running = Some(Running {
                    demand,
                    task,
                    cancelled: false,
                    cursor,
                });
                // Register the host's waker immediately, even for an idle driver.
                self.poll(session, cx, allow_start)
            }
            Err(Error::ResourceLimit) => Poll::Ready(Progress::Backpressure),
            Err(error) => {
                self.release = self.next.take().map(|(_, cursor)| cursor);
                self.changed();
                self.error = Some(error);
                self.finished = true;
                Poll::Ready(Progress::Advanced)
            }
        }
    }
}

/// Fixed document-level slots over one session. At most `max_active` operations
/// (including continuations and pending releases) hold backend capacity. Polling
/// rotates between slots after each page/transition so a dense row cannot
/// monopolize delivery while other admitted rows are ready.
///
/// Hosts poll outside painting. Backpressure requires a transport/executor
/// progress notification or release of pinned data before retrying.
pub struct WaveDemands<S: AsyncSession> {
    session: S,
    slots: Vec<WaveSlot<S::Task>>,
    max_active: usize,
    rotate: usize,
    _charge: Reservation,
}
impl<S: AsyncSession> WaveDemands<S> {
    pub(crate) fn session(&self) -> &S {
        &self.session
    }
    pub fn new(
        session: S,
        limits: Limits,
        max_items: u32,
        max_slots: usize,
        max_active: usize,
        budget: &Budget,
    ) -> Result<Self> {
        let prepared = (|| -> Result<_> {
            if max_slots == 0 || max_slots > 4096 || max_active == 0 || max_active > 4 {
                return Err(Error::Invalid(
                    "waveform scheduler needs 1..4096 slots and 1..4 active operations",
                ));
            }
            let charge = budget.reserve(max_slots * std::mem::size_of::<WaveSlot<S::Task>>())?;
            let mut slots = Vec::new();
            slots
                .try_reserve_exact(max_slots)
                .map_err(|_| Error::ResourceLimit)?;
            for _ in 0..max_slots {
                slots.push(WaveSlot::new(limits, max_items, budget)?);
            }
            Ok((charge, slots))
        })();
        let (charge, slots) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                session.close();
                return Err(error);
            }
        };
        Ok(Self {
            session,
            slots,
            max_active,
            rotate: 0,
            _charge: charge,
        })
    }
    pub fn set(&mut self, slot: usize, demand: Option<Demand>) -> Result<()> {
        self.slots
            .get_mut(slot)
            .ok_or(Error::Invalid("unknown demand slot"))?
            .set(&self.session, demand)
    }
    pub fn retry(&mut self, slot: usize) -> Result<()> {
        self.slots
            .get_mut(slot)
            .ok_or(Error::Invalid("unknown demand slot"))?
            .retry();
        Ok(())
    }
    pub fn slot(&self, slot: usize) -> Option<&WaveSlot<S::Task>> {
        self.slots.get(slot)
    }
    /// Freeze current render data outside the paint path. Rows can share the
    /// returned Arc; later query pages cannot mutate an installed snapshot.
    pub fn snapshot(&self, slot: usize) -> Result<Option<Arc<super::snapshot::WaveSnapshot>>> {
        let slot = self
            .slots
            .get(slot)
            .ok_or(Error::Invalid("unknown demand slot"))?;
        let Some(demand) = slot.displayed else {
            return Ok(None);
        };
        let exact = if let Some(pending) = &slot.pending_exact
            && pending
                .coverage()
                .is_some_and(|coverage| !coverage.is_empty())
        {
            Some(Arc::new(pending.snapshot(&slot.budget)?))
        } else {
            slot.exact.clone()
        };
        super::snapshot::WaveSnapshot::new(
            self.session.info().snapshot,
            demand,
            &slot.bins,
            exact,
            &slot.budget,
        )
        .map(Some)
    }
    pub fn active_operations(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.holds_operation())
            .count()
    }
    pub fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Progress> {
        self.session.register_progress_waker(cx.waker());
        let mut pending = false;
        let mut pressure = false;
        let mut active = self.active_operations();
        for offset in 0..self.slots.len() {
            let index = (self.rotate + offset) % self.slots.len();
            // Identical row/panel demands share immutable bins and one query.
            // A follower waits only for an operation which already exists,
            // preventing two idle identical slots from waiting on each other.
            if !self.slots[index].finished && !self.slots[index].holds_operation() {
                let desired = self.slots[index].desired;
                let ready = self.slots.iter().enumerate().find_map(|(peer, slot)| {
                    (peer != index
                        && desired.is_some()
                        && slot.desired == desired
                        && slot.displayed == desired
                        && slot.error.is_none()
                        && (matches!(desired, Some(Demand::Summary { .. })) || slot.is_complete())
                        && (self.slots[index].displayed != desired
                            || slot.bins.len() > self.slots[index].bins.len()
                            || slot.is_complete()))
                    .then_some(peer)
                });
                if let Some(peer) = ready {
                    let (source, target) = if peer < index {
                        let (before, after) = self.slots.split_at_mut(index);
                        (&before[peer], &mut after[0])
                    } else {
                        let (before, after) = self.slots.split_at_mut(peer);
                        (&after[0], &mut before[index])
                    };
                    target.changed();
                    target.bins.clear();
                    target.bins.extend(source.bins.iter().cloned());
                    target.exact = source.exact.clone();
                    target.pending_exact = None;
                    target.displayed = source.displayed;
                    target.received = source.received;
                    target.finished = source.is_complete();
                    target.error = None;
                    self.rotate = (index + 1) % self.slots.len();
                    return Poll::Ready(Progress::Advanced);
                }
                if self.slots.iter().enumerate().any(|(peer, slot)| {
                    peer != index
                        && desired.is_some()
                        && slot.desired == desired
                        && !slot.finished
                        && slot.holds_operation()
                }) {
                    pressure = true;
                    continue;
                }
            }
            let allow_start = active < self.max_active;
            let was_active = self.slots[index].holds_operation();
            let progress = self.slots[index].poll(&self.session, cx, allow_start);
            active =
                active - usize::from(was_active) + usize::from(self.slots[index].holds_operation());
            match progress {
                Poll::Ready(Progress::Advanced) => {
                    self.rotate = (index + 1) % self.slots.len();
                    return Poll::Ready(Progress::Advanced);
                }
                Poll::Pending => pending = true,
                Poll::Ready(Progress::Backpressure) => pressure = true,
                Poll::Ready(Progress::Idle) => {}
            }
        }
        if pending {
            Poll::Pending
        } else if pressure {
            Poll::Ready(Progress::Backpressure)
        } else {
            Poll::Ready(Progress::Idle)
        }
    }
}
impl<S: AsyncSession> Drop for WaveDemands<S> {
    fn drop(&mut self) {
        self.session.close();
    }
}
