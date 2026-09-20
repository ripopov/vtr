//! Local time-to-row navigation over the resident interval index.
use super::{PipelineModel, Rows};
use crate::document::Document;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FollowActivity {
    Off,
    #[default]
    Following,
    Suspended,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityCommand {
    Toggle,
    RevealAbove,
    RevealBelow,
}

#[derive(Default, Debug)]
pub struct Activity {
    pub above: usize,
    pub below: usize,
    pub visible: usize,
    pub nearest_above: Option<usize>,
    pub nearest_below: Option<usize>,
    pub target: Option<usize>,
    pub earlier: bool,
    pub later: bool,
}

impl PipelineModel {
    pub fn activity(&self, doc: &Document) -> Activity {
        let mut result = Activity::default();
        let Rows::Ready(set) = self.rows(doc) else {
            return result;
        };
        let view = self.nav.viewport(doc);
        let anchor = self
            .nav
            .cursor(doc)
            .map(|t| t as f64)
            .filter(|&t| t >= view.start && t <= view.end)
            .unwrap_or(view.start + (view.end - view.start) * 0.5);
        let rows = self
            .rows
            .target()
            .zoomed(self.last_layout().zoom.max(f32::EPSILON));
        let visible = f64::from(self.last_layout().cells.height() / rows.row_px);
        let center = rows.top + visible * 0.5;
        let mut best = (f64::INFINITY, f64::INFINITY);
        let mut offset = 0;
        for generator in set.generators() {
            let records = generator.transactions();
            result.earlier |= records
                .first()
                .is_some_and(|tx| (tx.begin as f64) < view.start);
            result.later |= records
                .last()
                .is_some_and(|tx| (tx.begin as f64) > view.end);
            // The inclusive owner query preserves long overlaps and point events.
            generator
                .visit_window(
                    view.start.max(0.0).floor() as u64,
                    view.end.max(0.0).ceil() as u64,
                    |tx| {
                        if (tx.end as f64) < view.start || (tx.begin as f64) > view.end {
                            return true;
                        }
                        let row = offset
                            + generator
                                .transaction_ordinal(tx.id)
                                .expect("indexed transaction");
                        if row as f64 + 1.0 <= rows.top {
                            result.above += 1;
                            result.nearest_above = Some(row);
                        } else if row as f64 >= rows.top + visible {
                            result.below += 1;
                            result.nearest_below.get_or_insert(row);
                        } else {
                            result.visible += 1;
                        }
                        let distance = (tx.begin as f64 - anchor)
                            .max(anchor - tx.end as f64)
                            .max(0.0);
                        let score = (distance, (row as f64 + 0.5 - center).abs());
                        if score < best {
                            best = score;
                            result.target = Some(row);
                        }
                        true
                    },
                )
                .expect("ordered viewport");
            offset += records.len();
        }
        result
    }

    pub fn activity_command(&mut self, doc: &Document, command: ActivityCommand) {
        match command {
            ActivityCommand::Toggle => {
                self.follow = match self.follow {
                    FollowActivity::Following => FollowActivity::Off,
                    _ => FollowActivity::Following,
                };
                self.follow_activity(doc);
            }
            ActivityCommand::RevealAbove | ActivityCommand::RevealBelow => {
                let activity = self.activity(doc);
                let row = match command {
                    ActivityCommand::RevealAbove => activity.nearest_above,
                    _ => activity.nearest_below,
                };
                if let Some(row) = row {
                    self.suspend_follow();
                    self.reveal_activity_row(doc, row);
                }
            }
        }
    }

    pub(super) fn suspend_follow(&mut self) {
        if self.follow == FollowActivity::Following {
            self.follow = FollowActivity::Suspended;
        }
    }

    pub(super) fn follow_activity(&mut self, doc: &Document) {
        if self.follow != FollowActivity::Following || self.last_layout().cells.height() <= 0.0 {
            return;
        }
        if let Some(row) = self.activity(doc).target {
            let rows = self.rows.target().zoomed(self.last_layout().zoom);
            let visible = f64::from(self.last_layout().cells.height() / rows.row_px);
            let position = row as f64 + 0.5;
            if position < rows.top + visible * 0.2 || position > rows.top + visible * 0.8 {
                self.reveal_activity_row(doc, row);
            }
        }
    }

    fn reveal_activity_row(&mut self, doc: &Document, row: usize) {
        let zoom = self.last_layout().zoom;
        let mut rows = self.rows.target().zoomed(zoom);
        rows.top =
            row as f64 + 0.5 - f64::from(self.last_layout().cells.height() / rows.row_px) * 0.5;
        rows.clamp(self.last_layout().cells.height(), self.row_count(doc), zoom);
        self.rows.set(rows.unzoomed(zoom));
    }
}
