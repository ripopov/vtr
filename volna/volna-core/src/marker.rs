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

/// What a pointer is over on the Markers lane. Markers are indices into
/// [`Document::markers`], which is sorted by time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaneHit {
    /// The chip of one marker.
    Chip(usize),
    /// A cluster standing for markers too close together to draw apart.
    Cluster(Range<usize>),
}

/// A zoomed-in cluster keeps this fraction of its time span as margin on
/// each side, so its outer markers stay clear of the view's edges.
const CLUSTER_MARGIN: f64 = 0.15;

/// A left press on the lane: a chip moves the cursor to its marker, and a
/// cluster zooms the view to its markers. Returns whether anything changed.
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
            let (Some(first), Some(last)) = (
                doc.markers().get(markers.start),
                markers
                    .end
                    .checked_sub(1)
                    .and_then(|i| doc.markers().get(i)),
            ) else {
                return false;
            };
            let (start, end) = (first.time as f64, last.time as f64);
            let margin = ((end - start) * CLUSTER_MARGIN).max(1.0);
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
    fn at_finds_only_an_exact_time() {
        let markers = with_ids(&[1, 2, 3]);
        assert_eq!(at(&markers, 10).map(|m| m.id.get()), Some(2));
        assert!(at(&markers, 11).is_none());
        assert!(at(&markers, 99).is_none());
    }
}
