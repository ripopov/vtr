//! The prepared view of one recorded transaction: a pure function of the
//! resident record, the document's time base and the reader's preferences.
//! Nothing here touches a reader, a transport or a panel; the frontends only
//! render what this produces, and the headless tests assert on it directly.

use std::collections::{BTreeMap, BTreeSet};

use crate::data::loaded_tracks::{LoadedGenerator, TransactionLocation};
use crate::data::text::{
    PREVIEW_BYTES, Radix, attribute_value_min_bytes, format_attribute, format_attribute_radix,
    is_integer, join_path_limited, truncate_ref,
};
use crate::data::transactions::{
    AttributeValue, Attributes, TrackKind, TrackRef, Transaction, TransactionRef, TxKind, TxStatus,
};
use crate::document::Document;
use crate::pipeline::model::LABEL_ATTRIBUTE;
use crate::pipeline::palette::{StagePalette, StageStyle};
use crate::wave::timeline::format_time;

/// Bytes one prepared view may materialize. A record whose attributes exceed
/// it is cut with the total still stated, as the table cuts a row.
pub const VIEW_BYTES: usize = 128 * 1024;
/// Attributes above this count get a filter box in the frontends.
pub const FILTER_THRESHOLD: usize = 8;
/// Attribute keys carrying a folded FTR phase (`docs/COVERAGE.md`).
const PHASES: [&str; 2] = ["record", "end"];

/// A collapsible section, addressed by both frontends and the workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SectionKey {
    Attributes,
    Stages,
    Events,
    Related,
}

impl SectionKey {
    pub const ALL: [SectionKey; 4] = [
        SectionKey::Attributes,
        SectionKey::Stages,
        SectionKey::Events,
        SectionKey::Related,
    ];

    pub fn key(self) -> &'static str {
        match self {
            SectionKey::Attributes => "attributes",
            SectionKey::Stages => "stages",
            SectionKey::Events => "events",
            SectionKey::Related => "related",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            SectionKey::Attributes => "Attributes",
            SectionKey::Stages => "Stages",
            SectionKey::Events => "Events",
            SectionKey::Related => "Related",
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|section| section.key() == key)
    }
}

/// What the reader chose, not what the recording says: the per-key radix, the
/// collapsed sections, the attribute filter and the per-section row limit.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewPrefs {
    pub detail_items: usize,
    pub radix: BTreeMap<String, Radix>,
    pub collapsed: BTreeSet<SectionKey>,
    pub filter: String,
}

impl Default for ViewPrefs {
    fn default() -> Self {
        Self {
            detail_items: 20,
            radix: BTreeMap::new(),
            collapsed: BTreeSet::new(),
            filter: String::new(),
        }
    }
}

impl ViewPrefs {
    pub fn radix_of(&self, key: &str) -> Radix {
        self.radix.get(key).copied().unwrap_or_default()
    }
}

/// A bounded list whose heading always states the recorded total.
#[derive(Clone, Debug, PartialEq)]
pub struct Section<T> {
    pub key: SectionKey,
    /// Entries in the record.
    pub total: usize,
    /// Entries left by the filter; equal to `total` when nothing filters.
    pub matched: usize,
    pub collapsed: bool,
    /// At most `ViewPrefs::detail_items` rows.
    pub rows: Vec<T>,
}

impl<T> Section<T> {
    pub fn cut(&self) -> bool {
        self.rows.len() < self.matched
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Identity {
    pub track: TrackRef,
    /// The owning stream's path; empty when the generator has no stream.
    pub stream: Vec<String>,
    pub generator: String,
    /// Position in the generator's record order.
    pub ordinal: usize,
    pub id: TransactionRef,
    /// `vtr.label`; the title, never an attribute row.
    pub label: Option<String>,
    pub status: TxStatus,
    /// The word on the badge; `TxStatus::name` is its tooltip.
    pub status_text: &'static str,
    /// The OpenTelemetry span kind, when the producer set one.
    pub kind: Option<&'static str>,
    pub parent: Option<TransactionRef>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Timing {
    pub begin: u64,
    /// `None` while the record is open; never a fabricated end.
    pub end: Option<u64>,
    pub duration: u64,
    pub begin_text: String,
    pub end_text: String,
    /// `≥` prefixed while open.
    pub duration_text: String,
}

/// One lane of the record's own Gantt, the primary lane first.
#[derive(Clone, Debug, PartialEq)]
pub struct LaneRow {
    pub lane: String,
    pub primary: bool,
    pub cells: Vec<LaneCell>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LaneCell {
    pub name: String,
    pub begin: u64,
    pub end: u64,
    /// Offset and width as fractions of the lifetime, for any pixel width.
    pub start: f32,
    pub width: f32,
    pub style: StageStyle,
    /// Index of the stage in the record, or `None` for a stageless lifetime.
    pub stage: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EventTick {
    pub name: String,
    pub time: u64,
    pub at: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AttrRow {
    /// The key without its folded phase suffix.
    pub key: String,
    /// The whole key, as recorded; what a radix choice is remembered by.
    pub full_key: String,
    pub phase: Option<&'static str>,
    pub value: String,
    /// The radix in effect, for integers only.
    pub radix: Option<Radix>,
    pub kind: &'static str,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Chip {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StageRow {
    pub name: String,
    pub lane: String,
    pub primary: bool,
    pub begin: u64,
    pub end: Option<u64>,
    pub duration: u64,
    /// Share of the lifetime, in `[0, 1]`.
    pub share: f32,
    pub style: StageStyle,
    pub begin_text: String,
    pub end_text: String,
    pub duration_text: String,
    pub attributes: Vec<Chip>,
    pub attributes_total: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EventRow {
    pub name: String,
    pub time: u64,
    pub time_text: String,
    pub attributes: Vec<Chip>,
    pub attributes_total: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefRole {
    Parent,
    Child,
    /// A recorded relation; `outgoing` is this record as the source.
    Relation {
        outgoing: bool,
    },
}

/// One reachable record: the parent, a child, or a relation's other end.
#[derive(Clone, Debug, PartialEq)]
pub struct RefRow {
    pub role: RefRole,
    /// `Parent`, `Children`, or `wakeup · from`.
    pub group: String,
    pub target: TransactionLocation,
    /// The target's caption, when its generator is loaded.
    pub label: Option<String>,
    /// The target's generator path when it is not this record's.
    pub track: Option<Vec<String>>,
    /// The relation's own `vtr.label`, when it has one.
    pub relation_label: Option<String>,
    /// False when the target's generator is not resident; a jump loads it.
    pub loaded: bool,
}

/// Everything a Transaction panel draws for one record.
#[derive(Clone, Debug, PartialEq)]
pub struct TxView {
    pub identity: Identity,
    pub timing: Timing,
    pub lifeline: Vec<LaneRow>,
    pub event_ticks: Vec<EventTick>,
    /// The shared cursor as a fraction of the lifetime, when it falls inside.
    pub cursor: Option<f32>,
    pub attributes: Section<AttrRow>,
    pub stages: Section<StageRow>,
    pub events: Section<EventRow>,
    pub related: Section<RefRow>,
    /// The admitted byte budget cut a value short.
    pub truncated_bytes: bool,
}

impl TxView {
    /// Whether the frontends offer a filter box over the attributes.
    pub fn filterable(&self) -> bool {
        self.attributes.total > FILTER_THRESHOLD
    }
}

/// Build the view of `id` in `track`, or `None` when the generator is not
/// resident or holds no such record.
pub fn view(
    doc: &Document,
    track: TrackRef,
    id: TransactionRef,
    prefs: &ViewPrefs,
) -> Option<TxView> {
    let generator = doc.resident_generator(track)?;
    let tx = generator.transaction(id)?;
    let session = doc.session()?;
    let catalog = session.tracks();
    let declaration = catalog.iter().find(|t| t.id == track)?;
    let stream = match declaration.kind {
        TrackKind::Generator { stream } => catalog
            .iter()
            .find(|t| t.id == stream)
            .map(|t| t.path.clone())
            .unwrap_or_default(),
        TrackKind::Stream { .. } => Vec::new(),
    };
    let base = doc.time_base();
    let time = |t: u64| format_time(t as f64, base);
    let palette = StagePalette::build(std::slice::from_ref(&generator));
    let limit = prefs.detail_items.clamp(1, 1000);
    let mut budget = Budget::new(VIEW_BYTES);

    let label = tx
        .attributes
        .iter()
        .find(|a| a.key == LABEL_ATTRIBUTE)
        .map(|a| budget.text(&format_attribute(&a.value, PREVIEW_BYTES)));
    let identity = Identity {
        track,
        stream,
        generator: declaration.path.last().cloned().unwrap_or_default(),
        ordinal: generator.transaction_ordinal(id).unwrap_or(0),
        id,
        label,
        status: tx.status,
        status_text: status_text(tx.status),
        kind: kind_name(tx.kind),
        parent: tx.parent,
    };

    let open = tx.status == TxStatus::Open;
    let duration = tx.end.saturating_sub(tx.begin);
    let timing = Timing {
        begin: tx.begin,
        end: (!open).then_some(tx.end),
        duration,
        begin_text: time(tx.begin),
        end_text: if open { "open".into() } else { time(tx.end) },
        duration_text: if open {
            format!("≥ {}", time(duration))
        } else {
            time(duration)
        },
    };

    let span = duration.max(1) as f64;
    let fraction = |t: u64| ((t.saturating_sub(tx.begin) as f64) / span).clamp(0.0, 1.0) as f32;
    let (lifeline, event_ticks) = lifeline(tx, &palette, &fraction);
    let cursor = doc
        .shared
        .cursor
        .filter(|c| (tx.begin..=tx.end.max(tx.begin)).contains(c))
        .map(fraction);

    let attributes = attributes(tx, prefs, limit, &mut budget);
    let mut stages = stages(tx, &palette, limit, duration, &time, &mut budget);
    let mut events = events(tx, limit, &time, &mut budget);
    let mut related = related(doc, &generator, tx, track, limit, &mut budget);
    stages.collapsed = prefs.collapsed.contains(&SectionKey::Stages);
    events.collapsed = prefs.collapsed.contains(&SectionKey::Events);
    related.collapsed = prefs.collapsed.contains(&SectionKey::Related);

    Some(TxView {
        identity,
        timing,
        lifeline,
        event_ticks,
        cursor,
        attributes,
        stages,
        events,
        related,
        truncated_bytes: budget.truncated,
    })
}

/// The byte ledger of one prepared view: every string is admitted through it
/// so a pathological record cuts its values instead of its counts.
struct Budget {
    remaining: usize,
    truncated: bool,
}

impl Budget {
    fn new(bytes: usize) -> Self {
        Self {
            remaining: bytes,
            truncated: false,
        }
    }

    /// A string bounded by what is left, never more than one preview.
    fn text(&mut self, value: &str) -> String {
        let room = self.remaining.min(PREVIEW_BYTES);
        let out = truncate_ref(value, room);
        self.truncated |= out.len() < value.len();
        self.remaining = self.remaining.saturating_sub(out.len());
        out
    }

    /// An attribute rendered in `radix`, bounded the same way.
    fn value(&mut self, value: &AttributeValue, radix: Radix) -> (String, bool) {
        let room = self.remaining.min(PREVIEW_BYTES);
        let out = format_attribute_radix(value, radix, room);
        let cut = attribute_value_min_bytes(value) > out.len();
        self.truncated |= cut;
        self.remaining = self.remaining.saturating_sub(out.len());
        (out, cut)
    }

    /// Whether another row still fits.
    fn open(&mut self) -> bool {
        if self.remaining == 0 {
            self.truncated = true;
            return false;
        }
        true
    }
}

fn status_text(status: TxStatus) -> &'static str {
    match status {
        // `Unset` means the record ended without a status, not that it failed.
        TxStatus::Unset => "ended",
        TxStatus::Ok => "ok",
        TxStatus::Error => "error",
        TxStatus::Aborted => "aborted",
        TxStatus::Open => "open",
    }
}

fn kind_name(kind: TxKind) -> Option<&'static str> {
    match kind {
        TxKind::Unspecified => None,
        TxKind::Internal => Some("internal"),
        TxKind::Server => Some("server"),
        TxKind::Client => Some("client"),
        TxKind::Producer => Some("producer"),
        TxKind::Consumer => Some("consumer"),
    }
}

fn value_kind(value: &AttributeValue) -> &'static str {
    match value {
        AttributeValue::Null => "null",
        AttributeValue::Bool(_) => "bool",
        AttributeValue::I64(_) | AttributeValue::U64(_) => "int",
        AttributeValue::F64(_) => "real",
        AttributeValue::Text(_) => "text",
        AttributeValue::Bytes(_) => "bytes",
        AttributeValue::Logic { .. } => "logic",
        AttributeValue::Time(_) => "time",
        AttributeValue::Enum { .. } => "enum",
        AttributeValue::Pointer(_) => "pointer",
        AttributeValue::Fixed { .. } | AttributeValue::UFixed { .. } => "fixed",
        AttributeValue::List(_) => "list",
        AttributeValue::Map(_) => "map",
    }
}

/// Split a folded FTR phase suffix off a key (`addr.end` → `addr`, `end`).
fn split_phase(key: &str) -> (&str, Option<&'static str>) {
    for phase in PHASES {
        if let Some(base) = key.strip_suffix(phase)
            && let Some(base) = base.strip_suffix('.')
            && !base.is_empty()
        {
            return (base, Some(phase));
        }
    }
    (key, None)
}

fn lifeline(
    tx: &Transaction,
    palette: &StagePalette,
    fraction: &impl Fn(u64) -> f32,
) -> (Vec<LaneRow>, Vec<EventTick>) {
    let primary = palette.primary_lane();
    let mut lanes: Vec<LaneRow> = vec![LaneRow {
        lane: primary.to_owned(),
        primary: true,
        cells: Vec::new(),
    }];
    for (index, stage) in tx.stages.iter().enumerate() {
        let begin = stage.begin;
        let end = crate::pipeline::PipelineModel::stage_end(tx, stage);
        let start = fraction(begin);
        let cell = LaneCell {
            name: stage.name.clone(),
            begin,
            end,
            start,
            width: (fraction(end) - start).max(0.0),
            style: if stage.lane == primary {
                palette.style(&stage.name)
            } else {
                StagePalette::fallback()
            },
            stage: Some(index),
        };
        match lanes.iter_mut().find(|row| row.lane == stage.lane) {
            Some(row) => row.cells.push(cell),
            None => lanes.push(LaneRow {
                lane: stage.lane.clone(),
                primary: false,
                cells: vec![cell],
            }),
        }
    }
    if tx.stages.is_empty() {
        // The pipeline paints a stageless record as one cell over its
        // lifetime; the lifeline shows the same shape.
        lanes[0].cells.push(LaneCell {
            name: String::new(),
            begin: tx.begin,
            end: tx.end,
            start: 0.0,
            width: 1.0,
            style: StagePalette::fallback(),
            stage: None,
        });
    }
    lanes.retain(|row| !row.cells.is_empty());
    let ticks = tx
        .events
        .iter()
        .map(|event| EventTick {
            name: event.name.clone(),
            time: event.time,
            at: fraction(event.time),
        })
        .collect();
    (lanes, ticks)
}

fn attributes(
    tx: &Transaction,
    prefs: &ViewPrefs,
    limit: usize,
    budget: &mut Budget,
) -> Section<AttrRow> {
    let filter = prefs.filter.trim().to_lowercase();
    // The caption is the panel's title, so it is never an attribute row.
    let listed = tx.attributes.iter().filter(|a| a.key != LABEL_ATTRIBUTE);
    let total = listed.clone().count();
    let matching = listed.filter(|a| {
        filter.is_empty()
            || a.key.to_lowercase().contains(&filter)
            || format_attribute(&a.value, PREVIEW_BYTES)
                .to_lowercase()
                .contains(&filter)
    });
    let matched = matching.clone().count();
    let mut rows = Vec::new();
    for attribute in matching.take(limit) {
        if !budget.open() {
            break;
        }
        let radix = is_integer(&attribute.value).then(|| prefs.radix_of(&attribute.key));
        let (value, truncated) = budget.value(&attribute.value, radix.unwrap_or_default());
        let (key, phase) = split_phase(&attribute.key);
        rows.push(AttrRow {
            key: budget.text(key),
            full_key: attribute.key.clone(),
            phase,
            value,
            radix,
            kind: value_kind(&attribute.value),
            truncated,
        });
    }
    Section {
        key: SectionKey::Attributes,
        total,
        matched,
        collapsed: prefs.collapsed.contains(&SectionKey::Attributes),
        rows,
    }
}

fn chips(attributes: &Attributes, limit: usize, budget: &mut Budget) -> Vec<Chip> {
    let mut chips = Vec::new();
    for (key, value) in attributes.iter().take(limit) {
        if !budget.open() {
            break;
        }
        let key = budget.text(key);
        let (value, _) = budget.value(value, Radix::Dec);
        chips.push(Chip { key, value });
    }
    chips
}

fn stages(
    tx: &Transaction,
    palette: &StagePalette,
    limit: usize,
    lifetime: u64,
    time: &impl Fn(u64) -> String,
    budget: &mut Budget,
) -> Section<StageRow> {
    let primary = palette.primary_lane();
    // Grouped by lane, the primary lane first, each lane in record order.
    let mut order: Vec<usize> = (0..tx.stages.len()).collect();
    let mut lanes: Vec<&str> = Vec::new();
    for stage in &tx.stages {
        if !lanes.contains(&stage.lane.as_str()) {
            lanes.push(&stage.lane);
        }
    }
    let lane_rank = |lane: &str| {
        if lane == primary {
            0
        } else {
            1 + lanes.iter().position(|l| *l == lane).unwrap_or(0)
        }
    };
    order.sort_by_key(|&index| (lane_rank(&tx.stages[index].lane), index));
    let mut rows = Vec::new();
    for index in order.into_iter().take(limit) {
        if !budget.open() {
            break;
        }
        let stage = &tx.stages[index];
        let end = crate::pipeline::PipelineModel::stage_end(tx, stage);
        let duration = end.saturating_sub(stage.begin);
        let attributes_total = stage.attributes.len();
        rows.push(StageRow {
            name: budget.text(&stage.name),
            lane: budget.text(&stage.lane),
            primary: stage.lane == primary,
            begin: stage.begin,
            end: stage.end,
            duration,
            share: if lifetime == 0 {
                1.0
            } else {
                (duration as f64 / lifetime as f64).clamp(0.0, 1.0) as f32
            },
            style: if stage.lane == primary {
                palette.style(&stage.name)
            } else {
                StagePalette::fallback()
            },
            begin_text: time(stage.begin),
            end_text: match stage.end {
                Some(end) => time(end),
                None => "open".into(),
            },
            duration_text: time(duration),
            attributes: chips(&stage.attributes, limit, budget),
            attributes_total,
        });
    }
    Section {
        key: SectionKey::Stages,
        total: tx.stages.len(),
        matched: tx.stages.len(),
        collapsed: false,
        rows,
    }
}

fn events(
    tx: &Transaction,
    limit: usize,
    time: &impl Fn(u64) -> String,
    budget: &mut Budget,
) -> Section<EventRow> {
    let mut order: Vec<usize> = (0..tx.events.len()).collect();
    order.sort_by_key(|&index| (tx.events[index].time, index));
    let mut rows = Vec::new();
    for index in order.into_iter().take(limit) {
        if !budget.open() {
            break;
        }
        let event = &tx.events[index];
        rows.push(EventRow {
            name: budget.text(&event.name),
            time: event.time,
            time_text: time(event.time),
            attributes_total: event.attributes.len(),
            attributes: chips(&event.attributes, limit, budget),
        });
    }
    Section {
        key: SectionKey::Events,
        total: tx.events.len(),
        matched: tx.events.len(),
        collapsed: false,
        rows,
    }
}

fn related(
    doc: &Document,
    generator: &LoadedGenerator,
    tx: &Transaction,
    track: TrackRef,
    limit: usize,
    budget: &mut Budget,
) -> Section<RefRow> {
    let here = TransactionLocation {
        transaction: tx.id,
        generator: track,
    };
    let mut entries: Vec<(String, TransactionLocation, RefRole, Option<String>)> = Vec::new();
    if let Some(parent) = generator.parent(tx.id) {
        entries.push(("Parent".into(), parent, RefRole::Parent, None));
    }
    // A child may be recorded in any loaded generator: VTR links the child to
    // its parent, so the reverse edge is only known where the child lives.
    for other in doc.resident_generators() {
        for &child in other.children(here) {
            entries.push((
                "Children".into(),
                TransactionLocation {
                    transaction: child,
                    generator: other.generator(),
                },
                RefRole::Child,
                None,
            ));
        }
    }
    // Relations are grouped by kind, then outgoing before incoming; within a
    // group they keep recording order.
    let mut edges: Vec<_> = generator.relations_of(tx.id).collect();
    edges.sort_by(|a, b| {
        let outgoing = |e: &&crate::data::loaded_tracks::LoadedRelation| {
            e.relation.from == tx.id && e.from_generator == track
        };
        (&a.relation.kind, !outgoing(a)).cmp(&(&b.relation.kind, !outgoing(b)))
    });
    for edge in edges {
        let outgoing = edge.relation.from == tx.id && edge.from_generator == track;
        let (target, target_generator) = if outgoing {
            (edge.relation.to, edge.to_generator)
        } else {
            (edge.relation.from, edge.from_generator)
        };
        let label = edge
            .relation
            .attributes
            .iter()
            .find(|(key, _)| key == LABEL_ATTRIBUTE)
            .map(|(_, value)| format_attribute(value, PREVIEW_BYTES));
        entries.push((
            format!(
                "{} · {}",
                edge.relation.kind,
                if outgoing { "to" } else { "from" }
            ),
            TransactionLocation {
                transaction: target,
                generator: target_generator,
            },
            RefRole::Relation { outgoing },
            label,
        ));
    }
    let total = entries.len();
    let catalog = doc.session().map(|s| s.tracks()).unwrap_or(&[]);
    let mut rows = Vec::new();
    for (group, target, role, relation_label) in entries.into_iter().take(limit) {
        if !budget.open() {
            break;
        }
        let resident = doc.resident_generator(target.generator);
        let label = resident
            .as_ref()
            .and_then(|g| g.transaction(target.transaction))
            .map(crate::pipeline::PipelineModel::label)
            .filter(|label| !label.is_empty())
            .map(|label| budget.text(&label));
        let path = (target.generator != track)
            .then(|| {
                catalog
                    .iter()
                    .find(|t| t.id == target.generator)
                    .map(|t| t.path.clone())
            })
            .flatten();
        rows.push(RefRow {
            role,
            group: budget.text(&group),
            target,
            label,
            track: path,
            relation_label: relation_label.map(|label| budget.text(&label)),
            loaded: resident.is_some(),
        });
    }
    Section {
        key: SectionKey::Related,
        total,
        matched: total,
        collapsed: false,
        rows,
    }
}

/// The complete record as TSV, refused rather than cut past `limit` bytes.
pub fn copy_tsv(
    doc: &Document,
    track: TrackRef,
    id: TransactionRef,
    limit: usize,
) -> anyhow::Result<String> {
    let generator = doc
        .resident_generator(track)
        .ok_or_else(|| anyhow::anyhow!("Record is not loaded."))?;
    let tx = generator
        .transaction(id)
        .ok_or_else(|| anyhow::anyhow!("Record is not loaded."))?;
    let catalog = doc.session().map(|s| s.tracks()).unwrap_or(&[]);
    let mut truncated = false;
    let path = catalog
        .iter()
        .find(|t| t.id == track)
        .map(|t| join_path_limited(&t.path, limit, &mut truncated))
        .unwrap_or_default();
    let mut text = String::new();
    let row = |text: &mut String, cells: &[&str]| -> anyhow::Result<()> {
        crate::data::text::append_exact(text, &cells.join("\t"), limit)?;
        crate::data::text::append_exact(text, "\n", limit)
    };
    row(
        &mut text,
        &[
            "Generator",
            "ID",
            "Begin",
            "End",
            "Duration",
            "Status",
            "Kind",
        ],
    )?;
    row(
        &mut text,
        &[
            &path,
            &tx.id.0.to_string(),
            &tx.begin.to_string(),
            &tx.end.to_string(),
            &tx.end.saturating_sub(tx.begin).to_string(),
            tx.status.name(),
            kind_name(tx.kind).unwrap_or("—"),
        ],
    )?;
    if !tx.attributes.is_empty() {
        row(&mut text, &["Attribute", "Value"])?;
        for attribute in &tx.attributes {
            let value = format_attribute(&attribute.value, limit);
            row(&mut text, &[&attribute.key, &value])?;
        }
    }
    if !tx.stages.is_empty() {
        row(&mut text, &["Stage", "Lane", "Begin", "End", "Attributes"])?;
        for stage in &tx.stages {
            let (attributes, _) = crate::data::text::format_attributes(&stage.attributes, limit);
            row(
                &mut text,
                &[
                    &stage.name,
                    &stage.lane,
                    &stage.begin.to_string(),
                    &stage
                        .end
                        .map_or_else(|| "open".into(), |end| end.to_string()),
                    &attributes,
                ],
            )?;
        }
    }
    if !tx.events.is_empty() {
        row(&mut text, &["Event", "Time", "Attributes"])?;
        for event in &tx.events {
            let (attributes, _) = crate::data::text::format_attributes(&event.attributes, limit);
            row(
                &mut text,
                &[&event.name, &event.time.to_string(), &attributes],
            )?;
        }
    }
    anyhow::ensure!(!truncated, "Record exceeds the 64 KiB clipboard limit.");
    Ok(text)
}
