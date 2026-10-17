//! Interned string table.

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StrId(pub u32);

impl StrId {
    pub const EMPTY: StrId = StrId(0);
}

pub struct Interner {
    map: HashMap<Box<str>, u32>,
    strings: Vec<Box<str>>,
    flushed: usize,
    bytes_pending: usize,
}

impl Interner {
    #[must_use]
    pub fn new() -> Self {
        let mut i = Interner {
            map: HashMap::new(),
            strings: Vec::new(),
            flushed: 0,
            bytes_pending: 0,
        };
        i.intern("");
        i
    }

    pub fn intern(&mut self, s: &str) -> StrId {
        if let Some(&id) = self.map.get(s) {
            return StrId(id);
        }
        let id = self.strings.len() as u32;
        let b: Box<str> = s.into();
        self.map.insert(b.clone(), id);
        self.strings.push(b);
        self.bytes_pending += s.len();
        StrId(id)
    }

    #[must_use]
    pub fn get(&self, id: StrId) -> &str {
        &self.strings[id.0 as usize]
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.strings.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }

    #[must_use]
    pub fn has_pending(&self) -> bool {
        self.flushed < self.strings.len()
    }

    #[must_use]
    pub fn pending_bytes(&self) -> usize {
        self.bytes_pending
    }

    pub fn take_chunk(&mut self) -> (u32, u32, Vec<u8>) {
        let first = self.flushed;
        let count = self.strings.len() - first;
        let mut out = Vec::with_capacity(self.bytes_pending + count * 2 + 16);
        crate::varint::put_u64(&mut out, first as u64);
        crate::varint::put_u64(&mut out, count as u64);
        for s in &self.strings[first..] {
            crate::varint::put_blob(&mut out, s.as_bytes());
        }
        self.flushed = self.strings.len();
        self.bytes_pending = 0;
        (first as u32, count as u32, out)
    }
}

impl Default for Interner {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default, Debug)]
pub struct StringTable {
    data: Vec<u8>,
    offsets: Vec<u32>,
}

impl StringTable {
    #[must_use]
    pub fn new() -> Self {
        StringTable {
            data: Vec::new(),
            offsets: vec![0],
        }
    }

    pub fn add_chunk(&mut self, payload: &[u8]) -> crate::Result<()> {
        let mut r = crate::varint::Reader::new(payload);
        let first = r.usize()?;
        let count = r.usize()?;
        if first != self.len() {
            return Err(crate::Error::Corrupt("string chunk out of order"));
        }
        self.offsets.reserve(count);
        for _ in 0..count {
            let s = r.blob()?;
            std::str::from_utf8(s).map_err(|_| crate::Error::Corrupt("string is not UTF-8"))?;
            self.data.extend_from_slice(s);
            self.offsets.push(self.data.len() as u32);
        }
        Ok(())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.offsets.len() - 1
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[must_use]
    pub fn get(&self, id: StrId) -> &str {
        let i = id.0 as usize;
        if i + 1 >= self.offsets.len() {
            return "";
        }
        let s = &self.data[self.offsets[i] as usize..self.offsets[i + 1] as usize];
        // SAFETY: add_chunk validates every entry as UTF-8 and records its complete byte boundaries.
        unsafe { std::str::from_utf8_unchecked(s) }
    }

    #[must_use]
    pub fn try_get(&self, id: StrId) -> Option<&str> {
        if (id.0 as usize) < self.len() {
            Some(self.get(id))
        } else {
            None
        }
    }

    #[must_use]
    pub fn resident_bytes(&self) -> u64 {
        (self.data.capacity() + 4 * self.offsets.capacity()) as u64
    }

    #[must_use]
    pub fn find(&self, s: &str) -> Option<StrId> {
        (0..self.len())
            .map(|i| StrId(i as u32))
            .find(|&id| self.get(id) == s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_chunk_preserves_valid_utf8_entries() {
        let mut table = StringTable::new();
        let mut payload = Vec::new();
        crate::varint::put_u64(&mut payload, 0);
        crate::varint::put_u64(&mut payload, 2);
        crate::varint::put_blob(&mut payload, "λ".as_bytes());
        crate::varint::put_blob(&mut payload, &[0xff]);
        assert!(table.add_chunk(&payload).is_err());
        assert_eq!(table.len(), 1);
        assert_eq!(table.get(StrId(0)), "λ");
        assert_eq!(table.try_get(StrId(1)), None);
    }

    #[test]
    fn intern_and_chunks() {
        let mut i = Interner::new();
        assert_eq!(i.intern(""), StrId::EMPTY);
        let a = i.intern("clk");
        let b = i.intern("data");
        assert_eq!(i.intern("clk"), a);
        let (first, count, c1) = i.take_chunk();
        assert_eq!((first, count), (0, 3));
        let c = i.intern("rst");
        let (first, count, c2) = i.take_chunk();
        assert_eq!((first, count), (3, 1));
        let mut t = StringTable::new();
        t.add_chunk(&c1).unwrap();
        t.add_chunk(&c2).unwrap();
        assert_eq!(t.get(a), "clk");
        assert_eq!(t.get(b), "data");
        assert_eq!(t.get(c), "rst");
        assert_eq!(t.get(StrId::EMPTY), "");
        assert_eq!(t.len(), 4);
    }
}
