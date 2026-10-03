//! One remote connection's complete-object queue. The host sends returned
//! commands, drives bounded steps, and delivers complete results to the caller.

use super::ClientStep;
use super::activity::ActivityTransfer;
use super::memory::MemoryBudget;
use super::open::OpenTransfer;
use super::signals::SignalTransfer;
use super::track_transfer::TrackTransfer;
use super::transport::{Body, Command, MAX_BATCH, Packet};
use crate::session::{LoadRequest, LoadResult};
use std::collections::VecDeque;

struct Queued<Tag> {
    job: LoadRequest<Tag>,
    reservation: Option<super::memory::Reservation>,
}
impl<Tag> Queued<Tag> {
    fn new(job: LoadRequest<Tag>) -> Self {
        Self {
            job,
            reservation: None,
        }
    }
}

enum Active<Tag> {
    Open {
        tag: Tag,
        generation: u64,
        transfer: OpenTransfer<Tag>,
    },
    Signals {
        job: LoadRequest<Tag>,
        transfer: SignalTransfer<Tag>,
    },
    Track {
        job: LoadRequest<Tag>,
        transfer: TrackTransfer<Tag>,
    },
    Activity {
        transfer: ActivityTransfer<Tag>,
    },
    Resolve {
        job: LoadRequest<Tag>,
        transfer: Option<SignalTransfer<Tag>>,
        next: usize,
        changed: Vec<crate::data::SignalRef>,
        error: Option<String>,
        _reservation: super::memory::Reservation,
    },
}

/// Queues Open, signal and track loads for one connection and runs one
/// command at a time; signal requests are deduplicated and split into
/// [`MAX_BATCH`] batches. Hosts send each [`ClientStep<Tag>`] acknowledgement and
/// deliver its result before [`take_command`](Self::take_command).
pub struct RemoteClient<Tag = u64> {
    session: Option<u64>,
    request: u64,
    limit: u64,
    budget: MemoryBudget,
    active: Option<Active<Tag>>,
    queued: VecDeque<Queued<Tag>>,
    opening: Option<Packet>,
    connected: bool,
}

impl<Tag: Copy> RemoteClient<Tag> {
    /// A connection that opens the recording of tag `tag`.
    pub fn new(
        tag: Tag,
        generation: u64,
        limit: u64,
        budget: MemoryBudget,
    ) -> anyhow::Result<Self> {
        let request = 1;
        let transfer = OpenTransfer::new(request, tag, generation, limit, budget.clone())?;
        let opening = Some(transfer.command());
        Ok(Self {
            session: None,
            request,
            limit,
            budget,
            active: Some(Active::Open {
                tag,
                generation,
                transfer,
            }),
            queued: VecDeque::new(),
            opening,
            connected: true,
        })
    }

    /// The server's identity for the open recording, once it is open.
    pub fn session(&self) -> Option<u64> {
        self.session
    }

    /// Submit the same work item used by the local executor. Rejected work
    /// returns a normal completion with the caller's correlation.
    pub fn submit(&mut self, request: LoadRequest<Tag>) -> Result<(), LoadResult<Tag>> {
        if !self.connected || self.session.is_none() || self.session != request.remote_id() {
            return Err(request.fail(anyhow::anyhow!(
                "remote connection is closed or belongs to another recording"
            )));
        }
        match request {
            LoadRequest::Signals {
                tag,
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
                    self.queued.push_back(Queued::new(LoadRequest::Signals {
                        tag,
                        generation,
                        session: session.clone(),
                        signals: ids.to_vec(),
                    }));
                }
            }
            request @ LoadRequest::ResolveActivity { .. } => {
                let LoadRequest::ResolveActivity {
                    signals, budget, ..
                } = &request
                else {
                    unreachable!()
                };
                if signals.is_empty() {
                    return Err(request.fail(anyhow::anyhow!("no undecided activity signals")));
                }
                let reservation = match budget
                    .reserve((signals.len() * std::mem::size_of::<crate::data::SignalRef>()) as u64)
                {
                    Ok(reservation) => reservation,
                    Err(error) => return Err(request.fail(error)),
                };
                self.queued.push_back(Queued {
                    job: request,
                    reservation: Some(reservation),
                });
            }
            request @ (LoadRequest::Track { .. } | LoadRequest::BuildActivity { .. }) => {
                self.queued.push_back(Queued::new(request))
            }
        }
        Ok(())
    }

    /// Call after sending the preceding step's ACK and delivering its result.
    /// At most one command is active, regardless of how many loads are queued.
    pub fn take_command(&mut self) -> anyhow::Result<Option<Packet>> {
        if let Some(open) = self.opening.take() {
            return Ok(Some(open));
        }
        if !self.connected {
            return Ok(None);
        }
        if self.active.is_none()
            && matches!(
                self.queued.front().map(|q| &q.job),
                Some(LoadRequest::ResolveActivity { .. })
            )
        {
            let queued = self.queued.pop_front().unwrap();
            let job = queued.job;
            let LoadRequest::ResolveActivity { signals, .. } = &job else {
                unreachable!()
            };
            let reservation = queued.reservation.expect("admitted activity read");
            let changed = Vec::with_capacity(signals.len());
            self.active = Some(Active::Resolve {
                job,
                transfer: None,
                next: 0,
                changed,
                error: None,
                _reservation: reservation,
            });
        }
        if let Some(Active::Resolve {
            job,
            transfer: transfer @ None,
            next,
            ..
        }) = &mut self.active
        {
            let LoadRequest::ResolveActivity {
                tag,
                generation,
                signals,
                ..
            } = job
            else {
                unreachable!()
            };
            self.request = self
                .request
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("request identity exhausted"))?;
            let end = (*next + MAX_BATCH).min(signals.len());
            let ids = &signals[*next..end];
            let session = self
                .session
                .ok_or_else(|| anyhow::anyhow!("recording is not open"))?;
            *transfer = Some(SignalTransfer::new(
                session,
                self.request,
                *tag,
                *generation,
                ids,
                self.limit,
                self.budget.clone(),
            )?);
            *next = end;
            return Ok(Some(Packet {
                session,
                request: self.request,
                sequence: 0,
                body: Body::Command(Command::Signals(ids.iter().map(|id| id.0).collect())),
            }));
        }
        if self.active.is_some() {
            return Ok(None);
        }
        let Some(job) = self.queued.front().map(|q| &q.job) else {
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
                tag,
                generation,
                signals,
                ..
            } => {
                let transfer = SignalTransfer::new(
                    session,
                    self.request,
                    *tag,
                    *generation,
                    signals,
                    self.limit,
                    self.budget.clone(),
                )?;
                let command = Command::Signals(signals.iter().map(|id| id.0).collect());
                self.active = Some(Active::Signals {
                    job: self.queued.pop_front().unwrap().job,
                    transfer,
                });
                command
            }
            LoadRequest::Track {
                tag,
                generation,
                request_id,
                session,
                track,
            } => {
                let transfer = TrackTransfer::new(
                    self.request,
                    *tag,
                    *generation,
                    *request_id,
                    session.clone(),
                    *track,
                    self.limit,
                    self.budget.clone(),
                )?;
                let command = Command::Track(track.0);
                self.active = Some(Active::Track {
                    job: self.queued.pop_front().unwrap().job,
                    transfer,
                });
                command
            }
            LoadRequest::BuildActivity { .. } => {
                let transfer = ActivityTransfer::new(
                    self.request,
                    self.queued.pop_front().unwrap().job,
                    self.limit,
                    self.budget.clone(),
                )?;
                let command = transfer.command();
                self.active = Some(Active::Activity { transfer });
                command
            }
            LoadRequest::ResolveActivity { .. } => {
                unreachable!("resolve starts before ordinary queue")
            }
        };
        let packet = Packet {
            session,
            request: self.request,
            sequence: 0,
            body: Body::Command(command),
        };
        Ok(Some(packet))
    }

    /// Filter queued demand without exposing a viewer or cancelling active protocol responses.
    pub fn retain_queued(&mut self, mut wanted: impl FnMut(&mut LoadRequest<Tag>) -> bool) {
        self.queued.retain_mut(|queued| wanted(&mut queued.job));
    }

    pub fn accept(&mut self, packet: Packet) -> anyhow::Result<ClientStep<Tag>> {
        let step = match self
            .active
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("unsolicited remote response"))?
        {
            Active::Open { transfer, .. } => transfer.accept(packet)?,
            Active::Signals { transfer, .. } => transfer.accept(packet)?,
            Active::Track { transfer, .. } => transfer.accept(packet)?,
            Active::Activity { transfer } => transfer.accept(packet)?,
            Active::Resolve { transfer, .. } => transfer
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("no active activity read"))?
                .accept(packet)?,
        };
        self.accept_step(step)
    }

    pub fn step(&mut self) -> anyhow::Result<ClientStep<Tag>> {
        let step = match self
            .active
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("no active remote decoder"))?
        {
            Active::Open { transfer, .. } => transfer.step()?,
            Active::Signals { transfer, .. } => transfer.step()?,
            Active::Track { transfer, .. } => transfer.step()?,
            Active::Activity { transfer } => transfer.step()?,
            Active::Resolve { transfer, .. } => transfer
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("no active activity read"))?
                .step()?,
        };
        self.accept_step(step)
    }

    fn accept_step(&mut self, step: ClientStep<Tag>) -> anyhow::Result<ClientStep<Tag>> {
        let step = if let (
            Some(Active::Resolve {
                job,
                transfer,
                next,
                changed,
                error,
                ..
            }),
            ClientStep::Complete {
                ack,
                result: LoadResult::Signals { results, .. },
            },
        ) = (self.active.as_mut(), &step)
        {
            let LoadRequest::ResolveActivity {
                tag,
                generation,
                signals,
                window,
                session,
                ..
            } = job
            else {
                unreachable!()
            };
            for (id, result) in results {
                match result {
                    Ok(history) => {
                        if window.0 <= window.1
                            && history.index_at(window.1).is_some_and(|i| {
                                history.time(i) >= window.0
                                    && history.time(i) != session.info().time_range.0
                            })
                        {
                            changed.push(*id);
                        }
                    }
                    Err(e) => *error = Some(format!("{e:#}")),
                }
            }
            let finished = transfer.as_ref().is_some_and(|t| t.is_complete());
            if finished {
                transfer.take().unwrap().finish()?;
                if *next == signals.len() || error.is_some() {
                    let result = if let Some(message) = error.take() {
                        Err(anyhow::anyhow!(message))
                    } else {
                        Ok(std::mem::take(changed))
                    };
                    let result = LoadResult::ActivityResolved {
                        tag: *tag,
                        generation: *generation,
                        window: *window,
                        result,
                    };
                    self.active = None;
                    return Ok(ClientStep::Complete {
                        ack: ack.clone(),
                        result,
                    });
                }
            }
            ClientStep::Ack(ack.clone())
        } else {
            step
        };
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
                LoadResult::Track { .. }
                | LoadResult::ActivityBuilt { .. }
                | LoadResult::ActivityResolved { .. } => {}
            }
        }
        let complete = match self.active.as_ref() {
            Some(Active::Open { transfer, .. }) => transfer.is_complete(),
            Some(Active::Signals { transfer, .. }) => transfer.is_complete(),
            Some(Active::Track { transfer, .. }) => transfer.is_complete(),
            Some(Active::Activity { transfer }) => transfer.is_complete(),
            Some(Active::Resolve { .. }) => false,
            None => false,
        };
        if complete {
            match self.active.take().unwrap() {
                Active::Open { transfer, .. } => transfer.finish()?,
                Active::Signals { transfer, .. } => transfer.finish()?,
                Active::Track { transfer, .. } => transfer.finish()?,
                Active::Activity { transfer } => transfer.finish()?,
                Active::Resolve { .. } => unreachable!(),
            }
        }
        Ok(step)
    }

    /// Fail unfinished loads on transport/decode failure. Previously delivered
    /// objects remain with their consumers and work without this connection.
    pub fn disconnect(&mut self, message: &str) -> Vec<LoadResult<Tag>> {
        self.connected = false;
        self.opening = None;
        let mut results = Vec::new();
        match self.active.take() {
            Some(Active::Open {
                tag, generation, ..
            }) => results.push(LoadResult::Opened {
                tag,
                generation,
                result: Err(anyhow::anyhow!(message.to_owned())),
            }),
            Some(Active::Activity { transfer }) => results.push(transfer.fail(message)),
            Some(
                Active::Signals { job, .. }
                | Active::Track { job, .. }
                | Active::Resolve { job, .. },
            ) => self.queued.push_front(Queued::new(job)),
            None => {}
        }
        results.extend(
            self.queued
                .drain(..)
                .map(|queued| queued.job.fail(anyhow::anyhow!(message.to_owned()))),
        );
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::SignalRef;
    use crate::data::history::VecHistory;
    use crate::data::{SignalShape, WaveValue};
    use crate::remote::history::PackedHistory;
    use crate::remote::objects::Metadata;
    use crate::remote::transport::{DATA_BYTES, ObjectId, acknowledgement};
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
        source: &dyn crate::session::Session,
    ) -> LoadResult {
        let mut sequence = 0;
        let header = crate::remote::hierarchy::Header::from_session(source).unwrap();
        let sizes = crate::data::ScopeSizes::count(source.hierarchy());
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
                bincode::serialize(&crate::remote::hierarchy::Page::scopes(
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
                bincode::serialize(&crate::remote::hierarchy::Page::vars(
                    source.hierarchy(),
                    page,
                ))
                .unwrap(),
            );
        }
        completed.expect("completed Open")
    }

    #[test]
    fn command_failure_keeps_work_for_failure_delivery() {
        let mut client =
            RemoteClient::new(0, 1, 1024 * 1024, MemoryBudget::new(4 * 1024 * 1024)).unwrap();
        let open = client.take_command().unwrap().unwrap();
        let local = crate::testing::ProceduralTrace::session(100);
        let LoadResult::Opened { result, .. } = opened(&mut client, open.request, local.as_ref())
        else {
            panic!("opened")
        };
        let session = result.unwrap();
        assert!(
            client
                .submit(LoadRequest::Signals {
                    tag: 0,
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
            tag: _,
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
        let mut client = RemoteClient::new(0, 70, 1024 * 1024, budget.clone()).unwrap();
        let open = client.take_command().unwrap().unwrap();
        assert_eq!(open.session, 0);
        assert!(client.take_command().unwrap().is_none());
        let local = crate::testing::ProceduralTrace::session(100);
        let opened = opened(&mut client, open.request, local.as_ref());
        let LoadResult::Opened {
            tag: _,
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
            tag: _,
            generation,
            results,
        }) = client.submit(LoadRequest::Signals {
            tag: 0,
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
                    tag: 0,
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
            tag: _,
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
                tag: _,
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
                    tag: 0,
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
