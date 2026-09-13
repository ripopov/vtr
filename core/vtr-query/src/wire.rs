//! Length-prefixed stdio transport. Payloads are opaque Protobuf envelopes.
//!
//! Native in-process sessions do not enable this module. Parsing admits one
//! frame at a time; a caller must retain delivery credit until its consumer has
//! consumed or discarded the frame. No unbounded collection of frames is built.

use std::sync::Arc;

use crate::{Budget, Error, Reservation, Result};

pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_REPLY_BYTES: usize = 1024 * 1024;
pub const MAX_DECODED_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_RECORDS: usize = 4096;
pub const MAX_OUTSTANDING: usize = 4;

#[derive(Debug)]
struct Storage {
    bytes: Vec<u8>,
    _reservation: Reservation,
}

/// Immutable payload and its allocation charge. Aliases share both.
#[derive(Clone, Debug)]
pub struct Frame(Arc<Storage>);

impl Frame {
    pub fn bytes(&self) -> &[u8] {
        &self.0.bytes
    }
}

/// Incremental parser for arbitrary pipe chunking. A malformed frame makes the
/// parser terminal: never try to resynchronize in an untrusted payload.
pub struct FrameDecoder {
    budget: Budget,
    max_payload: usize,
    header: [u8; 4],
    header_len: usize,
    payload: Option<Storage>,
    payload_len: usize,
    failed: bool,
}

impl FrameDecoder {
    pub fn new(max_payload: usize, budget: Budget) -> Result<Self> {
        if max_payload == 0 || max_payload > u32::MAX as usize {
            return Err(Error::Invalid("invalid frame size ceiling"));
        }
        Ok(Self {
            budget,
            max_payload,
            header: [0; 4],
            header_len: 0,
            payload: None,
            payload_len: 0,
            failed: false,
        })
    }

    /// Return consumed input bytes and at most one complete frame. Any suffix
    /// stays with the caller until it has capacity to admit another delivery.
    pub fn feed(&mut self, input: &[u8]) -> Result<(usize, Option<Frame>)> {
        if self.failed {
            return Err(Error::Frame("session already failed"));
        }
        let result = self.feed_inner(input);
        if result.is_err() {
            self.failed = true;
            self.payload = None;
        }
        result
    }

    fn feed_inner(&mut self, input: &[u8]) -> Result<(usize, Option<Frame>)> {
        let header_bytes = (4 - self.header_len).min(input.len());
        self.header[self.header_len..self.header_len + header_bytes]
            .copy_from_slice(&input[..header_bytes]);
        self.header_len += header_bytes;
        if self.header_len < 4 {
            return Ok((header_bytes, None));
        }
        if self.payload.is_none() {
            let len = u32::from_le_bytes(self.header) as usize;
            if len == 0 || len > self.max_payload {
                return Err(Error::Frame("payload length exceeds negotiated limits"));
            }
            let reservation = self.budget.reserve(len)?;
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(len)
                .map_err(|_| Error::ResourceLimit)?;
            // No geometric growth: this buffer is allocated once at its final
            // payload capacity. Allocator bookkeeping is process overhead.
            if bytes.capacity() != len {
                return Err(Error::ResourceLimit);
            }
            self.payload_len = len;
            self.payload = Some(Storage {
                bytes,
                _reservation: reservation,
            });
        }
        let payload = self.payload.as_mut().unwrap();
        let remaining = &input[header_bytes..];
        let n = (self.payload_len - payload.bytes.len()).min(remaining.len());
        payload.bytes.extend_from_slice(&remaining[..n]);
        let frame = if payload.bytes.len() == self.payload_len {
            self.header_len = 0;
            Some(Frame(Arc::new(self.payload.take().unwrap())))
        } else {
            None
        };
        Ok((header_bytes + n, frame))
    }

    /// A clean EOF is allowed only between frames.
    pub fn finish(&mut self) -> Result<()> {
        if self.failed || self.header_len != 0 {
            self.failed = true;
            self.payload = None;
            Err(Error::Frame("truncated or failed stream"))
        } else {
            Ok(())
        }
    }
}

/// Read exactly one frame without consuming the next frame from the buffer.
#[cfg(not(target_family = "wasm"))]
pub fn read_frame(
    input: &mut impl std::io::BufRead,
    max_payload: usize,
    budget: Budget,
) -> std::io::Result<Option<Frame>> {
    let invalid = |error| std::io::Error::new(std::io::ErrorKind::InvalidData, error);
    let mut decoder = FrameDecoder::new(max_payload, budget).map_err(invalid)?;
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            decoder.finish().map_err(invalid)?;
            return Ok(None);
        }
        let (consumed, frame) = decoder.feed(available).map_err(invalid)?;
        input.consume(consumed);
        if frame.is_some() {
            return Ok(frame);
        }
    }
}

/// Write one bounded payload without allocating another serialization buffer.
#[cfg(not(target_family = "wasm"))]
pub fn write_frame(
    output: &mut impl std::io::Write,
    payload: &[u8],
    max_payload: usize,
) -> std::io::Result<()> {
    if payload.is_empty() || payload.len() > max_payload || payload.len() > u32::MAX as usize {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid frame length",
        ));
    }
    output.write_all(&(payload.len() as u32).to_le_bytes())?;
    output.write_all(payload)?;
    output.flush()
}

/// Generated schema types. Decoding must be preceded by resource validation;
/// these message definitions alone do not provide allocation admission.
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/vtr.query.v1.rs"));
}
