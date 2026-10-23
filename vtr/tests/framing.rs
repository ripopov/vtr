use vtr::framing::{Completion, Container, FramedWriter, RecordingId, recover};

#[test]
fn cpp_container_round_trips_byte_exact_and_checks_every_section() {
    let bytes = include_bytes!("fixtures/typed.vtr");
    let container = Container::from_bytes(bytes.as_slice(), true).unwrap();
    assert_eq!(container.completion, Completion::Complete);
    let mut writer = FramedWriter::with_identity(Vec::new(), container.identity).unwrap();
    for section in container.sections() {
        writer
            .section(
                section.kind,
                section.flags,
                container.payload(section, true).unwrap(),
                section.aux0,
                section.aux1,
                section.block_number,
            )
            .unwrap();
    }
    assert_eq!(
        writer.finish(false, 0).unwrap(),
        container.fingerprint.unwrap()
    );
    assert_eq!(writer.into_inner(), bytes);
    for section in container.sections() {
        if section.length == 0 {
            continue;
        }
        let mut corrupt = bytes.to_vec();
        corrupt[section.payload_offset() as usize] ^= 1;
        let container = Container::from_bytes(corrupt, true).unwrap();
        assert!(container.payload(section, false).is_err());
    }
}

#[test]
fn truncation_recovers_only_verified_prefix_sections() {
    let bytes = include_bytes!("fixtures/typed.vtr");
    let complete = Container::from_bytes(bytes.as_slice(), true).unwrap();
    for end in 32..bytes.len() {
        let container = Container::from_bytes(&bytes[..end], true).unwrap();
        assert_eq!(container.completion, Completion::Unfinished);
        assert!(container.fingerprint.is_none());
        for section in container.sections() {
            assert!(section.payload_offset() + section.length <= end as u64);
            container.payload(section, true).unwrap();
        }
        assert_eq!(
            container.sections().len(),
            complete
                .sections()
                .iter()
                .filter(|section| section.payload_offset() + section.length <= end as u64)
                .count()
        );
    }
    let mut bytes = bytes.to_vec();
    let section = complete.sections().last().unwrap();
    bytes[section.payload_offset() as usize] ^= 1;
    bytes.truncate(section.payload_offset() as usize + section.length as usize);
    let prefix = Container::from_bytes(bytes, true).unwrap();
    assert_eq!(prefix.sections().len() + 1, complete.sections().len());
}

#[test]
fn recovery_preserves_identity_and_invalidates_the_old_fingerprint() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.vtr");
    let output = directory.path().join("output.vtr");
    let bytes = include_bytes!("fixtures/typed.vtr");
    std::fs::write(&input, bytes).unwrap();
    assert!(recover(&input, &input).is_err());
    std::fs::hard_link(&input, &output).unwrap();
    assert!(recover(&input, &output).is_err());
    std::fs::remove_file(&output).unwrap();
    recover(&input, &output).unwrap();
    let original = Container::open(&input, true).unwrap();
    let recovered = Container::open(&output, true).unwrap();
    assert_eq!(original.identity, recovered.identity);
    assert_ne!(original.fingerprint, recovered.fingerprint);
    assert_eq!(recovered.completion, Completion::Incomplete);
    assert_eq!(original.sections(), recovered.sections());
    for section in recovered.sections() {
        assert_eq!(
            recovered.payload(section, true).unwrap(),
            original.payload(section, true).unwrap()
        );
    }
}

#[test]
fn invalid_headers_and_final_directories_are_rejected() {
    let bytes = include_bytes!("fixtures/typed.vtr");
    for offset in [0, 8, 10, 12] {
        let mut corrupt = bytes.to_vec();
        corrupt[offset] ^= 1;
        assert!(Container::from_bytes(corrupt, false).is_err());
    }
    let mut corrupt = bytes.to_vec();
    corrupt[16..32].fill(0);
    assert!(Container::from_bytes(corrupt, false).is_err());
    let offset = u64::from_le_bytes(
        bytes[bytes.len() - 24..bytes.len() - 16]
            .try_into()
            .unwrap(),
    ) as usize;
    let mut corrupt = bytes.to_vec();
    corrupt[offset + 24] ^= 1;
    assert!(Container::from_bytes(corrupt, false).is_err());
    assert!(FramedWriter::with_identity(Vec::new(), RecordingId([0; 16])).is_err());
    let mut writer = FramedWriter::new(Vec::new()).unwrap();
    assert!(writer.section(5, 0, &[], 0, 0, 0).is_err());
    writer.section(1, 0, &[1], 0, 0, 0).unwrap();
    writer.finish(true, 7).unwrap();
    assert!(writer.finish(true, 7).is_err());
    assert!(writer.section(1, 0, &[], 0, 0, 0).is_err());
    let parsed = Container::from_bytes(writer.into_inner(), true).unwrap();
    assert_eq!(parsed.completion, Completion::Incomplete);
    assert_eq!(parsed.dropped, 7);
}

#[test]
fn recovery_stops_at_corruption_even_with_a_valid_final_directory() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("corrupt.vtr");
    let output = directory.path().join("recovered.vtr");
    let bytes = include_bytes!("fixtures/typed.vtr");
    let original = Container::from_bytes(bytes.as_slice(), true).unwrap();
    let record = original
        .sections()
        .iter()
        .find(|section| section.kind == 5)
        .unwrap();
    let mut corrupt = bytes.to_vec();
    corrupt[record.payload_offset() as usize] ^= 1;
    std::fs::write(&input, &corrupt).unwrap();
    recover(&input, &output).unwrap();
    let recovered = Container::open(&output, true).unwrap();
    assert_eq!(recovered.completion, Completion::Incomplete);
    assert_eq!(recovered.identity, original.identity);
    assert_eq!(recovered.dropped, bytes.len() as u64 - record.offset);
    assert!(
        recovered
            .sections()
            .iter()
            .all(|section| section.offset < record.offset)
    );
}
