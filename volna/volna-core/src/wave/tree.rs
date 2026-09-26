//! Signal groups: the rows of a wave panel as a tree stored in pre-order.
//!
//! Each [`Entry`] carries its depth; an entry's subtree is the entry plus
//! every following entry that is deeper than it. Selection, clipboard ranges,
//! moves and row heights stay operations on one vector, and the functions
//! here are the only code that knows how depths nest (`docs/wave_groups.html`).

use std::collections::BTreeSet;
use std::ops::{Deref, DerefMut, Range, RangeInclusive};
use std::sync::Arc;

use super::model::WaveRow;

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
    let mut limit = 0u8;
    for (i, e) in items.iter().enumerate() {
        if e.depth >= MAX_DEPTH {
            return Err(format!("row {i}: groups nested deeper than {MAX_DEPTH}"));
        }
        if e.depth > limit {
            return Err(format!("row {i}: depth {} below a non-group", e.depth));
        }
        limit = e.depth + u8::from(e.is_group());
    }
    Ok(())
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

/// A planned move: the new order as (old index, new depth), and where the
/// moved roots end up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Moved {
    order: Vec<(usize, u8)>,
    pub roots: BTreeSet<usize>,
    /// Where the first moved row lands.
    pub first: usize,
}

impl Moved {
    pub fn apply(self, items: &mut Vec<Entry>) {
        let mut old: Vec<Option<Entry>> = std::mem::take(items).into_iter().map(Some).collect();
        items.extend(self.order.into_iter().map(|(i, depth)| Entry {
            depth,
            ..old[i].take().expect("each entry moves once")
        }));
    }
}

/// Plan moving the selected subtrees, in order, to `to`. `None` when the
/// move would put a group inside itself, nest too deep, or change nothing.
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
        .all(|(k, &(i, d))| i == k && d == items[k].depth)
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
    Some(Moved {
        order,
        roots,
        first,
    })
}

/// Put the selected subtrees, in order, under a new group `group` at the
/// place and level of the first one. Returns the group's index, or `None`
/// when nothing is selected or the rows would nest too deep.
pub fn group(items: &mut Vec<Entry>, sel: &BTreeSet<usize>, group: WaveRow) -> Option<usize> {
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
    let bases: Vec<u8> = blocks.iter().map(|b| items[b.start].depth).collect();
    let mut inserted = vec![Entry::new(depth, group)];
    let mut rest = Vec::with_capacity(items.len());
    let mut before = 0;
    for (i, mut e) in std::mem::take(items).into_iter().enumerate() {
        match blocks.iter().position(|b| b.contains(&i)) {
            Some(k) => {
                e.depth = e.depth - bases[k] + depth + 1;
                inserted.push(e);
            }
            None => {
                before += usize::from(i < first);
                rest.push(e);
            }
        }
    }
    rest.splice(before..before, inserted);
    *items = rest;
    Some(before)
}

/// Dissolve the groups among `groups`: their children take their place one
/// level up. Returns the dissolved groups' former children, reindexed.
pub fn ungroup(items: &mut Vec<Entry>, groups: &BTreeSet<usize>) -> BTreeSet<usize> {
    let spans: Vec<(usize, u8, Range<usize>)> = groups
        .iter()
        .filter(|&&g| items.get(g).is_some_and(Entry::is_group))
        .map(|&g| (g, items[g].depth, subtree(items, g)))
        .collect();
    let mut children = BTreeSet::new();
    for (i, mut e) in std::mem::take(items).into_iter().enumerate() {
        if spans.iter().any(|(g, ..)| *g == i) {
            continue;
        }
        let holders = spans.iter().filter(|(g, _, s)| i > *g && s.contains(&i));
        let mut lift = 0;
        for (_, depth, _) in holders {
            lift += 1;
            if e.depth == depth + 1 {
                children.insert(items.len());
            }
        }
        e.depth -= lift;
        items.push(e);
    }
    children
}

/// Remove the selected subtrees; returns the removed ranges' first index.
pub fn remove(items: &mut Vec<Entry>, sel: &BTreeSet<usize>) -> Option<usize> {
    let blocks = blocks(items, sel);
    let first = blocks.first()?.start;
    let mut i = 0;
    items.retain(|_| {
        let keep = !blocks.iter().any(|b| b.contains(&i));
        i += 1;
        keep
    });
    Some(first)
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

/// Insert `rows` (roots at depth zero) at `to`. Returns the inserted roots,
/// or `None` when they would nest too deep.
pub fn insert(items: &mut Vec<Entry>, to: Place, rows: Vec<Entry>) -> Option<BTreeSet<usize>> {
    if rows.iter().any(|e| e.depth + to.depth >= MAX_DEPTH) {
        return None;
    }
    let roots = rows
        .iter()
        .enumerate()
        .filter(|(_, e)| e.depth == 0)
        .map(|(k, _)| to.at + k)
        .collect();
    items.splice(
        to.at..to.at,
        rows.into_iter().map(|e| Entry {
            depth: e.depth + to.depth,
            row: e.row,
        }),
    );
    Some(roots)
}

/// Fold or unfold group `i`, and with `deep` every group inside it.
/// Returns whether anything changed.
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
        Entry::new(depth, WaveRow::Clock(ClockRow::new(name)))
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
        let mut m = items.clone();
        moved.apply(&mut m);
        assert_eq!(m[8].name(), "irq");
        assert_eq!(m[8].depth, 2);
        assert_eq!(parent(&m, 8), Some(5));
        // clk to the end of AXI, one level in.
        let to = Place::gap(&items, &v, 6, 1);
        let moved = plan_move(&items, &set(&[0]), to).unwrap();
        let mut m = items.clone();
        moved.apply(&mut m);
        assert_eq!(m[7].name(), "clk");
        assert_eq!(parent(&m, 7), Some(0));
        assert_eq!(moved_roots(&m), ["AXI", "irq"]);
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
        let mut items = soc();
        let g = super::group(
            &mut items,
            &set(&[4, 8]),
            WaveRow::Group(GroupRow::new("new")),
        )
        .unwrap();
        assert_eq!(g, 4);
        let names: Vec<(u8, &str)> = items.iter().map(|e| (e.depth, e.name())).collect();
        assert_eq!(names[4..7], [(2, "new"), (3, "w"), (3, "irq")]);
        assert!(validate(&items).is_ok());
        let children = ungroup(&mut items, &set(&[4]));
        assert_eq!(children, set(&[4, 5]));
        assert_eq!(items[5].name(), "irq");
        assert_eq!(items[5].depth, 2);

        let mut items = soc();
        let copied = extract(&items, &set(&[5]));
        assert_eq!(
            copied.iter().map(|e| e.depth).collect::<Vec<_>>(),
            [0, 1, 1]
        );
        let roots = insert(&mut items, Place { at: 9, depth: 0 }, copied).unwrap();
        assert_eq!(roots, set(&[9]));
        assert!(validate(&items).is_ok());
        assert!(set_collapsed(&mut items, 1, true, true));
        assert!(
            items[1..8]
                .iter()
                .all(|e| e.group().is_none_or(|g| g.collapsed))
        );
        assert_eq!(remove(&mut items, &set(&[1, 3])), Some(1));
        assert_eq!(
            items.iter().map(|e| e.name()).collect::<Vec<_>>(),
            ["clk", "irq", "Read", "ar", "r"]
        );
    }
}
