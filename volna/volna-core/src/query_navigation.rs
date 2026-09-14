//! One bounded navigation operation; the model owns the replaceable intent.
use crate::{
    App,
    panels::PanelId,
    wave::{demand::Progress, model::EdgeIntent},
};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use vtr_query::{
    Error, Result,
    session::{AsyncSession, Continuation, Query, Reply},
    wave::{ChangeSearchResult, Limits},
};

pub(crate) struct Navigation<T> {
    active: Option<(PanelId, EdgeIntent)>,
    task: Option<T>,
    next: Option<Continuation>,
    release: Option<Continuation>,
    limits: Limits,
}
impl<T> Navigation<T> {
    pub fn new(limits: Limits) -> Self {
        Self {
            active: None,
            task: None,
            next: None,
            release: None,
            limits,
        }
    }
    pub fn stop(&mut self) {
        self.active = None;
        self.task = None;
        self.next = None;
        self.release = None;
    }
    fn finish(&mut self, app: &mut App, result: Result<Option<u64>>) {
        if let Some((panel, intent)) = self.active.take() {
            app.finish_query_edge(panel, intent, result, web_time::Instant::now());
        }
    }
}
impl<T: Future<Output = Result<std::sync::Arc<vtr_query::session::Delivery>>> + Unpin>
    Navigation<T>
{
    pub fn poll<S: AsyncSession<Task = T>>(
        &mut self,
        session: &S,
        app: &mut App,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Progress>> {
        session.register_progress_waker(cx.waker());
        if self.active.is_some_and(|(panel, intent)| {
            app.panels
                .waves_mut(panel)
                .and_then(|w| w.pending_edge(&app.doc))
                != Some(intent)
        }) {
            self.task = None; // abandoned futures cancel and release their operation
            self.release = self.next.take();
            self.active = None;
        }
        if let Some(cursor) = self.release {
            match session.release(cursor) {
                Ok(()) => self.release = None,
                Err(Error::ResourceLimit) => return Poll::Ready(Ok(Progress::Backpressure)),
                Err(error) => return Poll::Ready(Err(error)),
            }
        }
        if let Some(task) = &mut self.task {
            let delivery = std::task::ready!(Pin::new(task).poll(cx));
            self.task = None;
            match delivery {
                Ok(delivery) => {
                    self.next = delivery.next;
                    match &delivery.reply {
                        Reply::FindChange(page) => match page.result {
                            ChangeSearchResult::Pending if delivery.next.is_some() => {}
                            ChangeSearchResult::Found(time) if delivery.next.is_none() => {
                                self.finish(app, Ok(Some(time)))
                            }
                            ChangeSearchResult::Exhausted if delivery.next.is_none() => {
                                self.finish(app, Ok(None))
                            }
                            _ => self
                                .finish(app, Err(Error::Invalid("inconsistent edge continuation"))),
                        },
                        _ => self.finish(app, Err(Error::Invalid("wrong edge reply family"))),
                    }
                    if self.active.is_none() {
                        self.release = Some(delivery.request);
                        self.next = None;
                    }
                }
                Err(error) => {
                    self.release = self.next.take();
                    self.finish(app, Err(error));
                }
            }
            return Poll::Ready(Ok(Progress::Advanced));
        }
        if self.active.is_none() {
            self.active = app.panels.layout().visible().into_iter().find_map(|panel| {
                app.panels
                    .waves_mut(panel)?
                    .pending_edge(&app.doc)
                    .map(|intent| (panel, intent))
            });
        }
        let Some((_, intent)) = self.active else {
            return Poll::Ready(Ok(Progress::Idle));
        };
        let task = if let Some(next) = self.next {
            session.advance(next)
        } else {
            session.execute(
                Query::FindChange {
                    signal: intent.signal,
                    from: intent.from,
                    direction: intent.direction,
                },
                self.limits,
            )
        };
        match task {
            Ok(task) => self.task = Some(task),
            Err(Error::ResourceLimit) => return Poll::Ready(Ok(Progress::Backpressure)),
            Err(error) => {
                self.release = self.next.take();
                self.finish(app, Err(error));
            }
        }
        Poll::Ready(Ok(Progress::Advanced))
    }
}
