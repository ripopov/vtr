//! A tolerant JSON parser for `settings.json` (comments, trailing commas, a
//! byte-order mark, CRLF) that records the byte spans of top-level properties,
//! and the surgical edits that change one property while keeping every other
//! byte, comment included.

use std::ops::Range;

/// One top-level property with its byte spans in the source text.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub key: String,
    pub key_span: Range<usize>,
    pub value_span: Range<usize>,
    pub value: serde_json::Value,
    /// 1-based line of the key.
    pub line: usize,
}

/// A parsed document. `braces` is `None` for an empty (blank) text, which is
/// treated as `{}` for resolution and seeded on the first edit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Document {
    pub entries: Vec<Entry>,
    braces: Option<(usize, usize)>,
}

impl Document {
    pub fn get(&self, key: &str) -> Option<&Entry> {
        // Duplicate keys: the last one wins, as in JSON.
        self.entries.iter().rev().find(|e| e.key == key)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub message: String,
    pub offset: usize,
    pub line: usize,
}

impl std::fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

/// 1-based line of a byte offset.
fn line_of(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].matches('\n').count() + 1
}

/// Offset of the first byte of the line containing `at`.
fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |i| i + 1)
}

fn skip_blanks(bytes: &[u8], mut at: usize) -> usize {
    while bytes.get(at).is_some_and(|c| matches!(c, b' ' | b'\t')) {
        at += 1;
    }
    at
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    depth: usize,
}

const MAX_DEPTH: usize = 64;

impl<'a> Parser<'a> {
    fn error(&self, message: impl Into<String>, at: usize) -> SyntaxError {
        SyntaxError {
            message: message.into(),
            offset: at,
            line: line_of(self.text, at),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn starts_with(&self, s: &str) -> bool {
        self.bytes[self.pos..].starts_with(s.as_bytes())
    }

    /// Skip whitespace and comments.
    fn skip(&mut self) -> Result<(), SyntaxError> {
        loop {
            while self
                .peek()
                .is_some_and(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r'))
            {
                self.pos += 1;
            }
            if self.starts_with("//") {
                while self.peek().is_some_and(|c| c != b'\n') {
                    self.pos += 1;
                }
            } else if self.starts_with("/*") {
                let start = self.pos;
                match self.text[self.pos + 2..].find("*/") {
                    Some(end) => self.pos += 2 + end + 2,
                    None => return Err(self.error("unterminated block comment", start)),
                }
            } else {
                return Ok(());
            }
        }
    }

    fn string(&mut self) -> Result<String, SyntaxError> {
        let start = self.pos;
        self.pos += 1;
        let mut out = String::new();
        loop {
            let Some(c) = self.peek() else {
                return Err(self.error("unterminated string", start));
            };
            match c {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    let Some(d) = self.bytes.get(self.pos + 1).copied() else {
                        return Err(self.error("unterminated string", start));
                    };
                    self.pos += 2;
                    match d {
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => out.push('\r'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'u' => {
                            let hex = self
                                .text
                                .get(self.pos..self.pos + 4)
                                .and_then(|h| u32::from_str_radix(h, 16).ok())
                                .ok_or_else(|| self.error("invalid \\u escape", self.pos - 2))?;
                            self.pos += 4;
                            let ch = if (0xD800..0xDC00).contains(&hex) && self.starts_with("\\u") {
                                let low = self
                                    .text
                                    .get(self.pos + 2..self.pos + 6)
                                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                                    .filter(|l| (0xDC00..0xE000).contains(l))
                                    .ok_or_else(|| {
                                        self.error("invalid surrogate pair", self.pos)
                                    })?;
                                self.pos += 6;
                                0x10000 + ((hex - 0xD800) << 10) + (low - 0xDC00)
                            } else {
                                hex
                            };
                            out.push(
                                char::from_u32(ch)
                                    .ok_or_else(|| self.error("invalid character", self.pos))?,
                            );
                        }
                        _ => return Err(self.error("invalid escape", self.pos - 2)),
                    }
                }
                b'\n' => return Err(self.error("unterminated string", start)),
                _ => {
                    let ch = self.text[self.pos..].chars().next().unwrap();
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
    }

    fn value(&mut self) -> Result<serde_json::Value, SyntaxError> {
        let start = self.pos;
        match self.peek() {
            None => Err(self.error("unexpected end of file", start)),
            Some(b'"') => Ok(serde_json::Value::String(self.string()?)),
            Some(b'{') => {
                let mut map = serde_json::Map::new();
                for (key, value) in self.object(None)? {
                    map.insert(key, value);
                }
                Ok(serde_json::Value::Object(map))
            }
            Some(b'[') => {
                self.depth += 1;
                if self.depth > MAX_DEPTH {
                    return Err(self.error("nesting too deep", start));
                }
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.skip()?;
                    match self.peek() {
                        Some(b']') => {
                            self.pos += 1;
                            break;
                        }
                        Some(_) => {
                            items.push(self.value()?);
                            self.skip()?;
                            match self.peek() {
                                Some(b',') => self.pos += 1,
                                Some(b']') => {}
                                Some(_) => return Err(self.error("expected ',' or ']'", self.pos)),
                                None => return Err(self.error("unterminated array", start)),
                            }
                        }
                        None => return Err(self.error("unterminated array", start)),
                    }
                }
                self.depth -= 1;
                Ok(serde_json::Value::Array(items))
            }
            Some(b't') if self.starts_with("true") => {
                self.pos += 4;
                Ok(serde_json::Value::Bool(true))
            }
            Some(b'f') if self.starts_with("false") => {
                self.pos += 5;
                Ok(serde_json::Value::Bool(false))
            }
            Some(b'n') if self.starts_with("null") => {
                self.pos += 4;
                Ok(serde_json::Value::Null)
            }
            Some(c) if c == b'-' || c.is_ascii_digit() => {
                let mut end = self.pos + 1;
                while self.bytes.get(end).is_some_and(|c| {
                    c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-')
                }) {
                    end += 1;
                }
                let literal = &self.text[self.pos..end];
                let value: serde_json::Value = serde_json::from_str(literal)
                    .map_err(|_| self.error(format!("invalid number {literal}"), start))?;
                self.pos = end;
                Ok(value)
            }
            Some(_) => Err(self.error("unexpected character", start)),
        }
    }

    /// Parse `{ ... }` at the cursor; returns (key, value) pairs and records
    /// entries with spans when `record` is set (top level only).
    fn object(
        &mut self,
        mut record: Option<&mut Vec<Entry>>,
    ) -> Result<Vec<(String, serde_json::Value)>, SyntaxError> {
        let start = self.pos;
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error("nesting too deep", start));
        }
        self.pos += 1;
        let mut pairs = Vec::new();
        loop {
            self.skip()?;
            match self.peek() {
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                Some(b'"') => {
                    let key_start = self.pos;
                    let key = self.string()?;
                    let key_span = key_start..self.pos;
                    self.skip()?;
                    if self.peek() != Some(b':') {
                        return Err(self.error("expected ':'", self.pos));
                    }
                    self.pos += 1;
                    self.skip()?;
                    let value_start = self.pos;
                    let value = self.value()?;
                    let value_span = value_start..self.pos;
                    if let Some(entries) = record.as_deref_mut() {
                        entries.push(Entry {
                            key: key.clone(),
                            line: line_of(self.text, key_start),
                            key_span,
                            value_span,
                            value: value.clone(),
                        });
                    }
                    pairs.push((key, value));
                    self.skip()?;
                    match self.peek() {
                        Some(b',') => self.pos += 1,
                        Some(b'}') => {}
                        Some(_) => return Err(self.error("expected ',' or '}'", self.pos)),
                        None => return Err(self.error("unterminated object", start)),
                    }
                }
                Some(_) => return Err(self.error("expected a property name in quotes", self.pos)),
                None => return Err(self.error("unterminated object", start)),
            }
        }
        self.depth -= 1;
        Ok(pairs)
    }
}

/// Parse a settings document. Blank text is an empty document.
pub fn parse(text: &str) -> Result<Document, SyntaxError> {
    let mut parser = Parser {
        text,
        bytes: text.as_bytes(),
        pos: 0,
        depth: 0,
    };
    if text.starts_with('\u{feff}') {
        parser.pos = '\u{feff}'.len_utf8();
    }
    parser.skip()?;
    if parser.peek().is_none() {
        return Ok(Document::default());
    }
    if parser.peek() != Some(b'{') {
        return Err(parser.error("expected '{' at the top level", parser.pos));
    }
    let open = parser.pos;
    let mut entries = Vec::new();
    parser.object(Some(&mut entries))?;
    let close = parser.pos - 1;
    parser.skip()?;
    if parser.peek().is_some() {
        return Err(parser.error("unexpected text after the closing brace", parser.pos));
    }
    Ok(Document {
        entries,
        braces: Some((open, close)),
    })
}

/// Whether `range` of `text` holds a comma outside comments.
fn has_comma(text: &str, range: Range<usize>) -> bool {
    let slice = &text[range];
    let bytes = slice.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if slice[i..].starts_with("//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if slice[i..].starts_with("/*") {
            i = slice[i + 2..]
                .find("*/")
                .map_or(bytes.len(), |e| i + 2 + e + 2);
        } else if bytes[i] == b',' {
            return true;
        } else {
            i += 1;
        }
    }
    false
}

fn newline(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// The indentation of the first property, or two spaces.
fn indent(text: &str, doc: &Document) -> String {
    if let Some(first) = doc.entries.first() {
        let prefix = &text[line_start(text, first.key_span.start)..first.key_span.start];
        if !prefix.is_empty() && prefix.chars().all(|c| c == ' ' || c == '\t') {
            return prefix.to_owned();
        }
    }
    "  ".into()
}

/// Set `key` to `value`, replacing only the value span of a present key or
/// inserting one property before the closing brace. Inserted properties never
/// carry a trailing comma. `seed` are properties written into a document
/// that has no braces yet (the `$schema` line).
pub fn set(
    text: &str,
    doc: &Document,
    key: &str,
    value: &serde_json::Value,
    seed: &[(&str, serde_json::Value)],
) -> String {
    let json = value.to_string();
    if let Some(entry) = doc.get(key) {
        let mut out = String::with_capacity(text.len() + json.len());
        out.push_str(&text[..entry.value_span.start]);
        out.push_str(&json);
        out.push_str(&text[entry.value_span.end..]);
        return out;
    }
    let nl = newline(text);
    let Some((_, close)) = doc.braces else {
        let mut out = String::from("{");
        for (k, v) in seed {
            out.push_str(&format!("{nl}  \"{k}\": {v},"));
        }
        out.push_str(&format!("{nl}  \"{key}\": {json}{nl}}}{nl}"));
        return out;
    };
    let mut out = text.to_owned();
    let mut close = close;
    if let Some(last) = doc.entries.last()
        && !has_comma(text, last.value_span.end..close)
    {
        out.insert(last.value_span.end, ',');
        close += 1;
    }
    let ind = indent(text, doc);
    let before = out[..close].trim_end_matches([' ', '\t']).to_owned();
    let after = &out[close..];
    let mut result = before;
    if !result.ends_with('\n') {
        result.push_str(nl);
    }
    result.push_str(&format!("{ind}\"{key}\": {json}{nl}"));
    result.push_str(after);
    result
}

/// Remove the property `key` with its line (when the key started the line),
/// its trailing comma and a same-line comment. Removing the last property
/// also drops the comma that preceded it, so a strictly valid document stays
/// strictly valid. A document left with nothing but whitespace becomes `{}`.
pub fn remove(text: &str, doc: &Document, key: &str) -> String {
    let Some(entry) = doc.get(key) else {
        return text.to_owned();
    };
    let Some((open, close)) = doc.braces else {
        return text.to_owned();
    };
    let bytes = text.as_bytes();
    let line_start = line_start(text, entry.key_span.start);
    let only_ws = text[line_start..entry.key_span.start]
        .chars()
        .all(|c| c == ' ' || c == '\t');
    let start = if only_ws {
        line_start
    } else {
        entry.key_span.start
    };
    // `[ \t]*,?[ \t]*(//[^\n]*)?\r?\n?`
    let mut end = skip_blanks(bytes, entry.value_span.end);
    let had_comma = bytes.get(end) == Some(&b',');
    if had_comma {
        end += 1;
    }
    end = skip_blanks(bytes, end);
    if text[end..].starts_with("//") {
        while bytes.get(end).is_some_and(|c| *c != b'\n') {
            end += 1;
        }
    }
    if only_ws {
        if bytes.get(end) == Some(&b'\r') {
            end += 1;
        }
        if bytes.get(end) == Some(&b'\n') {
            end += 1;
        }
    }
    let mut out = String::with_capacity(text.len());
    // The removed entry was last without a trailing comma: drop the previous comma.
    let is_last = doc
        .entries
        .last()
        .is_some_and(|last| last.key_span == entry.key_span);
    if is_last && !had_comma && !has_comma(text, entry.value_span.end..close) {
        let previous = doc.entries.iter().rev().nth(1);
        if let Some(previous) = previous
            && let Some(comma) = text[previous.value_span.end..start].find(',')
        {
            let comma_at = previous.value_span.end + comma;
            out.push_str(&text[..comma_at]);
            out.push_str(&text[comma_at + 1..start]);
            out.push_str(&text[end..]);
            return finish_remove(out, open);
        }
    }
    out.push_str(&text[..start]);
    out.push_str(&text[end..]);
    finish_remove(out, open)
}

fn finish_remove(out: String, open: usize) -> String {
    if let Ok(doc) = parse(&out)
        && doc.entries.is_empty()
        && let Some((o, c)) = doc.braces
        && o == open
        && out[o + 1..c].trim().is_empty()
    {
        return format!("{{}}{}", newline(&out));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SAMPLE: &str = "{\n  // Volna user settings.\n  \"$schema\": \"./settings.schema.json\",\n\n  \"appearance.theme\": \"gruvbox-dark\",\n  \"waves.animation\": \"reduced\",   // \"on\" | \"reduced\" | \"off\"\n  \"waves.snapPixels\": 8,\n}\n";

    #[test]
    fn parses_comments_trailing_commas_bom_and_records_spans() {
        let doc = parse(SAMPLE).unwrap();
        assert_eq!(doc.entries.len(), 4);
        let snap = doc.get("waves.snapPixels").unwrap();
        assert_eq!(snap.value, json!(8));
        assert_eq!(&SAMPLE[snap.value_span.clone()], "8");
        assert_eq!(&SAMPLE[snap.key_span.clone()], "\"waves.snapPixels\"");
        assert_eq!(snap.line, 7);
        let with_bom = format!("\u{feff}{SAMPLE}");
        assert_eq!(parse(&with_bom).unwrap().entries.len(), 4);
        assert_eq!(parse("").unwrap(), Document::default());
        assert_eq!(parse("  // nothing\n").unwrap(), Document::default());
        let crlf = SAMPLE.replace('\n', "\r\n");
        assert_eq!(parse(&crlf).unwrap().entries.len(), 4);
        let nested =
            "{\"a\": {\"b\": [1, 2, {\"c\": \"d\\u00e9\"}], /* x */ \"e\": null}, \"f\": -1.5e3}";
        let doc = parse(nested).unwrap();
        assert_eq!(doc.entries[0].value["b"][2]["c"], json!("dé"));
        assert_eq!(doc.entries[1].value, json!(-1500.0));
        let dup = parse("{\"a\": 1, \"a\": 2}").unwrap();
        assert_eq!(dup.get("a").unwrap().value, json!(2));
    }

    #[test]
    fn syntax_errors_name_the_line() {
        let error = parse("{\n  \"a\": 1,\n  \"b\": tru\n}").unwrap_err();
        assert_eq!(error.line, 3);
        assert!(parse("{\"a\": \"unterminated}").is_err());
        assert!(parse("{\"a\": 1} trailing").is_err());
        assert!(parse("[1]").is_err());
        assert!(parse("{\"a\": /* open").is_err());
        assert!(parse("{a: 1}").is_err());
    }

    #[test]
    fn set_replaces_only_the_value_span() {
        let doc = parse(SAMPLE).unwrap();
        let out = set(SAMPLE, &doc, "waves.snapPixels", &json!(12), &[]);
        assert_eq!(
            out,
            SAMPLE.replace("\"waves.snapPixels\": 8", "\"waves.snapPixels\": 12")
        );
        let out = set(SAMPLE, &doc, "waves.animation", &json!("off"), &[]);
        assert!(out.contains("\"waves.animation\": \"off\",   // \"on\""));
    }

    #[test]
    fn set_inserts_with_document_indent_and_no_trailing_comma() {
        let doc = parse(SAMPLE).unwrap();
        let out = set(SAMPLE, &doc, "panels.linkByDefault", &json!(false), &[]);
        assert!(
            out.ends_with("  \"waves.snapPixels\": 8,\n  \"panels.linkByDefault\": false\n}\n")
        );
        let strict = "{\n    \"a\": 1\n}";
        let doc = parse(strict).unwrap();
        let out = set(strict, &doc, "b", &json!("x"), &[]);
        assert_eq!(out, "{\n    \"a\": 1,\n    \"b\": \"x\"\n}");
        serde_json::from_str::<serde_json::Value>(&out).unwrap();
        // A comment between the last value and the brace keeps its place.
        let commented = "{\n  \"a\": 1 // one\n  // trailing\n}\n";
        let doc = parse(commented).unwrap();
        let out = set(commented, &doc, "b", &json!(2), &[]);
        assert_eq!(out, "{\n  \"a\": 1, // one\n  // trailing\n  \"b\": 2\n}\n");
        let empty = parse("").unwrap();
        assert_eq!(set("", &empty, "a", &json!(1), &[]), "{\n  \"a\": 1\n}\n");
        assert_eq!(
            set(
                "",
                &empty,
                "a",
                &json!(1),
                &[("$schema", json!("./s.json"))]
            ),
            "{\n  \"$schema\": \"./s.json\",\n  \"a\": 1\n}\n"
        );
        let braces = parse("{}").unwrap();
        assert_eq!(set("{}", &braces, "a", &json!(1), &[]), "{\n  \"a\": 1\n}");
        let crlf = "{\r\n  \"a\": 1\r\n}\r\n";
        let doc = parse(crlf).unwrap();
        assert_eq!(
            set(crlf, &doc, "b", &json!(2), &[]),
            "{\r\n  \"a\": 1,\r\n  \"b\": 2\r\n}\r\n"
        );
    }

    #[test]
    fn remove_takes_one_property_and_keeps_the_rest_valid() {
        let doc = parse(SAMPLE).unwrap();
        let out = remove(SAMPLE, &doc, "waves.animation");
        assert_eq!(
            out,
            SAMPLE.replace(
                "  \"waves.animation\": \"reduced\",   // \"on\" | \"reduced\" | \"off\"\n",
                ""
            )
        );
        let strict = "{\n  \"a\": 1,\n  \"b\": 2\n}";
        let doc = parse(strict).unwrap();
        assert_eq!(remove(strict, &doc, "b"), "{\n  \"a\": 1\n}");
        assert_eq!(remove(strict, &doc, "a"), "{\n  \"b\": 2\n}");
        let single = "{\n  \"a\": 1\n}\n";
        assert_eq!(remove(single, &parse(single).unwrap(), "a"), "{}\n");
        let keep_comment = "{\n  // keep me\n  \"a\": 1\n}\n";
        assert_eq!(
            remove(keep_comment, &parse(keep_comment).unwrap(), "a"),
            "{\n  // keep me\n}\n"
        );
        let inline = "{ \"a\": 1, \"b\": 2 }";
        let out = remove(inline, &parse(inline).unwrap(), "a");
        serde_json::from_str::<serde_json::Value>(&out).unwrap();
        assert_eq!(remove(SAMPLE, &doc, "missing"), SAMPLE);
    }

    #[test]
    fn random_edits_keep_the_document_parsable_and_consistent() {
        let keys = ["a.b", "c.d", "e.f", "g.h"];
        let mut text = SAMPLE.to_owned();
        let mut expected: std::collections::BTreeMap<&str, serde_json::Value> = Default::default();
        let mut seed = 0x9e3779b97f4a7c15u64;
        for _ in 0..200 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let key = keys[(seed % 4) as usize];
            let doc = parse(&text).unwrap();
            if seed.is_multiple_of(3) {
                text = remove(&text, &doc, key);
                expected.remove(key);
            } else {
                let value = json!((seed >> 8) % 100);
                text = set(&text, &doc, key, &value, &[]);
                expected.insert(key, value);
            }
            let doc = parse(&text).unwrap();
            for key in keys {
                assert_eq!(doc.get(key).map(|e| &e.value), expected.get(key), "{text}");
            }
            assert_eq!(doc.get("waves.snapPixels").unwrap().value, json!(8));
        }
    }
}
