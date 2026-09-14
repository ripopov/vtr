//! Native executor adapter. Viewport policy and accepted rows belong to QueryView.
use crate::app::Workspace;
use futures::{Stream, StreamExt, channel::mpsc, future::poll_fn};
use gpui_kit::{Context, Task};
use std::{pin::Pin, task::Poll};
use volna_core::{query_view::QueryView, session::LoadResult, wave::demand::Progress};
use vtr_query::{Budget, local_session::LocalSession, wave::Limits};

pub(crate) struct Host {
    snapshot: vtr_query::session::SnapshotId,
    wake: mpsc::Sender<()>,
    _task: Task<()>,
    _repaint: Task<()>,
}

/// Readiness is paired with metadata delivery, so restored rows cannot start
/// full-history work before the bounded session is attached.
pub(crate) async fn prepare(mut result: LoadResult) -> (LoadResult, Option<(u64, LocalSession)>) {
    let LoadResult::Opened {
        generation,
        result: opened,
    } = &mut result
    else {
        return (result, None);
    };
    let queries = opened
        .as_ref()
        .ok()
        .and_then(|session| session.open_queries(Budget::new(512 << 20)));
    let queries = match queries {
        None => None,
        Some(opening) => match async { opening?.await }.await {
            Ok(session) => Some((*generation, session)),
            Err(error) => {
                *opened = Err(anyhow::anyhow!("Cannot open trace queries: {error}"));
                None
            }
        },
    };
    (result, queries)
}

impl Workspace {
    pub(crate) fn wake_queries(&mut self) {
        if self
            .queries
            .as_ref()
            .is_some_and(|host| Some(host.snapshot) != self.app.doc.query_snapshot())
        {
            self.queries = None; // dropping the task drops its controller and closes the worker
        }
        if let Some(host) = &mut self.queries {
            let _ = host.wake.try_send(()); // one coalesced notification, no input backlog
        }
    }
    pub(crate) fn attach_queries(&mut self, session: LocalSession, cx: &mut Context<Self>) {
        self.queries = None;
        let snapshot = session.info().snapshot;
        let mut view = match QueryView::attach(
            &mut self.app,
            session,
            Limits {
                bytes: 256 << 10,
                records: 128,
                work: 4096,
            },
            4096,
            256,
            &Budget::new(128 << 20),
        ) {
            Ok(view) => view,
            Err(error) => {
                // Do not fall back to unbounded history requests after admission fails.
                self.app.doc.close();
                self.app.deliver(LoadResult::Opened {
                    generation: self.app.doc.generation(),
                    result: Err(anyhow::anyhow!("Cannot attach trace queries: {error}")),
                });
                return;
            }
        };
        let (wake, mut receiver) = mpsc::channel(0);
        let (mut repaint, mut frames) = mpsc::channel(0);
        let frame_task = cx.spawn(async move |this, cx| {
            while frames.next().await.is_some() {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(16))
                    .await;
                if this.update(cx, |this, cx| this.after(None, cx)).is_err() {
                    break;
                }
            }
        });
        let task = cx.spawn(async move |this, cx| {
            poll_fn(|task_cx| {
                // Register input/layout wakeups even when the session itself is idle.
                loop {
                    match Pin::new(&mut receiver).poll_next(task_cx) {
                        Poll::Ready(Some(())) => continue,
                        Poll::Ready(None) => return Poll::Ready(()),
                        Poll::Pending => break,
                    }
                }
                let result = this.update(cx, |this, cx| {
                    let progress = view.poll(&mut this.app, task_cx);
                    match progress {
                        Poll::Ready(Ok(Progress::Advanced)) => {
                            let _ = repaint.try_send(());
                            task_cx.waker().wake_by_ref();
                        }
                        Poll::Ready(Err(error)) => {
                            this.app
                                .report_workspace_error(format!("Trace query failed: {error}"));
                            this.after(None, cx);
                            return Poll::Ready(());
                        }
                        _ => {}
                    }
                    if view.is_closed() {
                        Poll::Ready(())
                    } else {
                        Poll::Pending
                    }
                });
                result.unwrap_or(Poll::Ready(()))
            })
            .await;
        });
        self.queries = Some(Host {
            snapshot,
            wake,
            _task: task,
            _repaint: frame_task,
        });
    }
}
