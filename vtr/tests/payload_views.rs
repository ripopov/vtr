use vtr::{
    ExecutionMode, PayloadView, Reader, Recorder, SourceOptions, Status, Writer, WriterOptions,
};

#[derive(Debug, PartialEq)]
struct Collection {
    name: String,
    rows: Vec<Vec<String>>,
    words: [u16; 3],
    units: [(); 4],
}
vtr::impl_payload!(Collection {
    name,
    rows,
    words,
    units
});

fn children(payload: &PayloadView) -> Vec<PayloadView> {
    payload
        .children()
        .unwrap()
        .collect::<vtr::Result<_>>()
        .unwrap()
}

#[test]
fn sequential_children_preserve_nested_variable_empty_and_zero_size_values_after_reader_drop() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("collections.vtr");
    let mut writer = Writer::create_with(
        &path,
        WriterOptions {
            mode: ExecutionMode::Inline,
            ..Default::default()
        },
    )
    .unwrap();
    let source = writer
        .transactions::<Collection>(SourceOptions::new("collections", None), "OWNER")
        .unwrap();
    let rows = vec![vec![], vec!["λ".into(), String::new(), "日本語".into()]];
    let value = Collection {
        name: "packet".into(),
        rows: rows.clone(),
        words: [0, 17, u16::MAX],
        units: [(); 4],
    };
    let tx = writer.begin(source, &value, None).unwrap();
    writer.end(tx, Status::Ok).unwrap();
    writer.close().unwrap();
    let reader = Reader::open(&path).unwrap();
    let payload = reader.lookup(tx.id()).unwrap().unwrap().payload();
    drop(reader);
    let mut fields = payload.children().unwrap();
    assert_eq!(fields.len(), 4);
    let name = fields.next().unwrap().unwrap();
    assert_eq!(fields.len(), 3);
    assert_eq!(name.decode::<String>().unwrap(), value.name);
    assert!(name.children().is_err());
    let nested = fields.next().unwrap().unwrap();
    let words = fields.next().unwrap().unwrap();
    let units = fields.next().unwrap().unwrap();
    assert!(fields.next().is_none());
    assert_eq!(fields.len(), 0);
    assert!(fields.next().is_none());
    drop(fields);
    drop(payload);
    for (row, expected) in children(&nested).iter().zip(rows) {
        let actual: Vec<_> = children(row)
            .iter()
            .map(|text| text.decode::<String>().unwrap())
            .collect();
        assert_eq!(actual, expected);
    }
    assert_eq!(
        children(&words)
            .iter()
            .map(|word| word.decode::<u16>().unwrap())
            .collect::<Vec<_>>(),
        value.words
    );
    assert_eq!(children(&units).len(), 4);
    assert!(
        children(&units)
            .iter()
            .all(|unit| unit.decode::<()>().is_ok() && unit.bytes().is_empty())
    );
}
