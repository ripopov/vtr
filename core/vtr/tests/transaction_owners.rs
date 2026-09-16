use vtr::{Reader, TxStatus, Writer, WriterOptions};

#[test]
fn bulk_owners_preserve_order_duplicates_and_missing_ids_across_blocks() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut writer = Writer::create_with(
        file.path(),
        WriterOptions {
            tx_block_bytes: 128,
            ..Default::default()
        },
    )
    .unwrap();
    let stream = writer.add_stream(None, "stream", "raw");
    let a = writer.add_generator(stream, "a");
    let b = writer.add_generator(stream, "b");
    let ids: Vec<_> = (0..32)
        .map(|i| writer.begin_tx(if i % 2 == 0 { a } else { b }, i).unwrap())
        .collect();
    // End in a different order, producing blocks whose ID ranges overlap.
    for (position, &i) in [
        0, 31, 2, 29, 4, 27, 6, 25, 8, 23, 10, 21, 12, 19, 14, 17, 16, 15, 18, 13, 20, 11, 22, 9,
        24, 7, 26, 5, 28, 3, 30, 1,
    ]
    .iter()
    .enumerate()
    {
        writer.end_tx(ids[i], 100, TxStatus::Ok).unwrap();
        if position % 2 == 1 {
            writer.flush().unwrap();
        }
    }
    writer.close().unwrap();
    let reader = Reader::open(file.path()).unwrap();
    assert_eq!(reader.tx_block_count(), 16);
    let requested = [ids[31], ids[0], u64::MAX, ids[2], ids[31], ids[1]];
    assert_eq!(
        reader.transaction_generators(&requested).unwrap(),
        vec![Some(b), Some(a), None, Some(a), Some(b), Some(b)]
    );
    assert!(reader.transaction_generators(&[]).unwrap().is_empty());
    assert_eq!(
        reader.transaction_generators(&[u64::MAX]).unwrap(),
        vec![None]
    );
}
