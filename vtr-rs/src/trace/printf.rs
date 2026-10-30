//! Bounded parsing and typed rendering of deferred printf arguments.

use super::{PayloadView, Reader};
use crate::{Error, Result, Value, schema::TypeKind};

const LIMIT: usize = 65_536;
#[derive(Clone, Copy)]
enum Field {
    Constant(i32),
    Argument(usize),
}
struct Conversion {
    flags: String,
    width: Field,
    precision: Option<Field>,
    length: String,
    ty: u8,
    argument: usize,
}
enum Part<'a> {
    Literal(&'a [u8]),
    Conversion(Conversion),
}
enum Index {
    Automatic,
    Positional(usize),
}
struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    automatic: usize,
    positional: Option<bool>,
}
impl Parser<'_> {
    fn peek(&self) -> u8 {
        self.bytes.get(self.pos).copied().unwrap_or(0)
    }
    fn number(&mut self) -> Option<usize> {
        let start = self.pos;
        while self.peek().is_ascii_digit() {
            self.pos += 1;
        }
        (start != self.pos)
            .then(|| {
                std::str::from_utf8(&self.bytes[start..self.pos])
                    .ok()?
                    .parse()
                    .ok()
            })
            .flatten()
    }
    fn explicit(&mut self) -> Option<Index> {
        let start = self.pos;
        let number = self.number();
        if self.peek() == b'$' {
            self.pos += 1;
            Some(Index::Positional(number?.checked_sub(1)?))
        } else {
            self.pos = start;
            Some(Index::Automatic)
        }
    }
    fn index(&mut self, explicit: Index) -> Option<usize> {
        let positional = matches!(explicit, Index::Positional(_));
        if self.positional.is_some_and(|p| p != positional) {
            return None;
        }
        self.positional = Some(positional);
        Some(match explicit {
            Index::Positional(index) => index,
            Index::Automatic => {
                let index = self.automatic;
                self.automatic += 1;
                index
            }
        })
    }
    fn field(&mut self) -> Option<Field> {
        if self.peek() == b'*' {
            self.pos += 1;
            let explicit = self.explicit()?;
            Some(Field::Argument(self.index(explicit)?))
        } else {
            let start = self.pos;
            let value = self.number();
            let value = if start == self.pos { 0 } else { value? };
            (value <= LIMIT).then_some(Field::Constant(value as i32))
        }
    }
}

fn parse(format: &str) -> Option<Vec<Part<'_>>> {
    let mut p = Parser {
        bytes: format.as_bytes(),
        pos: 0,
        automatic: 0,
        positional: None,
    };
    let mut parts = Vec::new();
    while p.pos < p.bytes.len() {
        if parts.len() >= LIMIT {
            return None;
        }
        let start = p.pos;
        while p.pos < p.bytes.len() && p.peek() != b'%' {
            p.pos += 1;
        }
        if p.pos != start {
            parts.push(Part::Literal(&p.bytes[start..p.pos]));
        }
        if p.pos == p.bytes.len() {
            break;
        }
        p.pos += 1;
        if p.peek() == b'%' {
            parts.push(Part::Literal(b"%"));
            p.pos += 1;
            continue;
        }
        let explicit = p.explicit()?;
        let mut flags = String::new();
        while p.peek() != 0 && b"-+ #0'".contains(&p.peek()) {
            if !flags.as_bytes().contains(&p.peek()) {
                flags.push(p.peek() as char);
            }
            p.pos += 1;
        }
        let width = p.field()?;
        let precision = if p.peek() == b'.' {
            p.pos += 1;
            Some(p.field()?)
        } else {
            None
        };
        let start = p.pos;
        if p.peek() != 0 && b"hljztL".contains(&p.peek()) {
            let first = p.peek();
            p.pos += 1;
            if matches!(first, b'h' | b'l') && p.peek() == first {
                p.pos += 1;
            }
        }
        let length = std::str::from_utf8(&p.bytes[start..p.pos]).ok()?.to_owned();
        let ty = p.peek();
        if ty == 0 || !b"diuoxXbBfFeEgGaAcspn".contains(&ty) {
            return None;
        }
        p.pos += 1;
        let valid = if b"diuoxXbBn".contains(&ty) {
            length != "L"
        } else if b"fFeEgGaA".contains(&ty) {
            matches!(length.as_str(), "" | "l" | "L")
        } else if ty == b'p' {
            length.is_empty()
        } else {
            matches!(length.as_str(), "" | "l")
        };
        if !valid {
            return None;
        }
        parts.push(Part::Conversion(Conversion {
            flags,
            width,
            precision,
            length,
            ty,
            argument: p.index(explicit)?,
        }));
    }
    (parts.len() <= LIMIT).then_some(parts)
}

fn integer(value: &Value) -> Option<(u64, u32, bool)> {
    Some(match value {
        Value::Bool(v) => (u64::from(*v), 8, false),
        Value::I8(v) => (*v as u64, 8, true),
        Value::I16(v) => (*v as u64, 16, true),
        Value::I32(v) => (*v as u64, 32, true),
        Value::I64(v) | Value::Enum { value: v, .. } => (*v as u64, 64, true),
        Value::U8(v) => (u64::from(*v), 8, false),
        Value::U16(v) => (u64::from(*v), 16, false),
        Value::U32(v) => (u64::from(*v), 32, false),
        Value::U64(v) | Value::Time(v) | Value::Pointer(v) => (*v, 64, false),
        _ => return None,
    })
}
fn signed(bits: u64, width: u32) -> i64 {
    ((bits << (64 - width)) as i64) >> (64 - width)
}
fn unsigned(bits: u64, width: u32) -> u64 {
    bits & (u64::MAX >> (64 - width))
}

impl Reader {
    pub fn format_printf_arguments(&self, format: &str, payload: &PayloadView) -> Result<String> {
        String::from_utf8(self.format_printf_arguments_bytes(format, payload)?)
            .map_err(|_| Error::invalid("formatted printf log is not UTF-8"))
    }
    pub fn format_printf_arguments_bytes(
        &self,
        format: &str,
        payload: &PayloadView,
    ) -> Result<Vec<u8>> {
        if payload.types.origin != self.identity() {
            return Err(Error::invalid("payload belongs to another recording"));
        }
        let limit = self.options.max_payload_bytes;
        if format.len() > limit {
            return Err(Error::invalid("printf format exceeds reader limit"));
        }
        let Some(parts) = parse(format) else {
            return Ok(b"{?}".to_vec());
        };
        let structure = matches!(
            payload.schema().get(payload.datatype())?.kind,
            TypeKind::Struct(_)
        );
        let count = if structure {
            payload.len()?
        } else {
            usize::from(payload.datatype() != 0)
        };
        let argument = |index: usize| -> Result<Option<Value>> {
            if index >= count {
                return Ok(None);
            }
            let value = if structure {
                payload.at(index)?
            } else {
                payload.clone()
            };
            Ok(value.scalar().ok())
        };
        let field = |field: Field| -> Result<Option<i32>> {
            Ok(match field {
                Field::Constant(v) => Some(v),
                Field::Argument(index) => argument(index)?.as_ref().and_then(integer).and_then(
                    |(bits, width, is_signed)| {
                        if is_signed {
                            i32::try_from(signed(bits, width)).ok()
                        } else {
                            i32::try_from(unsigned(bits, width)).ok()
                        }
                    },
                ),
            })
        };
        let mut output = Vec::new();
        for part in parts {
            let text = match part {
                Part::Literal(bytes) => bytes.to_vec(),
                Part::Conversion(mut spec) => {
                    let width = field(spec.width)?;
                    let precision = spec.precision.map(&field).transpose()?.flatten();
                    let value = argument(spec.argument)?;
                    match (width, value) {
                        (Some(width), Some(value))
                            if (-65536..=65536).contains(&width)
                                && (spec.precision.is_none() || precision.is_some())
                                && precision.is_none_or(|p| p <= 65536) =>
                        {
                            if width < 0 && !spec.flags.contains('-') {
                                spec.flags.push('-');
                            }
                            self.render_printf(
                                &spec,
                                &value,
                                width.unsigned_abs() as usize,
                                precision.filter(|p| *p >= 0).map(|p| p as usize),
                            )?
                        }
                        _ => b"{?}".to_vec(),
                    }
                }
            };
            if text.len() > limit.saturating_sub(output.len()) {
                return Err(Error::invalid(
                    "formatted printf message exceeds reader limit",
                ));
            }
            output.extend(text);
        }
        Ok(output)
    }
    fn render_printf(
        &self,
        spec: &Conversion,
        value: &Value,
        width: usize,
        precision: Option<usize>,
    ) -> Result<Vec<u8>> {
        #[cfg(not(target_family = "wasm"))]
        if !matches!(spec.ty, b'b' | b'B') {
            return self.render_printf_native(spec, value, width, precision);
        }
        self.render_printf_portable(spec, value, width, precision)
    }
    fn render_printf_portable(
        &self,
        spec: &Conversion,
        value: &Value,
        width: usize,
        precision: Option<usize>,
    ) -> Result<Vec<u8>> {
        let mut prefix = String::new();
        let mut body;
        let mut numeric = false;
        let mut zero = spec.flags.contains('0') && !spec.flags.contains('-');
        let ty = spec.ty;
        if b"diuoxXbB".contains(&ty) {
            let Some((bits, captured_width, is_signed)) = integer(value) else {
                return Ok(b"{?}".to_vec());
            };
            let bits_width = match spec.length.as_str() {
                "hh" => 8,
                "h" => 16,
                "ll" | "j" => 64,
                "l" => std::mem::size_of::<std::ffi::c_long>() as u32 * 8,
                "z" | "t" => usize::BITS,
                _ => captured_width,
            };
            let signed_decimal = b"di".contains(&ty) && is_signed;
            let negative = signed_decimal && signed(bits, bits_width) < 0;
            let number = if signed_decimal {
                signed(bits, bits_width).unsigned_abs()
            } else {
                unsigned(bits, bits_width)
            };
            body = match ty {
                b'x' | b'X' => format!("{number:x}"),
                b'o' => format!("{number:o}"),
                b'b' | b'B' => format!("{number:b}"),
                _ => number.to_string(),
            }
            .into_bytes();
            if precision == Some(0) && number == 0 {
                body.clear();
            }
            if let Some(p) = precision {
                if p > body.len() {
                    let mut padded = vec![b'0'; p - body.len()];
                    padded.extend(body);
                    body = padded;
                }
                zero = false;
            }
            if signed_decimal {
                if negative {
                    prefix.push('-');
                } else if spec.flags.contains('+') {
                    prefix.push('+');
                } else if spec.flags.contains(' ') {
                    prefix.push(' ');
                }
            }
            if spec.flags.contains('#') {
                match ty {
                    b'x' | b'X' if number != 0 => prefix.push_str("0x"),
                    b'b' | b'B' if number != 0 => prefix.push_str("0b"),
                    b'o' if !body.starts_with(b"0") => prefix.push('0'),
                    _ => (),
                }
            }
            if matches!(ty, b'X' | b'B') {
                prefix.make_ascii_uppercase();
                body.make_ascii_uppercase();
            }
            numeric = true;
        } else if b"fFeEgGaA".contains(&ty) {
            let number = match value {
                Value::F32(v) => f64::from(*v),
                Value::F64(v) => *v,
                _ => return Ok(b"{?}".to_vec()),
            };
            body = super::format::printf_float(number, ty, precision, spec.flags.contains('#'));
            if number.is_sign_negative() {
                prefix.push('-');
            } else if spec.flags.contains('+') {
                prefix.push('+');
            } else if spec.flags.contains(' ') {
                prefix.push(' ');
            }
            if matches!(ty, b'a' | b'A') && number.is_finite() {
                prefix.push_str(if ty == b'A' { "0X" } else { "0x" });
            }
            if !number.is_finite() {
                zero = false;
            }
            numeric = true;
        } else {
            if spec.flags.bytes().any(|b| b != b'-') {
                return Ok(b"{?}".to_vec());
            }
            zero = false;
            body = match ty {
                b's' => {
                    let text = match value {
                        Value::Text(v) => v.as_str(),
                        Value::Str(id) => self.string(*id)?,
                        _ => return Ok(b"{?}".to_vec()),
                    };
                    let text = text.split('\0').next().unwrap();
                    let mut end = precision.unwrap_or(text.len()).min(text.len());
                    if spec.length == "l" {
                        while !text.is_char_boundary(end) {
                            end -= 1;
                        }
                    }
                    text.as_bytes()[..end].to_vec()
                }
                b'c' if precision.is_none() => {
                    let Some((bits, bits_width, _)) = integer(value) else {
                        return Ok(b"{?}".to_vec());
                    };
                    if spec.length == "l" {
                        let Some(c) = u32::try_from(unsigned(bits, bits_width))
                            .ok()
                            .and_then(char::from_u32)
                        else {
                            return Ok(b"{?}".to_vec());
                        };
                        c.to_string().into_bytes()
                    } else {
                        vec![bits as u8]
                    }
                }
                b'p' if precision.is_none() => {
                    let Some((bits, bits_width, _)) = integer(value) else {
                        return Ok(b"{?}".to_vec());
                    };
                    let Ok(address) = usize::try_from(unsigned(bits, bits_width)) else {
                        return Ok(b"{?}".to_vec());
                    };
                    format!("{address:#x}").into_bytes()
                }
                _ => return Ok(b"{?}".to_vec()),
            };
        }
        if body.len() + prefix.len() > LIMIT {
            return Ok(b"{?}".to_vec());
        }
        let padding = width.saturating_sub(body.len() + prefix.len());
        let mut output = Vec::new();
        if !spec.flags.contains('-') && !zero {
            output.resize(padding, b' ');
        }
        output.extend(prefix.bytes());
        if numeric && zero {
            output.resize(output.len() + padding, b'0');
        }
        output.extend(body);
        if spec.flags.contains('-') {
            output.resize(output.len() + padding, b' ');
        }
        Ok(output)
    }
}

#[cfg(not(target_family = "wasm"))]
unsafe extern "C" {
    fn vtr_printf_scalar(
        out: *mut std::ffi::c_char,
        size: usize,
        format: *const std::ffi::c_char,
        kind: std::ffi::c_int,
        bits: u64,
        number: f64,
        text: *const std::ffi::c_char,
    ) -> std::ffi::c_int;
}

#[cfg(not(target_family = "wasm"))]
impl Reader {
    fn render_printf_native(
        &self,
        spec: &Conversion,
        value: &Value,
        width: usize,
        precision: Option<usize>,
    ) -> Result<Vec<u8>> {
        let (mut bits, mut number, mut text) = (0, 0.0, String::new());
        let ty = spec.ty;
        let (kind, suffix) = if b"diuoxX".contains(&ty) {
            let Some((raw, captured_width, is_signed)) = integer(value) else {
                return Ok(b"{?}".to_vec());
            };
            let width = match spec.length.as_str() {
                "hh" => 8,
                "h" => 16,
                "ll" | "j" => 64,
                "l" => std::mem::size_of::<std::ffi::c_long>() as u32 * 8,
                "z" | "t" => usize::BITS,
                _ => captured_width,
            };
            if b"di".contains(&ty) && is_signed {
                bits = signed(raw, width) as u64;
                (0, "lld".to_owned())
            } else {
                bits = unsigned(raw, width);
                (
                    1,
                    format!("ll{}", if b"di".contains(&ty) { 'u' } else { ty as char }),
                )
            }
        } else if b"fFeEgGaA".contains(&ty) {
            number = match value {
                Value::F32(v) => f64::from(*v),
                Value::F64(v) => *v,
                _ => return Ok(b"{?}".to_vec()),
            };
            let extended = spec.length == "L";
            (
                if extended { 3 } else { 2 },
                format!("{}{}", if extended { "L" } else { "" }, ty as char),
            )
        } else {
            if spec.flags.bytes().any(|b| b != b'-') {
                return Ok(b"{?}".to_vec());
            }
            match ty {
                b's' => {
                    text = match value {
                        Value::Text(v) => v.clone(),
                        Value::Str(id) => self.string(*id)?.into(),
                        _ => return Ok(b"{?}".to_vec()),
                    };
                    (
                        if spec.length == "l" { 8 } else { 4 },
                        format!("{}s", spec.length),
                    )
                }
                b'c' | b'p' if precision.is_none() => {
                    let Some((raw, width, _)) = integer(value) else {
                        return Ok(b"{?}".to_vec());
                    };
                    bits = unsigned(raw, width);
                    if ty == b'p' {
                        if usize::try_from(bits).is_err() {
                            return Ok(b"{?}".to_vec());
                        }
                        (7, "p".into())
                    } else if spec.length == "l" {
                        if u32::try_from(bits).ok().and_then(char::from_u32).is_none() {
                            return Ok(b"{?}".to_vec());
                        }
                        (6, "lc".into())
                    } else {
                        (5, "c".into())
                    }
                }
                _ => return Ok(b"{?}".to_vec()),
            }
        };
        let format = format!(
            "%{}{}{}{}",
            spec.flags,
            if width == 0 {
                String::new()
            } else {
                width.to_string()
            },
            precision.map(|p| format!(".{p}")).unwrap_or_default(),
            suffix
        );
        let format = std::ffi::CString::new(format).unwrap();
        let text = std::ffi::CString::new(text.split('\0').next().unwrap()).unwrap();
        let mut small = [0u8; 256];
        // SAFETY: The format contains one validated conversion, reconstructed above
        // with the exact C argument type selected by kind. Both strings are terminated
        // and snprintf receives the full writable buffer size.
        let render = |output: &mut [u8]| unsafe {
            vtr_printf_scalar(
                output.as_mut_ptr().cast(),
                output.len(),
                format.as_ptr(),
                kind,
                bits,
                number,
                text.as_ptr(),
            )
        };
        let size = render(&mut small);
        if size < 0 || size as usize > LIMIT {
            return Ok(b"{?}".to_vec());
        }
        let size = size as usize;
        if size < small.len() {
            return Ok(small[..size].to_vec());
        }
        let mut output = vec![0; size + 1];
        if render(&mut output) != size as i32 {
            return Ok(b"{?}".to_vec());
        }
        output.truncate(size);
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portable_printf_byte_precision_numbers_and_wide_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("portable.vtr");
        crate::Writer::create(&path).unwrap().close().unwrap();
        let reader = Reader::open(path).unwrap();
        let cases = [
            ("%#08x", Value::I8(-1), "0x0000ff".as_bytes()),
            ("%#.0o", Value::U32(0), b"0"),
            ("%+.2f", Value::F64(-1.25), b"-1.25"),
            ("%a", Value::F64(1.25), b"0x1.4p+0"),
            (
                "%a",
                Value::F64(f64::from_bits(1)),
                b"0x0.0000000000001p-1022",
            ),
            ("%.2s", Value::Text("héllo".into()), b"h\xc3"),
            ("%.2ls", Value::Text("héllo".into()), b"h"),
            ("%lc", Value::U32(0x3bb), "λ".as_bytes()),
            ("%c", Value::U32(0), b"\0"),
            ("%n", Value::Pointer(0), b"{?}"),
        ];
        for (format, value, expected) in cases {
            let parts = parse(format).unwrap();
            let Part::Conversion(spec) = &parts[0] else {
                unreachable!()
            };
            let Field::Constant(width) = spec.width else {
                unreachable!()
            };
            let precision = match spec.precision {
                Some(Field::Constant(p)) => Some(p as usize),
                _ => None,
            };
            assert_eq!(
                reader
                    .render_printf_portable(spec, &value, width as usize, precision)
                    .unwrap(),
                expected,
                "{format}"
            );
        }
    }
}
