//! Bounded text for recorded values. Every panel that shows a transaction
//! attribute, event or stage formats it here, so the table preview, the
//! transaction panel and the clipboard agree on radix, escaping and the
//! byte budget that stops a pathological value from being materialized.

use super::transactions::AttributeValue;

/// Bytes of one projected value (a table cell, one detail row).
pub const PREVIEW_BYTES: usize = 256;
/// Bytes one clipboard copy may produce.
pub const COPY_BYTES: usize = 64 * 1024;

/// How an integer attribute is written. Cycled per key by the reader, never
/// recorded in the trace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Radix {
    #[default]
    Dec,
    Hex,
    Bin,
}

impl Radix {
    pub const ALL: [Radix; 3] = [Radix::Dec, Radix::Hex, Radix::Bin];

    pub fn name(self) -> &'static str {
        match self {
            Radix::Dec => "dec",
            Radix::Hex => "hex",
            Radix::Bin => "bin",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|radix| radix.name() == name)
    }

    pub fn next(self) -> Self {
        match self {
            Radix::Dec => Radix::Hex,
            Radix::Hex => Radix::Bin,
            Radix::Bin => Radix::Dec,
        }
    }

    /// A signed integer; negative values keep their sign before the base.
    pub fn format_i64(self, value: i64) -> String {
        let magnitude = value.unsigned_abs();
        let sign = if value < 0 { "-" } else { "" };
        format!("{sign}{}", self.format_u64(magnitude))
    }

    pub fn format_u64(self, value: u64) -> String {
        match self {
            Radix::Dec => value.to_string(),
            Radix::Hex => format!("0x{value:x}"),
            Radix::Bin => format!("0b{value:b}"),
        }
    }
}

/// Whether a value is an integer a radix applies to.
pub fn is_integer(value: &AttributeValue) -> bool {
    matches!(
        value,
        AttributeValue::I64(_)
            | AttributeValue::U64(_)
            | AttributeValue::Time(_)
            | AttributeValue::Pointer(_)
    )
}

/// One attribute value in at most `limit` bytes, decimal integers.
pub fn format_attribute(value: &AttributeValue, limit: usize) -> String {
    format_attribute_radix(value, Radix::Dec, limit)
}

/// One attribute value in at most `limit` bytes, integers in `radix`.
pub fn format_attribute_radix(value: &AttributeValue, radix: Radix, limit: usize) -> String {
    let text = match value {
        AttributeValue::Null => "null".into(),
        AttributeValue::Bool(v) => v.to_string(),
        AttributeValue::I64(v) => radix.format_i64(*v),
        AttributeValue::U64(v) | AttributeValue::Time(v) | AttributeValue::Pointer(v) => {
            radix.format_u64(*v)
        }
        AttributeValue::F64(v) => v.to_string(),
        AttributeValue::Text(v) => return truncate_ref(v, limit),
        AttributeValue::Bytes(v) => format!("{} bytes", v.len()),
        AttributeValue::Logic { width, states, .. } => format!("logic[{width}]/{states}-state"),
        AttributeValue::Enum { value, name } => format!("{name} ({value})"),
        AttributeValue::Fixed { raw, scale } => format!("{raw}e{scale}"),
        AttributeValue::UFixed { raw, scale } => format!("{raw}e{scale}"),
        AttributeValue::List(values) => format!("list[{}]", values.len()),
        AttributeValue::Map(values) => format!("map[{}]", values.len()),
    };
    truncate(text, limit)
}

/// Bytes the complete value would need, where a projection can lose data.
pub fn attribute_value_min_bytes(value: &AttributeValue) -> usize {
    match value {
        AttributeValue::Text(value) => value.len(),
        _ => 0,
    }
}

/// `key=value · key=value` in at most `limit` bytes, and whether it was cut.
pub fn format_attributes(attributes: &[(String, AttributeValue)], limit: usize) -> (String, bool) {
    if attributes.is_empty() {
        return ("no attributes".into(), false);
    }
    let mut out = String::new();
    let mut cut = false;
    for (index, (key, value)) in attributes.iter().enumerate() {
        if index > 0 {
            push_limited(&mut out, " · ", limit);
        }
        let before = out.len();
        push_limited(&mut out, key, limit);
        cut |= out.len().saturating_sub(before) < key.len();
        push_limited(&mut out, "=", limit);
        let remaining = limit.saturating_sub(out.len());
        let formatted = format_attribute(value, remaining);
        cut |= attribute_value_min_bytes(value) > formatted.len();
        push_limited(&mut out, &formatted, limit);
        if out.len() >= limit {
            cut |= index + 1 < attributes.len();
            break;
        }
    }
    (out, cut)
}

/// A dotted path in at most `limit` bytes; sets `truncated` when it was cut.
pub fn join_path_limited(path: &[String], limit: usize, truncated: &mut bool) -> String {
    let mut out = String::new();
    for (index, part) in path.iter().enumerate() {
        if index > 0 {
            push_limited(&mut out, ".", limit);
        }
        let before = out.len();
        push_limited(&mut out, part, limit);
        if out.len() - before < part.len() {
            *truncated = true;
            break;
        }
    }
    out
}

pub fn truncate(mut text: String, limit: usize) -> String {
    if text.len() <= limit {
        return text;
    }
    let suffix = "…";
    let mut end = limit.saturating_sub(suffix.len()).min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str(suffix);
    text
}

pub fn truncate_ref(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let suffix = "…";
    let mut end = limit.saturating_sub(suffix.len()).min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + suffix.len());
    out.push_str(&text[..end]);
    out.push_str(suffix);
    out
}

pub fn push_limited(out: &mut String, value: &str, limit: usize) {
    if out.len() >= limit {
        return;
    }
    let room = limit - out.len();
    if value.len() <= room {
        out.push_str(value);
        return;
    }
    let mut end = room;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    out.push_str(&value[..end]);
}

/// Append without truncating: a clipboard copy is complete or refused.
pub fn append_exact(out: &mut String, value: &str, limit: usize) -> anyhow::Result<()> {
    anyhow::ensure!(
        out.len()
            .checked_add(value.len())
            .is_some_and(|size| size <= limit),
        "Record exceeds the 64 KiB clipboard limit."
    );
    out.push_str(value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radix_cycles_and_keeps_the_sign_outside_the_base() {
        assert_eq!(Radix::Dec.next().next().next(), Radix::Dec);
        assert_eq!(Radix::Hex.format_u64(0x8000_0008), "0x80000008");
        assert_eq!(Radix::Bin.format_i64(-5), "-0b101");
        assert_eq!(Radix::Dec.format_i64(i64::MIN), i64::MIN.to_string());
        assert_eq!(
            format_attribute_radix(&AttributeValue::Pointer(255), Radix::Hex, 64),
            "0xff"
        );
        assert!(is_integer(&AttributeValue::Time(3)));
        assert!(!is_integer(&AttributeValue::F64(3.0)));
    }

    #[test]
    fn bounded_text_never_splits_a_character() {
        assert_eq!(truncate_ref("ααααα", 5), "α…");
        assert_eq!(truncate("abc".into(), 3), "abc");
        let mut out = String::new();
        push_limited(&mut out, "ααα", 3);
        assert_eq!(out, "α");
        assert!(append_exact(&mut out, "x", 1).is_err());
    }
}
