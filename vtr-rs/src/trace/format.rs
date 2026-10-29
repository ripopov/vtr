//! Typed value and argument rendering.

use super::{PayloadView, Reader};
use crate::{Error, Result, attributes::Value, schema::TypeKind};
use std::{borrow::Cow, fmt::Write};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

/// Per-call punctuation for `L` log presentations, matching `std::numpunct<char>`.
/// Separators are single bytes; boolean names may contain arbitrary bytes.
#[derive(Clone, Copy, Debug)]
pub struct FormatLocale<'a> {
    pub decimal_point: u8,
    pub thousands_sep: u8,
    /// Group sizes from right to left; the last repeats. Zero, a negative C
    /// `char`, or `CHAR_MAX` stops grouping the remaining leading digits.
    pub grouping: &'a [u8],
    pub true_name: &'a [u8],
    pub false_name: &'a [u8],
}
impl Default for FormatLocale<'_> {
    fn default() -> Self {
        Self {
            decimal_point: b'.',
            thousands_sep: b',',
            grouping: &[],
            true_name: b"true",
            false_name: b"false",
        }
    }
}

#[derive(Default)]
struct RenderBuffer(Vec<u8>);
impl RenderBuffer {
    fn push(&mut self, c: char) {
        self.push_str(c.encode_utf8(&mut [0; 4]));
    }
    fn push_str(&mut self, text: &str) {
        self.0.extend_from_slice(text.as_bytes());
    }
    fn text(self) -> Result<String> {
        String::from_utf8(self.0)
            .map_err(|_| Error::invalid("formatted log is not UTF-8; use format_log_bytes"))
    }
}
impl std::fmt::Write for RenderBuffer {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        self.push_str(text);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sign {
    NegativeOnly,
    Plus,
    Space,
}
impl Sign {
    fn prefix(self, negative: bool) -> &'static str {
        if negative {
            "-"
        } else {
            match self {
                Self::NegativeOnly => "",
                Self::Plus => "+",
                Self::Space => " ",
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Spec {
    fill: char,
    align: u8,
    sign: Option<Sign>,
    alt: bool,
    zero: bool,
    locale: bool,
    width: usize,
    precision: Option<usize>,
    ty: u8,
}

fn is_align(c: char) -> bool {
    matches!(c, '<' | '^' | '>')
}

fn parse_spec(s: &str) -> Option<Spec> {
    let mut sp = Spec {
        fill: ' ',
        align: 0,
        sign: None,
        alt: false,
        zero: false,
        locale: false,
        width: 0,
        precision: None,
        ty: 0,
    };
    let b = s.as_bytes();
    let mut i = 0;
    let mut chars = s.chars();
    let c0 = chars.next();
    let c1 = chars.next();
    if let (Some(f), Some(a)) = (c0, c1)
        && is_align(a)
    {
        sp.fill = f;
        sp.align = a as u8;
        i = f.len_utf8() + 1;
    }
    if i == 0
        && let Some(a) = c0
        && is_align(a)
    {
        sp.align = a as u8;
        i = 1;
    }
    if i < b.len() && b[i] == b'+' {
        sp.sign = Some(Sign::Plus);
        i += 1;
    } else if i < b.len() && b[i] == b' ' {
        sp.sign = Some(Sign::Space);
        i += 1;
    } else if i < b.len() && b[i] == b'-' {
        sp.sign = Some(Sign::NegativeOnly);
        i += 1;
    }
    if i < b.len() && b[i] == b'#' {
        sp.alt = true;
        i += 1;
    }
    if i < b.len() && b[i] == b'0' {
        sp.zero = true;
        i += 1;
    }
    while i < b.len() && b[i].is_ascii_digit() {
        sp.width = sp
            .width
            .checked_mul(10)?
            .checked_add((b[i] - b'0') as usize)?
            .min(65537);
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        if !b.get(i).is_some_and(u8::is_ascii_digit) {
            return None;
        }
        let mut p = 0usize;
        while i < b.len() && b[i].is_ascii_digit() {
            p = p
                .checked_mul(10)?
                .checked_add((b[i] - b'0') as usize)?
                .min(65537);
            i += 1;
        }
        sp.precision = Some(p);
    }
    if b.get(i) == Some(&b'L') {
        sp.locale = true;
        i += 1;
    }
    if i < b.len() {
        sp.ty = b[i];
        i += 1;
    }
    if i != b.len()
        || matches!(sp.fill, '{' | '}')
        || sp.width > 65536
        || sp.precision.is_some_and(|precision| precision > 65536)
        || !matches!(
            sp.ty,
            0 | b'd'
                | b'x'
                | b'X'
                | b'o'
                | b'b'
                | b'B'
                | b'e'
                | b'E'
                | b'f'
                | b'F'
                | b'g'
                | b'G'
                | b'a'
                | b'A'
                | b's'
                | b'c'
        )
    {
        return None;
    }
    Some(sp)
}

fn pad_number(out: &mut RenderBuffer, sign: &str, prefix: &str, body: &[u8], sp: &Spec) {
    let len = sign.len() + prefix.len() + body.len();
    if sp.zero && sp.align == 0 {
        out.push_str(sign);
        out.push_str(prefix);
        for _ in len..sp.width {
            out.push('0');
        }
        out.0.extend_from_slice(body);
        return;
    }
    let pad = sp.width.saturating_sub(len);
    let (left, right) = match sp.align {
        b'<' => (0, pad),
        b'^' => (pad / 2, pad - pad / 2),
        _ => (pad, 0),
    };
    for _ in 0..left {
        out.push(sp.fill);
    }
    out.push_str(sign);
    out.push_str(prefix);
    out.0.extend_from_slice(body);
    for _ in 0..right {
        out.push(sp.fill);
    }
}

fn localize_number<'a>(
    body: &'a str,
    spec: &Spec,
    locale: &FormatLocale<'_>,
    floating: bool,
) -> Cow<'a, [u8]> {
    if !spec.locale {
        return Cow::Borrowed(body.as_bytes());
    }
    let exponent = if matches!(spec.ty, b'a' | b'A') {
        ['p', 'P']
    } else {
        ['e', 'E']
    };
    let integer_end = body
        .find('.')
        .into_iter()
        .chain(floating.then(|| body.find(exponent)).flatten())
        .min()
        .unwrap_or(body.len());
    let mut boundaries = Vec::new();
    let mut remaining = integer_end;
    let mut group = 0;
    while let Some(&size) = locale.grouping.get(group) {
        if size as std::ffi::c_char <= 0
            || size == std::ffi::c_char::MAX as u8
            || remaining <= usize::from(size)
        {
            break;
        }
        remaining -= usize::from(size);
        boundaries.push(remaining);
        if group + 1 < locale.grouping.len() {
            group += 1;
        }
    }
    if boundaries.is_empty() && (locale.decimal_point == b'.' || !body.contains('.')) {
        return Cow::Borrowed(body.as_bytes());
    }
    let mut localized = Vec::with_capacity(body.len() + boundaries.len());
    for (index, byte) in body.bytes().enumerate() {
        if boundaries.last() == Some(&index) {
            localized.push(locale.thousands_sep);
            boundaries.pop();
        }
        localized.push(if byte == b'.' {
            locale.decimal_point
        } else {
            byte
        });
    }
    Cow::Owned(localized)
}

fn write_uint(
    out: &mut RenderBuffer,
    v: u64,
    negative: bool,
    sp: &Spec,
    locale: &FormatLocale<'_>,
) {
    let (prefix, radix, upper) = match sp.ty {
        b'x' => ("0x", 16u64, false),
        b'X' => ("0X", 16, true),
        b'o' => ("0", 8, false),
        b'b' => ("0b", 2, false),
        b'B' => ("0B", 2, true),
        _ => ("", 10, false),
    };
    let mut buf = [0u8; 64];
    let mut pos = buf.len();
    let mut x = v;
    if x == 0 {
        pos -= 1;
        buf[pos] = b'0';
    }
    while x > 0 {
        let d = (x % radix) as u8;
        pos -= 1;
        buf[pos] = if d < 10 {
            b'0' + d
        } else if upper {
            b'A' + d - 10
        } else {
            b'a' + d - 10
        };
        x /= radix;
    }
    let sign = sp.sign.unwrap_or(Sign::NegativeOnly).prefix(negative);
    let prefix = if sp.alt && !(sp.ty == b'o' && v == 0) {
        prefix
    } else {
        ""
    };
    // SAFETY: The digit buffer contains only ASCII digits and letters.
    let body = unsafe { std::str::from_utf8_unchecked(&buf[pos..]) };
    pad_number(
        out,
        sign,
        prefix,
        &localize_number(body, sp, locale, false),
        sp,
    );
}

fn pad_text(out: &mut RenderBuffer, text: &[u8], width: usize, sp: &Spec) {
    let pad = sp.width.saturating_sub(width);
    let (left, right) = match sp.align {
        b'>' => (pad, 0),
        b'^' => (pad / 2, pad - pad / 2),
        _ => (0, pad),
    };
    for _ in 0..left {
        out.push(sp.fill);
    }
    out.0.extend_from_slice(text);
    for _ in 0..right {
        out.push(sp.fill);
    }
}

fn write_str(out: &mut RenderBuffer, text: &str, sp: &Spec) {
    if sp.width == 0 && sp.precision.is_none() {
        out.push_str(text);
        return;
    }
    let mut width = 0;
    let mut end = 0;
    for (index, cluster) in text.grapheme_indices(true) {
        let size = cluster_width(cluster);
        if sp.precision.is_some_and(|limit| width + size > limit) {
            break;
        }
        width += size;
        end = index + cluster.len();
    }
    pad_text(out, &text.as_bytes()[..end], width, sp);
}

fn cluster_width(cluster: &str) -> usize {
    let first = cluster.chars().next().unwrap();
    let value = u32::from(first);
    if first.width() == Some(2)
        || matches!(value, 0x4dc0..=0x4dff | 0x1f300..=0x1f5ff | 0x1f900..=0x1f9ff)
    {
        2
    } else {
        1
    }
}

#[derive(Clone, Copy)]
enum Float {
    Single(f32),
    Double(f64),
}
impl Float {
    fn value(self) -> f64 {
        match self {
            Self::Single(value) => f64::from(value),
            Self::Double(value) => value,
        }
    }
    fn fixed(self, precision: Option<usize>) -> String {
        match (self, precision) {
            (Self::Single(value), Some(p)) => format!("{value:.p$}"),
            (Self::Double(value), Some(p)) => format!("{value:.p$}"),
            (Self::Single(value), None) => value.to_string(),
            (Self::Double(value), None) => value.to_string(),
        }
    }
    fn scientific(self, precision: Option<usize>) -> String {
        match (self, precision) {
            (Self::Single(value), Some(p)) => format!("{value:.p$e}"),
            (Self::Double(value), Some(p)) => format!("{value:.p$e}"),
            (Self::Single(value), None) => format!("{value:e}"),
            (Self::Double(value), None) => format!("{value:e}"),
        }
    }
    fn abs(self) -> Self {
        match self {
            Self::Single(value) => Self::Single(value.abs()),
            Self::Double(value) => Self::Double(value.abs()),
        }
    }
}

fn scientific_parts(body: &str) -> (&str, i32) {
    let (digits, exponent) = body.split_once('e').unwrap();
    (digits, exponent.parse().unwrap())
}

fn exponent_text(digits: &str, exponent: i32, marker: char) -> String {
    format!("{digits}{marker}{exponent:+03}")
}

fn strip_fraction(body: &mut String) {
    if body.contains('.') {
        let length = body.trim_end_matches('0').len();
        body.truncate(length);
        if body.ends_with('.') {
            body.pop();
        }
    }
}

fn decimal_point(body: &mut String) {
    let end = body.find(['e', 'E', 'p', 'P']).unwrap_or(body.len());
    if !body[..end].contains('.') {
        body.insert(end, '.');
    }
}

fn hexadecimal(value: Float, spec: &Spec) -> String {
    let bits = value.value().to_bits();
    let raw_exponent = (bits >> 52) as i32;
    let fraction = bits & ((1u64 << 52) - 1);
    let (mantissa, exponent) = if raw_exponent == 0 {
        (fraction, if fraction == 0 { 0 } else { -1022 })
    } else {
        ((1u64 << 52) | fraction, raw_exponent - 1023)
    };
    let (mantissa, exponent) = match value {
        Float::Single(single) if spec.precision.is_some() && single.is_subnormal() => {
            (u64::from(single.to_bits() & ((1 << 23) - 1)) << 29, -126)
        }
        _ if spec.precision.is_none() && raw_exponent == 0 && fraction != 0 => {
            let shift = fraction.leading_zeros() - 11;
            (fraction << shift, exponent - shift as i32)
        }
        _ => (mantissa, exponent),
    };
    let precision = spec.precision.unwrap_or(13);
    let mut digits = if precision < 13 {
        let shift = 52 - precision * 4;
        let mut rounded = mantissa >> shift;
        let remainder = mantissa & ((1u64 << shift) - 1);
        let halfway = 1u64 << (shift - 1);
        if remainder > halfway || (remainder == halfway && rounded & 1 != 0) {
            rounded += 1;
        }
        format!("{rounded:0width$x}", width = precision + 1)
    } else {
        format!("{mantissa:014x}")
    };
    if precision > 13 {
        digits.extend(std::iter::repeat_n('0', precision - 13));
    }
    if digits.len() > 1 {
        digits.insert(1, '.');
    }
    if spec.precision.is_none() {
        strip_fraction(&mut digits);
    }
    if spec.alt {
        decimal_point(&mut digits);
    }
    digits.push('p');
    write!(digits, "{exponent:+}").unwrap();
    digits
}

fn float_body(value: Float, spec: &Spec) -> String {
    let mut body = match spec.ty {
        b'a' | b'A' => hexadecimal(value, spec),
        b'f' | b'F' => value.fixed(Some(spec.precision.unwrap_or(6))),
        b'e' | b'E' => {
            let scientific = value.scientific(Some(spec.precision.unwrap_or(6)));
            let (digits, exponent) = scientific_parts(&scientific);
            exponent_text(digits, exponent, 'e')
        }
        0 if spec.precision.is_none() => {
            let fixed = value.fixed(None);
            let scientific = value.scientific(None);
            let (digits, exponent) = scientific_parts(&scientific);
            let scientific = exponent_text(digits, exponent, 'e');
            if fixed.len() <= scientific.len() {
                fixed
            } else {
                scientific
            }
        }
        _ => {
            let precision = spec.precision.unwrap_or(6).max(1);
            let scientific = value.scientific(Some(precision - 1));
            let (digits, exponent) = scientific_parts(&scientific);
            if exponent >= -4 && exponent < precision as i32 {
                let mut fixed = value.fixed(Some((precision as i32 - 1 - exponent) as usize));
                if !spec.alt {
                    strip_fraction(&mut fixed);
                }
                fixed
            } else {
                let mut digits = digits.to_owned();
                if !spec.alt {
                    strip_fraction(&mut digits);
                }
                exponent_text(&digits, exponent, 'e')
            }
        }
    };
    if spec.alt {
        decimal_point(&mut body);
    }
    if matches!(spec.ty, b'E' | b'F' | b'G' | b'A') {
        body.make_ascii_uppercase();
    }
    body
}

fn write_float(out: &mut RenderBuffer, value: Float, spec: &Spec, locale: &FormatLocale<'_>) {
    let number = value.value();
    let sign = spec
        .sign
        .unwrap_or(Sign::NegativeOnly)
        .prefix(number.is_sign_negative());
    let mut effective = *spec;
    let body = if number.is_finite() {
        float_body(value.abs(), spec)
    } else {
        effective.zero = false;
        let text = if number.is_nan() { "nan" } else { "inf" };
        if matches!(spec.ty, b'E' | b'F' | b'G' | b'A') {
            text.to_ascii_uppercase()
        } else {
            text.into()
        }
    };
    let body = if number.is_finite() {
        localize_number(&body, spec, locale, true)
    } else {
        Cow::Borrowed(body.as_bytes())
    };
    pad_number(out, sign, "", &body, &effective);
}

impl Reader {
    pub fn format_arguments_bytes(
        &self,
        format: &str,
        payload: &PayloadView,
        locale: &FormatLocale<'_>,
    ) -> Result<Vec<u8>> {
        if payload.types.origin != self.identity() {
            return Err(Error::invalid("payload belongs to another recording"));
        }
        let structure = matches!(
            payload.schema().get(payload.datatype())?.kind,
            TypeKind::Struct(_)
        );
        let count = if structure {
            payload.len()?
        } else {
            usize::from(payload.datatype() != 0)
        };
        let (mut output, mut automatic, mut position) = (RenderBuffer::default(), 0usize, 0);
        while position < format.len() {
            let character = format[position..].chars().next().unwrap();
            position += character.len_utf8();
            if (character == '{' || character == '}') && format[position..].starts_with(character) {
                output.push(character);
                position += 1;
            } else if character != '{' {
                output.push(character);
            } else {
                let Some(close) = format[position..].find('}').map(|end| position + end) else {
                    output.push('{');
                    output.push_str(&format[position..]);
                    break;
                };
                let field = &format[position..close];
                let (index, spec) = field.split_once(':').unwrap_or((field, ""));
                let index = if index.is_empty() {
                    let index = automatic;
                    automatic = automatic.saturating_add(1);
                    Some(index)
                } else {
                    index.parse::<usize>().ok()
                };
                if let (Some(index), Some(spec)) =
                    (index.filter(|&index| index < count), parse_spec(spec))
                {
                    let value = if structure {
                        payload.at(index)?
                    } else {
                        (*payload).clone()
                    };
                    match value.scalar() {
                        Ok(value) => self.write_scalar(&mut output, &value, &spec, locale)?,
                        Err(_) => output.push_str("{?}"),
                    }
                } else {
                    output.push_str("{?}");
                }
                position = close + 1;
            }
        }
        Ok(output.0)
    }
    fn write_scalar(
        &self,
        output: &mut RenderBuffer,
        value: &Value,
        spec: &Spec,
        locale: &FormatLocale<'_>,
    ) -> Result<()> {
        let supported = match value {
            Value::I8(_)
            | Value::I16(_)
            | Value::I32(_)
            | Value::I64(_)
            | Value::U8(_)
            | Value::U16(_)
            | Value::U32(_)
            | Value::U64(_)
            | Value::Time(_)
            | Value::Pointer(_)
            | Value::Enum { .. } => {
                spec.precision.is_none()
                    && matches!(spec.ty, 0 | b'd' | b'x' | b'X' | b'o' | b'b' | b'B' | b'c')
            }
            Value::F32(_) | Value::F64(_) => {
                matches!(
                    spec.ty,
                    0 | b'e' | b'E' | b'f' | b'F' | b'g' | b'G' | b'a' | b'A'
                )
            }
            Value::Bool(_) if matches!(spec.ty, b'd' | b'x' | b'X' | b'o' | b'b' | b'B') => {
                spec.precision.is_none()
            }
            Value::Bool(_) => {
                spec.precision.is_none()
                    && matches!(spec.ty, 0 | b's')
                    && !spec.zero
                    && spec.sign.is_none()
                    && !spec.alt
            }
            _ => {
                matches!(spec.ty, 0 | b's')
                    && !spec.locale
                    && !spec.zero
                    && spec.sign.is_none()
                    && !spec.alt
            }
        };
        if !supported {
            output.push_str("{?}");
            return Ok(());
        }
        if spec.ty == b'c' {
            let number = match value {
                Value::I8(v) => i128::from(*v),
                Value::I16(v) => i128::from(*v),
                Value::I32(v) => i128::from(*v),
                Value::I64(v) | Value::Enum { value: v, .. } => i128::from(*v),
                Value::U8(v) => i128::from(*v),
                Value::U16(v) => i128::from(*v),
                Value::U32(v) => i128::from(*v),
                Value::U64(v) | Value::Time(v) | Value::Pointer(v) => i128::from(*v),
                _ => unreachable!("only integer payloads support character formatting"),
            };
            if (i128::from(std::ffi::c_char::MIN)..=i128::from(std::ffi::c_char::MAX))
                .contains(&number)
            {
                pad_text(output, &[number as u8], 1, spec);
            } else {
                output.push_str("{?}");
            }
            return Ok(());
        }
        match value {
            Value::I8(value) => write_signed(output, i64::from(*value), spec, locale),
            Value::I16(value) => write_signed(output, i64::from(*value), spec, locale),
            Value::I32(value) => write_signed(output, i64::from(*value), spec, locale),
            Value::I64(value) | Value::Enum { value, .. } => {
                write_signed(output, *value, spec, locale);
            }
            Value::U8(value) => write_uint(output, u64::from(*value), false, spec, locale),
            Value::U16(value) => write_uint(output, u64::from(*value), false, spec, locale),
            Value::U32(value) => write_uint(output, u64::from(*value), false, spec, locale),
            Value::U64(value) | Value::Time(value) | Value::Pointer(value) => {
                write_uint(output, *value, false, spec, locale);
            }
            Value::F32(value) => write_float(output, Float::Single(*value), spec, locale),
            Value::F64(value) => write_float(output, Float::Double(*value), spec, locale),
            Value::Null => write_str(output, "null", spec),
            Value::Bool(value) => {
                if matches!(spec.ty, 0 | b's') {
                    if spec.locale {
                        let name = if *value {
                            locale.true_name
                        } else {
                            locale.false_name
                        };
                        // numpunct<char> boolean labels use byte width, unlike text payloads.
                        pad_text(output, name, name.len(), spec);
                    } else {
                        write_str(output, if *value { "true" } else { "false" }, spec);
                    }
                } else {
                    write_uint(output, u64::from(*value), false, spec, locale);
                }
            }
            Value::Str(id) => write_str(output, self.string(*id)?, spec),
            Value::Text(text) => write_str(output, text, spec),
            Value::Bytes(bytes) => {
                let mut text = String::new();
                for byte in bytes {
                    write!(text, "{byte:02x}").unwrap();
                }
                write_str(output, &text, spec);
            }
            Value::Bits { width, data }
            | Value::Logic { width, data }
            | Value::Logic9 { width, data } => {
                let width = u32::try_from(*width)
                    .map_err(|_| Error::invalid("logic value exceeds rendering width"))?;
                let states = match value {
                    Value::Bits { .. } => 2,
                    Value::Logic { .. } => 4,
                    _ => 9,
                };
                write_str(
                    output,
                    &crate::signal::SignalValue::Bits {
                        width,
                        states,
                        data,
                    }
                    .to_ascii(),
                    spec,
                );
            }
            Value::Fixed { raw, scale } => write_str(output, &format!("{raw}e{scale}"), spec),
            Value::UFixed { raw, scale } => write_str(output, &format!("{raw}e{scale}"), spec),
            Value::List(_) | Value::Map(_) => output.push_str("{?}"),
        }
        Ok(())
    }
    pub fn format_value(&self, value: &Value) -> Result<String> {
        let mut output = RenderBuffer::default();
        match value {
            Value::List(values) => {
                output.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        output.push_str(", ");
                    }
                    output.push_str(&self.format_value(value)?);
                }
                output.push(']');
            }
            Value::Map(attributes) => {
                output.push('{');
                for (index, attribute) in attributes.iter().enumerate() {
                    if index != 0 {
                        output.push_str(", ");
                    }
                    write!(
                        output,
                        "{}: {}",
                        self.string(attribute.key)?,
                        self.format_value(&attribute.value)?
                    )
                    .unwrap();
                }
                output.push('}');
            }
            Value::Text(text) => write!(output, "{text:?}").unwrap(),
            Value::Str(id) => write!(output, "{:?}", self.string(*id)?).unwrap(),
            _ => self.write_scalar(
                &mut output,
                value,
                &parse_spec("").unwrap(),
                &FormatLocale::default(),
            )?,
        }
        output.text()
    }
    pub fn format_payload(&self, payload: &PayloadView) -> Result<String> {
        if !std::sync::Arc::ptr_eq(&self.types, &payload.types) {
            return Err(Error::invalid("payload schema belongs to another reader"));
        }
        let datatype = payload.schema().get(payload.datatype())?;
        match &datatype.kind {
            TypeKind::Scalar(_) | TypeKind::Enum { .. } => self.format_value(&payload.scalar()?),
            TypeKind::Struct(fields) => {
                let mut output = String::from("{");
                for (index, field) in fields.iter().enumerate() {
                    if index != 0 {
                        output.push_str(", ");
                    }
                    write!(
                        output,
                        "{}: {}",
                        field.name,
                        self.format_payload(&payload.at(index)?)?
                    )
                    .unwrap();
                }
                output.push('}');
                Ok(output)
            }
            TypeKind::Array { .. } | TypeKind::Sequence { .. } => {
                let mut output = String::from("[");
                for index in 0..payload.len()? {
                    if index != 0 {
                        output.push_str(", ");
                    }
                    output.push_str(&self.format_payload(&payload.at(index)?)?);
                }
                output.push(']');
                Ok(output)
            }
        }
    }
}
fn write_signed(output: &mut RenderBuffer, value: i64, spec: &Spec, locale: &FormatLocale<'_>) {
    write_uint(output, value.unsigned_abs(), value < 0, spec, locale);
}
