//! Exact interval unions for generator activity.

use super::{BuildOptions, Source};
use crate::{Error, Result};
use std::collections::BTreeMap;

pub(super) type Ranges = BTreeMap<(u32, u64), u64>;

pub(super) fn collect(source: &impl Source, opts: &BuildOptions, base: u64) -> Result<Ranges> {
    let mut ranges = Ranges::new();
    let workspace = std::cell::Cell::new(0u64);
    let entries = std::cell::Cell::new(0usize);
    let check = |count: usize, raw: u64| -> Result<()> {
        if let Some(control) = &opts.control {
            control.check()?;
        }
        // Includes B-tree nodes and the bounded encoding workspace retained at publication.
        if base
            .saturating_add((count as u64).saturating_mul(128))
            .saturating_add(raw)
            > opts.memory
        {
            return Err(Error::invalid(
                "generator activity exceeds the memory limit",
            ));
        }
        Ok(())
    };
    source.generator_activity(
        &mut |raw| {
            workspace.set((raw as u64).saturating_mul(2));
            check(entries.get(), workspace.get())
        },
        &mut |generator, mut begin, mut end| {
            if begin > end {
                return Err(Error::Corrupt("reversed generator activity"));
            }
            if let Some((&(id, first), &last)) = ranges.range(..=(generator, begin)).next_back()
                && id == generator
                && last >= begin.saturating_sub(1)
            {
                if end <= last {
                    return check(ranges.len(), workspace.get());
                }
                begin = first;
                end = end.max(last);
            }
            while let Some((&(id, first), &last)) = ranges
                .range((
                    std::ops::Bound::Excluded((generator, begin)),
                    std::ops::Bound::Unbounded,
                ))
                .next()
                && id == generator
                && first <= end.saturating_add(1)
            {
                end = end.max(last);
                ranges.remove(&(id, first));
            }
            check(ranges.len() + 1, workspace.get())?;
            ranges.insert((generator, begin), end);
            entries.set(ranges.len());
            Ok(())
        },
    )?;
    Ok(ranges)
}
