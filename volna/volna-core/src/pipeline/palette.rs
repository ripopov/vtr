//! Stage colours. Each stage name on the primary lane gets a hue from a
//! ladder in pipeline order: names are ranked by their mean position within
//! a transaction's primary-lane stages (ties by first appearance), so hues
//! run from fetch to retire whatever order the trace first shows them in.
//! Lightness is fixed so the dark cell text reads in both appearances; with
//! more than eight names, neighbouring hues alternate between two
//! lightnesses so a dozen stages stay apart. A VDB stage table later fills
//! the same struct with authored colours; the painter only ever asks for
//! `style(name)`.

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

#[derive(Clone, Debug)]
pub struct StagePalette {
    by_name: HashMap<String, StageStyle>,
    names: Vec<String>,
    primary_lane: String,
}

impl Default for StagePalette {
    fn default() -> Self {
        Self {
            by_name: HashMap::new(),
            names: Vec::new(),
            primary_lane: DEFAULT_LANE.to_owned(),
        }
    }
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
        // Per name in first-appearance order: the sum and count of its positions.
        let mut seen: Vec<(String, f64, u64)> = Vec::new();
        for tx in generators.iter().flat_map(|g| g.transactions()) {
            let primary = tx.stages.iter().filter(|stage| stage.lane == primary_lane);
            for (position, stage) in primary.enumerate() {
                match seen.iter_mut().find(|(name, _, _)| *name == stage.name) {
                    Some((_, sum, n)) => {
                        *sum += position as f64;
                        *n += 1;
                    }
                    None => seen.push((stage.name.clone(), position as f64, 1)),
                }
            }
        }
        let mut order: Vec<usize> = (0..seen.len()).collect();
        // A stable sort keeps first appearance among equal positions.
        order.sort_by(|&a, &b| {
            let mean = |i: usize| seen[i].1 / seen[i].2 as f64;
            mean(a).total_cmp(&mean(b))
        });
        let names: Vec<String> = order.into_iter().map(|i| seen[i].0.clone()).collect();
        let steps = names.len().saturating_sub(1).max(1) as f32;
        let alternate = names.len() > 8;
        let by_name = names
            .iter()
            .enumerate()
            .map(|(k, name)| {
                let hue = (250.0 - k as f32 * (250.0 / steps)).rem_euclid(360.0) / 360.0;
                let lightness = if alternate && k % 2 == 1 { 0.70 } else { 0.58 };
                (name.clone(), ladder_style(hue, lightness))
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
    /// The empty name is a lane like any other: the vtr_trace package writes
    /// its default lane as `""`.
    pub fn primary_lane(&self) -> &str {
        &self.primary_lane
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

fn ladder_style(hue: f32, lightness: f32) -> StageStyle {
    StageStyle {
        fill: Color {
            h: hue,
            s: 0.52,
            l: lightness,
            a: 1.0,
        },
        edge: Color {
            h: hue,
            s: 0.52,
            l: lightness - 0.20,
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
    fn ladder_follows_pipeline_position_on_the_primary_lane() {
        // X first appears before D, but sits later in the pipeline on average.
        let g = generator(&[
            &[("F", "0"), ("stl", "1"), ("X", "0")],
            &[("F", "0"), ("D", "0"), ("X", "0")],
        ]);
        let p = StagePalette::build(&[g]);
        assert_eq!(p.names(), ["F", "D", "X"]);
        assert_eq!(p.primary_lane(), "0");
        assert_ne!(p.style("F"), p.style("X"));
        assert_eq!(p.style("stl"), StagePalette::fallback());
        assert!(p.style("F").fill.l > 0.5 && p.style("F").edge.l < p.style("F").fill.l);
        let named = generator(&[&[("F", "main"), ("M", "memory")], &[("W", "main")]]);
        let p = StagePalette::build(&[named]);
        assert_eq!(p.primary_lane(), "main");
        assert_eq!(p.names(), ["F", "W"]);
        assert_eq!(StagePalette::default().primary_lane(), "0");
        // The vtr_trace package's default lane is the empty name.
        let unnamed = generator(&[&[("F", ""), ("S", "stall"), ("X", "")]]);
        let p = StagePalette::build(&[unnamed]);
        assert_eq!(p.primary_lane(), "");
        assert_eq!(p.names(), ["F", "X"]);
    }

    #[test]
    fn a_dozen_stages_keep_pipeline_order_and_neighbours_apart() {
        // The C910 tracer's stages: alternatives (EX, BJ, AG) at the same position,
        // an LSU path that makes CM and RT later on average, and first appearance
        // out of pipeline order.
        let alu: &[(&str, &str)] = &[
            ("ID", ""),
            ("IR", ""),
            ("IS", ""),
            ("IQ", ""),
            ("RF", ""),
            ("EX", ""),
            ("CM", ""),
            ("RT", ""),
        ];
        let branch: &[(&str, &str)] = &[
            ("ID", ""),
            ("IR", ""),
            ("IS", ""),
            ("IQ", ""),
            ("RF", ""),
            ("BJ", ""),
            ("CM", ""),
            ("RT", ""),
        ];
        let load: &[(&str, &str)] = &[
            ("ID", ""),
            ("IR", ""),
            ("IS", ""),
            ("IQ", ""),
            ("RF", ""),
            ("AG", ""),
            ("DC", ""),
            ("DA", ""),
            ("WB", ""),
            ("CM", ""),
            ("RT", ""),
        ];
        let p = StagePalette::build(&[generator(&[alu, load, branch, alu])]);
        assert_eq!(
            p.names(),
            [
                "ID", "IR", "IS", "IQ", "RF", "EX", "AG", "BJ", "DC", "CM", "DA", "RT", "WB"
            ]
        );
        for pair in p.names().windows(2) {
            let (a, b) = (p.style(&pair[0]).fill, p.style(&pair[1]).fill);
            assert!((a.l - b.l).abs() > 0.1, "{pair:?} differ in lightness");
            assert!(a.h != b.h);
        }
        // Few stages keep one lightness.
        let few = StagePalette::build(&[generator(&[alu])]);
        assert!(few.names().iter().all(|n| few.style(n).fill.l == 0.58));
    }
}
