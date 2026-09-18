//! Cooperative decoding and validation of raw metadata. Publication still
//! requires the protocol's explicit End.

use super::decode::{Decoder, Reader, Step};
use super::memory::{MemoryBudget, Reservation};
use super::objects::Metadata;
use crate::data::source::{Direction, Scope, Variable};
use crate::data::transactions::{AttributeValue, Attributes, Track, TrackKind, TrackRef};
use crate::data::{Hierarchy, SignalRef, SignalShape, TraceInfo};
use crate::session::Capabilities;
use std::future::Future;
use std::pin::Pin;

pub struct MetadataDecoder(Decoder<Metadata>);

pub struct ValidatedMetadata {
    pub(super) metadata: Metadata,
    pub(super) reservation: Reservation,
}

pub enum MetadataStep {
    NeedInput,
    Yield,
    Decoded(Box<ValidatedMetadata>),
}

impl MetadataDecoder {
    pub fn new(declared: u64, limit: u64, budget: &MemoryBudget) -> anyhow::Result<Self> {
        Ok(Self(Decoder::new(declared, limit, budget, |reader| {
            Box::pin(async move { metadata(&reader).await })
        })?))
    }
    pub fn feed(&mut self, bytes: Vec<u8>) -> anyhow::Result<()> {
        self.0.feed(bytes)
    }
    pub fn step(&mut self) -> anyhow::Result<MetadataStep> {
        Ok(match self.0.step()? {
            Step::NeedInput => MetadataStep::NeedInput,
            Step::Yield => MetadataStep::Yield,
            Step::Ready(metadata, reservation) => {
                MetadataStep::Decoded(Box::new(ValidatedMetadata {
                    metadata,
                    reservation,
                }))
            }
        })
    }
}

async fn metadata(r: &Reader) -> anyhow::Result<Metadata> {
    let info = TraceInfo {
        name: r.string().await?,
        design_id: if r.boolean().await? {
            Some(r.string().await?)
        } else {
            None
        },
        timescale: r.u8().await? as i8,
        time_range: (r.u64().await?, r.u64().await?),
        signal_count: r.usize().await?,
        change_count: if r.boolean().await? {
            Some(r.u64().await?)
        } else {
            None
        },
    };
    let scopes = r
        .vector(33, || async {
            Ok(Scope {
                name: r.string().await?,
                kind: r.string().await?,
                parent: if r.boolean().await? {
                    Some(r.usize().await?)
                } else {
                    None
                },
                children: r.vector(8, || r.usize()).await?,
                vars: r.vector(8, || r.usize()).await?,
            })
        })
        .await?;
    let roots = r.vector(8, || r.usize()).await?;
    let vars = r
        .vector(36, || async {
            Ok(Variable {
                name: r.string().await?,
                scope: r.usize().await?,
                shape: shape(r).await?,
                var_type: r.string().await?,
                direction: match r.u32().await? {
                    0 => Direction::None,
                    1 => Direction::Input,
                    2 => Direction::Output,
                    3 => Direction::InOut,
                    _ => anyhow::bail!("invalid signal direction"),
                },
                signal: SignalRef(r.u32().await?),
            })
        })
        .await?;
    let capabilities = Capabilities {
        waveforms: r.boolean().await?,
        transactions: r.boolean().await?,
        relations: r.boolean().await?,
    };
    let tracks = r.vector(24, || track(r)).await?;
    let metadata = Metadata {
        info,
        hierarchy: Hierarchy {
            scopes,
            roots,
            vars,
        },
        capabilities,
        tracks,
    };
    // Reserve conservative validation workspace before constructing hash tables
    // or traversal stacks. Include explicit child lists, even in malformed trees.
    let mut items = metadata
        .hierarchy
        .scopes
        .len()
        .checked_add(metadata.hierarchy.vars.len())
        .and_then(|n| n.checked_add(metadata.hierarchy.roots.len()))
        .and_then(|n| n.checked_add(metadata.tracks.len()))
        .ok_or_else(|| anyhow::anyhow!("metadata workspace size overflow"))?;
    for scope in &metadata.hierarchy.scopes {
        r.checkpoint().await;
        items = items
            .checked_add(scope.children.len())
            .ok_or_else(|| anyhow::anyhow!("metadata workspace size overflow"))?;
    }
    r.charge(
        items
            .checked_mul(128)
            .ok_or_else(|| anyhow::anyhow!("metadata workspace size overflow"))?,
    )?;
    metadata.validate_with(|| r.checkpoint()).await?;
    Ok(metadata)
}

async fn shape(r: &Reader) -> anyhow::Result<SignalShape> {
    Ok(match r.u32().await? {
        0 => SignalShape::Event,
        1 => SignalShape::Bit,
        2 => SignalShape::Vector {
            width: r.u32().await?,
        },
        3 => SignalShape::Real,
        4 => SignalShape::Text,
        _ => anyhow::bail!("invalid signal shape"),
    })
}

async fn track(r: &Reader) -> anyhow::Result<Track> {
    Ok(Track {
        id: TrackRef(r.u32().await?),
        path: r.vector(8, || r.string()).await?,
        kind: match r.u32().await? {
            0 => TrackKind::Stream {
                kind: r.string().await?,
            },
            1 => TrackKind::Generator {
                stream: TrackRef(r.u32().await?),
            },
            _ => anyhow::bail!("invalid track kind"),
        },
        attributes: attributes(r, 0).await?,
    })
}

pub(super) async fn attributes(r: &Reader, depth: usize) -> anyhow::Result<Attributes> {
    r.vector(12, || async {
        Ok((r.string().await?, attribute(r, depth).await?))
    })
    .await
}

pub(super) fn attribute(
    r: &Reader,
    depth: usize,
) -> Pin<Box<dyn Future<Output = anyhow::Result<AttributeValue>> + Send + '_>> {
    Box::pin(async move {
        anyhow::ensure!(depth < 128, "attribute nesting exceeds client limit");
        use AttributeValue::*;
        Ok(match r.u32().await? {
            0 => Null,
            1 => Bool(r.boolean().await?),
            2 => I64(r.u64().await? as i64),
            3 => U64(r.u64().await?),
            4 => F64(f64::from_bits(r.u64().await?)),
            5 => Text(r.string().await?),
            6 => Bytes(r.bytes().await?),
            7 => Logic {
                width: r.u32().await?,
                states: r.u8().await?,
                data: r.bytes().await?,
            },
            8 => Time(r.u64().await?),
            9 => Enum {
                value: r.u64().await? as i64,
                name: r.string().await?,
            },
            10 => Pointer(r.u64().await?),
            11 => Fixed {
                raw: r.u64().await? as i64,
                scale: r.u32().await? as i32,
            },
            12 => UFixed {
                raw: r.u64().await?,
                scale: r.u32().await? as i32,
            },
            13 => List(r.vector(4, || attribute(r, depth + 1)).await?),
            14 => Map(attributes(r, depth + 1).await?),
            _ => anyhow::bail!("invalid attribute kind"),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::transport::DATA_BYTES;
    use crate::session::OpenSpec;

    fn sample() -> Metadata {
        let source = OpenSpec::Synthetic(100).open().unwrap();
        let mut metadata = Metadata::from_session(source.as_ref());
        metadata.info.design_id = Some("build λ".into());
        use AttributeValue::*;
        let values = vec![
            Null,
            Bool(true),
            I64(-7),
            U64(u64::MAX),
            F64(f64::NAN),
            Text("aλ𐀀".repeat(DATA_BYTES / 4)),
            Bytes(vec![0, 255]),
            Logic {
                width: 2,
                states: 9,
                data: vec![0x87],
            },
            Time(17),
            Enum {
                value: -2,
                name: "enum".into(),
            },
            Pointer(u64::MAX),
            Fixed { raw: -3, scale: -9 },
            UFixed { raw: 3, scale: -9 },
            List(vec![Bool(false)]),
            Map(vec![("key".into(), I64(4))]),
        ];
        metadata.tracks = vec![Track {
            id: TrackRef(0),
            path: vec!["top".into()],
            kind: TrackKind::Stream { kind: "raw".into() },
            attributes: values
                .into_iter()
                .map(|value| ("attr".into(), value))
                .collect(),
        }];
        metadata.capabilities.transactions = true;
        metadata
    }

    #[test]
    fn metadata_roundtrip_matches_bincode_across_fragmented_utf8_and_all_attributes() {
        let expected = sample();
        expected.validate().unwrap();
        let bytes = bincode::serialize(&expected).unwrap();
        for size in [1, 7, DATA_BYTES] {
            let budget = MemoryBudget::new(16 * 1024 * 1024);
            let mut decoder =
                MetadataDecoder::new(bytes.len() as u64, bytes.len() as u64, &budget).unwrap();
            let mut chunks = bytes.chunks(size);
            let (metadata, reservation) = loop {
                match decoder.step().unwrap() {
                    MetadataStep::NeedInput => decoder
                        .feed(chunks.next().expect("input remaining").to_vec())
                        .unwrap(),
                    MetadataStep::Yield => {}
                    MetadataStep::Decoded(decoded) => {
                        break (decoded.metadata, decoded.reservation);
                    }
                }
            };
            assert!(chunks.next().is_none());
            assert_eq!(bincode::serialize(&metadata).unwrap(), bytes);
            metadata.validate().unwrap();
            assert!(budget.used() > 0);
            drop(metadata);
            drop(reservation);
            assert_eq!(budget.used(), 0);
        }
    }

    #[test]
    fn declared_lengths_cannot_allocate_past_remaining_input_or_memory_budget() {
        for length in [u64::MAX, 8192] {
            let budget = MemoryBudget::new(1024 * 1024);
            let declared = if length == u64::MAX { 8 } else { 8200 };
            let mut decoder = MetadataDecoder::new(declared, declared, &budget).unwrap();
            // Hold the rest of the budget so the first name cannot allocate.
            let held = budget.reserve(1024 * 1024 - budget.used()).unwrap();
            decoder.feed(length.to_le_bytes().to_vec()).unwrap();
            assert!(decoder.step().is_err());
            drop(decoder);
            drop(held);
            assert_eq!(budget.used(), 0);
        }
    }

    #[test]
    fn large_catalog_yields_even_when_all_input_is_available() {
        let mut metadata = sample();
        metadata.tracks[0].attributes.clear();
        let template = metadata.tracks[0].clone();
        metadata.tracks = (0..4000)
            .map(|id| Track {
                id: TrackRef(id),
                ..template.clone()
            })
            .collect();
        let bytes = bincode::serialize(&metadata).unwrap();
        assert!(bytes.len() < DATA_BYTES);
        let budget = MemoryBudget::new(16 * 1024 * 1024);
        let mut decoder =
            MetadataDecoder::new(bytes.len() as u64, bytes.len() as u64, &budget).unwrap();
        decoder.feed(bytes).unwrap();
        assert!(matches!(decoder.step().unwrap(), MetadataStep::Yield));
        let mut yields = 1;
        loop {
            match decoder.step().unwrap() {
                MetadataStep::Yield => yields += 1,
                MetadataStep::NeedInput => panic!("all input supplied"),
                MetadataStep::Decoded(decoded) => {
                    let metadata = decoded.metadata;
                    assert_eq!(metadata.tracks.len(), 4000);
                    break;
                }
            }
        }
        assert!(yields > 1);
    }

    #[test]
    fn malformed_metadata_never_becomes_a_validated_session() {
        let mut metadata = Metadata::from_session(OpenSpec::Synthetic(10).open().unwrap().as_ref());
        let valid = bincode::serialize(&metadata).unwrap();
        let mut cases = vec![valid[..valid.len() - 1].to_vec()];
        let mut trailing = valid.clone();
        trailing.push(0);
        cases.push(trailing);
        let mut bad_utf8 = valid;
        bad_utf8[8] = 0xff;
        cases.push(bad_utf8);
        metadata.hierarchy.roots.push(usize::MAX);
        cases.push(bincode::serialize(&metadata).unwrap());
        for bytes in cases {
            let budget = MemoryBudget::new(4 * 1024 * 1024);
            let mut decoder =
                MetadataDecoder::new(bytes.len() as u64, bytes.len() as u64, &budget).unwrap();
            decoder.feed(bytes).unwrap();
            loop {
                match decoder.step() {
                    Err(_) => break,
                    Ok(MetadataStep::Yield) => {}
                    _ => panic!("malformed complete input accepted or waiting for more input"),
                }
            }
            drop(decoder);
            assert_eq!(budget.used(), 0);
        }
    }
}
