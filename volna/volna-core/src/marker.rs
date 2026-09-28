//! Markers: kept instants with a number people type and an optional name,
//! owned by [`Document`] as one time-sorted list. This module holds the
//! marker type, numbering and lookups, and the Markers lane input shared by
//! every timed panel: presses, chip drags, the lane's context menus as data
//! and the readable text they copy. The lane's layout and painting live in
//! [`crate::wave::overlay`]; the wave and pipeline models hit-test their
//! [`MarkerLane`] and hand the hit here, so both panels answer the same
//! gesture the same way.
//!
//! [`MarkerLane`]: crate::wave::overlay::MarkerLane

use std::fmt;
use std::num::NonZeroU32;
use std::ops::Range;

use crate::clock::{Clock, ClockView};
use crate::document::Document;
use crate::geometry::Point;
use crate::nav::NavState;
use crate::wave::model::{MenuAction, MenuEntry, MenuItem};
use crate::wave::timeline::{TimeBase, format_time};
use crate::wave::viewport::Viewport;
use web_time::Instant;

/// A marker's number: what its chip shows and a digit key will reach. It
/// is the lowest free positive number when the marker is created and never
/// changes while the marker exists, so removing marker 2 frees 2 for the
/// next one. Workspaces store it as a plain positive integer.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct MarkerId(pub NonZeroU32);

impl MarkerId {
    pub fn new(n: u32) -> Option<Self> {
        NonZeroU32::new(n).map(Self)
    }

    pub fn get(self) -> u32 {
        self.0.get()
    }

    /// Zero-based index into the theme's marker palette.
    pub fn palette_index(self) -> usize {
        self.get() as usize - 1
    }
}

impl fmt::Display for MarkerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Marker {
    pub id: MarkerId,
    pub time: u64,
    pub label: Option<String>,
}

/// Where measurements start: a time, or a marker whose time it follows.
/// At most one exists per document. It is navigation state like the cursor:
/// saved in workspaces, never journaled. Workspaces store it as
/// `{"marker": 4}` or `{"time": 186000}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reference {
    Time(u64),
    Marker(MarkerId),
}

/// The reference for `time`: the marker exactly there, else the time.
pub fn reference_at(markers: &[Marker], time: u64) -> Reference {
    at(markers, time).map_or(Reference::Time(time), |m| Reference::Marker(m.id))
}

/// Where a measuring gesture at `raw` puts the reference: on the nearest
/// marker within `tolerance` time units, unless the cursor's snapping put
/// `snapped` on an edge closer to the pointer; else at `snapped`.
pub fn reference_near(markers: &[Marker], raw: f64, tolerance: f64, snapped: u64) -> Reference {
    let ix = markers.partition_point(|m| (m.time as f64) < raw);
    let distance = |t: u64| (t as f64 - raw).abs();
    let nearest = [ix.checked_sub(1), Some(ix)]
        .into_iter()
        .flatten()
        .filter_map(|i| markers.get(i))
        .min_by(|a, b| distance(a.time).total_cmp(&distance(b.time)));
    // Without an edge nearby, snapping leaves the rounded pointer time.
    let edge = if snapped == raw.round().max(0.0) as u64 {
        f64::INFINITY
    } else {
        distance(snapped)
    };
    match nearest {
        Some(m) if distance(m.time) <= tolerance && distance(m.time) <= edge => {
            Reference::Marker(m.id)
        }
        _ => reference_at(markers, snapped),
    }
}

/// The longest marker name, in characters; longer names are cut.
pub const MAX_NAME: usize = 48;

/// A typed marker name as stored: trimmed, at most [`MAX_NAME`] characters,
/// and `None` when nothing is left, which returns the chip to its number.
pub fn clean_name(name: &str) -> Option<String> {
    let name = name.trim();
    let name = match name.char_indices().nth(MAX_NAME) {
        Some((cut, _)) => name[..cut].trim_end(),
        None => name,
    };
    (!name.is_empty()).then(|| name.to_owned())
}

/// The lowest positive number no marker has, if one is left.
pub fn free_id(markers: &[Marker]) -> Option<MarkerId> {
    let mut used: Vec<u32> = markers.iter().map(|m| m.id.get()).collect();
    used.sort_unstable();
    let mut next = 1u32;
    for n in used {
        if n > next {
            break;
        }
        next = next.max(n.checked_add(1)?);
    }
    MarkerId::new(next)
}

/// The marker at exactly `time` in a time-sorted list.
pub fn at(markers: &[Marker], time: u64) -> Option<&Marker> {
    let ix = markers.partition_point(|m| m.time < time);
    markers.get(ix).filter(|m| m.time == time)
}

/// Where `.`, `,` and the digit keys send the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Walk {
    Next,
    Prev,
    To(MarkerId),
}

/// The marker a walk lands on in a time-sorted list. Next and previous
/// count from the cursor and skip markers at its time, so markers sharing
/// an instant are one stop. Without a cursor they start from the view:
/// the first marker at or after its start, the last at or before its end.
pub fn walk_target(
    markers: &[Marker],
    walk: Walk,
    cursor: Option<u64>,
    view: Viewport,
) -> Option<&Marker> {
    match (walk, cursor) {
        (Walk::To(id), _) => markers.iter().find(|m| m.id == id),
        (Walk::Next, Some(c)) => markers.get(markers.partition_point(|m| m.time <= c)),
        (Walk::Prev, Some(c)) => markers[..markers.partition_point(|m| m.time < c)].last(),
        (Walk::Next, None) => {
            markers.get(markers.partition_point(|m| (m.time as f64) < view.start))
        }
        (Walk::Prev, None) => {
            markers[..markers.partition_point(|m| m.time as f64 <= view.end)].last()
        }
    }
}

/// Walk the cursor to a marker as a jump (see [`NavState::jump_cursor`]).
/// Returns what the status bar says either way: where the cursor is, or why
/// it stayed.
pub fn walk(
    doc: &mut Document,
    nav: &mut NavState,
    walk: Walk,
    now: Instant,
) -> Result<String, String> {
    let view = nav.viewport_state(doc).target();
    let cursor = nav.cursor(doc);
    let Some(m) = walk_target(doc.markers(), walk, cursor, view) else {
        return Err(match (walk, cursor) {
            (Walk::Next, Some(_)) => "No marker after the cursor".into(),
            (Walk::Prev, Some(_)) => "No marker before the cursor".into(),
            (Walk::Next, None) => "No marker in or after the view".into(),
            (Walk::Prev, None) => "No marker in or before the view".into(),
            (Walk::To(id), _) => format!("No marker {id}"),
        });
    };
    let (id, time) = (m.id, m.time);
    Ok(if nav.jump_cursor(doc, time, now) {
        format!("At marker {id} · ` returns")
    } else {
        format!("At marker {id}")
    })
}

/// What a pointer is over on the Markers lane. Markers are indices into
/// [`Document::markers`], which is sorted by time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaneHit {
    /// The chip of one marker.
    Chip(usize),
    /// A cluster standing for markers too close together to draw apart.
    Cluster(Range<usize>),
    /// The span from a marker to the next one.
    Span(usize),
    /// The live span from the reference to the cursor, on the Measure lane.
    Live,
    /// The reference's `R` tag on the Measure lane.
    Tag,
    /// The Measure lane's `×` button, which clears the reference.
    ClearReference,
}

/// Zooming to markers or a measurement keeps this fraction of their time span as margin on
/// each side, so the outer ones stay clear of the view's edges.
const ZOOM_MARGIN: f64 = 0.15;

/// Zoom `nav`'s view to the markers `first..=last` (indices), animated.
/// Returns whether both exist.
pub fn zoom_to(
    doc: &mut Document,
    nav: &mut NavState,
    first: usize,
    last: usize,
    now: Instant,
) -> bool {
    let (Some(a), Some(b)) = (doc.markers().get(first), doc.markers().get(last)) else {
        return false;
    };
    let (start, end) = (a.time, b.time);
    zoom_between(doc, nav, start, end, now);
    true
}

/// Zoom `nav`'s view to the time between `a` and `b`, in either order,
/// with [`ZOOM_MARGIN`] of it on each side, animated.
pub fn zoom_between(doc: &mut Document, nav: &mut NavState, a: u64, b: u64, now: Instant) {
    let (start, end) = (a.min(b) as f64, a.max(b) as f64);
    let margin = ((end - start) * ZOOM_MARGIN).max(1.0);
    nav.animate_to(
        doc,
        Viewport {
            start: start - margin,
            end: end + margin,
        },
        now,
    );
}

/// A left press on the lane: a cluster zooms the view to its markers, and
/// the Measure lane's `×` clears the reference. A chip takes no press here:
/// the panel holds it as a [`MarkerDrag`], which goes to the marker on
/// release or moves it. A span takes none either: a click on it moves the
/// cursor like one on the header, and a double-click zooms to it
/// ([`zoom_to`]). The live span and the `R` tag keep the cursor, which is
/// one of the span's ends; a double-click on the span zooms to it. Returns
/// whether anything changed. No modifier removes a marker: Shift-click
/// extends selections elsewhere, and `⇧M` removes the marker at the cursor.
pub fn press(doc: &mut Document, nav: &mut NavState, hit: LaneHit, now: Instant) -> bool {
    match hit {
        LaneHit::Cluster(markers) => {
            !markers.is_empty() && zoom_to(doc, nav, markers.start, markers.end - 1, now)
        }
        LaneHit::ClearReference => doc.set_reference(None),
        LaneHit::Chip(_) | LaneHit::Span(_) | LaneHit::Live | LaneHit::Tag => false,
    }
}

/// A press must travel this far (at zoom 1.0) to drag a chip; shorter
/// presses are clicks.
pub const DRAG_SLOP_PX: f32 = 3.0;

/// A marker chip held by the pointer. Released where it was pressed, it
/// moves the cursor to the marker; dragged, it moves the marker, which the
/// panel snaps like the cursor. The document changes as the pointer moves,
/// and the open gesture keeps the edits one undo step (*Move marker 4*),
/// which `Esc` rolls back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MarkerDrag {
    pub id: MarkerId,
    /// Where the marker was at the press; painted dotted while it moves.
    pub from: u64,
    /// The pointer's x minus the marker's at the press, so the flag keeps
    /// its place under the pointer.
    pub grab_dx: f32,
    pub start: Point,
    pub moved: bool,
}

impl MarkerDrag {
    /// Hold marker `ix` (an index into the markers), drawn at `marker_x`.
    pub fn begin(doc: &Document, ix: usize, p: Point, marker_x: f32) -> Option<Self> {
        let m = doc.markers().get(ix)?;
        Some(Self {
            id: m.id,
            from: m.time,
            grab_dx: p.x - marker_x,
            start: p,
            moved: false,
        })
    }

    /// Where the marker's flag should go for pointer `p`, once the press has
    /// travelled [`DRAG_SLOP_PX`]; `None` while it is still a click.
    pub fn target_x(&mut self, p: Point, zoom: f32) -> Option<f32> {
        self.moved |= (p.x - self.start.x).hypot(p.y - self.start.y) > DRAG_SLOP_PX * zoom;
        self.moved.then_some(p.x - self.grab_dx)
    }

    /// The press ended. A click moves the cursor to the marker as a jump
    /// (`` ` `` returns); a drag has already moved it. Returns whether the
    /// cursor moved.
    pub fn release(self, doc: &mut Document, nav: &mut NavState, now: Instant) -> bool {
        if self.moved {
            return false;
        }
        let Some(time) = doc
            .markers()
            .iter()
            .find(|m| m.id == self.id)
            .map(|m| m.time)
        else {
            return false;
        };
        nav.jump_cursor(doc, time, now)
    }
}

/// What a lane menu offers. Menus list the same verbs as the keys and
/// gestures, with those shown beside them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneVerb {
    GoTo(MarkerId),
    Rename(MarkerId),
    MeasureFrom(MarkerId),
    Copy(MarkerId),
    MoveToCursor(MarkerId),
    Remove(MarkerId),
    /// Put the reference on the first marker and the cursor on the second.
    MeasureSpan(MarkerId, MarkerId),
    ZoomToSpan(MarkerId, MarkerId),
    CopySpan(MarkerId, MarkerId),
    ZoomToMeasurement,
    ClearReference,
}

fn item(verb: LaneVerb, label: impl Into<String>, badge: Option<&str>) -> MenuEntry {
    MenuEntry::Item(MenuItem {
        action: MenuAction::Lane(verb),
        label: label.into(),
        badge: badge.map(str::to_owned),
        checked: false,
    })
}

/// `4 “req B”`, or `4` without a name.
pub fn name(m: &Marker) -> String {
    match &m.label {
        Some(label) => format!("{} “{label}”", m.id),
        None => m.id.to_string(),
    }
}

/// The context menu of what `hit` is over, or `None` where the lane has
/// no menu (a cluster).
pub fn menu(doc: &Document, hit: &LaneHit) -> Option<Vec<MenuEntry>> {
    let markers = doc.markers();
    let measure = || {
        vec![
            item(
                LaneVerb::ZoomToMeasurement,
                "Zoom to Measurement",
                Some("Z"),
            ),
            item(LaneVerb::ClearReference, "Clear Reference", Some("⇧R")),
        ]
    };
    Some(match *hit {
        LaneHit::Chip(ix) => {
            let m = markers.get(ix)?;
            let id = m.id;
            let digit = (id.get() <= 9).then(|| id.to_string());
            vec![
                MenuEntry::Label(format!("Marker {}", name(m))),
                item(LaneVerb::GoTo(id), "Go to Marker", digit.as_deref()),
                item(LaneVerb::Rename(id), "Rename…", Some("double-click")),
                item(
                    LaneVerb::MeasureFrom(id),
                    "Measure from Here",
                    Some("Alt-click"),
                ),
                item(LaneVerb::Copy(id), "Copy as Text", None),
                MenuEntry::Separator,
                item(LaneVerb::MoveToCursor(id), "Move to Cursor", None),
                item(LaneVerb::Remove(id), "Remove Marker", None),
            ]
        }
        LaneHit::Span(ix) => {
            let (a, b) = (markers.get(ix)?.id, markers.get(ix + 1)?.id);
            vec![
                item(
                    LaneVerb::MeasureSpan(a, b),
                    format!("Measure {a} → {b}"),
                    None,
                ),
                item(
                    LaneVerb::ZoomToSpan(a, b),
                    "Zoom to Span",
                    Some("double-click"),
                ),
                item(LaneVerb::CopySpan(a, b), "Copy as Text", None),
            ]
        }
        LaneHit::Live | LaneHit::Tag | LaneHit::ClearReference => {
            doc.reference()?;
            measure()
        }
        LaneHit::Cluster(_) => return None,
    })
}

/// The query of the palette's marker mode: what follows a leading `@`, or
/// `None` for an ordinary palette query.
pub fn navigator_query(query: &str) -> Option<&str> {
    query.trim_start().strip_prefix('@')
}

/// One row of the marker navigator (the palette's marker mode).
#[derive(Clone, Debug, PartialEq)]
pub struct NavigatorRow {
    pub id: MarkerId,
    pub name: Option<String>,
    /// The marker's time.
    pub time: String,
    /// From the previous marker in time, `+400 ns`; `None` for the first.
    pub step: Option<String>,
    /// From the reference, in time and in cycles of the selected clock:
    /// `−126 ns · −80 core_clk`; `None` without a reference.
    pub from_reference: Option<String>,
    /// Whether the reference is this marker.
    pub is_reference: bool,
}

/// Whether marker `m` matches every word of `query`: a word of digits
/// matches its number, and any word a part of its name, ignoring case.
fn matches_query(m: &Marker, query: &str) -> bool {
    let name = m.label.as_deref().unwrap_or_default().to_lowercase();
    query.split_whitespace().all(|word| {
        let word = word.to_lowercase();
        word == m.id.to_string() || name.contains(&word)
    })
}

/// The navigator's rows for `query`, in time order. Steps are measured from
/// the previous marker of the whole list, so filtering does not change
/// them, and distances from the reference count cycles of `selected`.
pub fn navigator_rows(doc: &Document, query: &str, selected: Option<&Clock>) -> Vec<NavigatorRow> {
    let base = doc.time_base();
    let reference = doc.reference_time();
    let attached = match doc.reference() {
        Some(Reference::Marker(id)) => Some(id),
        _ => None,
    };
    let clocks: Vec<&Clock> = selected.into_iter().collect();
    let markers = doc.markers();
    markers
        .iter()
        .enumerate()
        .filter(|(_, m)| matches_query(m, query))
        .map(|(ix, m)| NavigatorRow {
            id: m.id,
            name: m.label.clone(),
            time: format_time(m.time as f64, base),
            step: ix.checked_sub(1).map(|p| {
                let dt = i128::from(m.time) - i128::from(markers[p].time);
                format!("+{}", crate::wave::overlay::signed_time(dt, base))
            }),
            from_reference: reference.map(|r| {
                let measured = crate::measure::measure(&clocks, r, m.time);
                std::iter::once(crate::wave::overlay::signed_time(measured.dt(), base))
                    .chain(
                        measured
                            .clocks
                            .iter()
                            .map(|c| format!("{} {}", c.cycles, c.name)),
                    )
                    .collect::<Vec<_>>()
                    .join(" · ")
            }),
            is_reference: attached == Some(m.id),
        })
        .collect()
}

/// A marker as a readable reference for a bug report, a chat or an agent:
/// `lsu_ddr.vtr marker 4 “req B” at 186 ns (core_clk 185, axi_clk 73)`,
/// with the marker's position in each ruler clock.
pub fn text(
    trace: &str,
    m: &Marker,
    base: TimeBase<'_>,
    view: &ClockView,
    rulers: &[&Clock],
) -> String {
    let mut text = format!(
        "{trace} marker {} at {}",
        name(m),
        format_time(m.time as f64, base)
    );
    if !rulers.is_empty() {
        let clocks: Vec<String> = rulers
            .iter()
            .map(|c| crate::clock::position_at(view, c, m.time))
            .collect();
        text.push_str(&format!(" ({})", clocks.join(", ")));
    }
    text
}

/// The span between two markers as text: `lsu_ddr.vtr marker 1 “req A” →
/// marker 2 “resp A”: 126 ns · 80 core_clk · 26.4 axi_clk`.
pub fn span_text(
    trace: &str,
    a: &Marker,
    b: &Marker,
    base: TimeBase<'_>,
    rulers: &[&Clock],
) -> String {
    let m = crate::measure::measure(rulers, a.time, b.time);
    let parts: Vec<String> = std::iter::once(crate::wave::overlay::signed_time(m.dt(), base))
        .chain(m.clocks.iter().map(|c| format!("{} {}", c.cycles, c.name)))
        .collect();
    format!(
        "{trace} marker {} → marker {}: {}",
        name(a),
        name(b),
        parts.join(" · ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_ids(ids: &[u32]) -> Vec<Marker> {
        ids.iter()
            .enumerate()
            .map(|(i, &n)| Marker {
                id: MarkerId::new(n).unwrap(),
                time: i as u64 * 10,
                label: None,
            })
            .collect()
    }

    #[test]
    fn the_lowest_free_number_comes_first() {
        let free = |ids: &[u32]| free_id(&with_ids(ids)).map(MarkerId::get);
        assert_eq!(free(&[]), Some(1));
        assert_eq!(free(&[1, 2, 3]), Some(4));
        assert_eq!(free(&[3, 1, 4]), Some(2));
        assert_eq!(free(&[2, 7]), Some(1));
        assert_eq!(free(&[u32::MAX]), Some(1));
        assert_eq!(free(&[1, u32::MAX]), Some(2));
    }

    #[test]
    fn walks_step_in_time_order_and_treat_an_instant_as_one_stop() {
        // Markers 2 and 3 share time 20, as a restored workspace may have.
        let markers: Vec<Marker> = [(1, 10), (2, 20), (3, 20), (4, 30)]
            .into_iter()
            .map(|(n, time)| Marker {
                id: MarkerId::new(n).unwrap(),
                time,
                label: None,
            })
            .collect();
        let view = Viewport {
            start: 15.0,
            end: 25.0,
        };
        let go = |walk, cursor| walk_target(&markers, walk, cursor, view).map(|m| m.id.get());
        assert_eq!(go(Walk::Next, Some(10)), Some(2));
        assert_eq!(
            go(Walk::Next, Some(20)),
            Some(4),
            "both markers at 20 are passed"
        );
        assert_eq!(go(Walk::Prev, Some(30)), Some(3));
        assert_eq!(go(Walk::Prev, Some(20)), Some(1));
        assert_eq!(go(Walk::Next, Some(15)), Some(2));
        assert_eq!(go(Walk::Prev, Some(25)), Some(3));
        assert_eq!(go(Walk::Next, Some(30)), None);
        assert_eq!(go(Walk::Prev, Some(10)), None);
        assert_eq!(go(Walk::Next, Some(0)), Some(1));
        assert_eq!(go(Walk::Prev, Some(u64::MAX)), Some(4));
        // Without a cursor: from the view's start, back from its end.
        assert_eq!(go(Walk::Next, None), Some(2));
        assert_eq!(go(Walk::Prev, None), Some(3));
        let id = |n| Walk::To(MarkerId::new(n).unwrap());
        assert_eq!(go(id(4), None), Some(4));
        assert_eq!(go(id(5), Some(10)), None);
        assert_eq!(walk_target(&[], Walk::Next, Some(0), view), None);
    }

    #[test]
    fn names_are_trimmed_cut_and_empty_means_none() {
        assert_eq!(clean_name("  req B \t"), Some("req B".into()));
        assert_eq!(clean_name("   "), None);
        assert_eq!(clean_name(""), None);
        let long = "é".repeat(60);
        assert_eq!(clean_name(&long).unwrap().chars().count(), MAX_NAME);
        let spaced = format!("{} tail", "a".repeat(MAX_NAME - 1));
        assert_eq!(clean_name(&spaced), Some("a".repeat(MAX_NAME - 1)));
    }

    #[test]
    fn a_measuring_gesture_attaches_to_a_marker_unless_an_edge_is_closer() {
        let markers = with_ids(&[1, 2]); // at 0 and 10
        let near = |raw, snapped| reference_near(&markers, raw, 3.0, snapped);
        let marker = |n| Reference::Marker(MarkerId::new(n).unwrap());
        assert_eq!(
            near(12.2, 12),
            marker(2),
            "no edge: the marker within reach"
        );
        assert_eq!(near(14.0, 14), Reference::Time(14), "out of reach");
        assert_eq!(near(11.6, 12), marker(2), "a rounded time is no edge");
        assert_eq!(
            near(12.4, 13),
            Reference::Time(13),
            "an edge nearer than the marker"
        );
        assert_eq!(near(11.0, 13), marker(2), "the marker nearer than the edge");
        assert_eq!(near(0.0, 0), marker(1), "exactly on a marker");
    }

    #[test]
    fn at_finds_only_an_exact_time() {
        let markers = with_ids(&[1, 2, 3]);
        assert_eq!(at(&markers, 10).map(|m| m.id.get()), Some(2));
        assert!(at(&markers, 11).is_none());
        assert!(at(&markers, 99).is_none());
    }
}
