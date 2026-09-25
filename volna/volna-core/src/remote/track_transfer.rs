//! Frame-to-document delivery of one complete transaction track.
use super::ClientStep;
use super::memory::MemoryBudget;
use super::tracks::{TrackDecoder, TrackStep};
use super::transport::{ObjectId, Packet, Receive, Receiver, acknowledgement};
use crate::data::loaded_tracks::LoadedTrack;
use crate::data::transactions::TrackRef;
use crate::session::{LoadResult, Session};
use std::sync::Arc;

pub struct TrackTransfer {
    generation: u64,
    request_id: u64,
    session: Arc<dyn Session>,
    track: TrackRef,
    limit: u64,
    budget: MemoryBudget,
    receiver: Receiver,
    decoder: Option<TrackDecoder>,
    decoded: Option<LoadedTrack>,
    rejected: Option<String>,
    pending_ack: Option<Packet>,
    finished: bool,
    failed: bool,
}

impl TrackTransfer {
    pub fn new(
        request: u64,
        generation: u64,
        request_id: u64,
        session: Arc<dyn Session>,
        track: TrackRef,
        limit: u64,
        budget: MemoryBudget,
    ) -> anyhow::Result<Self> {
        let id = session
            .remote_id()
            .ok_or_else(|| anyhow::anyhow!("track session is not remote"))?;
        Ok(Self {
            generation,
            request_id,
            session,
            track,
            limit,
            budget,
            receiver: Receiver::new(id, request, vec![ObjectId::Track(track.0)], limit)?,
            decoder: None,
            decoded: None,
            rejected: None,
            pending_ack: None,
            finished: false,
            failed: false,
        })
    }

    pub fn accept(&mut self, packet: Packet) -> anyhow::Result<ClientStep> {
        let result = self.accept_inner(packet);
        self.poison_on_error(result)
    }

    fn accept_inner(&mut self, packet: Packet) -> anyhow::Result<ClientStep> {
        anyhow::ensure!(!self.failed && !self.finished, "track transfer finished");
        anyhow::ensure!(
            self.pending_ack.is_none(),
            "response before track decode finished"
        );
        let ack = acknowledgement(&packet);
        match self.receiver.accept(packet)? {
            Receive::Begin {
                object: ObjectId::Track(_),
                decoded_bytes,
            } => {
                match TrackDecoder::new(
                    decoded_bytes,
                    self.limit,
                    &self.budget,
                    self.session.clone(),
                ) {
                    Ok(decoder) => self.decoder = Some(decoder),
                    Err(error) => self.rejected = Some(format!("{error:#}")),
                }
            }
            Receive::Data(bytes) => {
                if self.rejected.is_none() {
                    self.decoder
                        .as_mut()
                        .ok_or_else(|| anyhow::anyhow!("missing track decoder"))?
                        .feed(bytes)?;
                    self.pending_ack = Some(ack);
                    return self.step_inner();
                }
            }
            Receive::Complete(ObjectId::Track(_)) => {
                let result = if let Some(message) = self.rejected.take() {
                    Err(anyhow::anyhow!(message))
                } else {
                    self.decoded
                        .take()
                        .ok_or_else(|| anyhow::anyhow!("incomplete track validation"))
                };
                return Ok(self.complete(ack, result));
            }
            Receive::Failed {
                object: ObjectId::Track(_),
                message,
            } => return Ok(self.complete(ack, Err(anyhow::anyhow!(message)))),
            _ => anyhow::bail!("unexpected track object"),
        }
        Ok(ClientStep::Ack(ack))
    }

    pub fn step(&mut self) -> anyhow::Result<ClientStep> {
        let result = self.step_inner();
        self.poison_on_error(result)
    }

    fn step_inner(&mut self) -> anyhow::Result<ClientStep> {
        anyhow::ensure!(
            !self.failed && self.pending_ack.is_some(),
            "no pending track decode"
        );
        match self
            .decoder
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("missing track decoder"))?
            .step()
        {
            Ok(TrackStep::Yield) => return Ok(ClientStep::Yield),
            Ok(TrackStep::NeedInput) => {}
            Ok(TrackStep::Decoded(track)) => {
                self.decoder = None;
                if track.track == self.track {
                    self.decoded = Some(track);
                } else {
                    self.rejected = Some("track payload identity mismatch".into());
                }
            }
            Err(error) => {
                // Drain the remainder with backpressure, releasing private
                // allocations immediately. Later requests can still succeed.
                self.decoder = None;
                self.rejected = Some(format!("{error:#}"));
            }
        }
        Ok(ClientStep::Ack(self.pending_ack.take().unwrap()))
    }

    fn complete(&mut self, ack: Packet, result: anyhow::Result<LoadedTrack>) -> ClientStep {
        self.decoder = None;
        self.decoded = None;
        self.rejected = None;
        self.finished = true;
        ClientStep::Complete {
            ack,
            result: LoadResult::Track {
                generation: self.generation,
                request_id: self.request_id,
                track: self.track,
                result,
            },
        }
    }

    fn poison_on_error<T>(&mut self, result: anyhow::Result<T>) -> anyhow::Result<T> {
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
        anyhow::ensure!(self.is_complete(), "incomplete track transfer");
        self.receiver.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::objects::{Metadata, TrackPayload};
    use crate::remote::session::RemoteSession;
    use crate::remote::transport::{Body, DATA_BYTES};
    use crate::session::OpenSpec;

    fn fixture() -> (Arc<dyn Session>, TrackRef, Vec<u8>) {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut writer = vtr::Writer::create(file.path()).unwrap();
        let stream = writer.add_stream(None, "stream", "raw").unwrap();
        let generator = writer.add_generator(stream, "generator").unwrap();
        let tx = writer.begin_tx(generator, 4).unwrap();
        writer.end_tx(tx, 4, vtr::TxStatus::Ok).unwrap();
        writer.close().unwrap();
        let local = OpenSpec::Path(file.path().into()).open().unwrap();
        let track = TrackRef(stream.0);
        let bytes = bincode::serialize(&TrackPayload::from_loaded(
            &local.load_track(track).unwrap(),
        ))
        .unwrap();
        let remote =
            Arc::new(RemoteSession::new(11, Metadata::from_session(local.as_ref())).unwrap());
        (remote, track, bytes)
    }

    fn packet(sequence: u64, body: Body) -> Packet {
        Packet {
            session: 11,
            request: 7,
            sequence,
            body,
        }
    }

    fn before_end(transfer: &mut TrackTransfer, track: TrackRef, bytes: &[u8]) -> u64 {
        assert!(matches!(
            transfer
                .accept(packet(
                    0,
                    Body::Begin {
                        object: ObjectId::Track(track.0),
                        decoded_bytes: bytes.len() as u64
                    }
                ))
                .unwrap(),
            ClientStep::Ack(_)
        ));
        let mut sequence = 1;
        for (i, bytes) in bytes.chunks(DATA_BYTES).enumerate() {
            let packet = packet(
                sequence,
                Body::Data {
                    offset: (i * DATA_BYTES) as u64,
                    bytes: bytes.to_vec(),
                },
            );
            let expected = acknowledgement(&packet);
            let mut step = transfer.accept(packet).unwrap();
            loop {
                match step {
                    ClientStep::Yield => step = transfer.step().unwrap(),
                    ClientStep::Ack(ack) => {
                        assert_eq!(ack, expected);
                        break;
                    }
                    ClientStep::Complete { .. } => panic!("track published before End"),
                }
            }
            sequence += 1;
        }
        assert!(!transfer.is_complete());
        sequence
    }

    #[test]
    fn end_commits_exact_document_ticket_and_stale_response_discards_storage() {
        let (session, track, bytes) = fixture();
        let budget = MemoryBudget::new(4 * 1024 * 1024);
        let mut transfer = TrackTransfer::new(
            7,
            99,
            55,
            session.clone(),
            track,
            1024 * 1024,
            budget.clone(),
        )
        .unwrap();
        let end = before_end(&mut transfer, track, &bytes);
        assert!(budget.used() > 0);
        let mut wrong = packet(end, Body::End);
        wrong.request += 1;
        assert!(transfer.accept(wrong).is_err());
        assert_eq!(budget.used(), 0);
        assert!(transfer.accept(packet(end, Body::End)).is_err());
        assert!(transfer.finish().is_err());

        let mut transfer =
            TrackTransfer::new(7, 99, 56, session, track, 1024 * 1024, budget.clone()).unwrap();
        let end = before_end(&mut transfer, track, &bytes);
        let ClientStep::Complete {
            result:
                LoadResult::Track {
                    generation: 99,
                    request_id: 56,
                    track: returned,
                    result,
                },
            ..
        } = transfer.accept(packet(end, Body::End)).unwrap()
        else {
            panic!("track completion");
        };
        assert_eq!(returned, track);
        let loaded = result.unwrap();
        transfer.finish().unwrap();
        assert!(budget.used() > 0);
        drop(loaded);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn admission_failure_drains_and_completes_as_a_track_error() {
        let (session, track, bytes) = fixture();
        let budget = MemoryBudget::new(0);
        let mut transfer =
            TrackTransfer::new(7, 99, 55, session, track, 1024 * 1024, budget.clone()).unwrap();
        let end = before_end(&mut transfer, track, &bytes);
        let ClientStep::Complete {
            result:
                LoadResult::Track {
                    request_id: 55,
                    result,
                    ..
                },
            ..
        } = transfer.accept(packet(end, Body::End)).unwrap()
        else {
            panic!("track failure");
        };
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("memory budget exceeded")
        );
        assert_eq!(budget.used(), 0);
        transfer.finish().unwrap();
    }
}
