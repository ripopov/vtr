//! Versioned binary frames and the receive-side object lifecycle.
//!
//! Every packet is one independently checksummed LZ4 frame. This bounds decode
//! work and lets a web receiver yield and acknowledge between packets. Objects
//! may span any number of packets; only their explicit End makes them complete.

use std::io::{Read, Write};

use bincode::Options;
use lz4_flex::frame::{BlockSize, FrameDecoder, FrameEncoder, FrameInfo};
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 3;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const DATA_BYTES: usize = 256 * 1024;
pub const MAX_BATCH: usize = 64;
const MAGIC: &[u8; 4] = b"VLNA";
const HEADER_BYTES: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectId {
    Metadata,
    Signal(u32),
    Track(u32),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    Open { max_object_bytes: u64 },
    Signals(Vec<u32>),
    Track(u32),
    Ack { sequence: u64 },
    Close,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Body {
    Command(Command),
    Begin {
        object: ObjectId,
        decoded_bytes: u64,
    },
    Data {
        offset: u64,
        #[serde(serialize_with = "super::serialize_bytes")]
        bytes: Vec<u8>,
    },
    End,
    Error {
        object: ObjectId,
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Packet {
    /// Zero is reserved for an Open command before the server assigns a session.
    pub session: u64,
    pub request: u64,
    /// Response sequence, starting at zero for each request, including Begin/End.
    pub sequence: u64,
    pub body: Body,
}

fn options() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .reject_trailing_bytes()
        .with_limit(MAX_FRAME_BYTES as u64)
}

fn validate(packet: &Packet) -> anyhow::Result<()> {
    match &packet.body {
        Body::Data { bytes, .. } => anyhow::ensure!(
            !bytes.is_empty() && bytes.len() <= DATA_BYTES,
            "invalid transport chunk size"
        ),
        Body::Command(Command::Signals(ids)) => {
            anyhow::ensure!(
                !ids.is_empty() && ids.len() <= MAX_BATCH,
                "invalid signal batch size"
            );
        }
        Body::Error { message, .. } => {
            anyhow::ensure!(message.len() <= 4096, "error message too large")
        }
        _ => {}
    }
    Ok(())
}

/// Encode one bounded packet.
pub fn encode(packet: &Packet) -> anyhow::Result<Vec<u8>> {
    validate(packet)?;
    let size = options().serialized_size(packet)?;
    let info = FrameInfo::new()
        .block_size(BlockSize::Max64KB)
        .content_size(Some(size))
        .content_checksum(true);
    let mut encoder = FrameEncoder::with_frame_info(info, Vec::new());
    options().serialize_into(&mut encoder, packet)?;
    let body = encoder.finish()?;
    anyhow::ensure!(body.len() <= MAX_FRAME_BYTES, "encoded frame too large");
    let mut result = Vec::with_capacity(HEADER_BYTES + body.len());
    result.extend_from_slice(MAGIC);
    result.extend_from_slice(&VERSION.to_le_bytes());
    result.extend_from_slice(&(body.len() as u32).to_le_bytes());
    result.extend_from_slice(&body);
    Ok(result)
}

fn body_len(header: &[u8]) -> anyhow::Result<usize> {
    anyhow::ensure!(header.len() == HEADER_BYTES, "truncated frame header");
    anyhow::ensure!(&header[..4] == MAGIC, "invalid frame magic");
    anyhow::ensure!(
        u32::from_le_bytes(header[4..8].try_into()?) == VERSION,
        "incompatible Volna protocol version"
    );
    let length = u32::from_le_bytes(header[8..12].try_into()?) as usize;
    anyhow::ensure!(
        length > 0 && length <= MAX_FRAME_BYTES,
        "encoded frame exceeds limit"
    );
    Ok(length)
}

// lz4_flex accepts EOF between blocks, including a missing end/checksum.
// Require exactly our encoder's frame layout before asking it to decompress.
fn validate_lz4(body: &[u8]) -> anyhow::Result<()> {
    anyhow::ensure!(
        body.len() >= 23 && body[..6] == [4, 34, 77, 24, 0x6c, 0x40],
        "invalid LZ4 frame descriptor"
    );
    let size = u64::from_le_bytes(body[6..14].try_into()?);
    anyhow::ensure!(
        size <= MAX_FRAME_BYTES as u64,
        "decoded frame exceeds limit"
    );
    let mut offset = 15;
    loop {
        let block = body
            .get(offset..offset + 4)
            .ok_or_else(|| anyhow::anyhow!("truncated LZ4 block header"))?;
        let size = u32::from_le_bytes(block.try_into()?);
        offset += 4;
        if size == 0 {
            anyhow::ensure!(
                offset + 4 == body.len(),
                "missing checksum or trailing LZ4 data"
            );
            return Ok(());
        }
        let length = (size & 0x7fff_ffff) as usize;
        anyhow::ensure!(length > 0 && length <= 65536, "LZ4 block exceeds limit");
        offset += length;
        anyhow::ensure!(offset <= body.len(), "truncated LZ4 block");
    }
}

pub fn decode(frame: &[u8]) -> anyhow::Result<Packet> {
    anyhow::ensure!(frame.len() >= HEADER_BYTES, "truncated frame header");
    let length = body_len(&frame[..HEADER_BYTES])?;
    anyhow::ensure!(
        frame.len() == HEADER_BYTES + length,
        "incorrect frame length"
    );
    validate_lz4(&frame[HEADER_BYTES..])?;
    let mut decoder = FrameDecoder::new(&frame[HEADER_BYTES..]);
    let mut raw = Vec::new();
    decoder
        .by_ref()
        .take(MAX_FRAME_BYTES as u64 + 1)
        .read_to_end(&mut raw)?;
    anyhow::ensure!(raw.len() <= MAX_FRAME_BYTES, "decoded frame exceeds limit");
    anyhow::ensure!(decoder.into_inner().is_empty(), "trailing compressed data");
    let packet: Packet = options().deserialize(&raw)?;
    validate(&packet)?;
    Ok(packet)
}

/// Read exactly one frame from a pipe. EOF is normal only between frames;
/// short headers and short bodies are errors, never complete objects.
pub fn read_packet(mut input: impl Read) -> anyhow::Result<Option<Packet>> {
    let mut header = [0; HEADER_BYTES];
    loop {
        match input.read(&mut header[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    input.read_exact(&mut header[1..])?;
    let length = body_len(&header)?;
    let mut frame = Vec::with_capacity(HEADER_BYTES + length);
    frame.extend_from_slice(&header);
    frame.resize(HEADER_BYTES + length, 0);
    input.read_exact(&mut frame[HEADER_BYTES..])?;
    decode(&frame).map(Some)
}

pub fn write_packet(mut output: impl Write, packet: &Packet) -> anyhow::Result<()> {
    output.write_all(&encode(packet)?)?;
    output.flush()?;
    Ok(())
}

/// Blocking server-side response stream. The client acknowledges each packet
/// only after consuming it. This writer cannot queue a second frame meanwhile.
pub struct ResponseWriter<R, W> {
    input: R,
    output: W,
    session: u64,
    request: u64,
    sequence: u64,
    failed: bool,
}

impl<R: Read, W: Write> ResponseWriter<R, W> {
    pub fn is_failed(&self) -> bool {
        self.failed
    }
    pub fn new(input: R, output: W, session: u64, request: u64) -> Self {
        Self {
            input,
            output,
            session,
            request,
            sequence: 0,
            failed: false,
        }
    }

    pub fn send(&mut self, body: Body) -> anyhow::Result<()> {
        anyhow::ensure!(!self.failed, "response stream failed");
        // Poison first; restore only after a matching acknowledgement.
        self.failed = true;
        let packet = Packet {
            session: self.session,
            request: self.request,
            sequence: self.sequence,
            body,
        };
        write_packet(&mut self.output, &packet)?;
        let ack = read_packet(&mut self.input)?
            .ok_or_else(|| anyhow::anyhow!("connection closed before acknowledgement"))?;
        anyhow::ensure!(
            ack == acknowledgement(&packet),
            "response acknowledgement mismatch"
        );
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("response sequence exhausted"))?;
        self.failed = false;
        Ok(())
    }

    /// Serialize directly into bounded transport chunks without making a
    /// second full serialized copy on the server. A caller enforces its own
    /// admitted object size before calling this method.
    pub fn object<T: Serialize>(
        &mut self,
        object: ObjectId,
        value: &T,
        limit: u64,
    ) -> anyhow::Result<()> {
        let size = options().with_no_limit().serialized_size(value)?;
        anyhow::ensure!(size <= limit, "object exceeds transfer limit");
        self.send(Body::Begin {
            object,
            decoded_bytes: size,
        })?;
        let result = (|| -> anyhow::Result<()> {
            let mut writer = ObjectWriter {
                stream: self,
                buffer: Vec::with_capacity(DATA_BYTES),
                offset: 0,
                expected: size,
            };
            options()
                .with_no_limit()
                .serialize_into(&mut writer, value)?;
            writer.flush()?;
            anyhow::ensure!(writer.offset == size, "serialized object size changed");
            Ok(())
        })();
        if let Err(error) = result {
            self.failed = true;
            return Err(error);
        }
        self.send(Body::End)
    }
}

pub fn acknowledgement(packet: &Packet) -> Packet {
    Packet {
        session: packet.session,
        request: packet.request,
        sequence: packet.sequence,
        body: Body::Command(Command::Ack {
            sequence: packet.sequence,
        }),
    }
}

struct ObjectWriter<'a, R, W> {
    stream: &'a mut ResponseWriter<R, W>,
    buffer: Vec<u8>,
    offset: u64,
    expected: u64,
}

impl<R: Read, W: Write> Write for ObjectWriter<'_, R, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let n = bytes.len().min(DATA_BYTES - self.buffer.len());
        let remaining = self
            .expected
            .saturating_sub(self.offset + self.buffer.len() as u64);
        if n as u64 > remaining {
            return Err(std::io::Error::other(
                "serialized object exceeds declared size",
            ));
        }
        self.buffer.extend_from_slice(&bytes[..n]);
        if self.buffer.len() == DATA_BYTES {
            self.flush()?;
        }
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if !self.buffer.is_empty() {
            let bytes = std::mem::replace(&mut self.buffer, Vec::with_capacity(DATA_BYTES));
            let length = bytes.len();
            self.stream
                .send(Body::Data {
                    offset: self.offset,
                    bytes,
                })
                .map_err(std::io::Error::other)?;
            self.offset += length as u64;
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Receive {
    Begin {
        object: ObjectId,
        decoded_bytes: u64,
    },
    /// Install into a private builder, then acknowledge; never paint this yet.
    Data(Vec<u8>),
    Complete(ObjectId),
    Failed {
        object: ObjectId,
        message: String,
    },
}

/// One active request, and at most one assembling object in its response.
/// The caller reserves client storage at Begin, consumes each Data chunk and
/// publishes only on Complete. Any error poisons this receiver: discard its
/// private builder and retry using a fresh request identity.
pub struct Receiver {
    session: u64,
    request: u64,
    sequence: u64,
    max_object_bytes: u64,
    expected: Vec<ObjectId>,
    active: Option<(ObjectId, u64, u64)>,
    failed: bool,
}

impl Receiver {
    pub fn new(
        session: u64,
        request: u64,
        expected: Vec<ObjectId>,
        max_object_bytes: u64,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(session != 0, "invalid session identity");
        anyhow::ensure!(
            !expected.is_empty() && expected.len() <= MAX_BATCH,
            "invalid object batch"
        );
        for (i, id) in expected.iter().enumerate() {
            anyhow::ensure!(!expected[..i].contains(id), "duplicate requested object");
        }
        Ok(Self {
            session,
            request,
            sequence: 0,
            max_object_bytes,
            expected,
            active: None,
            failed: false,
        })
    }

    pub fn accept(&mut self, packet: Packet) -> anyhow::Result<Receive> {
        let result = self.accept_inner(packet);
        if result.is_err() {
            self.failed = true;
            self.active = None;
        }
        result
    }

    fn accept_inner(&mut self, packet: Packet) -> anyhow::Result<Receive> {
        anyhow::ensure!(!self.failed, "receiver failed; retry with a new request");
        anyhow::ensure!(
            packet.session == self.session && packet.request == self.request,
            "response identity mismatch"
        );
        anyhow::ensure!(
            packet.sequence == self.sequence,
            "response sequence mismatch"
        );
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("response sequence exhausted"))?;
        validate(&packet)?;
        match packet.body {
            Body::Begin {
                object,
                decoded_bytes,
            } => {
                anyhow::ensure!(
                    self.active.is_none() && self.expected.contains(&object),
                    "unexpected object begin"
                );
                anyhow::ensure!(
                    decoded_bytes <= self.max_object_bytes,
                    "object exceeds transfer limit"
                );
                self.active = Some((object, decoded_bytes, 0));
                Ok(Receive::Begin {
                    object,
                    decoded_bytes,
                })
            }
            Body::Data { offset, bytes } => {
                let (_, total, received) = self
                    .active
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("data without object begin"))?;
                anyhow::ensure!(offset == *received, "object offset mismatch");
                let next = received
                    .checked_add(bytes.len() as u64)
                    .ok_or_else(|| anyhow::anyhow!("object size overflow"))?;
                anyhow::ensure!(next <= *total, "object exceeds declared size");
                *received = next;
                Ok(Receive::Data(bytes))
            }
            Body::End => {
                let (object, total, received) = self
                    .active
                    .take()
                    .ok_or_else(|| anyhow::anyhow!("end without object begin"))?;
                anyhow::ensure!(received == total, "truncated object");
                self.expected.retain(|id| *id != object);
                Ok(Receive::Complete(object))
            }
            Body::Error { object, message } => {
                anyhow::ensure!(self.expected.contains(&object), "unexpected object error");
                anyhow::ensure!(
                    self.active.is_none_or(|(id, _, _)| id == object),
                    "error for another active object"
                );
                self.active = None;
                self.expected.retain(|id| *id != object);
                Ok(Receive::Failed { object, message })
            }
            Body::Command(_) => anyhow::bail!("command in response stream"),
        }
    }

    pub fn is_complete(&self) -> bool {
        !self.failed && self.expected.is_empty() && self.active.is_none()
    }

    /// EOF before this point fails all still pending objects. Completed objects
    /// may remain resident; no partially received object survives disconnect.
    pub fn finish(self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.is_complete(),
            "connection ended before request completion"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn packet(sequence: u64, body: Body) -> Packet {
        Packet {
            session: 7,
            request: 9,
            sequence,
            body,
        }
    }
    fn receiver(limit: u64) -> Receiver {
        Receiver::new(7, 9, vec![ObjectId::Signal(2)], limit).unwrap()
    }
    fn begin(size: u64) -> Packet {
        packet(
            0,
            Body::Begin {
                object: ObjectId::Signal(2),
                decoded_bytes: size,
            },
        )
    }

    #[test]
    fn framed_pipe_roundtrip_and_all_truncations() {
        let p = packet(
            0,
            Body::Data {
                offset: u64::MAX,
                bytes: (0..DATA_BYTES).map(|i| (i % 251) as u8).collect(),
            },
        );
        let encoded = encode(&p).unwrap();
        assert_eq!(decode(&encoded).unwrap(), p);
        let mut pipe = vec![];
        write_packet(&mut pipe, &p).unwrap();
        write_packet(&mut pipe, &begin(0)).unwrap();
        let mut input = pipe.as_slice();
        assert_eq!(read_packet(&mut input).unwrap(), Some(p));
        assert_eq!(read_packet(&mut input).unwrap(), Some(begin(0)));
        assert!(read_packet(&mut input).unwrap().is_none());
        for end in 1..encoded.len() {
            assert!(
                read_packet(&encoded[..end]).is_err(),
                "accepted truncation at {end}"
            );
            if end >= HEADER_BYTES {
                let mut shortened = encoded[..end].to_vec();
                shortened[8..12].copy_from_slice(&((end - HEADER_BYTES) as u32).to_le_bytes());
                assert!(
                    decode(&shortened).is_err(),
                    "accepted missing LZ4 footer at {end}"
                );
            }
        }
        let mut wrong = encoded.clone();
        wrong[4] = 99;
        assert!(decode(&wrong).unwrap_err().to_string().contains("version"));
        let mut wrong = encoded.clone();
        wrong[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&wrong).is_err());
        let mut wrong = encoded;
        let n = wrong.len();
        wrong[n - 1] ^= 1;
        assert!(decode(&wrong).is_err());
    }

    #[test]
    fn only_explicit_exact_end_publishes_an_object() {
        let mut r = receiver(3);
        assert!(matches!(r.accept(begin(3)).unwrap(), Receive::Begin { .. }));
        assert_eq!(
            r.accept(packet(
                1,
                Body::Data {
                    offset: 0,
                    bytes: vec![1, 2, 3]
                }
            ))
            .unwrap(),
            Receive::Data(vec![1, 2, 3])
        );
        assert!(!r.is_complete());
        assert_eq!(
            r.accept(packet(2, Body::End)).unwrap(),
            Receive::Complete(ObjectId::Signal(2))
        );
        assert!(r.is_complete());
        r.finish().unwrap();
        let mut empty = receiver(0);
        empty.accept(begin(0)).unwrap();
        empty.accept(packet(1, Body::End)).unwrap();
        empty.finish().unwrap();
    }

    #[test]
    fn rejects_wrong_order_size_identity_and_incomplete_connections() {
        assert!(receiver(2).accept(begin(3)).is_err());
        for bad in [
            packet(1, Body::End),
            packet(
                1,
                Body::Data {
                    offset: 1,
                    bytes: vec![1],
                },
            ),
            packet(
                1,
                Body::Data {
                    offset: 0,
                    bytes: vec![1, 2, 3, 4],
                },
            ),
        ] {
            let mut r = receiver(3);
            r.accept(begin(3)).unwrap();
            assert!(r.accept(bad).is_err());
            assert!(r.accept(packet(2, Body::End)).is_err());
            assert!(r.finish().is_err());
        }
        let mut wrong = begin(0);
        wrong.session = 8;
        assert!(receiver(0).accept(wrong).is_err());
        let mut wrong = begin(0);
        wrong.request = 8;
        assert!(receiver(0).accept(wrong).is_err());
        assert!(receiver(0).accept(packet(1, Body::End)).is_err());
        let mut r = receiver(3);
        r.accept(begin(3)).unwrap();
        assert!(r.finish().is_err());
    }

    #[test]
    fn batch_errors_are_per_object_and_duplicates_are_rejected() {
        let a = ObjectId::Signal(1);
        let b = ObjectId::Signal(2);
        let mut r = Receiver::new(7, 9, vec![a, b], 10).unwrap();
        r.accept(packet(
            0,
            Body::Begin {
                object: a,
                decoded_bytes: 0,
            },
        ))
        .unwrap();
        r.accept(packet(1, Body::End)).unwrap();
        assert!(matches!(
            r.accept(packet(
                2,
                Body::Error {
                    object: b,
                    message: "decode failed".into()
                }
            ))
            .unwrap(),
            Receive::Failed { .. }
        ));
        assert!(r.is_complete());
        assert!(
            r.accept(packet(
                3,
                Body::Error {
                    object: b,
                    message: "duplicate".into()
                }
            ))
            .is_err()
        );
        assert!(Receiver::new(7, 9, vec![a, a], 10).is_err());
    }

    #[test]
    fn decompression_expansion_is_bounded() {
        let info = FrameInfo::new()
            .block_size(BlockSize::Max64KB)
            .content_size(Some(MAX_FRAME_BYTES as u64 + 1))
            .content_checksum(true);
        let mut encoder = FrameEncoder::with_frame_info(info, Vec::new());
        encoder.write_all(&vec![0; MAX_FRAME_BYTES + 1]).unwrap();
        let body = encoder.finish().unwrap();
        let mut frame = MAGIC.to_vec();
        frame.extend_from_slice(&VERSION.to_le_bytes());
        frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
        frame.extend(body);
        assert!(
            decode(&frame)
                .unwrap_err()
                .to_string()
                .contains("decoded frame exceeds")
        );
    }

    #[test]
    fn object_writer_streams_large_payload_and_obeys_acknowledgements() {
        let value = vec![42u8; DATA_BYTES * 3 + 7];
        let size = options().with_no_limit().serialized_size(&value).unwrap();
        let chunks = size.div_ceil(DATA_BYTES as u64);
        let mut acks = vec![];
        for sequence in 0..chunks + 2 {
            write_packet(&mut acks, &acknowledgement(&packet(sequence, Body::End))).unwrap();
        }
        let mut output = vec![];
        ResponseWriter::new(acks.as_slice(), &mut output, 7, 9)
            .object(ObjectId::Signal(2), &value, size)
            .unwrap();
        let mut receiver = receiver(size);
        let mut input = output.as_slice();
        let mut assembled = vec![];
        while let Some(packet) = read_packet(&mut input).unwrap() {
            if let Receive::Data(bytes) = receiver.accept(packet).unwrap() {
                assembled.extend(bytes);
            }
        }
        receiver.finish().unwrap();
        assert_eq!(options().deserialize::<Vec<u8>>(&assembled).unwrap(), value);

        let mut output = vec![];
        let mut sender = ResponseWriter::new(&[][..], &mut output, 7, 9);
        assert!(sender.object(ObjectId::Signal(2), &value, size).is_err());
        assert!(sender.send(Body::End).is_err(), "poison failed sender");
        let mut input = output.as_slice();
        assert!(matches!(
            read_packet(&mut input).unwrap().unwrap().body,
            Body::Begin { .. }
        ));
        assert!(
            read_packet(&mut input).unwrap().is_none(),
            "no second frame before ack"
        );
    }
}
