//! Signal groups: the rows of a wave panel as a tree stored in pre-order.
//!
//! Each [`Entry`] carries its depth; an entry's subtree is the entry plus
//! every following entry that is deeper than it. Selection, clipboard ranges,
//! moves and row heights stay operations on one vector, and the functions
//! here are the only code that knows how depths nest (`docs/wave_groups.html`).
//!
//! Every change of the vector is a list of [`Splice`]s: the planners here
//! return them, and [`apply`] performs them and returns their inverse, which
//! is what the undo journal keeps (`docs/undo-redo.html`).

use std::collections::BTreeSet;
use std::ops::{Deref, DerefMut, Range, RangeInclusive};
use std::sync::Arc;

use super::model::WaveRow;
pub use crate::history::Splice;

/// Groups nest at most this deep: every entry's depth is below it.
pub const MAX_DEPTH: u8 = 8;

/// One row of a wave panel at its depth in the group tree.
#[derive(Clone)]
pub struct Entry {
    pub depth: u8,
    pub row: WaveRow,
}

impl Entry {
    pub fn new(depth: u8, row: WaveRow) -> Self {
        Self { depth, row }
    }

    pub fn is_group(&self) -> bool {
        matches!(self.row, WaveRow::Group(_))
    }

    /// A copy that holds no trace data: what the clipboard and the undo
    /// journal keep (see [`WaveRow::detached`]).
    pub fn detached(&self) -> Self {
        Self {
            depth: self.depth,
            row: self.row.detached(),
        }
    }

    /// Whether two rows describe the same cockpit content: everything a
    /// workspace stores except the fold state.
    pub fn same(&self, other: &Self) -> bool {
        self.depth == other.depth && self.row.same(&other.row)
    }
}

impl Deref for Entry {
    type Target = WaveRow;
    fn deref(&self) -> &WaveRow {
        &self.row
    }
}

impl DerefMut for Entry {
    fn deref_mut(&mut self) -> &mut WaveRow {
        &mut self.row
    }
}

/// Check the tree invariant: the first entry is a root, an entry is at most
/// one level deeper than the one before it and only below a group, and no
/// entry is [`MAX_DEPTH`] deep.
pub fn validate(items: &[Entry]) -> Result<(), String> {
    validate_shape(items.iter().map(|e| (e.depth, e.is_group())))
}

fn validate_shape(shape: impl Iterator<Item = (u8, bool)>) -> Result<(), String> {
    let mut limit = 0u8;
    for (i, (depth, group)) in shape.enumerate() {
        if depth >= MAX_DEPTH {
            return Err(format!("row {i}: groups nested deeper than {MAX_DEPTH}"));
        }
        if depth > limit {
            return Err(format!("row {i}: depth {depth} below a non-group"));
        }
        limit = depth + u8::from(group);
    }
    Ok(())
}

/// Perform `splices` in order and return the splices that undo them, in the
/// order to perform, holding the replaced rows detached. Fails without
/// changing anything when a splice is out of bounds or the result would
/// break the tree invariant.
pub fn apply(items: &mut Vec<Entry>, splices: Vec<Splice>) -> Result<Vec<Splice>, String> {
    // Check the result on depths alone before touching the rows.
    let mut shape: Vec<(u8, bool)> = items.iter().map(|e| (e.depth, e.is_group())).collect();
    for s in &splices {
        if s.at + s.remove > shape.len() {
            return Err(format!(
                "splice {}+{} outside {} rows",
                s.at,
                s.remove,
                shape.len()
            ));
        }
        shape.splice(
            s.at..s.at + s.remove,
            s.insert.iter().map(|e| (e.depth, e.is_group())),
        );
    }
    validate_shape(shape.into_iter())?;
    let mut inverse = Vec::with_capacity(splices.len());
    for s in splices {
        let inserted = s.insert.len();
        let removed = items
            .splice(s.at..s.at + s.remove, s.insert)
            .map(|e| e.detached())
            .collect();
        inverse.push(Splice {
            at: s.at,
            remove: inserted,
            insert: removed,
        });
    }
    inverse.reverse();
    Ok(inverse)
}

/// One past the last entry of `i`'s subtree.
pub fn subtree_end(items: &[Entry], i: usize) -> usize {
    let depth = items[i].depth;
    i + 1
        + items[i + 1..]
            .iter()
            .take_while(|e| e.depth > depth)
            .count()
}

/// Entry `i` and every entry below it.
pub fn subtree(items: &[Entry], i: usize) -> Range<usize> {
    i..subtree_end(items, i)
}

/// The group that holds entry `i`.
pub fn parent(items: &[Entry], i: usize) -> Option<usize> {
    let depth = items.get(i)?.depth;
    (0..i).rev().find(|&j| items[j].depth < depth)
}

/// Whether `i` is `ancestor` or lies inside its subtree.
pub fn is_within(items: &[Entry], i: usize, ancestor: usize) -> bool {
    subtree(items, ancestor).contains(&i)
}

/// The entries that are painted: every entry outside a folded group.
pub fn visible(items: &[Entry]) -> Arc<[u32]> {
    let mut out = Vec::with_capacity(items.len());
    let mut hide_below: Option<u8> = None;
    for (i, e) in items.iter().enumerate() {
        if hide_below.is_some_and(|d| e.depth > d) {
            continue;
        }
        hide_below = None;
        out.push(i as u32);
        if let WaveRow::Group(g) = &e.row
            && g.collapsed
        {
            hide_below = Some(e.depth);
        }
    }
    out.into()
}

/// Selected entries without a selected ancestor, in order.
pub fn roots(items: &[Entry], sel: &BTreeSet<usize>) -> Vec<usize> {
    let mut out = Vec::new();
    let mut end = 0;
    for &i in sel.iter().filter(|&&i| i < items.len()) {
        if i < end {
            continue;
        }
        out.push(i);
        end = subtree_end(items, i);
    }
    out
}

/// The subtrees of [`roots`] as entry ranges.
pub fn blocks(items: &[Entry], sel: &BTreeSet<usize>) -> Vec<Range<usize>> {
    roots(items, sel)
        .into_iter()
        .map(|i| subtree(items, i))
        .collect()
}

/// Signal, lane and clock entries of `i`'s subtree (itself when it is one).
pub fn leaves(items: &[Entry], i: usize) -> impl Iterator<Item = usize> + '_ {
    subtree(items, i).filter(|&j| !items[j].is_group())
}

/// Every leaf under the entries of `sel`, each once, in order.
pub fn selected_leaves(items: &[Entry], sel: &BTreeSet<usize>) -> Vec<usize> {
    blocks(items, sel)
        .into_iter()
        .flatten()
        .filter(|&j| !items[j].is_group())
        .collect()
}

/// The depths at which rows can be inserted into visible gap `gap` (before
/// visible position `gap`): from the level of the row below up to one level
/// inside the open group above.
pub fn gap_depths(items: &[Entry], visible: &[u32], gap: usize) -> RangeInclusive<u8> {
    let above = gap.checked_sub(1).and_then(|g| visible.get(g));
    let below = visible.get(gap);
    let max = above.map_or(0, |&a| {
        let e = &items[a as usize];
        let open = matches!(&e.row, WaveRow::Group(g) if !g.collapsed);
        e.depth + u8::from(open)
    });
    let min = below.map_or(0, |&b| items[b as usize].depth);
    min..=max.min(MAX_DEPTH - 1).max(min)
}

/// Where moved or inserted rows land: before entry `at` (counted before the
/// move), with their roots at `depth`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub at: usize,
    pub depth: u8,
}

impl Place {
    /// Visible gap `gap` at `depth`.
    pub fn gap(items: &[Entry], visible: &[u32], gap: usize, depth: u8) -> Self {
        Self {
            at: visible.get(gap).map_or(items.len(), |&i| i as usize),
            depth,
        }
    }

    /// Appended as the last child of group `group`.
    pub fn into(items: &[Entry], group: usize) -> Self {
        Self {
            at: subtree_end(items, group),
            depth: items[group].depth + 1,
        }
    }
}

/// A planned move: the splices that perform it, and where the moved roots
/// end up.
#[derive(Clone)]
pub struct Moved {
    pub splices: Vec<Splice>,
    pub roots: BTreeSet<usize>,
    /// Where the first moved row lands.
    pub first: usize,
}

/// Plan moving the selected subtrees, in order, to `to`. `None` when the
/// move would put a group inside itself, nest too deep, or change nothing
/// (a row moved past an identical copy of itself changes nothing either).
pub fn plan_move(items: &[Entry], sel: &BTreeSet<usize>, to: Place) -> Option<Moved> {
    let blocks = blocks(items, sel);
    if blocks.is_empty() || to.at > items.len() {
        return None;
    }
    if blocks.iter().any(|b| to.at > b.start && to.at < b.end) {
        return None;
    }
    let mut moved = Vec::new();
    let mut rest = Vec::new();
    let mut next = blocks.iter().peekable();
    let mut current: Option<&Range<usize>> = None;
    for (i, e) in items.iter().enumerate() {
        if current.is_none_or(|b| i >= b.end)
            && let Some(b) = next.next_if(|b| b.start == i)
        {
            current = Some(b);
        }
        match current.filter(|b| b.contains(&i)) {
            Some(b) => {
                let depth =
                    u32::from(e.depth) - u32::from(items[b.start].depth) + u32::from(to.depth);
                if depth >= u32::from(MAX_DEPTH) {
                    return None;
                }
                moved.push((i, depth as u8));
            }
            None => rest.push((i, e.depth)),
        }
    }
    let first = rest.iter().filter(|(i, _)| *i < to.at).count();
    let mut order = rest[..first].to_vec();
    order.extend_from_slice(&moved);
    order.extend_from_slice(&rest[first..]);
    if order
        .iter()
        .enumerate()
        .all(|(k, &(i, d))| d == items[k].depth && (i == k || items[i].row.same(&items[k].row)))
    {
        return None;
    }
    // The rows after the drop must still hang from a group.
    let mut limit = 0u8;
    for &(i, d) in &order {
        if d > limit {
            return None;
        }
        limit = d + u8::from(items[i].is_group());
    }
    let mut roots = BTreeSet::new();
    let mut k = first;
    for b in &blocks {
        roots.insert(k);
        k += b.len();
    }
    let mut splices = removal_of(&blocks);
    splices.push(Splice {
        at: first,
        remove: 0,
        insert: moved
            .iter()
            .map(|&(i, depth)| Entry {
                depth,
                row: items[i].row.clone(),
            })
            .collect(),
    });
    Some(Moved {
        splices,
        roots,
        first,
    })
}

/// Splices removing `blocks` (ascending, disjoint), last first; adjacent
/// blocks go in one splice.
fn removal_of(blocks: &[Range<usize>]) -> Vec<Splice> {
    let mut runs: Vec<Range<usize>> = Vec::new();
    for b in blocks {
        match runs.last_mut() {
            Some(run) if run.end == b.start => run.end = b.end,
            _ => runs.push(b.clone()),
        }
    }
    runs.into_iter()
        .rev()
        .map(|b| Splice {
            at: b.start,
            remove: b.len(),
            insert: Vec::new(),
        })
        .collect()
}

/// Plan putting the selected subtrees, in order, under a new group `group`
/// at the place and level of the first one. Returns the splices and the
/// group's index after them, or `None` when nothing is selected or the rows
/// would nest too deep.
pub fn group(
    items: &[Entry],
    sel: &BTreeSet<usize>,
    group: WaveRow,
) -> Option<(Vec<Splice>, usize)> {
    let blocks = blocks(items, sel);
    let first = blocks.first()?.start;
    let depth = items[first].depth;
    let too_deep = blocks.iter().any(|b| {
        items[b.clone()]
            .iter()
            .any(|e| e.depth - items[b.start].depth + depth + 1 >= MAX_DEPTH)
    });
    if too_deep {
        return None;
    }
    let mut inserted = vec![Entry::new(depth, group)];
    for b in &blocks {
        let base = items[b.start].depth;
        inserted.extend(items[b.clone()].iter().map(|e| Entry {
            depth: e.depth - base + depth + 1,
            row: e.row.clone(),
        }));
    }
    let mut splices = removal_of(&blocks);
    splices.push(Splice {
        at: first,
        remove: 0,
        insert: inserted,
    });
    Some((splices, first))
}

/// Plan dissolving the groups among `groups`: their children take their
/// place one level up. Returns the splices and the dissolved groups' former
/// children, indexed as after them.
pub fn ungroup(items: &[Entry], groups: &BTreeSet<usize>) -> (Vec<Splice>, BTreeSet<usize>) {
    let groups: BTreeSet<usize> = groups
        .iter()
        .copied()
        .filter(|&g| items.get(g).is_some_and(Entry::is_group))
        .collect();
    let mut splices = Vec::new();
    let mut children = BTreeSet::new();
    let mut shift = 0;
    // Each outermost dissolved group's subtree is rewritten by one splice.
    for span in blocks(items, &groups) {
        let spans: Vec<(usize, u8, Range<usize>)> = groups
            .range(span.clone())
            .map(|&g| (g, items[g].depth, subtree(items, g)))
            .collect();
        let at = span.start - shift;
        let mut insert = Vec::with_capacity(span.len());
        for i in span.clone() {
            if spans.iter().any(|(g, ..)| *g == i) {
                continue;
            }
            let mut lift = 0;
            for (_, depth, _) in spans.iter().filter(|(g, _, s)| i > *g && s.contains(&i)) {
                lift += 1;
                if items[i].depth == depth + 1 {
                    children.insert(at + insert.len());
                }
            }
            insert.push(Entry {
                depth: items[i].depth - lift,
                row: items[i].row.clone(),
            });
        }
        shift += span.len() - insert.len();
        splices.push(Splice {
            at,
            remove: span.len(),
            insert,
        });
    }
    (splices, children)
}

/// What closing `trace` removes: its rows, and every group whose rows all
/// come from it. Groups that were empty already stay.
pub fn of_trace(items: &[Entry], trace: crate::trace::TraceId) -> BTreeSet<usize> {
    let of = |j: usize| items[j].row.trace() == Some(trace);
    (0..items.len())
        .filter(|&i| {
            if !items[i].is_group() {
                return of(i);
            }
            let mut leaves = leaves(items, i).peekable();
            leaves.peek().is_some() && leaves.all(of)
        })
        .collect()
}

/// Plan removing the selected subtrees.
pub fn removal(items: &[Entry], sel: &BTreeSet<usize>) -> Vec<Splice> {
    removal_of(&blocks(items, sel))
}

/// Copies of the selected subtrees with their roots at depth zero.
pub fn extract(items: &[Entry], sel: &BTreeSet<usize>) -> Vec<Entry> {
    blocks(items, sel)
        .into_iter()
        .flat_map(|b| {
            let base = items[b.start].depth;
            items[b].iter().map(move |e| Entry {
                depth: e.depth - base,
                row: e.row.clone(),
            })
        })
        .collect()
}

/// Plan inserting `rows` (roots at depth zero) at `to`. Returns the splice
/// and the inserted roots, or `None` when they would nest too deep.
pub fn insertion(to: Place, rows: Vec<Entry>) -> Option<(Splice, BTreeSet<usize>)> {
    if rows.iter().any(|e| e.depth + to.depth >= MAX_DEPTH) {
        return None;
    }
    let roots = rows
        .iter()
        .enumerate()
        .filter(|(_, e)| e.depth == 0)
        .map(|(k, _)| to.at + k)
        .collect();
    let insert = rows
        .into_iter()
        .map(|e| Entry {
            depth: e.depth + to.depth,
            row: e.row,
        })
        .collect();
    Some((
        Splice {
            at: to.at,
            remove: 0,
            insert,
        },
        roots,
    ))
}

/// Fold or unfold group `i`, and with `deep` every group inside it.
/// Returns whether anything changed. Folds are navigation: they change
/// entries in place and are not journaled.
pub fn set_collapsed(items: &mut [Entry], i: usize, collapsed: bool, deep: bool) -> bool {
    let end = if deep { subtree_end(items, i) } else { i + 1 };
    let mut changed = false;
    for e in &mut items[i..end] {
        if let WaveRow::Group(g) = &mut e.row
            && g.collapsed != collapsed
        {
            g.collapsed = collapsed;
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wave::model::{ClockRow, GroupRow};

    fn leaf(depth: u8, name: &str) -> Entry {
        Entry::new(
            depth,
            WaveRow::Clock(ClockRow::new(crate::trace::Traced::new(
                crate::trace::TraceId::A,
                name.to_owned(),
            ))),
        )
    }

    fn group(depth: u8, name: &str, collapsed: bool) -> Entry {
        let mut g = GroupRow::new(name);
        g.collapsed = collapsed;
        Entry::new(depth, WaveRow::Group(g))
    }

    /// clk, [AXI: [Write: aw, w], [Read (folded): ar, r]], irq
    fn soc() -> Vec<Entry> {
        vec![
            leaf(0, "clk"),
            group(0, "AXI", false),
            group(1, "Write", false),
            leaf(2, "aw"),
            leaf(2, "w"),
            group(1, "Read", true),
            leaf(2, "ar"),
            leaf(2, "r"),
            leaf(0, "irq"),
        ]
    }

    fn set(rows: &[usize]) -> BTreeSet<usize> {
        rows.iter().copied().collect()
    }

    fn names(items: &[Entry]) -> Vec<(u8, &str)> {
        items.iter().map(|e| (e.depth, e.name())).collect()
    }

    /// Apply `splices`, check the tree, undo them and check the rows are
    /// back exactly; returns the rows as the splices left them.
    fn round_trip(items: &[Entry], splices: Vec<Splice>) -> Vec<Entry> {
        let mut edited = items.to_vec();
        let inverse = apply(&mut edited, splices).expect("valid splices");
        assert!(validate(&edited).is_ok());
        let mut undone = edited.clone();
        let redo = apply(&mut undone, inverse).expect("valid inverse");
        assert_eq!(names(&undone), names(items));
        assert!(
            undone
                .iter()
                .zip(items)
                .all(|(a, b)| a.same(b) && a.group() == b.group())
        );
        let mut redone = undone;
        apply(&mut redone, redo).expect("valid redo");
        assert_eq!(names(&redone), names(&edited));
        edited
    }

    #[test]
    fn the_invariant_rejects_orphans_and_depth() {
        assert!(validate(&soc()).is_ok());
        assert!(
            validate(&[leaf(1, "a")]).is_err(),
            "first row below the top"
        );
        assert!(
            validate(&[leaf(0, "a"), leaf(1, "b")]).is_err(),
            "child of a signal"
        );
        let deep: Vec<Entry> = (0..MAX_DEPTH).map(|d| group(d, "g", false)).collect();
        assert!(validate(&deep).is_ok());
        let mut deeper = deep;
        deeper.push(leaf(MAX_DEPTH, "x"));
        assert!(validate(&deeper).is_err());
    }

    #[test]
    fn subtrees_parents_leaves_and_visible_rows() {
        let items = soc();
        assert_eq!(subtree(&items, 1), 1..8);
        assert_eq!(subtree(&items, 3), 3..4);
        assert_eq!(parent(&items, 4), Some(2));
        assert_eq!(parent(&items, 2), Some(1));
        assert_eq!(parent(&items, 1), None);
        assert_eq!(leaves(&items, 1).collect::<Vec<_>>(), [3, 4, 6, 7]);
        assert_eq!(&*visible(&items), &[0, 1, 2, 3, 4, 5, 8]);
        // A selected group and its own row count once.
        assert_eq!(roots(&items, &set(&[2, 3, 8])), [2, 8]);
        assert_eq!(selected_leaves(&items, &set(&[2, 3, 8])), [3, 4, 8]);
    }

    #[test]
    fn gaps_offer_the_levels_between_their_neighbours() {
        let items = soc();
        let v = visible(&items);
        assert_eq!(gap_depths(&items, &v, 0), 0..=0);
        // Below open AXI: only inside it.
        assert_eq!(gap_depths(&items, &v, 2), 1..=1);
        // Below w, above Read: Write's level or inside Write.
        assert_eq!(gap_depths(&items, &v, 5), 1..=2);
        // Below folded Read, above irq: Read is closed, so AXI's level or the top.
        assert_eq!(gap_depths(&items, &v, 6), 0..=1);
        assert_eq!(gap_depths(&items, &v, 7), 0..=0);
    }

    #[test]
    fn moves_refuse_cycles_depth_and_no_ops() {
        let items = soc();
        let v = visible(&items);
        // AXI into its own Write group.
        assert!(plan_move(&items, &set(&[1]), Place { at: 3, depth: 2 }).is_none());
        // clk where it is.
        assert!(plan_move(&items, &set(&[0]), Place { at: 0, depth: 0 }).is_none());
        // irq into folded Read, as its last row.
        let to = Place::into(&items, 5);
        assert_eq!(to, Place { at: 8, depth: 2 });
        let moved = plan_move(&items, &set(&[8]), to).unwrap();
        assert_eq!(moved.roots, set(&[8]));
        let m = round_trip(&items, moved.splices);
        assert_eq!(m[8].name(), "irq");
        assert_eq!(m[8].depth, 2);
        assert_eq!(parent(&m, 8), Some(5));
        // clk to the end of AXI, one level in.
        let to = Place::gap(&items, &v, 6, 1);
        let moved = plan_move(&items, &set(&[0]), to).unwrap();
        let m = round_trip(&items, moved.splices);
        assert_eq!(m[7].name(), "clk");
        assert_eq!(parent(&m, 7), Some(0));
        assert_eq!(moved_roots(&m), ["AXI", "irq"]);
        // Two blocks at once keep their order.
        let moved = plan_move(&items, &set(&[0, 8]), Place { at: 3, depth: 2 }).unwrap();
        let m = round_trip(&items, moved.splices);
        assert_eq!(
            names(&m)[..5],
            [(0, "AXI"), (1, "Write"), (2, "clk"), (2, "irq"), (2, "aw")]
        );
        assert_eq!(moved.roots, set(&[2, 3]));
        // Too deep.
        let deep: Vec<Entry> = (0..MAX_DEPTH)
            .map(|d| group(d, "g", false))
            .chain([leaf(0, "x")])
            .collect();
        let to = Place {
            at: MAX_DEPTH as usize,
            depth: MAX_DEPTH,
        };
        assert!(plan_move(&deep, &set(&[0]), to).is_none());
    }

    fn moved_roots(items: &[Entry]) -> Vec<&str> {
        items
            .iter()
            .filter(|e| e.depth == 0)
            .map(|e| e.name())
            .collect()
    }

    #[test]
    fn group_ungroup_insert_and_fold() {
        let items = soc();
        let (splices, g) =
            super::group(&items, &set(&[4, 8]), WaveRow::Group(GroupRow::new("new"))).unwrap();
        assert_eq!(g, 4);
        let items = round_trip(&items, splices);
        assert_eq!(names(&items)[4..7], [(2, "new"), (3, "w"), (3, "irq")]);
        let (splices, children) = ungroup(&items, &set(&[4]));
        assert_eq!(children, set(&[4, 5]));
        let items = round_trip(&items, splices);
        assert_eq!(items[5].name(), "irq");
        assert_eq!(items[5].depth, 2);

        // Nested groups dissolve together; their rows lift by both.
        let items = soc();
        let (splices, children) = ungroup(&items, &set(&[1, 5]));
        let flat = round_trip(&items, splices);
        assert_eq!(
            names(&flat),
            [
                (0, "clk"),
                (0, "Write"),
                (1, "aw"),
                (1, "w"),
                (0, "ar"),
                (0, "r"),
                (0, "irq")
            ]
        );
        assert_eq!(children, set(&[1, 4, 5]));

        let items = soc();
        let copied = extract(&items, &set(&[5]));
        assert_eq!(
            copied.iter().map(|e| e.depth).collect::<Vec<_>>(),
            [0, 1, 1]
        );
        let (splice, roots) = insertion(Place { at: 9, depth: 0 }, copied).unwrap();
        assert_eq!(roots, set(&[9]));
        let mut items = round_trip(&items, vec![splice]);
        assert!(set_collapsed(&mut items, 1, true, true));
        assert!(
            items[1..8]
                .iter()
                .all(|e| e.group().is_none_or(|g| g.collapsed))
        );
        let items = round_trip(&items, removal(&items, &set(&[1, 3])));
        assert_eq!(
            items.iter().map(|e| e.name()).collect::<Vec<_>>(),
            ["clk", "irq", "Read", "ar", "r"]
        );
    }

    #[test]
    fn invalid_splices_change_nothing() {
        let mut items = soc();
        let before = names(&items).len();
        let outside = Splice {
            at: 8,
            remove: 2,
            insert: Vec::new(),
        };
        assert!(apply(&mut items, vec![outside]).is_err());
        // Removing a group's row orphans its children.
        let orphan = Splice {
            at: 1,
            remove: 1,
            insert: Vec::new(),
        };
        assert!(apply(&mut items, vec![orphan]).is_err());
        assert_eq!(names(&items).len(), before);
    }
}
