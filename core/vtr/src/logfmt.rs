//! Rendering of log records: format strings with `{}` placeholders.
//!
//! The placeholder language is the common subset of Rust's `std::fmt` and
//! C++ `std::format`: `{}` (next argument), `{2}` (explicit index),
//! `{:spec}` with `spec = [[fill]align][sign][#][0][width][.precision][type]`,
//! `align` one of `<`, `^`, `>`, `type` one of `x X o b e E f F s ?`, and
//! `{{` / `}}` for literal braces. Integers format two's complement in the
//! non-decimal radixes (Rust semantics); floats without a precision print
//! their shortest round-trip form. Rendering allocates nothing except for
//! floats that need width padding.

use crate::logblock::{LogArg, LogRecord};
use crate::strings::StringTable;
use std::fmt::Write;

#[derive(Clone, Copy, Debug)]
struct Spec {
    fill: char,
    /// 0 = default, else `<`, `^` or `>`.
    align: u8,
    sign_plus: bool,
    alt: bool,
    zero: bool,
    width: usize,
    precision: Option<usize>,
    /// 0 = none.
    ty: u8,
}

fn is_align(c: char) -> bool {
    matches!(c, '<' | '^' | '>')
}

fn parse_spec(s: &str) -> Spec {
    let mut sp = Spec { fill: ' ', align: 0, sign_plus: false, alt: false, zero: false, width: 0, precision: None, ty: 0 };
    let b = s.as_bytes();
    let mut i = 0;
    let mut chars = s.chars();
    let c0 = chars.next();
    let c1 = chars.next();
    if let (Some(f), Some(a)) = (c0, c1) {
        if is_align(a) {
            sp.fill = f;
            sp.align = a as u8;
            i = f.len_utf8() + 1;
        }
    }
    if i == 0 {
        if let Some(a) = c0 {
            if is_align(a) {
                sp.align = a as u8;
                i = 1;
            }
        }
    }
    if i < b.len() && b[i] == b'+' {
        sp.sign_plus = true;
        i += 1;
    } else if i < b.len() && b[i] == b'-' {
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
        sp.width = sp.width * 10 + (b[i] - b'0') as usize;
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let mut p = 0usize;
        while i < b.len() && b[i].is_ascii_digit() {
            p = p * 10 + (b[i] - b'0') as usize;
            i += 1;
        }
        sp.precision = Some(p);
    }
    if i < b.len() {
        sp.ty = b[i];
    }
    sp
}

fn pad_number(out: &mut String, sign: &str, prefix: &str, body: &str, sp: &Spec) {
    let len = sign.len() + prefix.len() + body.len();
    if sp.zero && sp.align == 0 {
        out.push_str(sign);
        out.push_str(prefix);
        for _ in len..sp.width {
            out.push('0');
        }
        out.push_str(body);
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
    out.push_str(body);
    for _ in 0..right {
        out.push(sp.fill);
    }
}

fn write_uint(out: &mut String, v: u64, negative: bool, sp: &Spec) {
    let (prefix, radix, upper) = match sp.ty {
        b'x' => ("0x", 16u64, false),
        b'X' => ("0x", 16, true),
        b'o' => ("0o", 8, false),
        b'b' => ("0b", 2, false),
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
        buf[pos] = if d < 10 { b'0' + d } else if upper { b'A' + d - 10 } else { b'a' + d - 10 };
        x /= radix;
    }
    let sign = if negative {
        "-"
    } else if sp.sign_plus {
        "+"
    } else {
        ""
    };
    let prefix = if sp.alt { prefix } else { "" };
    // ASCII digits only.
    let body = unsafe { std::str::from_utf8_unchecked(&buf[pos..]) };
    pad_number(out, sign, prefix, body, sp);
}

fn write_str(out: &mut String, s: &str, sp: &Spec) {
    let s = match sp.precision {
        Some(p) => match s.char_indices().nth(p) {
            Some((i, _)) => &s[..i],
            None => s,
        },
        None => s,
    };
    if sp.width == 0 {
        out.push_str(s);
        return;
    }
    let len = s.chars().count();
    let pad = sp.width.saturating_sub(len);
    let (left, right) = match sp.align {
        b'>' => (pad, 0),
        b'^' => (pad / 2, pad - pad / 2),
        _ => (0, pad),
    };
    for _ in 0..left {
        out.push(sp.fill);
    }
    out.push_str(s);
    for _ in 0..right {
        out.push(sp.fill);
    }
}

fn write_float_body(out: &mut String, v: f64, sp: &Spec) {
    let a = v.abs();
    let _ = match (sp.ty, sp.precision) {
        (b'e', Some(p)) => write!(out, "{:.*e}", p, a),
        (b'e', None) => write!(out, "{:e}", a),
        (b'E', Some(p)) => write!(out, "{:.*E}", p, a),
        (b'E', None) => write!(out, "{:E}", a),
        (b'f' | b'F', None) => write!(out, "{:.6}", a),
        (_, Some(p)) => write!(out, "{:.*}", p, a),
        (_, None) => write!(out, "{}", a),
    };
}

fn write_float(out: &mut String, v: f64, sp: &Spec) {
    let sign = if v.is_sign_negative() && !v.is_nan() {
        "-"
    } else if sp.sign_plus {
        "+"
    } else {
        ""
    };
    if sp.width == 0 {
        out.push_str(sign);
        write_float_body(out, v, sp);
        return;
    }
    let mut body = String::with_capacity(32);
    write_float_body(&mut body, v, sp);
    pad_number(out, sign, "", &body, sp);
}

/// Writes one argument according to `spec`.
pub fn write_arg(out: &mut String, a: LogArg, spec: &str, strings: &StringTable) {
    write_arg_spec(out, a, &parse_spec(spec), strings)
}

fn write_arg_spec(out: &mut String, a: LogArg, sp: &Spec, strings: &StringTable) {
    match a {
        LogArg::Bool(b) => write_str(out, if b { "true" } else { "false" }, sp),
        LogArg::I64(v) => {
            if matches!(sp.ty, b'x' | b'X' | b'o' | b'b') {
                write_uint(out, v as u64, false, sp);
            } else {
                write_uint(out, v.unsigned_abs(), v < 0, sp);
            }
        }
        LogArg::U64(v) | LogArg::Time(v) => write_uint(out, v, false, sp),
        LogArg::Pointer(v) => {
            if sp.ty != 0 {
                write_uint(out, v, false, sp);
            } else {
                let s2 = Spec { alt: true, ty: b'x', ..*sp };
                write_uint(out, v, false, &s2);
            }
        }
        LogArg::F64(v) => write_float(out, v, sp),
        LogArg::Str(s) => write_str(out, strings.get(s), sp),
        LogArg::Text(s) => write_str(out, s, sp),
        LogArg::Bytes(b) => {
            const HEX: &[u8; 16] = b"0123456789abcdef";
            if sp.width == 0 && sp.precision.is_none() {
                for x in b {
                    out.push(HEX[(x >> 4) as usize] as char);
                    out.push(HEX[(x & 15) as usize] as char);
                }
            } else {
                let mut h = String::with_capacity(b.len() * 2);
                for x in b {
                    h.push(HEX[(x >> 4) as usize] as char);
                    h.push(HEX[(x & 15) as usize] as char);
                }
                write_str(out, &h, sp);
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Seg {
    /// Literal text `fmt[a..b]`.
    Lit(u32, u32),
    /// Argument `index` rendered with `spec`.
    Arg(u32, Spec),
    /// Placeholder with an unparsable index: renders `{?}`.
    Bad,
}

/// A format string parsed once (per log site) into literal and argument segments.
#[derive(Clone, Debug, Default)]
pub struct ParsedFmt {
    segs: Vec<Seg>,
}

impl ParsedFmt {
    pub fn parse(fmt: &str) -> ParsedFmt {
        let b = fmt.as_bytes();
        let mut segs = Vec::new();
        let mut i = 0;
        let mut next = 0u32;
        let mut lit_start = 0usize;
        let lit = |segs: &mut Vec<Seg>, a: usize, b: usize| {
            if a < b {
                segs.push(Seg::Lit(a as u32, b as u32));
            }
        };
        while i < b.len() {
            match b[i] {
                b'{' => {
                    lit(&mut segs, lit_start, i);
                    if i + 1 < b.len() && b[i + 1] == b'{' {
                        segs.push(Seg::Lit(i as u32, i as u32 + 1));
                        i += 2;
                        lit_start = i;
                        continue;
                    }
                    let close = match fmt[i + 1..].find('}') {
                        Some(c) => i + 1 + c,
                        None => {
                            lit(&mut segs, i, b.len());
                            return ParsedFmt { segs };
                        }
                    };
                    let inner = &fmt[i + 1..close];
                    let (idx_s, spec) = match inner.find(':') {
                        Some(c) => (&inner[..c], &inner[c + 1..]),
                        None => (inner, ""),
                    };
                    let idx = if idx_s.is_empty() {
                        let k = next;
                        next += 1;
                        Some(k)
                    } else {
                        idx_s.parse::<u32>().ok()
                    };
                    segs.push(match idx {
                        Some(k) => Seg::Arg(k, parse_spec(spec)),
                        None => Seg::Bad,
                    });
                    i = close + 1;
                    lit_start = i;
                }
                b'}' => {
                    lit(&mut segs, lit_start, i);
                    segs.push(Seg::Lit(i as u32, i as u32 + 1));
                    i += if i + 1 < b.len() && b[i + 1] == b'}' { 2 } else { 1 };
                    lit_start = i;
                }
                _ => i += 1,
            }
        }
        lit(&mut segs, lit_start, b.len());
        ParsedFmt { segs }
    }

    /// Renders with the original format string `fmt` (for the literal pieces)
    /// and the arguments returned by `arg(index)`.
    pub fn render<'a>(&self, fmt: &str, mut arg: impl FnMut(usize) -> Option<LogArg<'a>>, strings: &StringTable, out: &mut String) {
        for s in &self.segs {
            match *s {
                Seg::Lit(a, b) => out.push_str(&fmt[a as usize..b as usize]),
                Seg::Arg(k, ref sp) => match arg(k as usize) {
                    Some(a) => write_arg_spec(out, a, sp, strings),
                    None => out.push_str("{?}"),
                },
                Seg::Bad => out.push_str("{?}"),
            }
        }
    }
}

/// Renders `fmt` with the arguments returned by `arg(index)`. A placeholder
/// whose argument does not exist renders as `{?}`. Parses `fmt` on every
/// call; readers use the per-site [`ParsedFmt`].
pub fn format_message<'a>(fmt: &str, arg: impl FnMut(usize) -> Option<LogArg<'a>>, strings: &StringTable, out: &mut String) {
    ParsedFmt::parse(fmt).render(fmt, arg, strings, out)
}

impl<'a> LogRecord<'a> {
    /// Appends the rendered message (format string with arguments substituted).
    pub fn format_into(&self, strings: &StringTable, out: &mut String) {
        self.fmt.render(strings.get(self.site.fmt), |i| self.arg(i), strings, out)
    }
    /// The rendered message.
    pub fn format(&self, strings: &StringTable) -> String {
        let mut s = String::new();
        self.format_into(strings, &mut s);
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(fmt: &str, args: &[LogArg]) -> String {
        let mut out = String::new();
        format_message(fmt, |i| args.get(i).copied(), &StringTable::new(), &mut out);
        out
    }

    #[test]
    fn placeholders() {
        assert_eq!(f("a={} b={}", &[LogArg::U64(5), LogArg::Text("x")]), "a=5 b=x");
        assert_eq!(f("{1} {0}", &[LogArg::U64(1), LogArg::U64(2)]), "2 1");
        assert_eq!(f("{{literal}} {}", &[LogArg::I64(-3)]), "{literal} -3");
        assert_eq!(f("{:#x} {:X} {:#010x} {:o} {:#b}", &[LogArg::U64(255), LogArg::U64(255), LogArg::U64(255), LogArg::U64(8), LogArg::U64(5)]), "0xff FF 0x000000ff 10 0b101");
        assert_eq!(f("{:>6}|{:<6}|{:^6}|", &[LogArg::U64(42), LogArg::Text("ab"), LogArg::Text("ab")]), "    42|ab    |  ab  |");
        assert_eq!(f("{:.2} {:08.3} {} {:.2f}", &[LogArg::F64(1.23456), LogArg::F64(-2.5), LogArg::F64(0.5), LogArg::F64(3.0)]), "1.23 -002.500 0.5 3.00");
        assert_eq!(f("{:+} {:x} {}", &[LogArg::I64(7), LogArg::I64(-1), LogArg::I64(0)]), "+7 ffffffffffffffff 0");
        assert_eq!(f("{} {}", &[LogArg::Bool(true), LogArg::Pointer(0x1000)]), "true 0x1000");
        assert_eq!(f("{} {:.3}", &[LogArg::Bytes(&[1, 0xab]), LogArg::Text("abcdef")]), "01ab abc");
        assert_eq!(f("{} {}", &[LogArg::U64(1)]), "1 {?}");
        assert_eq!(f("{:*^7}", &[LogArg::Text("mid")]), "**mid**");
        assert_eq!(f("{:e}", &[LogArg::F64(1500.0)]), "1.5e3");
        assert_eq!(f("{:f} {:>8.1}", &[LogArg::F64(1.5), LogArg::F64(2.25)]), "1.500000      2.2");
        assert_eq!(f("{:>4} {:.1}", &[LogArg::Text("héllo"), LogArg::Text("héllo")]), "héllo h");
    }
}
