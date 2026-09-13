//! Native session execution. Run this state on a bounded worker, never in a
//! viewer frame. Both in-process delivery and the stdio server use these typed
//! operations; this module never invokes a wire codec.
use crate::session::{Continuation, Delivery, Query, Reply, SessionInfo, SnapshotId};
use crate::wave::{Bytes, Limits};
use crate::{
    native::{Summary, Window},
    native_metadata::{text_part, Children, ResolvePaths, Search},
};
use crate::{Budget, Cancellation, Error, Interval, Result, TimeBound};
use std::sync::Arc;
use vtr::Reader;

enum Operation<'a> {
    Window(Window<'a>),
    Summary(Summary<'a>),
    Children(Children<'a>),
    Search(Search<'a>),
    Resolve(ResolvePaths<'a>),
    Text { id: u32, offset: u64, length: usize },
}
impl Operation<'_> {
    fn next(
        &mut self,
        reader: &Reader,
        budget: &Budget,
        cancellation: &Cancellation,
    ) -> Result<Reply> {
        match self {
            Self::Window(query) => query.next_page().map(Reply::Window),
            Self::Summary(query) => query.next_page().map(Reply::Summary),
            Self::Children(query) => query.next_page().map(Reply::Children),
            Self::Search(query) => query.next_page().map(Reply::Search),
            Self::Resolve(query) => query.next_page().map(Reply::Resolve),
            Self::Text { id, offset, length } => {
                text_part(reader, *id, *offset, *length, budget, cancellation).map(Reply::Text)
            }
        }
    }
}
struct Slot<'a> {
    operation: Operation<'a>,
    cancellation: Cancellation,
    next_step: u64,
    complete: bool,
    last: Option<Arc<Delivery>>,
}

pub struct Session<'a> {
    reader: &'a Reader,
    info: SessionInfo,
    budget: Budget,
    slots: Vec<Option<(u64, Slot<'a>)>>,
    _slots_charge: crate::Reservation,
    next_operation: u64,
}
impl<'a> Session<'a> {
    pub fn new(reader: &'a Reader, budget: Budget, max_operations: usize) -> Result<Self> {
        if max_operations == 0 {
            return Err(Error::Invalid("session needs operation capacity"));
        }
        let charge = budget.reserve(
            max_operations
                .checked_mul(std::mem::size_of::<Option<(u64, Slot<'a>)>>())
                .ok_or(Error::ResourceLimit)?,
        )?;
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(max_operations)
            .map_err(|_| Error::ResourceLimit)?;
        slots.resize_with(max_operations, || None);
        let time_range = reader
            .time_range()
            .map(|(start, end)| {
                Interval::new(
                    start,
                    end.checked_add(1)
                        .map_or(TimeBound::AfterMax, TimeBound::Tick),
                )
            })
            .transpose()?;
        Ok(Self {
            reader,
            info: SessionInfo {
                snapshot: SnapshotId(rand::random()),
                timescale: reader.meta().timescale,
                time_range,
                signals: reader.signal_count(),
                declarations: reader.hierarchy().len() as u64,
            },
            budget,
            slots,
            _slots_charge: charge,
            next_operation: 0,
        })
    }
    pub fn info(&self) -> &SessionInfo {
        &self.info
    }
    pub fn active_operations(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_some()).count()
    }

    /// Admit one operation. Supply a cancellation token that the host can set
    /// concurrently while a native scan is running on this worker.
    pub fn start(
        &mut self,
        query: Query,
        limits: Limits,
        cancellation: Cancellation,
    ) -> Result<Continuation> {
        cancellation.check()?;
        let limits = Limits {
            bytes: limits
                .bytes
                .checked_sub(std::mem::size_of::<Delivery>())
                .ok_or(Error::ResourceLimit)?,
            ..limits
        };
        let index = self
            .slots
            .iter()
            .position(Option::is_none)
            .ok_or(Error::ResourceLimit)?;
        let operation_id = self
            .next_operation
            .checked_add(1)
            .ok_or(Error::ResourceLimit)?;
        let operation = match query {
            Query::Window { signal, interval } => Operation::Window(Window::new(
                self.reader,
                signal,
                interval,
                limits,
                self.budget.clone(),
                cancellation.clone(),
            )?),
            Query::Summary { signal, grid } => Operation::Summary(Summary::new(
                self.reader,
                signal,
                grid,
                limits,
                self.budget.clone(),
                cancellation.clone(),
            )?),
            Query::Children { parent } => Operation::Children(Children::new(
                self.reader,
                parent,
                limits,
                self.budget.clone(),
                cancellation.clone(),
            )?),
            Query::Search { scope, needle } => Operation::Search(Search::new(
                self.reader,
                scope,
                &needle,
                limits,
                self.budget.clone(),
                cancellation.clone(),
            )?),
            Query::Resolve { paths } => Operation::Resolve(ResolvePaths::new(
                self.reader,
                paths,
                limits,
                self.budget.clone(),
                cancellation.clone(),
            )?),
            Query::Text { id, offset, length } => {
                let overhead =
                    std::mem::size_of::<crate::metadata::TextPart>() + Bytes::retained_size(0)?;
                if length == 0 || length > limits.bytes.saturating_sub(overhead) {
                    return Err(Error::ResourceLimit);
                }
                Operation::Text { id, offset, length }
            }
        };
        self.next_operation = operation_id;
        self.slots[index] = Some((
            operation_id,
            Slot {
                operation,
                cancellation,
                next_step: 0,
                complete: false,
                last: None,
            },
        ));
        Ok(Continuation {
            snapshot: self.info.snapshot,
            operation: operation_id,
            step: 0,
        })
    }

    /// Produce one page. Retrying its cursor returns the same immutable delivery
    /// until the caller advances again or releases the operation. Requesting the
    /// next page acknowledges the previous one and permits its eviction.
    pub fn advance(&mut self, cursor: Continuation) -> Result<Arc<Delivery>> {
        self.validate(cursor)?;
        let (_, slot) = self
            .slots
            .iter_mut()
            .flatten()
            .find(|(id, _)| *id == cursor.operation)
            .ok_or(Error::Invalid("unknown or released operation"))?;
        slot.cancellation.check()?;
        if let Some(last) = &slot.last {
            if last.request == cursor {
                return Ok(last.clone());
            }
        }
        if cursor.step != slot.next_step || slot.complete {
            return Err(Error::Invalid("stale or future continuation"));
        }
        let next_step = cursor.step.checked_add(1).ok_or(Error::ResourceLimit)?;
        slot.last = None;
        let charge = self.budget.reserve(std::mem::size_of::<Delivery>())?;
        let result = slot
            .operation
            .next(self.reader, &self.budget, &slot.cancellation);
        match result {
            Ok(reply) => {
                slot.complete = reply.complete();
                slot.next_step = next_step;
                let next = (!slot.complete).then_some(Continuation {
                    step: next_step,
                    ..cursor
                });
                let delivery = Arc::new(Delivery {
                    request: cursor,
                    next,
                    reply,
                    _charge: charge,
                });
                slot.last = Some(delivery.clone());
                Ok(delivery)
            }
            Err(Error::ResourceLimit) => Err(Error::ResourceLimit),
            Err(error) => {
                self.release(cursor)?;
                Err(error)
            }
        }
    }
    pub(crate) fn cancellation(&self, cursor: Continuation) -> Result<Cancellation> {
        self.validate(cursor)?;
        self.slots
            .iter()
            .flatten()
            .find(|(id, _)| *id == cursor.operation)
            .map(|(_, slot)| slot.cancellation.clone())
            .ok_or(Error::Invalid("unknown or released operation"))
    }
    fn validate(&self, cursor: Continuation) -> Result<()> {
        if cursor.snapshot != self.info.snapshot {
            return Err(Error::Invalid("continuation belongs to another snapshot"));
        }
        Ok(())
    }
    /// Idempotent release/cancel. Completed operations retain one retryable page
    /// until explicitly released; they still count against operation capacity.
    pub fn release(&mut self, cursor: Continuation) -> Result<()> {
        self.validate(cursor)?;
        if let Some(slot) = self
            .slots
            .iter_mut()
            .find(|slot| slot.as_ref().is_some_and(|(id, _)| *id == cursor.operation))
        {
            if let Some((_, slot)) = slot.take() {
                slot.cancellation.cancel();
            }
        }
        Ok(())
    }
}
impl Drop for Session<'_> {
    fn drop(&mut self) {
        for (_, slot) in self.slots.iter().flatten() {
            slot.cancellation.cancel();
        }
    }
}
