use std::{
    cell::Cell,
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

#[derive(Clone, Copy)]
pub enum SignalRef<'a> {
    Unsigned(u64),
    Packed {
        alphabet: u8,
        width: Option<u32>,
        bytes: &'a [u8],
    },
    Real(u64),
    Bytes(&'a [u8]),
}
impl SignalInput {
    #[inline]
    pub fn borrow(&self) -> SignalRef<'_> {
        match self {
            Self::Unsigned(value) => SignalRef::Unsigned(*value),
            Self::Real(value) => SignalRef::Real(*value),
            Self::Packed {
                alphabet,
                width,
                bytes,
            } => SignalRef::Packed {
                alphabet: *alphabet,
                width: *width,
                bytes,
            },
            Self::Bytes(bytes) => SignalRef::Bytes(bytes),
        }
    }
}
impl SignalRef<'_> {
    pub fn owned(self) -> SignalInput {
        match self {
            Self::Unsigned(value) => SignalInput::Unsigned(value),
            Self::Real(value) => SignalInput::Real(value),
            Self::Packed {
                alphabet,
                width,
                bytes,
            } => SignalInput::Packed {
                alphabet,
                width,
                bytes: bytes.to_vec(),
            },
            Self::Bytes(bytes) => SignalInput::Bytes(bytes.to_vec()),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SignalUpdate {
    pub signal: SignalId,
    pub value: u64,
}

use super::{
    ActiveRecord, ClockSource, Origin, OwnerRef, ProducerId, Source, SpanKind, Stage, Status,
    Transaction, session::Session, source,
};
use crate::{
    Error, Result,
    attributes::{Attribute, Attributes, Value},
    hierarchy::SignalId,
    schema::Payload,
    transactions::{EmbeddedEvent, EmbeddedStage, Relation},
};

pub struct Frontend {
    pub(super) origin: Origin,
    pub(super) ids: RecordIds,
    pub(super) time: u64,
    pub(super) producer: ProducerId,
    pub(super) finished: bool,
    pub(super) payload: Vec<u8>,
}
pub(super) enum RecordIds {
    Local(Cell<u64>),
    Shared(Arc<AtomicU64>),
}
impl RecordIds {
    #[inline]
    fn reserve(&self) -> Result<u64> {
        let ids = match self {
            Self::Local(next) => {
                let id = next.get();
                if id == 0 {
                    return Err(Error::invalid("record identifiers exhausted"));
                }
                next.set(id.wrapping_add(1));
                return Ok(id);
            }
            Self::Shared(ids) => ids,
        };
        let mut id = ids.load(Ordering::Relaxed);
        loop {
            if id == 0 {
                return Err(Error::invalid("record identifiers exhausted"));
            }
            match ids.compare_exchange_weak(
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
}
impl Frontend {
    #[inline]
    pub(super) fn reserve(&self) -> Result<u64> {
        self.ids.reserve()
    }
    #[inline]
    pub(super) fn check(&self, origin: Origin) -> Result<()> {
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

#[derive(Clone, Copy)]
pub enum PayloadOperation {
    Begin {
        generator: u32,
        time: u64,
        parent: u64,
    },
    BeginStage {
        generator: u32,
        time: u64,
        parent: u64,
    },
    Point {
        generator: u32,
        time: u64,
        parent: u64,
        log: bool,
    },
    End {
        id: u64,
        time: u64,
        status: u8,
    },
    EndStage {
        id: u64,
        time: u64,
    },
    Replace {
        id: u64,
    },
}
impl PayloadOperation {
    #[inline]
    pub fn record_id(self, front: &Frontend) -> Result<u64> {
        match self {
            Self::End { id, .. } | Self::EndStage { id, .. } | Self::Replace { id } => Ok(id),
            _ => front.reserve(),
        }
    }
    pub fn owned(self, id: u64, payload: Option<Vec<u8>>) -> Operation {
        match self {
            Self::Begin {
                generator,
                time,
                parent,
            } => Operation::Begin {
                id,
                generator,
                time,
                parent,
                payload,
            },
            Self::BeginStage {
                generator,
                time,
                parent,
            } => Operation::BeginStage {
                id,
                generator,
                time,
                parent,
                payload,
            },
            Self::Point {
                generator,
                time,
                parent,
                log,
            } => Operation::Point {
                id,
                generator,
                time,
                parent,
                payload: payload.unwrap(),
                log,
            },
            Self::End { time, status, .. } => Operation::End {
                id,
                time,
                status,
                payload,
            },
            Self::EndStage { time, .. } => Operation::EndStage { id, time, payload },
            Self::Replace { .. } => Operation::Payload {
                id,
                payload: payload.unwrap(),
            },
        }
    }
}

pub trait Capture {
    fn state(&self) -> &Frontend;
    fn session(&mut self) -> Session<'_>;
    fn time(&self) -> u64 {
        self.state().time
    }
}

/// Recording calls copy payloads before returning; backend failures poison recording.
pub trait Recorder: Capture {
    /// Intern a string in the recording's shared dictionary.
    fn intern(&mut self, name: &str) -> Result<u32>;

    fn current_time(&self) -> u64 {
        self.time()
    }
    #[inline]
    fn advance_time(&mut self, time: u64) -> Result<()> {
        let mut session = self.session();
        if time < session.state().time {
            return Err(Error::invalid("producer time moved backwards"));
        }
        if time == session.state().time {
            return session.check();
        }
        session.state_mut().time = time;
        session.dispatch_progress(time)
    }
    #[inline]
    fn begin<T: Payload>(
        &mut self,
        source: Source<T>,
        payload: &T,
        parent: Option<OwnerRef>,
    ) -> Result<Transaction<T>> {
        let mut session = self.session();
        let time = session.state().time;
        begin_transaction(&mut session, source, time, Some(payload), parent)
    }
    #[inline]
    fn begin_at<T: Payload>(
        &mut self,
        source: Source<T>,
        time: u64,
        payload: &T,
        parent: Option<OwnerRef>,
    ) -> Result<Transaction<T>> {
        let mut session = self.session();
        begin_transaction(&mut session, source, time, Some(payload), parent)
    }
    #[inline]
    fn begin_deferred<T: Payload>(
        &mut self,
        source: Source<T>,
        parent: Option<OwnerRef>,
    ) -> Result<Transaction<T>> {
        let mut session = self.session();
        let time = session.state().time;
        begin_transaction(&mut session, source, time, None, parent)
    }
    #[inline]
    fn begin_deferred_at<T: Payload>(
        &mut self,
        source: Source<T>,
        time: u64,
        parent: Option<OwnerRef>,
    ) -> Result<Transaction<T>> {
        let mut session = self.session();
        begin_transaction(&mut session, source, time, None, parent)
    }
    #[inline]
    fn begin_stage<T: Payload>(
        &mut self,
        source: Source<T, source::Stage>,
        owner: OwnerRef,
        payload: &T,
    ) -> Result<Stage<T>> {
        let mut session = self.session();
        let time = session.state().time;
        begin_stage(&mut session, source, owner, time, Some(payload))
    }
    #[inline]
    fn begin_stage_at<T: Payload>(
        &mut self,
        source: Source<T, source::Stage>,
        owner: OwnerRef,
        time: u64,
        payload: &T,
    ) -> Result<Stage<T>> {
        let mut session = self.session();
        begin_stage(&mut session, source, owner, time, Some(payload))
    }
    #[inline]
    fn begin_stage_deferred<T: Payload>(
        &mut self,
        source: Source<T, source::Stage>,
        owner: OwnerRef,
    ) -> Result<Stage<T>> {
        let mut session = self.session();
        let time = session.state().time;
        begin_stage(&mut session, source, owner, time, None)
    }
    #[inline]
    fn begin_stage_deferred_at<T: Payload>(
        &mut self,
        source: Source<T, source::Stage>,
        owner: OwnerRef,
        time: u64,
    ) -> Result<Stage<T>> {
        let mut session = self.session();
        begin_stage(&mut session, source, owner, time, None)
    }
    /// Replace the owned payload of an active transaction or stage.
    fn payload<T: Payload, K>(&mut self, record: ActiveRecord<T, K>, payload: &T) -> Result<()> {
        let mut session = self.session();
        session.state().check(record.origin)?;
        session
            .payload(PayloadOperation::Replace { id: record.id }, payload)
            .map(|_| ())
    }
    fn end<T>(&mut self, transaction: Transaction<T>, status: Status) -> Result<()> {
        let mut session = self.session();
        session.end_at(transaction, session.current_time(), status)
    }
    fn end_at<T>(&mut self, transaction: Transaction<T>, time: u64, status: Status) -> Result<()> {
        let mut session = self.session();
        session.state().check(transaction.origin)?;
        session.dispatch_payload(
            PayloadOperation::End {
                id: transaction.id,
                time,
                status: status as u8,
            },
            transaction.id,
        )
    }
    fn end_stage<T>(&mut self, stage: Stage<T>) -> Result<()> {
        let mut session = self.session();
        session.end_stage_at(stage, session.current_time())
    }
    fn end_stage_at<T>(&mut self, stage: Stage<T>, time: u64) -> Result<()> {
        let mut session = self.session();
        session.state().check(stage.origin)?;
        session.dispatch_payload(PayloadOperation::EndStage { id: stage.id, time }, stage.id)
    }
    fn end_with_payload<T: Payload>(
        &mut self,
        transaction: Transaction<T>,
        payload: &T,
        status: Status,
    ) -> Result<()> {
        let mut session = self.session();
        session.end_at_with_payload(transaction, session.current_time(), payload, status)
    }
    fn end_at_with_payload<T: Payload>(
        &mut self,
        transaction: Transaction<T>,
        time: u64,
        payload: &T,
        status: Status,
    ) -> Result<()> {
        let mut session = self.session();
        session.state().check(transaction.origin)?;
        session
            .payload(
                PayloadOperation::End {
                    id: transaction.id,
                    time,
                    status: status as u8,
                },
                payload,
            )
            .map(|_| ())
    }
    fn end_stage_with_payload<T: Payload>(&mut self, stage: Stage<T>, payload: &T) -> Result<()> {
        let mut session = self.session();
        session.end_stage_at_with_payload(stage, session.current_time(), payload)
    }
    fn end_stage_at_with_payload<T: Payload>(
        &mut self,
        stage: Stage<T>,
        time: u64,
        payload: &T,
    ) -> Result<()> {
        let mut session = self.session();
        session.state().check(stage.origin)?;
        session
            .payload(PayloadOperation::EndStage { id: stage.id, time }, payload)
            .map(|_| ())
    }
    fn event<T: Payload>(
        &mut self,
        source: Source<T, source::Event>,
        owner: OwnerRef,
        payload: &T,
    ) -> Result<u64> {
        let mut session = self.session();
        session.event_at(source, owner, session.current_time(), payload)
    }
    fn event_at<T: Payload>(
        &mut self,
        source: Source<T, source::Event>,
        owner: OwnerRef,
        time: u64,
        payload: &T,
    ) -> Result<u64> {
        let mut session = self.session();
        session.state().check(source.origin)?;
        session.state().check(owner.origin)?;
        session.payload(
            PayloadOperation::Point {
                generator: source.id,
                time,
                parent: owner.id,
                log: false,
            },
            payload,
        )
    }
    fn log<T: Payload>(
        &mut self,
        source: Source<T, source::Log>,
        payload: &T,
        owner: Option<OwnerRef>,
    ) -> Result<u64> {
        let mut session = self.session();
        session.log_at(source, session.current_time(), payload, owner)
    }
    fn log_at<T: Payload>(
        &mut self,
        source: Source<T, source::Log>,
        time: u64,
        payload: &T,
        owner: Option<OwnerRef>,
    ) -> Result<u64> {
        let mut session = self.session();
        session.state().check(source.origin)?;
        if let Some(owner) = owner {
            session.state().check(owner.origin)?;
        }
        session.payload(
            PayloadOperation::Point {
                generator: source.id,
                time,
                parent: owner.map_or(0, |owner| owner.id),
                log: true,
            },
            payload,
        )
    }
    fn attribute(&mut self, owner: OwnerRef, key: u32, value: Value) -> Result<()> {
        let mut session = self.session();
        session.state().check(owner.origin)?;
        session.dispatch(Operation::Attribute {
            id: owner.id,
            attribute: Attribute { key, value },
        })
    }
    fn span_kind(&mut self, owner: OwnerRef, kind: SpanKind) -> Result<()> {
        let mut session = self.session();
        session.state().check(owner.origin)?;
        session.dispatch(Operation::SpanKind {
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
        let mut session = self.session();
        session.state().check(owner.origin)?;
        let id = session.state().reserve()?;
        session.dispatch(Operation::EmbeddedEvent {
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
        let mut session = self.session();
        session.state().check(owner.origin)?;
        let id = session.state().reserve()?;
        session.dispatch(Operation::EmbeddedStage {
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
        let mut session = self.session();
        session.state().check(owner.origin)?;
        session.dispatch(Operation::EndEmbedded {
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
        let mut session = self.session();
        session.dispatch(Operation::Relate(Relation {
            kind,
            source,
            destination,
            attributes,
        }))
    }
    fn clock_run(&mut self, clock: ClockSource, first: u64, period: u64) -> Result<u64> {
        let mut session = self.session();
        session.state().check(clock.origin)?;
        let id = session.state().reserve()?;
        session.dispatch(Operation::ClockRun {
            generator: clock.id,
            id,
            first,
            period,
        })?;
        Ok(id)
    }
    fn clock_stop(&mut self, clock: ClockSource, time: u64) -> Result<()> {
        let mut session = self.session();
        session.state().check(clock.origin)?;
        session.dispatch(Operation::ClockStop {
            generator: clock.id,
            time,
        })
    }
    #[inline]
    fn emit_u64(&mut self, signal: SignalId, value: u64) -> Result<()> {
        let mut session = self.session();
        session.dispatch_signal(signal, SignalInput::Unsigned(value))
    }
    /// Copy scalar updates into one ordered capture. All signals must be valid
    /// bit vectors at most 64 bits wide before any sample in the batch is recorded.
    fn emit_batch(&mut self, updates: &[SignalUpdate]) -> Result<()> {
        let mut session = self.session();
        session.dispatch(Operation::EmitBatch(updates.to_vec()))
    }
    fn emit_packed(&mut self, signal: SignalId, alphabet: u8, bytes: &[u8]) -> Result<()> {
        let mut session = self.session();
        session.dispatch_signal_ref(
            signal,
            SignalRef::Packed {
                alphabet,
                width: None,
                bytes,
            },
        )
    }
    fn emit_words(&mut self, signal: SignalId, width: u32, words: &[u32]) -> Result<()> {
        let mut session = self.session();
        session.dispatch_words(signal, width, words)
    }

    #[inline]
    fn emit_real(&mut self, signal: SignalId, value: f64) -> Result<()> {
        let mut session = self.session();
        session.dispatch_signal(signal, SignalInput::Real(value.to_bits()))
    }
    fn emit_bytes(&mut self, signal: SignalId, bytes: &[u8]) -> Result<()> {
        let mut session = self.session();
        session.dispatch_signal_ref(signal, SignalRef::Bytes(bytes))
    }
    /// Enable or disable waveform capture at the current producer time.
    /// Transitions emit blackout markers; transactions, clocks and logs continue.
    fn dump(&mut self, enabled: bool) -> Result<()> {
        let mut session = self.session();
        session.dispatch(Operation::Dump(enabled))
    }
    /// Annotate an imported waveform gap without changing capture state.
    fn blackout(&mut self, time: u64, active: bool) -> Result<()> {
        let mut session = self.session();
        session.dispatch(Operation::Blackout { time, active })
    }
}

#[inline]
fn begin_transaction<T: Payload>(
    recorder: &mut Session<'_>,
    source: Source<T>,
    time: u64,
    payload: Option<&T>,
    parent: Option<OwnerRef>,
) -> Result<Transaction<T>> {
    recorder.state().check(source.origin)?;
    if let Some(parent) = parent {
        recorder.state().check(parent.origin)?;
    }
    let action = PayloadOperation::Begin {
        generator: source.id,
        time,
        parent: parent.map_or(0, |parent| parent.id),
    };
    let id = if let Some(payload) = payload {
        recorder.payload(action, payload)?
    } else {
        let id = recorder.state().reserve()?;
        recorder.dispatch_payload(action, id)?;
        id
    };
    Ok(Transaction {
        id,
        origin: source.origin,
        marker: PhantomData,
    })
}
#[inline]
fn begin_stage<T: Payload>(
    recorder: &mut Session<'_>,
    source: Source<T, source::Stage>,
    owner: OwnerRef,
    time: u64,
    payload: Option<&T>,
) -> Result<Stage<T>> {
    recorder.state().check(source.origin)?;
    recorder.state().check(owner.origin)?;
    let action = PayloadOperation::BeginStage {
        generator: source.id,
        time,
        parent: owner.id,
    };
    let id = if let Some(payload) = payload {
        recorder.payload(action, payload)?
    } else {
        let id = recorder.state().reserve()?;
        recorder.dispatch_payload(action, id)?;
        id
    };
    Ok(Stage {
        id,
        origin: source.origin,
        marker: PhantomData,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_and_racing_shared_ids_exhaust_without_reusing_zero_or_live_ids() {
        let local = RecordIds::Local(Cell::new(u64::MAX - 1));
        assert_eq!(local.reserve().unwrap(), u64::MAX - 1);
        assert_eq!(local.reserve().unwrap(), u64::MAX);
        assert!(local.reserve().is_err());
        assert!(local.reserve().is_err());

        let shared = Arc::new(AtomicU64::new(u64::MAX - 1));
        let mut ids = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..3)
                .map(|_| {
                    let source = RecordIds::Shared(shared.clone());
                    scope.spawn(move || source.reserve().ok())
                })
                .collect();
            handles
                .into_iter()
                .filter_map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        ids.sort_unstable();
        assert_eq!(ids, [u64::MAX - 1, u64::MAX]);
        assert!(RecordIds::Shared(shared).reserve().is_err());
    }
}
