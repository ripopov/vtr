//! Where a trace's times sit on the session timeline.

use std::sync::Arc;

use crate::data::loaded_tracks::LoadedTrack;
use crate::data::value_view::ValueView;
use crate::data::{Bit, SignalHistory, SignalShape, WaveValue};

/// The map from a trace's own times to session times: `t` becomes
/// `t * scale`, where the session unit is `scale` times finer than the
/// trace's. The document applies it to every history and track as it
/// arrives, so panels, readouts and snapping only ever see session times.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    scale: u64,
}

impl Placement {
    pub const IDENTITY: Placement = Placement { scale: 1 };

    pub(crate) fn scaled(scale: u64) -> Self {
        assert!(scale > 0, "a placement scale is positive");
        Self { scale }
    }

    pub fn scale(self) -> u64 {
        self.scale
    }

    pub fn is_identity(self) -> bool {
        self.scale == 1
    }

    /// This placement on a timeline `factor` times finer.
    pub(crate) fn refined(self, factor: u64) -> Self {
        Self::scaled(self.scale.saturating_mul(factor))
    }

    /// Whether trace time `t` has a session time.
    pub(crate) fn fits(self, t: u64) -> bool {
        t.checked_mul(self.scale).is_some()
    }

    pub fn to_session(self, t: u64) -> u64 {
        t.saturating_mul(self.scale)
    }

    /// The last trace time at or before session time `t`.
    pub fn to_trace(self, t: u64) -> u64 {
        t / self.scale
    }

    /// `history` in session times.
    pub fn place_history(self, history: Arc<dyn SignalHistory>) -> Arc<dyn SignalHistory> {
        if self.is_identity() {
            history
        } else {
            Arc::new(Placed {
                inner: history,
                placement: self,
            })
        }
    }

    /// Rewrite a freshly loaded track's times in place. Scaling keeps order
    /// and overlap, so the generators' indexes stay valid.
    pub(crate) fn place_track(self, track: &mut LoadedTrack) -> anyhow::Result<()> {
        if self.is_identity() {
            return Ok(());
        }
        for generator in &mut track.generators {
            Arc::get_mut(generator)
                .ok_or_else(|| anyhow::anyhow!("a new track shared its generator"))?
                .scale_times(self.scale);
        }
        Ok(())
    }
}

/// A history seen in session times.
struct Placed {
    inner: Arc<dyn SignalHistory>,
    placement: Placement,
}

impl SignalHistory for Placed {
    fn resident_bytes(&self) -> u64 {
        self.inner.resident_bytes()
    }
    fn shape(&self) -> SignalShape {
        self.inner.shape()
    }
    fn len(&self) -> usize {
        self.inner.len()
    }
    fn time(&self, i: usize) -> u64 {
        self.placement.to_session(self.inner.time(i))
    }
    fn value(&self, i: Option<usize>) -> WaveValue {
        self.inner.value(i)
    }
    fn value_view(&self, i: Option<usize>) -> ValueView<'_> {
        self.inner.value_view(i)
    }
    fn bit(&self, i: Option<usize>) -> Bit {
        self.inner.bit(i)
    }
    fn index_at(&self, t: u64) -> Option<usize> {
        self.inner.index_at(self.placement.to_trace(t))
    }
    fn index_at_hint(&self, t: u64, hint: usize) -> Option<usize> {
        self.inner.index_at_hint(self.placement.to_trace(t), hint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Changes(Vec<u64>);

    impl SignalHistory for Changes {
        fn shape(&self) -> SignalShape {
            SignalShape::Bit
        }
        fn len(&self) -> usize {
            self.0.len()
        }
        fn time(&self, i: usize) -> u64 {
            self.0[i]
        }
        fn value(&self, _: Option<usize>) -> WaveValue {
            WaveValue::Bits("1".into())
        }
        fn bit(&self, _: Option<usize>) -> Bit {
            Bit::One
        }
    }

    #[test]
    fn a_placed_history_is_searched_in_session_times() {
        let placed = Placement::scaled(1000).place_history(Arc::new(Changes(vec![2, 5, 9])));
        assert_eq!(placed.time(1), 5000);
        assert_eq!(placed.index_at(1999), None);
        assert_eq!(placed.index_at(2000), Some(0));
        assert_eq!(placed.index_at(8999), Some(1));
        assert_eq!(placed.index_at_hint(9000, 0), Some(2));
    }

    #[test]
    fn the_identity_keeps_the_history() {
        let history: Arc<dyn SignalHistory> = Arc::new(Changes(vec![1]));
        let placed = Placement::IDENTITY.place_history(Arc::clone(&history));
        assert!(Arc::ptr_eq(&history, &placed));
    }
}
