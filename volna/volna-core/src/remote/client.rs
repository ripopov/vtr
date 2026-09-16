//! One remote connection's complete-object queue. The host sends returned
//! commands, drives bounded steps, and delivers results to the ordinary App.

use super::ClientStep;
use super::memory::MemoryBudget;
use super::open::OpenTransfer;
use super::signals::SignalTransfer;
use super::track_transfer::TrackTransfer;
use super::transport::{Body, Command, MAX_BATCH, Packet};
use crate::session::{LoadRequest, LoadResult};
use std::collections::VecDeque;

enum Active {
    Open {
        generation: u64,
        transfer: OpenTransfer,
    },
    Signals {
        job: LoadRequest,
        transfer: SignalTransfer,
    },
    Track {
        job: LoadRequest,
        transfer: TrackTransfer,
    },
}

pub struct RemoteClient {
    session: Option<u64>,
    request: u64,
    limit: u64,
    budget: MemoryBudget,
    active: Option<Active>,
    queued: VecDeque<LoadRequest>,
    opening: Option<Packet>,
    connected: bool,
}

impl RemoteClient {
    pub fn new(generation: u64, limit: u64, budget: MemoryBudget) -> anyhow::Result<Self> {
        let transfer = OpenTransfer::new(1, generation, limit, budget.clone())?;
        let opening = Some(transfer.command());
        Ok(Self {
            session: None,
            request: 1,
            limit,
            budget,
            active: Some(Active::Open {
                generation,
                transfer,
            }),
            queued: VecDeque::new(),
            opening,
            connected: true,
        })
    }

    /// Submit the same work item used by the local executor. Rejected work
    /// returns a normal completion for the document to deliver.
    pub fn submit(&mut self, request: LoadRequest) -> Result<(), LoadResult> {
        if !self.connected || self.session.is_none() || self.session != request.remote_id() {
            return Err(request.fail(anyhow::anyhow!(
                "remote connection is closed or belongs to another recording"
            )));
        }
        match request {
            LoadRequest::Signals {
                generation,
                signals,
                session,
            } => {
                let mut unique = std::collections::HashSet::new();
                let signals: Vec<_> = signals
                    .into_iter()
                    .filter(|id| unique.insert(*id))
                    .collect();
                for ids in signals.chunks(MAX_BATCH) {
                    self.queued.push_back(LoadRequest::Signals {
                        generation,
                        session: session.clone(),
                        signals: ids.to_vec(),
                    });
                }
            }
            request @ LoadRequest::Track { .. } => self.queued.push_back(request),
            LoadRequest::Open { .. } => unreachable!("open requests have no remote identity"),
        }
        Ok(())
    }

    /// Call after sending the preceding step's ACK and delivering its result.
    /// At most one command is active, regardless of how many loads are queued.
    pub fn take_command(&mut self) -> anyhow::Result<Option<Packet>> {
        if let Some(open) = self.opening.take() {
            return Ok(Some(open));
        }
        if !self.connected || self.active.is_some() {
            return Ok(None);
        }
        let Some(job) = self.queued.front() else {
            return Ok(None);
        };
        self.request = self
            .request
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("request identity exhausted"))?;
        let session = self
            .session
            .ok_or_else(|| anyhow::anyhow!("recording is not open"))?;
        let command = match job {
            LoadRequest::Signals {
                generation,
                signals,
                ..
            } => {
                let transfer = SignalTransfer::new(
                    session,
                    self.request,
                    *generation,
                    signals,
                    self.limit,
                    self.budget.clone(),
                )?;
                let command = Command::Signals(signals.iter().map(|id| id.0).collect());
                self.active = Some(Active::Signals {
                    job: self.queued.pop_front().unwrap(),
                    transfer,
                });
                command
            }
            LoadRequest::Track {
                generation,
                request_id,
                session,
                track,
            } => {
                let transfer = TrackTransfer::new(
                    self.request,
                    *generation,
                    *request_id,
                    session.clone(),
                    *track,
                    self.limit,
                    self.budget.clone(),
                )?;
                let command = Command::Track(track.0);
                self.active = Some(Active::Track {
                    job: self.queued.pop_front().unwrap(),
                    transfer,
                });
                command
            }
            LoadRequest::Open { .. } => unreachable!("only object loads are queued"),
        };
        let packet = Packet {
            session,
            request: self.request,
            sequence: 0,
            body: Body::Command(command),
        };
        Ok(Some(packet))
    }

    /// Discard queued viewer work whose last consumer has gone away. Active
    /// responses keep their normal acknowledgement/completion lifecycle.
    pub fn sync_demand(&mut self, app: &mut crate::app::App) {
        if self.queued.is_empty() {
            return;
        }
        let wanted = app.signal_demand();
        self.queued.retain_mut(|job| match job {
            LoadRequest::Signals {
                generation,
                signals,
                ..
            } => app.doc.retain_queued_signals(*generation, signals, &wanted),
            LoadRequest::Track {
                generation,
                request_id,
                track,
                ..
            } => app
                .doc
                .wants_track_request(*generation, *request_id, *track),
            LoadRequest::Open { .. } => unreachable!("only object loads are queued"),
        });
    }

    pub fn accept(&mut self, packet: Packet) -> anyhow::Result<ClientStep> {
        let step = match self
            .active
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("unsolicited remote response"))?
        {
            Active::Open { transfer, .. } => transfer.accept(packet)?,
            Active::Signals { transfer, .. } => transfer.accept(packet)?,
            Active::Track { transfer, .. } => transfer.accept(packet)?,
        };
        self.accept_step(step)
    }

    pub fn step(&mut self) -> anyhow::Result<ClientStep> {
        let step = match self
            .active
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("no active remote decoder"))?
        {
            Active::Open { transfer, .. } => transfer.step()?,
            Active::Signals { transfer, .. } => transfer.step()?,
            Active::Track { transfer, .. } => transfer.step()?,
        };
        self.accept_step(step)
    }

    fn accept_step(&mut self, step: ClientStep) -> anyhow::Result<ClientStep> {
        if let ClientStep::Complete { result, .. } = &step {
            match result {
                LoadResult::Opened { result, .. } => {
                    self.session = result.as_ref().ok().and_then(|s| s.remote_id());
                    if self.session.is_none() {
                        self.connected = false;
                    }
                }
                LoadResult::Signals { results, .. } => {
                    if let Some(Active::Signals {
                        job: LoadRequest::Signals { signals, .. },
                        ..
                    }) = self.active.as_mut()
                    {
                        signals.retain(|id| !results.iter().any(|(done, _)| id == done));
                    }
                }
                LoadResult::Track { .. } => {}
            }
        }
        let complete = match self.active.as_ref() {
            Some(Active::Open { transfer, .. }) => transfer.is_complete(),
            Some(Active::Signals { transfer, .. }) => transfer.is_complete(),
            Some(Active::Track { transfer, .. }) => transfer.is_complete(),
            None => false,
        };
        if complete {
            match self.active.take().unwrap() {
                Active::Open { transfer, .. } => transfer.finish()?,
                Active::Signals { transfer, .. } => transfer.finish()?,
                Active::Track { transfer, .. } => transfer.finish()?,
            }
        }
        Ok(step)
    }

    /// Fail unfinished loads on transport/decode failure. Previously delivered
    /// objects remain with their consumers and work without this connection.
    pub fn disconnect(&mut self, message: &str) -> Vec<LoadResult> {
        self.connected = false;
        self.opening = None;
        let mut results = Vec::new();
        match self.active.take() {
            Some(Active::Open { generation, .. }) => results.push(LoadResult::Opened {
                generation,
                result: Err(anyhow::anyhow!(message.to_owned())),
            }),
            Some(Active::Signals { job, .. } | Active::Track { job, .. }) => {
                self.queued.push_front(job)
            }
            None => {}
        }
        results.extend(
            self.queued
                .drain(..)
                .map(|job| job.fail(anyhow::anyhow!(message.to_owned()))),
        );
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::SignalRef;
    use crate::data::history::VecHistory;
    use crate::data::transactions::TrackRef;
    use crate::data::{SignalShape, WaveValue};
    use crate::remote::history::PackedHistory;
    use crate::remote::objects::Metadata;
    use crate::remote::transport::{DATA_BYTES, ObjectId, acknowledgement};
    use crate::session::OpenSpec;
    use std::sync::Arc;

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

    fn object(
        client: &mut RemoteClient,
        request: u64,
        sequence: &mut u64,
        object: ObjectId,
        bytes: Vec<u8>,
    ) -> LoadResult {
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
        send(Body::End).unwrap()
    }

    #[test]
    fn removed_queued_signals_do_not_send_and_active_results_can_fill_readded_rows() {
        use crate::app::{Action, App, Command as ViewerCommand};
        let mut client =
            RemoteClient::new(1, 1024 * 1024, MemoryBudget::new(4 * 1024 * 1024)).unwrap();
        let open = client.take_command().unwrap().unwrap();
        let local = OpenSpec::Synthetic(100).open().unwrap();
        let opened = object(
            &mut client,
            open.request,
            &mut 0,
            ObjectId::Metadata,
            bincode::serialize(&Metadata::from_session(local.as_ref())).unwrap(),
        );
        let LoadResult::Opened { result, .. } = opened else {
            panic!("opened")
        };
        let mut app = App::new();
        app.set_session(result.unwrap());
        let enqueue = |app: &mut App, client: &mut RemoteClient| {
            for request in app.take_requests() {
                assert!(client.submit(request).is_ok());
            }
        };
        app.handle(ViewerCommand::AddVars(vec![0]));
        enqueue(&mut app, &mut client);
        let active = client.take_command().unwrap().unwrap();
        app.handle(ViewerCommand::AddVars(vec![1]));
        enqueue(&mut app, &mut client);
        app.handle(ViewerCommand::Action(Action::SelectAll));
        app.handle(ViewerCommand::Action(Action::RemoveSelected));
        client.sync_demand(&mut app);
        assert!(client.queued.is_empty());
        assert!(
            app.doc.is_pending(SignalRef(0)),
            "active work still completes"
        );
        assert!(
            !app.doc.is_pending(SignalRef(1)),
            "removed queued demand is cleared"
        );
        app.handle(ViewerCommand::AddVars(vec![0, 1]));
        enqueue(&mut app, &mut client);
        let completed = object(
            &mut client,
            active.request,
            &mut 0,
            ObjectId::Signal(0),
            bincode::serialize(
                &PackedHistory::from_history(local.load_signal(SignalRef(0)).unwrap().as_ref())
                    .unwrap(),
            )
            .unwrap(),
        );
        app.deliver(completed);
        assert!(
            app.panels.focused_waves().unwrap().items[0]
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
        let stream = writer.add_stream(None, "stream", "raw");
        writer.add_generator(stream, "generator");
        writer.close().unwrap();
        let local = OpenSpec::Path(file.path().into()).open().unwrap();
        let track = TrackRef(stream.0);
        let mut client =
            RemoteClient::new(1, 1024 * 1024, MemoryBudget::new(4 * 1024 * 1024)).unwrap();
        let open = client.take_command().unwrap().unwrap();
        let opened = object(
            &mut client,
            open.request,
            &mut 0,
            ObjectId::Metadata,
            bincode::serialize(&Metadata::from_session(local.as_ref())).unwrap(),
        );
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
        assert_eq!(client.queued.len(), 1);
        assert!(
            matches!(client.queued.front(), Some(LoadRequest::Track { request_id, .. }) if *request_id == current)
        );
        app.doc.release_track(track);
        client.sync_demand(&mut app);
        assert!(client.take_command().unwrap().is_none());
    }

    #[test]
    fn command_failure_keeps_work_for_failure_delivery() {
        let mut client =
            RemoteClient::new(1, 1024 * 1024, MemoryBudget::new(4 * 1024 * 1024)).unwrap();
        let open = client.take_command().unwrap().unwrap();
        let local = OpenSpec::Synthetic(100).open().unwrap();
        let LoadResult::Opened { result, .. } = object(
            &mut client,
            open.request,
            &mut 0,
            ObjectId::Metadata,
            bincode::serialize(&Metadata::from_session(local.as_ref())).unwrap(),
        ) else {
            panic!("opened")
        };
        let session = result.unwrap();
        assert!(
            client
                .submit(LoadRequest::Signals {
                    generation: 7,
                    session,
                    signals: vec![SignalRef(0)]
                })
                .is_ok()
        );
        client.request = u64::MAX;
        assert!(client.take_command().is_err());
        let mut failures = client.disconnect("request identity exhausted");
        assert_eq!(failures.len(), 1);
        let LoadResult::Signals {
            generation,
            results,
        } = failures.pop().unwrap()
        else {
            panic!("signals")
        };
        assert_eq!(generation, 7);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, SignalRef(0));
        assert!(results[0].1.is_err());
    }

    #[test]
    fn queue_serializes_batches_and_disconnect_preserves_completed_histories() {
        let budget = MemoryBudget::new(4 * 1024 * 1024);
        let mut client = RemoteClient::new(70, 1024 * 1024, budget.clone()).unwrap();
        let open = client.take_command().unwrap().unwrap();
        assert_eq!(open.session, 0);
        assert!(client.take_command().unwrap().is_none());
        let local = OpenSpec::Synthetic(100).open().unwrap();
        let opened = object(
            &mut client,
            open.request,
            &mut 0,
            ObjectId::Metadata,
            bincode::serialize(&Metadata::from_session(local.as_ref())).unwrap(),
        );
        let LoadResult::Opened {
            generation: 70,
            result,
        } = opened
        else {
            panic!("Open result");
        };
        let session = result.unwrap();
        let mut ids: Vec<_> = (0..65).map(SignalRef).collect();
        ids.push(SignalRef(0));
        let wrong_session = Arc::new(
            crate::remote::session::RemoteSession::new(32, Metadata::from_session(local.as_ref()))
                .unwrap(),
        );
        let Err(LoadResult::Signals {
            generation,
            results,
        }) = client.submit(LoadRequest::Signals {
            generation: 71,
            session: wrong_session,
            signals: ids.clone(),
        })
        else {
            panic!("rejected submission must return signal completions")
        };
        assert_eq!(generation, 71);
        assert_eq!(results.iter().map(|(id, _)| *id).collect::<Vec<_>>(), ids);
        assert!(results.iter().all(|(_, result)| result.is_err()));
        assert!(
            client
                .submit(LoadRequest::Signals {
                    generation: 71,
                    session: session.clone(),
                    signals: ids
                })
                .is_ok()
        );
        let command = client.take_command().unwrap().unwrap();
        let Body::Command(Command::Signals(ids)) = command.body else {
            panic!("signal command");
        };
        assert_eq!(ids.len(), MAX_BATCH);
        assert_eq!(command.request, open.request + 1);
        assert!(client.take_command().unwrap().is_none());
        let source = VecHistory {
            shape: SignalShape::Bit,
            times: vec![],
            values: vec![],
            initial: WaveValue::Bits("1".into()),
        };
        let completed = object(
            &mut client,
            command.request,
            &mut 0,
            ObjectId::Signal(0),
            bincode::serialize(&PackedHistory::from_history(&source).unwrap()).unwrap(),
        );
        let LoadResult::Signals {
            generation: 71,
            mut results,
        } = completed
        else {
            panic!("signal result");
        };
        let history = results.pop().unwrap().1.unwrap();
        assert!(client.take_command().unwrap().is_none());
        let failures = client.disconnect("connection lost");
        let mut failed = Vec::new();
        for result in failures {
            let LoadResult::Signals {
                generation: 71,
                results,
            } = result
            else {
                panic!("unfinished signal result");
            };
            for (id, error) in results {
                assert!(error.is_err());
                failed.push(id);
            }
        }
        assert_eq!(failed, (1..65).map(SignalRef).collect::<Vec<_>>());
        assert!(client.take_command().unwrap().is_none());
        assert!(
            client
                .submit(LoadRequest::Signals {
                    generation: 71,
                    session: session.clone(),
                    signals: vec![SignalRef(65)]
                })
                .is_err()
        );
        drop(client);
        drop(session);
        assert_eq!(history.value(None), WaveValue::Bits("1".into()));
        assert!(budget.used() > 0);
        drop(history);
        assert_eq!(budget.used(), 0);
    }
}
