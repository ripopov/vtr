use vtr::{
    Compression, Direction, ExecutionMode, Reader, Recorder, SignalKind, SourceOptions, VarType,
    Writer, WriterOptions,
};

fn writer(path: &std::path::Path, mode: ExecutionMode, compression: Compression) -> Writer {
    let mut options = WriterOptions {
        mode,
        ..WriterOptions::default()
    };
    options.encoding.compression = compression;
    options.buffer.pending_bytes = 128;
    Writer::create_with(path, options).unwrap()
}

#[test]
fn words_check_input_length_and_preserve_exact_owned_bits_through_spill() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        for compression in [Compression::NONE, Compression::LZ4, Compression::ZSTD_FAST] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("words.vtr");
            let mut writer = writer(&path, mode, compression);
            let signal = writer
                .signal(
                    SourceOptions::new("wide", None),
                    SignalKind::Bits {
                        width: 129,
                        states: 4,
                    },
                    VarType::Wire,
                    Direction::Implicit,
                )
                .unwrap();
            assert!(writer.emit_words(signal, 0, &[]).is_err());
            assert!(writer.emit_words(signal, 129, &[1; 4]).is_err());
            if mode == ExecutionMode::Threaded {
                let mut producer = writer.add_producer().unwrap();
                record(&mut producer, signal);
                producer.finish().unwrap();
                assert!(writer.stats().unwrap().spilled_bytes > 0);
            } else {
                record(&mut writer, signal);
            }
            writer.advance_time(2).unwrap();
            writer.close().unwrap();
            let reader = Reader::open(path).unwrap();
            reader.validate().unwrap();
            let expected = format!("1{:0128b}", 0x1234_5678u128);
            assert_eq!(reader.value_at(signal, 2).unwrap().to_ascii(), expected);
        }
    }
}

fn record(recorder: &mut impl Recorder, signal: vtr::SignalId) {
    recorder.advance_time(1).unwrap();
    let mut words = [0x1234_5678, 0, 0, 0, u32::MAX, u32::MAX];
    recorder.emit_words(signal, 129, &words).unwrap();
    words.fill(0);
}

#[test]
fn mismatched_widths_with_the_same_byte_count_are_rejected_while_disabled_too() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        for disabled in [false, true] {
            for (declared, supplied) in [(8, 1), (64, 63), (129, 130)] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("wrong-width.vtr");
                let mut writer = writer(&path, mode, Compression::LZ4);
                let signal = writer
                    .signal(
                        SourceOptions::new("bits", None),
                        SignalKind::Bits {
                            width: declared,
                            states: 2,
                        },
                        VarType::Wire,
                        Direction::Implicit,
                    )
                    .unwrap();
                writer.advance_time(1).unwrap();
                if disabled {
                    writer.dump(false).unwrap();
                }
                let result = writer.emit_words(signal, supplied, &[1; 5]);
                assert!(result.and_then(|()| writer.flush()).is_err());
                writer.seal().unwrap();
                let reader = Reader::open(path).unwrap();
                reader.validate().unwrap();
                let mut count = 0;
                reader
                    .changes(None, vtr::TimeWindow::default(), |_, _, _| {
                        count += 1;
                        true
                    })
                    .unwrap();
                assert_eq!(count, 0);
            }
        }
    }
}
