use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Source {
    pub file: String,
    pub line: u32,
    pub column: u32,
}
impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.column)
    }
}
#[derive(Clone, Debug, Deserialize)]
pub struct Type {
    pub text: String,
    pub width: u32,
    pub signed: bool,
    pub four_state: bool,
    pub integral: bool,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Symbol {
    pub owner: String,
    pub path: String,
    pub kind: String,
    #[serde(rename = "type")]
    pub ty: Type,
    pub source: Source,
    pub value: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Expr {
    #[serde(rename = "type")]
    pub ty: Type,
    pub source: Source,
    #[serde(flatten)]
    pub op: ExprOp,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind")]
pub enum ExprOp {
    NamedValue {
        symbol: String,
    },
    HierarchicalValue {
        symbol: String,
    },
    Constant {
        value: String,
    },
    BinaryOp {
        op: String,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    UnaryOp {
        op: String,
        operand: Box<Expr>,
    },
    Conversion {
        operand: Box<Expr>,
    },
    ConditionalOp {
        cond: Box<Expr>,
        yes: Box<Expr>,
        no: Box<Expr>,
    },
    Concatenation {
        operands: Vec<Expr>,
    },
    Replication {
        count: u32,
        operand: Box<Expr>,
    },
    RangeSelect {
        value: Box<Expr>,
        left: Box<Expr>,
        right: Box<Expr>,
        selection: String,
        range_left: i64,
        range_right: i64,
    },
    ElementSelect {
        value: Box<Expr>,
        selector: Box<Expr>,
        range_left: i64,
        range_right: i64,
    },
    Unsupported {
        reason: String,
        #[serde(default)]
        reads: Vec<String>,
    },
}
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind")]
pub enum Statement {
    Empty,
    Sequence {
        statements: Vec<Statement>,
    },
    Assign {
        target: String,
        value: Expr,
        nba: bool,
        source: Source,
    },
    If {
        cond: Expr,
        yes: Box<Statement>,
        no: Box<Statement>,
        source: Source,
    },
    Unsupported {
        reason: String,
        #[serde(default)]
        source: Source,
    },
}
#[derive(Clone, Debug, Deserialize)]
pub struct Event {
    pub edge: String,
    pub expr: Expr,
    pub source: Source,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Process {
    pub owner: String,
    pub origin: String,
    pub reads: Vec<String>,
    pub id: u32,
    pub mode: String,
    pub targets: Vec<String>,
    pub events: Vec<Event>,
    pub body: Statement,
    pub source: Source,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Port {
    pub name: String,
    pub symbol: String,
    pub direction: String,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Instance {
    pub parent: Option<String>,
    pub ports: Vec<Port>,
    pub path: String,
    pub definition: String,
    pub source: Source,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Connection {
    pub instance: String,
    pub port: String,
    pub direction: String,
    pub expression: Expr,
    pub source: Source,
}
#[derive(Clone, Debug, Deserialize)]
pub struct SourceFile {
    pub path: String,
    pub sha256: String,
}
/// One command-line preprocessor define.
#[derive(Clone, Debug, Deserialize)]
pub struct Define {
    pub name: String,
    pub value: String,
}
/// Structured elaboration inputs: the design file set and preprocessor environment the
/// producer elaborated, recorded so another frontend can re-elaborate the same design.
/// Relative paths resolve against `work_dir`.
#[derive(Clone, Debug, Deserialize)]
pub struct Elaboration {
    pub work_dir: String,
    pub language: String,
    pub files: Vec<String>,
    #[serde(default)]
    pub library_files: Vec<String>,
    #[serde(default)]
    pub include_dirs: Vec<String>,
    #[serde(default)]
    pub library_exts: Vec<String>,
    #[serde(default)]
    pub defines: Vec<Define>,
}
/// One classified token of an indexed source file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexedToken {
    /// One-based line.
    pub line: u32,
    /// One-based byte column.
    pub column: u32,
    /// Length in bytes; tokens never span lines.
    pub length: u32,
    /// Index into [`SourceIndex::classes`].
    pub class: u32,
    /// Bit set over [`SourceIndex::modifiers`].
    pub modifiers: u32,
    /// Declaration of the symbol the token denotes, for resolved identifiers.
    pub declaration: Option<IndexLocation>,
}
/// A position in an indexed file: index into [`SourceIndex::files`] plus one-based
/// line and byte column, comparable to any other [`Source`] of the same file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IndexLocation {
    pub file: u32,
    pub line: u32,
    pub column: u32,
}
/// A generate block an instance leaves uninstantiated: `[start, end)` in one-based
/// line and byte column coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InactiveRange {
    pub file: u32,
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
}
impl InactiveRange {
    /// Whether the byte span `[start, end)` of `line` lies inside the range.
    pub fn covers(&self, line: u32, start: u32, end: u32) -> bool {
        let last = end.max(start + 1) - 1;
        (line, start) >= (self.start_line, self.start_column)
            && (line, last) < (self.end_line, self.end_column)
    }
}
/// One parsed source file of the [`SourceIndex`]: flat token and declaration groups
/// as serialized; decode them with [`IndexedFile::tokens`].
#[derive(Clone, Debug, Deserialize)]
pub struct IndexedFile {
    pub path: String,
    #[serde(default)]
    pub tokens: Vec<u32>,
    #[serde(default)]
    pub declarations: Vec<u32>,
}
impl IndexedFile {
    /// Decoded tokens in position order.
    pub fn tokens(&self) -> Vec<IndexedToken> {
        let mut tokens: Vec<IndexedToken> = self
            .tokens
            .chunks_exact(5)
            .map(|group| IndexedToken {
                line: group[0],
                column: group[1],
                length: group[2],
                class: group[3],
                modifiers: group[4],
                declaration: None,
            })
            .collect();
        for group in self.declarations.chunks_exact(4) {
            if let Some(token) = tokens.get_mut(group[0] as usize) {
                token.declaration = Some(IndexLocation {
                    file: group[1],
                    line: group[2],
                    column: group[3],
                });
            }
        }
        tokens
    }
}
/// Static rendering index of the design sources, produced after the identity and
/// outside it: token classes and declarations per file, definitions of modules,
/// interfaces and packages, and the generate blocks each instance leaves
/// uninstantiated. See `docs/VDB_RTL.md`, "Source index".
#[derive(Clone, Debug, Deserialize)]
pub struct SourceIndex {
    pub producer: String,
    pub classes: Vec<String>,
    pub modifiers: Vec<String>,
    pub files: Vec<IndexedFile>,
    #[serde(default)]
    pub definitions: BTreeMap<String, Vec<u32>>,
    #[serde(default)]
    pub inactive: BTreeMap<String, Vec<Vec<u32>>>,
}
impl SourceIndex {
    /// Index of the file recorded under `path` (the `sources` spelling).
    pub fn file_index(&self, path: &str) -> Option<usize> {
        self.files.iter().position(|file| file.path == path)
    }
    /// Declaration of the module, interface or package called `name`.
    pub fn definition(&self, name: &str) -> Option<IndexLocation> {
        let group = self.definitions.get(name)?;
        (group.len() == 3).then(|| IndexLocation {
            file: group[0],
            line: group[1],
            column: group[2],
        })
    }
    /// Generate blocks `instance` leaves uninstantiated, all files.
    pub fn inactive_ranges(&self, instance: &str) -> Vec<InactiveRange> {
        self.inactive
            .get(instance)
            .map(|ranges| {
                ranges
                    .iter()
                    .filter(|group| group.len() == 5)
                    .map(|group| InactiveRange {
                        file: group[0],
                        start_line: group[1],
                        start_column: group[2],
                        end_line: group[3],
                        end_column: group[4],
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    /// Shape checks: group sizes, class numbers and file indices.
    pub fn validate(&self) -> Result<(), String> {
        let files = self.files.len() as u32;
        for file in &self.files {
            if file.tokens.len() % 5 != 0 || file.declarations.len() % 4 != 0 {
                return Err(format!(
                    "source index: malformed token groups in {}",
                    file.path
                ));
            }
            let count = (file.tokens.len() / 5) as u32;
            for group in file.tokens.chunks_exact(5) {
                if group[3] as usize >= self.classes.len() {
                    return Err(format!(
                        "source index: unknown class {} in {}",
                        group[3], file.path
                    ));
                }
            }
            for group in file.declarations.chunks_exact(4) {
                if group[0] >= count || group[1] >= files {
                    return Err(format!(
                        "source index: declaration out of range in {}",
                        file.path
                    ));
                }
            }
        }
        for (name, group) in &self.definitions {
            if group.len() != 3 || group[0] >= files {
                return Err(format!("source index: malformed definition {name}"));
            }
        }
        for (instance, ranges) in &self.inactive {
            if ranges
                .iter()
                .any(|group| group.len() != 5 || group[0] >= files)
            {
                return Err(format!(
                    "source index: malformed inactive range in {instance}"
                ));
            }
        }
        Ok(())
    }
}
/// Exact attachment emitted alongside a simulator recording.
#[derive(Clone, Debug, Deserialize)]
pub struct TraceBinding {
    pub prefix: String,
    pub signals: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Database {
    pub format: String,
    pub version: u32,
    pub producer: String,
    pub top: String,
    pub design_id: String,
    #[serde(default)]
    pub trace_binding: Option<TraceBinding>,
    pub sources: Vec<SourceFile>,
    pub options: Vec<String>,
    #[serde(default)]
    pub elaboration: Option<Elaboration>,
    #[serde(default)]
    pub source_index: Option<SourceIndex>,
    pub instances: Vec<Instance>,
    pub symbols: BTreeMap<String, Symbol>,
    pub connections: Vec<Connection>,
    pub processes: Vec<Process>,
}
impl Database {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let header: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("invalid VDB: {e}"))?;
        if header["format"] != "vtr-rtl-vdb" || header["version"] != 2 {
            return Err(format!(
                "unsupported VDB format/version: {} {} (re-export RTL with VDB v2)",
                header["format"], header["version"]
            ));
        }
        let db: Self = serde_json::from_value(header).map_err(|e| format!("invalid VDB: {e}"))?;
        if db.format != "vtr-rtl-vdb" || db.version != 2 {
            return Err(format!(
                "unsupported VDB format/version: {} {}",
                db.format, db.version
            ));
        }
        for (key, s) in &db.symbols {
            if key != &s.path {
                return Err(format!("VDB symbol key/path mismatch: {key}"));
            }
        }
        if let Some(index) = &db.source_index {
            index.validate()?;
        }
        for p in &db.processes {
            for t in &p.targets {
                if !db.symbols.contains_key(t) {
                    return Err(format!("VDB process {} has unknown target {t}", p.id));
                }
            }
        }
        crate::netlist::NetlistIndex::new(&db)?;
        Ok(db)
    }
}

#[cfg(test)]
mod source_index_tests {
    use super::*;

    fn sample() -> SourceIndex {
        serde_json::from_value(serde_json::json!({
            "producer": "slang 11.0.0",
            "classes": ["keyword", "variable"],
            "modifiers": ["declaration", "input"],
            "files": [
                {"path": "a.sv", "tokens": [1, 1, 6, 0, 0, 1, 8, 3, 1, 3, 3, 5, 3, 1, 0],
                 "declarations": [1, 0, 1, 8, 2, 1, 4, 2]},
                {"path": "b.svh", "tokens": []}
            ],
            "definitions": {"top": [0, 1, 8]},
            "inactive": {"top.u": [[0, 2, 3, 4, 1]]}
        }))
        .unwrap()
    }

    #[test]
    fn tokens_decode_with_declarations() {
        let index = sample();
        index.validate().unwrap();
        let tokens = index.files[0].tokens();
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0].declaration, None);
        assert_eq!(
            tokens[1],
            IndexedToken {
                line: 1,
                column: 8,
                length: 3,
                class: 1,
                modifiers: 3,
                declaration: Some(IndexLocation {
                    file: 0,
                    line: 1,
                    column: 8
                })
            }
        );
        assert_eq!(tokens[2].declaration.unwrap().file, 1);
        assert_eq!(index.file_index("b.svh"), Some(1));
        assert_eq!(
            index.definition("top"),
            Some(IndexLocation {
                file: 0,
                line: 1,
                column: 8
            })
        );
        let ranges = index.inactive_ranges("top.u");
        assert_eq!(ranges.len(), 1);
        assert!(ranges[0].covers(2, 3, 10));
        assert!(!ranges[0].covers(2, 2, 10));
        assert!(ranges[0].covers(3, 0, 80));
        assert!(!ranges[0].covers(4, 0, 2));
        assert!(index.inactive_ranges("top").is_empty());
    }

    #[test]
    fn malformed_indexes_are_rejected() {
        let mut index = sample();
        index.files[0].tokens.push(9);
        assert!(index.validate().unwrap_err().contains("token groups"));
        let mut index = sample();
        index.files[0].tokens[3] = 7;
        assert!(index.validate().unwrap_err().contains("unknown class"));
        let mut index = sample();
        index.files[0].declarations[1] = 5;
        assert!(index.validate().unwrap_err().contains("out of range"));
        let mut index = sample();
        index.definitions.insert("x".into(), vec![9, 1, 1]);
        assert!(index.validate().unwrap_err().contains("definition"));
    }
}
