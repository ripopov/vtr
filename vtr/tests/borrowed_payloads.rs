use std::borrow::Cow;
use vtr::schema::Bytes;

use vtr::{ExecutionMode, Reader, Recorder, SourceOptions, Writer, WriterOptions};

#[test]
fn borrowed_captures_own_data_before_return_and_decode_after_reader_destruction() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("borrowed.vtr");
        let mut writer = Writer::create_with(
            &path,
            WriterOptions {
                mode,
                ..WriterOptions::default()
            },
        )
        .unwrap();
        let text_source = writer
            .logs::<Cow<'static, str>>(SourceOptions::new("text {}", None))
            .unwrap();
        let words_source = writer
            .logs::<Cow<'static, [u64]>>(SourceOptions::new("words {}", None))
            .unwrap();
        let mut text = String::from("borrowed Δ");
        let mut words = vec![0, u64::MAX, 42];
        let bytes_source = writer
            .logs::<Bytes<Cow<'static, [u8]>>>(SourceOptions::new("bytes {}", None))
            .unwrap();
        let mut bytes = vec![0, 255, 128];
        let text_id = writer
            .log(text_source, &Cow::Borrowed(text.as_str()), None)
            .unwrap();
        let words_id = writer
            .log(words_source, &Cow::Borrowed(words.as_slice()), None)
            .unwrap();
        let bytes_id = writer
            .log(bytes_source, &Bytes(Cow::Borrowed(bytes.as_slice())), None)
            .unwrap();
        text.clear();
        words.fill(7);
        bytes.clear();
        writer.close().unwrap();
        let reader = Reader::open(path).unwrap();
        reader.validate().unwrap();
        let text = reader.lookup(text_id).unwrap().unwrap().payload();
        let words = reader.lookup(words_id).unwrap().unwrap().payload();
        let bytes = reader.lookup(bytes_id).unwrap().unwrap().payload();
        drop(reader);
        assert_eq!(text.decode::<String>().unwrap(), "borrowed Δ");
        assert_eq!(text.decode::<Cow<'_, str>>().unwrap(), "borrowed Δ");
        assert_eq!(words.decode::<Vec<u64>>().unwrap(), [0, u64::MAX, 42]);
        assert_eq!(
            words.decode::<Cow<'_, [u64]>>().unwrap().as_ref(),
            [0, u64::MAX, 42]
        );
        assert!(words.decode::<Vec<u32>>().is_err());
        assert_eq!(bytes.decode::<Bytes>().unwrap().0, [0, 255, 128]);
        assert_eq!(
            bytes.decode::<Bytes<Cow<'_, [u8]>>>().unwrap().0.as_ref(),
            [0, 255, 128]
        );
        assert!(bytes.decode::<Vec<u8>>().is_err());
    }
}
