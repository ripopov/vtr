use std::io::Write;
use std::sync::{Arc, Mutex};
use vtr_progress::Phase;

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), Default::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

#[test]
fn memory_output_matches_native_conversion_and_reports_final_size() {
    for (name, bytes) in [
        (
            "pipeline.fst",
            include_bytes!("../../vtr-convert/tests/fixtures/pipeline.fst").as_slice(),
        ),
        (
            "pipeline.kanata",
            include_bytes!("../../vtr-convert/tests/fixtures/pipeline.kanata").as_slice(),
        ),
        (
            "waves.vcd",
            include_bytes!("../../vtr-convert/tests/fixtures/waves.vcd").as_slice(),
        ),
    ] {
        let snapshots = Arc::new(Mutex::new(Vec::new()));
        let observed = snapshots.clone();
        let output = vtr_convert_wasm::convert_to_vtr(name, bytes.to_vec(), move |snapshot| {
            observed.lock().unwrap().push(snapshot);
        })
        .unwrap();
        let reader = vtr::trace::Reader::from_bytes(output.clone()).unwrap();
        let report = reader.validate().unwrap();
        assert!(reader.signal_count() > 0 || report.records > 0);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("native.vtr");
        let mut writer = vtr::recording::Writer::create_with(
            &path,
            vtr::recording::WriterOptions {
                mode: vtr::recording::ExecutionMode::Inline,
                encoding: vtr::recording::EncodingOptions {
                    waveform_deduplicate: false,
                    compression: vtr::Compression::LZ4,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        let input = std::io::Cursor::new(bytes.to_vec());
        match name.rsplit_once('.').unwrap().1 {
            "fst" => vtr_convert::fst::convert_input(input, &mut writer, &Default::default()),
            "kanata" => vtr_convert::kanata::convert_input(
                input,
                bytes.len() as u64,
                &mut writer,
                &Default::default(),
            ),
            _ => vtr_convert::vcd::convert_input(
                input,
                bytes.len() as u64,
                &mut writer,
                &Default::default(),
            ),
        }
        .unwrap();
        writer.close().unwrap();
        let native = std::fs::read(path).unwrap();
        assert_ne!(&native[16..32], &output[16..32]);
        assert_same_observations(&native, &output);
        let snapshots = snapshots.lock().unwrap();
        assert!(snapshots.iter().all(vtr_progress::Snapshot::valid));
        assert!(snapshots.iter().any(|s| s.phase == Phase::Reading));
        assert!(snapshots.iter().any(|s| s.phase == Phase::Finalizing));
        let last = snapshots.last().unwrap();
        assert_eq!(last.phase, Phase::Complete);
        assert_eq!(last.input_bytes, bytes.len() as u64);
        assert_eq!(last.output_bytes, output.len() as u64);

        let compressed =
            vtr_convert_wasm::convert_to_vtr(&format!("{name}.gz"), gzip(bytes), |_| {}).unwrap();
        assert_same_observations(&compressed, &output);
    }
}

#[test]
fn failed_conversions_never_return_partial_vtr_images() {
    for (name, bytes) in [
        ("bad.fst", b"invalid".as_slice()),
        (
            "bad.kanata",
            b"Kanata\t0004\nI\t0\t0\t0\nC=\t8\nC=\t7\n".as_slice(),
        ),
        (
            "bad.vcd",
            b"$var wire 1 ! a $end\n$enddefinitions $end\n1?\n".as_slice(),
        ),
        ("bad.log.gz", &[0x1f, 0x8b, 0]),
        ("unknown.bin", b"unknown".as_slice()),
    ] {
        let phases = Arc::new(Mutex::new(Vec::new()));
        let observed = phases.clone();
        assert!(
            vtr_convert_wasm::convert_to_vtr(name, bytes.to_vec(), move |s| {
                observed.lock().unwrap().push(s.phase);
            })
            .is_err()
        );
        assert_eq!(phases.lock().unwrap().last(), Some(&Phase::Failed));
        assert!(!phases.lock().unwrap().contains(&Phase::Complete));
    }
}

fn assert_same_observations(left: &[u8], right: &[u8]) {
    assert_eq!(&left[..16], &right[..16]);
    assert_eq!(&left[32..], &right[32..]);
}
