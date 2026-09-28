//! Markers lane input shared by every timed panel: what a press on a chip
//! does. The lane's layout and painting live in [`crate::wave::overlay`];
//! the wave and pipeline models hit-test their [`MarkerLane`] and hand the
//! hit here, so both panels answer the same gesture the same way.
//!
//! [`MarkerLane`]: crate::wave::overlay::MarkerLane

use std::ops::Range;

use crate::document::Document;
use crate::geometry::Modifiers;
use crate::nav::NavState;
use crate::wave::viewport::Viewport;
use web_time::Instant;

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

/// A left press on the lane: a chip moves the cursor to its marker, Shift
/// removes the marker instead, and a cluster zooms the view to its markers.
/// Returns whether anything changed.
pub fn press(
    doc: &mut Document,
    nav: &mut NavState,
    hit: LaneHit,
    modifiers: Modifiers,
    now: Instant,
) -> bool {
    match hit {
        LaneHit::Chip(ix) if modifiers.shift => doc.remove_marker(ix),
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
