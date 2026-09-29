//! Marks the wave painters share: the X hatch, dotted and dashed levels,
//! and aliased columns. Every rule works on one pixel column, so none adds
//! work per transition (`docs/volna-theme.html`, "Waveform panel rendering").

use crate::color::Color;
use crate::data::{SignalHistory, ValueKind};
use crate::geometry::{Point, Rect, point};
use crate::scene::Scene;
use crate::theme::Theme;

/// Spacing of the 45° X hatch, in design pixels.
pub const HATCH_PX: f32 = 5.0;
/// Opacity of the X hatch strokes.
pub const HATCH_ALPHA: f32 = 0.3;
/// Don't-care is dotted: on, off.
pub const DOTTED: (f32, f32) = (1.0, 2.0);
/// Weak drive is dashed: on, off.
pub const DASHED: (f32, f32) = (4.0, 3.0);
/// Most values an aliased column reads to learn which kinds it holds.
pub const KIND_SAMPLES: usize = 8;

/// Which value kinds occur in an aliased column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Kinds(u8);

impl Kinds {
    pub const NONE: Kinds = Kinds(0);

    pub fn of(kind: ValueKind) -> Self {
        Kinds(match kind {
            ValueKind::Normal => 1,
            ValueKind::Undef => 2,
            ValueKind::HighImp => 4,
            ValueKind::DontCare => 8,
            ValueKind::Weak => 16,
        })
    }

    pub fn with(self, kind: ValueKind) -> Self {
        Kinds(self.0 | Self::of(kind).0)
    }

    pub fn union(self, other: Kinds) -> Self {
        Kinds(self.0 | other.0)
    }

    pub fn has(self, kind: ValueKind) -> bool {
        self.0 & Self::of(kind).0 != 0
    }

    /// Only unknown values: the column is the X band.
    pub fn only_undef(self) -> bool {
        self == Self::of(ValueKind::Undef)
    }
}

/// Reads the kinds of the values aliased columns hold, one row at a time.
/// The value leaving a column enters the next one, so it is read once.
pub struct KindReader<'a> {
    h: &'a dyn SignalHistory,
    bit: bool,
    normal: bool,
    last: Option<(Option<usize>, ValueKind)>,
}

impl<'a> KindReader<'a> {
    /// `bit` reads 1-bit values through [`SignalHistory::bit`].
    pub fn new(h: &'a dyn SignalHistory, bit: bool) -> Self {
        Self {
            h,
            bit,
            normal: h.always_normal(),
            last: None,
        }
    }

    fn kind(&mut self, i: Option<usize>) -> ValueKind {
        if let Some((at, kind)) = self.last
            && at == i
        {
            return kind;
        }
        let kind = if self.bit {
            self.h.bit(i).kind()
        } else {
            self.h.value_view(i).kind()
        };
        self.last = Some((i, kind));
        kind
    }

    /// The kinds of the values a column holds: the value entering it
    /// (`before`) and changes `before + 1 ..= last`. Up to [`KIND_SAMPLES`]
    /// of them are read, evenly spaced and always including both ends, so
    /// the cost of a column is bounded whatever it holds.
    pub fn column(&mut self, before: Option<usize>, last: usize) -> Kinds {
        if self.normal {
            return Kinds::of(ValueKind::Normal);
        }
        let first = before.map_or(0, |b| b + 1);
        let mut kinds = if before.is_some() {
            Kinds::of(self.kind(before))
        } else {
            Kinds::NONE
        };
        if first > last {
            return kinds;
        }
        let n = last - first + 1;
        if n <= KIND_SAMPLES {
            for i in first..=last {
                kinds = kinds.with(self.kind(Some(i)));
            }
        } else {
            for k in 0..KIND_SAMPLES {
                let i = first + k * (n - 1) / (KIND_SAMPLES - 1);
                kinds = kinds.with(self.kind(Some(i)));
            }
        }
        kinds
    }
}

/// Opacity of an aliased column holding `changes` changes: 0.30 plus up to
/// 0.45 on a log scale, full at 64 changes.
pub fn activity_alpha(changes: usize) -> f32 {
    let n = changes.max(2) as f32;
    0.30 + 0.45 * (n.log2() / 6.0).min(1.0)
}

/// One aliased column: its change count and the kinds it holds.
#[derive(Clone, Copy, Debug)]
pub struct Aliased {
    pub x: usize,
    pub changes: usize,
    pub kinds: Kinds,
}

/// Paint aliased columns between `top` and `bottom`, `x0` being column 0.
/// A column of only unknowns is the hatched X band. Any other column is an
/// activity fill whose strength follows its change count, with 1px rails
/// in the X colour (or the Z colour) when unknown or floating values are
/// mixed in, so neither the unknowns nor the traffic around them hides.
pub fn paint_aliased(
    scene: &mut Scene,
    t: &Theme,
    columns: &[Aliased],
    x0: f32,
    top: f32,
    bottom: f32,
) {
    let h = bottom - top;
    let mut i = 0;
    while i < columns.len() {
        let c = columns[i];
        // A run of neighbouring columns that look the same is one quad.
        let style = look(t, c);
        let mut j = i + 1;
        while j < columns.len()
            && columns[j].x == columns[j - 1].x + 1
            && look(t, columns[j]) == style
        {
            j += 1;
        }
        let (xa, w) = (x0 + c.x as f32, (columns[j - 1].x + 1 - c.x) as f32);
        let (fill, rail, x_band) = style;
        scene.fill(Rect::from_xywh(xa, top, w, h), fill);
        if x_band {
            hatch(scene, t, Rect::from_xywh(xa, top, w, h), x0);
        }
        if let Some(rail) = rail {
            scene.fill(Rect::from_xywh(xa, top, w, 1.0), rail);
            scene.fill(Rect::from_xywh(xa, bottom - 1.0, w, 1.0), rail);
        }
        i = j;
    }
}

/// Fill, rail colour and whether it is the X band. Alphas are quantized to
/// 1/64 so neighbouring columns of similar activity merge.
fn look(t: &Theme, c: Aliased) -> (Color, Option<Color>, bool) {
    if c.kinds.only_undef() {
        return (t.wave_undef_fill, None, true);
    }
    let alpha = (activity_alpha(c.changes) * 64.0).round() / 64.0;
    let rail = if c.kinds.has(ValueKind::Undef) {
        Some(t.wave_undef)
    } else if c.kinds.has(ValueKind::HighImp) {
        Some(t.wave_highimp)
    } else {
        None
    };
    (t.wave_signal.with_alpha(alpha), rail, false)
}

/// The X hatch over `rect`: 45° strokes every [`HATCH_PX`], aligned to `x0`
/// so neighbouring hatched stretches continue one pattern.
pub fn hatch(scene: &mut Scene, t: &Theme, rect: Rect, x0: f32) {
    hatch_with(
        scene,
        rect,
        x0,
        HATCH_PX * t.zoom,
        t.wave_undef.with_alpha(HATCH_ALPHA),
    );
}

/// 45° strokes of `color` every `step` pixels over `rect`, aligned to `x0`.
pub fn hatch_with(scene: &mut Scene, rect: Rect, x0: f32, step: f32, color: Color) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 || step <= 0.0 {
        return;
    }
    let h = rect.height();
    let first = x0 + ((rect.left() - h - x0) / step).floor() * step;
    let mut segments = Vec::new();
    let mut u = first;
    while u < rect.right() {
        segments.push([point(u, rect.bottom()), point(u + h, rect.top())]);
        u += step;
    }
    scene.clipped(rect, |scene| scene.lines(segments, color, 1.0));
}

/// Horizontal dashes from `xa` to `xb` on the pixel row `y`, the pattern
/// aligned to `x0`: `(on, off)` pixels.
pub fn dashes(out: &mut Vec<[Point; 2]>, xa: f32, xb: f32, y: f32, x0: f32, (on, off): (f32, f32)) {
    let period = on + off;
    if xb <= xa || period <= 0.0 {
        return;
    }
    let y = y + 0.5;
    let mut x = x0 + ((xa - x0) / period).floor() * period;
    while x < xb {
        let (a, b) = (x.max(xa), (x + on).min(xb));
        if b > a {
            out.push([point(a, y), point(b, y)]);
        }
        x += period;
    }
}

/// Vertical dashes from `ya` to `yb` at pixel column `x`.
pub fn dashes_v(out: &mut Vec<[Point; 2]>, x: f32, ya: f32, yb: f32, (on, off): (f32, f32)) {
    let x = x + 0.5;
    let mut y = ya;
    while y < yb {
        out.push([point(x, y), point(x, (y + on).min(yb))]);
        y += on + off;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::history::VecHistory;
    use crate::data::{SignalShape, WaveValue};

    #[test]
    fn activity_grows_on_a_log_scale_and_saturates() {
        assert!((activity_alpha(2) - 0.375).abs() < 1e-6);
        assert!(activity_alpha(8) > activity_alpha(4));
        assert!((activity_alpha(64) - 0.75).abs() < 1e-6);
        assert_eq!(activity_alpha(10_000), activity_alpha(64));
    }

    #[test]
    fn a_column_reports_the_kinds_it_holds_within_its_budget() {
        let bits = |s: &str| WaveValue::Bits(s.into());
        let values: Vec<_> = (0..100)
            .map(|i| {
                bits(if i == 50 {
                    "x"
                } else if i % 2 == 0 {
                    "0"
                } else {
                    "1"
                })
            })
            .collect();
        let h = VecHistory {
            shape: SignalShape::Bit,
            times: (0..100).collect(),
            values,
            initial: bits("x"),
        };
        let mut read = KindReader::new(&h, true);
        let k = read.column(Some(3), 8);
        assert!(k.has(ValueKind::Normal) && !k.has(ValueKind::Undef));
        assert!(read.column(Some(46), 53).has(ValueKind::Undef));
        assert!(read.column(None, 0) == Kinds::of(ValueKind::Normal));
        // Beyond the budget both ends are still read.
        assert!(read.column(Some(10), 50).has(ValueKind::Undef));
        assert!(Kinds::of(ValueKind::Undef).only_undef());
        assert!(
            !Kinds::of(ValueKind::Undef)
                .with(ValueKind::Normal)
                .only_undef()
        );
    }
}
