//! Whole-window frame timing: reads GPUI's frame profiler into the core's
//! [`FrameStats`](volna_core::frames::FrameStats) and draws the status-bar
//! item and its details popup.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::{
    Anchor, Bounds, Context, CursorStyle, FocusHandle, FrameDurationSnapshot, Hsla,
    InputLatencySnapshot, MouseButton, Pixels, SharedString, Window, anchored, canvas, deferred,
    div, point, px,
};
use hdrhistogram::Histogram;
use volna_core::frames::{
    self, BUCKETS, Distribution, FrameDetails, FrameRow, FrameSample, FrameStatus, Level, Summary,
    format_ms,
};

use crate::app::Workspace;
use crate::theme::{Theme, ThemePx, theme};

/// Frontend state around the core's frame statistics.
#[derive(Default)]
pub(crate) struct FrameView {
    frames: Option<FrameDurationSnapshot>,
    input: Option<InputLatencySnapshot>,
    /// Where the status item was last painted: the popup hangs above it,
    /// and a press on it toggles rather than dismisses.
    trigger: Rc<Cell<Bounds<Pixels>>>,
    focus: Option<FocusHandle>,
}

impl FrameView {
    /// The frames GPUI recorded since the previous call.
    fn sample(&mut self, window: &Window) -> FrameSample {
        let frames = window.frame_duration_snapshot();
        let input = window.input_latency_snapshot();
        let old = self.frames.as_ref();
        let sample = FrameSample {
            draw: delta(
                &frames.draw_duration_histogram,
                old.map(|o| &o.draw_duration_histogram),
            ),
            latency: delta(
                &frames.dirty_to_present_histogram,
                old.map(|o| &o.dirty_to_present_histogram),
            ),
            input: delta(
                &input.latency_histogram,
                self.input.as_ref().map(|o| &o.latency_histogram),
            ),
        };
        self.frames = Some(frames);
        self.input = Some(input);
        sample
    }
}

fn delta(now: &Histogram<u64>, before: Option<&Histogram<u64>>) -> Distribution {
    let mut out = Distribution::default();
    for v in now.iter_recorded() {
        let value = v.value_iterated_to();
        let old = before.map_or(0, |b| b.count_at(value));
        out.record(value, v.count_at_value().saturating_sub(old));
    }
    out
}

impl Workspace {
    /// Sample GPUI's frame histograms every tick while the window lives.
    pub(crate) fn start_frame_sampling(&self, window: &Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs_f64(frames::TICK_SECONDS))
                    .await;
                if this
                    .update_in(cx, |this, window, cx| this.sample_frames(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn sample_frames(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sample = self.frame_view.sample(window);
        if self.app.frames.push(sample) {
            cx.notify();
        }
    }

    fn set_frame_details(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.app.frames.details = open;
        if open {
            let focus = self
                .frame_view
                .focus
                .get_or_insert_with(|| cx.focus_handle());
            window.focus(focus, cx);
        } else {
            window.focus(&self.waves_focus, cx);
        }
        cx.notify();
    }

    /// The status-bar item: recent medians, tinted when frames ran over budget.
    pub(crate) fn render_frame_status(
        &self,
        status: FrameStatus,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = *theme(cx);
        let color = match status.level {
            Level::Ok => t.bar.text_placeholder,
            Level::Warn => t.bar.warning,
            Level::Bad => t.bar.error,
        };
        let color = if status.idle {
            color.alpha(0.55)
        } else {
            color
        };
        let trigger = self.frame_view.trigger.clone();
        div()
            .id("status-frames")
            .debug_selector(|| "status-frames".into())
            .relative()
            .px_1p5()
            .h(t.px(18.0))
            .flex()
            .items_center()
            .rounded_sm()
            .cursor(CursorStyle::PointingHand)
            .font_family(t.mono_font)
            .text_size(px(t.ui_size_small))
            .text_color(color)
            .hover(move |s| s.bg(t.bar_hover.bg))
            .on_click(cx.listener(|this, _, window, cx| {
                let open = !this.app.frames.details;
                this.set_frame_details(open, window, cx);
            }))
            .child(SharedString::from(status.text))
            .child(
                canvas(move |bounds, _, _| trigger.set(bounds), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
    }

    /// The details popup above the status item, while open.
    pub(crate) fn render_frame_details(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.app.frames.details {
            return None;
        }
        let t = *theme(cx);
        let details = self.app.frames.details();
        let trigger = self.frame_view.trigger.get();
        let focus = self.frame_view.focus.clone()?;
        let ignore = self.frame_view.trigger.clone();
        let panel = div()
            .id("frame-details")
            .debug_selector(|| "frame-details".into())
            .track_focus(&focus)
            .w(t.px(460.0))
            .p(t.px(12.0))
            .flex()
            .flex_col()
            .gap(t.px(10.0))
            .rounded(t.px(4.0))
            .border_1()
            .border_color(t.border)
            .bg(t.elevated.bg)
            .text_color(t.elevated.text)
            .text_size(px(t.ui_size_small))
            .shadow_lg()
            .occlude()
            .on_mouse_down_out(cx.listener(
                move |this, ev: &gpui_kit::MouseDownEvent, window, cx| {
                    // A press on the status item toggles the popup itself.
                    if !ignore.get().contains(&ev.position) {
                        this.set_frame_details(false, window, cx);
                    }
                },
            ))
            .on_key_down(
                cx.listener(|this, ev: &gpui_kit::KeyDownEvent, window, cx| {
                    if ev.keystroke.key == "escape" {
                        this.set_frame_details(false, window, cx);
                    }
                }),
            )
            .child(header(&details, &t, cx))
            .children(details.rows.iter().map(|row| metric(row, &t)));
        Some(
            deferred(
                anchored()
                    .anchor(Anchor::BottomRight)
                    .position(point(trigger.right(), trigger.top() - t.px(4.0)))
                    .snap_to_window_with_margin(t.px(8.0))
                    .child(panel),
            )
            .with_priority(10),
        )
    }
}

fn header(details: &FrameDetails, t: &Theme, cx: &mut Context<Workspace>) -> impl IntoElement {
    let fps = details.fps.map_or("—".into(), |fps| format!("{fps:.0}"));
    div()
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex()
                .items_baseline()
                .gap_2()
                .child(div().text_size(px(t.ui_size)).child("Frame timing"))
                .child(
                    div()
                        .text_color(t.elevated.text_muted)
                        .child(format!("{fps} frames/s while active")),
                ),
        )
        .child(
            div()
                .id("frame-details-reset")
                .px_2()
                .py_0p5()
                .rounded_sm()
                .border_1()
                .border_color(t.border)
                .cursor(CursorStyle::PointingHand)
                .hover(|s| s.bg(t.menu_hover.bg).text_color(t.menu_hover.text))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.app.frames.reset();
                    cx.notify();
                }))
                .child("Reset"),
        )
}

const COLUMNS: [&str; 5] = ["median", "p90", "p99", "max", "frames"];
const LABEL_W: f32 = 76.0;
const COLUMN_W: f32 = 52.0;

fn metric(row: &FrameRow, t: &Theme) -> impl IntoElement {
    let cell = |text: String, color: Hsla| {
        div()
            .w(t.px(COLUMN_W))
            .flex()
            .justify_end()
            .text_color(color)
            .child(text)
    };
    let line = |label: &'static str, summary: Option<Summary>| {
        let mut el = div().flex().font_family(t.mono_font).child(
            div()
                .w(t.px(LABEL_W))
                .text_color(t.elevated.text_muted)
                .child(label),
        );
        match summary {
            Some(s) => {
                let over = |value: f64| match row.budget_ms {
                    Some(budget) if value > 2.0 * budget => t.elevated.error,
                    Some(budget) if value > budget => t.elevated.warning,
                    _ => t.elevated.text,
                };
                for value in [s.median, s.p90, s.p99, s.max] {
                    el = el.child(cell(format_ms(value), over(value)));
                }
                el.child(cell(s.count.to_string(), t.elevated.text_muted))
            }
            None => el.child(cell("—".into(), t.elevated.text_muted)),
        }
    };
    let mut heading = div()
        .flex()
        .font_family(t.mono_font)
        .text_color(t.elevated.text_placeholder)
        .child(div().w(t.px(LABEL_W)).child("ms"));
    for column in COLUMNS {
        heading = heading.child(cell(column.into(), t.elevated.text_placeholder));
    }
    div()
        .flex()
        .flex_col()
        .gap(t.px(2.0))
        .pt(t.px(8.0))
        .border_t_1()
        .border_color(t.border_variant)
        .child(div().child(row.title))
        .child(div().text_color(t.elevated.text_muted).child(row.hint))
        .child(heading)
        .child(line("recent", row.recent))
        .child(line("since reset", row.total))
        .child(chart(row, t))
}

const BAR_W: f32 = 22.0;
const CHART_H: f32 = 28.0;

/// Frames per duration bucket since the reset, on log scales both ways so
/// a single slow frame stays visible next to thousands of fast ones.
fn chart(row: &FrameRow, t: &Theme) -> impl IntoElement {
    let peak = row.buckets.iter().copied().max().unwrap_or(0);
    let height = |count: u64| {
        if count == 0 {
            0.0
        } else {
            (1.0 + (count as f32).ln()) / (1.0 + (peak as f32).ln())
        }
    };
    let budget = row.budget_ms.map(frames::bucket_position);
    let mut bars = div()
        .relative()
        .flex()
        .items_end()
        .h(t.px(CHART_H))
        .border_b_1()
        .border_color(t.border_variant);
    for (i, &count) in row.buckets.iter().enumerate() {
        let slow = budget.is_some_and(|b| i as f64 >= b.ceil());
        let color = if slow {
            t.elevated.warning
        } else {
            t.elevated.icon_accent
        };
        bars = bars.child(
            div()
                .w(t.px(BAR_W))
                .h_full()
                .flex()
                .items_end()
                .px(t.px(2.0))
                .child(
                    div()
                        .w_full()
                        .h(t.px(CHART_H * height(count)))
                        .bg(color.alpha(0.8)),
                ),
        );
    }
    if let Some(b) = budget {
        bars = bars.child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(t.px(BAR_W * b as f32))
                .w(px(1.0))
                .bg(t.elevated.warning),
        );
    }
    let mut labels = div()
        .flex()
        .font_family(t.mono_font)
        .text_color(t.elevated.text_placeholder);
    for i in 0..BUCKETS {
        let text = match i {
            0 => String::new(),
            _ => short_ms(frames::BUCKET_EDGES_MS[i - 1]),
        };
        labels = labels.child(div().w(t.px(BAR_W)).child(text));
    }
    div()
        .flex()
        .flex_col()
        .gap(t.px(2.0))
        .pt(t.px(4.0))
        .child(bars)
        .child(labels)
}

fn short_ms(value: f64) -> String {
    if value < 1.0 {
        format!("{}", value).trim_start_matches('0').to_owned()
    } else {
        format!("{value:.0}")
    }
}
