//! Incremental, output-capped construction over the ordinary reader columns.
use super::*;

/// Exhaustion and refusal are distinct: a partial history is never published.
pub enum HistoryProgress {
    Pending,
    Complete(SignalData),
    BudgetExceeded,
}

/// One signal's construction cursor. Drop it between steps to cancel.
/// The reader must outlive the cursor; completed histories own their storage.
pub struct HistoryLoad<'a> {
    reader: &'a Reader,
    signal: SignalId,
    bytes: usize,
    block: usize,
    first_seen: bool,
    builder: Option<SignalDataBuilder>,
    complete: Option<SignalData>,
    failed: bool,
}

impl<'a> HistoryLoad<'a> {
    pub(super) fn new(reader: &'a Reader, signal: SignalId, bytes: usize) -> Result<Self> {
        let kind = reader.signal_kind(signal)?;
        // Check before allocating even the default value of a very wide signal.
        let minimum = std::mem::size_of::<SignalDataInner>()
            .saturating_add(kind.packed_len().unwrap_or(0))
            .saturating_add(std::mem::size_of::<u32>());
        let mut builder = if minimum <= bytes {
            let mut initial = Vec::new();
            initial.try_reserve_exact(kind.packed_len().unwrap_or(0))
                .map_err(|_| Error::State("history allocation failed"))?;
            signal::default_value(kind, &mut initial);
            Some(SignalDataBuilder { kind, initial, times: Vec::new(), data: Vec::new(), offsets: vec![0] })
        } else {
            None
        };
        if let Some(b) = &mut builder {
            if !prepare(b, b.initial.len(), 0, 0, 1, bytes)? {
                builder = None;
            }
        }
        Ok(Self { reader, signal, bytes, block: 0, first_seen: false,
            builder, complete: None, failed: false })
    }

    /// Process at most `blocks` file blocks (nonzero). Check cancellation between
    /// calls. A block includes decompression and two column walks: sizing, then
    /// ordinary append. This is not a wall-time or decoder-scratch bound.
    /// Completed/refused results are repeatable; a decode error is terminal.
    pub fn advance(&mut self, blocks: usize) -> Result<HistoryProgress> {
        if blocks == 0 {
            return Err(Error::invalid("history work budget must be nonzero"));
        }
        if self.failed {
            return Err(Error::State("history load cannot resume after an error"));
        }
        let result = self.advance_inner(blocks);
        if result.is_err() {
            self.failed = true;
            self.builder = None;
        }
        result
    }

    fn advance_inner(&mut self, blocks: usize) -> Result<HistoryProgress> {
        if let Some(history) = &self.complete {
            return Ok(HistoryProgress::Complete(history.clone()));
        }
        let Some(builder) = &mut self.builder else {
            return Ok(HistoryProgress::BudgetExceeded);
        };
        let end = self.block.saturating_add(blocks).min(self.reader.sig_blocks.len());
        let group = self.reader.group_of(self.signal);
        while self.block < end {
            let bi = self.block;
            self.block += 1;
            let Some(view) = self.reader.group_view(bi, group)? else { continue };
            if !self.first_seen {
                let frame = self.reader.piece(bi, group, &view, 0)?;
                let (start, end) = frame.ranges[(self.signal.0 - view.first_sig) as usize];
                let initial = &frame.data[start as usize..end as usize];
                if !prepare(builder, initial.len(), builder.times.len(), builder.data.len(), builder.offsets.len(), self.bytes)? {
                    self.builder = None;
                    return Ok(HistoryProgress::BudgetExceeded);
                }
                builder.initial.clear();
                builder.initial.extend_from_slice(initial);
                self.first_seen = true;
            }
            let (piece, local) = self.reader.column(bi, group, &view, self.signal.0)?;
            let column = piece.col(local, builder.kind)?;
            let mut iter = ColumnIter::new(column, builder.kind);
            let mut entries = builder.times.len();
            let mut data = builder.data.len();
            let mut offsets = builder.offsets.len();
            while let Some(change) = iter.next_raw()? {
                entries = entries.checked_add(1).ok_or(Error::State("history size overflow"))?;
                let payload = builder.kind.packed_len().unwrap_or_else(|| iter.value_bytes(&change).len());
                data = data.checked_add(payload).ok_or(Error::State("history size overflow"))?;
                if builder.kind == SignalKind::VarLen {
                    // SignalData's offsets are u32; never truncate a large history.
                    if data > u32::MAX as usize {
                        self.builder = None;
                        return Ok(HistoryProgress::BudgetExceeded);
                    }
                    offsets = offsets.checked_add(1).ok_or(Error::State("history size overflow"))?;
                }
                let minimum = std::mem::size_of::<SignalDataInner>()
                    .saturating_add(builder.initial.capacity())
                    .saturating_add(entries.saturating_mul(8))
                    .saturating_add(data)
                    .saturating_add(offsets.saturating_mul(4));
                if minimum > self.bytes {
                    self.builder = None;
                    return Ok(HistoryProgress::BudgetExceeded);
                }
            }
            if !prepare(builder, builder.initial.len(), entries, data, offsets, self.bytes)? {
                self.builder = None;
                return Ok(HistoryProgress::BudgetExceeded);
            }
            if !column.is_empty() {
                builder.append_column(column, &self.reader.block_times(bi)?)?;
            }
        }
        if self.block == self.reader.sig_blocks.len() {
            let history = self.builder.take().unwrap().finish();
            if history.retained_bytes() > self.bytes {
                return Ok(HistoryProgress::BudgetExceeded);
            }
            self.complete = Some(history.clone());
            Ok(HistoryProgress::Complete(history))
        } else {
            Ok(HistoryProgress::Pending)
        }
    }
}

// Prepay both old and replacement buffers during a possible moving realloc.
// Prefer geometric growth, falling back to exact capacity near the ceiling.
fn prepare(b: &mut SignalDataBuilder, initial: usize, times: usize, data: usize, offsets: usize, limit: usize) -> Result<bool> {
    let old = [b.initial.capacity(), b.times.capacity(), b.data.capacity(), b.offsets.capacity()];
    let needed = [initial, times, data, offsets];
    let units = [1usize, 8, 1, 4];
    let fits = |target: &[usize; 4]| {
        let mut total = std::mem::size_of::<SignalDataInner>();
        for i in 0..4 {
            total = total.saturating_add(old[i].saturating_mul(units[i]));
            if target[i] > old[i] {
                total = total.saturating_add(target[i].saturating_mul(units[i]));
            }
        }
        total <= limit
    };
    let exact = std::array::from_fn(|i| needed[i].max(old[i]));
    let growth = std::array::from_fn(|i| if needed[i] > old[i] { needed[i].max(old[i].saturating_mul(2)) } else { old[i] });
    let target = if fits(&growth) { growth } else if fits(&exact) { exact } else { return Ok(false) };
    fn reserve<T>(values: &mut Vec<T>, capacity: usize) -> Result<()> {
        if capacity > values.capacity() {
            values.try_reserve_exact(capacity - values.len()).map_err(|_| Error::State("history allocation failed"))?;
        }
        Ok(())
    }
    reserve(&mut b.initial, target[0])?;
    reserve(&mut b.times, target[1])?;
    reserve(&mut b.data, target[2])?;
    reserve(&mut b.offsets, target[3])?;
    // Vec may report more capacity than requested; do not retain an oversize builder.
    Ok(std::mem::size_of::<SignalDataInner>()
        .saturating_add(b.initial.capacity())
        .saturating_add(b.times.capacity().saturating_mul(8))
        .saturating_add(b.data.capacity())
        .saturating_add(b.offsets.capacity().saturating_mul(4)) <= limit)
}
