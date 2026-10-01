//! The pipeline panel's workspace schema, capture, validation and resolution.
//! Preparing content never retains data or changes the live document.

use super::{PipelineModel, RowView, TrackSource};
use crate::clock::ClockView;
use crate::panels::workspace::valid_viewport;
use crate::panels::{Panel, PanelId, PanelKind, workspace::RestoreContext};
use crate::trace::{TraceId, Traced};
use crate::wave::{model::Link, viewport::Viewport};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

pub(crate) const VERSION: u32 = 1;

/// A pipeline panel: the track path, its navigation, row axis and label column.
#[derive(Serialize, Deserialize)]
struct PipelinePanel {
    #[serde(default)]
    follow: crate::pipeline::FollowActivity,
    id: PanelId,
    kind: String,
    version: u32,
    title: Option<String>,
    #[serde(default, skip_serializing_if = "TraceId::is_a")]
    trace: TraceId,
    track: Vec<String>,
    link: Link,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    viewport: Option<Viewport>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::panels::workspace::present_raw"
    )]
    cursor: Option<Box<RawValue>>,
    rows: RowView,
    /// The two-axis zoom's row cap, when a rows-only zoom raised it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    row_cap: Option<f32>,
    label_width: f32,
    #[serde(default, skip_serializing_if = "ClockView::is_default")]
    clocks: ClockView,
}

pub(crate) fn save(p: &PipelineModel, id: PanelId, title: Option<String>) -> Result<Box<RawValue>> {
    Ok(serde_json::value::to_raw_value(&PipelinePanel {
        follow: p.follow,
        id,
        kind: "pipeline".into(),
        version: VERSION,
        title,
        trace: p.track.trace(),
        track: p.track.path().to_vec(),
        link: p.nav.link,
        viewport: (!p.nav.link.viewport).then(|| p.nav.local_viewport.target()),
        cursor: (!p.nav.link.cursor)
            .then(|| serde_json::value::to_raw_value(&p.nav.local_cursor))
            .transpose()?,
        rows: p.rows.target(),
        row_cap: (p.row_cap != crate::pipeline::zoom::ROW_PX_CAP).then_some(p.row_cap),
        label_width: p.label_width,
        clocks: p.nav.clocks().clone(),
    })?)
}

pub(crate) fn restore(raw: &RawValue, ctx: &mut RestoreContext<'_>) -> Result<Panel> {
    let saved: PipelinePanel = serde_json::from_str(raw.get()).context("invalid pipeline panel")?;
    ensure!(!saved.track.is_empty(), "empty pipeline track path");
    ensure!(
        saved.rows.top.is_finite()
            && saved.rows.row_px.is_finite()
            && (crate::pipeline::rows::ROW_PX_MIN..=crate::pipeline::rows::ROW_PX_MAX)
                .contains(&saved.rows.row_px),
        "invalid pipeline rows"
    );
    ensure!(
        saved.row_cap.is_none_or(|cap| {
            (crate::pipeline::zoom::ROW_PX_CAP..=crate::pipeline::rows::ROW_PX_MAX).contains(&cap)
        }),
        "invalid pipeline row cap"
    );
    ensure!(
        saved.label_width.is_finite()
            && (crate::pipeline::layout::LABEL_W_MIN..=crate::pipeline::layout::LABEL_W_MAX)
                .contains(&saved.label_width),
        "invalid pipeline label width"
    );
    ensure!(
        saved.link.viewport == saved.viewport.is_none(),
        "local viewport must exist exactly when unlinked"
    );
    ensure!(
        saved.link.cursor == saved.cursor.is_none(),
        "local cursor must exist exactly when unlinked"
    );
    let trace = saved.trace;
    let track = match ctx
        .traces
        .tracks(trace)
        .iter()
        .find(|t| t.path == saved.track)
    {
        Some(t) => TrackSource::Resolved {
            track: Traced::new(trace, t.id),
            path: saved.track,
        },
        None => {
            ctx.report
                .push(format!("Missing pipeline track: {:?}", saved.track));
            TrackSource::Unresolved {
                trace,
                path: saved.track,
            }
        }
    };
    let mut p = PipelineModel::new(track, saved.link);
    p.follow = saved.follow;
    p.nav.restore_clocks(saved.clocks);
    if let Some(v) = saved.viewport {
        valid_viewport(v)?;
        p.nav.local_viewport.set(v);
    }
    if let Some(c) = saved.cursor {
        p.nav.local_cursor = serde_json::from_str(c.get()).context("invalid local cursor")?;
    }
    p.rows.set(saved.rows);
    p.row_cap = saved.row_cap.unwrap_or(crate::pipeline::zoom::ROW_PX_CAP);
    p.label_width = saved.label_width;
    Ok(Panel {
        id: saved.id,
        title: crate::history::Journaled::new(saved.title),
        kind: PanelKind::Pipeline(Box::new(p)),
    })
}
