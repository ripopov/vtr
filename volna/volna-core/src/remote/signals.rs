//! Asynchronous complete-history installation. The host handles transport and
//! yields between `step` calls; document results use the ordinary local seam.

use super::ClientStep;
use super::history::stream::HistoryDecoder;
use super::memory::MemoryBudget;
use super::transport::{ObjectId, Packet, Receive, Receiver, acknowledgement};
use crate::data::SignalRef;
use crate::session::LoadResult;
use std::sync::Arc;

pub struct SignalTransfer {
    receiver: Receiver,
    generation: u64,
    limit: u64,
    budget: MemoryBudget,
    builder: Option<(SignalRef, HistoryDecoder)>,
    rejected: Option<(SignalRef, String)>,
    end_ack: Option<Packet>,
    failed: bool,
}

impl SignalTransfer {
    pub fn new(
        session: u64,
        request: u64,
        generation: u64,
        signals: &[SignalRef],
        max_object_bytes: u64,
        budget: MemoryBudget,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            receiver: Receiver::new(
                session,
                request,
                signals.iter().map(|id| ObjectId::Signal(id.0)).collect(),
                max_object_bytes,
            )?,
            generation,
            limit: max_object_bytes,
            budget,
            builder: None,
            rejected: None,
            end_ack: None,
            failed: false,
        })
    }

    pub fn accept(&mut self, packet: Packet) -> anyhow::Result<ClientStep> {
        let result = self.accept_inner(packet);
        self.poison_on_error(result)
    }

    fn accept_inner(&mut self, packet: Packet) -> anyhow::Result<ClientStep> {
        anyhow::ensure!(!self.failed, "signal transfer failed");
        anyhow::ensure!(
            self.end_ack.is_none(),
            "response before validation finished"
        );
        let ack = acknowledgement(&packet);
        match self.receiver.accept(packet)? {
            Receive::Begin {
                object: ObjectId::Signal(id),
                decoded_bytes,
            } => match HistoryDecoder::with_budget(decoded_bytes, self.limit, &self.budget) {
                Ok(builder) => self.builder = Some((SignalRef(id), builder)),
                Err(error) => self.rejected = Some((SignalRef(id), format!("{error:#}"))),
            },
            Receive::Data(bytes) => {
                // Drain a refused object with normal backpressure so remaining
                // signals in the batch still complete. No payload is retained.
                if self.rejected.is_none() {
                    self.builder
                        .as_mut()
                        .ok_or_else(|| anyhow::anyhow!("missing signal builder"))?
                        .1
                        .feed(&bytes)?;
                }
            }
            Receive::Complete(ObjectId::Signal(_)) => {
                if let Some((id, message)) = self.rejected.take() {
                    return Ok(self.failed(ack, id, message));
                }
                self.end_ack = Some(ack);
                return self.step_inner();
            }
            Receive::Failed {
                object: ObjectId::Signal(id),
                message,
            } => {
                self.builder = None;
                self.rejected = None;
                return Ok(self.failed(ack, SignalRef(id), message));
            }
            _ => anyhow::bail!("unexpected signal object"),
        }
        Ok(ClientStep::Ack(ack))
    }

    /// Performs at most one bounded history validation step.
    pub fn step(&mut self) -> anyhow::Result<ClientStep> {
        let result = self.step_inner();
        self.poison_on_error(result)
    }

    fn step_inner(&mut self) -> anyhow::Result<ClientStep> {
        anyhow::ensure!(
            !self.failed && self.end_ack.is_some(),
            "no pending signal validation"
        );
        let (_, builder) = self
            .builder
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("missing signal builder"))?;
        if !builder.step()? {
            return Ok(ClientStep::Yield);
        }
        let (id, builder) = self.builder.take().unwrap();
        let history = builder.finish()?;
        Ok(ClientStep::Complete {
            ack: self.end_ack.take().unwrap(),
            result: LoadResult::Signals {
                generation: self.generation,
                results: vec![(id, Ok(Arc::new(history)))],
            },
        })
    }

    fn failed(&self, ack: Packet, id: SignalRef, message: String) -> ClientStep {
        ClientStep::Complete {
            ack,
            result: LoadResult::Signals {
                generation: self.generation,
                results: vec![(id, Err(anyhow::anyhow!(message)))],
            },
        }
    }

    fn poison_on_error<T>(&mut self, result: anyhow::Result<T>) -> anyhow::Result<T> {
        if result.is_err() {
            self.failed = true;
            self.builder = None;
            self.rejected = None;
            self.end_ack = None;
        }
        result
    }

    pub fn is_complete(&self) -> bool {
        !self.failed && self.end_ack.is_none() && self.receiver.is_complete()
    }

    /// A disconnect is successful only after every requested result was delivered.
    pub fn finish(self) -> anyhow::Result<()> {
        anyhow::ensure!(self.is_complete(), "incomplete signal transfer");
        self.receiver.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::history::VecHistory;
    use crate::data::{SignalShape, WaveValue};
    use crate::remote::history::PackedHistory;
    use crate::remote::transport::{Body, DATA_BYTES};

    fn packet(sequence: u64, body: Body) -> Packet {
        Packet {
            session: 5,
            request: 8,
            sequence,
            body,
        }
    }

    #[test]
    fn large_value_yields_before_ack_and_atomic_completion() {
        let source = VecHistory {
            shape: SignalShape::Text,
            times: vec![1],
            initial: WaveValue::Unavailable,
            values: vec![WaveValue::Text("λ".repeat(DATA_BYTES))],
        };
        let packed = PackedHistory::from_history(&source).unwrap();
        let bytes = bincode::serialize(&packed).unwrap();
        let budget = MemoryBudget::new(bytes.len() as u64 + 1024);
        let mut transfer = SignalTransfer::new(
            5,
            8,
            99,
            &[SignalRef(3)],
            bytes.len() as u64,
            budget.clone(),
        )
        .unwrap();
        assert!(matches!(
            transfer
                .accept(packet(
                    0,
                    Body::Begin {
                        object: ObjectId::Signal(3),
                        decoded_bytes: bytes.len() as u64,
                    }
                ))
                .unwrap(),
            ClientStep::Ack(_)
        ));
        let mut sequence = 1;
        for (i, chunk) in bytes.chunks(DATA_BYTES).enumerate() {
            assert!(matches!(
                transfer
                    .accept(packet(
                        sequence,
                        Body::Data {
                            offset: (i * DATA_BYTES) as u64,
                            bytes: chunk.to_vec(),
                        }
                    ))
                    .unwrap(),
                ClientStep::Ack(_)
            ));
            sequence += 1;
        }
        assert!(!transfer.is_complete());
        assert!(matches!(
            transfer.accept(packet(sequence, Body::End)).unwrap(),
            ClientStep::Yield
        ));
        assert!(!transfer.is_complete());
        let completed = loop {
            match transfer.step().unwrap() {
                ClientStep::Yield => continue,
                ClientStep::Complete { ack, result } => {
                    assert_eq!(ack, acknowledgement(&packet(sequence, Body::End)));
                    break result;
                }
                ClientStep::Ack(_) => panic!("ACK before completion"),
            }
        };
        let LoadResult::Signals {
            generation,
            mut results,
        } = completed
        else {
            panic!("signal result");
        };
        assert_eq!(generation, 99);
        let (id, history) = results.pop().unwrap();
        assert_eq!(id, SignalRef(3));
        let history = history.unwrap();
        assert_eq!(history.value(Some(0)), source.values[0]);
        let shared = Arc::clone(&history);
        assert!(transfer.is_complete());
        transfer.finish().unwrap();
        assert!(budget.used() >= bytes.len() as u64);
        assert!(budget.reserve(bytes.len() as u64).is_err());
        drop(history);
        assert!(budget.used() > 0);
        drop(shared);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn per_item_failure_completes_but_wrong_identity_poisons_transfer() {
        let mut transfer =
            SignalTransfer::new(5, 8, 99, &[SignalRef(3)], 1024, MemoryBudget::new(4096)).unwrap();
        let error = packet(
            0,
            Body::Error {
                object: ObjectId::Signal(3),
                message: "too large".into(),
            },
        );
        let ClientStep::Complete {
            result: LoadResult::Signals { results, .. },
            ..
        } = transfer.accept(error.clone()).unwrap()
        else {
            panic!("per-item failure");
        };
        assert!(results[0].1.is_err());
        transfer.finish().unwrap();

        let mut transfer =
            SignalTransfer::new(5, 8, 99, &[SignalRef(3)], 1024, MemoryBudget::new(4096)).unwrap();
        let mut wrong = error.clone();
        wrong.request += 1;
        assert!(transfer.accept(wrong).is_err());
        assert!(transfer.accept(error).is_err());
        assert!(transfer.finish().is_err());
    }

    #[test]
    fn failed_or_abandoned_assembly_releases_its_reservation() {
        let budget = MemoryBudget::new(4096);
        let begin = packet(
            0,
            Body::Begin {
                object: ObjectId::Signal(3),
                decoded_bytes: 30,
            },
        );
        let mut transfer =
            SignalTransfer::new(5, 8, 99, &[SignalRef(3)], 1024, budget.clone()).unwrap();
        transfer.accept(begin.clone()).unwrap();
        assert!(budget.used() >= 30);
        drop(transfer);
        assert_eq!(budget.used(), 0);

        let mut transfer =
            SignalTransfer::new(5, 8, 99, &[SignalRef(3)], 1024, budget.clone()).unwrap();
        transfer.accept(begin).unwrap();
        assert!(transfer.accept(packet(1, Body::End)).is_err());
        assert_eq!(budget.used(), 0);

        let mut transfer = SignalTransfer::new(
            5,
            8,
            99,
            &[SignalRef(3), SignalRef(4)],
            8192,
            budget.clone(),
        )
        .unwrap();
        transfer
            .accept(packet(
                0,
                Body::Begin {
                    object: ObjectId::Signal(3),
                    decoded_bytes: 8192,
                },
            ))
            .unwrap();
        assert_eq!(budget.used(), 0);
        transfer
            .accept(packet(
                1,
                Body::Data {
                    offset: 0,
                    bytes: vec![0; 8192],
                },
            ))
            .unwrap();
        let ClientStep::Complete {
            result: LoadResult::Signals { results, .. },
            ..
        } = transfer.accept(packet(2, Body::End)).unwrap()
        else {
            panic!("expected refused object completion");
        };
        assert!(
            results[0]
                .1
                .as_ref()
                .err()
                .unwrap()
                .to_string()
                .contains("memory budget exceeded")
        );
        assert_eq!(budget.used(), 0);
        assert!(!transfer.is_complete());
        let source = VecHistory {
            shape: SignalShape::Bit,
            times: vec![],
            values: vec![],
            initial: WaveValue::Unavailable,
        };
        let bytes = bincode::serialize(&PackedHistory::from_history(&source).unwrap()).unwrap();
        transfer
            .accept(packet(
                3,
                Body::Begin {
                    object: ObjectId::Signal(4),
                    decoded_bytes: bytes.len() as u64,
                },
            ))
            .unwrap();
        transfer
            .accept(packet(4, Body::Data { offset: 0, bytes }))
            .unwrap();
        let ClientStep::Complete {
            result: LoadResult::Signals { results, .. },
            ..
        } = transfer.accept(packet(5, Body::End)).unwrap()
        else {
            panic!("expected next signal success");
        };
        assert!(results[0].1.is_ok());
        transfer.finish().unwrap();
        assert!(budget.used() > 0);
        drop(results);
        assert_eq!(budget.used(), 0);
    }
}
