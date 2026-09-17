//! Ranked fuzzy search over the registry. Deterministic and allocation-light;
//! the frontend shows the ranked hits and highlights the matched ranges.
//!
//! Filters first: `@modified`, `@page:<id or title prefix>`, `@id:<prefix>`.
//! Every remaining term must match (AND); per term the best field wins with
//! the ladder title 1.0 > id 0.8 > keywords 0.7 > description 0.4. Word
//! matches score 100/80/60 (whole word, word prefix, substring); the fzf-like
//! subsequence match scores up to 50, needs three characters and is discarded
//! under 15 so a short term cannot collect stray letters.

use std::ops::Range;

use super::registry::{Host, Page, REGISTRY, Spec};

/// Why an entry matched, for the secondary line under the title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Matched {
    Title,
    Id,
    Keyword(String),
    Description,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub spec: &'static Spec,
    pub score: f64,
    /// Byte ranges in the title to highlight.
    pub title_ranges: Vec<Range<usize>>,
    /// Byte ranges in the id (with dots replaced by spaces) to highlight.
    pub id_ranges: Vec<Range<usize>>,
    pub matched: Matched,
}

/// The parsed query.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub modified: bool,
    pub page: Option<String>,
    pub id_prefix: Option<String>,
    pub terms: Vec<String>,
}

impl Query {
    pub fn parse(query: &str) -> Self {
        let mut out = Query::default();
        for token in query.split_whitespace() {
            let lower = token.to_lowercase();
            if lower == "@modified" {
                out.modified = true;
            } else if let Some(page) = lower.strip_prefix("@page:") {
                out.page = Some(page.to_owned());
            } else if let Some(id) = lower.strip_prefix("@id:") {
                out.id_prefix = Some(id.to_owned());
            } else if lower.starts_with('@') {
                // Unknown filters are ignored.
            } else {
                out.terms.push(lower);
            }
        }
        out
    }

    /// Whether the query narrows the list at all.
    pub fn is_active(&self) -> bool {
        self.modified || self.page.is_some() || self.id_prefix.is_some() || !self.terms.is_empty()
    }
}

fn is_boundary(hay: &[u8], k: usize) -> bool {
    if k == 0 {
        return true;
    }
    let a = hay[k - 1];
    let b = hay[k];
    !a.is_ascii_alphanumeric() || (a.is_ascii_lowercase() && b.is_ascii_uppercase())
}

fn merge(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_by_key(|r| r.start);
    let mut out: Vec<Range<usize>> = Vec::new();
    for r in ranges {
        match out.last_mut() {
            Some(last) if last.end >= r.start => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

struct Match {
    score: f64,
    ranges: Vec<Range<usize>>,
}

/// fzf-like in-order character match. `hay` is matched case-insensitively.
fn fuzzy(hay: &str, term: &str) -> Option<Match> {
    if term.chars().count() < 3 {
        return None;
    }
    let lower = hay.to_lowercase();
    // Only ASCII is boundary-aware; positions stay byte offsets.
    if lower.len() != hay.len() {
        return None;
    }
    let bytes = hay.as_bytes();
    let lower = lower.as_bytes();
    let mut score = 0.0;
    let mut prev: Option<usize> = None;
    let mut pos = 0;
    let mut ranges = Vec::new();
    for ch in term.bytes() {
        let k = lower[pos..].iter().position(|c| *c == ch)? + pos;
        let mut s = 1.0;
        if is_boundary(bytes, k) {
            s += 3.0;
        }
        if prev.is_some_and(|p| k == p + 1) {
            s += 2.0;
        }
        s -= ((k - pos).min(6) as f64) * 0.3;
        score += s;
        ranges.push(k..k + 1);
        prev = Some(k);
        pos = k + 1;
    }
    let total = score * 3.0;
    if total < 15.0 {
        return None;
    }
    Some(Match {
        score: total.min(50.0),
        ranges: merge(ranges),
    })
}

fn word_match(hay: &str, term: &str) -> Option<Match> {
    let lower = hay.to_lowercase();
    let words: Vec<(usize, &str)> = {
        let mut out = Vec::new();
        let mut start = None;
        for (i, c) in lower.char_indices() {
            if c.is_ascii_alphanumeric() {
                start.get_or_insert(i);
            } else if let Some(s) = start.take() {
                out.push((s, &lower[s..i]));
            }
        }
        if let Some(s) = start {
            out.push((s, &lower[s..]));
        }
        out
    };
    let hit = |at: usize, score: f64| {
        Some(Match {
            score,
            ranges: std::iter::once(at..at + term.len()).collect(),
        })
    };
    if let Some((at, _)) = words.iter().find(|(_, w)| *w == term) {
        return hit(*at, 100.0);
    }
    if let Some((at, _)) = words.iter().find(|(_, w)| w.starts_with(term)) {
        return hit(*at, 80.0);
    }
    if let Some(k) = lower.find(term) {
        return hit(k, 60.0);
    }
    None
}

fn best(hay: &str, term: &str, weight: f64, allow_fuzzy: bool) -> Option<Match> {
    if let Some(w) = word_match(hay, term) {
        return Some(Match {
            score: w.score * weight,
            ranges: w.ranges,
        });
    }
    if allow_fuzzy && let Some(f) = fuzzy(hay, term) {
        return Some(Match {
            score: f.score * weight,
            ranges: f.ranges,
        });
    }
    None
}

/// Rank the registry entries available on `host` for `query`. `modified`
/// reports whether an id is present in the user's document.
pub fn search(query: &str, host: Host, modified: &dyn Fn(&str) -> bool) -> Vec<Hit> {
    let query = Query::parse(query);
    let mut hits: Vec<(Hit, usize)> = Vec::new();
    for (order, spec) in REGISTRY.iter().enumerate() {
        if !spec.available(host) {
            continue;
        }
        if query.modified && !modified(spec.id) {
            continue;
        }
        if let Some(page) = &query.page
            && !(spec.page.id() == page || spec.page.title().to_lowercase().starts_with(page))
        {
            continue;
        }
        if let Some(prefix) = &query.id_prefix
            && !spec.id.to_lowercase().starts_with(prefix)
        {
            continue;
        }
        let mut total = 0.0;
        let mut title_ranges = Vec::new();
        let mut id_ranges = Vec::new();
        let mut matched: Option<Matched> = None;
        let id_hay = spec.id.replace('.', " ");
        let mut all = true;
        for term in &query.terms {
            let mut candidates: Vec<(Match, Matched)> = Vec::new();
            if let Some(m) = best(spec.title, term, 1.0, true) {
                candidates.push((m, Matched::Title));
            }
            if let Some(m) = best(&id_hay, term, 0.8, true) {
                candidates.push((m, Matched::Id));
            }
            for kw in spec.keywords {
                if let Some(m) = best(kw, term, 0.7, false) {
                    candidates.push((
                        Match {
                            score: m.score,
                            ranges: Vec::new(),
                        },
                        Matched::Keyword((*kw).to_owned()),
                    ));
                }
            }
            let joined = spec.keywords.join(" ");
            if let Some(f) = fuzzy(&joined, term) {
                // Name the keywords the match spans.
                let mut at = 0;
                let mut names = Vec::new();
                for kw in spec.keywords {
                    let range = at..at + kw.len();
                    if f.ranges
                        .iter()
                        .any(|r| r.start < range.end && r.end > range.start)
                    {
                        names.push(*kw);
                    }
                    at += kw.len() + 1;
                }
                candidates.push((
                    Match {
                        score: f.score * 0.7,
                        ranges: Vec::new(),
                    },
                    Matched::Keyword(names.join(", ")),
                ));
            }
            if let Some(m) = best(spec.description, term, 0.4, false) {
                candidates.push((
                    Match {
                        score: m.score,
                        ranges: Vec::new(),
                    },
                    Matched::Description,
                ));
            }
            let Some((m, why)) = candidates
                .into_iter()
                .max_by(|a, b| a.0.score.partial_cmp(&b.0.score).unwrap())
            else {
                all = false;
                break;
            };
            total += m.score;
            match &why {
                Matched::Title => title_ranges.extend(m.ranges),
                Matched::Id => id_ranges.extend(m.ranges),
                _ => {}
            }
            if !matches!(why, Matched::Title) && matched.is_none() {
                matched = Some(why);
            }
        }
        if !all {
            continue;
        }
        hits.push((
            Hit {
                spec,
                score: total,
                title_ranges: merge(title_ranges),
                id_ranges: merge(id_ranges),
                matched: matched.unwrap_or(Matched::Title),
            },
            order,
        ));
    }
    hits.sort_by(|(a, ao), (b, bo)| {
        b.score
            .partial_cmp(&a.score)
            .unwrap()
            .then_with(|| modified(b.spec.id).cmp(&modified(a.spec.id)))
            .then_with(|| ao.cmp(bo))
    });
    hits.into_iter().map(|(hit, _)| hit).collect()
}

/// Hits per page, for the table of contents.
pub fn count_per_page(hits: &[Hit]) -> Vec<(Page, usize)> {
    Page::ALL
        .iter()
        .map(|page| (*page, hits.iter().filter(|h| h.spec.page == *page).count()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> bool {
        false
    }

    #[test]
    fn folcur_finds_link_new_panels_through_its_keywords() {
        let hits = search("folcur", Host::Native, &none);
        assert_eq!(hits[0].spec.id, "panels.linkByDefault");
        assert_eq!(hits[0].matched, Matched::Keyword("follow, cursor".into()));
        let hits = search("cursor follow", Host::Native, &none);
        assert_eq!(hits[0].spec.id, "panels.linkByDefault");
    }

    #[test]
    fn snap_px_matches_title_and_id_segment_with_ranges() {
        let hits = search("snap px", Host::Native, &none);
        assert_eq!(hits[0].spec.id, "waves.snapPixels");
        assert_eq!(hits[0].title_ranges, vec![0..4]);
        let hits = search("snap", Host::Native, &none);
        assert_eq!(hits[0].spec.id, "waves.snapPixels");
        assert!(hits[0].score > 60.0);
    }

    #[test]
    fn reduce_matches_animation_by_keyword_and_short_terms_never_fuzz() {
        let hits = search("reduce", Host::Native, &none);
        assert_eq!(hits[0].spec.id, "waves.animation");
        assert!(matches!(hits[0].matched, Matched::Keyword(ref k) if k == "reduce"));
        assert!(search("zq", Host::Native, &none).is_empty());
        assert!(search("xyzzy", Host::Native, &none).is_empty());
    }

    #[test]
    fn filters_and_host_availability_narrow_the_list() {
        let modified = |id: &str| id == "waves.snapPixels" || id == "waves.animation";
        let hits = search("@modified snap", Host::Native, &modified);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].spec.id, "waves.snapPixels");
        let hits = search("@modified", Host::Native, &modified);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].spec.id, "waves.animation");
        let hits = search("@page:work", Host::Native, &none);
        assert!(hits.iter().all(|h| h.spec.page == Page::Workspace));
        assert_eq!(search("@id:waves.snap", Host::Native, &none).len(), 1);
        assert!(search("memory", Host::Native, &none).is_empty());
        assert_eq!(
            search("memory", Host::Vscode, &none)[0].spec.id,
            "remote.memoryMiB"
        );
        assert_eq!(
            search("", Host::Native, &none).len(),
            REGISTRY
                .iter()
                .filter(|s| s.available(Host::Native))
                .count()
        );
    }

    #[test]
    fn ranking_is_stable_and_modified_wins_ties() {
        let ids = |hits: Vec<Hit>| hits.iter().map(|h| h.spec.id).collect::<Vec<_>>();
        let a = ids(search("limit", Host::Vscode, &none));
        let b = ids(search("limit", Host::Vscode, &none));
        assert_eq!(a, b);
        let modified = |id: &str| id == "remote.objectMiB";
        let hits = search("@page:remote limit", Host::Vscode, &modified);
        assert_eq!(hits[0].spec.id, "remote.objectMiB");
    }
}
