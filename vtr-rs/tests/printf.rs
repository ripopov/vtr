use vtr::{
    logging::{self, CallsiteOptions, FormatStyle},
    *,
};
struct Args {
    width: i32,
    precision: i32,
    number: f64,
    text: String,
    integer: i8,
}
vtr::impl_payload!(Args {
    width,
    precision,
    number,
    text,
    integer
});
#[test]
fn printf_dispatch_dynamic_fields_types_and_limits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("printf.vtr");
    let mut w = Writer::create(&path).unwrap();
    let logger = w.logger(SourceOptions::new("logs", None)).unwrap();
    let cases = [
        ("[%3$*1$.*2$f] %4$.3s %5$x", "[-1.250      ] hé ff"),
        ("%1$d %d", "{?}"),
        ("%4$d/%5$n/%3$d", "{?}/{?}/{?}"),
        ("%5$hhu/%5$hhd", "255/-1"),
        ("%3$65537f", "{?}"),
    ];
    let mut ids = Vec::new();
    for (format, _) in cases {
        let site = logging::declare::<Args>(
            &mut w,
            CallsiteOptions {
                style: FormatStyle::Printf,
                ..CallsiteOptions::new(format)
            },
        )
        .unwrap();
        ids.push(
            w.log(
                logger,
                site,
                &Args {
                    width: -12,
                    precision: 3,
                    number: -1.25,
                    text: "héllo".into(),
                    integer: -1,
                },
                None,
            )
            .unwrap(),
        );
    }
    w.close().unwrap();
    let r = Reader::open(&path).unwrap();
    let foreign_path = dir.path().join("foreign.vtr");
    Writer::create(&foreign_path).unwrap().close().unwrap();
    let foreign = Reader::open(&foreign_path).unwrap();
    for (id, (format, expected)) in ids.into_iter().zip(cases) {
        let record = r.lookup(id).unwrap().unwrap();
        assert_eq!(
            logging::format_log(&r, &record).unwrap(),
            expected,
            "{format}"
        );
        let payload = record.payload().field("value").unwrap();
        assert!(foreign.format_printf_arguments(format, &payload).is_err());
    }
}

#[test]
fn printf_metadata_and_output_limits_are_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("limits.vtr");
    let mut writer = Writer::create(&path).unwrap();
    let logger = writer.logger(SourceOptions::new("logs", None)).unwrap();
    let site = logging::declare::<u32>(
        &mut writer,
        CallsiteOptions {
            style: FormatStyle::Printf,
            ..CallsiteOptions::new("%1$200d%1$200d")
        },
    )
    .unwrap();
    let id = writer.log(logger, site, &42, None).unwrap();
    let mut invalid = Vec::new();
    for style in [Value::Text("unknown".into()), Value::U32(1)] {
        let mut options = DatatypeOptions {
            name: "invalid".into(),
            attributes: Vec::new(),
        };
        for (key, value) in [
            ("kind", Value::Text("callsite".into())),
            ("log.format", Value::Text("%d".into())),
            ("log.format_style", style),
            (
                "log.severity",
                Value::Enum {
                    value: 2,
                    name: writer.intern("info").unwrap(),
                },
            ),
        ] {
            options.attributes.push(Attribute {
                key: writer.intern(key).unwrap(),
                value,
            });
        }
        let site = writer.datatype::<u32>(options).unwrap();
        invalid.push(writer.log(logger, site, &1, None).unwrap());
    }
    writer.close().unwrap();
    let reader = Reader::open_with(
        &path,
        ReadOptions {
            max_payload_bytes: 256,
            ..Default::default()
        },
    )
    .unwrap();
    let record = reader.lookup(id).unwrap().unwrap();
    assert!(logging::format_log_bytes(&reader, &record).is_err());
    assert!(
        reader
            .format_printf_arguments_bytes(
                &"x".repeat(257),
                &record.payload().field("value").unwrap()
            )
            .is_err()
    );
    for id in invalid {
        assert!(logging::format_log(&reader, &reader.lookup(id).unwrap().unwrap()).is_err());
    }
}
