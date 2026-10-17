use vtr::{
    Direction, FileType, Logic, LogicStates, NodeData, Reader, SignalKind, Timescale, Value,
    VarType, Writer, WriterOptions,
};

#[test]
fn builder_metadata_and_helpers_round_trip_with_dynamic_declarations() -> vtr::Result<()> {
    let dir = tempfile::tempdir()?;
    for background in [false, true] {
        let path = dir.path().join(format!("builder-{background}.vtr"));
        let mut writer = Writer::builder()
            .options(WriterOptions {
                background,
                group_size: 3,
                ..Default::default()
            })
            .timescale(Timescale::Exponent(-8))
            .time_zero(-10)
            .file_type(FileType::SystemC)
            .writer_name("test simulator")
            .date("2026-10-04")
            .comment("Metadata round trip")
            .file_attr("seed", Value::U64(42))
            .create(&path)?;
        let top = writer.add_module(None, "top", "counter")?;
        let valid = writer.add_wire(Some(top), "valid", 1, Direction::Input)?;
        writer.set_time(0)?;
        writer.emit_bit(valid, Logic::Zero)?;
        writer.flush()?;
        assert!(writer.set_timescale(Timescale::Picoseconds).is_err());
        let count = writer.add_reg(Some(top), "count", 8, Direction::Output)?;
        writer.set_time(10)?;
        writer.emit_bit(valid, Logic::One)?;
        writer.emit_u64(count, 42)?;
        writer.close()?;

        let reader = Reader::open(&path)?;
        let meta = reader.meta();
        assert_eq!(meta.timescale, -8);
        assert_eq!(meta.time_zero, -10);
        assert_eq!(meta.file_type, FileType::SystemC);
        assert_eq!(meta.writer, "test simulator");
        assert_eq!(meta.date, "2026-10-04");
        assert_eq!(meta.comment, "Metadata round trip");
        assert_eq!(meta.group_size, 4);
        assert_eq!(meta.attrs.len(), 1);
        assert_eq!(reader.strings().get(meta.attrs[0].0), "seed");
        assert_eq!(meta.attrs[0].1, Value::U64(42));
        for (name, var_type, direction, width) in [
            ("valid", VarType::Wire, Direction::Input, 1),
            ("count", VarType::Reg, Direction::Output, 8),
        ] {
            let node = reader.find_node(&["top", name]).unwrap();
            match reader.hierarchy().node_data(node) {
                NodeData::Var {
                    var_type: actual_type,
                    direction: actual_direction,
                    declares,
                    ..
                } => {
                    assert_eq!(actual_type, var_type);
                    assert_eq!(actual_direction, direction);
                    assert_eq!(declares, Some(SignalKind::bits(width, LogicStates::Four)));
                }
                _ => panic!("expected variable"),
            }
        }
        assert_eq!(reader.value_at(count, 10)?.borrow().as_u64(), Some(42));
    }
    Ok(())
}

#[test]
fn named_logic_respects_signal_alphabets() -> vtr::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("logic.vtr");
    let mut writer = Writer::create(&path)?;
    let mut signals = Vec::new();
    let values = [
        Logic::Zero,
        Logic::One,
        Logic::X,
        Logic::Z,
        Logic::U,
        Logic::W,
        Logic::L,
        Logic::H,
        Logic::DontCare,
    ];
    for states in [LogicStates::Two, LogicStates::Four, LogicStates::Nine] {
        let (_, signal) = writer.add_var(
            None,
            &format!("logic{}", states as u8),
            VarType::Wire,
            Direction::Implicit,
            SignalKind::bits(1, states),
        )?;
        signals.push((signal, states));
    }
    let wide = writer.add_wire(None, "wide", 8, Direction::Implicit)?;
    assert!(writer.emit_bit(wide, Logic::X).is_err());
    for (time, value) in values.into_iter().enumerate() {
        writer.set_time(time as u64)?;
        for &(signal, states) in &signals {
            let result = writer.emit_bit(signal, value);
            if (value as u8) < states as u8 {
                result?;
            } else {
                assert!(result.is_err());
            }
        }
    }
    writer.close()?;
    let reader = Reader::open(&path)?;
    for (signal, states) in signals {
        for (time, ascii) in b"01xzuwlh-".iter().take(states as usize).enumerate() {
            assert_eq!(
                reader.value_at(signal, time as u64)?.to_ascii(),
                (*ascii as char).to_string()
            );
        }
    }
    assert_eq!(Logic::from(false), Logic::Zero);
    assert_eq!(Logic::from(true), Logic::One);
    Ok(())
}

#[test]
fn named_timescales_round_trip() -> vtr::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("timescale.vtr");
    for (scale, exponent) in [
        (Timescale::Seconds, 0),
        (Timescale::Milliseconds, -3),
        (Timescale::Microseconds, -6),
        (Timescale::Nanoseconds, -9),
        (Timescale::Picoseconds, -12),
        (Timescale::Femtoseconds, -15),
        (Timescale::Exponent(-7), -7),
    ] {
        let mut writer = Writer::create(&path)?;
        writer.set_timescale(scale)?;
        writer.close()?;
        assert_eq!(Reader::open(&path)?.meta().timescale, exponent);
    }
    Ok(())
}

#[test]
fn default_timescale_is_picoseconds() -> vtr::Result<()> {
    assert_eq!(Timescale::default(), Timescale::Picoseconds);
    assert_eq!(vtr::Meta::default().timescale, -12);
    let dir = tempfile::tempdir()?;
    for builder in [false, true] {
        let path = dir.path().join(format!("default-{builder}.vtr"));
        let mut writer = if builder {
            Writer::builder().create(&path)?
        } else {
            Writer::create(&path)?
        };
        writer.close()?;
        assert_eq!(Reader::open(&path)?.meta().timescale, -12);
    }
    Ok(())
}
