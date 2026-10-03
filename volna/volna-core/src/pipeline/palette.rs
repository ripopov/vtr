//! Stage colours. Each stage name on the primary lane gets a step of the
//! theme's stage ladder in pipeline order: names are ranked by their mean
//! position within a transaction's primary-lane stages (ties by first
//! appearance), so hues run from fetch to retire whatever order the trace
//! first shows them in. The theme turns a rank into colours
//! ([`Theme::stage_style`]), so one palette serves a dark pipeline and a light
//! transaction panel. The counts come from each generator's `StageCensus`,
//! taken once at load, so building a palette costs lanes × names rather than
//! a scan of every record. A VDB stage table later fills the same struct with
//! authored colours; the painter only ever asks for `style(name, theme)`.

use std::collections::HashMap;
use std::sync::Arc;

use crate::color::Color;
use crate::data::loaded_tracks::LoadedGenerator;
use crate::theme::Theme;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StageStyle {
    pub fill: Color,
    pub edge: Color,
    pub text: Color,
}

/// Where a stage sits on the theme's ladder, resolved to colours by
/// [`Theme::swatch`] with whichever theme paints it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageSwatch {
    /// Step `rank` of a ladder of `of` stages.
    Step { rank: usize, of: usize },
    /// Grey: a name outside the ladder, or a transaction without stages.
    Fallback,
}

/// Konata's default lane, used as the primary lane whenever it occurs.
pub const DEFAULT_LANE: &str = "0";

#[derive(Clone, Debug)]
pub struct StagePalette {
    /// Rank in pipeline order.
    by_name: HashMap<String, usize>,
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
        // Merging the census of each generator in order keeps first
        // appearance across generators, so no record is scanned here.
        let lanes = || generators.iter().flat_map(|g| &g.stage_census().lanes);
        let mut lane_counts: Vec<(&str, u64)> = Vec::new();
        for lane in lanes() {
            match lane_counts.iter_mut().find(|(name, _)| *name == lane.lane) {
                Some((_, n)) => *n += lane.stages,
                None => lane_counts.push((&lane.lane, lane.stages)),
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
        let mut seen: Vec<(String, u64, u64)> = Vec::new();
        for census in lanes()
            .filter(|lane| lane.lane == primary_lane)
            .flat_map(|lane| &lane.names)
        {
            match seen.iter_mut().find(|(name, _, _)| *name == census.name) {
                Some((_, sum, n)) => {
                    *sum += census.position_sum;
                    *n += census.count;
                }
                None => seen.push((census.name.clone(), census.position_sum, census.count)),
            }
        }
        let mut order: Vec<usize> = (0..seen.len()).collect();
        // A stable sort keeps first appearance among equal positions.
        order.sort_by(|&a, &b| {
            let mean = |i: usize| seen[i].1 as f64 / seen[i].2 as f64;
            mean(a).total_cmp(&mean(b))
        });
        let names: Vec<String> = order.into_iter().map(|i| seen[i].0.clone()).collect();
        let by_name = names
            .iter()
            .enumerate()
            .map(|(k, name)| (name.clone(), k))
            .collect();
        Self {
            by_name,
            names,
            primary_lane,
        }
    }

    /// Where stage `name` sits on the ladder.
    pub fn swatch(&self, name: &str) -> StageSwatch {
        match self.by_name.get(name) {
            Some(&rank) => StageSwatch::Step {
                rank,
                of: self.names.len(),
            },
            None => StageSwatch::Fallback,
        }
    }

    /// The colours of stage `name` in `theme`; names outside the ladder are grey.
    pub fn style(&self, name: &str, theme: &Theme) -> StageStyle {
        theme.swatch(self.swatch(name))
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use volna_trace::data::transactions::*;

    fn one_dark() -> Theme {
        Theme::one_dark()
    }

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
        assert_ne!(p.style("F", &one_dark()), p.style("X", &one_dark()));
        assert_eq!(p.style("stl", &one_dark()), one_dark().stage_fallback());
        assert!(
            p.style("F", &one_dark()).fill.l > 0.5
                && p.style("F", &one_dark()).edge.l < p.style("F", &one_dark()).fill.l
        );
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
            let (a, b) = (
                p.style(&pair[0], &one_dark()).fill,
                p.style(&pair[1], &one_dark()).fill,
            );
            assert!((a.l - b.l).abs() > 0.1, "{pair:?} differ in lightness");
            assert!(a.h != b.h);
        }
        // Few stages keep one lightness.
        let few = StagePalette::build(&[generator(&[alu])]);
        assert!(
            few.names()
                .iter()
                .all(|n| few.style(n, &one_dark()).fill.l == 0.58)
        );
    }

    #[test]
    fn generators_merge_as_one_record_list() {
        // X is seen before D, but D sits earlier once both generators count;
        // lane "p" is primary only by the combined stage count.
        let first: &[&[(&str, &str)]] = &[&[("F", "p"), ("X", "p")], &[("s", "q"), ("s", "q")]];
        let second: &[&[(&str, &str)]] = &[
            &[("F", "p"), ("D", "p"), ("X", "p")],
            &[("s", "q"), ("F", "p"), ("D", "p"), ("X", "p")],
        ];
        let merged = StagePalette::build(&[generator(first), generator(second)]);
        let together = [first, second].concat();
        let single = StagePalette::build(&[generator(&together)]);
        assert_eq!(merged.primary_lane(), "p");
        assert_eq!(merged.names(), ["F", "D", "X"]);
        assert_eq!(merged.names(), single.names());
        assert_eq!(
            merged.style("D", &one_dark()),
            single.style("D", &one_dark())
        );
    }
}
