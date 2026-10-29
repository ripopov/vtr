use std::path::Path;
use vtr::{
    ExecutionMode, Reader, Recorder, SourceOptions, Status, Tracker, TransactionQuery, Value,
    Writer, WriterOptions,
};

pub fn write(path: &Path, mode: ExecutionMode) -> vtr::Result<()> {
    let mut writer = Writer::create_with(
        path,
        WriterOptions {
            mode,
            ..WriterOptions::default()
        },
    )?;
    let scope = writer.scope("cpu", None)?;
    let pipeline =
        writer.transactions::<()>(SourceOptions::new("pipeline", Some(scope)), "PIPELINE")?;
    let requests = writer.transactions::<()>(SourceOptions::new("lsu", Some(scope)), "LSU")?;
    let mut p = Tracker::new(&mut writer, pipeline, "cpu.pipeline")?;
    let mut l = Tracker::new(&mut writer, requests, "cpu.lsu")?;
    let fe = p.keyspace("fe")?;
    let iid = p.keyspace("iid")?;
    let reg = p.keyspace("reg")?;
    let sq = l.keyspace("sq")?;
    writer.advance_time(10)?;
    assert_eq!(p.open(&mut writer, fe, 1, "ID", 10, None)?, Some(1));
    p.attribute(&mut writer, fe, 1, "pc", Value::U64(0x100))?;
    p.label(&mut writer, fe, 1, "one")?;
    assert_eq!(p.open(&mut writer, fe, 2, "ID", 10, None)?, Some(3));
    assert_eq!(p.open(&mut writer, fe, 3, "ID", 10, None)?, Some(5));
    writer.advance_time(11)?;
    for key in 1..=3 {
        p.stage(&mut writer, fe, key, "IR", 11)?;
    }
    writer.advance_time(12)?;
    assert_eq!(p.bind_oldest(&mut writer, fe, 2, iid, 5, 12)?, 2);
    p.stage(&mut writer, iid, 5, "IQ", 12)?;
    assert_eq!(p.bind_oldest(&mut writer, fe, 1, iid, 6, 12)?, 1);
    p.stage(&mut writer, iid, 6, "IQ", 12)?;
    p.bind(&mut writer, iid, 6, reg, 40, 12)?;
    writer.advance_time(13)?;
    p.stage(&mut writer, reg, 40, "EX", 13)?;
    p.lane(&mut writer, reg, 40, "mem", "DC", 13)?;
    p.open(&mut writer, fe, 4, "ID", 13, None)?;
    p.open(&mut writer, fe, 5, "ID", 13, None)?;
    writer.advance_time(14)?;
    p.lane(&mut writer, reg, 40, "mem", "", 14)?;
    p.stage(&mut writer, iid, 6, "EX", 14)?;
    p.attribute(&mut writer, iid, 5, "pc", Value::U64(0x1000))?;
    p.attribute(&mut writer, iid, 6, "pc", Value::U64(0x2000))?;
    p.label(&mut writer, iid, 5, "fused")?;
    p.event(&mut writer, iid, 5, "replay", 14)?;
    p.event(&mut writer, iid, 5, "replay", 14)?;
    writer.advance_time(15)?;
    assert_eq!(p.close(&mut writer, iid, 5, Status::Ok, 15)?, 2);
    assert_eq!(p.abort_younger(&mut writer, iid, 6, 15)?, 2);
    writer.advance_time(16)?;
    p.stage(&mut writer, fe, 4, "IR", 16)?;
    assert_eq!(p.open(&mut writer, fe, 7, "ID", 16, None)?, Some(23));
    let parent = p.owner(iid, 6);
    assert_eq!(l.open(&mut writer, sq, 0, "Q", 16, parent)?, Some(25));
    assert_eq!(p.relate(&mut writer, "wakeup", (iid, 6), (fe, 7))?, 1);
    p.open(&mut writer, fe, 8, "ID", 16, None)?;
    p.bind(&mut writer, fe, 8, iid, 9, 16)?;
    p.open(&mut writer, fe, 9, "ID", 16, None)?;
    writer.advance_time(17)?;
    p.open(&mut writer, fe, 7, "ID", 17, None)?;
    p.bind(&mut writer, fe, 9, iid, 9, 17)?;
    writer.advance_time(18)?;
    assert_eq!(p.abort_keyspace(&mut writer, fe, 18)?, 2);
    writer.advance_time(19)?;
    assert_eq!(l.close(&mut writer, sq, 0, Status::Ok, 19)?, 1);
    l.open(&mut writer, sq, 1, "Q", 19, None)?;
    l.open(&mut writer, sq, 2, "Q", 19, None)?;
    assert_eq!(l.abort_all(&mut writer, 19)?, 2);
    assert_eq!((p.open_count(), l.open_count()), (1, 0));
    writer.advance_time(20)?;
    p.detach(&mut writer)?;
    l.detach(&mut writer)?;
    assert_eq!(p.open_count(), 0);
    assert_eq!(p.take_diagnostics().len(), 3);
    assert!(p.diagnostics().is_empty());
    writer.close()?;
    read(path)
}

pub fn read(path: &Path) -> vtr::Result<()> {
    let reader = Reader::open(path)?;
    assert!(reader.validate()?.missing.is_empty());
    let mut counts = [0usize; 5];
    reader.transactions(TransactionQuery::default(), |record| {
        counts[record.status() as usize] += 1;
        true
    })?;
    assert_eq!(counts, [0, 3, 0, 8, 1]);
    let attribute = |record: &vtr::Record, name: &str| {
        record
            .attributes()
            .iter()
            .find(|a| reader.string(a.key).unwrap() == name)
            .unwrap()
            .value
            .clone()
    };
    for id in [1, 3] {
        let record = reader.lookup(id)?.unwrap();
        assert_eq!((record.begin(), record.end()), (10, 15));
        assert_eq!(attribute(&record, "pc"), Value::U64(0x1000));
        let Value::Str(label) = attribute(&record, "vtr.label") else {
            panic!("label is not interned");
        };
        assert_eq!(reader.string(label)?, "fused");
        assert_eq!(record.events().len(), 2);
        assert_ne!(record.events()[0].id, record.events()[1].id);
    }
    let tail = reader.lookup(5)?.unwrap();
    assert_eq!(
        (tail.status(), tail.end(), tail.stages().len()),
        (Status::Open, 20, 5)
    );
    assert!(
        tail.stages()
            .iter()
            .any(|s| reader.string(s.lane).unwrap() == "mem" && s.begin == 13 && s.end == Some(14))
    );
    assert!(
        tail.stages()
            .iter()
            .any(|s| reader.string(s.name).unwrap() == "EX" && s.begin == 13 && s.end.is_none())
    );
    assert_eq!(attribute(&tail, "pc"), Value::U64(0x2000));
    assert_eq!(reader.lookup(25)?.unwrap().parent(), Some(5));
    let mut linked = false;
    reader.relations(5, false, |relation| {
        linked |= relation.destination == 23 && reader.string(relation.kind).unwrap() == "wakeup";
        true
    })?;
    assert!(linked);
    assert_eq!(reader.lookup(23)?.unwrap().status(), Status::Aborted);
    Ok(())
}
