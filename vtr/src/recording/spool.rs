use super::{Operation, SignalInput, SignalUpdate};
use crate::{
    Error, Result,
    attributes::{Attributes, encode_attributes, read_attributes},
    hierarchy::SignalId,
    schema::DEFAULT_PAYLOAD_LIMIT,
    transactions::{EmbeddedEvent, EmbeddedStage, Relation},
    wire::Cursor,
};

struct Encoder(Vec<u8>);
impl Encoder {
    fn number(&mut self, value: impl Into<u64>) {
        self.0.extend_from_slice(&value.into().to_le_bytes());
    }
    fn blob(&mut self, bytes: &[u8]) {
        crate::varint::put_blob(&mut self.0, bytes);
    }
    fn attributes(&mut self, values: &[crate::attributes::Attribute]) -> Result<()> {
        encode_attributes(&mut self.0, values, u64::MAX, 0)
    }
    fn payload(&mut self, payload: Option<&[u8]>) {
        self.number(payload.is_some());
        if let Some(payload) = payload {
            self.blob(payload);
        }
    }
}
struct Decoder<'a>(Cursor<'a>);
impl Decoder<'_> {
    fn number(&mut self) -> Result<u64> {
        self.0.u64()
    }
    fn id(&mut self) -> Result<u32> {
        u32::try_from(self.number()?).map_err(|_| Error::Corrupt("spool identifier overflow"))
    }
    fn byte(&mut self) -> Result<u8> {
        u8::try_from(self.number()?).map_err(|_| Error::Corrupt("spool byte overflow"))
    }
    fn boolean(&mut self) -> Result<bool> {
        match self.number()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::Corrupt("invalid spool boolean")),
        }
    }
    fn blob(&mut self) -> Result<Vec<u8>> {
        Ok(self.0.blob()?.to_vec())
    }
    fn attributes(&mut self) -> Result<Attributes> {
        let mut budget = DEFAULT_PAYLOAD_LIMIT;
        read_attributes(&mut self.0, u64::MAX, 0, &mut budget)
    }
    fn payload(&mut self) -> Result<Option<Vec<u8>>> {
        if self.boolean()? {
            Ok(Some(self.blob()?))
        } else {
            Ok(None)
        }
    }
}

pub(super) fn encode(operation: &Operation) -> Result<Vec<u8>> {
    let tag = match operation {
        Operation::Progress(_) => 0,
        Operation::Emit { .. } => 1,
        Operation::Begin { .. } => 2,
        Operation::BeginStage { .. } => 3,
        Operation::Point { .. } => 4,
        Operation::End { .. } => 5,
        Operation::EndStage { .. } => 6,
        Operation::Attribute { .. } => 7,
        Operation::SpanKind { .. } => 8,
        Operation::EmbeddedEvent { .. } => 9,
        Operation::EmbeddedStage { .. } => 10,
        Operation::EndEmbedded { .. } => 11,
        Operation::Relate(_) => 12,
        Operation::ClockRun { .. } => 13,
        Operation::ClockStop { .. } => 14,
        Operation::Blackout { .. } => 15,
        Operation::Dump(_) => 16,
        Operation::Payload { .. } => 17,
        Operation::EmitBatch(_) => 18,
    };
    let mut out = Encoder(vec![tag]);
    match operation {
        Operation::Progress(time) => out.number(*time),
        Operation::EmitBatch(updates) => {
            out.number(updates.len() as u64);
            for update in updates {
                out.number(update.signal.0);
                out.number(update.value);
            }
        }
        Operation::Emit { signal, value } => {
            out.number(signal.0);
            match value {
                SignalInput::Unsigned(value) => {
                    out.number(0u8);
                    out.number(*value);
                }
                SignalInput::Packed {
                    alphabet,
                    width,
                    bytes,
                } => {
                    out.number(1u8);
                    out.number(*alphabet);
                    out.number(width.map_or(0, |width| u64::from(width) + 1));
                    out.blob(bytes);
                }
                SignalInput::Real(value) => {
                    out.number(2u8);
                    out.number(*value);
                }
                SignalInput::Bytes(bytes) => {
                    out.number(3u8);
                    out.blob(bytes);
                }
            }
        }
        Operation::Begin {
            id,
            generator,
            time,
            parent,
            payload,
        }
        | Operation::BeginStage {
            id,
            generator,
            time,
            parent,
            payload,
        } => {
            out.number(*id);
            out.number(*generator);
            out.number(*time);
            out.number(*parent);
            out.payload(payload.as_deref());
        }
        Operation::Point {
            id,
            generator,
            time,
            parent,
            payload,
            log,
        } => {
            out.number(*id);
            out.number(*generator);
            out.number(*time);
            out.number(*parent);
            out.blob(payload);
            out.number(*log);
        }
        Operation::End {
            id,
            time,
            status,
            payload,
        } => {
            out.number(*id);
            out.number(*time);
            out.number(*status);
            out.payload(payload.as_deref());
        }
        Operation::EndStage { id, time, payload } => {
            out.number(*id);
            out.number(*time);
            out.payload(payload.as_deref());
        }
        Operation::Payload { id, payload } => {
            out.number(*id);
            out.blob(payload);
        }
        Operation::Attribute { id, attribute } => {
            out.number(*id);
            out.attributes(std::slice::from_ref(attribute))?;
        }
        Operation::SpanKind { id, kind } => {
            out.number(*id);
            out.number(*kind);
        }
        Operation::EmbeddedEvent { owner, event } => {
            out.number(*owner);
            out.number(event.id);
            out.number(event.time);
            out.number(event.name);
            out.attributes(&event.attributes)?;
        }
        Operation::EmbeddedStage { owner, stage } => {
            out.number(*owner);
            out.number(stage.id);
            out.number(stage.name);
            out.number(stage.lane);
            out.number(stage.begin);
            out.number(stage.end.is_some());
            if let Some(end) = stage.end {
                out.number(end);
            }
            out.attributes(&stage.attributes)?;
        }
        Operation::EndEmbedded { owner, stage, time } => {
            out.number(*owner);
            out.number(*stage);
            out.number(*time);
        }
        Operation::Relate(relation) => {
            out.number(relation.kind);
            out.number(relation.source);
            out.number(relation.destination);
            out.attributes(&relation.attributes)?;
        }
        Operation::ClockRun {
            generator,
            id,
            first,
            period,
        } => {
            out.number(*generator);
            out.number(*id);
            out.number(*first);
            out.number(*period);
        }
        Operation::ClockStop { generator, time } => {
            out.number(*generator);
            out.number(*time);
        }
        Operation::Blackout { time, active } => {
            out.number(*time);
            out.number(*active);
        }
        Operation::Dump(enabled) => out.number(*enabled),
    }
    Ok(out.0)
}

pub(super) fn decode(bytes: &[u8]) -> Result<Operation> {
    let mut input = Decoder(Cursor::new(bytes));
    let tag = input.0.byte()?;
    let operation = match tag {
        0 => Operation::Progress(input.number()?),
        1 => {
            let signal = SignalId(input.id()?);
            let value = match input.number()? {
                0 => SignalInput::Unsigned(input.number()?),
                1 => SignalInput::Packed {
                    alphabet: input.byte()?,
                    width: match input.number()? {
                        0 => None,
                        width => Some(
                            u32::try_from(width - 1)
                                .map_err(|_| Error::Corrupt("spool width overflow"))?,
                        ),
                    },
                    bytes: input.blob()?,
                },
                2 => SignalInput::Real(input.number()?),
                3 => SignalInput::Bytes(input.blob()?),
                _ => return Err(Error::Corrupt("unknown spool signal value")),
            };
            Operation::Emit { signal, value }
        }
        2 | 3 => {
            let id = input.number()?;
            let generator = input.id()?;
            let time = input.number()?;
            let parent = input.number()?;
            let payload = input.payload()?;
            if tag == 2 {
                Operation::Begin {
                    id,
                    generator,
                    time,
                    parent,
                    payload,
                }
            } else {
                Operation::BeginStage {
                    id,
                    generator,
                    time,
                    parent,
                    payload,
                }
            }
        }
        4 => Operation::Point {
            id: input.number()?,
            generator: input.id()?,
            time: input.number()?,
            parent: input.number()?,
            payload: input.blob()?,
            log: input.boolean()?,
        },
        5 => Operation::End {
            id: input.number()?,
            time: input.number()?,
            status: input.byte()?,
            payload: input.payload()?,
        },
        6 => Operation::EndStage {
            id: input.number()?,
            time: input.number()?,
            payload: input.payload()?,
        },
        7 => {
            let id = input.number()?;
            let mut attributes = input.attributes()?;
            if attributes.len() != 1 {
                return Err(Error::Corrupt("invalid spool attribute"));
            }
            Operation::Attribute {
                id,
                attribute: attributes.pop().unwrap(),
            }
        }
        8 => Operation::SpanKind {
            id: input.number()?,
            kind: input.byte()?,
        },
        9 => Operation::EmbeddedEvent {
            owner: input.number()?,
            event: EmbeddedEvent {
                id: input.number()?,
                time: input.number()?,
                name: input.id()?,
                attributes: input.attributes()?,
            },
        },
        10 => {
            let owner = input.number()?;
            let id = input.number()?;
            let name = input.id()?;
            let lane = input.id()?;
            let begin = input.number()?;
            let end = if input.boolean()? {
                Some(input.number()?)
            } else {
                None
            };
            let attributes = input.attributes()?;
            Operation::EmbeddedStage {
                owner,
                stage: EmbeddedStage {
                    id,
                    name,
                    lane,
                    begin,
                    end,
                    attributes,
                },
            }
        }
        11 => Operation::EndEmbedded {
            owner: input.number()?,
            stage: input.number()?,
            time: input.number()?,
        },
        12 => Operation::Relate(Relation {
            kind: input.id()?,
            source: input.number()?,
            destination: input.number()?,
            attributes: input.attributes()?,
        }),
        13 => Operation::ClockRun {
            generator: input.id()?,
            id: input.number()?,
            first: input.number()?,
            period: input.number()?,
        },
        14 => Operation::ClockStop {
            generator: input.id()?,
            time: input.number()?,
        },
        15 => Operation::Blackout {
            time: input.number()?,
            active: input.boolean()?,
        },
        16 => Operation::Dump(input.boolean()?),
        17 => Operation::Payload {
            id: input.number()?,
            payload: input.blob()?,
        },
        18 => {
            let count = input.number()?;
            if count > (input.0.remaining() / 16) as u64 {
                return Err(Error::Corrupt("spool batch exceeds capture size"));
            }
            let mut updates = Vec::with_capacity(count as usize);
            for _ in 0..count {
                updates.push(SignalUpdate {
                    signal: SignalId(input.id()?),
                    value: input.number()?,
                });
            }
            Operation::EmitBatch(updates)
        }
        _ => return Err(Error::Corrupt("unknown staged operation")),
    };
    input.0.done()?;
    Ok(operation)
}
