//! FST conversion fidelity and malformed-input handling through the native VTR reader.

use std::collections::BTreeMap;
use std::path::Path;
use vtr::{
    OwnedSignalValue, SignalKind,
    declarations::NodeData,
    recording::{ExecutionMode, Writer, WriterOptions},
    trace::{Reader, TimeWindow},
};

fn converted(bytes: &[u8]) -> anyhow::Result<Reader> {
    let dir = tempfile::tempdir()?;
    let input = dir.path().join("input.fst");
    let output = dir.path().join("output.vtr");
    std::fs::write(&input, bytes)?;
    let mut writer = Writer::create_with(
        &output,
        WriterOptions {
            mode: ExecutionMode::Inline,
            encoding: vtr::recording::EncodingOptions {
                waveform_deduplicate: false,
                ..Default::default()
            },
            ..Default::default()
        },
    )?;
    vtr_convert::fst::convert_fst(&input, &mut writer, &Default::default())?;
    writer.close()?;
    Ok(Reader::from_bytes(std::fs::read(output)?)?)
}

#[test]
fn every_sample_matches_the_fst_decoder() {
    use fst_reader::{FstFilter, FstHierarchyEntry, FstReader, FstSignalValue};

    for name in [
        "values.fst",
        "values-wrapped.fst",
        "fst_types.fst",
        "zlib.fst",
        "landing_dram.fst",
    ] {
        let bytes = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name),
        )
        .unwrap();
        let converted = converted(&bytes).unwrap();
        let mut original = FstReader::open(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(
            converted.metadata().timescale,
            original.get_header().timescale_exponent
        );
        assert_eq!(
            converted.waveform_time_range().unwrap().last,
            original.get_header().end_time
        );
        converted.validate().unwrap();
        let variables: Vec<_> = converted
            .declarations()
            .nodes
            .iter()
            .filter_map(|node| {
                if let NodeData::Signal { signal, .. } = node.data {
                    Some((node, signal))
                } else {
                    None
                }
            })
            .collect();
        let mut declarations = Vec::new();
        original
            .read_hierarchy(|entry| {
                if let FstHierarchyEntry::Var { name, handle, .. } = entry {
                    declarations.push((name, handle.get_index()));
                }
            })
            .unwrap();
        assert_eq!(variables.len(), declarations.len());
        let mut histories = BTreeMap::new();
        for (&(node, signal), (name, handle)) in variables.iter().zip(&declarations) {
            assert_eq!(converted.string(node.name).unwrap(), name);
            if let Some((other, _, _)) = histories.get(handle) {
                assert_eq!(signal, *other, "alias {name}");
            } else {
                histories.insert(
                    *handle,
                    (
                        signal,
                        {
                            let mut samples = Vec::new();
                            converted
                                .changes(Some(signal), TimeWindow::default(), |time, _, value| {
                                    samples.push((time, value.to_owned()));
                                    true
                                })
                                .unwrap();
                            samples
                        },
                        0usize,
                    ),
                );
            }
        }
        original
            .read_signals(
                &FstFilter::all(),
                |time, handle, value| -> anyhow::Result<()> {
                    let (signal, history, offset) = histories.get_mut(&handle.get_index()).unwrap();
                    let (actual_time, actual) = &history[*offset];
                    assert_eq!(*actual_time, time, "{name}: {handle:?}");
                    match value {
                        FstSignalValue::Real(expected) => {
                            let OwnedSignalValue::Real(actual) = actual else {
                                panic!("expected real")
                            };
                            assert_eq!(actual.to_bits(), expected.to_bits());
                        }
                        FstSignalValue::String(bytes) => {
                            if converted.signal_kind(*signal)? == SignalKind::VarLen {
                                assert_eq!(actual, &OwnedSignalValue::VarLen(bytes.to_vec()));
                            } else {
                                assert_eq!(
                                    actual.to_ascii(),
                                    std::str::from_utf8(bytes)?.to_ascii_lowercase()
                                );
                            }
                        }
                    }
                    *offset += 1;
                    Ok(())
                },
            )
            .unwrap();
        for (_, history, count) in histories.into_values() {
            assert_eq!(history.len(), count, "{name}: sample count");
        }
    }
}

#[test]
fn malformed_and_truncated_images_return_errors() {
    for bytes in [vec![], b"not a trace".to_vec(), vec![0; 9]] {
        assert!(converted(&bytes).is_err());
    }
    let original = include_bytes!("fixtures/values.fst");
    for end in [9, 100, 329, original.len() - 1] {
        assert!(converted(&original[..end]).is_err(), "prefix {end}");
    }
    for size in [0u64, 7, u64::MAX] {
        let mut bytes = original.to_vec();
        bytes[331..339].copy_from_slice(&size.to_be_bytes());
        assert!(converted(&bytes).is_err());
    }
}
