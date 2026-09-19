//! Stage colours. Version 1 assigns each stage name on the primary lane a
//! hue from a ladder in order of first appearance (Konata's "unique" scheme),
//! at a fixed lightness so the dark cell text reads in both appearances. A
//! VDB stage table later fills the same struct with authored colours; the
//! painter only ever asks for `style(name)`.

use std::collections::HashMap;
use std::sync::Arc;

use crate::color::Color;
use crate::data::loaded_tracks::LoadedGenerator;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StageStyle {
    pub fill: Color,
    pub edge: Color,
    pub text: Color,
}

/// Konata's default lane, used as the primary lane whenever it occurs.
pub const DEFAULT_LANE: &str = "0";

#[derive(Clone, Debug, Default)]
pub struct StagePalette {
    by_name: HashMap<String, StageStyle>,
    names: Vec<String>,
    primary_lane: String,
}

impl StagePalette {
    /// The ladder over every stage name on the primary lane of `generators`.
    /// The primary lane is `0` when any stage uses it, otherwise the lane
    /// with the most stages (ties go to the first seen).
    pub fn build(generators: &[Arc<LoadedGenerator>]) -> Self {
        let mut lane_counts: Vec<(&str, usize)> = Vec::new();
        for stage in generators
            .iter()
            .flat_map(|g| g.transactions())
            .flat_map(|tx| &tx.stages)
        {
            match lane_counts.iter_mut().find(|(lane, _)| *lane == stage.lane) {
                Some((_, n)) => *n += 1,
                None => lane_counts.push((&stage.lane, 1)),
            }
        }
        let primary_lane = if lane_counts.iter().any(|(lane, _)| *lane == DEFAULT_LANE) {
            DEFAULT_LANE.to_owned()
        } else {
            lane_counts
                .iter()
                .max_by_key(|(_, n)| *n)
                .map(|(lane, _)| (*lane).to_owned())
                .unwrap_or_else(|| DEFAULT_LANE.to_owned())
        };
        let mut names: Vec<String> = Vec::new();
        for stage in generators
            .iter()
            .flat_map(|g| g.transactions())
            .flat_map(|tx| &tx.stages)
            .filter(|stage| stage.lane == primary_lane)
        {
            if !names.contains(&stage.name) {
                names.push(stage.name.clone());
            }
        }
        let steps = names.len().saturating_sub(1).max(1) as f32;
        let by_name = names
            .iter()
            .enumerate()
            .map(|(k, name)| {
                let hue = (250.0 - k as f32 * (250.0 / steps)).rem_euclid(360.0) / 360.0;
                (name.clone(), ladder_style(hue))
            })
            .collect();
        Self {
            by_name,
            names,
            primary_lane,
        }
    }

    pub fn style(&self, name: &str) -> StageStyle {
        self.by_name
            .get(name)
            .copied()
            .unwrap_or_else(Self::fallback)
    }

    /// Stage names in ladder order.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Stages on this lane are cells; every other lane is an overlay band.
    pub fn primary_lane(&self) -> &str {
        if self.primary_lane.is_empty() {
            DEFAULT_LANE
        } else {
            &self.primary_lane
        }
    }

    /// Grey for stage names outside the ladder and for transactions without stages.
    pub fn fallback() -> StageStyle {
        StageStyle {
            fill: Color {
                h: 0.0,
                s: 0.0,
                l: 0.58,
                a: 1.0,
            },
            edge: Color {
                h: 0.0,
                s: 0.0,
                l: 0.38,
                a: 1.0,
            },
            text: Color::rgb(0x101820),
        }
    }
}

fn ladder_style(hue: f32) -> StageStyle {
    StageStyle {
        fill: Color {
            h: hue,
            s: 0.52,
            l: 0.58,
            a: 1.0,
        },
        edge: Color {
            h: hue,
            s: 0.52,
            l: 0.38,
            a: 1.0,
        },
        text: Color::rgb(0x101820),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::transactions::*;
    use std::collections::HashMap;

    fn generator(stages: &[&[(&str, &str)]]) -> Arc<LoadedGenerator> {
        let transactions = stages
            .iter()
            .enumerate()
            .map(|(i, stages)| Transaction {
                id: TransactionRef(i as u64),
                generator: TrackRef(1),
                begin: i as u64,
                end: i as u64 + 4,
                status: TxStatus::Unset,
                kind: TxKind::Unspecified,
                parent: None,
                attributes: vec![],
                events: vec![],
                stages: stages
                    .iter()
                    .enumerate()
                    .map(|(k, (name, lane))| TransactionStage {
                        name: (*name).into(),
                        lane: (*lane).into(),
                        begin: i as u64 + k as u64,
                        end: Some(i as u64 + k as u64 + 1),
                        attributes: vec![],
                    })
                    .collect(),
            })
            .collect();
        Arc::new(LoadedGenerator::new(TrackRef(1), transactions, HashMap::new(), vec![]).unwrap())
    }

    #[test]
    fn ladder_follows_first_appearance_on_the_primary_lane() {
        let g = generator(&[
            &[("F", "0"), ("stl", "1"), ("X", "0")],
            &[("F", "0"), ("D", "0"), ("X", "0")],
        ]);
        let p = StagePalette::build(&[g]);
        assert_eq!(p.names(), ["F", "X", "D"]);
        assert_eq!(p.primary_lane(), "0");
        assert_ne!(p.style("F"), p.style("X"));
        assert_eq!(p.style("stl"), StagePalette::fallback());
        assert!(p.style("F").fill.l > 0.5 && p.style("F").edge.l < p.style("F").fill.l);
        let named = generator(&[&[("F", "main"), ("M", "memory")], &[("W", "main")]]);
        let p = StagePalette::build(&[named]);
        assert_eq!(p.primary_lane(), "main");
        assert_eq!(p.names(), ["F", "W"]);
        assert_eq!(StagePalette::default().primary_lane(), "0");
    }
}
