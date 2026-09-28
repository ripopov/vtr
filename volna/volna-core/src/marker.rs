//! Markers: kept instants with a number people type and an optional name,
//! owned by [`Document`] as one time-sorted list. This module holds the
//! marker type, numbering and lookups, and the Markers lane input shared by
//! every timed panel. The lane's layout and painting live in
//! [`crate::wave::overlay`]; the wave and pipeline models hit-test their
//! [`MarkerLane`] and hand the hit here, so both panels answer the same
//! gesture the same way.
//!
//! [`MarkerLane`]: crate::wave::overlay::MarkerLane

use std::fmt;
use std::num::NonZeroU32;
use std::ops::Range;

use crate::document::Document;
use crate::nav::NavState;
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
}

/// Zooming to markers keeps this fraction of their time span as margin on
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
    let (start, end) = (a.time as f64, b.time as f64);
    let margin = ((end - start) * ZOOM_MARGIN).max(1.0);
    nav.animate_to(
        doc,
        Viewport {
            start: start - margin,
            end: end + margin,
        },
        now,
    );
    true
}

/// A left press on the lane: a chip moves the cursor to its marker, and a
/// cluster zooms the view to its markers. A span takes no press here: a
/// click on it moves the cursor like one on the header, and a double-click
/// zooms to it ([`zoom_to`]). Returns whether anything changed.
/// No modifier removes a marker: Shift-click extends selections elsewhere,
/// and `⇧M` removes the marker at the cursor.
pub fn press(doc: &mut Document, nav: &mut NavState, hit: LaneHit, now: Instant) -> bool {
    match hit {
        LaneHit::Chip(ix) => match doc.markers().get(ix) {
            Some(m) => {
                let time = m.time;
                nav.set_cursor(doc, Some(time))
            }
            None => false,
        },
        LaneHit::Cluster(markers) => {
            !markers.is_empty() && zoom_to(doc, nav, markers.start, markers.end - 1, now)
        }
        LaneHit::Span(_) => false,
    }
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
    fn at_finds_only_an_exact_time() {
        let markers = with_ids(&[1, 2, 3]);
        assert_eq!(at(&markers, 10).map(|m| m.id.get()), Some(2));
        assert!(at(&markers, 11).is_none());
        assert!(at(&markers, 99).is_none());
    }
}
