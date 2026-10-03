//! Viewer demand and analysis coordination over the standalone trace client.
use super::ClientStep;
use crate::session::{LoadRequest, LoadResult};
use crate::trace::TraceId;
use std::sync::Arc;
use volna_trace::remote::memory::MemoryBudget;
use volna_trace::remote::transport::Packet;
use volna_trace::remote::{ClientStep as RawStep, client::RemoteClient as TraceClient};
use volna_trace::session::{LoadRequest as RawRequest, LoadResult as RawResult};

struct Preparation {
    ack: Packet,
    credits: Arc<std::sync::atomic::AtomicUsize>,
    work: std::pin::Pin<Box<dyn std::future::Future<Output = LoadResult> + Send>>,
    trace: TraceId,
    generation: u64,
    request_id: u64,
    track: volna_trace::data::transactions::TrackRef,
}
struct Resolution {
    trace: TraceId,
    generation: u64,
    window: (u64, u64),
    counter: Arc<crate::data::ActivityCounter>,
    counts: Arc<crate::data::ActivityCounts>,
    budget: MemoryBudget,
}
/// Adapts viewer requests and demand to raw loading; transport and decoding belong to `volna-trace`.
pub struct RemoteClient {
    inner: TraceClient<TraceId>,
    resolutions: Vec<Resolution>,
    preparing: Option<Preparation>,
}
impl RemoteClient {
    pub fn new(
        trace: TraceId,
        generation: u64,
        limit: u64,
        budget: MemoryBudget,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            inner: TraceClient::new(trace, generation, limit, budget)?,
            resolutions: Vec::new(),
            preparing: None,
        })
    }
    pub fn session(&self) -> Option<u64> {
        self.inner.session()
    }
    pub fn submit(&mut self, job: LoadRequest) -> Result<(), LoadResult> {
        let raw = match job {
            LoadRequest::Signals {
                trace,
                generation,
                session,
                signals,
            } => RawRequest::Signals {
                tag: trace,
                generation,
                session,
                signals,
            },
            LoadRequest::Track {
                trace,
                generation,
                request_id,
                session,
                track,
            } => RawRequest::Track {
                tag: trace,
                generation,
                request_id,
                session,
                track,
            },
            LoadRequest::BuildActivity {
                trace,
                generation,
                session,
                options,
                cache_dir,
                budget,
            } => RawRequest::BuildActivity {
                tag: trace,
                generation,
                session,
                options,
                cache_dir,
                budget,
            },
            LoadRequest::ResolveActivity {
                trace,
                generation,
                session,
                counter,
                counts,
                budget,
            } => {
                let signals = counts.undecided.clone();
                let window = counts.window;
                self.resolutions.push(Resolution {
                    trace,
                    generation,
                    window,
                    counter,
                    counts,
                    budget: budget.clone(),
                });
                RawRequest::ResolveActivity {
                    tag: trace,
                    generation,
                    session,
                    signals,
                    window,
                    budget,
                }
            }
            job => {
                return Err(job.fail(anyhow::anyhow!(
                    "viewer analysis is not remote loading work"
                )));
            }
        };
        match self.inner.submit(raw) {
            Ok(()) => Ok(()),
            Err(result) => Err(self.result(result)),
        }
    }
    pub fn take_command(&mut self) -> anyhow::Result<Option<Packet>> {
        if self.preparing.is_some() {
            Ok(None)
        } else {
            self.inner.take_command()
        }
    }
    pub fn sync_demand(&mut self, app: &mut crate::app::App) {
        let wanted = app.signal_demand();
        self.inner.retain_queued(|job| match job {
            RawRequest::Signals {
                tag,
                generation,
                signals,
                ..
            } => app
                .doc
                .retain_queued_signals(*tag, *generation, signals, &wanted),
            RawRequest::Track {
                tag,
                generation,
                request_id,
                track,
                ..
            } => app
                .doc
                .wants_track_request(*tag, *generation, *request_id, *track),
            RawRequest::BuildActivity { .. } | RawRequest::ResolveActivity { .. } => true,
        });
    }
    pub fn accept(&mut self, packet: Packet) -> anyhow::Result<ClientStep> {
        anyhow::ensure!(
            self.preparing.is_none(),
            "response before viewer preparation finished"
        );
        let step = self.inner.accept(packet)?;
        Ok(self.convert(step))
    }
    pub fn step(&mut self) -> anyhow::Result<ClientStep> {
        if self.preparing.is_some() {
            Ok(self.prepare_step())
        } else {
            let step = self.inner.step()?;
            Ok(self.convert(step))
        }
    }
    fn convert(&mut self, step: RawStep<TraceId>) -> ClientStep {
        if let RawStep::Complete {
            ack,
            result:
                RawResult::Track {
                    tag,
                    generation,
                    request_id,
                    track,
                    result: Ok(raw),
                },
        } = step
        {
            let budget = raw.generators.first().and_then(|g| g.memory_budget());
            let credits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let checkpoint = credits.clone();
            let work = Box::pin(async move {
                let result = crate::data::loaded_tracks::LoadedTrack::prepare_with(
                    raw,
                    budget.as_ref(),
                    move || {
                        let credits = checkpoint.clone();
                        std::future::poll_fn(move |_| {
                            if credits.load(std::sync::atomic::Ordering::Relaxed) > 0 {
                                credits.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                                std::task::Poll::Ready(())
                            } else {
                                std::task::Poll::Pending
                            }
                        })
                    },
                )
                .await;
                LoadResult::Track {
                    trace: tag,
                    generation,
                    request_id,
                    track,
                    result,
                }
            });
            self.preparing = Some(Preparation {
                ack,
                credits,
                work,
                trace: tag,
                generation,
                request_id,
                track,
            });
            self.prepare_step()
        } else {
            match step {
                RawStep::Ack(ack) => ClientStep::Ack(ack),
                RawStep::Yield => ClientStep::Yield,
                RawStep::Complete { ack, result } => ClientStep::Complete {
                    ack,
                    result: self.result(result),
                },
            }
        }
    }
    fn prepare_step(&mut self) -> ClientStep {
        let p = self.preparing.as_mut().expect("pending preparation");
        p.credits.store(1024, std::sync::atomic::Ordering::Relaxed);
        match p
            .work
            .as_mut()
            .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
        {
            std::task::Poll::Pending => ClientStep::Yield,
            std::task::Poll::Ready(result) => {
                let p = self.preparing.take().unwrap();
                ClientStep::Complete { ack: p.ack, result }
            }
        }
    }
    fn result(&mut self, raw: RawResult<TraceId>) -> LoadResult {
        match raw {
            RawResult::Opened {
                tag,
                generation,
                result,
            } => LoadResult::Opened {
                trace: tag,
                generation,
                result,
            },
            RawResult::Signals {
                tag,
                generation,
                results,
            } => LoadResult::Signals {
                trace: tag,
                generation,
                results,
            },
            RawResult::Track {
                tag,
                generation,
                request_id,
                track,
                result,
            } => LoadResult::Track {
                trace: tag,
                generation,
                request_id,
                track,
                result: result
                    .and_then(|raw| crate::data::loaded_tracks::LoadedTrack::prepare(raw, None)),
            },
            RawResult::ActivityBuilt {
                tag,
                generation,
                result,
            } => LoadResult::ActivityBuilt {
                trace: tag,
                generation,
                result,
            },
            RawResult::ActivityResolved {
                tag,
                generation,
                window,
                result,
            } => {
                let pos = self
                    .resolutions
                    .iter()
                    .position(|r| {
                        r.trace == tag && r.generation == generation && r.window == window
                    })
                    .expect("submitted activity resolution");
                let work = self.resolutions.remove(pos);
                let result = result
                    .and_then(|changed| {
                        work.counts
                            .resolved(&work.counter, &changed)
                            .account(&work.budget)
                    })
                    .map(Arc::new);
                LoadResult::ActivityResolved {
                    trace: tag,
                    generation,
                    window,
                    result,
                }
            }
        }
    }
    pub fn disconnect(&mut self, message: &str) -> Vec<LoadResult> {
        let mut results: Vec<_> = self
            .inner
            .disconnect(message)
            .into_iter()
            .map(|r| self.result(r))
            .collect();
        if let Some(p) = self.preparing.take() {
            results.push(LoadResult::Track {
                trace: p.trace,
                generation: p.generation,
                request_id: p.request_id,
                track: p.track,
                result: Err(anyhow::anyhow!(message.to_owned())),
            });
        }
        results
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::Traced;
    use volna_trace::data::SignalRef;
    use volna_trace::data::transactions::TrackRef;
    use volna_trace::remote::history::PackedHistory;
    use volna_trace::remote::transport::{Body, Command, DATA_BYTES, ObjectId, acknowledgement};
    use volna_trace::session::OpenSpec;

    fn response(client: &mut RemoteClient, packet: Packet) -> Option<LoadResult> {
        let expected = acknowledgement(&packet);
        let mut step = client.accept(packet).unwrap();
        loop {
            match step {
                ClientStep::Yield => step = client.step().unwrap(),
                ClientStep::Ack(ack) => {
                    assert_eq!(ack, expected);
                    return None;
                }
                ClientStep::Complete { ack, result } => {
                    assert_eq!(ack, expected);
                    return Some(result);
                }
            }
        }
    }

    fn send_object(
        client: &mut RemoteClient,
        request: u64,
        sequence: &mut u64,
        object: ObjectId,
        bytes: Vec<u8>,
    ) -> Option<LoadResult> {
        let mut send = |body| {
            let result = response(
                client,
                Packet {
                    session: 31,
                    request,
                    sequence: *sequence,
                    body,
                },
            );
            *sequence += 1;
            result
        };
        assert!(
            send(Body::Begin {
                object,
                decoded_bytes: bytes.len() as u64
            })
            .is_none()
        );
        for (i, chunk) in bytes.chunks(DATA_BYTES).enumerate() {
            assert!(
                send(Body::Data {
                    offset: (i * DATA_BYTES) as u64,
                    bytes: chunk.to_vec()
                })
                .is_none()
            );
        }
        send(Body::End)
    }

    fn object(
        client: &mut RemoteClient,
        request: u64,
        sequence: &mut u64,
        object: ObjectId,
        bytes: Vec<u8>,
    ) -> LoadResult {
        send_object(client, request, sequence, object, bytes).expect("completed object")
    }
    fn opened(
        client: &mut RemoteClient,
        request: u64,
        source: &dyn volna_trace::session::Session,
    ) -> LoadResult {
        let mut sequence = 0;
        let header = volna_trace::remote::hierarchy::Header::from_session(source).unwrap();
        let sizes = volna_trace::data::ScopeSizes::count(source.hierarchy());
        let mut completed = send_object(
            client,
            request,
            &mut sequence,
            ObjectId::Metadata,
            bincode::serialize(&header).unwrap(),
        );
        for page in 0..header.scope_pages() {
            completed = send_object(
                client,
                request,
                &mut sequence,
                ObjectId::Scopes(page),
                bincode::serialize(&volna_trace::remote::hierarchy::Page::scopes(
                    source.hierarchy(),
                    &sizes,
                    page,
                ))
                .unwrap(),
            );
        }
        for page in 0..header.var_pages() {
            completed = send_object(
                client,
                request,
                &mut sequence,
                ObjectId::Variables(page),
                bincode::serialize(&volna_trace::remote::hierarchy::Page::vars(
                    source.hierarchy(),
                    page,
                ))
                .unwrap(),
            );
        }
        completed.expect("completed Open")
    }

    #[test]
    fn removed_queued_signals_do_not_send_and_active_results_can_fill_readded_rows() {
        use crate::app::{Action, App, Command as ViewerCommand};
        let mut client = RemoteClient::new(
            TraceId::A,
            1,
            1024 * 1024,
            MemoryBudget::new(4 * 1024 * 1024),
        )
        .unwrap();
        let open = client.take_command().unwrap().unwrap();
        let local = crate::testing::ProceduralTrace::session(100);
        let opened = opened(&mut client, open.request, local.as_ref());
        let LoadResult::Opened { result, .. } = opened else {
            panic!("opened")
        };
        let mut app = App::new();
        app.set_session(result.unwrap());
        // Routed as frontends do: client-side work (scope sizes) stays local.
        let enqueue = |app: &mut App, client: &mut RemoteClient| {
            for request in app.take_requests() {
                if request.remote_id().is_some() {
                    assert!(client.submit(request).is_ok());
                } else {
                    app.deliver(request.perform());
                }
            }
        };
        app.handle(ViewerCommand::AddVars(vec![Traced::new(TraceId::A, 0)]));
        enqueue(&mut app, &mut client);
        let active = client.take_command().unwrap().unwrap();
        app.handle(ViewerCommand::AddVars(vec![Traced::new(TraceId::A, 1)]));
        enqueue(&mut app, &mut client);
        app.handle(ViewerCommand::Action(Action::SelectAll));
        app.handle(ViewerCommand::Action(Action::RemoveSelected));
        client.sync_demand(&mut app);
        assert!(
            app.doc.is_pending(Traced::new(TraceId::A, SignalRef(0))),
            "active work still completes"
        );
        assert!(
            !app.doc.is_pending(Traced::new(TraceId::A, SignalRef(1))),
            "removed queued demand is cleared"
        );
        app.handle(ViewerCommand::AddVars(vec![
            Traced::new(TraceId::A, 0),
            Traced::new(TraceId::A, 1),
        ]));
        enqueue(&mut app, &mut client);
        let completed = object(
            &mut client,
            active.request,
            &mut 0,
            ObjectId::Signal(0),
            bincode::serialize(
                &PackedHistory::from_history_with_limit(
                    local.load_signal(SignalRef(0)).unwrap().as_ref(),
                    u64::MAX,
                )
                .unwrap(),
            )
            .unwrap(),
        );
        app.deliver(completed);
        assert!(
            app.panels.focused_waves().unwrap().items()[0]
                .signal()
                .unwrap()
                .history
                .is_some()
        );
        client.sync_demand(&mut app);
        let next = client.take_command().unwrap().unwrap();
        assert_eq!(next.body, Body::Command(Command::Signals(vec![1])));
    }

    #[test]
    fn removing_and_readding_a_queued_track_keeps_only_the_new_request() {
        use crate::app::App;
        use crate::session::LoadRequest;
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut writer = vtr::Writer::create(file.path()).unwrap();
        let stream = writer.add_stream(None, "stream", "raw").unwrap();
        writer.add_generator(stream, "generator").unwrap();
        writer.close().unwrap();
        let local = OpenSpec::Path(file.path().into()).open().unwrap();
        let track = Traced::new(TraceId::A, TrackRef(stream.0));
        let mut client = RemoteClient::new(
            TraceId::A,
            1,
            1024 * 1024,
            MemoryBudget::new(4 * 1024 * 1024),
        )
        .unwrap();
        let open = client.take_command().unwrap().unwrap();
        let opened = opened(&mut client, open.request, local.as_ref());
        let LoadResult::Opened { result, .. } = opened else {
            panic!("opened")
        };
        let mut app = App::new();
        app.set_session(result.unwrap());
        let enqueue = |app: &mut App, client: &mut RemoteClient| {
            let request = app.take_requests().pop().unwrap();
            let LoadRequest::Track { request_id, .. } = &request else {
                panic!("track")
            };
            let request_id = *request_id;
            assert!(client.submit(request).is_ok());
            request_id
        };
        app.doc.retain_track(track).unwrap();
        let old = enqueue(&mut app, &mut client);
        app.doc.release_track(track);
        app.doc.retain_track(track).unwrap();
        let current = enqueue(&mut app, &mut client);
        assert_ne!(old, current);
        client.sync_demand(&mut app);
        let mut queued = 0;
        client.inner.retain_queued(|job| {
            assert!(matches!(job, RawRequest::Track {request_id,..} if *request_id == current));
            queued += 1;
            true
        });
        assert_eq!(queued, 1);
        app.doc.release_track(track);
        client.sync_demand(&mut app);
        assert!(client.take_command().unwrap().is_none());
    }

    #[test]
    fn track_preparation_holds_ack_yields_and_releases_on_disconnect() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut writer = vtr::Writer::create(file.path()).unwrap();
        let stream = writer.add_stream(None, "stream", "raw").unwrap();
        let generator = writer.add_generator(stream, "generator").unwrap();
        for time in 0..5000 {
            let tx = writer.begin_tx(generator, time * 10).unwrap();
            writer.end_tx(tx, time * 10 + 5, vtr::TxStatus::Ok).unwrap();
        }
        writer.close().unwrap();
        let budget = MemoryBudget::new(16 << 20);
        let session = volna_trace::session::account_local_session(
            OpenSpec::Path(file.path().into()).open().unwrap(),
            budget.clone(),
        )
        .unwrap();
        let baseline = budget.used();
        let track = TrackRef(stream.0);
        let ack = acknowledgement(&Packet {
            session: 31,
            request: 17,
            sequence: 19,
            body: Body::End,
        });
        for cancel in [false, true] {
            let mut client = RemoteClient::new(TraceId::A, 1, 8 << 20, budget.clone()).unwrap();
            let raw = session.load_track(track).unwrap();
            let raw_owner = Arc::downgrade(&raw.generators[0]);
            let mut step = client.convert(RawStep::Complete {
                ack: ack.clone(),
                result: RawResult::Track {
                    tag: TraceId::A,
                    generation: 3,
                    request_id: 7,
                    track,
                    result: Ok(raw),
                },
            });
            assert!(matches!(step, ClientStep::Yield));
            assert!(client.take_command().unwrap().is_none());
            // Cross the allowance scan and enter admitted index construction.
            for _ in 0..5 {
                step = client.step().unwrap();
                assert!(matches!(step, ClientStep::Yield));
            }
            assert!(budget.used() > baseline);
            if cancel {
                assert!(client.disconnect("cancelled").iter().any(|r| matches!(
                    r,
                    LoadResult::Track {
                        generation: 3,
                        request_id: 7,
                        result: Err(_),
                        ..
                    }
                )));
                assert!(client.preparing.is_none());
            } else {
                let mut yields = 0;
                loop {
                    match step {
                        ClientStep::Yield => {
                            yields += 1;
                            assert!(yields < 100, "preparation must finish");
                            step = client.step().unwrap();
                        }
                        ClientStep::Complete {
                            ack: actual,
                            result,
                        } => {
                            assert_eq!(actual, ack);
                            let LoadResult::Track {
                                generation: 3,
                                request_id: 7,
                                result,
                                ..
                            } = result
                            else {
                                panic!("prepared track");
                            };
                            let loaded = result.unwrap();
                            assert_eq!(loaded.generators[0].transactions().len(), 5000);
                            assert_eq!(loaded.generators[0].depth(), 1);
                            assert_eq!(loaded.generators[0].median_lifetime(), 5);
                            assert!(raw_owner.upgrade().is_some());
                            break;
                        }
                        ClientStep::Ack(_) => panic!("acknowledgement before preparation"),
                    }
                }
            }
            drop(client);
            assert!(raw_owner.upgrade().is_none());
            assert_eq!(budget.used(), baseline);
        }
    }
}
