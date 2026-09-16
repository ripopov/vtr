//! One asynchronous Open response. Metadata stays private through decoding and
//! validation; only the matching End produces a document load completion.

use super::ClientStep;
use super::memory::MemoryBudget;
use super::metadata::{MetadataDecoder, MetadataStep, ValidatedMetadata};
use super::transport::{Body, Command, ObjectId, Packet, Receive, Receiver, acknowledgement};
use crate::session::LoadResult;
use std::sync::Arc;

pub struct OpenTransfer {
    request: u64,
    generation: u64,
    limit: u64,
    budget: MemoryBudget,
    session: u64,
    receiver: Option<Receiver>,
    decoder: Option<MetadataDecoder>,
    decoded: Option<Box<ValidatedMetadata>>,
    pending_ack: Option<Packet>,
    finished: bool,
    failed: bool,
}

impl OpenTransfer {
    pub fn new(
        request: u64,
        generation: u64,
        limit: u64,
        budget: MemoryBudget,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(request != 0, "invalid Open request identity");
        Ok(Self {
            request,
            generation,
            limit,
            budget,
            session: 0,
            receiver: None,
            decoder: None,
            decoded: None,
            pending_ack: None,
            finished: false,
            failed: false,
        })
    }

    pub fn command(&self) -> Packet {
        Packet {
            session: 0,
            request: self.request,
            sequence: 0,
            body: Body::Command(Command::Open {
                max_object_bytes: self.limit,
            }),
        }
    }

    pub fn accept(&mut self, packet: Packet) -> anyhow::Result<ClientStep> {
        let result = self.accept_inner(packet);
        self.poison_on_error(result)
    }

    fn accept_inner(&mut self, packet: Packet) -> anyhow::Result<ClientStep> {
        anyhow::ensure!(!self.failed && !self.finished, "Open transfer finished");
        anyhow::ensure!(
            self.pending_ack.is_none(),
            "response before metadata decode finished"
        );
        if self.receiver.is_none() {
            self.receiver = Some(Receiver::new(
                packet.session,
                self.request,
                vec![ObjectId::Metadata],
                self.limit,
            )?);
            self.session = packet.session;
        }
        let ack = acknowledgement(&packet);
        match self.receiver.as_mut().unwrap().accept(packet)? {
            Receive::Begin {
                object: ObjectId::Metadata,
                decoded_bytes,
            } => {
                self.decoder = Some(MetadataDecoder::new(
                    decoded_bytes,
                    self.limit,
                    &self.budget,
                )?);
            }
            Receive::Data(bytes) => {
                self.decoder
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("missing metadata decoder"))?
                    .feed(bytes)?;
                self.pending_ack = Some(ack);
                return self.step_inner();
            }
            Receive::Complete(ObjectId::Metadata) => {
                let decoded = self
                    .decoded
                    .take()
                    .ok_or_else(|| anyhow::anyhow!("metadata validation incomplete"))?;
                let session = Arc::new(decoded.into_session(self.session)?);
                self.finished = true;
                return Ok(ClientStep::Complete {
                    ack,
                    result: LoadResult::Opened {
                        generation: self.generation,
                        result: Ok(session),
                    },
                });
            }
            Receive::Failed {
                object: ObjectId::Metadata,
                message,
            } => {
                self.decoder = None;
                self.decoded = None;
                self.finished = true;
                return Ok(ClientStep::Complete {
                    ack,
                    result: LoadResult::Opened {
                        generation: self.generation,
                        result: Err(anyhow::anyhow!(message)),
                    },
                });
            }
            _ => anyhow::bail!("unexpected Open object"),
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
            "no pending metadata decode"
        );
        match self
            .decoder
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("missing metadata decoder"))?
            .step()?
        {
            MetadataStep::Yield => return Ok(ClientStep::Yield),
            MetadataStep::NeedInput => {}
            MetadataStep::Decoded(decoded) => {
                self.decoded = Some(decoded);
                self.decoder = None;
            }
        }
        Ok(ClientStep::Ack(self.pending_ack.take().unwrap()))
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
        anyhow::ensure!(self.is_complete(), "incomplete Open transfer");
        self.receiver.unwrap().finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::objects::Metadata;
    use crate::remote::transport::DATA_BYTES;
    use crate::session::OpenSpec;

    fn packet(sequence: u64, body: Body) -> Packet {
        Packet {
            session: 31,
            request: 7,
            sequence,
            body,
        }
    }

    fn decode_without_end(transfer: &mut OpenTransfer) -> u64 {
        let local = OpenSpec::Synthetic(100).open().unwrap();
        let bytes = bincode::serialize(&Metadata::from_session(local.as_ref())).unwrap();
        assert!(matches!(
            transfer
                .accept(packet(
                    0,
                    Body::Begin {
                        object: ObjectId::Metadata,
                        decoded_bytes: bytes.len() as u64
                    }
                ))
                .unwrap(),
            ClientStep::Ack(_)
        ));
        let mut sequence = 1;
        for (i, chunk) in bytes.chunks(DATA_BYTES).enumerate() {
            let response = packet(
                sequence,
                Body::Data {
                    offset: (i * DATA_BYTES) as u64,
                    bytes: chunk.to_vec(),
                },
            );
            let expected = acknowledgement(&response);
            let mut step = transfer.accept(response).unwrap();
            loop {
                match step {
                    ClientStep::Ack(ack) => {
                        assert_eq!(ack, expected);
                        break;
                    }
                    ClientStep::Yield => step = transfer.step().unwrap(),
                    ClientStep::Complete { .. } => panic!("published before End"),
                }
            }
            sequence += 1;
        }
        assert!(!transfer.is_complete());
        sequence
    }

    #[test]
    fn metadata_is_private_until_end_and_reservation_follows_shared_session() {
        let budget = MemoryBudget::new(4 * 1024 * 1024);
        let mut transfer = OpenTransfer::new(7, 99, 1024 * 1024, budget.clone()).unwrap();
        decode_without_end(&mut transfer);
        assert!(budget.used() > 0);
        assert!(transfer.finish().is_err());
        assert_eq!(budget.used(), 0);

        let mut transfer = OpenTransfer::new(7, 99, 1024 * 1024, budget.clone()).unwrap();
        let sequence = decode_without_end(&mut transfer);
        let ClientStep::Complete {
            result: LoadResult::Opened { generation, result },
            ..
        } = transfer.accept(packet(sequence, Body::End)).unwrap()
        else {
            panic!("Open completion");
        };
        assert_eq!(generation, 99);
        let session = result.unwrap();
        let shared = Arc::clone(&session);
        assert_eq!(session.remote_id(), Some(31));
        transfer.finish().unwrap();
        drop(session);
        assert!(budget.used() > 0);
        drop(shared);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn wrong_identity_discards_private_metadata_and_prevents_reuse() {
        let budget = MemoryBudget::new(4 * 1024 * 1024);
        let mut transfer = OpenTransfer::new(7, 99, 1024 * 1024, budget.clone()).unwrap();
        let sequence = decode_without_end(&mut transfer);
        let mut wrong = packet(sequence, Body::End);
        wrong.request += 1;
        assert!(transfer.accept(wrong).is_err());
        assert_eq!(budget.used(), 0);
        assert!(transfer.accept(packet(sequence, Body::End)).is_err());
        assert!(transfer.finish().is_err());
    }
}
