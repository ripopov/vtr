//! Click selection with platform conventions, shared by every list.

use std::collections::BTreeSet;

use crate::geometry::Modifiers;

/// Apply a click: shift extends the anchor range, cmd/ctrl toggles, plain replaces.
pub fn select(
    selected: &mut BTreeSet<usize>,
    anchor: &mut Option<usize>,
    row: usize,
    modifiers: Modifiers,
) {
    if modifiers.shift {
        let a = anchor.unwrap_or(row);
        selected.extend(a.min(row)..=a.max(row));
    } else {
        if modifiers.secondary() {
            if !selected.remove(&row) {
                selected.insert(row);
            }
        } else {
            selected.clear();
            selected.insert(row);
        }
        *anchor = Some(row);
    }
}
