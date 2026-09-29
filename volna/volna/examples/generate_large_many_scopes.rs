---
[package]
edition = "2021"

[dependencies]
vtr = { path = "../../../core/vtr" }

[profile.dev]
opt-level = 3
---

//! Generate a hierarchy-heavy 4 GB waveform for VTR and Volna profiling.
//!
//! From the repository root:
//! cargo -Zscript volna/volna/examples/generate_large_many_scopes.rs \
//!   volna/volna/examples/large_many_scopes.vtr
//!
//! Optional arguments are output path and samples per signal. The default is
//! 10 million samples for each of 128 real-valued sine waves in 32 scopes.

use std::error::Error;
use std::f64::consts::TAU;
use std::path::{Path, PathBuf};
use std::time::Instant;
use vtr::{
    Direction, NodeKind, Reader, ScopeType, SignalId, SignalKind, VarType, Writer, WriterOptions,
};

const SCOPE_COUNT: usize = 32;
const SIGNALS_PER_SCOPE: usize = 4;
const SIGNAL_COUNT: usize = SCOPE_COUNT * SIGNALS_PER_SCOPE;
const DEFAULT_SAMPLES: u64 = 10_000_000;
const RESEED_INTERVAL: u64 = 1_000_000;

struct Oscillator {
    id: SignalId,
    amplitude: f64,
    delta_sin: f64,
    delta_cos: f64,
    sin: f64,
    cos: f64,
    cycles: f64,
}

fn args() -> (PathBuf, u64) {
    let mut args = std::env::args_os().skip(1);
    let path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("volna/volna/examples/large_many_scopes.vtr"));
    let samples = args
        .next()
        .map(|value| {
            value
                .to_string_lossy()
                .parse()
                .expect("samples must be an integer")
        })
        .unwrap_or(DEFAULT_SAMPLES);
    assert!(
        args.next().is_none(),
        "usage: generate_large_many_scopes.rs [OUTPUT] [SAMPLES_PER_SIGNAL]"
    );
    (path, samples)
}

fn generate(path: &Path, samples: u64) -> Result<(), Box<dyn Error>> {
    let started = Instant::now();
    let mut writer = Writer::create_with(
        path,
        WriterOptions {
            dedup: false,
            ..Default::default()
        },
    )?;
    writer.set_timescale(-9)?;
    writer.set_writer_name("Volna many-scopes performance fixture")?;
    writer.set_comment("32 nested scopes, 128 sine-wave variables, 10 million samples per variable by default; benchmarks hierarchy and dense multi-signal waveform access.")?;

    let top = writer.add_scope(
        None,
        "many_scopes",
        ScopeType::Module,
        "performance_fixture",
    )?;
    let mut oscillators = Vec::with_capacity(SIGNAL_COUNT);
    for scope_index in 0..SCOPE_COUNT {
        let scope = writer.add_scope(
            Some(top),
            &format!("cluster_{scope_index:02}"),
            ScopeType::Module,
            "wave_cluster",
        )?;
        for local_index in 0..SIGNALS_PER_SCOPE {
            let index = scope_index * SIGNALS_PER_SCOPE + local_index;
            let cycles = 1.137 + (index as f64 + 1.0) * 1.61803398875;
            let amplitude = 0.25 + ((index * 73) % 997) as f64 / 37.0;
            let phase = (index as f64 * 0.7548776662466927).rem_euclid(TAU);
            let (sin, cos) = phase.sin_cos();
            let delta = TAU * cycles / samples as f64;
            let (delta_sin, delta_cos) = delta.sin_cos();
            let name = format!("sine_{index:03}_amp_{amplitude:.3}");
            let id = writer
                .add_var(
                    Some(scope),
                    &name,
                    VarType::Real,
                    Direction::Output,
                    SignalKind::Real,
                )?
                .1;
            oscillators.push(Oscillator {
                id,
                amplitude,
                delta_sin,
                delta_cos,
                sin,
                cos,
                cycles,
            });
        }
    }

    eprintln!(
        "writing {samples} samples for {SIGNAL_COUNT} signals across {SCOPE_COUNT} scopes to {}",
        path.display()
    );
    for t in 0..samples {
        writer.set_time(t)?;
        for osc in &mut oscillators {
            writer.emit_real(osc.id, osc.amplitude * osc.sin)?;
            (osc.sin, osc.cos) = (
                osc.sin * osc.delta_cos + osc.cos * osc.delta_sin,
                osc.cos * osc.delta_cos - osc.sin * osc.delta_sin,
            );
        }
        if (t + 1) % RESEED_INTERVAL == 0 {
            let phase_fraction = (t + 1) as f64 / samples as f64;
            for (index, osc) in oscillators.iter_mut().enumerate() {
                let phase = (TAU * osc.cycles * phase_fraction + index as f64 * 0.7548776662466927)
                    .rem_euclid(TAU);
                (osc.sin, osc.cos) = phase.sin_cos();
            }
        }
        if (t + 1) % 1_000_000 == 0 {
            eprintln!("  timestamps: {}/{}", t + 1, samples);
        }
    }

    let expected_records = samples * SIGNAL_COUNT as u64;
    assert_eq!(writer.stats().records, expected_records);
    writer.close()?;
    let reader = Reader::open(path)?;
    assert_eq!(reader.signal_count(), SIGNAL_COUNT as u32);
    assert_eq!(
        reader.hierarchy().nodes_of_kind(NodeKind::Scope).count(),
        SCOPE_COUNT + 1
    );
    for index in 0..SIGNAL_COUNT {
        let signal = SignalId(index as u32);
        assert_eq!(reader.signal_var_type(signal)?, VarType::Real);
        assert_eq!(reader.signal_kind(signal)?, SignalKind::Real);
    }
    assert_eq!(reader.time_range(), Some((0, samples - 1)));
    let bytes = std::fs::metadata(path)?.len();
    eprintln!(
        "wrote {}: {expected_records} records, {} scopes, {:.2} GiB in {:.1}s",
        path.display(),
        SCOPE_COUNT + 1,
        bytes as f64 / (1024.0 * 1024.0 * 1024.0),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let (path, samples) = args();
    assert!(samples > 0, "samples per signal must be positive");
    generate(&path, samples)
}
