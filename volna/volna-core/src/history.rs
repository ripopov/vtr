//! Undo and redo of the cockpit (`volna/volna/ARCHITECTURE.md`, "Undo and redo").
//!
//! One linear history per open trace. Changes to cockpit state are
//! [`Edit`]s: data whose application returns the edit that undoes it, so
//! undo and redo are the same operation ([`crate::App`]'s `flip`). Every
//! journaled field has exactly one writer, which keeps the value the field
//! had before its first change since the app last collected edits; the app
//! collects those inverses after each command into the open [`Step`]. Steps
//! hold descriptions (detached rows and panels), never trace data.

use std::collections::{BTreeSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::time::Duration;

use web_time::Instant;

use crate::marker::Marker;
use crate::panels::{Layout, Panel, PanelId, content::Prop};
use crate::wave::tree::Entry;

/// Repeated adjustments of the same targets merge into one step while they
/// follow each other this closely.
pub const MERGE_WINDOW: Duration = Duration::from_secs(1);
/// The journal's memory cap. The oldest steps go first; the newest step is
/// always kept, even when it alone is larger.
pub const MAX_BYTES: usize = 64 << 20;

/// The value a journaled field had before its first change since the app
/// last collected edits. Every journaled field keeps its inverse in one of
/// these. Debug builds count the ones that hold a value on each thread, so
/// the app can check that collecting took every one ([`pending`]): a field
/// its owner forgot to hand over fails the next command in any test.
pub struct Before<T>(Option<T>);

#[cfg(debug_assertions)]
thread_local! {
    static PENDING: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many [`Before`]s on this thread hold a value; always 0 in release
/// builds.
pub fn pending() -> usize {
    #[cfg(debug_assertions)]
    return PENDING.with(std::cell::Cell::get);
    #[cfg(not(debug_assertions))]
    0
}

impl<T> Before<T> {
    /// Keep `value()` unless a value is already kept.
    pub(crate) fn note(&mut self, value: impl FnOnce() -> T) {
        if self.0.is_none() {
            self.0 = Some(value());
            #[cfg(debug_assertions)]
            PENDING.with(|n| n.set(n.get() + 1));
        }
    }

    /// The kept value, handed to the journal.
    pub(crate) fn take(&mut self) -> Option<T> {
        let value = self.0.take();
        #[cfg(debug_assertions)]
        if value.is_some() {
            PENDING.with(|n| n.set(n.get() - 1));
        }
        value
    }

    pub(crate) fn is_held(&self) -> bool {
        self.0.is_some()
    }
}

impl<T> Default for Before<T> {
    fn default() -> Self {
        Self(None)
    }
}

/// A copy holds nothing: the pending inverse stays with the original.
impl<T> Clone for Before<T> {
    fn clone(&self) -> Self {
        Self(None)
    }
}

impl<T> Drop for Before<T> {
    fn drop(&mut self) {
        self.take();
    }
}

impl<T> std::fmt::Debug for Before<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.is_held() {
            "Before(held)"
        } else {
            "Before(none)"
        })
    }
}

/// A field of cockpit state that only changes through [`Journaled::set`]
/// (or [`Journaled::update`]), which remembers the value before the first
/// change since the last [`Journaled::take_before`]. Reads go through
/// `Deref`; there is deliberately no `DerefMut`.
#[derive(Debug, Default)]
pub struct Journaled<T> {
    value: T,
    before: Before<T>,
}

impl<T: Clone + PartialEq> Journaled<T> {
    pub fn new(value: T) -> Self {
        Self {
            value,
            before: Before::default(),
        }
    }

    pub fn get(&self) -> &T {
        &self.value
    }

    /// Change the value as an edit. Returns whether it changed.
    pub(crate) fn set(&mut self, value: T) -> bool {
        if self.value == value {
            return false;
        }
        let old = std::mem::replace(&mut self.value, value);
        self.before.note(|| old);
        true
    }

    /// Change a copy of the value and [`Journaled::set`] it.
    pub(crate) fn update(&mut self, f: impl FnOnce(&mut T)) -> bool {
        let mut value = self.value.clone();
        f(&mut value);
        self.set(value)
    }

    /// Install a value that is not an edit (a restored workspace, a new
    /// trace): nothing is remembered.
    pub(crate) fn restore(&mut self, value: T) {
        self.value = value;
        self.before.take();
    }

    /// Install `value` while undoing or redoing; returns the value it replaces.
    pub(crate) fn swap(&mut self, value: T) -> T {
        debug_assert!(!self.before.is_held(), "edits were not collected");
        std::mem::replace(&mut self.value, value)
    }

    /// The value before the edits since the last call, unless they cancelled out.
    pub(crate) fn take_before(&mut self) -> Option<T> {
        self.before.take().filter(|before| *before != self.value)
    }
}

impl<T> std::ops::Deref for Journaled<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

/// A copy holds the value only; the pending inverse stays with the original.
impl<T: Clone> Clone for Journaled<T> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            before: Before::default(),
        }
    }
}

impl<T: PartialEq> PartialEq for Journaled<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<T: Eq> Eq for Journaled<T> {}

/// Replace `remove` rows at `at` of a wave panel's pre-order rows with
/// `insert`. Rows kept in the journal are detached: no history, no error.
#[derive(Clone)]
pub struct Splice {
    pub at: usize,
    pub remove: usize,
    pub insert: Vec<Entry>,
}

impl Splice {
    /// Whether the splice rewrites rows in place without moving others.
    pub fn in_place(&self) -> bool {
        self.remove == self.insert.len()
    }
}

/// The selected rows of a wave panel, restored with each step.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowSelection {
    pub selected: BTreeSet<usize>,
    pub anchor: Option<usize>,
}

/// One change to cockpit state. Applying it returns the edit that undoes it.
pub(crate) enum Edit {
    /// Replace runs of a wave panel's pre-order rows, applied in order.
    Rows {
        panel: PanelId,
        splices: Vec<Splice>,
    },
    /// The document's markers, swapped whole (tens, not thousands), and the
    /// marker the reference was attached to when the edit detached it:
    /// applying the edit reattaches it if the reference has not moved since.
    Markers {
        markers: Vec<Marker>,
        attached: Option<crate::marker::MarkerId>,
    },
    /// One property of one panel, swapped whole.
    Prop { panel: PanelId, prop: Prop },
    /// The panel tree, the panels to close, and detached panels to put back.
    Layout(Box<Structure>),
    /// A trace joining, leaving or being renamed. A trace that left keeps
    /// its open session here, so putting it back is instant.
    Traces(crate::document::TraceEdit),
}

/// What a workspace's panel structure was, as a layout edit restores it.
pub(crate) struct Structure {
    /// The dock tree without the settings tab, which is chrome.
    pub layout: Layout,
    pub focused: PanelId,
    /// Panels that exist now and go away.
    pub close: Vec<PanelId>,
    /// Detached panels that come back under their own IDs.
    pub reopen: Vec<Panel>,
}

/// Journal bytes: a row is its inline entry plus its own strings
/// (translators are shared); a closed panel adds a fixed allowance.
const ROW_BYTES: usize = std::mem::size_of::<Entry>();
const PANEL_BYTES: usize = 1024;

impl Edit {
    pub(crate) fn bytes(&self) -> usize {
        let rows = |rows: &[Entry]| {
            rows.iter()
                .map(|e| ROW_BYTES + e.name().len() + text_bytes(&e.row))
                .sum::<usize>()
        };
        std::mem::size_of::<Self>()
            + match self {
                Edit::Rows { splices, .. } => splices
                    .iter()
                    .map(|s| std::mem::size_of::<Splice>() + rows(&s.insert))
                    .sum(),
                Edit::Markers { markers, .. } => markers
                    .iter()
                    .map(|m| {
                        std::mem::size_of::<Marker>() + m.label.as_ref().map_or(0, String::len)
                    })
                    .sum(),
                Edit::Prop { prop, .. } => prop.bytes(),
                Edit::Layout(s) => {
                    s.close.len() * 8
                        + s.reopen
                            .iter()
                            .map(|p| PANEL_BYTES + p.kind.waves().map_or(0, |w| rows(w.items())))
                            .sum::<usize>()
                }
                // The session a closed trace keeps is charged to the memory
                // budget, not to the journal.
                Edit::Traces(_) => PANEL_BYTES,
            }
    }

    /// The target of a swap edit: a later swap of the same target within a
    /// step is not recorded, since the first inverse restores the start.
    fn swap_target(&self) -> Option<(u8, u8, Option<PanelId>)> {
        match self {
            Edit::Markers { .. } => Some((0, 0, None)),
            Edit::Prop { panel, prop } => {
                let (kind, field) = prop.kind();
                Some((1 + kind, field, Some(*panel)))
            }
            _ => None,
        }
    }

    /// The runs an in-place row edit rewrites, when it only rewrites.
    fn in_place_shape(&self) -> Option<(PanelId, Vec<(usize, usize)>)> {
        let Edit::Rows { panel, splices } = self else {
            return None;
        };
        splices
            .iter()
            .all(Splice::in_place)
            .then(|| (*panel, splices.iter().map(|s| (s.at, s.remove)).collect()))
    }

    /// The panel whose content the edit changes.
    fn panel(&self) -> Option<PanelId> {
        match self {
            Edit::Rows { panel, .. } | Edit::Prop { panel, .. } => Some(*panel),
            Edit::Markers { .. } | Edit::Layout(_) | Edit::Traces(_) => None,
        }
    }
}

fn text_bytes(row: &crate::wave::WaveRow) -> usize {
    use crate::wave::WaveRow;
    match row {
        WaveRow::Signal(s) => s.scope.len() + s.requested_format.as_ref().map_or(0, String::len),
        WaveRow::Lane(l) => l.scope.len() + l.source.path().iter().map(String::len).sum::<usize>(),
        WaveRow::Clock(c) => c.key.item.len(),
        WaveRow::Group(_) => 0,
    }
}

/// The kind and targets of an adjustment. Equal keys merge within
/// [`MERGE_WINDOW`]; a step that [`Step::continues`] a key is joined by the
/// next step with that key, however late.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MergeKey {
    pub kind: &'static str,
    pub panel: Option<PanelId>,
    pub target: u64,
}

impl MergeKey {
    /// A key for `kind` over `target` (hashed), with the panel filled in by
    /// the app when the edit is collected.
    pub(crate) fn of(kind: &'static str, target: &impl Hash) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        target.hash(&mut hasher);
        Self {
            kind,
            panel: None,
            target: hasher.finish(),
        }
    }

    pub(crate) fn in_panel(mut self, panel: PanelId) -> Self {
        self.panel = Some(panel);
        self
    }
}

/// Focus and the selection of each wave panel a step touched.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Context {
    pub focus: Option<PanelId>,
    pub rows: Vec<(PanelId, RowSelection)>,
}

/// What one undo takes back: the inverse edits of one command or gesture.
pub struct Step {
    pub label: String,
    /// Recorded order; a flip applies them newest first.
    pub(crate) edits: Vec<Edit>,
    pub before: Context,
    pub after: Context,
    pub(crate) merge: Option<MergeKey>,
    pub(crate) continues: Option<MergeKey>,
    pub(crate) at: Instant,
    bytes: usize,
}

impl Step {
    fn new(focus: PanelId, at: Instant) -> Self {
        Self {
            label: String::new(),
            edits: Vec::new(),
            before: Context {
                focus: Some(focus),
                rows: Vec::new(),
            },
            after: Context::default(),
            merge: None,
            continues: None,
            at,
            bytes: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Append an inverse, unless an earlier inverse in this step already
    /// restores the same target: the first swap of a target, and the first
    /// of a run of same-shaped in-place row rewrites with nothing between.
    fn push(&mut self, edit: Edit) {
        if let Some(target) = edit.swap_target()
            && self.edits.iter().any(|e| e.swap_target() == Some(target))
        {
            return;
        }
        if let Some(shape) = edit.in_place_shape()
            && let Some(last) = self.edits.iter().rev().find(|e| {
                matches!(e, Edit::Rows { panel, .. } if *panel == shape.0)
                    || matches!(e, Edit::Layout(_))
            })
            && last.in_place_shape().as_ref() == Some(&shape)
        {
            return;
        }
        self.bytes += edit.bytes();
        self.edits.push(edit);
    }

    fn note_selection(&mut self, panel: PanelId, selection: RowSelection) {
        if !self.before.rows.iter().any(|(p, _)| *p == panel) {
            self.before.rows.push((panel, selection));
        }
    }

    /// Join a later step: its edits follow ours, earlier inverses win.
    pub(crate) fn absorb(&mut self, later: Step) {
        for (panel, selection) in later.before.rows {
            self.note_selection(panel, selection);
        }
        for edit in later.edits {
            self.push(edit);
        }
        self.after = later.after;
        self.at = later.at;
        if self.continues.is_none() || later.merge != self.continues {
            // A merge (not a continuation) names the step after its latest part.
            self.label = later.label;
        }
        self.merge = later.merge;
        self.continues = later.continues;
    }

    /// Keep only the edits for which `keep` holds.
    pub(crate) fn retain_edits(&mut self, mut keep: impl FnMut(&Edit) -> bool) {
        self.edits.retain(|e| keep(e));
        self.bytes = self.edits.iter().map(Edit::bytes).sum();
    }

    /// The panels whose content the step changes, most recent last.
    pub(crate) fn touched(&self) -> Vec<PanelId> {
        self.edits.iter().filter_map(Edit::panel).collect()
    }

    /// Whether the step changes the panel structure.
    pub(crate) fn has_layout(&self) -> bool {
        self.edits.iter().any(|e| matches!(e, Edit::Layout(_)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Last {
    Commit,
    Undo,
    Redo,
}

/// The journal: undo and redo lists and the step being recorded.
pub struct History {
    undo: VecDeque<Step>,
    redo: Vec<Step>,
    open: Option<Step>,
    /// Focus and time when the current command began: a step it opens
    /// starts there.
    focus: Option<PanelId>,
    now: Option<Instant>,
    last: Last,
    bytes: usize,
    max_bytes: usize,
    /// Counts every change of the undo and redo lists.
    revision: u64,
    /// Steps dropped by the cap since the history began.
    evicted: u64,
}

impl Default for History {
    fn default() -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            open: None,
            focus: None,
            now: None,
            last: Last::Commit,
            bytes: 0,
            max_bytes: MAX_BYTES,
            revision: 0,
            evicted: 0,
        }
    }
}

impl History {
    fn with_max_bytes(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            ..Self::default()
        }
    }

    /// Forget every step: a new trace, a closed one or a restored workspace
    /// replaced the objects the steps name.
    pub fn clear(&mut self) {
        let (max_bytes, revision) = (self.max_bytes, self.revision + 1);
        *self = Self {
            revision,
            ..Self::with_max_bytes(max_bytes)
        };
    }

    /// Changes whenever the undo or redo list does: menus that show the
    /// labels rebuild when it moves.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Steps the memory cap has dropped since the history began.
    pub fn evicted(&self) -> u64 {
        self.evicted
    }

    /// Set the memory cap (tests force eviction with a small one).
    pub fn set_max_bytes(&mut self, max_bytes: usize) {
        self.max_bytes = max_bytes;
        self.evict();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.back().map(|s| s.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|s| s.label.as_str())
    }

    /// Steps that can be undone, oldest first.
    pub fn undo_steps(&self) -> impl DoubleEndedIterator<Item = &Step> {
        self.undo.iter()
    }

    /// Steps that can be redone, next first.
    pub fn redo_steps(&self) -> impl DoubleEndedIterator<Item = &Step> {
        self.redo.iter().rev()
    }

    /// Bytes held by the undo and redo lists.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// Whether a gesture or command is recording.
    pub fn is_recording(&self) -> bool {
        self.open.is_some()
    }

    /// A command begins: a step it opens starts with this focus, at `now`.
    pub(crate) fn begin_command(&mut self, focus: PanelId, now: Instant) {
        if self.open.is_none() {
            self.focus = Some(focus);
        }
        self.now = Some(now);
    }

    fn open(&mut self) -> &mut Step {
        let focus = self.focus.unwrap_or(PanelId(0));
        let now = self.now.unwrap_or_else(Instant::now);
        let step = self.open.get_or_insert_with(|| Step::new(focus, now));
        step.at = now;
        step
    }

    /// Journal an inverse in the open step; the first label names it.
    pub(crate) fn record(&mut self, edit: Edit, label: Option<String>) {
        let step = self.open();
        if step.label.is_empty()
            && let Some(label) = label
        {
            step.label = label;
        }
        step.push(edit);
    }

    /// Name the step being recorded, whatever its first edit called it.
    pub(crate) fn name_step(&mut self, label: String) {
        self.open().label = label;
    }

    /// The session unit changed: rewrite every time the journal keeps, so
    /// undoing lands where the edit was.
    pub(crate) fn retime(&mut self, by: crate::trace::Rescale) {
        use crate::trace::Retime;
        let steps = self
            .undo
            .iter_mut()
            .chain(self.redo.iter_mut())
            .chain(self.open.iter_mut());
        for edit in steps.flat_map(|step| step.edits.iter_mut()) {
            match edit {
                Edit::Markers { markers, .. } => {
                    for m in markers {
                        m.time.retime(by);
                    }
                }
                Edit::Prop { prop, .. } => prop.retime(by),
                Edit::Layout(structure) => {
                    for panel in &mut structure.reopen {
                        panel.kind.retime(by);
                    }
                }
                Edit::Rows { .. } | Edit::Traces(_) => {}
            }
        }
    }

    /// The selection a wave panel had before the open step first changed it.
    pub(crate) fn note_selection(&mut self, panel: PanelId, selection: RowSelection) {
        self.open().note_selection(panel, selection);
    }

    pub(crate) fn set_merge(&mut self, key: MergeKey) {
        self.open().merge = Some(key);
    }

    pub(crate) fn set_continues(&mut self, key: MergeKey) {
        self.open().continues = Some(key);
    }

    /// The step being recorded, taken out for committing or rolling back.
    pub(crate) fn take_open(&mut self) -> Option<Step> {
        self.open.take()
    }

    /// Resume recording `step`, taken out with [`History::take_open`] so a
    /// step of its own could be committed below it.
    pub(crate) fn resume_open(&mut self, step: Option<Step>) {
        debug_assert!(self.open.is_none(), "another step is recording");
        self.open = step;
    }

    /// The step `step` joins instead of standing alone: the last committed
    /// step when `step` repeats its adjustment within [`MERGE_WINDOW`], or
    /// continues it.
    pub(crate) fn take_merge_target(&mut self, step: &Step) -> Option<Step> {
        let key = step.merge?;
        if self.last != Last::Commit {
            return None;
        }
        let last = self.undo.back()?;
        let repeats =
            last.merge == Some(key) && step.at.saturating_duration_since(last.at) < MERGE_WINDOW;
        let continues = last.continues == Some(key);
        if !(repeats || continues) {
            return None;
        }
        let last = self.undo.pop_back().expect("checked above");
        self.bytes -= last.bytes;
        self.revision += 1;
        Some(last)
    }

    /// Commit a finished step: it empties the redo list.
    pub(crate) fn push(&mut self, step: Step) {
        self.last = Last::Commit;
        if step.is_empty() {
            return;
        }
        self.revision += 1;
        for dropped in self.redo.drain(..) {
            self.bytes -= dropped.bytes;
        }
        self.bytes += step.bytes;
        self.undo.push_back(step);
        self.evict();
    }

    /// Drop the oldest steps over the cap, always keeping the newest.
    fn evict(&mut self) {
        while self.bytes > self.max_bytes && self.undo.len() > 1 {
            let oldest = self.undo.pop_front().expect("more than one step");
            self.bytes -= oldest.bytes;
            self.evicted += 1;
        }
    }

    pub(crate) fn pop_undo(&mut self) -> Option<Step> {
        let step = self.undo.pop_back()?;
        self.bytes -= step.bytes;
        self.revision += 1;
        Some(step)
    }

    pub(crate) fn pop_redo(&mut self) -> Option<Step> {
        let step = self.redo.pop()?;
        self.bytes -= step.bytes;
        self.revision += 1;
        Some(step)
    }

    /// A flipped step goes to the other list.
    pub(crate) fn push_undone(&mut self, mut step: Step) {
        step.bytes = step.edits.iter().map(Edit::bytes).sum();
        self.bytes += step.bytes;
        self.redo.push(step);
        self.last = Last::Undo;
        self.evict();
    }

    pub(crate) fn push_redone(&mut self, mut step: Step) {
        step.bytes = step.edits.iter().map(Edit::bytes).sum();
        self.bytes += step.bytes;
        self.undo.push_back(step);
        self.last = Last::Redo;
        self.evict();
    }
}

/// "1 row", "3 rows".
pub(crate) fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn markers(times: &[u64]) -> Vec<Marker> {
        times
            .iter()
            .map(|&time| Marker {
                id: crate::marker::MarkerId::new(time as u32 + 1).unwrap(),
                time,
                label: None,
            })
            .collect()
    }

    #[test]
    fn a_journaled_field_keeps_its_earliest_value_until_collected() {
        let mut field = Journaled::new(1);
        assert!(field.set(2));
        assert!(field.update(|v| *v = 3));
        assert!(!field.set(3), "an equal value is no change");
        assert_eq!(field.take_before(), Some(1));
        assert_eq!(field.take_before(), None);
        field.set(4);
        field.set(3);
        assert_eq!(field.take_before(), None, "changes that cancel out");
        field.restore(9);
        assert_eq!((*field, field.take_before()), (9, None));
        assert_eq!(field.clone().take_before(), None);
    }

    #[test]
    #[cfg(debug_assertions)]
    fn debug_builds_count_inverses_nobody_collected() {
        let base = pending();
        let mut field = Journaled::new(1);
        field.set(2);
        field.set(3);
        let copy = field.clone();
        assert_eq!(pending(), base + 1, "one per field, none for a copy");
        field.take_before();
        assert_eq!(pending(), base);
        let mut kept = Before::default();
        kept.note(|| 5);
        drop(copy);
        assert_eq!(pending(), base + 1);
        drop(kept);
        assert_eq!(pending(), base, "dropping a held value releases it");
    }

    fn step_with(history: &mut History, at: Instant, edit: Edit, merge: Option<MergeKey>) {
        history.begin_command(PanelId(1), at);
        history.record(edit, Some(format!("{at:?}")));
        if let Some(key) = merge {
            history.set_merge(key);
        }
        let step = history.take_open().unwrap();
        match history.take_merge_target(&step) {
            Some(mut earlier) => {
                earlier.absorb(step);
                history.push(earlier);
            }
            None => history.push(step),
        }
    }

    #[test]
    fn steps_merge_by_key_within_the_window_and_after_a_commit_only() {
        let t = Instant::now();
        let key = MergeKey::of("format", &[1usize]).in_panel(PanelId(1));
        let other = MergeKey::of("format", &[2usize]).in_panel(PanelId(1));
        let mut h = History::default();
        step_with(
            &mut h,
            t,
            Edit::Markers {
                markers: markers(&[1]),
                attached: None,
            },
            Some(key),
        );
        step_with(
            &mut h,
            t + Duration::from_millis(900),
            Edit::Markers {
                markers: markers(&[2]),
                attached: None,
            },
            Some(key),
        );
        assert_eq!(h.undo_steps().count(), 1);
        // The earliest inverse wins.
        assert!(
            matches!(&h.undo.back().unwrap().edits[..], [Edit::Markers { markers: m, .. }] if m[0].time == 1)
        );
        step_with(
            &mut h,
            t + Duration::from_millis(2000),
            Edit::Markers {
                markers: markers(&[3]),
                attached: None,
            },
            Some(key),
        );
        assert_eq!(h.undo_steps().count(), 2, "past the window");
        step_with(
            &mut h,
            t + Duration::from_millis(2100),
            Edit::Markers {
                markers: markers(&[4]),
                attached: None,
            },
            Some(other),
        );
        assert_eq!(h.undo_steps().count(), 3, "another target");
        let undone = h.pop_undo().unwrap();
        h.push_undone(undone);
        step_with(
            &mut h,
            t + Duration::from_millis(2200),
            Edit::Markers {
                markers: markers(&[5]),
                attached: None,
            },
            Some(other),
        );
        assert_eq!(h.undo_steps().count(), 3, "no merge right after an undo");
        assert!(!h.can_redo(), "a new step empties the redo list");
    }

    #[test]
    fn a_continuation_joins_however_late() {
        let t = Instant::now();
        let key = MergeKey::of("rename", &4usize).in_panel(PanelId(1));
        let mut h = History::default();
        h.begin_command(PanelId(1), t);
        h.record(
            Edit::Markers {
                markers: markers(&[1]),
                attached: None,
            },
            Some("Group 2 rows".into()),
        );
        h.set_continues(key);
        let step = h.take_open().unwrap();
        h.push(step);
        step_with(
            &mut h,
            t + Duration::from_secs(60),
            Edit::Markers {
                markers: markers(&[2]),
                attached: None,
            },
            Some(key),
        );
        assert_eq!(h.undo_steps().count(), 1);
        assert_eq!(h.undo_label(), Some("Group 2 rows"));
    }

    #[test]
    fn the_cap_drops_the_oldest_steps_and_keeps_the_newest() {
        let t = Instant::now();
        let mut h = History::default();
        let big = Edit::Markers {
            markers: markers(&(0..100).collect::<Vec<_>>()),
            attached: None,
        };
        let size = big.bytes();
        h.set_max_bytes(size * 2 + size / 2);
        for k in 0..4 {
            step_with(
                &mut h,
                t + Duration::from_secs(k * 5),
                Edit::Markers {
                    markers: markers(&(0..100).collect::<Vec<_>>()),
                    attached: None,
                },
                None,
            );
        }
        assert_eq!(h.undo_steps().count(), 2);
        assert_eq!(h.evicted(), 2);
        assert!(h.bytes() <= h.max_bytes());
        h.set_max_bytes(1);
        assert_eq!(h.undo_steps().count(), 1, "the newest step stays");
        let revision = h.revision();
        h.clear();
        assert!(!h.can_undo() && h.revision() > revision);
    }
}
