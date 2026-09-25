---
[package]
edition = "2021"

[dependencies]
vtr = { path = "../../../core/vtr" }

[profile.dev]
opt-level = 3
---

//! Generate the local large.vtr stress trace.
//!
//! From the repository root:
//! cargo -Zscript volna/volna/examples/generate_large_vtr.rs \
//!   volna/volna/examples/large.vtr

use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Instant;
use vtr::{
    Direction, Reader, ScopeType, SignalId, SignalKind, TxStatus, VarType, Writer,
    WriterOptions,
};

const FAST_SAMPLES: u64 = 100_000_000;
const TRANSACTIONS_PER_GENERATOR: u64 = 10_000_000;
const SINE_STEPS: usize = 1_000;

fn bits(w: &mut Writer, parent: NodeId, name: &str, ty: VarType, width: u32) -> vtr::Result<SignalId> {
    Ok(w.add_var(Some(parent), name, ty, Direction::Output, SignalKind::Bits { width, states: 2 })?.1)
}

fn as_bits(value: i64, width: u32) -> u64 {
    if width == 64 {
        value as u64
    } else {
        (value as u64) & ((1_u64 << width) - 1)
    }
}

fn output_path() -> PathBuf {
    std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("volna/volna/examples/large.vtr"))
}

fn generate(path: &Path) -> Result<(), Box<dyn Error>> {
    let started = Instant::now();
    let mut w = Writer::create_with(
        path,
        WriterOptions {
            // Every call below represents a sample, including repeated integer values.
            dedup: false,
            ..Default::default()
        },
    )?;
    w.set_timescale(-9)?;
    w.set_writer_name("Volna large-file performance generator")?;
    w.set_comment(
        "Synthetic 100 ms trace: four clocks, four typed sine waves, two streams, and four transaction generators.",
    )?;

    let top = w.add_scope(None, "large", ScopeType::Module, "performance_fixture")?;
    let clocks = [
        bits(&mut w, top, "clock_500mhz", VarType::Wire, 1)?,
        bits(&mut w, top, "clock_100mhz", VarType::Wire, 1)?,
        bits(&mut w, top, "clock_10mhz", VarType::Wire, 1)?,
        bits(&mut w, top, "clock_1mhz", VarType::Wire, 1)?,
    ];
    let sine_real = w
        .add_var(
            Some(top),
            "sine_1mhz_real64",
            VarType::Real,
            Direction::Output,
            SignalKind::Real,
        )?
        .1;
    let sine_i32 = bits(&mut w, top, "sine_100khz_int32", VarType::Int, 32)?;
    let sine_i16 = bits(&mut w, top, "sine_10khz_int16", VarType::ShortInt, 16)?;
    let sine_i8 = bits(&mut w, top, "sine_1khz_int8", VarType::Byte, 8)?;

    let cpu = w.add_stream(Some(top), "cpu_requests", "PERFORMANCE")?;
    let fabric = w.add_stream(Some(top), "fabric_packets", "PERFORMANCE")?;
    let generators = [
        w.add_generator(cpu, "reads")?,
        w.add_generator(cpu, "writes")?,
        w.add_generator(fabric, "requests")?,
        w.add_generator(fabric, "responses")?,
    ];

    let sine: Vec<f64> = (0..SINE_STEPS)
        .map(|i| (std::f64::consts::TAU * i as f64 / SINE_STEPS as f64).sin())
        .collect();
    // A slightly offset 1 MHz frequency has a one-second sampled period. It
    // remains non-repeating during this 100 ms trace instead of compressing to
    // a tiny repeated lookup table.
    let fast_delta = std::f64::consts::TAU * 1_000_003.0 / 1_000_000_000.0;
    let (delta_sin, delta_cos) = fast_delta.sin_cos();
    let (mut fast_sin, mut fast_cos) = (0.0_f64, 1.0_f64);

    eprintln!("writing {FAST_SAMPLES} fast waveform samples to {}", path.display());
    for t in 0..FAST_SAMPLES {
        w.set_time(t)?;
        w.emit_bit(clocks[0], (t & 1) as u8)?;
        w.emit_real(sine_real, fast_sin)?;
        (fast_sin, fast_cos) = (
            fast_sin * delta_cos + fast_cos * delta_sin,
            fast_cos * delta_cos - fast_sin * delta_sin,
        );
        // Bound accumulated oscillator error without changing the sampled phase.
        if t % 1_000_000 == 999_999 {
            let phase = (fast_delta * (t + 1) as f64).rem_euclid(std::f64::consts::TAU);
            (fast_sin, fast_cos) = phase.sin_cos();
        }

        if t % 5 == 0 {
            w.emit_bit(clocks[1], ((t / 5) & 1) as u8)?;
        }
        if t % 10 == 0 {
            let sample = sine[(t / 10) as usize % SINE_STEPS];
            w.emit_u64(sine_i32, as_bits((sample * i32::MAX as f64).round() as i64, 32))?;
        }
        if t % 50 == 0 {
            w.emit_bit(clocks[2], ((t / 50) & 1) as u8)?;
        }
        if t % 100 == 0 {
            let sample = sine[(t / 100) as usize % SINE_STEPS];
            w.emit_u64(sine_i16, as_bits((sample * i16::MAX as f64).round() as i64, 16))?;
        }
        if t % 500 == 0 {
            w.emit_bit(clocks[3], ((t / 500) & 1) as u8)?;
        }
        if t % 1_000 == 0 {
            let sample = sine[(t / 1_000) as usize % SINE_STEPS];
            w.emit_u64(sine_i8, as_bits((sample * i8::MAX as f64).round() as i64, 8))?;
        }

        if t != 0 && t % 10_000_000 == 0 {
            eprintln!("  waveforms: {t}/{FAST_SAMPLES}");
        }
    }

    eprintln!(
        "writing {} transactions across four generators",
        TRANSACTIONS_PER_GENERATOR * generators.len() as u64
    );
    for i in 0..TRANSACTIONS_PER_GENERATOR {
        let base = i * 10;
        for (lane, generator) in generators.iter().enumerate() {
            let begin = base + lane as u64;
            let tx = w.begin_tx(*generator, begin)?;
            w.end_tx(tx, begin + 3 + lane as u64, TxStatus::Ok)?;
        }
        if i != 0 && i % 1_000_000 == 0 {
            eprintln!("  transactions per generator: {i}/{TRANSACTIONS_PER_GENERATOR}");
        }
    }

    let stats = w.stats();
    let expected_waveform_samples =
        100_000_000 + 20_000_000 + 2_000_000 + 200_000 + 100_000_000 + 10_000_000
            + 1_000_000
            + 100_000;
    let expected_transactions = TRANSACTIONS_PER_GENERATOR * generators.len() as u64;
    assert_eq!(stats.records, expected_waveform_samples);
    assert_eq!(stats.transactions, expected_transactions);
    w.close()?;

    // Header-only checks keep validation bounded even for this intentionally large file.
    let reader = Reader::open(path)?;
    assert_eq!(reader.signal_count(), 8);
    assert_eq!(reader.tx_counts().0, expected_transactions);
    assert_eq!(reader.time_range(), Some((0, FAST_SAMPLES - 1)));
    let bytes = std::fs::metadata(path)?.len();
    eprintln!(
        "wrote {}: {} samples, {} transactions, {:.1} MiB in {:.1}s",
        path.display(),
        expected_waveform_samples,
        expected_transactions,
        bytes as f64 / (1024.0 * 1024.0),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    generate(&output_path())
}
