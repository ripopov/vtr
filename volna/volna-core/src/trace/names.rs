//! Short names for the traces of a set: the part of each source that tells
//! it apart from the others.

/// The last segment of a path or URI.
pub(crate) fn file_name(source: &str) -> &str {
    source
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(source)
}

fn segments(source: &str) -> Vec<&str> {
    source
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .collect()
}

/// A name for each source, in order. Different file names name their
/// traces (`cpu.vtr`, `dram.fst`); runs of one design in different
/// directories are named by the first directory that differs, counted from
/// the file (`runs/base/landing.vtr` and `runs/dram20/landing.vtr` are
/// `base` and `dram20`). Identical sources share their file name.
pub fn derive_names(sources: &[&str]) -> Vec<String> {
    let split: Vec<Vec<&str>> = sources.iter().map(|s| segments(s)).collect();
    let file = |s: &[&str], i: usize| s.last().copied().unwrap_or(sources[i]).to_owned();
    if split.len() < 2 {
        return (0..split.len()).map(|i| file(&split[i], i)).collect();
    }
    // Segments counted from the end, so files at different depths line up.
    fn back<'a>(s: &[&'a str], k: usize) -> Option<&'a str> {
        s.len().checked_sub(k + 1).map(|i| s[i])
    }
    let longest = split.iter().map(Vec::len).max().unwrap_or(0);
    let differs = |k: usize| {
        let first = back(&split[0], k);
        split.iter().any(|s| back(s, k) != first)
    };
    match (0..longest).find(|&k| differs(k)) {
        Some(0) | None => (0..split.len()).map(|i| file(&split[i], i)).collect(),
        Some(k) => split
            .iter()
            .enumerate()
            .map(|(i, s)| back(s, k).map_or_else(|| file(s, i), str::to_owned))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn different_files_are_named_by_file() {
        assert_eq!(
            derive_names(&["/sim/cpu/landing.vtr", "/sim/dram/dram.fst"]),
            ["landing.vtr", "dram.fst"]
        );
    }

    #[test]
    fn runs_of_one_design_are_named_by_the_directory_that_differs() {
        assert_eq!(
            derive_names(&[
                "/work/runs/base/out/landing.vtr",
                "/work/runs/dram20/out/landing.vtr",
                "C:\\work\\runs\\reset12\\out\\landing.vtr",
            ]),
            ["base", "dram20", "reset12"]
        );
    }

    #[test]
    fn a_single_or_repeated_source_keeps_its_file_name() {
        assert_eq!(derive_names(&["file:///a/b/c.fst"]), ["c.fst"]);
        assert_eq!(derive_names(&["/a/c.vtr", "/a/c.vtr"]), ["c.vtr", "c.vtr"]);
        assert_eq!(derive_names(&[]), Vec::<String>::new());
    }

    #[test]
    fn a_shallower_path_falls_back_to_its_file_name() {
        assert_eq!(derive_names(&["/runs/a/x.vtr", "x.vtr"]), ["a", "x.vtr"]);
    }
}
