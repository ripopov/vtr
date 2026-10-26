use vtr::*;

#[test]
fn truncated_and_corrupt_traces_retain_only_published_verified_sections() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("full.vtr");
    let mut options = WriterOptions {
        mode: ExecutionMode::Inline,
        ..WriterOptions::default()
    };
    options.encoding.waveform_records = 10;
    options.encoding.block_records = 5;
    let mut writer = Writer::create_with(&path, options).unwrap();
    let logger = writer
        .logger(SourceOptions::new("diagnostics", None))
        .unwrap();
    let signal = writer
        .signal(
            SourceOptions::new("signal", None),
            SignalKind::Bits {
                width: 16,
                states: 2,
            },
            VarType::Wire,
            Direction::Output,
        )
        .unwrap();
    let source = writer
        .callsite::<String>(vtr::CallsiteOptions::new("step {}"))
        .unwrap();
    let retry = writer
        .callsite::<u64>(CallsiteOptions::new("retry {}"))
        .unwrap();
    for time in 0..50 {
        writer.advance_time(time).unwrap();
        writer.emit_u64(signal, time).unwrap();
        writer
            .log(logger, source, &format!("{time}"), None)
            .unwrap();
        writer.log(logger, retry, &time, None).unwrap();
        if time % 10 == 0 {
            writer.scope(format!("scope{time}"), None).unwrap();
            writer.flush().unwrap();
        }
    }
    writer.close().unwrap();
    let full = std::fs::read(path).unwrap();
    let reader = Reader::from_bytes(full.clone()).unwrap();
    let sections = reader.sections().to_vec();
    let metadata = sections.iter().find(|section| section.kind == 1).unwrap();
    let first_end = (metadata.payload_offset() + metadata.length) as usize;
    let mut cuts = Vec::new();
    for section in &sections {
        let start = section.offset as usize;
        let payload = section.payload_offset() as usize;
        let end = payload + section.length as usize;
        cuts.extend([
            start,
            start + 1,
            payload,
            payload + section.length as usize / 2,
            end,
        ]);
    }
    cuts.extend([full.len() - 1, full.len() - 16, full.len() - 80]);
    cuts.sort_unstable();
    cuts.dedup();
    for cut in cuts.into_iter().filter(|cut| *cut >= first_end) {
        let truncated = Reader::from_bytes(full[..cut].to_vec()).unwrap();
        assert_eq!(truncated.completion(), Completion::Unfinished);
        assert_eq!(truncated.identity(), reader.identity());
        truncated.validate().unwrap();
        truncated
            .records(RecordQuery::default(), |record| {
                if record.callsite() == Some(source.id()) {
                    assert_eq!(
                        record.decode::<String>().unwrap(),
                        record.begin().to_string()
                    );
                } else {
                    assert_eq!(record.callsite(), Some(retry.id()));
                    assert_eq!(record.decode::<u64>().unwrap(), record.begin());
                }
                true
            })
            .unwrap();
        if truncated.signal_count() != 0 {
            truncated
                .changes(Some(signal), TimeWindow::default(), |time, _, value| {
                    assert_eq!(value.as_u64(), Some(time));
                    true
                })
                .unwrap();
        }
        let input = directory.path().join("cut.vtr");
        let output = directory.path().join("recovered.vtr");
        std::fs::write(&input, &full[..cut]).unwrap();
        recover(&input, &output).unwrap();
        let recovered = Reader::open(&output).unwrap();
        assert_eq!(recovered.completion(), Completion::Incomplete);
        assert_eq!(recovered.identity(), truncated.identity());
        assert_eq!(recovered.sections().len(), truncated.sections().len());
        recovered.validate().unwrap();
    }
    for section in sections
        .iter()
        .filter(|section| section.kind == 4 || section.kind == 9)
    {
        let mut corrupt = full[..(section.payload_offset() + section.length) as usize].to_vec();
        corrupt[section.payload_offset() as usize] ^= 1;
        let recovered = Reader::from_bytes(corrupt).unwrap();
        assert!(
            recovered
                .sections()
                .iter()
                .all(|valid| valid.offset < section.offset)
        );
        recovered.validate().unwrap();
    }
    let mut zeros = full[..sections.last().unwrap().offset as usize].to_vec();
    zeros.resize(zeros.len() + 4096, 0);
    let recovered = Reader::from_bytes(zeros).unwrap();
    assert_eq!(recovered.completion(), Completion::Unfinished);
    recovered.validate().unwrap();
}
