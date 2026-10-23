use std::{
    marker::PhantomData,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone, Debug)]
pub enum SignalInput {
    Unsigned(u64),
    Packed {
        alphabet: u8,
        width: Option<u32>,
        bytes: Vec<u8>,
    },
    Real(u64),
    Bytes(Vec<u8>),
}

#[derive(Clone, Copy, Debug)]
pub struct SignalUpdate {
    pub signal: SignalId,
    pub value: u64,
}

use super::{
    ActiveRecord, ClockSource, OwnerRef, ProducerId, Source, SpanKind, Stage, Status, Transaction,
    source,
};
use crate::{
    Error, Result,
    attributes::{Attribute, Attributes, Value},
    framing::RecordingId,
    hierarchy::SignalId,
    schema::{Payload, capture},
    transactions::{EmbeddedEvent, EmbeddedStage, Relation},
};

pub struct Frontend {
    pub(super) origin: RecordingId,
    pub(super) ids: Arc<AtomicU64>,
    pub(super) time: u64,
    pub(super) producer: ProducerId,
    pub(super) finished: bool,
}
impl Frontend {
    pub(super) fn reserve(&self) -> Result<u64> {
        let mut id = self.ids.load(Ordering::Relaxed);
        loop {
            if id == 0 {
                return Err(Error::invalid("record identifiers exhausted"));
            }
            match self.ids.compare_exchange_weak(
                id,
                id.wrapping_add(1),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(id),
                Err(actual) => id = actual,
            }
        }
    }
    pub(super) fn check(&self, origin: RecordingId) -> Result<()> {
        if origin == self.origin {
            Ok(())
        } else {
            Err(Error::invalid(
                "source or handle belongs to another recording",
            ))
        }
    }
}

pub enum Operation {
    Progress(u64),
    Emit {
        signal: SignalId,
        value: SignalInput,
    },
    EmitBatch(Vec<SignalUpdate>),
    Begin {
        id: u64,
        generator: u32,
        time: u64,
        parent: u64,
        payload: Option<Vec<u8>>,
    },
    BeginStage {
        id: u64,
        generator: u32,
        time: u64,
        parent: u64,
        payload: Option<Vec<u8>>,
    },
    Point {
        id: u64,
        generator: u32,
        time: u64,
        parent: u64,
        payload: Vec<u8>,
        log: bool,
    },
    End {
        id: u64,
        time: u64,
        status: u8,
        payload: Option<Vec<u8>>,
    },
    EndStage {
        id: u64,
        time: u64,
        payload: Option<Vec<u8>>,
    },
    Payload {
        id: u64,
        payload: Vec<u8>,
    },
    Attribute {
        id: u64,
        attribute: Attribute,
    },
    SpanKind {
        id: u64,
        kind: u8,
    },
    EmbeddedEvent {
        owner: u64,
        event: EmbeddedEvent,
    },
    EmbeddedStage {
        owner: u64,
        stage: EmbeddedStage,
    },
    EndEmbedded {
        owner: u64,
        stage: u64,
        time: u64,
    },
    Relate(Relation),
    ClockRun {
        generator: u32,
        id: u64,
        first: u64,
        period: u64,
    },
    ClockStop {
        generator: u32,
        time: u64,
    },
    Blackout {
        time: u64,
        active: bool,
    },
    Dump(bool),
}

pub trait Capture {
    fn guard(&self) -> Option<super::Guarded> {
        None
    }
    fn state(&self) -> &Frontend;
    fn state_mut(&mut self) -> &mut Frontend;
    fn dispatch(&mut self, operation: Operation) -> Result<()>;
}

/// Recording calls copy payloads before returning; backend failures poison recording.
pub trait Recorder: Capture {
    /// Intern a string in the recording's shared dictionary.
    fn intern(&mut self, name: &str) -> Result<u32>;

    fn current_time(&self) -> u64 {
        let _guard = self.guard();
        self.state().time
    }
    fn advance_time(&mut self, time: u64) -> Result<()> {
        let _guard = self.guard();
        if time < self.state().time {
            return Err(Error::invalid("producer time moved backwards"));
        }
        self.state_mut().time = time;
        self.dispatch(Operation::Progress(time))
    }
    fn begin<T: Payload>(
        &mut self,
        source: Source<T>,
        payload: &T,
        parent: Option<OwnerRef>,
    ) -> Result<Transaction<T>> {
        let _guard = self.guard();
        self.begin_at(source, self.current_time(), payload, parent)
    }
    fn begin_at<T: Payload>(
        &mut self,
        source: Source<T>,
        time: u64,
        payload: &T,
        parent: Option<OwnerRef>,
    ) -> Result<Transaction<T>> {
        let _guard = self.guard();
        begin_transaction(self, source, time, Some(payload), parent)
    }
    fn begin_deferred<T: Payload>(
        &mut self,
        source: Source<T>,
        parent: Option<OwnerRef>,
    ) -> Result<Transaction<T>> {
        let _guard = self.guard();
        self.begin_deferred_at(source, self.current_time(), parent)
    }
    fn begin_deferred_at<T: Payload>(
        &mut self,
        source: Source<T>,
        time: u64,
        parent: Option<OwnerRef>,
    ) -> Result<Transaction<T>> {
        let _guard = self.guard();
        begin_transaction(self, source, time, None, parent)
    }
    fn begin_stage<T: Payload>(
        &mut self,
        source: Source<T, source::Stage>,
        owner: OwnerRef,
        payload: &T,
    ) -> Result<Stage<T>> {
        let _guard = self.guard();
        self.begin_stage_at(source, owner, self.current_time(), payload)
    }
    fn begin_stage_at<T: Payload>(
        &mut self,
        source: Source<T, source::Stage>,
        owner: OwnerRef,
        time: u64,
        payload: &T,
    ) -> Result<Stage<T>> {
        let _guard = self.guard();
        begin_stage(self, source, owner, time, Some(payload))
    }
    fn begin_stage_deferred<T: Payload>(
        &mut self,
        source: Source<T, source::Stage>,
        owner: OwnerRef,
    ) -> Result<Stage<T>> {
        let _guard = self.guard();
        self.begin_stage_deferred_at(source, owner, self.current_time())
    }
    fn begin_stage_deferred_at<T: Payload>(
        &mut self,
        source: Source<T, source::Stage>,
        owner: OwnerRef,
        time: u64,
    ) -> Result<Stage<T>> {
        let _guard = self.guard();
        begin_stage(self, source, owner, time, None)
    }
    /// Replace the owned payload of an active transaction or stage.
    fn payload<T: Payload, K>(&mut self, record: ActiveRecord<T, K>, payload: &T) -> Result<()> {
        let _guard = self.guard();
        self.state().check(record.origin)?;
        self.dispatch(Operation::Payload {
            id: record.id,
            payload: capture(payload)?,
        })
    }
    fn end<T>(&mut self, transaction: Transaction<T>, status: Status) -> Result<()> {
        let _guard = self.guard();
        self.end_at(transaction, self.current_time(), status)
    }
    fn end_at<T>(&mut self, transaction: Transaction<T>, time: u64, status: Status) -> Result<()> {
        let _guard = self.guard();
        self.state().check(transaction.origin)?;
        self.dispatch(Operation::End {
            id: transaction.id,
            time,
            status: status as u8,
            payload: None,
        })
    }
    fn end_stage<T>(&mut self, stage: Stage<T>) -> Result<()> {
        let _guard = self.guard();
        self.end_stage_at(stage, self.current_time())
    }
    fn end_stage_at<T>(&mut self, stage: Stage<T>, time: u64) -> Result<()> {
        let _guard = self.guard();
        self.state().check(stage.origin)?;
        self.dispatch(Operation::EndStage {
            id: stage.id,
            time,
            payload: None,
        })
    }
    fn end_with_payload<T: Payload>(
        &mut self,
        transaction: Transaction<T>,
        payload: &T,
        status: Status,
    ) -> Result<()> {
        let _guard = self.guard();
        self.end_at_with_payload(transaction, self.current_time(), payload, status)
    }
    fn end_at_with_payload<T: Payload>(
        &mut self,
        transaction: Transaction<T>,
        time: u64,
        payload: &T,
        status: Status,
    ) -> Result<()> {
        let _guard = self.guard();
        self.state().check(transaction.origin)?;
        self.dispatch(Operation::End {
            id: transaction.id,
            time,
            status: status as u8,
            payload: Some(capture(payload)?),
        })
    }
    fn end_stage_with_payload<T: Payload>(&mut self, stage: Stage<T>, payload: &T) -> Result<()> {
        let _guard = self.guard();
        self.end_stage_at_with_payload(stage, self.current_time(), payload)
    }
    fn end_stage_at_with_payload<T: Payload>(
        &mut self,
        stage: Stage<T>,
        time: u64,
        payload: &T,
    ) -> Result<()> {
        let _guard = self.guard();
        self.state().check(stage.origin)?;
        self.dispatch(Operation::EndStage {
            id: stage.id,
            time,
            payload: Some(capture(payload)?),
        })
    }
    fn event<T: Payload>(
        &mut self,
        source: Source<T, source::Event>,
        owner: OwnerRef,
        payload: &T,
    ) -> Result<u64> {
        let _guard = self.guard();
        self.event_at(source, owner, self.current_time(), payload)
    }
    fn event_at<T: Payload>(
        &mut self,
        source: Source<T, source::Event>,
        owner: OwnerRef,
        time: u64,
        payload: &T,
    ) -> Result<u64> {
        let _guard = self.guard();
        self.state().check(source.origin)?;
        self.state().check(owner.origin)?;
        let payload = capture(payload)?;
        let id = self.state().reserve()?;
        self.dispatch(Operation::Point {
            id,
            generator: source.id,
            time,
            parent: owner.id,
            payload,
            log: false,
        })?;
        Ok(id)
    }
    fn log<T: Payload>(
        &mut self,
        source: Source<T, source::Log>,
        payload: &T,
        owner: Option<OwnerRef>,
    ) -> Result<u64> {
        let _guard = self.guard();
        self.log_at(source, self.current_time(), payload, owner)
    }
    fn log_at<T: Payload>(
        &mut self,
        source: Source<T, source::Log>,
        time: u64,
        payload: &T,
        owner: Option<OwnerRef>,
    ) -> Result<u64> {
        let _guard = self.guard();
        self.state().check(source.origin)?;
        if let Some(owner) = owner {
            self.state().check(owner.origin)?;
        }
        let payload = capture(payload)?;
        let id = self.state().reserve()?;
        self.dispatch(Operation::Point {
            id,
            generator: source.id,
            time,
            parent: owner.map_or(0, |owner| owner.id),
            payload,
            log: true,
        })?;
        Ok(id)
    }
    fn attribute(&mut self, owner: OwnerRef, key: u32, value: Value) -> Result<()> {
        let _guard = self.guard();
        self.state().check(owner.origin)?;
        self.dispatch(Operation::Attribute {
            id: owner.id,
            attribute: Attribute { key, value },
        })
    }
    fn span_kind(&mut self, owner: OwnerRef, kind: SpanKind) -> Result<()> {
        let _guard = self.guard();
        self.state().check(owner.origin)?;
        self.dispatch(Operation::SpanKind {
            id: owner.id,
            kind: kind as u8,
        })
    }
    fn embedded_event(
        &mut self,
        owner: OwnerRef,
        time: u64,
        name: u32,
        attributes: Attributes,
    ) -> Result<u64> {
        let _guard = self.guard();
        self.state().check(owner.origin)?;
        let id = self.state().reserve()?;
        self.dispatch(Operation::EmbeddedEvent {
            owner: owner.id,
            event: EmbeddedEvent {
                id,
                time,
                name,
                attributes,
            },
        })?;
        Ok(id)
    }
    fn embedded_stage(
        &mut self,
        owner: OwnerRef,
        begin: u64,
        name: u32,
        lane: u32,
        end: Option<u64>,
        attributes: Attributes,
    ) -> Result<u64> {
        let _guard = self.guard();
        self.state().check(owner.origin)?;
        let id = self.state().reserve()?;
        self.dispatch(Operation::EmbeddedStage {
            owner: owner.id,
            stage: EmbeddedStage {
                id,
                name,
                lane,
                begin,
                end,
                attributes,
            },
        })?;
        Ok(id)
    }
    fn end_embedded_stage(&mut self, owner: OwnerRef, stage: u64, time: u64) -> Result<()> {
        let _guard = self.guard();
        self.state().check(owner.origin)?;
        self.dispatch(Operation::EndEmbedded {
            owner: owner.id,
            stage,
            time,
        })
    }
    fn relate(
        &mut self,
        kind: u32,
        source: u64,
        destination: u64,
        attributes: Attributes,
    ) -> Result<()> {
        let _guard = self.guard();
        self.dispatch(Operation::Relate(Relation {
            kind,
            source,
            destination,
            attributes,
        }))
    }
    fn clock_run(&mut self, clock: ClockSource, first: u64, period: u64) -> Result<u64> {
        let _guard = self.guard();
        self.state().check(clock.origin)?;
        let id = self.state().reserve()?;
        self.dispatch(Operation::ClockRun {
            generator: clock.id,
            id,
            first,
            period,
        })?;
        Ok(id)
    }
    fn clock_stop(&mut self, clock: ClockSource, time: u64) -> Result<()> {
        let _guard = self.guard();
        self.state().check(clock.origin)?;
        self.dispatch(Operation::ClockStop {
            generator: clock.id,
            time,
        })
    }
    fn emit_u64(&mut self, signal: SignalId, value: u64) -> Result<()> {
        let _guard = self.guard();
        self.dispatch(Operation::Emit {
            signal,
            value: SignalInput::Unsigned(value),
        })
    }
    /// Copy scalar updates into one ordered capture. All signals must be valid
    /// bit vectors at most 64 bits wide before any sample in the batch is recorded.
    fn emit_batch(&mut self, updates: &[SignalUpdate]) -> Result<()> {
        let _guard = self.guard();
        self.dispatch(Operation::EmitBatch(updates.to_vec()))
    }
    fn emit_packed(&mut self, signal: SignalId, alphabet: u8, bytes: &[u8]) -> Result<()> {
        let _guard = self.guard();
        self.dispatch(Operation::Emit {
            signal,
            value: SignalInput::Packed {
                alphabet,
                width: None,
                bytes: bytes.to_vec(),
            },
        })
    }
    fn emit_words(&mut self, signal: SignalId, width: u32, words: &[u32]) -> Result<()> {
        let _guard = self.guard();
        if width == 0 {
            return Err(Error::invalid("zero-width word emission"));
        }
        if words.len() < (width as usize).div_ceil(32) {
            return Err(Error::invalid("word emission has insufficient input words"));
        }
        let mut bytes = vec![0; (width as usize).div_ceil(8)];
        crate::signal::pack_words32(words, width, &mut bytes);
        self.dispatch(Operation::Emit {
            signal,
            value: SignalInput::Packed {
                alphabet: 2,
                width: Some(width),
                bytes,
            },
        })
    }
    fn emit_real(&mut self, signal: SignalId, value: f64) -> Result<()> {
        let _guard = self.guard();
        self.dispatch(Operation::Emit {
            signal,
            value: SignalInput::Real(value.to_bits()),
        })
    }
    fn emit_bytes(&mut self, signal: SignalId, bytes: &[u8]) -> Result<()> {
        let _guard = self.guard();
        self.dispatch(Operation::Emit {
            signal,
            value: SignalInput::Bytes(bytes.to_vec()),
        })
    }
    /// Enable or disable waveform capture at the current producer time.
    /// Transitions emit blackout markers; transactions, clocks and logs continue.
    fn dump(&mut self, enabled: bool) -> Result<()> {
        let _guard = self.guard();
        self.dispatch(Operation::Dump(enabled))
    }
    /// Annotate an imported waveform gap without changing capture state.
    fn blackout(&mut self, time: u64, active: bool) -> Result<()> {
        let _guard = self.guard();
        self.dispatch(Operation::Blackout { time, active })
    }
}

fn begin_transaction<T: Payload>(
    recorder: &mut (impl Capture + ?Sized),
    source: Source<T>,
    time: u64,
    payload: Option<&T>,
    parent: Option<OwnerRef>,
) -> Result<Transaction<T>> {
    recorder.state().check(source.origin)?;
    if let Some(parent) = parent {
        recorder.state().check(parent.origin)?;
    }
    let payload = payload.map(capture).transpose()?;
    let id = recorder.state().reserve()?;
    recorder.dispatch(Operation::Begin {
        id,
        generator: source.id,
        time,
        parent: parent.map_or(0, |parent| parent.id),
        payload,
    })?;
    Ok(Transaction {
        id,
        origin: source.origin,
        marker: PhantomData,
    })
}
fn begin_stage<T: Payload>(
    recorder: &mut (impl Capture + ?Sized),
    source: Source<T, source::Stage>,
    owner: OwnerRef,
    time: u64,
    payload: Option<&T>,
) -> Result<Stage<T>> {
    recorder.state().check(source.origin)?;
    recorder.state().check(owner.origin)?;
    let payload = payload.map(capture).transpose()?;
    let id = recorder.state().reserve()?;
    recorder.dispatch(Operation::BeginStage {
        id,
        generator: source.id,
        time,
        parent: owner.id,
        payload,
    })?;
    Ok(Stage {
        id,
        origin: source.origin,
        marker: PhantomData,
    })
}
