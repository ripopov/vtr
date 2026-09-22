//! `TransactionModel`: the state of a Transaction panel. It holds which
//! record it shows, whether it is pinned to it, the records it has shown
//! before, and the reader's per-panel preferences. Data is the document's
//! loaded track; the model never copies records.

use web_time::Instant;

use super::view::{SectionKey, TxView, VIEW_BYTES, ViewPrefs};
use crate::data::text::{COPY_BYTES, Radix};
use crate::data::transactions::{TrackRef, TransactionRef};
use crate::document::{Document, TrackLoadState, TxSelection};
use crate::pipeline::TrackSource;
use crate::remote::memory::{MemoryBudget, Reservation};
use crate::wave::viewport::Viewport;

/// Bytes a Transaction panel admits: one prepared view.
pub const PANEL_BYTES: u64 = VIEW_BYTES as u64;
/// Records Back and Forward step through.
pub const MAX_HISTORY: usize = 64;

/// A record the panel has shown: the durable track reference a workspace
/// saves, plus the identity inside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShownRecord {
    pub track: TrackSource,
    pub id: TransactionRef,
}

impl ShownRecord {
    pub fn selection(&self, origin: crate::panels::PanelId) -> Option<TxSelection> {
        Some(TxSelection {
            track: self.track.track()?,
            id: self.id,
            origin,
        })
    }
}

/// What the panel draws.
pub enum TxPanelState {
    /// Nothing has been selected yet.
    Empty,
    Refused(String),
    /// The record's generator is being loaded (a jump to another track).
    Loading,
    Failed(String),
    /// The saved track is not part of this trace, or holds no such record.
    Missing,
    Ready(Box<TxView>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum TransactionCommand {
    /// Show another record, loading its generator when needed.
    Jump {
        track: TrackRef,
        id: TransactionRef,
    },
    Back,
    Forward,
    Pin(bool),
    /// Move the shared cursor to a time inside the record.
    Cursor(u64),
    /// Fit the shared viewport to the record's lifetime.
    FitLifetime,
    /// Cycle the radix of one attribute key, remembered for the session.
    Radix(String),
    Collapse(SectionKey, bool),
    Filter(String),
}

pub struct TransactionModel {
    /// Records shown, oldest first; `cursor` is the one on screen.
    history: Vec<ShownRecord>,
    cursor: usize,
    /// A pinned panel ignores the document selection, so two panels compare.
    pub pinned: bool,
    pub prefs: ViewPrefs,
    /// The track retained for the shown record, released when it changes.
    retained: Option<TrackRef>,
    attached: bool,
    refused: Option<String>,
    budget: MemoryBudget,
    _reservation: Option<Reservation>,
}

impl TransactionModel {
    pub fn new(budget: MemoryBudget, detail_items: usize) -> Self {
        let reservation = budget.reserve(PANEL_BYTES);
        let refused = reservation.as_ref().err().map(|error| {
            format!("Transaction panel needs {PANEL_BYTES} bytes; admission failed: {error}")
        });
        Self {
            history: Vec::new(),
            cursor: 0,
            pinned: false,
            prefs: ViewPrefs {
                detail_items: detail_items.clamp(1, 1000),
                ..ViewPrefs::default()
            },
            retained: None,
            attached: false,
            refused,
            budget,
            _reservation: reservation.ok(),
        }
    }

    /// Copy the record and the reader's choices for a split; the copy is not
    /// attached, so it retains its track when it is installed.
    pub fn clone_view(&self) -> Self {
        let mut clone = Self::new(self.budget.clone(), self.prefs.detail_items);
        clone.prefs = self.prefs.clone();
        clone.pinned = self.pinned;
        if let Some(shown) = self.shown() {
            clone.history = vec![shown.clone()];
        }
        clone
    }

    pub fn shown(&self) -> Option<&ShownRecord> {
        self.history.get(self.cursor)
    }

    pub fn can_go_back(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.cursor + 1 < self.history.len()
    }

    pub fn set_detail_items(&mut self, value: usize) {
        self.prefs.detail_items = value.clamp(1, 1000);
    }

    /// Install a record without a document (workspace restore); the track is
    /// retained when the panel attaches.
    pub(crate) fn restore(&mut self, record: ShownRecord, pinned: bool, prefs: ViewPrefs) {
        self.history = vec![record];
        self.cursor = 0;
        self.pinned = pinned;
        self.restore_prefs(prefs);
    }

    /// The reader's saved choices, keeping the resolved item limit.
    pub(crate) fn restore_prefs(&mut self, prefs: ViewPrefs) {
        let detail_items = self.prefs.detail_items;
        self.prefs = ViewPrefs {
            detail_items,
            ..prefs
        };
    }

    pub fn is_attached(&self) -> bool {
        self.attached
    }

    /// Retain the shown record's track. Pair with [`TransactionModel::detach`].
    pub fn attach(&mut self, doc: &mut Document) -> anyhow::Result<()> {
        if self.attached {
            return Ok(());
        }
        self.attached = true;
        if self.refused.is_some() {
            return Ok(());
        }
        if let Some(track) = self.shown().and_then(|shown| shown.track.track()) {
            doc.retain_track(track)?;
            self.retained = Some(track);
        }
        Ok(())
    }

    pub fn detach(&mut self, doc: &mut Document) {
        if !self.attached {
            return;
        }
        if let Some(track) = self.retained.take() {
            doc.release_track(track);
        }
        self.attached = false;
    }

    /// Retain `track` in place of the one held, once attached. Returns false
    /// when the document refuses it; the previous hold is then kept.
    fn hold(&mut self, doc: &mut Document, track: Option<TrackRef>) -> bool {
        if !self.attached || self.retained == track {
            return true;
        }
        if let Some(track) = track
            && doc.retain_track(track).is_err()
        {
            return false;
        }
        if let Some(previous) = std::mem::replace(&mut self.retained, track) {
            doc.release_track(previous);
        }
        true
    }

    /// Show `record`, retaining its track and remembering the previous one.
    /// A repeat of the shown record changes nothing.
    fn show(&mut self, doc: &mut Document, record: ShownRecord) -> bool {
        if self.shown() == Some(&record) || !self.hold(doc, record.track.track()) {
            return false;
        }
        self.history.truncate(self.cursor + 1);
        self.history.push(record);
        if self.history.len() > MAX_HISTORY {
            let excess = self.history.len() - MAX_HISTORY;
            self.history.drain(..excess);
        }
        self.cursor = self.history.len() - 1;
        true
    }

    /// Step to `cursor` in the history and publish that record as the
    /// document selection, so the panels that show it highlight it again.
    fn step(&mut self, doc: &mut Document, panel: crate::panels::PanelId, cursor: usize) -> bool {
        let Some(track) = self.history.get(cursor).map(|r| r.track.track()) else {
            return false;
        };
        if !self.hold(doc, track) {
            return false;
        }
        self.cursor = cursor;
        if let Some(selection) = self.shown().and_then(|shown| shown.selection(panel)) {
            doc.select(Some(selection));
        }
        true
    }

    /// Adopt the document selection unless this panel is pinned. Returns
    /// whether the shown record changed.
    pub fn follow_selection(&mut self, doc: &mut Document) -> bool {
        if self.pinned {
            return false;
        }
        // Clearing a highlight elsewhere must not blank the page being read.
        let Some(selection) = doc.selection() else {
            return false;
        };
        let Some(path) = track_path(doc, selection.track) else {
            return false;
        };
        self.show(
            doc,
            ShownRecord {
                track: TrackSource::Resolved {
                    track: selection.track,
                    path,
                },
                id: selection.id,
            },
        )
    }

    /// Everything the panel draws for the record it shows.
    pub fn state(&self, doc: &Document) -> TxPanelState {
        if let Some(refusal) = &self.refused {
            return TxPanelState::Refused(refusal.clone());
        }
        let Some(shown) = self.shown() else {
            return TxPanelState::Empty;
        };
        let Some(track) = shown.track.track() else {
            return TxPanelState::Missing;
        };
        match doc.track(track) {
            Some(TrackLoadState::Loading) => return TxPanelState::Loading,
            Some(TrackLoadState::Failed(error)) => return TxPanelState::Failed(error.clone()),
            _ => {}
        }
        match super::view::view(doc, track, shown.id, &self.prefs) {
            Some(view) => TxPanelState::Ready(Box::new(view)),
            None if doc.track(track).is_none() => TxPanelState::Loading,
            None => TxPanelState::Missing,
        }
    }

    /// The complete record as TSV, refused rather than cut.
    pub fn copy_tsv(&self, doc: &Document) -> anyhow::Result<String> {
        let shown = self
            .shown()
            .ok_or_else(|| anyhow::anyhow!("No record is shown."))?;
        let track = shown
            .track
            .track()
            .ok_or_else(|| anyhow::anyhow!("Record is not loaded."))?;
        super::view::copy_tsv(doc, track, shown.id, COPY_BYTES)
    }

    /// The record's lifetime, when it is resident.
    pub fn lifetime(&self, doc: &Document) -> Option<(u64, u64)> {
        let shown = self.shown()?;
        let generator = doc.resident_generator(shown.track.track()?)?;
        let tx = generator.transaction(shown.id)?;
        Some((tx.begin, tx.end))
    }

    /// Handle a panel command. `panel` owns the selection this panel writes
    /// when it jumps. Returns true when something visible changed.
    pub fn command(
        &mut self,
        doc: &mut Document,
        panel: crate::panels::PanelId,
        command: TransactionCommand,
        now: Instant,
    ) -> bool {
        match command {
            TransactionCommand::Jump { track, id } => {
                let Some(path) = track_path(doc, track) else {
                    return false;
                };
                let changed = self.show(
                    doc,
                    ShownRecord {
                        track: TrackSource::Resolved { track, path },
                        id,
                    },
                );
                // A jump is a selection: every panel showing that generator
                // highlights the record, pinned panels excepted.
                doc.select(Some(TxSelection {
                    track,
                    id,
                    origin: panel,
                }));
                changed
            }
            TransactionCommand::Back => {
                self.can_go_back() && self.step(doc, panel, self.cursor - 1)
            }
            TransactionCommand::Forward => {
                self.can_go_forward() && self.step(doc, panel, self.cursor + 1)
            }
            TransactionCommand::Pin(pinned) => {
                let changed = self.pinned != pinned;
                self.pinned = pinned;
                if changed && !pinned {
                    self.follow_selection(doc);
                }
                changed
            }
            TransactionCommand::Cursor(time) => {
                let changed = doc.shared.cursor != Some(time);
                doc.shared.cursor = Some(time);
                changed
            }
            TransactionCommand::FitLifetime => {
                let Some((begin, end)) = self.lifetime(doc) else {
                    return false;
                };
                let pad = ((end.saturating_sub(begin)) as f64 * 0.05).max(1.0);
                let mut target = Viewport {
                    start: begin as f64 - pad,
                    end: end as f64 + pad,
                };
                target.clamp(doc.limits());
                let animation = doc.navigation.animation;
                doc.shared.viewport.animate_to(target, now, animation);
                true
            }
            TransactionCommand::Radix(key) => {
                let next = self.prefs.radix_of(&key).next();
                if next == Radix::default() {
                    self.prefs.radix.remove(&key);
                } else {
                    self.prefs.radix.insert(key, next);
                }
                true
            }
            TransactionCommand::Collapse(section, collapsed) => {
                if collapsed {
                    self.prefs.collapsed.insert(section)
                } else {
                    self.prefs.collapsed.remove(&section)
                }
            }
            TransactionCommand::Filter(text) => {
                let changed = self.prefs.filter != text;
                self.prefs.filter = text;
                changed
            }
        }
    }
}

/// The catalog path of a track of the open trace.
fn track_path(doc: &Document, track: TrackRef) -> Option<Vec<String>> {
    doc.session()?
        .tracks()
        .iter()
        .find(|t| t.id == track)
        .map(|t| t.path.clone())
}
