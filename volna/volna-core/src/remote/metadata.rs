//! Cooperative decoding and validation of raw metadata. Publication still
//! requires the protocol's explicit End.

use super::decode::{Decoder, Reader, Step};
use super::memory::{MemoryBudget, Reservation};
use crate::data::TraceInfo;
use crate::data::source::Generator;
use crate::data::transactions::{AttributeValue, Attributes, Track, TrackKind, TrackRef};
use crate::session::Capabilities;
use std::future::Future;
use std::pin::Pin;

/// Decodes one metadata object from bounded chunks, reserving decoded storage
/// (and validation workspace) from the budget before allocation. Nested
/// attributes are limited to 128 levels. Dropping it releases everything.
pub struct MetadataDecoder(Decoder<super::hierarchy::Header>);

pub struct ValidatedMetadata {
    pub(super) header: super::hierarchy::Header,
    pub(super) reservation: Reservation,
}

pub enum MetadataStep {
    /// Feed another chunk.
    NeedInput,
    /// Yield to input and painting, then step again.
    Yield,
    /// Validated, but private until the protocol End.
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
                    header: metadata,
                    reservation,
                }))
            }
        })
    }
}

async fn metadata(r: &Reader) -> anyhow::Result<super::hierarchy::Header> {
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
        time_unit: if r.boolean().await? {
            Some(r.string().await?)
        } else {
            None
        },
    };
    let capabilities = Capabilities {
        waveforms: r.boolean().await?,
        transactions: r.boolean().await?,
        relations: r.boolean().await?,
    };
    let tracks = r.vector(24, || track(r)).await?;
    let generators = r
        .vector(28, || async {
            Ok(Generator {
                name: r.string().await?,
                stream: r.usize().await?,
                track: TrackRef(r.u32().await?),
                attributes: attributes(r, 0).await?,
            })
        })
        .await?;
    let scopes = r.u32().await?;
    let vars = r.u32().await?;
    Ok(super::hierarchy::Header {
        info,
        capabilities,
        tracks,
        generators,
        scopes,
        vars,
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
    use crate::remote::hierarchy::Header;
    use crate::remote::objects::Metadata;
    use crate::remote::transport::DATA_BYTES;

    fn sample() -> Metadata {
        let source = crate::testing::ProceduralTrace::session(100);
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
        roundtrip(
            Header::from_session(crate::testing::ProceduralTrace::session(100).as_ref())
                .map(|mut header| {
                    header.info = expected.info;
                    header.tracks = expected.tracks;
                    header.capabilities = expected.capabilities;
                    header.generators = expected.hierarchy.generators.as_ref().clone();
                    header
                })
                .unwrap(),
        );
    }

    fn roundtrip(expected: Header) {
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
                        break (decoded.header, decoded.reservation);
                    }
                }
            };
            assert!(chunks.next().is_none());
            assert_eq!(bincode::serialize(&metadata).unwrap(), bytes);

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
        let bytes = bincode::serialize(&Header {
            info: metadata.info.clone(),
            capabilities: metadata.capabilities,
            tracks: metadata.tracks.clone(),
            generators: metadata.hierarchy.generators.as_ref().clone(),
            scopes: metadata.hierarchy.scope_count() as u32,
            vars: metadata.hierarchy.var_count() as u32,
        })
        .unwrap();
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
                    let metadata = decoded.header;
                    assert_eq!(metadata.tracks.len(), 4000);
                    break;
                }
            }
        }
        assert!(yields > 1);
    }

    #[test]
    fn malformed_header_never_decodes() {
        let header =
            Header::from_session(crate::testing::ProceduralTrace::session(10).as_ref()).unwrap();
        let valid = bincode::serialize(&header).unwrap();
        let mut trailing = valid.clone();
        trailing.push(0);
        let mut utf8 = valid.clone();
        utf8[8] = 255;
        for bytes in [valid[..valid.len() - 1].to_vec(), trailing, utf8] {
            let budget = MemoryBudget::new(4 * 1024 * 1024);
            let mut decoder =
                MetadataDecoder::new(bytes.len() as u64, bytes.len() as u64, &budget).unwrap();
            decoder.feed(bytes).unwrap();
            loop {
                match decoder.step() {
                    Err(_) => break,
                    Ok(MetadataStep::Yield) => {}
                    _ => panic!("malformed header accepted"),
                }
            }
            drop(decoder);
            assert_eq!(budget.used(), 0);
        }
    }
}
