//! Cooperative decoding of complete transaction payloads. Records and their
//! typed details use the same fixed bincode schema as the headless service.

use super::decode::{Decoder, Reader, Step};
use super::memory::MemoryBudget;
use super::metadata::{attribute, attributes};
use super::objects::{GeneratorPayload, TrackPayload};
use crate::data::loaded_tracks::LoadedTrack;
use crate::data::loaded_tracks::{LoadedRelation, TransactionLocation};
use crate::data::transactions::*;
use crate::session::Session;
use std::sync::Arc;

pub struct TrackDecoder(Decoder<LoadedTrack>);

pub enum TrackStep {
    NeedInput,
    Yield,
    /// Validated/indexed, but still private until the protocol End. Each
    /// generator owns the reservation for its decoded storage and indexes.
    Decoded(LoadedTrack),
}

impl TrackDecoder {
    pub fn new(
        declared: u64,
        limit: u64,
        budget: &MemoryBudget,
        session: Arc<dyn Session>,
    ) -> anyhow::Result<Self> {
        Ok(Self(Decoder::new(
            declared,
            limit,
            budget,
            move |reader| {
                Box::pin(async move {
                    let payload = payload(&reader).await?;
                    let mut workspace = session
                        .tracks()
                        .len()
                        .checked_mul(128)
                        .ok_or_else(|| anyhow::anyhow!("track workspace size overflow"))?;
                    for generator in &payload.generators {
                        reader.checkpoint().await;
                        workspace = generator
                            .transactions
                            .len()
                            .checked_add(generator.relations.len())
                            .and_then(|n| n.checked_mul(128))
                            .and_then(|n| n.checked_add(workspace))
                            .ok_or_else(|| anyhow::anyhow!("track workspace size overflow"))?;
                    }
                    reader.charge(workspace)?;
                    payload
                        .into_loaded_with(session.tracks(), || reader.checkpoint())
                        .await
                })
            },
        )?))
    }
    pub fn feed(&mut self, bytes: Vec<u8>) -> anyhow::Result<()> {
        self.0.feed(bytes)
    }
    pub fn step(&mut self) -> anyhow::Result<TrackStep> {
        Ok(match self.0.step()? {
            Step::NeedInput => TrackStep::NeedInput,
            Step::Yield => TrackStep::Yield,
            Step::Ready(track, scratch) => {
                drop(scratch);
                TrackStep::Decoded(track)
            }
        })
    }
}

async fn payload(r: &Reader) -> anyhow::Result<TrackPayload> {
    Ok(TrackPayload {
        track: TrackRef(r.u32().await?),
        generators: r
            .vector(28, || async {
                let before = r.charged();
                let mut generator = GeneratorPayload {
                    reservation: None,
                    generator: TrackRef(r.u32().await?),
                    transactions: r.vector(55, || transaction(r)).await?,
                    parents: r
                        .vector(20, || async {
                            Ok((
                                TransactionRef(r.u64().await?),
                                TransactionLocation {
                                    transaction: TransactionRef(r.u64().await?),
                                    generator: TrackRef(r.u32().await?),
                                },
                            ))
                        })
                        .await?,
                    relations: r
                        .vector(48, || async {
                            Ok(LoadedRelation {
                                id: r.u64().await?,
                                from_generator: TrackRef(r.u32().await?),
                                to_generator: TrackRef(r.u32().await?),
                                relation: Relation {
                                    kind: r.string().await?,
                                    from: TransactionRef(r.u64().await?),
                                    to: TransactionRef(r.u64().await?),
                                    attributes: attributes(r, 0).await?,
                                },
                            })
                        })
                        .await?,
                };
                // Conservative storage for ID/parent maps, relation identity
                // checks, and a power-of-two interval tree. Reserve before any
                // index construction, alongside the already charged raw fields.
                let index_bytes = generator
                    .transactions
                    .len()
                    .checked_mul(128)
                    .and_then(|n| {
                        generator
                            .parents
                            .len()
                            .checked_mul(64)
                            .and_then(|p| n.checked_add(p))
                    })
                    .and_then(|n| {
                        generator
                            .relations
                            .len()
                            .checked_mul(64)
                            .and_then(|p| n.checked_add(p))
                    })
                    .and_then(|n| {
                        n.checked_add(
                            std::mem::size_of::<crate::data::loaded_tracks::LoadedGenerator>() + 64,
                        )
                    })
                    .ok_or_else(|| anyhow::anyhow!("track index size overflow"))?;
                r.charge(index_bytes)?;
                generator.reservation = Some(r.take_since(before)?);
                Ok(generator)
            })
            .await?,
    })
}

async fn transaction(r: &Reader) -> anyhow::Result<Transaction> {
    Ok(Transaction {
        id: TransactionRef(r.u64().await?),
        generator: TrackRef(r.u32().await?),
        begin: r.u64().await?,
        end: r.u64().await?,
        status: {
            let code = r.u8().await?;
            anyhow::ensure!(code <= 4, "unknown transaction status");
            TxStatus::from_u8(code)
        },
        kind: {
            let code = r.u8().await?;
            anyhow::ensure!(code <= 5, "unknown transaction kind");
            TxKind::from_u8(code)
        },
        parent: if r.boolean().await? {
            Some(TransactionRef(r.u64().await?))
        } else {
            None
        },
        attributes: r
            .vector(13, || async {
                Ok(TransactionAttribute {
                    key: r.string().await?,
                    phase: {
                        let code = r.u8().await?;
                        anyhow::ensure!(code <= 2, "unknown attribute phase");
                        AttrPhase::from_u8(code)
                    },
                    value: attribute(r, 0).await?,
                })
            })
            .await?,
        events: r
            .vector(24, || async {
                Ok(TransactionEvent {
                    time: r.u64().await?,
                    name: r.string().await?,
                    attributes: attributes(r, 0).await?,
                })
            })
            .await?,
        stages: r
            .vector(33, || async {
                Ok(TransactionStage {
                    name: r.string().await?,
                    lane: r.string().await?,
                    begin: r.u64().await?,
                    end: if r.boolean().await? {
                        Some(r.u64().await?)
                    } else {
                        None
                    },
                    attributes: attributes(r, 0).await?,
                })
            })
            .await?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::transport::DATA_BYTES;

    fn sample() -> TrackPayload {
        let transactions = (0..30)
            .map(|id| Transaction {
                id: TransactionRef(id),
                generator: TrackRef(1),
                begin: id,
                end: if id == 0 { 10000 } else { id },
                status: TxStatus::from_u8((id % 5) as u8),
                kind: TxKind::from_u8((id % 6) as u8),
                parent: (id > 0).then_some(TransactionRef(0)),
                attributes: (0..3)
                    .map(|phase| TransactionAttribute {
                        key: "phase".into(),
                        phase: AttrPhase::from_u8(phase),
                        value: AttributeValue::F64(f64::NAN),
                    })
                    .collect(),
                events: vec![TransactionEvent {
                    time: id,
                    name: "event".into(),
                    attributes: vec![("bytes".into(), AttributeValue::Bytes(vec![0, 255]))],
                }],
                stages: vec![TransactionStage {
                    name: "stage".into(),
                    lane: "raw lane".into(),
                    begin: id,
                    end: (id % 2 == 0).then_some(id),
                    attributes: vec![(
                        "text".into(),
                        AttributeValue::Text(if id == 0 {
                            "λ𐀀".repeat(DATA_BYTES / 3)
                        } else {
                            "label".into()
                        }),
                    )],
                }],
            })
            .collect();
        TrackPayload {
            track: TrackRef(0),
            generators: vec![
                GeneratorPayload {
                    reservation: None,
                    generator: TrackRef(1),
                    transactions,
                    parents: (1..30)
                        .map(|id| {
                            (
                                TransactionRef(id),
                                TransactionLocation {
                                    transaction: TransactionRef(0),
                                    generator: TrackRef(1),
                                },
                            )
                        })
                        .collect(),
                    relations: (0..2)
                        .map(|id| LoadedRelation {
                            id,
                            from_generator: TrackRef(1),
                            to_generator: TrackRef(3),
                            relation: Relation {
                                kind: "parallel".into(),
                                from: TransactionRef(0),
                                to: TransactionRef(100),
                                attributes: vec![("real".into(), AttributeValue::F64(f64::NAN))],
                            },
                        })
                        .collect(),
                },
                GeneratorPayload {
                    reservation: None,
                    generator: TrackRef(2),
                    transactions: vec![],
                    parents: vec![],
                    relations: vec![],
                },
            ],
        }
    }

    fn session() -> Arc<dyn Session> {
        let catalog: Vec<_> = (0..4)
            .map(|id| Track {
                id: TrackRef(id),
                path: vec![id.to_string()],
                kind: if id == 0 {
                    TrackKind::Stream { kind: "raw".into() }
                } else {
                    TrackKind::Generator {
                        stream: TrackRef(0),
                    }
                },
                attributes: vec![],
            })
            .collect();
        // The external generator must not be included in this stream load.
        let mut catalog = catalog;
        catalog[3].kind = TrackKind::Generator {
            stream: TrackRef(4),
        };
        catalog.push(Track {
            id: TrackRef(4),
            path: vec!["external".into()],
            kind: TrackKind::Stream { kind: "raw".into() },
            attributes: vec![],
        });

        let local = crate::session::OpenSpec::Synthetic(1).open().unwrap();
        let mut metadata = crate::remote::objects::Metadata::from_session(local.as_ref());
        metadata.tracks = catalog;
        metadata.capabilities.transactions = true;
        Arc::new(crate::remote::session::RemoteSession::new(11, metadata).unwrap())
    }

    #[test]
    fn full_transaction_details_roundtrip_with_chunked_large_values() {
        let bytes = bincode::serialize(&sample()).unwrap();
        for size in [1, 7, DATA_BYTES] {
            let budget = MemoryBudget::new(8 * 1024 * 1024);
            let mut decoder =
                TrackDecoder::new(bytes.len() as u64, bytes.len() as u64, &budget, session())
                    .unwrap();
            let mut chunks = bytes.chunks(size);
            let loaded = loop {
                match decoder.step().unwrap() {
                    TrackStep::NeedInput => decoder
                        .feed(chunks.next().expect("remaining bytes").to_vec())
                        .unwrap(),
                    TrackStep::Yield => {}
                    TrackStep::Decoded(loaded) => break loaded,
                }
            };
            assert_eq!(
                bincode::serialize(&TrackPayload::from_loaded(&loaded)).unwrap(),
                bytes
            );
            assert!(chunks.next().is_none());
            assert_eq!(loaded.generators.len(), 2);
            let mut overlap = Vec::new();
            loaded.generators[0]
                .visit_window(20, 20, |tx| {
                    overlap.push(tx.id);
                    true
                })
                .unwrap();
            assert_eq!(overlap, [TransactionRef(0), TransactionRef(20)]);
            assert!(loaded.generators[1].transactions().is_empty());
            assert_eq!(loaded.generators[0].relations().len(), 2);
            let total = budget.used();
            assert!(total > 0);
            let retained_empty = Arc::clone(&loaded.generators[1]);
            drop(loaded);
            assert!(budget.used() > 0 && budget.used() < total);
            drop(retained_empty);
            assert_eq!(budget.used(), 0);
        }
    }

    #[test]
    fn invalid_status_truncation_and_oversized_collections_are_rejected() {
        let valid = bincode::serialize(&sample()).unwrap();
        let mut bad_status = valid.clone();
        bad_status[52] = 255;
        let mut bad_count = valid.clone();
        bad_count[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
        for bytes in [bad_status, bad_count, valid[..valid.len() - 1].to_vec()] {
            let budget = MemoryBudget::new(8 * 1024 * 1024);
            let mut decoder =
                TrackDecoder::new(bytes.len() as u64, bytes.len() as u64, &budget, session())
                    .unwrap();
            let mut chunks = bytes.chunks(DATA_BYTES);
            loop {
                match decoder.step() {
                    Err(_) => break,
                    Ok(TrackStep::NeedInput) => decoder
                        .feed(chunks.next().expect("complete malformed input").to_vec())
                        .unwrap(),
                    Ok(TrackStep::Yield) => {}
                    Ok(TrackStep::Decoded(_)) => panic!("accepted invalid track"),
                }
            }
            drop(decoder);
            assert_eq!(budget.used(), 0);
        }
    }
}
