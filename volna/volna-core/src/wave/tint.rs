//! Row colours (`docs/RATIONALE.md`, "Volna row colours"): a row or group stores one of a few
//! named theme colours, never RGB, and the theme resolves the name per
//! appearance at the contrast floor of every wave stroke.
//!
//! Red and yellow are not offered, so a colour never passes for X or Z.

use super::tree::{self, Entry};

/// A named row colour. A row without one uses its group's, else the
/// theme's default signal colour (green), which the menu calls Default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tint {
    Blue,
    Cyan,
    Violet,
    Pink,
    Grey,
}

impl Tint {
    /// Every colour, in menu order after Default.
    pub const ALL: [Tint; 5] = [Tint::Blue, Tint::Cyan, Tint::Violet, Tint::Pink, Tint::Grey];

    /// The position in [`Tint::ALL`] and in `Theme::wave_tints`.
    pub fn index(self) -> usize {
        self as usize
    }

    /// The name a workspace stores and commands use.
    pub fn id(self) -> &'static str {
        match self {
            Tint::Blue => "blue",
            Tint::Cyan => "cyan",
            Tint::Violet => "violet",
            Tint::Pink => "pink",
            Tint::Grey => "grey",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.id() == id)
    }

    /// The menu and palette label; `None` is Default.
    pub fn label(tint: Option<Self>) -> &'static str {
        match tint {
            None => "Default",
            Some(Tint::Blue) => "Blue",
            Some(Tint::Cyan) => "Cyan",
            Some(Tint::Violet) => "Violet",
            Some(Tint::Pink) => "Pink",
            Some(Tint::Grey) => "Grey",
        }
    }
}

/// The colour entry `i` is drawn in: its own, else the nearest enclosing
/// group's, else `None` for the theme default.
pub fn ink(items: &[Entry], i: usize) -> Option<Tint> {
    let mut at = i;
    loop {
        if let Some(t) = items.get(at)?.row.tint() {
            return Some(t);
        }
        at = tree::parent(items, at)?;
    }
}

/// Stored colours: a name, where an unknown one reads as Default so a
/// workspace from a later palette still opens.
pub(crate) mod serde_name {
    use super::Tint;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(tint: &Option<Tint>, s: S) -> Result<S::Ok, S::Error> {
        match tint {
            Some(t) => s.serialize_str(t.id()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Tint>, D::Error> {
        let name = Option::<String>::deserialize(d)?;
        Ok(name.as_deref().and_then(Tint::from_id))
    }
}

/// [`ink`] for rows visited in increasing entry order, such as the painted
/// rows of a frame: the first row walks up to its ancestors once, later
/// rows reuse the colours of the groups above them.
#[derive(Default)]
pub(crate) struct InkWalk {
    started: bool,
    /// Open groups above the current row: their depth and colour.
    groups: Vec<(u8, Option<Tint>)>,
}

impl InkWalk {
    pub fn ink(&mut self, items: &[Entry], ix: usize) -> Option<Tint> {
        let depth = items[ix].depth;
        if !self.started {
            self.started = true;
            let mut chain = Vec::new();
            let mut at = tree::parent(items, ix);
            while let Some(p) = at {
                chain.push(p);
                at = tree::parent(items, p);
            }
            for &g in chain.iter().rev() {
                let inherited = self.groups.last().and_then(|g| g.1);
                self.groups
                    .push((items[g].depth, items[g].row.tint().or(inherited)));
            }
        }
        while self.groups.last().is_some_and(|g| g.0 >= depth) {
            self.groups.pop();
        }
        let ink = items[ix]
            .row
            .tint()
            .or_else(|| self.groups.last().and_then(|g| g.1));
        if items[ix].is_group() {
            self.groups.push((depth, ink));
        }
        ink
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wave::model::{ClockRow, GroupRow, WaveRow};

    fn leaf(depth: u8, tint: Option<Tint>) -> Entry {
        let mut row = WaveRow::Clock(ClockRow::new(crate::trace::Traced::new(
            crate::trace::TraceId::A,
            "c".to_owned(),
        )));
        row.set_tint(tint);
        Entry::new(depth, row)
    }

    fn group(depth: u8, tint: Option<Tint>) -> Entry {
        let mut row = WaveRow::Group(GroupRow::new("g"));
        row.set_tint(tint);
        Entry::new(depth, row)
    }

    #[test]
    fn the_walk_from_any_row_matches_ink() {
        use Tint::*;
        // a, [G cyan: b, [H: c, [I pink: d, e]], f], g, [J: h]
        let items = vec![
            leaf(0, None),
            group(0, Some(Cyan)),
            leaf(1, None),
            group(1, None),
            leaf(2, Some(Grey)),
            group(2, Some(Pink)),
            leaf(3, None),
            leaf(3, None),
            leaf(1, None),
            leaf(0, None),
            group(0, None),
            leaf(1, None),
        ];
        crate::wave::tree::validate(&items).unwrap();
        let expected: Vec<_> = (0..items.len()).map(|i| ink(&items, i)).collect();
        assert_eq!(
            expected,
            [
                None,
                Some(Cyan),
                Some(Cyan),
                Some(Cyan),
                Some(Grey),
                Some(Pink),
                Some(Pink),
                Some(Pink),
                Some(Cyan),
                None,
                None,
                None
            ]
        );
        for start in 0..items.len() {
            let mut walk = InkWalk::default();
            for (i, want) in expected.iter().enumerate().skip(start) {
                assert_eq!(walk.ink(&items, i), *want, "from {start}, row {i}");
            }
            // Skipping a folded group's rows keeps the walk right.
            let mut walk = InkWalk::default();
            for i in (start..items.len()).filter(|i| !(6..8).contains(i)) {
                assert_eq!(walk.ink(&items, i), expected[i]);
            }
        }
    }
}
