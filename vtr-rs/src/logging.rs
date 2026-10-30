//! Logging conventions carried by generic datatype attributes.

use crate::{
    Attribute, DatatypeOptions, Error, FormatLocale, Payload, PayloadType, Reader, Record,
    RecordKind, Result, TypeId, Value, Writer,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FormatStyle {
    #[default]
    Brace,
    Printf,
}

#[derive(Clone, Debug)]
pub struct CallsiteOptions {
    pub format: String,
    pub style: FormatStyle,
    pub severity: u64,
    pub file: Option<String>,
    pub line: Option<u64>,
    pub function: Option<String>,
}
impl CallsiteOptions {
    pub fn new(format: impl Into<String>) -> Self {
        Self {
            format: format.into(),
            style: FormatStyle::Brace,
            severity: 2,
            file: None,
            line: None,
            function: None,
        }
    }
}

pub fn declare<T: Payload + 'static>(
    writer: &mut Writer,
    options: CallsiteOptions,
) -> Result<PayloadType<T>> {
    let metadata = datatype_options(options, |text| writer.intern(text))?;
    writer.datatype(metadata)
}

pub fn declare_raw(
    writer: &mut Writer,
    options: CallsiteOptions,
    arguments: TypeId,
) -> Result<TypeId> {
    let metadata = datatype_options(options, |text| writer.intern(text))?;
    writer.declare_datatype(
        metadata,
        crate::schema::TypeKind::Struct(vec![crate::schema::Field::new("value", arguments)]),
    )
}

pub(crate) fn datatype_options(
    options: CallsiteOptions,
    mut intern: impl FnMut(&str) -> Result<u32>,
) -> Result<DatatypeOptions> {
    let mut attributes = vec![
        Attribute {
            key: intern("kind")?,
            value: Value::Text("callsite".into()),
        },
        Attribute {
            key: intern("log.format")?,
            value: Value::Text(options.format),
        },
        Attribute {
            key: intern("log.severity")?,
            value: Value::Enum {
                value: i64::try_from(options.severity)
                    .map_err(|_| Error::invalid("log severity exceeds i64"))?,
                name: intern(Severity::from_code(options.severity).name())?,
            },
        },
    ];
    if options.style == FormatStyle::Printf {
        attributes.push(Attribute {
            key: intern("log.format_style")?,
            value: Value::Text("printf".into()),
        });
    }
    for (key, value) in [
        ("log.file", options.file.map(Value::Text)),
        ("log.line", options.line.map(Value::U64)),
        ("log.func", options.function.map(Value::Text)),
    ] {
        if let Some(value) = value {
            attributes.push(Attribute {
                key: intern(key)?,
                value,
            });
        }
    }
    Ok(DatatypeOptions {
        name: "LogMessage".into(),
        attributes,
    })
}

#[derive(Clone, Debug)]
pub struct LogMetadata {
    pub datatype: TypeId,
    pub format: String,
    pub style: FormatStyle,
    pub severity: Severity,
    pub file: Option<String>,
    pub line: Option<u64>,
    pub function: Option<String>,
}

fn text(reader: &Reader, value: &Value) -> Result<String> {
    match value {
        Value::Text(text) => Ok(text.clone()),
        Value::Str(id) => Ok(reader.string(*id)?.into()),
        _ => Err(Error::invalid("log metadata requires text")),
    }
}

pub fn metadata(reader: &Reader, datatype: TypeId) -> Result<Option<LogMetadata>> {
    let declaration = reader.declarations().types.get(datatype)?;
    let mut values = std::collections::HashMap::new();
    for attribute in &declaration.attributes {
        values
            .entry(reader.string(attribute.key)?)
            .or_insert(&attribute.value);
    }
    let Some(kind) = values.get("kind") else {
        return Ok(None);
    };
    if !matches!(kind, Value::Text(_) | Value::Str(_)) || text(reader, kind)? != "callsite" {
        return Ok(None);
    }
    let format = text(
        reader,
        values
            .get("log.format")
            .ok_or_else(|| Error::invalid("log format is missing"))?,
    )?;
    let style = match values
        .get("log.format_style")
        .map(|v| text(reader, v))
        .transpose()?
        .as_deref()
    {
        None | Some("brace") => FormatStyle::Brace,
        Some("printf") => FormatStyle::Printf,
        _ => return Err(Error::invalid("unknown log format style")),
    };
    let severity = match values.get("log.severity") {
        Some(Value::Enum { value, .. }) if *value >= 0 => Severity::from_code(*value as u64),
        _ => return Err(Error::invalid("log severity requires a nonnegative enum")),
    };
    let file = values
        .get("log.file")
        .map(|v| text(reader, v))
        .transpose()?;
    let function = values
        .get("log.func")
        .map(|v| text(reader, v))
        .transpose()?;
    let line = match values.get("log.line") {
        None => None,
        Some(Value::U64(value)) => Some(*value),
        _ => return Err(Error::invalid("log line requires u64")),
    };
    Ok(Some(LogMetadata {
        datatype,
        format,
        style,
        severity,
        file,
        line,
        function,
    }))
}

pub fn callsites(reader: &Reader) -> Result<Vec<LogMetadata>> {
    let mut result = vec![];
    for datatype in crate::schema::FIRST_DECLARED_TYPE..reader.declarations().types.len() as u32 {
        if let Some(metadata) = metadata(reader, datatype)? {
            result.push(metadata);
        }
    }
    Ok(result)
}

pub fn format_log(reader: &Reader, record: &Record) -> Result<String> {
    format_log_with_locale(reader, record, &FormatLocale::default())
}

pub fn format_log_with_locale(
    reader: &Reader,
    record: &Record,
    locale: &FormatLocale<'_>,
) -> Result<String> {
    String::from_utf8(format_log_bytes_with_locale(reader, record, locale)?)
        .map_err(|_| Error::invalid("formatted log is not UTF-8"))
}

pub fn format_log_bytes(reader: &Reader, record: &Record) -> Result<Vec<u8>> {
    format_log_bytes_with_locale(reader, record, &FormatLocale::default())
}

pub fn format_log_bytes_with_locale(
    reader: &Reader,
    record: &Record,
    locale: &FormatLocale<'_>,
) -> Result<Vec<u8>> {
    if record.kind() != RecordKind::Log {
        return Err(Error::invalid("log formatting requires a LOG record"));
    }
    let metadata = metadata(reader, record.datatype())?
        .ok_or_else(|| Error::invalid("datatype has no log metadata"))?;
    let arguments = record.payload().field("value")?;
    match metadata.style {
        FormatStyle::Brace => reader.format_arguments_bytes(&metadata.format, &arguments, locale),
        FormatStyle::Printf => reader.format_printf_arguments_bytes(&metadata.format, &arguments),
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Severity {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
    Other(u64),
}
impl Severity {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
            Self::Fatal => "fatal",
            Self::Other(_) => "other",
        }
    }
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "trace" => Some(Self::Trace),
            "debug" => Some(Self::Debug),
            "info" => Some(Self::Info),
            "warn" => Some(Self::Warn),
            "error" => Some(Self::Error),
            "fatal" => Some(Self::Fatal),
            _ => None,
        }
    }
    fn from_code(code: u64) -> Self {
        match code {
            0 => Self::Trace,
            1 => Self::Debug,
            2 => Self::Info,
            3 => Self::Warn,
            4 => Self::Error,
            5 => Self::Fatal,
            _ => Self::Other(code),
        }
    }
}
