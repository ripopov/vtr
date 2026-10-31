//! Schema-guided numeric, floating, text, dictionary, and miscellaneous columns.

use std::collections::{HashMap, HashSet};

use crate::{
    Error, Result,
    schema::{
        DEFAULT_PAYLOAD_LIMIT, PayloadVisitor, Primitive, TypeId, TypeKind, Types, integer_range,
    },
    varint::{put_blob, put_i64, put_u64, unzigzag},
    wire::Cursor,
};

pub struct ArgumentColumns<'a> {
    columns: [Vec<u8>; 5],
    dictionary: Vec<String>,
    text_ids: HashMap<String, u32>,
    string_count: u64,
    shared_text: Option<&'a HashMap<String, u32>>,
}

impl Default for ArgumentColumns<'_> {
    fn default() -> Self {
        Self::new(u64::MAX, None)
    }
}

impl<'a> ArgumentColumns<'a> {
    #[must_use]
    pub fn new(string_count: u64, shared_text: Option<&'a HashMap<String, u32>>) -> Self {
        Self {
            columns: std::array::from_fn(|_| Vec::new()),
            dictionary: Vec::new(),
            text_ids: HashMap::new(),
            string_count,
            shared_text,
        }
    }

    pub fn append(&mut self, types: &Types, datatype: TypeId, payload: &[u8]) -> Result<()> {
        types.validate(datatype, payload, self.string_count)?;
        let mut row = Cursor::new(payload);
        types.walk(datatype, &mut row, self)?;
        row.done()
    }

    #[must_use]
    pub fn finish(mut self) -> [Vec<u8>; 5] {
        put_u64(
            &mut self.columns[3],
            (self.dictionary.len() as u64) << 1 | u64::from(self.shared_text.is_some()),
        );
        for text in self.dictionary {
            put_blob(&mut self.columns[3], text.as_bytes());
        }
        self.columns
    }

    pub(crate) fn encoded_lengths(&self) -> [usize; 5] {
        let mut lengths = self.columns.each_ref().map(Vec::len);
        lengths[3] = crate::varint::len_u64(
            (self.dictionary.len() as u64) << 1 | u64::from(self.shared_text.is_some()),
        ) + self
            .dictionary
            .iter()
            .map(|text| crate::varint::len_u64(text.len() as u64) + text.len())
            .sum::<usize>();
        lengths
    }

    pub(crate) fn checkpoint(&self) -> ([usize; 5], usize) {
        (self.columns.each_ref().map(Vec::len), self.dictionary.len())
    }

    pub(crate) fn restore(&mut self, (lengths, dictionary): ([usize; 5], usize)) {
        for (column, length) in self.columns.iter_mut().zip(lengths) {
            column.truncate(length);
        }
        for text in self.dictionary.drain(dictionary..) {
            self.text_ids.remove(&text);
        }
    }
}

impl PayloadVisitor for ArgumentColumns<'_> {
    fn count(&mut self, count: u32) -> Result<()> {
        put_u64(&mut self.columns[0], u64::from(count));
        Ok(())
    }

    fn scalar(&mut self, primitive: Primitive, row: &mut Cursor<'_>) -> Result<()> {
        match primitive {
            Primitive::Unit => (),
            Primitive::Bool => self.columns[0].push(row.byte()?),
            p if p.integer() || matches!(p, Primitive::Time | Primitive::Pointer) => {
                let width = p.width().unwrap();
                let mut bits = [0; 8];
                bits[..width].copy_from_slice(row.take(width)?);
                let value = u64::from_le_bytes(bits);
                if p.signed() {
                    let shift = 64 - width * 8;
                    put_i64(&mut self.columns[0], (value << shift) as i64 >> shift);
                } else {
                    put_u64(&mut self.columns[0], value);
                }
            }
            p @ (Primitive::F32 | Primitive::F64) => {
                self.columns[1].extend_from_slice(row.take(p.width().unwrap())?);
            }
            Primitive::Str => put_u64(&mut self.columns[4], u64::from(row.u32()?)),
            Primitive::Text | Primitive::Bytes => {
                let size = row.u32()? as usize;
                let bytes = row.take(size)?;
                if primitive == Primitive::Bytes {
                    put_blob(&mut self.columns[4], bytes);
                } else {
                    let text =
                        std::str::from_utf8(bytes).map_err(|_| Error::Corrupt("invalid UTF-8"))?;
                    let index = if let Some(id) = self.shared_text.and_then(|table| table.get(text))
                    {
                        if u64::from(*id) >= self.string_count {
                            return Err(Error::Corrupt("unknown shared text identifier"));
                        }
                        (u64::from(*id) << 1) | 1
                    } else {
                        let id = if let Some(id) = self.text_ids.get(text) {
                            *id
                        } else {
                            let id = u32::try_from(self.dictionary.len())
                                .map_err(|_| Error::Corrupt("argument dictionary exceeds u32"))?;
                            self.dictionary.push(text.into());
                            self.text_ids.insert(text.into(), id);
                            id
                        };
                        u64::from(id) << u32::from(self.shared_text.is_some())
                    };
                    put_u64(&mut self.columns[2], index);
                }
            }
            p @ (Primitive::Bits | Primitive::Logic | Primitive::Logic9) => {
                let bits = row.u64()?;
                put_u64(&mut self.columns[4], bits);
                self.columns[4].extend_from_slice(row.take(p.packed_bytes(bits)?)?);
            }
            Primitive::Fixed | Primitive::Ufixed => {
                let raw = row.u64()?;
                if primitive == Primitive::Fixed {
                    put_i64(&mut self.columns[0], raw as i64);
                } else {
                    put_u64(&mut self.columns[0], raw);
                }
                put_i64(&mut self.columns[0], row.u64()? as i64);
            }
            _ => return Err(Error::Corrupt("enum requires a declaration")),
        }
        Ok(())
    }
}

#[derive(Default)]
struct ArgumentOutput<const RETAIN: bool> {
    bytes: Vec<u8>,
    length: usize,
}

impl<const RETAIN: bool> ArgumentOutput<RETAIN> {
    fn len(&self) -> usize {
        self.length
    }
}

pub struct ArgumentReader<'a> {
    columns: [Cursor<'a>; 5],
    dictionary: Vec<&'a str>,
    next_dictionary: usize,
    shared: bool,
    shared_text: &'a [String],
    string_count: u64,
    max_payload_bytes: usize,
}

impl<'a> ArgumentReader<'a> {
    pub fn new(
        columns: [&'a [u8]; 5],
        string_count: u64,
        max_payload_bytes: usize,
        shared_text: &'a [String],
    ) -> Result<Self> {
        let mut columns = columns.map(Cursor::new);
        let descriptor = columns[3].var()?;
        let shared = descriptor & 1 != 0;
        if shared && shared_text.is_empty() {
            return Err(Error::Corrupt("shared text table is unavailable"));
        }
        let count = descriptor >> 1;
        if count > columns[3].remaining() as u64 {
            return Err(Error::Corrupt("invalid argument dictionary count"));
        }
        let mut dictionary = Vec::new();
        let mut seen = HashSet::new();
        for _ in 0..count {
            let text = columns[3].text()?;
            if !seen.insert(text) {
                return Err(Error::Corrupt("duplicate argument dictionary entry"));
            }
            dictionary.push(text);
        }
        columns[3].done()?;
        Ok(Self {
            columns,
            dictionary,
            next_dictionary: 0,
            shared,
            shared_text,
            string_count,
            max_payload_bytes: max_payload_bytes.min(u32::MAX as usize),
        })
    }

    pub fn with_defaults(columns: [&'a [u8]; 5]) -> Result<Self> {
        Self::new(columns, u64::MAX, DEFAULT_PAYLOAD_LIMIT, &[])
    }

    /// A failed row does not consume columns or dictionary first-use state.
    pub fn read(&mut self, types: &Types, datatype: TypeId) -> Result<Vec<u8>> {
        self.decode::<true>(types, datatype)
    }

    /// Validate and consume a row without retaining its canonical payload bytes.
    /// Failure preserves the columns and dictionary first-use state.
    pub fn skip(&mut self, types: &Types, datatype: TypeId) -> Result<()> {
        self.decode::<false>(types, datatype).map(|_| ())
    }

    fn decode<const RETAIN: bool>(&mut self, types: &Types, datatype: TypeId) -> Result<Vec<u8>> {
        let saved = self.columns;
        let saved_dictionary = self.next_dictionary;
        let mut out = ArgumentOutput::<RETAIN>::default();
        if let Err(error) = self.read_type(types, datatype, &mut out) {
            self.columns = saved;
            self.next_dictionary = saved_dictionary;
            return Err(error);
        }
        Ok(out.bytes)
    }

    pub fn done(&self) -> Result<()> {
        for column in self.columns {
            column.done()?;
        }
        if self.next_dictionary != self.dictionary.len() {
            return Err(Error::Corrupt("unused argument dictionary entries"));
        }
        Ok(())
    }

    fn append<const RETAIN: bool>(
        &self,
        out: &mut ArgumentOutput<RETAIN>,
        bytes: &[u8],
    ) -> Result<()> {
        if bytes.len() > self.max_payload_bytes - out.len() {
            return Err(Error::Corrupt("typed payload byte limit exceeded"));
        }
        out.length += bytes.len();
        if RETAIN {
            out.bytes.extend_from_slice(bytes);
        }
        Ok(())
    }

    fn length<const RETAIN: bool>(
        &self,
        out: &mut ArgumentOutput<RETAIN>,
        size: usize,
    ) -> Result<()> {
        let size = u32::try_from(size).map_err(|_| Error::Corrupt("collection exceeds u32"))?;
        self.append(out, &size.to_le_bytes())
    }

    fn read_type<const RETAIN: bool>(
        &mut self,
        types: &Types,
        datatype: TypeId,
        out: &mut ArgumentOutput<RETAIN>,
    ) -> Result<()> {
        let datatype = types.get(datatype)?;
        if let Some(program) = &datatype.scalars {
            for primitive in program {
                self.scalar(*primitive, out)?;
            }
            return Ok(());
        }
        let (element, count) = match &datatype.kind {
            TypeKind::Struct(fields) => {
                for field in fields {
                    self.read_type(types, field.datatype, out)?;
                }
                return Ok(());
            }
            TypeKind::Array { element, count } => (*element, *count),
            TypeKind::Sequence { element } => {
                let count = self.columns[0].id()?;
                self.length(out, count as usize)?;
                (*element, count)
            }
            _ => return Err(Error::Corrupt("invalid datatype traversal")),
        };
        let child = types.get(element)?;
        if child.fixed_size == Some(0) {
            return Ok(());
        }
        let remaining = self.max_payload_bytes - out.len();
        if child.minimum_size != 0 && count as usize > remaining / child.minimum_size as usize {
            return Err(Error::Corrupt("typed payload byte limit exceeded"));
        }
        if count as usize > self.columns.iter().map(Cursor::remaining).sum::<usize>() {
            return Err(Error::Corrupt("invalid encoded array count"));
        }
        for _ in 0..count {
            self.read_type(types, element, out)?;
        }
        Ok(())
    }

    fn scalar<const RETAIN: bool>(
        &mut self,
        primitive: Primitive,
        out: &mut ArgumentOutput<RETAIN>,
    ) -> Result<()> {
        match primitive {
            Primitive::Unit => (),
            Primitive::Bool => {
                let value = self.columns[0].byte()?;
                if value > 1 {
                    return Err(Error::Corrupt("invalid bool argument"));
                }
                self.append(out, &[value])?;
            }
            p if p.integer() || matches!(p, Primitive::Time | Primitive::Pointer) => {
                let value = self.columns[0].var()?;
                let value = if p.signed() {
                    unzigzag(value) as u64
                } else {
                    value
                };
                integer_range(p, value)?;
                self.append(out, &value.to_le_bytes()[..p.width().unwrap()])?;
            }
            p @ (Primitive::F32 | Primitive::F64) => {
                let bytes = self.columns[1].take(p.width().unwrap())?;
                self.append(out, bytes)?;
            }
            Primitive::Str => {
                let id = self.columns[4].id()?;
                if u64::from(id) >= self.string_count {
                    return Err(Error::Corrupt("unknown argument string"));
                }
                self.append(out, &id.to_le_bytes())?;
            }
            Primitive::Text => {
                let index = self.columns[2].var()?;
                let text = if self.shared && index & 1 != 0 {
                    let index = index >> 1;
                    if index >= self.string_count {
                        return Err(Error::Corrupt("unknown shared text identifier"));
                    }
                    self.shared_text
                        .get(
                            usize::try_from(index)
                                .map_err(|_| Error::Corrupt("unknown shared text identifier"))?,
                        )
                        .ok_or(Error::Corrupt("unknown shared text identifier"))?
                        .as_str()
                } else {
                    let index = if self.shared { index >> 1 } else { index };
                    if index > self.next_dictionary as u64 {
                        return Err(Error::Corrupt("invalid argument dictionary order"));
                    }
                    let text = *self
                        .dictionary
                        .get(index as usize)
                        .ok_or(Error::Corrupt("invalid argument dictionary index"))?;
                    if index == self.next_dictionary as u64 {
                        self.next_dictionary += 1;
                    }
                    text
                };
                self.length(out, text.len())?;
                self.append(out, text.as_bytes())?;
            }
            Primitive::Bytes => {
                let bytes = self.columns[4].blob()?;
                self.length(out, bytes.len())?;
                self.append(out, bytes)?;
            }
            p @ (Primitive::Bits | Primitive::Logic | Primitive::Logic9) => {
                let bits = self.columns[4].var()?;
                let bytes = self.columns[4].take(p.packed_bytes(bits)?)?;
                p.validate_packed(bytes, bits)?;
                self.append(out, &bits.to_le_bytes())?;
                self.append(out, bytes)?;
            }
            Primitive::Fixed | Primitive::Ufixed => {
                let raw = self.columns[0].var()?;
                let raw = if primitive == Primitive::Fixed {
                    unzigzag(raw) as u64
                } else {
                    raw
                };
                let exponent = unzigzag(self.columns[0].var()?);
                self.append(out, &raw.to_le_bytes())?;
                self.append(out, &exponent.to_le_bytes())?;
            }
            _ => return Err(Error::Corrupt("enum requires a declaration")),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Field, Payload};

    #[test]
    fn skipping_rows_preserves_dictionary_order_and_validates_limits() {
        let mut types = Types::new();
        let datatype = types
            .structure(
                "Row",
                vec![
                    Field::new("text", Primitive::Text.id()),
                    Field::new("valid", Primitive::Bool.id()),
                ],
            )
            .unwrap();
        let mut columns = ArgumentColumns::default();
        let mut rows = Vec::new();
        for text in ["first", "second", "first", "last"] {
            let mut row = Vec::new();
            text.to_owned().encode(&mut row).unwrap();
            true.encode(&mut row).unwrap();
            columns.append(&types, datatype, &row).unwrap();
            rows.push(row);
        }
        let columns = columns.finish();
        let views = columns.each_ref().map(Vec::as_slice);
        let mut reader = ArgumentReader::with_defaults(views).unwrap();
        reader.skip(&types, datatype).unwrap();
        assert_eq!(reader.read(&types, datatype).unwrap(), rows[1]);
        reader.skip(&types, datatype).unwrap();
        assert_eq!(reader.read(&types, datatype).unwrap(), rows[3]);
        reader.done().unwrap();

        let mut limited = ArgumentReader::new(views, 0, rows[0].len() - 1, &[]).unwrap();
        assert!(limited.skip(&types, datatype).is_err());
        assert_eq!(
            limited.read(&types, Primitive::Text.id()).unwrap(),
            rows[0][..rows[0].len() - 1]
        );
        let mut malformed = columns.clone();
        malformed[0][0] = 2;
        let mut reader =
            ArgumentReader::with_defaults(malformed.each_ref().map(Vec::as_slice)).unwrap();
        assert!(reader.skip(&types, datatype).is_err());
        assert_eq!(
            reader.read(&types, Primitive::Text.id()).unwrap(),
            rows[0][..rows[0].len() - 1]
        );
    }
}
