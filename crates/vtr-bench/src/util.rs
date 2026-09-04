//! Timing helpers.

use std::time::Instant;

/// Process CPU time (user + system) in seconds, from /proc/self/stat.
pub fn cpu_seconds() -> f64 {
    let s = match std::fs::read_to_string("/proc/self/stat") {
        Ok(s) => s,
        Err(_) => return 0.0,
    };
    // Fields after the closing paren of the command name.
    let rest = match s.rfind(')') {
        Some(i) => &s[i + 2..],
        None => return 0.0,
    };
    let f: Vec<&str> = rest.split_whitespace().collect();
    // utime is field 14 overall => index 11 after "state"; stime index 12.
    let ut: f64 = f.get(11).and_then(|x| x.parse().ok()).unwrap_or(0.0);
    let st: f64 = f.get(12).and_then(|x| x.parse().ok()).unwrap_or(0.0);
    (ut + st) / 100.0
}

/// Peak resident set size in bytes (VmHWM).
#[allow(dead_code)]
pub fn peak_rss() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmHWM:")).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()))
        .map(|kb| kb * 1024)
        .unwrap_or(0)
}

pub struct Stopwatch {
    start: Instant,
    cpu0: f64,
}

impl Stopwatch {
    pub fn start() -> Self {
        Stopwatch { start: Instant::now(), cpu0: cpu_seconds() }
    }
    /// (wall seconds, cpu seconds)
    pub fn stop(&self) -> (f64, f64) {
        (self.start.elapsed().as_secs_f64(), cpu_seconds() - self.cpu0)
    }
}

pub fn file_size(path: &str) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Best-of-N wall time for a closure (returns min wall, plus the last result).
pub fn best_of<T>(n: usize, mut f: impl FnMut() -> T) -> (f64, T) {
    let mut best = f64::MAX;
    let mut last = None;
    for _ in 0..n.max(1) {
        let t = Instant::now();
        let r = f();
        best = best.min(t.elapsed().as_secs_f64());
        last = Some(r);
    }
    (best, last.unwrap())
}
