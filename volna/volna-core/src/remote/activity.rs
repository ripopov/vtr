//! Raw activity-sidecar identity and availability beside a remote reader.

use crate::session::Session;
use serde::{Deserialize, Serialize};

/// Raw source identity used to reject stale or mismatched sidecars. Format
/// codes match the activity sidecar: 1 is VTR, 2 is FST.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Descriptor {
    pub format: u8,
    pub length: u64,
    pub toc_crc: u32,
    pub available: bool,
}

impl Descriptor {
    pub(crate) fn from_session(session: &dyn Session) -> Option<Self> {
        let id = session.activity_identity()?;
        Some(Self {
            format: match id.format {
                vtr::activity::SourceFormat::Vtr => 1,
                vtr::activity::SourceFormat::Fst => 2,
            },
            length: id.length,
            toc_crc: id.toc_crc,
            available: session.activity_available(),
        })
    }

    /// Validated library identity. Unknown format codes fail explicitly.
    pub fn identity(&self) -> anyhow::Result<vtr::activity::Identity> {
        Ok(vtr::activity::Identity {
            format: match self.format {
                1 => vtr::activity::SourceFormat::Vtr,
                2 => vtr::activity::SourceFormat::Fst,
                _ => anyhow::bail!("invalid activity source format"),
            },
            length: self.length,
            toc_crc: self.toc_crc,
        })
    }
}

use super::ClientStep;
use super::decode::{Decoder, Step};
use super::memory::MemoryBudget;
use super::transport::{ObjectId, Packet, Receive, Receiver, acknowledgement};
use crate::session::{LoadRequest, LoadResult};
use std::sync::Arc;

/// One raw-sidecar transfer. Encoded storage, decoded tables and temporary
/// arrays are admitted before allocation. Checkpoints yield while decoding;
/// cancellation stops client work and drains the server response. A server
/// scan already authorized may still publish its cache for other clients.
pub(super) struct ActivityTransfer {
    job: LoadRequest,
    receiver: Receiver,
    decoder: Option<Decoder<vtr::activity::Index>>,
    decoded: Option<Arc<crate::data::ActivityIndex>>,
    rejected: Option<String>,
    pending_ack: Option<Packet>,
    limit: u64,
    budget: MemoryBudget,
    finished: bool,
    failed: bool,
}

impl ActivityTransfer {
    pub fn new(
        request: u64,
        job: LoadRequest,
        limit: u64,
        budget: MemoryBudget,
    ) -> anyhow::Result<Self> {
        let id = job
            .remote_id()
            .ok_or_else(|| anyhow::anyhow!("activity session is not remote"))?;
        Ok(Self {
            job,
            receiver: Receiver::new(id, request, vec![ObjectId::Activity], limit)?,
            decoder: None,
            decoded: None,
            rejected: None,
            pending_ack: None,
            limit,
            budget,
            finished: false,
            failed: false,
        })
    }

    pub fn command(&self) -> super::transport::Command {
        let LoadRequest::BuildActivity {
            session, options, ..
        } = &self.job
        else {
            unreachable!()
        };
        super::transport::Command::Activity {
            build: !session.activity_available()
                && !options.control.as_ref().is_some_and(|c| c.is_cancelled()),
        }
    }

    pub fn accept(&mut self, packet: Packet) -> anyhow::Result<ClientStep> {
        let result = self.accept_inner(packet);
        self.poison(result)
    }
    fn accept_inner(&mut self, packet: Packet) -> anyhow::Result<ClientStep> {
        anyhow::ensure!(
            !self.failed && !self.finished && self.pending_ack.is_none(),
            "activity transfer finished or decoding"
        );
        let ack = acknowledgement(&packet);
        match self.receiver.accept(packet)? {
            Receive::Begin {
                object: ObjectId::Activity,
                decoded_bytes,
            } => {
                let LoadRequest::BuildActivity {
                    session, options, ..
                } = &self.job
                else {
                    unreachable!()
                };
                let identity = session
                    .activity_identity()
                    .ok_or_else(|| anyhow::anyhow!("missing activity identity"))?;
                let control = options
                    .control
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("missing activity control"))?;
                let limit = self.limit;
                match Decoder::new(decoded_bytes, limit, &self.budget, move |r| {
                    Box::pin(async move {
                        let bytes = r.bytes().await?;
                        let mut charged = 0;
                        let index = vtr::activity::Index::decode_with(
                            &bytes,
                            &identity,
                            |peak| {
                                super::memory::check_object(
                                    "the activity index",
                                    peak,
                                    limit,
                                    "reopen the trace",
                                )
                                .and_then(|_| r.charge(peak.saturating_sub(charged) as usize))
                                .map_err(|e| vtr::Error::Invalid(e.to_string()))?;
                                charged = charged.max(peak);
                                Ok(())
                            },
                            || async {
                                r.checkpoint().await;
                                if control.is_cancelled() {
                                    Err(vtr::Error::Invalid("activity request cancelled".into()))
                                } else {
                                    Ok(())
                                }
                            },
                        )
                        .await?;
                        Ok(index)
                    })
                }) {
                    Ok(decoder) => self.decoder = Some(decoder),
                    Err(error) => self.rejected = Some(format!("{error:#}")),
                }
            }
            Receive::Data(bytes) => {
                if self.rejected.is_none() {
                    self.decoder
                        .as_mut()
                        .ok_or_else(|| anyhow::anyhow!("missing activity decoder"))?
                        .feed(bytes)?;
                    self.pending_ack = Some(ack);
                    return self.step_inner();
                }
            }
            Receive::Complete(ObjectId::Activity) => {
                let result = if let Some(message) = self.rejected.take() {
                    Err(anyhow::anyhow!(message))
                } else {
                    let index = self
                        .decoded
                        .take()
                        .ok_or_else(|| anyhow::anyhow!("incomplete activity validation"))?;
                    let LoadRequest::BuildActivity {
                        session, options, ..
                    } = &self.job
                    else {
                        unreachable!()
                    };
                    options
                        .control
                        .as_ref()
                        .expect("activity control")
                        .complete()
                        .map_err(anyhow::Error::from)
                        .and_then(|_| session.install_activity(index))
                };
                return Ok(self.complete(ack, result));
            }
            Receive::Failed {
                object: ObjectId::Activity,
                message,
            } => {
                return Ok(self.complete(ack, Err(anyhow::anyhow!(message))));
            }
            _ => anyhow::bail!("unexpected activity object"),
        }
        Ok(ClientStep::Ack(ack))
    }
    pub fn step(&mut self) -> anyhow::Result<ClientStep> {
        let result = self.step_inner();
        self.poison(result)
    }
    fn step_inner(&mut self) -> anyhow::Result<ClientStep> {
        anyhow::ensure!(
            self.pending_ack.is_some() && !self.failed,
            "no pending activity decode"
        );
        match self
            .decoder
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("missing activity decoder"))?
            .step()
        {
            Ok(Step::Yield) => return Ok(ClientStep::Yield),
            Ok(Step::NeedInput) => {}
            Ok(Step::Ready(index, mut reservation)) => {
                reservation.shrink(reservation.bytes() - index.memory_bytes())?;
                self.decoded = Some(Arc::new(crate::data::ActivityIndex {
                    index,
                    _reservation: Some(reservation),
                }));
                self.decoder = None;
            }
            Err(error) => {
                self.decoder = None;
                self.rejected = Some(format!("{error:#}"));
            }
        }
        Ok(ClientStep::Ack(self.pending_ack.take().unwrap()))
    }
    fn complete(&mut self, ack: Packet, result: anyhow::Result<()>) -> ClientStep {
        self.decoder = None;
        self.decoded = None;
        self.rejected = None;
        self.finished = true;
        let LoadRequest::BuildActivity {
            trace, generation, ..
        } = &self.job
        else {
            unreachable!()
        };
        ClientStep::Complete {
            ack,
            result: LoadResult::ActivityBuilt {
                trace: *trace,
                generation: *generation,
                result,
            },
        }
    }
    fn poison<T>(&mut self, result: anyhow::Result<T>) -> anyhow::Result<T> {
        if result.is_err() {
            self.failed = true;
            self.decoder = None;
            self.decoded = None;
            self.pending_ack = None;
        }
        result
    }
    pub fn is_complete(&self) -> bool {
        self.finished && !self.failed
    }
    pub fn finish(self) -> anyhow::Result<()> {
        anyhow::ensure!(self.is_complete(), "incomplete activity transfer");
        self.receiver.finish()
    }
    pub fn fail(self, message: &str) -> LoadResult {
        self.job.fail(anyhow::anyhow!(message.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::objects::Metadata;
    use crate::remote::session::RemoteSession;
    use crate::remote::transport::{Body, DATA_BYTES};
    use crate::trace::TraceId;

    fn fixture(signals: usize) -> (tempfile::TempDir, Arc<dyn Session>, Vec<u8>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote.vtr");
        let mut writer = vtr::Writer::create(&path).unwrap();
        let top = Some(
            writer
                .add_scope(None, "top", vtr::ScopeType::Module, "top")
                .unwrap(),
        );
        for s in 0..signals {
            writer
                .add_var(
                    top,
                    &format!("s{s}"),
                    vtr::VarType::Wire,
                    vtr::Direction::Input,
                    vtr::SignalKind::Bits {
                        width: 1,
                        states: 2,
                    },
                )
                .unwrap();
        }
        writer.close().unwrap();
        let source = crate::session::OpenSpec::Path(path.clone()).open().unwrap();
        let reader = vtr::Reader::open(&path).unwrap();
        let mut image = vec![];
        vtr::activity::build(&reader, &mut image, &Default::default()).unwrap();
        (dir, source, image)
    }
    fn transfer(
        source: &dyn Session,
        limit: u64,
        budget: MemoryBudget,
    ) -> (
        Arc<dyn Session>,
        Arc<vtr::activity::BuildControl>,
        ActivityTransfer,
    ) {
        let session: Arc<dyn Session> =
            Arc::new(RemoteSession::new(73, Metadata::from_session(source)).unwrap());
        let control = Arc::new(vtr::activity::BuildControl::default());
        let job = LoadRequest::BuildActivity {
            trace: TraceId::A,
            generation: 9,
            session: session.clone(),
            options: vtr::activity::BuildOptions {
                control: Some(control.clone()),
                ..Default::default()
            },
            cache_dir: None,
            budget: budget.clone(),
        };
        (
            session,
            control,
            ActivityTransfer::new(8, job, limit, budget).unwrap(),
        )
    }
    fn packet(sequence: u64, body: Body) -> Packet {
        Packet {
            session: 73,
            request: 8,
            sequence,
            body,
        }
    }
    fn response(transfer: &mut ActivityTransfer, packet: Packet) -> (usize, Option<LoadResult>) {
        let expected = acknowledgement(&packet);
        let mut step = transfer.accept(packet).unwrap();
        let mut yields = 0;
        loop {
            match step {
                ClientStep::Yield => {
                    yields += 1;
                    step = transfer.step().unwrap();
                }
                ClientStep::Ack(ack) => {
                    assert_eq!(ack, expected);
                    return (yields, None);
                }
                ClientStep::Complete { ack, result } => {
                    assert_eq!(ack, expected);
                    return (yields, Some(result));
                }
            }
        }
    }
    fn feed(transfer: &mut ActivityTransfer, image: &[u8]) -> (u64, usize) {
        let bytes = bincode::serialize(&image).unwrap();
        response(
            transfer,
            packet(
                0,
                Body::Begin {
                    object: ObjectId::Activity,
                    decoded_bytes: bytes.len() as u64,
                },
            ),
        );
        let mut yields = 0;
        for (i, chunk) in bytes.chunks(DATA_BYTES).enumerate() {
            let (n, result) = response(
                transfer,
                packet(
                    i as u64 + 1,
                    Body::Data {
                        offset: (i * DATA_BYTES) as u64,
                        bytes: chunk.to_vec(),
                    },
                ),
            );
            yields += n;
            assert!(result.is_none(), "no publication before End");
        }
        (bytes.len().div_ceil(DATA_BYTES) as u64 + 1, yields)
    }
    fn finish(transfer: &mut ActivityTransfer, end: u64) -> anyhow::Result<()> {
        let (_, result) = response(transfer, packet(end, Body::End));
        let Some(LoadResult::ActivityBuilt {
            trace: TraceId::A,
            generation: 9,
            result,
        }) = result
        else {
            panic!("wrong completion identity")
        };
        result
    }

    #[test]
    fn cooperative_decode_installs_only_at_end_and_reservation_follows_last_owner() {
        let (_dir, source, image) = fixture(20_000);
        let budget = MemoryBudget::new(4 << 20);
        let (session, _, mut t) = transfer(source.as_ref(), 2 << 20, budget.clone());
        let (end, yields) = feed(&mut t, &image);
        assert!(yields > 20, "signal-table decoding yields to its host");
        assert!(session.activity().is_none());
        finish(&mut t, end).unwrap();
        t.finish().unwrap();
        let index = session.activity().unwrap();
        assert_eq!(budget.used(), index.memory_bytes());
        drop(session);
        assert_eq!(budget.used(), index.memory_bytes());
        drop(index);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn cancelled_or_incomplete_transfers_release_private_storage() {
        let (_dir, source, image) = fixture(2000);
        for cancelled in [false, true] {
            let budget = MemoryBudget::new(2 << 20);
            let (session, control, mut t) = transfer(source.as_ref(), 1 << 20, budget.clone());
            let (end, _) = feed(&mut t, &image);
            assert!(session.activity().is_none());
            if cancelled {
                assert!(control.cancel());
                assert!(
                    finish(&mut t, end)
                        .unwrap_err()
                        .to_string()
                        .contains("cancelled")
                );
                t.finish().unwrap();
            } else {
                assert!(t.finish().is_err());
            }
            assert!(session.activity().is_none());
            assert_eq!(budget.used(), 0);
        }
    }

    #[test]
    fn budget_and_decoded_object_failures_drain_without_installing() {
        let (_dir, source, image) = fixture(2000);
        for (limit, pool) in [(1 << 20, 10), (1024, 2 << 20)] {
            let budget = MemoryBudget::new(pool);
            let (session, _, mut t) = transfer(source.as_ref(), limit, budget.clone());
            let (end, _) = feed(&mut t, &image);
            assert!(finish(&mut t, end).is_err());
            t.finish().unwrap();
            assert!(session.activity().is_none());
            assert_eq!(budget.used(), 0);
        }
    }

    #[test]
    fn mismatched_identity_and_damaged_images_are_object_errors() {
        let (_dir, source, mut image) = fixture(20);
        let mut metadata = Metadata::from_session(source.as_ref());
        metadata.activity.as_mut().unwrap().length += 1;
        let wrong: Arc<dyn Session> = Arc::new(RemoteSession::new(72, metadata).unwrap());
        let budget = MemoryBudget::new(2 << 20);
        let (session, _, mut t) = transfer(wrong.as_ref(), 1 << 20, budget.clone());
        let (end, _) = feed(&mut t, &image);
        assert!(finish(&mut t, end).is_err());
        t.finish().unwrap();
        assert!(session.activity().is_none());
        image[vtr::container::FILE_HEADER_LEN + vtr::container::SECTION_HEADER_LEN] ^= 1;
        let (session, _, mut t) = transfer(source.as_ref(), 1 << 20, budget.clone());
        let (end, _) = feed(&mut t, &image);
        assert!(finish(&mut t, end).is_err());
        t.finish().unwrap();
        assert!(session.activity().is_none());
        assert_eq!(budget.used(), 0);
    }
}
