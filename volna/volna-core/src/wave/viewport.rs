//! The visible time window and its mapping to pixels.

use web_time::Instant;

/// The animation belongs to the value being animated: linked panels advance
/// the document's one animation, independent panels advance their own.
#[derive(Clone)]
pub struct ViewportState {
    pub viewport: Viewport,
    animation: Option<Animation>,
}

#[derive(Clone)]
struct Animation {
    from: Viewport,
    to: Viewport,
    start: Instant,
    duration: std::time::Duration,
}

impl ViewportState {
    pub fn new(viewport: Viewport) -> Self {
        Self {
            viewport,
            animation: None,
        }
    }
    pub fn is_animating(&self) -> bool {
        self.animation.is_some()
    }
    pub fn target(&self) -> Viewport {
        self.animation.as_ref().map_or(self.viewport, |a| a.to)
    }
    pub fn set(&mut self, viewport: Viewport) {
        self.viewport = viewport;
        self.animation = None;
    }
    /// Move to `target` with the user's animation mode; `Off` jumps.
    pub fn animate_to(
        &mut self,
        mut target: Viewport,
        limits: (u64, u64),
        now: Instant,
        mode: crate::settings::Animation,
    ) {
        self.tick(now);
        target.clamp(limits);
        let Some(duration) = mode.duration() else {
            self.viewport = target;
            self.animation = None;
            return;
        };
        self.animation = if self.viewport.approx_eq(&target) {
            None
        } else {
            Some(Animation {
                from: self.viewport,
                to: target,
                start: now,
                duration,
            })
        };
    }
    pub fn tick(&mut self, now: Instant) -> bool {
        let Some(a) = &self.animation else {
            return false;
        };
        let t = (now.saturating_duration_since(a.start).as_secs_f64() / a.duration.as_secs_f64())
            .min(1.0);
        self.viewport = Viewport::lerp(&a.from, &a.to, 1.0 - (1.0 - t).powi(3));
        if t >= 1.0 {
            self.viewport = a.to;
            self.animation = None;
        }
        self.is_animating()
    }
}

/// Fraction of the trace length the view may scroll past either end.
const EDGE_SPACE: f64 = 0.2;
/// Narrowest window, in time units.
const MIN_WIDTH: f64 = 0.25;

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Viewport {
    pub start: f64,
    pub end: f64,
}

impl Viewport {
    pub fn fit(range: (u64, u64)) -> Self {
        let (a, b) = (range.0 as f64, range.1 as f64);
        let end = if b > a { b } else { a + 1.0 };
        Viewport { start: a, end }
    }

    pub fn width(&self) -> f64 {
        self.end - self.start
    }

    pub fn px_per_unit(&self, width_px: f64) -> f64 {
        width_px / self.width()
    }

    pub fn time_at(&self, x_px: f64, width_px: f64) -> f64 {
        self.start + x_px / width_px * self.width()
    }

    pub fn x_of(&self, t: f64, width_px: f64) -> f64 {
        (t - self.start) / self.width() * width_px
    }

    /// Zoom by `factor` (> 1 zooms in) keeping the time under `x_px` fixed.
    pub fn zoom_about(&mut self, x_px: f64, width_px: f64, factor: f64, limits: (u64, u64)) {
        let anchor = self.time_at(x_px, width_px);
        let new_width = (self.width() / factor).max(MIN_WIDTH);
        let frac = if width_px > 0.0 { x_px / width_px } else { 0.5 };
        self.start = anchor - new_width * frac;
        self.end = self.start + new_width;
        self.clamp(limits);
    }

    /// Pan by a pixel delta (positive moves the view to later times).
    pub fn pan_px(&mut self, dx_px: f64, width_px: f64, limits: (u64, u64)) {
        let dt = dx_px / width_px * self.width();
        self.start += dt;
        self.end += dt;
        self.clamp(limits);
    }

    pub fn center_on(&mut self, t: f64, limits: (u64, u64)) {
        let w = self.width();
        self.start = t - w / 2.0;
        self.end = t + w / 2.0;
        self.clamp(limits);
    }

    pub fn go_to_start(&mut self, limits: (u64, u64)) {
        let w = self.width();
        self.start = limits.0 as f64;
        self.end = self.start + w;
        self.clamp(limits);
    }

    pub fn go_to_end(&mut self, limits: (u64, u64)) {
        let w = self.width();
        self.end = limits.1 as f64;
        self.start = self.end - w;
        self.clamp(limits);
    }

    /// Keep the window inside the trace plus edge space and above the minimum width.
    pub fn clamp(&mut self, limits: (u64, u64)) {
        let (a, b) = (limits.0 as f64, limits.1 as f64);
        let span = (b - a).max(1.0);
        let lo = a - span * EDGE_SPACE;
        let hi = b + span * EDGE_SPACE;
        let mut w = self.width().clamp(MIN_WIDTH, hi - lo);
        if !w.is_finite() {
            w = span;
        }
        if self.start < lo {
            self.start = lo;
            self.end = lo + w;
        }
        if self.end > hi {
            self.end = hi;
            self.start = hi - w;
        }
        if self.start < lo {
            self.start = lo;
        }
        self.end = self.start + w;
    }

    fn lerp(a: &Viewport, b: &Viewport, t: f64) -> Viewport {
        Viewport {
            start: a.start + (b.start - a.start) * t,
            end: a.end + (b.end - a.end) * t,
        }
    }

    pub fn approx_eq(&self, other: &Viewport) -> bool {
        let eps = self.width() * 1e-6;
        (self.start - other.start).abs() < eps && (self.end - other.end).abs() < eps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_keeps_anchor() {
        let mut v = Viewport::fit((0, 1000));
        let before = v.time_at(250.0, 1000.0);
        v.zoom_about(250.0, 1000.0, 2.0, (0, 1000));
        let after = v.time_at(250.0, 1000.0);
        assert!((before - after).abs() < 1e-9);
        assert!((v.width() - 500.0).abs() < 1e-9);
    }

    #[test]
    fn clamp_limits() {
        let mut v = Viewport {
            start: -5000.0,
            end: -4000.0,
        };
        v.clamp((0, 1000));
        assert_eq!(v.start, -200.0);
        assert_eq!(v.end, 800.0);
        let mut v = Viewport {
            start: 0.0,
            end: 1e9,
        };
        v.clamp((0, 1000));
        assert_eq!(v.start, -200.0);
        assert_eq!(v.end, 1200.0);
        let mut v = Viewport {
            start: 10.0,
            end: 10.0000001,
        };
        v.clamp((0, 1000));
        assert!((v.width() - MIN_WIDTH).abs() < 1e-9);
    }

    #[test]
    fn edges() {
        let mut v = Viewport {
            start: 100.0,
            end: 200.0,
        };
        v.go_to_end((0, 1000));
        assert_eq!((v.start, v.end), (900.0, 1000.0));
        v.go_to_start((0, 1000));
        assert_eq!((v.start, v.end), (0.0, 100.0));
        v.pan_px(50.0, 100.0, (0, 1000));
        assert_eq!((v.start, v.end), (50.0, 150.0));
    }
}
