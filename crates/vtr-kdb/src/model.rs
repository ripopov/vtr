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
    ElementSelect {
        value: Box<Expr>,
        selector: Box<Expr>,
        range_left: i64,
        range_right: i64,
    },
    Unsupported {
        reason: String,
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
    pub id: u32,
    pub mode: String,
    pub targets: Vec<String>,
    pub events: Vec<Event>,
    pub body: Statement,
    pub source: Source,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Instance {
    pub path: String,
    pub definition: String,
    pub source: Source,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Connection {
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
#[derive(Clone, Debug, Deserialize)]
pub struct Database {
    pub format: String,
    pub version: u32,
    pub producer: String,
    pub top: String,
    pub design_id: String,
    pub sources: Vec<SourceFile>,
    pub options: Vec<String>,
    pub instances: Vec<Instance>,
    pub symbols: BTreeMap<String, Symbol>,
    pub connections: Vec<Connection>,
    pub processes: Vec<Process>,
}
impl Database {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let db: Self = serde_json::from_slice(&bytes).map_err(|e| format!("invalid KDB: {e}"))?;
        if db.format != "vtr-rtl-kdb" || db.version != 1 {
            return Err(format!(
                "unsupported KDB format/version: {} {}",
                db.format, db.version
            ));
        }
        for (key, s) in &db.symbols {
            if key != &s.path {
                return Err(format!("KDB symbol key/path mismatch: {key}"));
            }
        }
        for p in &db.processes {
            for t in &p.targets {
                if !db.symbols.contains_key(t) {
                    return Err(format!("KDB process {} has unknown target {t}", p.id));
                }
            }
        }
        Ok(db)
    }
}
