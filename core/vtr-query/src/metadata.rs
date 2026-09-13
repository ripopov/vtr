//! Raw declaration pages and snapshot-local string references. Presentation
//! paths and UI tree expansion state do not belong in these records.
use crate::{
    wave::{Bytes, Kind},
    Error, Reservation, Result,
};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub enum Text {
    Inline(Bytes),
    /// Read through the string-part operation on the same opened snapshot.
    Reference {
        id: u32,
        bytes: u64,
    },
}
impl Text {
    pub fn as_str(&self) -> Result<Option<&str>> {
        match self {
            Self::Inline(bytes) => std::str::from_utf8(bytes.as_slice())
                .map(Some)
                .map_err(|_| Error::Invalid("metadata text is not UTF-8")),
            Self::Reference { .. } => Ok(None),
        }
    }
}

#[derive(Clone, Debug)]
pub enum DeclarationData {
    Scope {
        type_code: u16,
        component: Text,
    },
    Variable {
        type_code: u16,
        direction: u8,
        signal: u32,
        kind: Kind,
        alias: bool,
    },
    Stream {
        kind: Text,
    },
    Generator,
    EnumTable {
        entries: u64,
    },
}
#[derive(Clone, Debug)]
pub struct Declaration {
    pub id: u32,
    pub parent: Option<u32>,
    pub name: Text,
    pub data: DeclarationData,
    pub children: u64,
    pub attributes: u64,
}
#[derive(Debug)]
pub struct DeclarationPage {
    pub parent: Option<u32>,
    /// Declaration-order position among this parent's direct children.
    pub offset: u64,
    pub complete: bool,
    pub(crate) declarations: Vec<Declaration>,
    pub(crate) _charge: Reservation,
}
impl DeclarationPage {
    pub fn declarations(&self) -> &[Declaration] {
        &self.declarations
    }
}
#[derive(Debug)]
pub struct TextPart {
    pub id: u32,
    pub offset: u64,
    pub total_bytes: u64,
    /// A byte range may split UTF-8. Decode only after assembling the needed
    /// byte ranges; never substitute a replacement character into raw metadata.
    pub bytes: Bytes,
    pub(crate) _charge: Reservation,
}
pub type SharedDeclarations = Arc<DeclarationPage>;

#[derive(Debug)]
pub struct SearchPage {
    /// Source declaration at which this work slice started. This is diagnostic
    /// progress, not a stateless continuation for a partially inspected name.
    pub start_node: u32,
    pub complete: bool,
    pub(crate) declarations: Vec<Declaration>,
    pub(crate) _charge: Reservation,
}
impl SearchPage {
    pub fn declarations(&self) -> &[Declaration] {
        &self.declarations
    }
}

/// Literal path segments; dots and HDL escape characters are not separators.
#[derive(Debug)]
pub struct Path {
    pub segments: Vec<String>,
    /// Optional zero-based occurrence of the terminal declaration. Intermediate
    /// scopes must always be unambiguous.
    pub occurrence: Option<u32>,
    /// Raw VTR node-kind code for the terminal node, or any kind when absent.
    pub kind: Option<u8>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    Found(u32),
    Missing,
    Ambiguous,
}
#[derive(Debug)]
pub struct ResolvePage {
    pub offset: usize,
    pub complete: bool,
    pub(crate) results: Vec<Resolution>,
    pub(crate) _charge: Reservation,
}
impl ResolvePage {
    pub fn results(&self) -> &[Resolution] {
        &self.results
    }
}
