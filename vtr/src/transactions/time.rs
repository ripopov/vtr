use super::TransactionBounds;
use crate::{
    Error, Result,
    varint::unzigzag,
    wire::{self, Cursor},
};

const COLUMNS: [usize; 7] = [2, 3, 13, 14, 16, 19, 20];

struct TimeColumns<'a> {
    columns: [Cursor<'a>; 7],
    previous_begin: u64,
}

impl<'a> TimeColumns<'a> {
    fn new(columns: &[&'a [u8]; 33]) -> Self {
        Self {
            columns: COLUMNS.map(|i| Cursor::new(columns[i])),
            previous_begin: 0,
        }
    }

    fn owner(&mut self) -> Result<(u64, u64)> {
        let begin = self
            .previous_begin
            .wrapping_add(unzigzag(self.columns[0].var()?) as u64);
        let end = begin
            .checked_add(self.columns[1].var()?)
            .ok_or(Error::Corrupt("transaction end overflow"))?;
        self.previous_begin = begin;
        Ok((begin, end))
    }

    fn event_count(&mut self) -> Result<u64> {
        let count = self.columns[2].var()?;
        if count > self.columns[3].remaining() as u64 {
            return Err(Error::Corrupt("invalid embedded event count"));
        }
        Ok(count)
    }

    fn event(&mut self, owner_begin: u64) -> Result<u64> {
        Ok(owner_begin.wrapping_add(unzigzag(self.columns[3].var()?) as u64))
    }

    fn stage_count(&mut self) -> Result<u64> {
        let count = self.columns[4].var()?;
        if count > self.columns[5].remaining().min(self.columns[6].remaining()) as u64 {
            return Err(Error::Corrupt("invalid embedded stage count"));
        }
        Ok(count)
    }

    fn stage(&mut self, owner_begin: u64) -> Result<(u64, Option<u64>)> {
        let begin = owner_begin.wrapping_add(unzigzag(self.columns[5].var()?) as u64);
        Ok((begin, stage_end(&mut self.columns[6], begin)?))
    }

    fn done(self) -> Result<()> {
        for column in self.columns {
            column.done()?;
        }
        Ok(())
    }
}

// Duration plus one needs one additional value for the full u64 time domain.
pub(super) const FULL_DOMAIN_END: [u8; 10] = [128, 128, 128, 128, 128, 128, 128, 128, 128, 2];

pub(super) fn stage_end(cursor: &mut Cursor<'_>, begin: u64) -> Result<Option<u64>> {
    if cursor.starts_with(&FULL_DOMAIN_END) {
        cursor.take(10)?;
        if begin != 0 {
            return Err(Error::Corrupt("full-domain stage end overflows"));
        }
        return Ok(Some(u64::MAX));
    }
    let encoded = cursor.var()?;
    if encoded == 0 {
        return Ok(None);
    }
    begin
        .checked_add(encoded - 1)
        .map(Some)
        .ok_or(Error::Corrupt("embedded stage end overflows"))
}

pub(super) fn range(raw: &[u8], expected: TransactionBounds) -> Result<(u64, u64)> {
    let columns = wire::unpack_columns::<33>(raw)?;
    if expected.transactions > columns[2].len() as u64 {
        return Err(Error::Corrupt("invalid transaction page count"));
    }
    let mut times = TimeColumns::new(&columns);
    let mut owners = (u64::MAX, 0);
    let mut range = (expected.begin, expected.end);
    let mut embedded = false;
    for _ in 0..expected.transactions {
        let (begin, end) = times.owner()?;
        owners = (owners.0.min(begin), owners.1.max(end));
        let count = times.event_count()?;
        embedded |= count != 0;
        for _ in 0..count {
            let time = times.event(begin)?;
            range = (range.0.min(time), range.1.max(time));
        }
        let count = times.stage_count()?;
        embedded |= count != 0;
        for _ in 0..count {
            let (start, finish) = times.stage(begin)?;
            range = (
                range.0.min(start),
                range.1.max(finish.unwrap_or(end).max(start)),
            );
        }
    }
    times.done()?;
    if owners != (expected.begin, expected.end) || !embedded {
        return Err(Error::Corrupt("transaction time bounds disagree"));
    }
    Ok(range)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::varint::{put_i64, put_u64};

    #[test]
    fn time_summaries_reject_unbounded_counts_overflow_trailing_data_and_false_bounds() {
        let bounds = TransactionBounds {
            transactions: 1,
            begin: 100,
            end: 110,
            max_embedded: 3,
            ..Default::default()
        };
        let valid = || {
            let mut columns: [Vec<u8>; 33] = std::array::from_fn(|_| vec![]);
            for (column, value) in [
                (2, 200),
                (3, 10),
                (13, 1),
                (14, 160),
                (16, 1),
                (19, 99),
                (20, 111),
            ] {
                put_u64(&mut columns[column], value);
            }
            columns
        };
        let summarize =
            |columns: &[Vec<u8>; 33], bounds| range(&wire::pack_columns(columns).unwrap(), bounds);
        assert_eq!(summarize(&valid(), bounds).unwrap(), (50, 180));
        for column in [13, 16] {
            let mut columns = valid();
            columns[column].clear();
            put_u64(&mut columns[column], u64::MAX);
            assert!(summarize(&columns, bounds).is_err());
        }
        for column in COLUMNS {
            let mut columns = valid();
            columns[column].push(0);
            assert!(
                summarize(&columns, bounds).is_err(),
                "trailing column {column}"
            );
        }
        let mut columns = valid();
        columns[2].clear();
        put_i64(&mut columns[2], (u64::MAX - 3) as i64);
        assert!(summarize(&columns, bounds).is_err());
        let mut columns = valid();
        columns[20].clear();
        columns[20].extend_from_slice(&FULL_DOMAIN_END);
        assert!(summarize(&columns, bounds).is_err());
        let mut columns = valid();
        columns[20].clear();
        put_u64(&mut columns[20], u64::MAX);
        assert!(summarize(&columns, bounds).is_err());
        assert!(
            summarize(
                &valid(),
                TransactionBounds {
                    begin: 99,
                    ..bounds
                }
            )
            .is_err()
        );
        assert!(
            summarize(
                &valid(),
                TransactionBounds {
                    transactions: u64::MAX,
                    ..bounds
                }
            )
            .is_err()
        );
    }
}
