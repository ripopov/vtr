//! The traces a document shows together (`docs/multiple-traces.html`).
//!
//! Every open file is a [`TraceSlot`] of the document's [`TraceSet`], named
//! by a letter: A is the first. Everything that names trace data — a signal,
//! a variable, a scope, a track, a clock — is local to one trace's session,
//! so the rest of the viewer pairs it with its [`TraceId`] as a
//! [`Traced`] value. There is no primary trace: a panel or row that shows
//! trace data says which trace it comes from.
//!
//! Each trace keeps its own time unit on disk. The set places every trace on
//! one session timeline ([`Placement`]) in the finest unit among them, so an
//! FST in picoseconds and a VTR in nanoseconds share one ruler exactly.

mod names;
mod placement;

use std::sync::Arc;

pub use names::derive_names;
pub use placement::Placement;

use crate::data::TraceInfo;
use crate::session::Session;

/// One trace of the set, by its letter. Letters are handed out lowest free
/// first and never change while the trace is open, so a workspace names
/// rows and panels by them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TraceId(u8);

impl TraceId {
    /// The first trace opened.
    pub const A: TraceId = TraceId(0);
    /// Letters A to Z.
    pub const MAX: usize = 26;

    /// The `index`th letter (0 is A).
    pub fn new(index: usize) -> Option<Self> {
        (index < Self::MAX).then_some(TraceId(index as u8))
    }

    pub fn index(self) -> usize {
        usize::from(self.0)
    }

    pub fn letter(self) -> char {
        char::from(b'A' + self.0)
    }

    pub fn from_letter(letter: char) -> Option<Self> {
        letter
            .is_ascii_uppercase()
            .then(|| TraceId(letter as u8 - b'A'))
    }

    /// Whether this is A, which workspaces leave unsaid.
    pub fn is_a(&self) -> bool {
        *self == TraceId::A
    }
}

/// Rows and panels of a one-trace workspace name no trace: they are A's.
impl Default for TraceId {
    fn default() -> Self {
        TraceId::A
    }
}

impl std::fmt::Display for TraceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.letter())
    }
}

/// Workspaces store a trace as its letter.
impl serde::Serialize for TraceId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for TraceId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        let mut chars = text.chars();
        match (chars.next().and_then(TraceId::from_letter), chars.next()) {
            (Some(id), None) => Ok(id),
            _ => Err(serde::de::Error::custom(format!(
                "trace letter expected, found {text:?}"
            ))),
        }
    }
}

/// Something that belongs to one trace: a signal, a variable, a scope, a
/// member, a track or a clock path. Stored as `[letter, item]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Traced<T> {
    pub trace: TraceId,
    pub item: T,
}

impl<T> Traced<T> {
    pub fn new(trace: TraceId, item: T) -> Self {
        Self { trace, item }
    }

    /// The same trace with another item.
    pub fn with<U>(&self, item: U) -> Traced<U> {
        Traced::new(self.trace, item)
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Traced<U> {
        Traced::new(self.trace, f(self.item))
    }
}

impl<T: serde::Serialize> serde::Serialize for Traced<T> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        (self.trace, &self.item).serialize(s)
    }
}

impl<'de, T: serde::Deserialize<'de>> serde::Deserialize<'de> for Traced<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let (trace, item) = <(TraceId, T)>::deserialize(d)?;
        Ok(Self { trace, item })
    }
}

/// The unit of the session timeline: `10^timescale` seconds, or a
/// producer-named unit (`cycle`) that is never rescaled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    pub timescale: i8,
    pub name: Option<String>,
}

impl Unit {
    pub fn of(info: &TraceInfo) -> Self {
        Self {
            timescale: info.timescale,
            name: info.time_unit.clone(),
        }
    }

    /// How many of `self` make one of `coarser`, when both are SI units and
    /// `coarser` is not finer. `None` when a producer-named unit is involved
    /// or the factor does not fit in a `u64`.
    fn factor_to(&self, coarser: &Unit) -> Option<u64> {
        if self.name.is_some() || coarser.name.is_some() {
            return (self == coarser).then_some(1);
        }
        let steps = u32::try_from(i32::from(coarser.timescale) - i32::from(self.timescale)).ok()?;
        10u64.checked_pow(steps)
    }

    /// `ns`, `ps`, or the producer's name.
    pub fn label(&self) -> String {
        match &self.name {
            Some(name) => name.clone(),
            None => crate::wave::timeline::unit_label(self.timescale),
        }
    }
}

/// Whether a trace is open yet.
#[derive(Clone)]
pub enum SlotState {
    Loading,
    Loaded(Arc<dyn Session>),
    Error(String),
}

/// One open trace: where it came from, its session once opened, and where
/// its times sit on the session timeline.
#[derive(Clone)]
pub struct TraceSlot {
    pub id: TraceId,
    /// Where it was opened from, as a path or URI; names are derived from it.
    pub source: String,
    /// Its durable identity for workspaces and the recent list, when the
    /// host gave one.
    pub uri: Option<String>,
    /// A name the user gave it; otherwise [`TraceSet::name`] derives one.
    pub rename: Option<String>,
    pub(crate) state: SlotState,
    /// Loads requested under another generation are stale. Every slot gets
    /// a fresh one when it is added and when its placement changes.
    pub(crate) generation: u64,
    pub(crate) placement: Placement,
}

impl TraceSlot {
    pub(crate) fn new(id: TraceId, source: String, uri: Option<String>, generation: u64) -> Self {
        Self {
            id,
            source,
            uri,
            rename: None,
            state: SlotState::Loading,
            generation,
            placement: Placement::IDENTITY,
        }
    }

    pub fn state(&self) -> &SlotState {
        &self.state
    }

    pub fn session(&self) -> Option<&Arc<dyn Session>> {
        match &self.state {
            SlotState::Loaded(s) => Some(s),
            _ => None,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn placement(&self) -> Placement {
        self.placement
    }

    /// The file name, as a loading chip shows it.
    pub fn file_name(&self) -> &str {
        names::file_name(&self.source)
    }

    /// The format its session was read from: `VTR`, `FST`, or `remote`
    /// for a recording a host serves.
    pub fn format(&self) -> Option<&'static str> {
        let session = self.session()?;
        Some(
            session
                .format()
                .unwrap_or(if session.remote_id().is_some() {
                    "remote"
                } else {
                    "trace"
                }),
        )
    }

    /// Its own time unit.
    pub fn unit(&self) -> Option<Unit> {
        self.session().map(|s| Unit::of(s.info()))
    }

    /// Its time range on the session timeline.
    pub fn range(&self) -> Option<(u64, u64)> {
        let (start, end) = self.session()?.info().time_range;
        Some((
            self.placement.to_session(start),
            self.placement.to_session(end),
        ))
    }
}

/// The traces of one document, in letter order, and the unit of the
/// timeline they share.
#[derive(Clone, Default)]
pub struct TraceSet {
    slots: Vec<TraceSlot>,
    /// Set by the first trace that opens and refined by finer ones; never
    /// coarsened while the document lives, so times already shown keep
    /// their precision.
    unit: Option<Unit>,
    /// Counts every change of a slot (see [`TraceSet::revision`]).
    revision: u64,
}

impl TraceSet {
    pub fn iter(&self) -> impl Iterator<Item = &TraceSlot> {
        self.slots.iter()
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Whether more than one trace is open, so rows and panels show letters.
    pub fn is_combined(&self) -> bool {
        self.slots.len() > 1
    }

    pub fn ids(&self) -> impl Iterator<Item = TraceId> + '_ {
        self.slots.iter().map(|s| s.id)
    }

    pub fn get(&self, id: TraceId) -> Option<&TraceSlot> {
        self.slots.iter().find(|s| s.id == id)
    }

    pub(crate) fn get_mut(&mut self, id: TraceId) -> Option<&mut TraceSlot> {
        self.revision += 1;
        self.slots.iter_mut().find(|s| s.id == id)
    }

    /// Changes whenever a trace joins, leaves, opens, is renamed or placed
    /// anew: what frontends derive from the set holds until then.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn session(&self, id: TraceId) -> Option<&Arc<dyn Session>> {
        self.get(id)?.session()
    }

    /// The opened traces and their sessions.
    pub fn loaded(&self) -> impl Iterator<Item = (TraceId, &Arc<dyn Session>)> {
        self.slots
            .iter()
            .filter_map(|s| s.session().map(|session| (s.id, session)))
    }

    /// The first opened trace.
    pub fn first_loaded(&self) -> Option<(TraceId, &Arc<dyn Session>)> {
        self.loaded().next()
    }

    pub fn unit(&self) -> Option<&Unit> {
        self.unit.as_ref()
    }

    /// The span every opened trace covers on the session timeline.
    pub fn limits(&self) -> Option<(u64, u64)> {
        self.slots
            .iter()
            .filter_map(TraceSlot::range)
            .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
    }

    /// The letter the next trace takes: the lowest one not in use.
    pub fn free_id(&self) -> Option<TraceId> {
        (0..TraceId::MAX)
            .filter_map(TraceId::new)
            .find(|id| self.get(*id).is_none())
    }

    /// Put a slot in its letter's place.
    pub(crate) fn insert(&mut self, slot: TraceSlot) {
        debug_assert!(self.get(slot.id).is_none());
        self.revision += 1;
        let at = self.slots.partition_point(|s| s.id < slot.id);
        self.slots.insert(at, slot);
    }

    pub(crate) fn remove(&mut self, id: TraceId) -> Option<TraceSlot> {
        let at = self.slots.iter().position(|s| s.id == id)?;
        self.revision += 1;
        Some(self.slots.remove(at))
    }

    /// Place an opened trace on the session timeline, refining the unit
    /// when it is finer. Refused when its unit cannot share a timeline with
    /// the others (a producer-named unit meets another unit) or its times
    /// would not fit.
    pub(crate) fn admit(&self, info: &TraceInfo) -> anyhow::Result<(Unit, Placement, u64)> {
        let own = Unit::of(info);
        let Some(unit) = &self.unit else {
            return Ok((own, Placement::IDENTITY, 1));
        };
        let refusal = || {
            anyhow::anyhow!(
                "its times are in {}, the others' in {}: traces share a timeline only in \
                 seconds-based units or in the same named unit",
                own.label(),
                unit.label()
            )
        };
        let (session, factor, scale) = match (own.factor_to(unit), unit.factor_to(&own)) {
            (Some(1), _) | (_, Some(1)) => (unit.clone(), 1, 1),
            (Some(factor), _) => (own.clone(), factor, 1),
            (_, Some(scale)) => (unit.clone(), 1, scale),
            _ => return Err(refusal()),
        };
        let placement = Placement::scaled(scale);
        let end = info.time_range.1;
        anyhow::ensure!(
            placement.fits(end)
                && self
                    .limits()
                    .is_none_or(|(_, e)| e.checked_mul(factor).is_some()),
            "its time range does not fit on a timeline in {}",
            session.label()
        );
        Ok((session, placement, factor))
    }

    /// Start over: the next trace to open sets the unit.
    pub(crate) fn forget_unit(&mut self) {
        self.unit = None;
    }

    /// Install the result of [`TraceSet::admit`].
    pub(crate) fn set_unit(&mut self, unit: Unit) {
        self.unit = Some(unit);
    }

    /// The trace's name: the one it was given, or the part of its source
    /// that tells it apart from the other traces.
    pub fn name(&self, id: TraceId) -> Option<String> {
        let slot = self.get(id)?;
        if let Some(name) = &slot.rename {
            return Some(name.clone());
        }
        let sources: Vec<&str> = self.slots.iter().map(|s| s.source.as_str()).collect();
        let at = self.slots.iter().position(|s| s.id == id)?;
        derive_names(&sources).into_iter().nth(at)
    }
}

/// A change of the unit times are counted in: to a unit `factor` times
/// finer (times multiply, exactly) or coarser (times divide, rounding to
/// the nearest).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rescale {
    Finer(u64),
    Coarser(u64),
}

impl Rescale {
    /// From times in `10^from` seconds to times in `10^to` seconds; `None`
    /// when the units are the same or too far apart for a `u64` factor.
    pub(crate) fn between(from: i8, to: i8) -> Option<Self> {
        let steps = i32::from(from) - i32::from(to);
        let factor = 10u64.checked_pow(steps.unsigned_abs())?;
        match steps.signum() {
            1 => Some(Self::Finer(factor)),
            -1 => Some(Self::Coarser(factor)),
            _ => None,
        }
    }

    pub(crate) fn time(self, t: u64) -> u64 {
        match self {
            Self::Finer(k) => t.saturating_mul(k),
            Self::Coarser(k) => t / k + u64::from(t % k >= k.div_ceil(2)),
        }
    }

    pub(crate) fn real(self, t: f64) -> f64 {
        match self {
            Self::Finer(k) => t * k as f64,
            Self::Coarser(k) => t / k as f64,
        }
    }
}

/// State that holds session times, rewritten when the unit they are counted
/// in changes (a finer trace joins, or a workspace saved in another unit is
/// restored) so every time keeps its meaning.
pub(crate) trait Retime {
    fn retime(&mut self, by: Rescale);
}

impl Retime for u64 {
    fn retime(&mut self, by: Rescale) {
        *self = by.time(*self);
    }
}

impl<T: Retime> Retime for Option<T> {
    fn retime(&mut self, by: Rescale) {
        if let Some(t) = self {
            t.retime(by);
        }
    }
}

impl<T: Retime> Retime for Vec<T> {
    fn retime(&mut self, by: Rescale) {
        self.iter_mut().for_each(|t| t.retime(by));
    }
}

impl Retime for crate::wave::viewport::Viewport {
    fn retime(&mut self, by: Rescale) {
        self.start = by.real(self.start);
        self.end = by.real(self.end);
    }
}

impl<T: crate::nav::Lerp + Retime> Retime for crate::nav::Tween<T> {
    fn retime(&mut self, by: Rescale) {
        self.map(|v| {
            let mut v = v.clone();
            v.retime(by);
            v
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_round_trip_and_serialize() {
        let b = TraceId::new(1).unwrap();
        assert_eq!(b.letter(), 'B');
        assert_eq!(TraceId::from_letter('B'), Some(b));
        assert_eq!(TraceId::from_letter('b'), None);
        assert_eq!(TraceId::new(26), None);
        assert_eq!(serde_json::to_string(&b).unwrap(), "\"B\"");
        let traced = Traced::new(b, "soc.clk".to_owned());
        let json = serde_json::to_string(&traced).unwrap();
        assert_eq!(json, r#"["B","soc.clk"]"#);
        assert_eq!(
            serde_json::from_str::<Traced<String>>(&json).unwrap(),
            traced
        );
        assert!(serde_json::from_str::<TraceId>("\"AB\"").is_err());
    }

    fn info(timescale: i8, end: u64, unit: Option<&str>) -> TraceInfo {
        TraceInfo {
            name: "t".into(),
            design_id: None,
            timescale,
            time_range: (0, end),
            signal_count: 0,
            change_count: None,
            time_unit: unit.map(str::to_owned),
        }
    }

    #[test]
    fn a_finer_trace_refines_the_unit_and_a_coarser_one_is_scaled() {
        let mut set = TraceSet::default();
        let (unit, placement, factor) = set.admit(&info(-9, 1000, None)).unwrap();
        assert_eq!((placement, factor), (Placement::IDENTITY, 1));
        set.set_unit(unit);
        // Picoseconds: the session unit refines a thousandfold.
        let (unit, placement, factor) = set.admit(&info(-12, 5, None)).unwrap();
        assert_eq!(
            (unit.timescale, placement, factor),
            (-12, Placement::IDENTITY, 1000)
        );
        set.set_unit(unit);
        // Microseconds sit a million picoseconds apart.
        let (_, placement, factor) = set.admit(&info(-6, 5, None)).unwrap();
        assert_eq!((placement.to_session(5), factor), (5_000_000, 1));
    }

    #[test]
    fn named_units_combine_only_with_themselves() {
        let mut set = TraceSet::default();
        set.set_unit(Unit::of(&info(0, 10, Some("cycle"))));
        assert!(set.admit(&info(0, 10, Some("cycle"))).is_ok());
        let error = set.admit(&info(-9, 10, None)).unwrap_err().to_string();
        assert!(error.contains("in ns, the others' in cycle"), "{error}");
    }

    #[test]
    fn rescaling_is_exact_finer_and_rounds_coarser() {
        assert_eq!(Rescale::between(-9, -9), None);
        let finer = Rescale::between(-9, -12).unwrap();
        assert_eq!((finer, finer.time(783)), (Rescale::Finer(1000), 783_000));
        let coarser = Rescale::between(-12, -9).unwrap();
        assert_eq!(coarser, Rescale::Coarser(1000));
        assert_eq!(
            [
                coarser.time(783_000),
                coarser.time(1_499),
                coarser.time(1_500)
            ],
            [783, 1, 2]
        );
        assert_eq!(coarser.real(2_500.0), 2.5);
    }

    #[test]
    fn a_range_that_would_overflow_is_refused() {
        let mut set = TraceSet::default();
        set.set_unit(Unit::of(&info(-15, 10, None)));
        assert!(set.admit(&info(0, u64::MAX / 10, None)).is_err());
    }
}
