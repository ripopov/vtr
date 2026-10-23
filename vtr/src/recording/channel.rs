//! Bounded SPSC capture storage with contiguous packets across the logical ring end.

use super::{
    Operation, QueueStats, SignalInput,
    capture::{PayloadOperation, SignalRef},
    transport::charge,
};
use crate::{Error, Result, SignalId};
use std::{
    cell::{Cell, UnsafeCell},
    mem::MaybeUninit,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Instant,
};

const PROGRESS: u64 = 0;
const UNSIGNED: u64 = 1;
const REAL: u64 = 2;
const OWNED: u64 = 3;
const BEGIN: u64 = 4;
const BEGIN_STAGE: u64 = 5;
const POINT: u64 = 6;
const END: u64 = 7;
const END_STAGE: u64 = 8;
const REPLACE: u64 = 9;
const LOG: u64 = 10;
const PACKED: u64 = 11;
const BYTES: u64 = 12;
const NONE: usize = 0x00ff_ffff;

#[derive(Clone, Copy)]
#[repr(C)]
struct Entry {
    header: u64,
    value: Content,
}
#[derive(Clone, Copy)]
union Content {
    bits: u64,
    operation: *mut Operation,
}
#[repr(align(64))]
struct Counter(AtomicUsize);
#[repr(align(64))]
struct MessageCount(AtomicU64);

pub(super) struct Channel {
    entries: Box<[UnsafeCell<MaybeUninit<Entry>>]>,
    slots: usize,
    capacity: usize,
    written: Counter,
    read: Counter,
    consumed: Counter,
    active: Counter,
    closed: AtomicBool,
    submitted: MessageCount,
    completed: MessageCount,
    peak: AtomicUsize,
    waits: AtomicUsize,
    wait_nanoseconds: AtomicUsize,
}
// SAFETY: Exclusive cursors transfer initialized packet storage with release/acquire
// publication. The doubled allocation keeps wrapped packets contiguous without copying.
unsafe impl Sync for Channel {}
// SAFETY: Owned entries contain Send operations; the last owner reclaims abandoned ones.
unsafe impl Send for Channel {}

#[derive(Default)]
struct WriteCursor {
    written: usize,
    bytes: usize,
    cached_read: usize,
    cached_consumed: usize,
    peak: usize,
    submitted: u64,
}
pub(super) struct Sender {
    channel: Arc<Channel>,
    cursor: WriteCursor,
    independent_stop: Cell<bool>,
}
pub(super) struct Receiver {
    channel: Arc<Channel>,
    read: usize,
    consumed: usize,
    published_read: usize,
    cached_written: usize,
    completed: u64,
    time: u64,
}
struct Publication<'a>(Option<&'a AtomicUsize>);
impl Drop for Publication<'_> {
    fn drop(&mut self) {
        if let Some(active) = self.0 {
            active.store(0, Ordering::Release);
        }
    }
}

pub(super) fn channel(capacity: usize, time: u64) -> (Sender, Receiver) {
    make_channel(capacity, time, true)
}

// Its writer can stop the implicit channel only between captures. Creating an
// independent sealer enables the handshake while that writer is exclusively idle.
pub(super) fn exclusive_channel(capacity: usize, time: u64) -> (Sender, Receiver) {
    make_channel(capacity, time, false)
}

fn make_channel(capacity: usize, time: u64, independent_stop: bool) -> (Sender, Receiver) {
    let slots = (capacity / size_of::<Entry>()).max(1).next_power_of_two();
    let shared = Arc::new(Channel {
        entries: (0..slots * 2)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect(),
        slots,
        capacity,
        written: Counter(AtomicUsize::new(0)),
        read: Counter(AtomicUsize::new(0)),
        consumed: Counter(AtomicUsize::new(0)),
        active: Counter(AtomicUsize::new(0)),
        closed: AtomicBool::new(false),
        submitted: MessageCount(AtomicU64::new(0)),
        completed: MessageCount(AtomicU64::new(0)),
        peak: AtomicUsize::new(0),
        waits: AtomicUsize::new(0),
        wait_nanoseconds: AtomicUsize::new(0),
    });
    (
        Sender {
            channel: shared.clone(),
            cursor: WriteCursor::default(),
            independent_stop: Cell::new(independent_stop),
        },
        Receiver {
            channel: shared,
            read: 0,
            consumed: 0,
            published_read: 0,
            cached_written: 0,
            completed: 0,
            time,
        },
    )
}
impl Channel {
    #[inline]
    fn enter(&self, independent_stop: bool) -> Result<Publication<'_>> {
        if !independent_stop {
            return if self.closed.load(Ordering::Acquire) {
                Err(Error::State("recording is finalized"))
            } else {
                Ok(Publication(None))
            };
        }
        // Sequential consistency orders admission against close even when each
        // side initially observes the other flag unset. Only this sender writes active.
        self.active.0.store(1, Ordering::SeqCst);
        if self.closed.load(Ordering::SeqCst) {
            self.active.0.store(0, Ordering::Release);
            return Err(Error::State("recording is finalized"));
        }
        Ok(Publication(Some(&self.active.0)))
    }
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        while self.active.0.load(Ordering::SeqCst) != 0 {
            std::thread::yield_now();
        }
    }
    pub fn stats(&self) -> QueueStats {
        QueueStats {
            submitted: self.submitted.0.load(Ordering::Acquire),
            completed: self.completed.0.load(Ordering::Acquire),
            peak_bytes: self.peak.load(Ordering::Relaxed),
            waits: self.waits.load(Ordering::Relaxed) as u64,
            wait_nanoseconds: self.wait_nanoseconds.load(Ordering::Relaxed) as u64,
        }
    }
}
impl WriteCursor {
    fn fits(&self, channel: &Channel, cells: usize, bytes: usize) -> bool {
        cells
            <= channel
                .slots
                .saturating_sub(self.written.wrapping_sub(self.cached_read))
            && (self.bytes == self.cached_consumed
                || bytes
                    <= channel
                        .capacity
                        .saturating_sub(self.bytes.wrapping_sub(self.cached_consumed)))
    }
    #[inline]
    fn reserve(
        &mut self,
        channel: &Channel,
        cells: usize,
        bytes: usize,
        check: impl Fn() -> Result<()>,
    ) -> Result<()> {
        check()?;
        if !self.fits(channel, cells, bytes) {
            self.cached_read = channel.read.0.load(Ordering::Acquire);
            self.cached_consumed = channel.consumed.0.load(Ordering::Acquire);
            if !self.fits(channel, cells, bytes) {
                self.wait(channel, cells, bytes, check)?;
            }
        }
        Ok(())
    }
    #[cold]
    fn wait(
        &mut self,
        channel: &Channel,
        cells: usize,
        bytes: usize,
        check: impl Fn() -> Result<()>,
    ) -> Result<()> {
        channel.waits.fetch_add(1, Ordering::Relaxed);
        let began = Instant::now();
        let mut polls = 0usize;
        loop {
            check()?;
            if channel.closed.load(Ordering::Acquire) {
                return Err(Error::State("recording is finalized"));
            }
            self.cached_read = channel.read.0.load(Ordering::Acquire);
            self.cached_consumed = channel.consumed.0.load(Ordering::Acquire);
            if self.fits(channel, cells, bytes) {
                break;
            }
            if polls == 0 || polls.is_multiple_of(64) {
                std::thread::yield_now();
            } else {
                std::hint::spin_loop();
            }
            if polls % 256 == 255 {
                std::thread::sleep(std::time::Duration::from_micros(1));
            }
            polls = polls.wrapping_add(1);
        }
        channel.wait_nanoseconds.fetch_add(
            usize::try_from(began.elapsed().as_nanos()).unwrap_or(usize::MAX),
            Ordering::Relaxed,
        );
        Ok(())
    }
    #[inline]
    fn commit(&mut self, channel: &Channel, cells: usize, bytes: usize) {
        self.bytes = self.bytes.wrapping_add(bytes);
        self.written = self.written.wrapping_add(cells);
        self.submitted = self.submitted.wrapping_add(1);
        channel.submitted.0.store(self.submitted, Ordering::Relaxed);
        channel.written.0.store(self.written, Ordering::Release);
        let queued = self.bytes.wrapping_sub(self.cached_consumed);
        if queued > self.peak {
            self.peak = queued;
            channel.peak.store(queued, Ordering::Relaxed);
        }
    }
}

fn metadata(action: PayloadOperation) -> (u64, u32, u64, u64) {
    match action {
        PayloadOperation::Begin {
            generator,
            time,
            parent,
        } => (BEGIN, generator, time, parent),
        PayloadOperation::BeginStage {
            generator,
            time,
            parent,
        } => (BEGIN_STAGE, generator, time, parent),
        PayloadOperation::Point {
            generator,
            time,
            parent,
            log,
        } => (if log { LOG } else { POINT }, generator, time, parent),
        PayloadOperation::End { time, status, .. } => (END, u32::from(status), time, 0),
        PayloadOperation::EndStage { time, .. } => (END_STAGE, 0, time, 0),
        PayloadOperation::Replace { .. } => (REPLACE, 0, 0, 0),
    }
}
fn action(tag: u64, source: u32, time: u64, parent: u64, id: u64) -> PayloadOperation {
    match tag {
        BEGIN => PayloadOperation::Begin {
            generator: source,
            time,
            parent,
        },
        BEGIN_STAGE => PayloadOperation::BeginStage {
            generator: source,
            time,
            parent,
        },
        POINT | LOG => PayloadOperation::Point {
            generator: source,
            time,
            parent,
            log: tag == LOG,
        },
        END => PayloadOperation::End {
            id,
            time,
            status: source as u8,
        },
        END_STAGE => PayloadOperation::EndStage { id, time },
        REPLACE => PayloadOperation::Replace { id },
        _ => unreachable!("internal payload tag"),
    }
}
fn cells(entry: Entry) -> usize {
    if entry.header & 255 < BEGIN {
        1
    } else {
        let length = (entry.header >> 40) as usize;
        let base = if entry.header & 255 >= PACKED { 1 } else { 2 };
        base + if length == NONE {
            0
        } else {
            length.div_ceil(size_of::<Entry>())
        }
    }
}

impl Sender {
    pub fn enable_independent_stop(&self) {
        self.independent_stop.set(true);
    }

    pub fn channel(&self) -> Arc<Channel> {
        self.channel.clone()
    }
    #[inline]
    fn entry(&mut self, entry: Entry, bytes: usize, check: impl Fn() -> Result<()>) -> Result<()> {
        let _publication = self.channel.enter(self.independent_stop.get())?;
        self.cursor.reserve(&self.channel, 1, bytes, check)?;
        let slot = self.cursor.written & (self.channel.slots - 1);
        // SAFETY: Cached acquire credits certify exclusive ownership of this slot.
        unsafe {
            (*self.channel.entries[slot].get()).write(entry);
        }
        self.cursor.commit(&self.channel, 1, bytes);
        Ok(())
    }
    pub fn publish_owned(
        &mut self,
        operation: Operation,
        check: impl Fn() -> Result<()>,
    ) -> Result<()> {
        let bytes = size_of::<Entry>() + charge(&operation);
        // Allocate after admission so a full ring retains only one waiting capture per producer.
        let _publication = self.channel.enter(self.independent_stop.get())?;
        self.cursor.reserve(&self.channel, 1, bytes, check)?;
        let entry = Entry {
            header: OWNED,
            value: Content {
                operation: Box::into_raw(Box::new(operation)),
            },
        };
        let slot = self.cursor.written & (self.channel.slots - 1);
        // SAFETY: This producer owns the credited slot until release publication.
        unsafe {
            (*self.channel.entries[slot].get()).write(entry);
        }
        self.cursor.commit(&self.channel, 1, bytes);
        Ok(())
    }
    pub fn payload_fits(&self, payload: Option<&[u8]>) -> bool {
        payload.is_none_or(|bytes| bytes.len() < NONE)
            && 2 + payload.map_or(0, |bytes| bytes.len().div_ceil(size_of::<Entry>()))
                <= self.channel.slots
    }
    fn payload_owned(
        &mut self,
        action: PayloadOperation,
        id: u64,
        payload: Option<Vec<u8>>,
        check: impl Fn() -> Result<()>,
    ) -> Result<()> {
        if self.payload_fits(payload.as_deref()) {
            self.publish_payload(action, id, payload.as_deref(), check)
        } else {
            self.publish_owned(action.owned(id, payload), check)
        }
    }
    pub fn publish_payload(
        &mut self,
        action: PayloadOperation,
        id: u64,
        payload: Option<&[u8]>,
        check: impl Fn() -> Result<()>,
    ) -> Result<()> {
        let length = payload.map_or(NONE, <[u8]>::len);
        let cells = 2 + payload.map_or(0, |bytes| bytes.len().div_ceil(size_of::<Entry>()));
        if payload.is_some() && length >= NONE || cells > self.channel.slots {
            return self.publish_owned(action.owned(id, payload.map(<[u8]>::to_vec)), check);
        }
        let bytes = cells * size_of::<Entry>();
        let _publication = self.channel.enter(self.independent_stop.get())?;
        self.cursor.reserve(&self.channel, cells, bytes, check)?;
        let slot = self.cursor.written & (self.channel.slots - 1);
        let (tag, source, time, parent) = metadata(action);
        let first = Entry {
            header: tag | (u64::from(source) << 8) | ((length as u64) << 40),
            value: Content { bits: id },
        };
        let second = Entry {
            header: time,
            value: Content { bits: parent },
        };
        // SAFETY: One reservation owns all packet cells. Doubled storage makes the
        // entire packet contiguous even when its logical positions wrap at the ring end.
        unsafe {
            (*self.channel.entries[slot].get()).write(first);
            (*self.channel.entries[slot + 1].get()).write(second);
            if let Some(payload) = payload {
                std::ptr::copy_nonoverlapping(
                    payload.as_ptr(),
                    self.channel
                        .entries
                        .as_ptr()
                        .wrapping_add(slot + 2)
                        .cast_mut()
                        .cast::<u8>(),
                    payload.len(),
                );
            }
        }
        self.cursor.commit(&self.channel, cells, bytes);
        Ok(())
    }
    #[inline]
    pub fn publish_progress(&mut self, time: u64, check: impl Fn() -> Result<()>) -> Result<()> {
        self.entry(
            Entry {
                header: PROGRESS,
                value: Content { bits: time },
            },
            size_of::<Entry>(),
            check,
        )
    }
    pub fn publish_signal_ref(
        &mut self,
        signal: SignalId,
        value: SignalRef<'_>,
        check: impl Fn() -> Result<()>,
    ) -> Result<()> {
        let (tag, metadata, payload) = match value {
            SignalRef::Unsigned(bits) => {
                return self.publish_signal(signal, SignalInput::Unsigned(bits), check);
            }
            SignalRef::Real(bits) => {
                return self.publish_signal(signal, SignalInput::Real(bits), check);
            }
            SignalRef::Packed {
                alphabet,
                width,
                bytes,
            } => (
                PACKED,
                u64::from(width.unwrap_or(0))
                    | (u64::from(alphabet) << 32)
                    | (u64::from(width.is_some()) << 40),
                bytes,
            ),
            SignalRef::Bytes(bytes) => (BYTES, 0, bytes),
        };
        let cells = 1 + payload.len().div_ceil(size_of::<Entry>());
        if payload.len() >= NONE || cells > self.channel.slots {
            return self.publish_owned(
                Operation::Emit {
                    signal,
                    value: value.owned(),
                },
                check,
            );
        }
        let _publication = self.channel.enter(self.independent_stop.get())?;
        let bytes = cells * size_of::<Entry>();
        self.cursor.reserve(&self.channel, cells, bytes, check)?;
        let slot = self.cursor.written & (self.channel.slots - 1);
        let entry = Entry {
            header: tag | (u64::from(signal.0) << 8) | ((payload.len() as u64) << 40),
            value: Content { bits: metadata },
        };
        // SAFETY: The reservation owns every packet cell until release publication.
        // Allocation-derived pointers cover payloads spanning multiple cells.
        unsafe {
            (*self.channel.entries[slot].get()).write(entry);
            std::ptr::copy_nonoverlapping(
                payload.as_ptr(),
                self.channel
                    .entries
                    .as_ptr()
                    .wrapping_add(slot + 1)
                    .cast_mut()
                    .cast::<u8>(),
                payload.len(),
            );
        }
        self.cursor.commit(&self.channel, cells, bytes);
        Ok(())
    }
    #[inline]
    pub fn publish_signal(
        &mut self,
        signal: SignalId,
        value: SignalInput,
        check: impl Fn() -> Result<()>,
    ) -> Result<()> {
        let (tag, bits) = match value {
            SignalInput::Unsigned(bits) => (UNSIGNED, bits),
            SignalInput::Real(bits) => (REAL, bits),
            value => {
                let (SignalInput::Packed { bytes: payload, .. } | SignalInput::Bytes(payload)) =
                    &value
                else {
                    unreachable!()
                };
                if payload.len() >= NONE
                    || 1 + payload.len().div_ceil(size_of::<Entry>()) > self.channel.slots
                {
                    return self.publish_owned(Operation::Emit { signal, value }, check);
                }
                return self.publish_signal_ref(signal, value.borrow(), check);
            }
        };
        self.entry(
            Entry {
                header: (u64::from(signal.0) << 8) | tag,
                value: Content { bits },
            },
            size_of::<Entry>(),
            check,
        )
    }
    pub fn publish(&mut self, operation: Operation, check: impl Fn() -> Result<()>) -> Result<()> {
        match operation {
            Operation::Progress(time) => self.publish_progress(time, check),
            Operation::Emit { signal, value } => self.publish_signal(signal, value, check),
            Operation::Begin {
                id,
                generator,
                time,
                parent,
                payload,
            } => self.payload_owned(
                PayloadOperation::Begin {
                    generator,
                    time,
                    parent,
                },
                id,
                payload,
                check,
            ),
            Operation::BeginStage {
                id,
                generator,
                time,
                parent,
                payload,
            } => self.payload_owned(
                PayloadOperation::BeginStage {
                    generator,
                    time,
                    parent,
                },
                id,
                payload,
                check,
            ),
            Operation::Point {
                id,
                generator,
                time,
                parent,
                payload,
                log,
            } => self.payload_owned(
                PayloadOperation::Point {
                    generator,
                    time,
                    parent,
                    log,
                },
                id,
                Some(payload),
                check,
            ),
            Operation::End {
                id,
                time,
                status,
                payload,
            } => self.payload_owned(
                PayloadOperation::End { id, time, status },
                id,
                payload,
                check,
            ),
            Operation::EndStage { id, time, payload } => {
                self.payload_owned(PayloadOperation::EndStage { id, time }, id, payload, check)
            }
            Operation::Payload { id, payload } => {
                self.payload_owned(PayloadOperation::Replace { id }, id, Some(payload), check)
            }
            operation => self.publish_owned(operation, check),
        }
    }
}

pub(super) struct Capture<'a> {
    receiver: &'a mut Receiver,
    entry: Entry,
    bytes: usize,
    owned: bool,
}
impl Capture<'_> {
    #[inline]
    pub fn stamp(&self) -> u64 {
        self.receiver.time
    }
    #[inline]
    pub fn progress(&self) -> Option<u64> {
        (self.entry.header & 255 == PROGRESS).then_some(self.receiver.time)
    }
    #[inline]
    pub fn cells(&self) -> usize {
        cells(self.entry)
    }
    #[inline]
    pub fn scalar(&self) -> Option<(SignalId, SignalInput)> {
        let tag = self.entry.header & 255;
        if !matches!(tag, UNSIGNED | REAL) {
            return None;
        }
        // SAFETY: Both scalar tags initialize the bits union member.
        let bits = unsafe { self.entry.value.bits };
        Some((
            SignalId((self.entry.header >> 8) as u32),
            if tag == UNSIGNED {
                SignalInput::Unsigned(bits)
            } else {
                SignalInput::Real(bits)
            },
        ))
    }
    #[inline]
    fn bytes(&self, offset: usize, length: usize) -> &[u8] {
        let slot = self.receiver.read & (self.receiver.channel.slots - 1);
        // SAFETY: This guard withholds read credits for the complete initialized
        // packet. The allocation-derived pointer spans every borrowed payload cell.
        unsafe {
            std::slice::from_raw_parts(
                self.receiver
                    .channel
                    .entries
                    .as_ptr()
                    .wrapping_add(slot + offset)
                    .cast::<u8>(),
                length,
            )
        }
    }
    #[inline]
    pub fn signal_ref(&self) -> Option<(SignalId, SignalRef<'_>)> {
        let tag = self.entry.header & 255;
        if !matches!(tag, PACKED | BYTES) {
            return None;
        }
        let payload = self.bytes(1, (self.entry.header >> 40) as usize);
        // SAFETY: Packed and byte packet headers initialize the scalar bits member.
        let metadata = unsafe { self.entry.value.bits };
        let value = if tag == BYTES {
            SignalRef::Bytes(payload)
        } else {
            SignalRef::Packed {
                alphabet: (metadata >> 32) as u8,
                width: (metadata & (1 << 40) != 0).then_some(metadata as u32),
                bytes: payload,
            }
        };
        Some((SignalId((self.entry.header >> 8) as u32), value))
    }
    #[inline]
    pub fn payload(&self) -> Option<(PayloadOperation, u64, Option<&[u8]>)> {
        let tag = self.entry.header & 255;
        if !(BEGIN..=LOG).contains(&tag) {
            return None;
        }
        let slot = self.receiver.read & (self.receiver.channel.slots - 1);
        // SAFETY: Payload packets initialize both scalar metadata cells before publication.
        let second = unsafe { (*self.receiver.channel.entries[slot + 1].get()).assume_init_read() };
        // SAFETY: Non-owned payload headers store scalar record and parent IDs.
        let (id, parent) = unsafe { (self.entry.value.bits, second.value.bits) };
        let operation = action(
            tag,
            (self.entry.header >> 8) as u32,
            second.header,
            parent,
            id,
        );
        let length = (self.entry.header >> 40) as usize;
        let payload = if length == NONE {
            None
        } else {
            Some(self.bytes(2, length))
        };
        Some((operation, id, payload))
    }
    pub fn into_operation(mut self) -> Operation {
        if self.owned {
            self.owned = false;
            // SAFETY: OWNED transfers one Box into this capture; dropping the capture only releases cells now.
            return unsafe { *Box::from_raw(self.entry.value.operation) };
        }
        if let Some((action, id, payload)) = self.payload() {
            return action.owned(id, payload.map(<[u8]>::to_vec));
        }
        if let Some((signal, value)) = self.signal_ref() {
            return Operation::Emit {
                signal,
                value: value.owned(),
            };
        }
        // SAFETY: Non-owned scalar headers initialize the bits member.
        let value = unsafe { self.entry.value.bits };
        match self.entry.header & 255 {
            PROGRESS => Operation::Progress(value),
            UNSIGNED => Operation::Emit {
                signal: SignalId((self.entry.header >> 8) as u32),
                value: SignalInput::Unsigned(value),
            },
            REAL => Operation::Emit {
                signal: SignalId((self.entry.header >> 8) as u32),
                value: SignalInput::Real(value),
            },
            _ => unreachable!("internal capture tag"),
        }
    }
}
impl Drop for Capture<'_> {
    #[inline]
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: An unconsumed OWNED capture still holds its unique Box.
            unsafe {
                drop(Box::from_raw(self.entry.value.operation));
            }
        }
        self.receiver.read = self.receiver.read.wrapping_add(self.cells());
        self.receiver.consumed = self.receiver.consumed.wrapping_add(self.bytes);
        self.receiver.completed = self.receiver.completed.wrapping_add(1);
        if self
            .receiver
            .read
            .wrapping_sub(self.receiver.published_read)
            >= (self.receiver.channel.slots / 16).clamp(1, 256)
            || self.receiver.read == self.receiver.cached_written
        {
            self.receiver.release();
        }
    }
}
impl Receiver {
    pub fn available(&mut self) -> usize {
        self.cached_written = self.channel.written.0.load(Ordering::Acquire);
        self.cached_written.wrapping_sub(self.read)
    }
    #[inline]
    pub fn next(&mut self) -> Option<Capture<'_>> {
        if self.read == self.cached_written && self.available() == 0 {
            self.release();
            return None;
        }
        let slot = self.read & (self.channel.slots - 1);
        // SAFETY: The acquire publication covers the complete initialized packet.
        let entry = unsafe { (*self.channel.entries[slot].get()).assume_init_read() };
        let tag = entry.header & 255;
        let bytes = if tag == OWNED {
            // SAFETY: The published Box remains owned by this receiver until capture consumption.
            size_of::<Entry>() + charge(unsafe { &*entry.value.operation })
        } else {
            if tag == PROGRESS {
                // SAFETY: Progress headers initialize the bits member with their timestamp.
                self.time = unsafe { entry.value.bits };
            }
            cells(entry) * size_of::<Entry>()
        };
        Some(Capture {
            receiver: self,
            entry,
            bytes,
            owned: tag == OWNED,
        })
    }

    #[cfg(test)]
    fn pop(&mut self) -> Option<(u64, Operation)> {
        self.next()
            .map(|capture| (capture.stamp(), capture.into_operation()))
    }
    #[inline]
    pub fn release(&mut self) {
        if self.read == self.published_read {
            return;
        }
        self.channel
            .consumed
            .0
            .store(self.consumed, Ordering::Release);
        self.channel
            .completed
            .0
            .store(self.completed, Ordering::Relaxed);
        self.channel.read.0.store(self.read, Ordering::Release);
        self.published_read = self.read;
    }
}
impl Drop for Receiver {
    fn drop(&mut self) {
        self.release();
    }
}
impl Drop for Channel {
    fn drop(&mut self) {
        let mut read = *self.read.0.get_mut();
        let written = *self.written.0.get_mut();
        while read != written {
            let slot = read & (self.slots - 1);
            // SAFETY: No producer or consumer remains, and each cursor points to a complete packet header.
            let entry = unsafe { self.entries[slot].get_mut().assume_init_read() };
            if entry.header & 255 == OWNED {
                // SAFETY: Only abandoned OWNED packets still hold their original Box.
                unsafe {
                    drop(Box::from_raw(entry.value.operation));
                }
            }
            read = read.wrapping_add(cells(entry));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn waveform_packets_copy_borrowed_storage_and_preserve_widths_across_wraps() {
        let count = if cfg!(miri) { 16u64 } else { 512 };
        for capacity in [16, 32, 64, 256, 1024] {
            let (mut sender, mut receiver) = channel(capacity, 0);
            let producer = std::thread::spawn(move || {
                for time in 0..count {
                    sender.publish_progress(time, || Ok(())).unwrap();
                    let mut bytes = waveform_bytes(time);
                    let width = [None, Some(0), Some(1), Some(u32::MAX)][time as usize % 4];
                    sender
                        .publish_signal_ref(
                            SignalId(7),
                            SignalRef::Packed {
                                alphabet: [2, 4, 9][time as usize % 3],
                                width,
                                bytes: &bytes,
                            },
                            || Ok(()),
                        )
                        .unwrap();
                    sender
                        .publish_signal_ref(SignalId(8), SignalRef::Bytes(&bytes), || Ok(()))
                        .unwrap();
                    bytes.fill(255);
                }
            });
            for time in 0..count {
                for kind in 0..3 {
                    let capture = loop {
                        if receiver.available() != 0 {
                            break receiver.next().unwrap();
                        }
                        std::thread::yield_now();
                    };
                    assert_eq!(capture.stamp(), time);
                    if kind != 0 && waveform_bytes(time).len().div_ceil(16) < capacity / 16 {
                        assert!(!capture.owned);
                        let (signal, value) = capture.signal_ref().unwrap();
                        let bytes = match value {
                            SignalRef::Packed {
                                alphabet,
                                width,
                                bytes,
                            } => {
                                assert_eq!(kind, 1);
                                assert_eq!(signal, SignalId(7));
                                assert_eq!(alphabet, [2, 4, 9][time as usize % 3]);
                                assert_eq!(
                                    width,
                                    [None, Some(0), Some(1), Some(u32::MAX)][time as usize % 4]
                                );
                                bytes
                            }
                            SignalRef::Bytes(bytes) => {
                                assert_eq!(signal, SignalId(8));
                                assert_eq!(kind, 2);
                                bytes
                            }
                            _ => panic!("unexpected waveform packet"),
                        };
                        std::thread::yield_now();
                        assert_eq!(bytes, waveform_bytes(time));
                    } else {
                        match (kind, capture.into_operation()) {
                            (0, Operation::Progress(actual)) => assert_eq!(actual, time),
                            (
                                1,
                                Operation::Emit {
                                    signal: SignalId(7),
                                    value:
                                        SignalInput::Packed {
                                            alphabet,
                                            width,
                                            bytes,
                                        },
                                },
                            ) => {
                                assert_eq!(alphabet, [2, 4, 9][time as usize % 3]);
                                assert_eq!(
                                    width,
                                    [None, Some(0), Some(1), Some(u32::MAX)][time as usize % 4]
                                );
                                assert_eq!(bytes, waveform_bytes(time));
                            }
                            (
                                2,
                                Operation::Emit {
                                    signal: SignalId(8),
                                    value: SignalInput::Bytes(bytes),
                                },
                            ) => assert_eq!(bytes, waveform_bytes(time)),
                            _ => panic!("unexpected owned waveform capture"),
                        }
                    }
                }
            }
            producer.join().unwrap();
            receiver.release();
            let stats = receiver.channel.stats();
            assert_eq!(stats.submitted, count * 3);
            assert_eq!(stats.completed, count * 3);
        }
    }

    fn waveform_bytes(time: u64) -> Vec<u8> {
        vec![time as u8; [0, 1, 15, 16, 17, 31, 32, 33, 64, 255, 1025][time as usize % 11]]
    }

    #[test]
    fn payload_packets_preserve_borrowed_bytes_optional_payloads_and_wrapped_boundaries() {
        for capacity in [16, 32, 64, 128, 256] {
            let (mut sender, mut receiver) = channel(capacity, 0);
            let count = if cfg!(miri) { 16 } else { 256 };
            let producer = std::thread::spawn(move || {
                for time in 0..count {
                    let lengths = [0, 1, 15, 16, 17, 31, 32, 33, 64, 129];
                    let bytes = vec![time as u8; lengths[time as usize % lengths.len()]];
                    sender
                        .publish(Operation::Progress(time), || Ok(()))
                        .unwrap();
                    sender
                        .publish_payload(
                            PayloadOperation::Begin {
                                generator: 7,
                                time,
                                parent: 0,
                            },
                            time + 1,
                            Some(&bytes),
                            || Ok(()),
                        )
                        .unwrap();
                    sender
                        .publish(
                            Operation::End {
                                id: time + 1,
                                time: time + 2,
                                status: 0,
                                payload: None,
                            },
                            || Ok(()),
                        )
                        .unwrap();
                }
            });
            for time in 0..count {
                for kind in 0..3 {
                    let capture = loop {
                        if receiver.available() != 0 {
                            break receiver.next().unwrap();
                        }
                        std::thread::yield_now();
                    };
                    assert_eq!(capture.stamp(), time);
                    if let Some((action, id, bytes)) = capture.payload() {
                        assert_ne!(kind, 0);
                        assert_eq!(id, time + 1);
                        if kind == 1 {
                            assert!(
                                matches!(action, PayloadOperation::Begin { generator: 7, time: t, parent: 0 } if t == time)
                            );
                            std::thread::yield_now();
                            assert_eq!(
                                bytes.unwrap(),
                                vec![
                                    time as u8;
                                    [0, 1, 15, 16, 17, 31, 32, 33, 64, 129][time as usize % 10]
                                ]
                            );
                        } else {
                            assert!(bytes.is_none());
                        }
                    } else {
                        match (kind, capture.into_operation()) {
                            (0, Operation::Progress(t)) => assert_eq!(t, time),
                            (
                                1,
                                Operation::Begin {
                                    id,
                                    payload: Some(bytes),
                                    ..
                                },
                            ) => {
                                assert_eq!(id, time + 1);
                                assert_eq!(
                                    bytes,
                                    vec![
                                        time as u8;
                                        [0, 1, 15, 16, 17, 31, 32, 33, 64, 129][time as usize % 10]
                                    ]
                                );
                            }
                            (2, Operation::End { payload: None, .. }) => (),
                            _ => panic!("payload capture changed shape"),
                        }
                    }
                }
            }
            producer.join().unwrap();
            receiver.release();
            assert_eq!(receiver.channel.stats().submitted, count * 3);
            assert_eq!(receiver.channel.stats().completed, count * 3);
        }
    }

    #[test]
    fn cursor_wraparound_does_not_reuse_unconsumed_storage() {
        let (mut sender, mut receiver) = channel(64, 0);
        let start = usize::MAX - 1;
        sender.cursor.written = start;
        sender.cursor.cached_read = start;
        sender.channel.written.0.store(start, Ordering::Relaxed);
        sender.channel.read.0.store(start, Ordering::Relaxed);
        receiver.read = start;
        receiver.published_read = start;
        receiver.cached_written = start;
        for time in 0..16 {
            sender
                .publish(Operation::Progress(time), || Ok(()))
                .unwrap();
            assert!(
                matches!(receiver.pop(), Some((t, Operation::Progress(p))) if t == time && p == time)
            );
            receiver.release();
        }
        assert!(receiver.pop().is_none());
    }

    #[test]
    fn wrapping_slots_transfer_scalar_and_oversized_owned_captures_in_order() {
        for capacity in [1, 16, 256, 4096] {
            let (mut sender, mut receiver) = channel(capacity, 0);
            let producer = std::thread::spawn(move || {
                for time in 0..if cfg!(miri) { 16 } else { 1000 } {
                    sender
                        .publish(Operation::Progress(time), || Ok(()))
                        .unwrap();
                    sender
                        .publish(
                            Operation::Emit {
                                signal: SignalId(7),
                                value: SignalInput::Real(time),
                            },
                            || Ok(()),
                        )
                        .unwrap();
                    sender
                        .publish(
                            Operation::Emit {
                                signal: SignalId(8),
                                value: SignalInput::Bytes(vec![time as u8; 1024]),
                            },
                            || Ok(()),
                        )
                        .unwrap();
                }
            });
            for time in 0..if cfg!(miri) { 16 } else { 1000 } {
                for kind in 0..3 {
                    let (stamp, operation) = loop {
                        if let Some(item) = receiver.pop() {
                            break item;
                        }
                        std::thread::yield_now();
                    };
                    assert_eq!(stamp, time);
                    match (kind, operation) {
                        (0, Operation::Progress(actual)) => assert_eq!(actual, time),
                        (
                            1,
                            Operation::Emit {
                                signal: SignalId(7),
                                value: SignalInput::Real(bits),
                            },
                        ) => assert_eq!(bits, time),
                        (
                            2,
                            Operation::Emit {
                                signal: SignalId(8),
                                value: SignalInput::Bytes(bytes),
                            },
                        ) => assert_eq!(bytes, vec![time as u8; 1024]),
                        _ => panic!("capture reordered or corrupted"),
                    }
                }
            }
            receiver.release();
            producer.join().unwrap();
            let stats = receiver.channel.stats();
            assert_eq!(
                (stats.submitted, stats.completed),
                (
                    if cfg!(miri) { 48 } else { 3000 },
                    if cfg!(miri) { 48 } else { 3000 }
                )
            );
        }
    }

    #[test]
    fn stopping_a_full_queue_cancels_unaccepted_capture_and_keeps_accepted_data() {
        let (mut sender, mut receiver) = channel(16, 0);
        let channel = sender.channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        sender.publish(Operation::Progress(42), || Ok(())).unwrap();
        let cancel = cancelled.clone();
        let producer = std::thread::spawn(move || {
            sender.publish(
                Operation::Emit {
                    signal: SignalId(0),
                    value: SignalInput::Bytes(vec![5; 1024]),
                },
                || {
                    if cancel.load(Ordering::Acquire) {
                        Err(Error::State("cancelled"))
                    } else {
                        Ok(())
                    }
                },
            )
        });
        while channel.stats().waits == 0 {
            std::thread::yield_now();
        }
        cancelled.store(true, Ordering::Release);
        channel.close();
        assert!(producer.join().unwrap().is_err());
        assert!(matches!(
            receiver.pop(),
            Some((42, Operation::Progress(42)))
        ));
        assert!(receiver.pop().is_none());
    }

    #[test]
    fn close_preserves_all_admitted_captures_and_rejects_later_publication() {
        let (mut sender, mut receiver) = channel(256, 0);
        let channel = sender.channel();
        let producer = std::thread::spawn(move || {
            let mut accepted = 0;
            for time in 1..=128 {
                if sender
                    .publish(Operation::Progress(time), || Ok(()))
                    .is_err()
                {
                    break;
                }
                accepted += 1;
            }
            (sender, accepted)
        });
        while channel.stats().submitted == 0 {
            std::thread::yield_now();
        }
        channel.close();
        let (mut sender, accepted) = producer.join().unwrap();
        assert!(sender.publish(Operation::Progress(129), || Ok(())).is_err());
        for expected in 1..=accepted {
            assert!(
                matches!(receiver.pop(), Some((time, Operation::Progress(value)))
                if time == expected && value == expected)
            );
        }
        assert!(receiver.pop().is_none());
        assert_eq!(channel.stats().submitted, accepted);
        assert_eq!(channel.stats().completed, accepted);
    }

    #[test]
    fn an_exclusive_publisher_can_enable_independent_stop_before_handoff() {
        let (mut sender, mut receiver) = exclusive_channel(256, 0);
        sender
            .publish_signal_ref(SignalId(3), SignalRef::Bytes(&[42; 17]), || Ok(()))
            .unwrap();
        assert!(
            matches!(receiver.pop(), Some((0, Operation::Emit { signal: SignalId(3), value: SignalInput::Bytes(bytes) })) if bytes == [42; 17])
        );
        sender.enable_independent_stop();
        let channel = sender.channel();
        let producer = std::thread::spawn(move || {
            let mut accepted = 0;
            for time in 1..=128 {
                if sender.publish_progress(time, || Ok(())).is_err() {
                    break;
                }
                accepted += 1;
            }
            (sender, accepted)
        });
        while channel.stats().submitted == 1 {
            std::thread::yield_now();
        }
        channel.close();
        let (mut sender, accepted) = producer.join().unwrap();
        assert!(sender.publish_progress(129, || Ok(())).is_err());
        for expected in 1..=accepted {
            assert!(
                matches!(receiver.pop(), Some((time, Operation::Progress(value))) if time == expected && value == expected)
            );
        }
        assert!(receiver.pop().is_none());
        assert_eq!(channel.stats().submitted, accepted + 1);
        assert_eq!(channel.stats().completed, accepted + 1);
    }

    #[test]
    fn abandoned_owned_captures_are_reclaimed_without_a_consumer() {
        let (mut sender, receiver) = channel(4096, 0);
        for _ in 0..2 {
            sender
                .publish_owned(
                    Operation::Emit {
                        signal: SignalId(0),
                        value: SignalInput::Bytes(vec![3; 512]),
                    },
                    || Ok(()),
                )
                .unwrap();
        }
        drop(receiver);
        drop(sender);
    }

    #[test]
    fn abandoning_wrapped_payloads_skips_data_cells_and_reclaims_following_boxes() {
        let (mut sender, mut receiver) = channel(256, 0);
        for time in 0..13 {
            sender
                .publish(Operation::Progress(time), || Ok(()))
                .unwrap();
            assert!(receiver.pop().is_some());
        }
        sender
            .publish_payload(
                PayloadOperation::Replace { id: 1 },
                1,
                Some(&[3; 48]),
                || Ok(()),
            )
            .unwrap();
        sender
            .publish_owned(
                Operation::Emit {
                    signal: SignalId(0),
                    value: SignalInput::Bytes(vec![7; 32]),
                },
                || Ok(()),
            )
            .unwrap();
        drop(receiver);
        drop(sender);
    }
}
