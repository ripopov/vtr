#[derive(Clone, Debug, Default)]
pub enum Selection {
    #[default]
    Empty,
    Columns {
        columns: Vec<bool>,
        anchor: usize,
    },
    Rows {
        anchor: u32,
        end: u32,
    },
    Cells {
        anchor: (u32, usize),
        end: (u32, usize),
    },
}
impl Selection {
    pub fn column(&mut self, column: usize, shift: bool, toggle: bool, visible: &[bool]) {
        let (mut columns, anchor) = match self {
            Self::Columns { columns, anchor } => (columns.clone(), *anchor),
            _ => (vec![false; visible.len()], column),
        };
        if shift {
            if !toggle {
                columns.fill(false);
            }
            for (c, &shown) in visible.iter().enumerate() {
                if shown && (anchor.min(column)..=anchor.max(column)).contains(&c) {
                    columns[c] = true;
                }
            }
            *self = Self::Columns { columns, anchor };
        } else {
            if toggle {
                columns[column] = !columns[column];
            } else {
                columns.fill(false);
                columns[column] = true;
            }
            *self = Self::Columns {
                columns,
                anchor: column,
            };
        }
    }
    pub fn cell(&mut self, point: (u32, usize), extend: bool) {
        let anchor = if extend {
            match *self {
                Self::Cells { anchor, .. } => anchor,
                _ => point,
            }
        } else {
            point
        };
        *self = Self::Cells { anchor, end: point };
    }
    pub fn extend_row(&mut self, current: u32, next: u32) {
        let anchor = match *self {
            Self::Rows { anchor, end } if end == current => anchor,
            _ => current,
        };
        *self = Self::Rows { anchor, end: next };
    }
    pub fn contains(&self, row: u32, column: usize) -> bool {
        match *self {
            Self::Empty => false,
            Self::Columns { ref columns, .. } => columns.get(column).copied().unwrap_or(false),
            Self::Rows { anchor, end } => (anchor.min(end)..=anchor.max(end)).contains(&row),
            Self::Cells {
                anchor: (r, c),
                end: (rr, cc),
            } => {
                (r.min(rr)..=r.max(rr)).contains(&row) && (c.min(cc)..=c.max(cc)).contains(&column)
            }
        }
    }
    pub fn snapshot(&self, rows: u32, visible: &[bool]) -> Option<CopyRange> {
        if rows == 0 {
            return None;
        }
        let (first, last) = match *self {
            Self::Empty => return None,
            Self::Columns { .. } => (0, rows - 1),
            Self::Rows { anchor: a, end: b } => (a.min(b).min(rows - 1), a.max(b).min(rows - 1)),
            Self::Cells {
                anchor: (a, _),
                end: (b, _),
            } => (a.min(b).min(rows - 1), a.max(b).min(rows - 1)),
        };
        let columns: Vec<_> = (0..visible.len())
            .filter(|&c| visible[c] && self.contains(first, c))
            .collect();
        (!columns.is_empty()).then_some(CopyRange {
            first,
            last,
            columns,
        })
    }
}
#[derive(Clone, Debug)]
pub struct CopyRange {
    pub first: u32,
    pub last: u32,
    pub columns: Vec<usize>,
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranges_and_column_modifiers() {
        let mut shown = [true; 30];
        shown[2] = false;
        let mut s = Selection::Empty;
        s.column(1, false, false, &shown);
        s.column(4, true, false, &shown);
        assert_eq!(s.snapshot(10_000_000, &shown).unwrap().columns, [1, 3, 4]);
        s.column(3, false, true, &shown);
        assert_eq!(s.snapshot(10_000_000, &shown).unwrap().columns, [1, 4]);
        s.cell((10, 4), false);
        s.cell((2, 1), true);
        let r = s.snapshot(20, &shown).unwrap();
        assert_eq!((r.first, r.last, r.columns), (2, 10, vec![1, 3, 4]));
    }
}
