//! Bounded exact cursor samples shared by visible aliases and panels.
use crate::{
    App, Event,
    wave::{demand::Progress, snapshot::CursorSample},
};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use vtr_query::{
    Budget, Error, Reservation, Result,
    session::{AsyncSession, Continuation, Query, Reply},
    wave::{Limits, SignalTime},
};

pub(crate) struct CursorQueries<T> {
    pairs: Vec<SignalTime>,
    wanted: Vec<SignalTime>,
    samples: Vec<Option<CursorSample>>,
    task: Option<T>,
    next: Option<Continuation>,
    release: Option<Continuation>,
    generation: u64,
    complete: bool,
    limit: usize,
    limits: Limits,
    budget: Budget,
    _charge: Reservation,
}
impl<T> CursorQueries<T> {
    pub fn new(limit: usize, limits: Limits, budget: &Budget) -> Result<Self> {
        let charge = budget.reserve(
            limit
                .checked_mul(
                    2 * std::mem::size_of::<SignalTime>()
                        + std::mem::size_of::<Option<CursorSample>>(),
                )
                .ok_or(Error::ResourceLimit)?,
        )?;
        let mut pairs = Vec::new();
        let mut wanted = Vec::new();
        let mut samples = Vec::new();
        pairs
            .try_reserve_exact(limit)
            .map_err(|_| Error::ResourceLimit)?;
        wanted
            .try_reserve_exact(limit)
            .map_err(|_| Error::ResourceLimit)?;
        samples
            .try_reserve_exact(limit)
            .map_err(|_| Error::ResourceLimit)?;
        if pairs.capacity() != limit || wanted.capacity() != limit || samples.capacity() != limit {
            return Err(Error::ResourceLimit);
        }
        Ok(Self {
            pairs,
            wanted,
            samples,
            task: None,
            next: None,
            release: None,
            generation: 0,
            complete: true,
            limit,
            limits,
            budget: budget.clone(),
            _charge: charge,
        })
    }
    pub fn stop(&mut self) {
        self.task = None;
        self.next = None;
        self.release = None;
        self.pairs.clear();
        self.wanted.clear();
        self.samples.clear();
        self.complete = true;
    }
    fn synchronize(&mut self, app: &mut App) -> Result<bool> {
        self.wanted.clear();
        let visible = app.panels.layout().visible();
        for id in &visible {
            let Some(waves) = app.panels.waves(*id) else {
                continue;
            };
            let Some(time) = waves.cursor(&app.doc) else {
                continue;
            };
            for row in waves.last_layout().rows.clone() {
                let Some(signal) = waves.items.get(row).and_then(|item| item.source.signal())
                else {
                    continue;
                };
                let pair = SignalTime {
                    signal: signal.0,
                    time,
                };
                if !self.wanted.contains(&pair) {
                    if self.wanted.len() == self.limit {
                        return Err(Error::ResourceLimit);
                    }
                    self.wanted.push(pair);
                }
            }
        }
        self.wanted
            .sort_unstable_by_key(|pair| (pair.signal, pair.time));
        let changed = self.generation != app.doc.generation() || self.wanted != self.pairs;
        if changed {
            self.task = None;
            // Retire a completed or partially advanced operation before starting
            // another; do not overwrite an earlier pending release.
            if self.next.is_some() {
                self.release = self.next.take();
            }
            self.generation = app.doc.generation();
            std::mem::swap(&mut self.pairs, &mut self.wanted);
            self.samples.clear();
            self.samples.resize_with(self.pairs.len(), || None);
            self.complete = self.pairs.is_empty();
        }
        for panel in app.panels.iter_mut() {
            let Some(waves) = panel.kind.waves_mut() else {
                continue;
            };
            let cursor = waves.cursor(&app.doc);
            let rows = waves.last_layout().rows.clone();
            for (index, item) in waves.items.iter_mut().enumerate() {
                item.cursor_sample = if visible.contains(&panel.id) && rows.contains(&index) {
                    cursor
                        .zip(item.source.signal())
                        .and_then(|(time, signal)| {
                            self.pairs
                                .binary_search_by_key(&(signal.0, time), |pair| {
                                    (pair.signal, pair.time)
                                })
                                .ok()
                        })
                        .and_then(|slot| self.samples[slot].clone())
                } else {
                    None
                };
            }
        }
        Ok(changed)
    }
}
impl<T: Future<Output = Result<std::sync::Arc<vtr_query::session::Delivery>>> + Unpin>
    CursorQueries<T>
{
    pub fn poll<S: AsyncSession<Task = T>>(
        &mut self,
        session: &S,
        app: &mut App,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Progress>> {
        session.register_progress_waker(cx.waker());
        let changed = self.synchronize(app)?;
        if let Some(cursor) = self.release {
            match session.release(cursor) {
                Ok(()) => self.release = None,
                Err(Error::ResourceLimit) => return Poll::Ready(Ok(Progress::Backpressure)),
                Err(error) => return Poll::Ready(Err(error)),
            }
        }
        if let Some(task) = &mut self.task {
            let result = std::task::ready!(Pin::new(task).poll(cx));
            self.task = None;
            match result {
                Ok(delivery) => {
                    self.next = delivery.next;
                    let Reply::Values(page) = &delivery.reply else {
                        return Poll::Ready(Err(Error::Invalid("wrong cursor reply family")));
                    };
                    let end = page
                        .offset
                        .checked_add(page.samples().len())
                        .ok_or(Error::ResourceLimit)?;
                    if delivery.request.snapshot != session.info().snapshot
                        || page.offset
                            != self
                                .samples
                                .iter()
                                .position(Option::is_none)
                                .unwrap_or(self.pairs.len())
                        || page.complete != (end == self.pairs.len())
                        || end > self.pairs.len()
                        || page.complete != delivery.next.is_none()
                        || page
                            .samples()
                            .iter()
                            .zip(&self.pairs[page.offset..end])
                            .any(|(sample, pair)| sample.pair != *pair)
                    {
                        return Poll::Ready(Err(Error::Invalid(
                            "cursor samples do not match demand",
                        )));
                    }
                    for (index, value) in page.samples().iter().enumerate() {
                        self.samples[page.offset + index] = Some(CursorSample {
                            generation: self.generation,
                            snapshot: session.info().snapshot,
                            pair: value.pair,
                            sample: value.sample.clone(),
                        });
                    }
                    self.complete = page.complete;
                    if self.complete {
                        self.release = Some(delivery.request);
                    }
                    self.synchronize(app)?;
                    app.changed();
                }
                Err(error) => {
                    self.release = self.next.take();
                    self.complete = true;
                    app.events.push(Event::Notice(format!(
                        "Cannot sample cursor values: {error}"
                    )));
                    app.changed();
                }
            }
            return Poll::Ready(Ok(Progress::Advanced));
        }
        if self.complete {
            return Poll::Ready(Ok(if changed {
                Progress::Advanced
            } else {
                Progress::Idle
            }));
        }
        let result = if let Some(next) = self.next {
            session.advance(next)
        } else {
            (|| {
                let _input_charge = self
                    .budget
                    .reserve(std::mem::size_of_val(self.pairs.as_slice()))?;
                let mut pairs = Vec::new();
                pairs
                    .try_reserve_exact(self.pairs.len())
                    .map_err(|_| Error::ResourceLimit)?;
                if pairs.capacity() != self.pairs.len() {
                    return Err(Error::ResourceLimit);
                }
                pairs.extend_from_slice(&self.pairs);
                session.execute(Query::ValuesAt { pairs }, self.limits)
            })()
        };
        match result {
            Ok(task) => self.task = Some(task),
            Err(Error::ResourceLimit) => return Poll::Ready(Ok(Progress::Backpressure)),
            Err(error) => return Poll::Ready(Err(error)),
        }
        Poll::Ready(Ok(Progress::Advanced))
    }
}
