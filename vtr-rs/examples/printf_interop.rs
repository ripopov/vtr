//! Cross-language printf fixture generation and byte-exact verification.
use std::{fmt::Write as _, path::Path};
use vtr::{
    Reader, Recorder, SourceOptions, Writer,
    logging::{self, CallsiteOptions, FormatStyle},
};

struct Arguments {
    width: i32,
    precision: i32,
    number: f64,
    text: String,
    integer: i8,
}
vtr::impl_payload!(Arguments {
    width,
    precision,
    number,
    text,
    integer
});

fn write(path: &Path, threaded: bool) -> vtr::Result<()> {
    let mut writer = Writer::create_with(
        path,
        vtr::WriterOptions {
            mode: if threaded {
                vtr::ExecutionMode::Threaded
            } else {
                vtr::ExecutionMode::Inline
            },
            ..Default::default()
        },
    )?;
    let logger = writer.logger(SourceOptions::new("printf", None))?;
    let mut expected = String::new();
    for (format, message) in [
        ("value=%#018x", "value=0xffffffffffffffff"),
        ("%1$llu/%1$X", "18446744073709551615/FFFFFFFFFFFFFFFF"),
        ("%n %u", "{?} {?}"),
    ] {
        let site = logging::declare::<u64>(
            &mut writer,
            CallsiteOptions {
                style: FormatStyle::Printf,
                ..CallsiteOptions::new(format)
            },
        )?;
        let id = writer.log(logger, site, &u64::MAX, None)?;
        write!(expected, "{id} ").unwrap();
        for byte in message.bytes() {
            write!(expected, "{byte:02x}").unwrap();
        }
        expected.push('\n');
    }
    let site = logging::declare::<Arguments>(
        &mut writer,
        CallsiteOptions {
            style: FormatStyle::Printf,
            ..CallsiteOptions::new("[%3$*1$.*2$f] %4$.3s %5$x")
        },
    )?;
    let id = writer.log(
        logger,
        site,
        &Arguments {
            width: -12,
            precision: 3,
            number: -1.25,
            text: "héllo".into(),
            integer: -1,
        },
        None,
    )?;
    write!(expected, "{id} ").unwrap();
    for byte in "[-1.250      ] hé ff".bytes() {
        write!(expected, "{byte:02x}").unwrap();
    }
    expected.push('\n');
    writer.close()?;
    std::fs::write(format!("{}.expected", path.display()), expected)?;
    Ok(())
}
fn read(path: &Path) -> vtr::Result<()> {
    let reader = Reader::open(path)?;
    reader.validate()?;
    for line in std::fs::read_to_string(format!("{}.expected", path.display()))?.lines() {
        let (id, hex) = line.split_once(' ').unwrap();
        let expected: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let record = reader.lookup(id.parse().unwrap())?.unwrap();
        let actual = logging::format_log_bytes(&reader, &record)?;
        let metadata = logging::metadata(&reader, record.datatype())?.unwrap();
        assert_eq!(actual, expected, "id={id}, format={:?}", metadata.format);
        assert_eq!(
            actual,
            reader.format_printf_arguments_bytes(
                &metadata.format,
                &record.payload().field("value")?
            )?
        );
    }
    Ok(())
}
fn main() -> vtr::Result<()> {
    unsafe extern "C" {
        fn setlocale(
            category: std::ffi::c_int,
            locale: *const std::ffi::c_char,
        ) -> *mut std::ffi::c_char;
    }
    #[cfg(target_os = "macos")]
    const LC_CTYPE: i32 = 2;
    #[cfg(not(target_os = "macos"))]
    const LC_CTYPE: i32 = 0;
    // SAFETY: This single-threaded fixture verifier initializes the C character
    // locale before creating readers or workers; both strings are NUL terminated.
    unsafe {
        if setlocale(LC_CTYPE, c"C.UTF-8".as_ptr()).is_null() {
            assert!(!setlocale(LC_CTYPE, c"en_US.UTF-8".as_ptr()).is_null());
        }
    }
    let args: Vec<_> = std::env::args().collect();
    let path = Path::new(&args[2]);
    if args[1] == "write" {
        write(path, args.get(3).is_some_and(|m| m == "threaded"))
    } else {
        read(path)
    }
}
