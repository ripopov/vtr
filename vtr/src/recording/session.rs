//! A capture scope borrows mutable frontend state independently of its panic guard.

use std::sync::Arc;

use super::{
    Backend, CrashState, Dispatch, Operation, Recorder, SignalInput,
    capture::{Capture, Frontend, PayloadOperation, SignalRef},
    channel,
    crash::Scoped,
    transport::Shared,
};
use crate::{Error, Result, SignalId, schema::Payload};

enum Sink<'a> {
    Inline(&'a mut Backend),
    Threaded(&'a mut channel::Sender, &'a Arc<Shared>),
}

pub struct Session<'a> {
    front: &'a mut Frontend,
    sink: Sink<'a>,
    failure: Option<&'a mut Option<String>>,
    closed: bool,
    crash: Option<&'a CrashState>,
    _guard: Option<Scoped<'a>>,
}

impl<'a> Session<'a> {
    #[inline]
    pub(super) fn writer(
        front: &'a mut Frontend,
        dispatch: &'a mut Dispatch,
        failure: &'a mut Option<String>,
        closed: bool,
        crash: &'a CrashState,
    ) -> Self {
        let sink = match dispatch {
            Dispatch::Inline(backend) => Sink::Inline(backend),
            Dispatch::Threaded(transport) => {
                let (sender, shared) = transport.capture_parts();
                Sink::Threaded(sender, shared)
            }
        };
        Self {
            front,
            sink,
            failure: Some(failure),
            closed,
            crash: Some(crash),
            _guard: Some(crash.scoped()),
        }
    }
    #[inline]
    pub(super) fn producer(
        front: &'a mut Frontend,
        sender: &'a mut channel::Sender,
        shared: &'a Arc<Shared>,
    ) -> Self {
        Self {
            front,
            sink: Sink::Threaded(sender, shared),
            failure: None,
            closed: false,
            crash: None,
            _guard: None,
        }
    }
    #[inline]
    pub fn check(&self) -> Result<()> {
        if self.closed {
            return Err(Error::State("recording is finalized"));
        }
        if self.crash.is_some_and(CrashState::poisoned) {
            return Err(Error::State("recording frontend panicked"));
        }
        if let Sink::Threaded(_, shared) = &self.sink {
            shared.error()?;
        }
        if let Some(Some(error)) = &self.failure {
            return Err(Error::Recording(error.clone()));
        }
        if self.front.finished {
            return Err(Error::State("producer is finished"));
        }
        Ok(())
    }
    #[inline]
    pub fn state_mut(&mut self) -> &mut Frontend {
        self.front
    }
    #[inline]
    fn result<T>(&mut self, result: Result<T>) -> Result<T> {
        if let Err(error) = &result {
            self.record_failure(error);
        }
        result
    }
    #[cold]
    #[inline(never)]
    fn record_failure(&mut self, error: &Error) {
        if let Some(failure) = &mut self.failure {
            **failure = Some(error.to_string());
        }
    }
    #[inline]
    pub fn payload<T: Payload>(&mut self, action: PayloadOperation, payload: &T) -> Result<u64> {
        self.check()?;
        let hint = T::fixed_size().unwrap_or(0);
        if hint > u32::MAX as usize {
            return Err(Error::invalid("payload exceeds u32"));
        }
        let result = match &mut self.sink {
            Sink::Inline(backend) => {
                let mut bytes = backend.payload_buffer(hint);
                payload.encode(&mut bytes)?;
                if bytes.len() > u32::MAX as usize {
                    return Err(Error::invalid("payload exceeds u32"));
                }
                let id = action.record_id(self.front)?;
                backend
                    .apply_owned_payload(
                        self.front.producer,
                        self.front.time,
                        action,
                        id,
                        Some(bytes),
                    )
                    .map(|()| id)
            }
            Sink::Threaded(sender, shared) => {
                self.front.payload.clear();
                self.front.payload.reserve(hint);
                payload.encode(&mut self.front.payload)?;
                if self.front.payload.len() > u32::MAX as usize {
                    return Err(Error::invalid("payload exceeds u32"));
                }
                let id = action.record_id(self.front)?;
                let result = if sender.payload_fits(Some(&self.front.payload)) {
                    sender.publish_payload(action, id, Some(&self.front.payload), || shared.error())
                } else {
                    sender.publish_owned(
                        action.owned(id, Some(std::mem::take(&mut self.front.payload))),
                        || shared.error(),
                    )
                };
                if self.front.payload.capacity() > 64 << 10 {
                    self.front.payload = Vec::new();
                }
                result.map(|()| {
                    shared.captured();
                    id
                })
            }
        };
        self.result(result)
    }
    #[inline]
    pub fn dispatch_progress(&mut self, time: u64) -> Result<()> {
        self.check()?;
        let result = match &mut self.sink {
            Sink::Inline(backend) => backend.advance(time),
            Sink::Threaded(sender, shared) => sender
                .publish_progress(time, || shared.error())
                .map(|()| shared.captured()),
        };
        self.result(result)
    }
    #[inline]
    pub fn dispatch_payload(&mut self, action: PayloadOperation, id: u64) -> Result<()> {
        self.check()?;
        let result = match &mut self.sink {
            Sink::Inline(backend) => {
                backend.apply_owned_payload(self.front.producer, self.front.time, action, id, None)
            }
            Sink::Threaded(sender, shared) => sender
                .publish_payload(action, id, None, || shared.error())
                .map(|()| shared.captured()),
        };
        self.result(result)
    }
    #[inline]
    pub fn dispatch(&mut self, operation: Operation) -> Result<()> {
        self.check()?;
        let result = match &mut self.sink {
            Sink::Inline(backend) => backend.apply(self.front.producer, self.front.time, operation),
            Sink::Threaded(sender, shared) => sender
                .publish(operation, || shared.error())
                .map(|()| shared.captured()),
        };
        self.result(result)
    }
    pub fn dispatch_words(&mut self, signal: SignalId, width: u32, words: &[u32]) -> Result<()> {
        if width == 0 {
            return Err(Error::invalid("zero-width word emission"));
        }
        if words.len() < (width as usize).div_ceil(32) {
            return Err(Error::invalid("word emission has insufficient input words"));
        }
        self.check()?;
        let mut bytes = std::mem::take(&mut self.front.payload);
        bytes.clear();
        bytes.resize((width as usize).div_ceil(8), 0);
        crate::signal::pack_words32(words, width, &mut bytes);
        let result = self.dispatch_signal_ref(
            signal,
            SignalRef::Packed {
                alphabet: 2,
                width: Some(width),
                bytes: &bytes,
            },
        );
        if bytes.capacity() <= 64 << 10 {
            self.front.payload = bytes;
        }
        result
    }
    pub fn dispatch_signal_ref(&mut self, signal: SignalId, value: SignalRef<'_>) -> Result<()> {
        self.check()?;
        let result = match &mut self.sink {
            Sink::Inline(backend) => {
                backend.emit_signal_ref(self.front.producer, self.front.time, signal, value)
            }
            Sink::Threaded(sender, shared) => sender
                .publish_signal_ref(signal, value, || shared.error())
                .map(|()| shared.captured()),
        };
        self.result(result)
    }
    #[inline]
    pub fn dispatch_signal(&mut self, signal: SignalId, value: SignalInput) -> Result<()> {
        self.check()?;
        let result = match &mut self.sink {
            Sink::Inline(backend) => {
                backend.emit_signal(self.front.producer, self.front.time, signal, value)
            }
            Sink::Threaded(sender, shared) => sender
                .publish_signal(signal, value, || shared.error())
                .map(|()| shared.captured()),
        };
        self.result(result)
    }
}

impl Capture for Session<'_> {
    fn state(&self) -> &Frontend {
        self.front
    }
    #[inline]
    fn session(&mut self) -> Session<'_> {
        let sink = match &mut self.sink {
            Sink::Inline(backend) => Sink::Inline(backend),
            Sink::Threaded(sender, shared) => Sink::Threaded(sender, shared),
        };
        Session {
            front: self.front,
            sink,
            failure: self.failure.as_deref_mut(),
            closed: self.closed,
            crash: self.crash,
            _guard: None,
        }
    }
}
impl Recorder for Session<'_> {
    fn intern(&mut self, name: &str) -> Result<u32> {
        self.check()?;
        let result = match &mut self.sink {
            Sink::Inline(backend) => backend.intern(name),
            Sink::Threaded(_, shared) => {
                let name = name.to_owned();
                shared.control(move |backend| backend.intern(&name))
            }
        };
        self.result(result)
    }
}
