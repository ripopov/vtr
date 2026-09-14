//! One bounded metadata operation sharing the viewer's query session. The
//! owning QueryView closes that session on drop, including retained cursors.
use crate::{App, data::query_hierarchy::QueryText, wave::demand::Progress};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use vtr_query::{
    Budget, Error, Result,
    metadata::Text,
    session::{AsyncSession, Continuation, Query},
    wave::Limits,
};

pub(crate) struct MetadataQueries<T> {
    task: Option<T>,
    text: Option<(u32, QueryText)>,
    budget: Budget,
    next: Option<Continuation>,
    release: Option<Continuation>,
    error: Option<Error>,
    stopped: bool,
    limits: Limits,
}
impl<T> MetadataQueries<T> {
    pub fn stop(&mut self) {
        self.task = None;
        self.text = None;
        self.next = None;
        self.release = None;
        self.error = None;
        self.stopped = true;
    }
    pub fn new(limits: Limits, budget: &Budget) -> Self {
        Self {
            task: None,
            text: None,
            budget: budget.clone(),
            next: None,
            release: None,
            error: None,
            stopped: false,
            limits,
        }
    }
}
impl<T: Future<Output = Result<std::sync::Arc<vtr_query::session::Delivery>>> + Unpin>
    MetadataQueries<T>
{
    fn text_task<S: AsyncSession<Task = T>>(&self, session: &S) -> Result<T> {
        let (id, text) = self.text.as_ref().unwrap();
        // Leave room for the reply envelope and shared byte allocation on both
        // native and wasm hosts. Text parts also yield between bounded chunks.
        let length = text
            .remaining()
            .min(4096)
            .min(self.limits.bytes.saturating_sub(256));
        if length == 0 {
            return Err(Error::Invalid(
                "metadata page budget cannot fit a text part",
            ));
        }
        session.execute(
            Query::Text {
                id: *id,
                offset: text.offset(),
                length,
            },
            self.limits,
        )
    }
    pub fn poll<S: AsyncSession<Task = T>>(
        &mut self,
        session: &S,
        app: &mut App,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Progress>> {
        session.register_progress_waker(cx.waker());
        if let Some(cursor) = self.release {
            match session.release(cursor) {
                Ok(()) => self.release = None,
                Err(Error::ResourceLimit) => return Poll::Ready(Ok(Progress::Backpressure)),
                Err(error) => {
                    self.release = None;
                    self.text = None;
                    self.stopped = true;
                    return Poll::Ready(Err(error));
                }
            }
        }
        if let Some(error) = self.error.take() {
            return Poll::Ready(Err(error));
        }
        if self.stopped {
            return Poll::Ready(Ok(Progress::Idle));
        }
        if let Some(task) = &mut self.task {
            let delivery = match std::task::ready!(Pin::new(task).poll(cx)) {
                Ok(delivery) => delivery,
                Err(error) => {
                    self.task = None;
                    self.text = None;
                    self.stopped = true;
                    // A failed continuation may still own an operation.
                    self.release = self.next.take();
                    self.error = Some(error);
                    return Poll::Ready(Ok(Progress::Advanced));
                }
            };
            self.task = None;
            let accepted = if let Some((_, text)) = &mut self.text {
                match text.append(&delivery) {
                    Ok(_) if text.remaining() == 0 => {
                        let (_, text) = self.text.take().unwrap();
                        app.doc
                            .install_query_text(app.doc.generation(), text)
                            .map(|_| true)
                    }
                    result => result,
                }
            } else {
                app.doc
                    .append_query_children(app.doc.generation(), delivery.clone())
            };
            match accepted {
                Ok(_) => {
                    self.next = delivery.next;
                    app.refresh_query_metadata();
                }
                Err(error) => {
                    self.next = None;
                    self.text = None;
                    self.stopped = true;
                    self.error = Some(error);
                }
            }
            if self.next.is_none() {
                self.release = Some(delivery.request);
            }
            return Poll::Ready(Ok(Progress::Advanced));
        }
        let task = if let Some(next) = self.next {
            session.advance(next)
        } else if self.text.is_some() {
            self.text_task(session)
        } else {
            let Some(hierarchy) = app.doc.query_hierarchy() else {
                return Poll::Ready(Ok(Progress::Idle));
            };
            let parent = if !hierarchy.state(None).is_some_and(|s| s.complete) {
                Some(None)
            } else {
                app.scopes
                    .selected
                    .into_iter()
                    .chain(app.scopes.expanded())
                    .filter_map(|id| u32::try_from(id).ok())
                    .find(|id| !hierarchy.state(Some(*id)).is_some_and(|s| s.complete))
                    .map(Some)
            };
            let parent = parent.or_else(|| {
                let browser = crate::data::browser::BrowserHierarchy::Paged(hierarchy);
                app.scopes
                    .unresolved_selected
                    .iter()
                    .chain(app.scopes.unresolved_expanded.iter())
                    .find_map(|path| browser.pending_path_children(path, false))
                    .or_else(|| {
                        app.panels
                            .iter()
                            .filter_map(|panel| panel.kind.waves())
                            .flat_map(|waves| &waves.items)
                            .find_map(|row| {
                                if let crate::wave::model::RowSource::Unresolved { path, .. } =
                                    &row.source
                                {
                                    browser.pending_path_children(path, true)
                                } else {
                                    None
                                }
                            })
                    })
            });
            let Some(parent) = parent else {
                let missing = hierarchy.declarations().find_map(|node| {
                    if hierarchy.text(&node.name).is_some() {
                        return None;
                    }
                    match node.name {
                        Text::Reference { id, bytes } => Some((id, bytes)),
                        _ => None,
                    }
                });
                let Some((id, bytes)) = missing else {
                    return Poll::Ready(Ok(Progress::Idle));
                };
                match QueryText::new(
                    session.info().snapshot,
                    id,
                    bytes,
                    self.budget.limit().min(1 << 20),
                    &self.budget,
                ) {
                    Ok(text) => {
                        if text.remaining() == 0 {
                            let result = app.doc.install_query_text(app.doc.generation(), text);
                            app.refresh_query_metadata();
                            return Poll::Ready(result.map(|_| Progress::Advanced));
                        }
                        self.text = Some((id, text));
                        return Poll::Ready(Ok(Progress::Advanced));
                    }
                    Err(error) => {
                        self.stopped = true;
                        return Poll::Ready(Err(error));
                    }
                }
            };
            // A cache may have been installed before attachment. Continue its
            // exact operation instead of restarting at an incompatible offset.
            if let Some(next) = hierarchy.state(parent).and_then(|s| s.next) {
                self.next = Some(next);
                session.advance(next)
            } else {
                session.execute(Query::Children { parent }, self.limits)
            }
        };
        match task {
            Ok(task) => {
                self.task = Some(task);
                Poll::Ready(Ok(Progress::Advanced))
            }
            Err(Error::ResourceLimit) => Poll::Ready(Ok(Progress::Backpressure)),
            Err(error) => {
                self.text = None;
                self.stopped = true;
                self.release = self.next.take();
                self.error = Some(error);
                Poll::Ready(Ok(Progress::Advanced))
            }
        }
    }
}
