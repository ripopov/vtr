---
[package]
edition = "2021"

[dependencies]
vtr = { path = "../../../core/vtr" }

[profile.dev]
opt-level = 3
---

//! Generate a dense 15-signal analog waveform for VTR and Volna profiling.
//!
//! From the repository root:
//! cargo -Zscript volna/volna/examples/generate_large_analog.rs \
//!   volna/volna/examples/large_analog_signals.vtr
//!
//! Optional arguments are output path and samples per signal. The default is
//! 130 million samples per signal, across two scopes and multiple VTR types.

use std::error::Error;
use std::f64::consts::TAU;
use std::path::{Path, PathBuf};
use std::time::Instant;
use vtr::{
    Direction, NodeKind, Reader, ScopeType, SignalId, SignalKind, VarType, Writer, WriterOptions,
};

const DEFAULT_SAMPLES: u64 = 130_000_000;
const RESEED_INTERVAL: u64 = 1_000_000;

#[derive(Clone, Copy)]
enum Wave {
    Sine,
    Cosine,
    Tanh,
    Noise,
    Saw,
    Triangle,
    Chirp,
    Damped,
    Rectified,
    Harmonics,
    Pulse,
    Am,
    SoftClip,
    Ripple,
    NoiseEnvelope,
}

#[derive(Clone, Copy)]
enum Encoding {
    Real,
    Bits { ty: VarType, width: u32 },
}

#[derive(Clone, Copy)]
struct Spec {
    name: &'static str,
    wave: Wave,
    encoding: Encoding,
    amplitude: f64,
    offset: f64,
    cycles: f64,
}

struct Channel {
    id: SignalId,
    spec: Spec,
    phase: f64,
    delta: f64,
    noise: u64,
}

const SPECS: [Spec; 15] = [
    Spec {
        name: "sine_real",
        wave: Wave::Sine,
        encoding: Encoding::Real,
        amplitude: 1.0,
        offset: 0.0,
        cycles: 7.25,
    },
    Spec {
        name: "cosine_real",
        wave: Wave::Cosine,
        encoding: Encoding::Real,
        amplitude: 12.0,
        offset: -2.0,
        cycles: 19.5,
    },
    Spec {
        name: "tanh_real",
        wave: Wave::Tanh,
        encoding: Encoding::Real,
        amplitude: 4.0,
        offset: 0.5,
        cycles: 3.75,
    },
    Spec {
        name: "white_noise_real",
        wave: Wave::Noise,
        encoding: Encoding::Real,
        amplitude: 3.0,
        offset: 0.0,
        cycles: 1.0,
    },
    Spec {
        name: "saw_int32",
        wave: Wave::Saw,
        encoding: Encoding::Bits {
            ty: VarType::Int,
            width: 32,
        },
        amplitude: 1_000_000.0,
        offset: -100_000.0,
        cycles: 11.125,
    },
    Spec {
        name: "triangle_int32",
        wave: Wave::Triangle,
        encoding: Encoding::Bits {
            ty: VarType::Int,
            width: 32,
        },
        amplitude: 500_000.0,
        offset: 100_000.0,
        cycles: 31.75,
    },
    Spec {
        name: "chirp_real",
        wave: Wave::Chirp,
        encoding: Encoding::Real,
        amplitude: 2.5,
        offset: 0.25,
        cycles: 2.0,
    },
    Spec {
        name: "damped_real",
        wave: Wave::Damped,
        encoding: Encoding::Real,
        amplitude: 20.0,
        offset: 0.0,
        cycles: 43.5,
    },
    Spec {
        name: "rectified_shortint",
        wave: Wave::Rectified,
        encoding: Encoding::Bits {
            ty: VarType::ShortInt,
            width: 16,
        },
        amplitude: 25_000.0,
        offset: 0.0,
        cycles: 5.625,
    },
    Spec {
        name: "harmonics_shortint",
        wave: Wave::Harmonics,
        encoding: Encoding::Bits {
            ty: VarType::ShortInt,
            width: 16,
        },
        amplitude: 12_000.0,
        offset: 500.0,
        cycles: 13.25,
    },
    Spec {
        name: "pulse_byte",
        wave: Wave::Pulse,
        encoding: Encoding::Bits {
            ty: VarType::Byte,
            width: 8,
        },
        amplitude: 110.0,
        offset: 10.0,
        cycles: 2.375,
    },
    Spec {
        name: "am_byte",
        wave: Wave::Am,
        encoding: Encoding::Bits {
            ty: VarType::Byte,
            width: 8,
        },
        amplitude: 100.0,
        offset: 0.0,
        cycles: 37.5,
    },
    Spec {
        name: "softclip_real",
        wave: Wave::SoftClip,
        encoding: Encoding::Real,
        amplitude: 8.0,
        offset: -0.5,
        cycles: 23.125,
    },
    Spec {
        name: "ripple_real",
        wave: Wave::Ripple,
        encoding: Encoding::Real,
        amplitude: 5.0,
        offset: 2.0,
        cycles: 53.75,
    },
    Spec {
        name: "noise_envelope_real",
        wave: Wave::NoiseEnvelope,
        encoding: Encoding::Real,
        amplitude: 6.0,
        offset: 1.0,
        cycles: 17.875,
    },
];

fn args() -> (PathBuf, u64) {
    let mut args = std::env::args_os().skip(1);
    let path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("volna/volna/examples/large_analog_signals.vtr"));
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
        "usage: generate_large_analog.rs [OUTPUT] [SAMPLES_PER_SIGNAL]"
    );
    (path, samples)
}

fn bits(width: u32) -> u64 {
    if width == 64 {
        u64::MAX
    } else {
        (1_u64 << width) - 1
    }
}

fn next_noise(state: &mut u64) -> f64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    ((*state >> 11) as f64 / ((1_u64 << 53) as f64)) * 2.0 - 1.0
}

fn value(channel: &mut Channel, t: u64, samples: u64, phase: f64) -> f64 {
    let sine = phase.sin();
    let cosine = phase.cos();
    let fraction = phase / TAU;
    let cycle = (t as f64 / samples as f64 * channel.spec.cycles).fract();
    match channel.spec.wave {
        Wave::Sine => sine,
        Wave::Cosine => cosine,
        Wave::Tanh => (5.0 * sine).tanh(),
        Wave::Noise => next_noise(&mut channel.noise),
        Wave::Saw => 2.0 * fraction - 1.0,
        Wave::Triangle => 1.0 - 4.0 * (fraction - 0.5).abs(),
        Wave::Chirp => ((TAU * (2.0 + 38.0 * (t as f64 / samples as f64)) * t as f64
            / samples as f64)
            .rem_euclid(TAU))
        .sin(),
        Wave::Damped => sine * (-8.0 * t as f64 / samples as f64).exp(),
        Wave::Rectified => sine.abs(),
        Wave::Harmonics => 0.7 * sine + 0.2 * (2.0 * phase).sin() + 0.1 * (5.0 * phase).cos(),
        Wave::Pulse => {
            if cycle < 0.08 {
                1.0
            } else {
                -1.0
            }
        }
        Wave::Am => (0.35 + 0.65 * (3.0 * phase).sin().abs()) * sine,
        Wave::SoftClip => (2.5 * sine).tanh(),
        Wave::Ripple => 0.7 * sine + 0.3 * (11.0 * phase).sin(),
        Wave::NoiseEnvelope => next_noise(&mut channel.noise) * (0.1 + 0.9 * cosine.abs()),
    }
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
    writer.set_writer_name("Volna analog-signals performance fixture")?;
    writer.set_comment("15 dense analog waveforms across two scopes: periodic and nonlinear real signals, deterministic noise, and quantized 8/16/32-bit integer channels. Benchmarks long selected-history loading and value decoding.")?;

    let scope_a = writer.add_scope(None, "sensors", ScopeType::Module, "analog_bank")?;
    let scope_b = writer.add_scope(None, "actuators", ScopeType::Module, "analog_bank")?;
    let mut channels = Vec::with_capacity(SPECS.len());
    for (index, spec) in SPECS.into_iter().enumerate() {
        let parent = if index < 8 { scope_a } else { scope_b };
        let (ty, kind) = match spec.encoding {
            Encoding::Real => (VarType::Real, SignalKind::Real),
            Encoding::Bits { ty, width } => (ty, SignalKind::Bits { width, states: 2 }),
        };
        let id = writer
            .add_var(Some(parent), spec.name, ty, Direction::Output, kind)?
            .1;
        let phase = (index as f64 * 0.6180339887498948).rem_euclid(TAU);
        channels.push(Channel {
            id,
            spec,
            phase,
            delta: TAU * spec.cycles / samples as f64,
            noise: 0x9e3779b97f4a7c15_u64 ^ (index as u64 + 1).wrapping_mul(0xd1b54a32d192ed03),
        });
    }

    eprintln!(
        "writing {samples} samples for {} signals across two scopes to {}",
        SPECS.len(),
        path.display()
    );
    for t in 0..samples {
        writer.set_time(t)?;
        for channel in &mut channels {
            let sample = channel.spec.offset
                + channel.spec.amplitude * value(channel, t, samples, channel.phase);
            match channel.spec.encoding {
                Encoding::Real => writer.emit_real(channel.id, sample)?,
                Encoding::Bits { width, .. } => {
                    let signed = sample.round() as i64;
                    writer.emit_u64(channel.id, signed as u64 & bits(width))?;
                }
            }
            channel.phase = (channel.phase + channel.delta).rem_euclid(TAU);
        }
        if (t + 1) % RESEED_INTERVAL == 0 {
            for (index, channel) in channels.iter_mut().enumerate() {
                let phase = (index as f64 * 0.6180339887498948
                    + TAU * channel.spec.cycles * (t + 1) as f64 / samples as f64)
                    .rem_euclid(TAU);
                channel.phase = phase;
            }
        }
        if (t + 1) % 1_000_000 == 0 {
            eprintln!("  timestamps: {}/{}", t + 1, samples);
        }
    }

    let expected_records = samples * SPECS.len() as u64;
    assert_eq!(writer.stats().records, expected_records);
    writer.close()?;
    let reader = Reader::open(path)?;
    assert_eq!(reader.signal_count(), SPECS.len() as u32);
    assert_eq!(reader.hierarchy().nodes_of_kind(NodeKind::Scope).count(), 2);
    for (index, spec) in SPECS.iter().enumerate() {
        let signal = SignalId(index as u32);
        let (expected_type, expected_kind) = match spec.encoding {
            Encoding::Real => (VarType::Real, SignalKind::Real),
            Encoding::Bits { ty, width } => (ty, SignalKind::Bits { width, states: 2 }),
        };
        assert_eq!(reader.signal_var_type(signal)?, expected_type);
        assert_eq!(reader.signal_kind(signal)?, expected_kind);
    }
    assert_eq!(reader.time_range(), Some((0, samples - 1)));
    let bytes = std::fs::metadata(path)?.len();
    eprintln!(
        "wrote {}: {expected_records} records, 2 scopes, {:.2} GiB in {:.1}s",
        path.display(),
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
