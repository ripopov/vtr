//! Render raw query bins without fabricating an exact history. Each bin proves
//! coverage only of its own interval; pages outside the viewport are ignored.
//! Missing coverage is left to the row's loading/error background.
use crate::{
    geometry::{Rect, point, size},
    scene::Scene,
    theme::Theme,
};
use std::sync::Arc;
use vtr_query::{
    Interval,
    summary::WaveBin,
    wave::{Kind, Sample, Value},
};

/// Integer subtraction precedes float conversion, preserving narrow windows
/// near u64::MAX, including an exclusive end of AfterMax.
fn x(time: u128, viewport: Interval, row: Rect) -> f32 {
    let start = u128::from(viewport.start());
    let width = viewport.end().wide() - start;
    row.left() + ((time - start) as f64 / width as f64 * f64::from(row.width())) as f32
}

/// Paint complete raw bins retained by a row's demand/cache owner. The caller
/// supplies bins in any order; this function does not infer coverage between
/// them, extrapolate entry/exit samples, or perform queries during painting.
/// Multi-change bins remain visibly aggregated even when zoomed wider.
pub fn paint_summary(
    bins: &[Arc<WaveBin>],
    viewport: Interval,
    row: Rect,
    theme: &Theme,
    scene: &mut Scene,
) {
    if viewport.is_empty() || row.width() <= 0.0 || row.height() <= 0.0 {
        return;
    }
    scene.clipped(row, |scene| {
        for bin in bins {
            let left = u128::from(bin.interval.start()).max(u128::from(viewport.start()));
            let right = bin.interval.end().wide().min(viewport.end().wide());
            if left >= right {
                continue;
            }
            let a = x(left, viewport, row);
            let b = x(right, viewport, row);
            let top = row.top() + row.height().min(10.0) / 2.0;
            let bottom = row.bottom() - row.height().min(10.0) / 2.0;
            if matches!(bin.entry, Sample::Event) {
                // Events are occurrences. An empty bin has no held level.
                if bin.changes == 1 {
                    if let Some(change) = &bin.first {
                        let time = u128::from(change.time);
                        if time >= left && time < right {
                            let px = x(time, viewport, row);
                            scene.lines(
                                vec![[point(px, top), point(px, bottom)]],
                                theme.wave_signal,
                                1.0,
                            );
                        }
                    }
                } else if bin.changes > 1 {
                    scene.fill(
                        Rect::new(point(a, top), size(b - a, bottom - top)),
                        theme.wave_event_coalesced,
                    );
                }
                continue;
            }
            if bin.changes > 1 {
                // First/last are insufficient to reconstruct intermediate
                // values or edge positions; never draw an invented pulse.
                scene.fill(
                    Rect::new(point(a, top), size(b - a, bottom - top)),
                    theme.wave_dense,
                );
            } else if let Some(change) = &bin.first {
                let split = u128::from(change.time).clamp(left, right);
                held(
                    &bin.entry,
                    a,
                    x(split, viewport, row),
                    top,
                    bottom,
                    theme,
                    scene,
                );
                held(
                    &bin.exit,
                    x(split, viewport, row),
                    b,
                    top,
                    bottom,
                    theme,
                    scene,
                );
                if u128::from(change.time) >= left && u128::from(change.time) < right {
                    let px = x(split, viewport, row);
                    scene.lines(
                        vec![[point(px, top), point(px, bottom)]],
                        theme.wave_signal,
                        1.0,
                    );
                }
            } else {
                held(&bin.entry, a, b, top, bottom, theme, scene);
            }
        }
    });
}

/// Paint only the proven prefix of an exact query. The unfinished last
/// timestamp stays blank until a later timestamp or completion proves it.
/// Coincident/subpixel events retain a distinct coalesced-event stroke.
pub fn paint_exact(
    window: &super::exact::ExactWindow,
    viewport: Interval,
    row: Rect,
    theme: &Theme,
    scene: &mut Scene,
) {
    let Some(coverage) = window.coverage() else {
        return;
    };
    let left = u128::from(coverage.start()).max(u128::from(viewport.start()));
    let right = coverage.end().wide().min(viewport.end().wide());
    if left >= right || row.width() <= 0.0 || row.height() <= 0.0 {
        return;
    }
    let top = row.top() + row.height().min(10.0) / 2.0;
    let bottom = row.bottom() - row.height().min(10.0) / 2.0;
    let event = matches!(window.predecessor(), Some(Sample::Event));
    scene.clipped(row, |scene| {
        let mut sample = window.sample_at(left as u64);
        let mut from = left;
        let mut stroke: Option<(f32, usize)> = None;
        for change in window
            .changes()
            .filter(|change| u128::from(change.time) >= left && u128::from(change.time) < right)
        {
            let time = u128::from(change.time);
            let px = x(time, viewport, row);
            if let Some(current) = &sample {
                held(
                    current,
                    x(from, viewport, row),
                    px,
                    top,
                    bottom,
                    theme,
                    scene,
                );
                sample = Some(Sample::Known(change.value.clone()));
            }
            from = time;
            let column = px.floor();
            match stroke {
                Some((old, count)) if old == column => stroke = Some((old, count + 1)),
                old => {
                    if let Some((old, count)) = old {
                        let color = if count > 1 {
                            if event {
                                theme.wave_event_coalesced
                            } else {
                                theme.wave_dense
                            }
                        } else {
                            theme.wave_signal
                        };
                        scene.lines(vec![[point(old, top), point(old, bottom)]], color, 1.0);
                    }
                    stroke = Some((column, 1));
                }
            }
        }
        if let Some(sample) = &sample {
            held(
                sample,
                x(from, viewport, row),
                x(right, viewport, row),
                top,
                bottom,
                theme,
                scene,
            );
        }
        if let Some((column, count)) = stroke {
            let color = if count > 1 {
                if event {
                    theme.wave_event_coalesced
                } else {
                    theme.wave_dense
                }
            } else {
                theme.wave_signal
            };
            scene.lines(
                vec![[point(column, top), point(column, bottom)]],
                color,
                1.0,
            );
        }
    });
}

fn held(sample: &Sample, a: f32, b: f32, top: f32, bottom: f32, theme: &Theme, scene: &mut Scene) {
    if a >= b {
        return;
    }
    let color = match sample {
        Sample::BackendDefault(Kind::Bits {
            width: 1,
            states: 2,
        }) => {
            scene.lines(
                vec![[point(a, bottom), point(b, bottom)]],
                theme.wave_signal,
                1.0,
            );
            return;
        }
        Sample::BackendDefault(Kind::Real | Kind::Bytes) => theme.wave_signal,
        Sample::BackendDefault(_) => theme.wave_undef,
        Sample::Event => return,
        Sample::Known(Value::Bits {
            width: 1,
            states,
            data,
        }) => {
            let mask = match states {
                2 => 1,
                4 => 3,
                9 => 15,
                _ => 255,
            };
            match data.as_slice().first().map(|value| value & mask) {
                Some(0) => {
                    scene.lines(
                        vec![[point(a, bottom), point(b, bottom)]],
                        theme.wave_signal,
                        1.0,
                    );
                    return;
                }
                Some(1) => {
                    scene.fill(
                        Rect::new(point(a, top), size(b - a, bottom - top)),
                        theme.wave_high_fill,
                    );
                    scene.lines(vec![[point(a, top), point(b, top)]], theme.wave_signal, 1.0);
                    return;
                }
                Some(3) => theme.wave_highimp,
                Some(6..=8) => theme.wave_weak,
                _ => theme.wave_undef,
            }
        }
        Sample::Known(_) => theme.wave_signal,
    };
    // Multibit/text/real stable samples use an unlabelled bus outline. Exact
    // labels and analog scaling belong to the client's translator/view mode.
    scene.lines(
        vec![
            [point(a, top), point(b, top)],
            [point(a, bottom), point(b, bottom)],
        ],
        color,
        1.0,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Prim;
    use vtr_query::{Budget, TimeBound, summary::BinBuilder, wave::Bytes};
    fn value(bit: u8, budget: &Budget) -> Value {
        Value::Bits {
            width: 1,
            states: 2,
            data: Bytes::from_slice(&[bit], budget).unwrap(),
        }
    }
    fn row() -> Rect {
        Rect::new(point(0.0, 0.0), size(100.0, 24.0))
    }
    fn interval(a: u64, b: u64) -> Interval {
        Interval::new(a, TimeBound::Tick(b)).unwrap()
    }
    #[test]
    fn multiple_changes_remain_aggregate_after_zoom() {
        let budget = Budget::new(8192);
        let mut bin =
            BinBuilder::new(interval(0, 100), Sample::Known(value(0, &budget)), &budget).unwrap();
        bin.push(10, value(1, &budget)).unwrap();
        bin.push(20, value(0, &budget)).unwrap();
        let mut scene = Scene::default();
        let theme = Theme::one_dark();
        paint_summary(&[bin.finish()], interval(5, 25), row(), &theme, &mut scene);
        assert!(
            matches!(&scene.prims[1], Prim::Quad { rect, fill, .. } if rect.width() == 100.0 && *fill == theme.wave_dense)
        );
        assert_eq!(
            scene.prims.len(),
            3,
            "one aggregate band, never invented exact edges"
        );
    }
    #[test]
    fn missing_bins_and_offscreen_changes_are_not_extrapolated() {
        let budget = Budget::new(8192);
        let mut bin =
            BinBuilder::new(interval(20, 40), Sample::Known(value(0, &budget)), &budget).unwrap();
        bin.push(25, value(1, &budget)).unwrap();
        let bin = bin.finish();
        let mut scene = Scene::default();
        paint_summary(
            std::slice::from_ref(&bin),
            interval(0, 100),
            row(),
            &Theme::one_dark(),
            &mut scene,
        );
        for prim in &scene.prims {
            if let Prim::Lines { segments, .. } = prim {
                assert!(
                    segments
                        .iter()
                        .flatten()
                        .all(|p| p.x >= 20.0 && p.x <= 40.0)
                );
            }
        }
        scene.clear();
        paint_summary(
            &[bin],
            interval(30, 40),
            row(),
            &Theme::one_dark(),
            &mut scene,
        );
        let vertical = scene.prims.iter().filter(|p| matches!(p, Prim::Lines { segments, .. } if segments.iter().any(|s| s[0].x == s[1].x))).count();
        assert_eq!(vertical, 0, "the change at 25 is outside this viewport");
    }
    #[test]
    fn endpoint_projection_preserves_single_ticks_at_maximum_time() {
        let budget = Budget::new(8192);
        let viewport = Interval::new(u64::MAX - 1, TimeBound::AfterMax).unwrap();
        let mut bin = BinBuilder::new(viewport, Sample::Event, &budget).unwrap();
        bin.push(u64::MAX, value(1, &budget)).unwrap();
        let mut scene = Scene::default();
        paint_summary(
            &[bin.finish()],
            viewport,
            row(),
            &Theme::one_dark(),
            &mut scene,
        );
        assert!(
            matches!(&scene.prims[1], Prim::Lines { segments, .. } if segments[0][0].x == 50.0)
        );
    }
    #[test]
    fn event_counts_do_not_become_held_levels_or_deduplicated_pulses() {
        let budget = Budget::new(8192);
        let interval = interval(0, 10);
        let empty = BinBuilder::new(interval, Sample::Event, &budget)
            .unwrap()
            .finish();
        let mut scene = Scene::default();
        let theme = Theme::one_dark();
        paint_summary(&[empty], interval, row(), &theme, &mut scene);
        assert_eq!(
            scene.prims.len(),
            2,
            "empty event bin paints only clip markers"
        );
        let mut bin = BinBuilder::new(interval, Sample::Event, &budget).unwrap();
        bin.push(5, value(1, &budget)).unwrap();
        bin.push(5, value(1, &budget)).unwrap();
        scene.clear();
        paint_summary(&[bin.finish()], interval, row(), &theme, &mut scene);
        assert!(
            matches!(&scene.prims[1], Prim::Quad { fill, .. } if *fill == theme.wave_event_coalesced)
        );
    }
}
