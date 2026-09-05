//! Source semantics remain in VDB. This crate only reads immutable VTR runtime data.
mod model;
pub mod netlist;
pub use model::*;
use std::collections::{BTreeMap, BTreeSet};
use vtr::{NodeData, Reader, SignalData, SignalId, SignalKind};

/// `before` means the value immediately before a timestamp, not a delta cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Moment {
    pub time: u64,
    pub before: bool,
}
impl std::fmt::Display for Moment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{}",
            self.time,
            if self.before { "- (pre-event)" } else { "" }
        )
    }
}
impl Moment {
    fn tick(self) -> Result<u64, String> {
        if self.before {
            self.time
                .checked_sub(1)
                .ok_or("missing trace before time 0".into())
        } else {
            Ok(self.time)
        }
    }
}
#[derive(Clone, Debug)]
pub struct Dependency {
    pub symbol: String,
    pub at: Moment,
    pub why: String,
    pub source: Source,
}
#[derive(Clone, Debug)]
pub struct TraceNode {
    pub symbol: String,
    pub at: Moment,
    pub value: String,
    pub why: String,
    pub source: Source,
    pub notes: Vec<String>,
    pub children: Vec<TraceNode>,
}
impl TraceNode {
    /// Stable, plain-text dependency tree suitable for stdout or a terminal viewer.
    pub fn render(&self) -> String {
        fn walk(n: &TraceNode, level: usize, out: &mut String) {
            let pad = "  ".repeat(level);
            out.push_str(&format!(
                "{pad}{} = {} @ {} [{}] {}\n",
                n.symbol, n.value, n.at, n.why, n.source
            ));
            for note in &n.notes {
                out.push_str(&format!("{pad}  ! {note}\n"));
            }
            for c in &n.children {
                walk(c, level + 1, out);
            }
        }
        let mut s = String::new();
        walk(self, 0, &mut s);
        s
    }
}
#[derive(Clone)]
struct Evaluated {
    value: Option<u64>,
    deps: Vec<Dependency>,
}
#[derive(Default)]
struct Execution {
    locals: BTreeMap<String, Evaluated>,
    assigned: BTreeMap<String, (Evaluated, Source)>,
    controls: Vec<Dependency>,
}

/// A checked structural mapping. Missing signals are retained as diagnostics;
/// incompatible widths and ambiguous names prevent attachment.
pub struct Debugger<'a> {
    db: &'a Database,
    reader: &'a Reader,
    mapping: BTreeMap<String, SignalId>,
    histories: BTreeMap<SignalId, SignalData>,
    pub diagnostics: Vec<String>,
}
impl<'a> Debugger<'a> {
    /// `prefix` is an explicit simulator wrapper (e.g. `TOP`), never a suffix guess.
    pub fn attach(db: &'a Database, reader: &'a Reader, prefix: &str) -> Result<Self, String> {
        let mut candidates: BTreeMap<String, BTreeSet<SignalId>> = BTreeMap::new();
        for n in reader.hierarchy().ids() {
            if let NodeData::Var { signal, .. } = reader.hierarchy().node(n).data {
                // Verilator / VCD may append a separate packed range to the name.
                let path = reader.full_path(n, ".");
                let path = path.split(" [").next().unwrap_or(&path).to_string();
                candidates.entry(path).or_default().insert(signal);
            }
        }
        let mut mapping = BTreeMap::new();
        let mut diagnostics = Vec::new();
        for instance in &db.instances {
            let path = if prefix.is_empty() {
                instance.path.clone()
            } else {
                format!(
                    "{}.{path}",
                    prefix.trim_end_matches('.'),
                    path = instance.path
                )
            };
            let parts: Vec<_> = path.split('.').collect();
            if let Some(node) = reader.find_node(&parts) {
                if let NodeData::Scope { component, .. } = reader.hierarchy().node(node).data {
                    let component = reader.str(component);
                    if !component.is_empty() && component != instance.definition {
                        return Err(format!(
                            "design mismatch at {path}: VDB module {}, VTR module {component}",
                            instance.definition
                        ));
                    }
                }
            }
        }
        for (path, s) in &db.symbols {
            if matches!(s.kind.as_str(), "Parameter" | "EnumValue") {
                continue;
            }
            let trace_path = if prefix.is_empty() {
                path.clone()
            } else {
                format!("{}.{path}", prefix.trim_end_matches('.'))
            };
            let Some(ids) = candidates.get(&trace_path) else {
                diagnostics.push(format!("missing trace signal: {trace_path}"));
                continue;
            };
            if ids.len() != 1 {
                return Err(format!("ambiguous trace path: {trace_path}"));
            }
            let id = *ids.first().unwrap();
            match reader.signal_kind(id).map_err(|e| e.to_string())? {
                SignalKind::Bits { width, .. } if s.ty.integral && width == s.ty.width => {}
                k => {
                    return Err(format!(
                        "design mismatch at {trace_path}: VDB {} ({} bits), VTR {k:?}",
                        s.ty.text, s.ty.width
                    ))
                }
            }
            mapping.insert(path.clone(), id);
        }
        if mapping.is_empty() {
            return Err("design mismatch: no VDB signals match VTR (check --prefix)".into());
        }
        // Optional generic producer identity attribute, without extending VTR.
        let identity = reader
            .meta()
            .attrs
            .iter()
            .find(|(k, _)| reader.str(*k) == "design.vdb_id");
        if let Some((_, value)) = identity {
            if !matches!(value, vtr::Value::Str(id) if reader.str(*id) == db.design_id) {
                return Err("design mismatch: design.vdb_id differs from VDB design_id".into());
            }
        } else {
            diagnostics.push("identity: structural match only; VTR has no design.vdb_id (same-interface RTL revisions cannot be distinguished statically)".into());
        }
        Ok(Self {
            db,
            reader,
            mapping,
            histories: BTreeMap::new(),
            diagnostics,
        })
    }

    fn history(&mut self, symbol: &str) -> Result<&SignalData, String> {
        let id = *self
            .mapping
            .get(symbol)
            .ok_or_else(|| format!("missing trace signal: {symbol}"))?;
        if !self.histories.contains_key(&id) {
            self.histories
                .insert(id, self.reader.load_signal(id).map_err(|e| e.to_string())?);
        }
        Ok(&self.histories[&id])
    }
    fn sample(&mut self, symbol: &str, at: Moment) -> Result<String, String> {
        let t = at.tick()?;
        let (start, end) = self.reader.time_range().ok_or("empty trace")?;
        if t < start || t > end {
            return Err(format!(
                "missing trace at {at}: recorded range {start}..{end}"
            ));
        }
        // A dump gap invalidates retained values until this signal is recorded again.
        let resume = self.reader.blackout().iter().rev().find(|b| b.time <= t);
        if resume.is_some_and(|b| !b.active) {
            return Err(format!("missing trace: dumping disabled at {at}"));
        }
        let resume_time = resume.map(|b| b.time);
        let h = self.history(symbol)?;
        let i = h
            .index_at(t)
            .ok_or_else(|| format!("missing initial sample: {symbol} at {at}"))?;
        if resume_time.is_some_and(|r| h.times()[i] < r) {
            return Err(format!(
                "missing fresh sample after dump gap: {symbol} at {at}"
            ));
        }
        Ok(h.get(i).to_ascii())
    }
    fn drivers(&self, symbol: &str) -> Vec<Process> {
        self.db
            .processes
            .iter()
            .filter(|p| p.targets.iter().any(|s| s == symbol))
            .cloned()
            .collect()
    }
    fn eval(
        &mut self,
        e: &Expr,
        at: Moment,
        events: &BTreeSet<String>,
        locals: &BTreeMap<String, Evaluated>,
    ) -> Result<Evaluated, String> {
        if !e.ty.integral || e.ty.width == 0 || e.ty.width > 64 {
            return Err(format!(
                "unsupported evaluation type {} at {} (requires 1..64 integral bits)",
                e.ty.text, e.source
            ));
        }
        let mut deps = Vec::new();
        let value = match &e.op {
            ExprOp::NamedValue { symbol } | ExprOp::HierarchicalValue { symbol } => {
                if let Some(v) = locals.get(symbol) {
                    return Ok(v.clone());
                }
                let at = if events.contains(symbol) {
                    Moment {
                        before: false,
                        ..at
                    }
                } else {
                    at
                };
                let bits = self.sample(symbol, at)?;
                if at.before
                    && self.drivers(symbol).is_empty()
                    && self.history(symbol)?.times().contains(&at.time)
                {
                    return Err(format!("ambiguous scheduling: external input {symbol} changes at event time {}; VTR has no event-region ordering", at.time));
                }
                deps.push(Dependency {
                    symbol: symbol.clone(),
                    at,
                    why: "data".into(),
                    source: e.source.clone(),
                });
                u64::from_str_radix(&bits, 2).ok()
            }
            ExprOp::Constant { value } => constant(value, e.ty.width)?,
            ExprOp::Unsupported { reason, .. } => {
                return Err(format!("unsupported {reason} at {}", e.source))
            }
            ExprOp::Conversion { operand } => {
                let v = self.eval(operand, at, events, locals)?;
                deps = v.deps;
                v.value.map(|v| {
                    if operand.ty.signed {
                        signed(v, operand.ty.width) as u64
                    } else {
                        v
                    }
                })
            }
            ExprOp::UnaryOp { op, operand } => {
                let v = self.eval(operand, at, events, locals)?;
                deps = v.deps;
                match (op.as_str(), v.value) {
                    ("Plus", n) => n,
                    ("Minus", n) => n.map(u64::wrapping_neg),
                    ("BitwiseNot", n) => n.map(|v| !v),
                    ("LogicalNot", n) => n.map(|v| u64::from(v == 0)),
                    ("BitwiseAnd", n) => n.map(|v| u64::from(v == mask(operand.ty.width))),
                    ("BitwiseOr", n) => n.map(|v| u64::from(v != 0)),
                    ("BitwiseXor", n) => n.map(|v| u64::from(v.count_ones() % 2 != 0)),
                    _ => return Err(format!("unsupported unary operator {op}")),
                }
            }
            ExprOp::BinaryOp { op, left, right } => {
                let a = self.eval(left, at, events, locals)?;
                deps.extend(a.deps);
                if (op == "LogicalAnd" && a.value == Some(0))
                    || (op == "LogicalOr" && a.value.is_some_and(|v| v != 0))
                {
                    for d in &mut deps {
                        d.why = "short-circuit control".into();
                    }
                    Some(u64::from(op == "LogicalOr"))
                } else {
                    let b = self.eval(right, at, events, locals)?;
                    deps.extend(b.deps);
                    match (a.value, b.value) {
                        (Some(a), Some(b)) => Some(binary(op, a, b, &left.ty)?),
                        _ => None,
                    }
                }
            }
            ExprOp::ConditionalOp { cond, yes, no } => {
                let c = self.eval(cond, at, events, locals)?;
                let b = c
                    .value
                    .ok_or_else(|| format!("ambiguous mux control (X/Z) at {}", cond.source))?
                    != 0;
                deps = c.deps;
                for d in &mut deps {
                    d.why = format!("mux control: {} branch", if b { "true" } else { "false" });
                }
                let branch = self.eval(if b { yes } else { no }, at, events, locals)?;
                deps.extend(branch.deps);
                branch.value
            }
            ExprOp::RangeSelect { .. } => {
                return Err(format!(
                    "unsupported range-select evaluation at {}",
                    e.source
                ))
            }
            ExprOp::Replication { count, operand } => {
                let v = self.eval(operand, at, events, locals)?;
                deps = v.deps;
                if *count > 64 {
                    return Err("unsupported replication count above 64 in evaluator".into());
                }
                v.value.map(|part| {
                    (0..*count).fold(0, |acc, _| {
                        if operand.ty.width == 64 {
                            part
                        } else {
                            (acc << operand.ty.width) | part
                        }
                    })
                })
            }
            ExprOp::Concatenation { operands } => {
                let mut result = Some(0u64);
                for e in operands {
                    let v = self.eval(e, at, events, locals)?;
                    deps.extend(v.deps);
                    result = result.zip(v.value).map(|(a, b)| {
                        if e.ty.width == 64 {
                            b
                        } else {
                            (a << e.ty.width) | b
                        }
                    });
                }
                result
            }
            ExprOp::ElementSelect {
                value,
                selector,
                range_left,
                range_right,
            } => {
                let a = self.eval(value, at, events, locals)?;
                let mut b = self.eval(selector, at, events, locals)?;
                for d in &mut b.deps {
                    d.why = "bit-select index".into();
                }
                deps = a.deps;
                deps.extend(b.deps);
                a.value.zip(b.value).and_then(|(v, i)| {
                    let i = if selector.ty.signed {
                        signed(i, selector.ty.width)
                    } else {
                        i as i64
                    };
                    let offset = if range_left >= range_right {
                        i.checked_sub(*range_right)
                    } else {
                        range_right.checked_sub(i)
                    }?;
                    if offset < 0 || offset >= i64::from(value.ty.width) {
                        None
                    } else {
                        Some((v >> offset) & 1)
                    }
                })
            }
        };
        Ok(Evaluated {
            value: value.map(|v| v & mask(e.ty.width)),
            deps,
        })
    }
    fn execute(
        &mut self,
        s: &Statement,
        at: Moment,
        events: &BTreeSet<String>,
        seq: bool,
        x: &mut Execution,
        guards: &[Dependency],
    ) -> Result<(), String> {
        match s {
            Statement::Empty => {}
            Statement::Unsupported { reason, source } => {
                return Err(format!("unsupported {reason} at {source}"))
            }
            Statement::Sequence { statements } => {
                for s in statements {
                    self.execute(s, at, events, seq, x, guards)?;
                }
            }
            Statement::Assign {
                target,
                value,
                nba,
                source,
            } => {
                if seq && !nba {
                    return Err(format!(
                        "unsupported blocking assignment in edge-triggered process at {source}"
                    ));
                }
                if !seq && *nba {
                    return Err(format!(
                        "unsupported nonblocking combinational assignment at {source}"
                    ));
                }
                let mut v = self.eval(value, at, events, &x.locals)?;
                v.deps.extend_from_slice(guards);
                if !nba {
                    x.locals.insert(target.clone(), v.clone());
                }
                x.assigned.insert(target.clone(), (v, source.clone()));
            }
            Statement::If {
                cond,
                yes,
                no,
                source,
            } => {
                let mut c = self.eval(cond, at, events, &x.locals)?;
                let b = c
                    .value
                    .ok_or_else(|| format!("ambiguous control (X/Z) at {source}"))?
                    != 0;
                for d in &mut c.deps {
                    d.why = format!(
                        "control: if {} at {source}",
                        if b { "true" } else { "false" }
                    );
                }
                x.controls.extend(c.deps.clone());
                // A false later conditional write also controls an earlier assignment.
                // Preserve that guard only for targets this conditional can overwrite.
                let mut targets = BTreeSet::new();
                statement_targets(yes, &mut targets);
                statement_targets(no, &mut targets);
                for target in targets {
                    if let Some((v, _)) = x.assigned.get_mut(target) {
                        v.deps.extend(c.deps.clone());
                    }
                    if let Some(v) = x.locals.get_mut(target) {
                        v.deps.extend(c.deps.clone());
                    }
                }
                let mut g = guards.to_vec();
                g.extend(c.deps);
                self.execute(if b { yes } else { no }, at, events, seq, x, &g)?;
            }
        }
        Ok(())
    }

    /// Trace a signal at a settled timestamp. Depth counts dependency edges.
    pub fn trace(&mut self, symbol: &str, time: u64, depth: usize) -> Result<TraceNode, String> {
        if depth > 128 {
            return Err("depth must be <= 128".into());
        }
        if !self.db.symbols.contains_key(symbol) {
            return Err(format!("unknown VDB symbol: {symbol}"));
        }
        let mut remaining = 10000;
        Ok(self.walk(
            symbol,
            Moment {
                time,
                before: false,
            },
            depth,
            "selected signal",
            &mut BTreeSet::new(),
            &mut remaining,
        ))
    }
    fn walk(
        &mut self,
        symbol: &str,
        at: Moment,
        depth: usize,
        why: &str,
        stack: &mut BTreeSet<(String, Moment)>,
        remaining: &mut usize,
    ) -> TraceNode {
        let mut node = TraceNode {
            symbol: symbol.into(),
            at,
            value: "?".into(),
            why: why.into(),
            source: self
                .db
                .symbols
                .get(symbol)
                .map(|s| s.source.clone())
                .unwrap_or_default(),
            notes: vec![],
            children: vec![],
        };
        if *remaining == 0 {
            node.notes.push("node budget reached (10000)".into());
            return node;
        }
        *remaining -= 1;
        match self.sample(symbol, at) {
            Ok(v) => node.value = v,
            Err(e) => {
                node.notes.push(e);
                return node;
            }
        }
        if depth == 0 {
            node.notes.push("depth limit".into());
            return node;
        }
        let key = (symbol.to_string(), at);
        if !stack.insert(key.clone()) {
            node.notes
                .push("ambiguous combinational dependency cycle".into());
            return node;
        }
        let result = self.explain(symbol, at, &mut node.notes);
        match result {
            Ok(deps) => {
                let mut seen = BTreeSet::new();
                for d in deps {
                    if seen.insert((d.symbol.clone(), d.at, d.why.clone())) {
                        if *remaining == 0 {
                            node.notes.push("node budget reached (10000)".into());
                            break;
                        }
                        let mut child =
                            self.walk(&d.symbol, d.at, depth - 1, &d.why, stack, remaining);
                        child
                            .notes
                            .insert(0, format!("dependency source: {}", d.source));
                        node.children.push(child);
                    }
                }
            }
            Err(e) => node.notes.push(e),
        }
        stack.remove(&key);
        node
    }
    fn explain(
        &mut self,
        symbol: &str,
        at: Moment,
        notes: &mut Vec<String>,
    ) -> Result<Vec<Dependency>, String> {
        let drivers = self.drivers(symbol);
        if drivers.is_empty() {
            notes.push("external input / no elaborated driver".into());
            if at.before && self.history(symbol)?.times().contains(&at.time) {
                notes.push(format!("ambiguous scheduling: external input changes at event time {}; VTR has no event-region ordering", at.time));
            }
            return Ok(vec![]);
        }
        if drivers.len() != 1 {
            return Err(format!(
                "ambiguous: {} procedural/continuous drivers at {}",
                drivers.len(),
                drivers
                    .iter()
                    .map(|p| p.source.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let p = &drivers[0];
        match p.mode.as_str() {
            "comb" => {
                let mut x = Execution::default();
                self.execute(&p.body, at, &BTreeSet::new(), false, &mut x, &[])?;
                let (v, src) = x
                    .assigned
                    .remove(symbol)
                    .ok_or("ambiguous combinational hold/latch: no active assignment")?;
                notes.push(format!("active assignment at {src}"));
                self.compare(symbol, at, v.value, notes)?;
                Ok(v.deps)
            }
            "seq" => self.sequential(symbol, at, p, notes),
            _ => Err(format!("unsupported process at {}", p.source)),
        }
    }
    fn compare(
        &mut self,
        symbol: &str,
        at: Moment,
        value: Option<u64>,
        notes: &mut Vec<String>,
    ) -> Result<(), String> {
        let actual = self.sample(symbol, at)?;
        let actual = u64::from_str_radix(&actual, 2).ok();
        match (actual, value) {
            (Some(a), Some(b)) if a != b => notes.push(format!("design/trace mismatch: active assignment predicts {b}, recorded {a}; possible scheduling ambiguity")),
            (_, None) => notes.push("assignment value contains X/Z or unavailable arithmetic result".into()),
            (None, _) => notes.push("recorded value contains X/Z; assignment consistency unverified".into()),
            _ => {},
        }
        Ok(())
    }
    fn sequential(
        &mut self,
        symbol: &str,
        at: Moment,
        p: &Process,
        notes: &mut Vec<String>,
    ) -> Result<Vec<Dependency>, String> {
        let limit = at.tick()?;
        let mut occurrences: BTreeMap<u64, Vec<(String, String, usize, Source)>> = BTreeMap::new();
        let mut event_symbols = BTreeSet::new();
        for e in &p.events {
            let name = match &e.expr.op {
                ExprOp::NamedValue { symbol } | ExprOp::HierarchicalValue { symbol }
                    if e.expr.ty.width == 1 =>
                {
                    symbol
                }
                _ => {
                    return Err(format!(
                        "unsupported clock/reset expression at {}",
                        e.source
                    ))
                }
            };
            if !matches!(e.edge.as_str(), "PosEdge" | "NegEdge") {
                return Err("unsupported event edge".into());
            }
            event_symbols.insert(name.clone());
            let h = self.history(name)?.clone();
            let mut cycle = 0;
            for i in 1..h.len() {
                let t = h.times()[i];
                if t > limit {
                    break;
                }
                if h.times()[i - 1] == t {
                    return Err(format!("ambiguous multiple transitions of {name} at {t}"));
                }
                let a = h.get(i - 1).to_ascii();
                let b = h.get(i).to_ascii();
                let edge = match (e.edge.as_str(), a.as_str(), b.as_str()) {
                    ("PosEdge", "0", "1") | ("NegEdge", "1", "0") => true,
                    (_, "0" | "1", "0" | "1") => false,
                    _ => return Err(format!("ambiguous X/Z event transition on {name} at {t}")),
                };
                if edge {
                    cycle += 1;
                    occurrences.entry(t).or_default().push((
                        name.clone(),
                        e.edge.clone(),
                        cycle,
                        e.source.clone(),
                    ));
                }
            }
        }
        let mut deps = Vec::new();
        for (t, triggers) in occurrences.iter().rev() {
            if self
                .reader
                .blackout()
                .iter()
                .any(|b| b.time >= *t && b.time <= limit)
            {
                return Err(format!(
                    "missing event history: dump gap between {t} and {limit}"
                ));
            }
            for event_symbol in &event_symbols {
                if self.history(event_symbol)?.times().contains(t)
                    && !triggers.iter().any(|(name, _, _, _)| name == event_symbol)
                {
                    return Err(format!("ambiguous scheduling: non-triggering clock/reset transition on {event_symbol} at {t}"));
                }
            }
            let sample = Moment {
                time: *t,
                before: true,
            };
            let mut x = Execution::default();
            // Event signals use post-transition values (notably asynchronous reset).
            self.execute(&p.body, sample, &event_symbols, true, &mut x, &[])?;
            for (name, edge, cycle, src) in triggers {
                notes.push(format!("{edge} {name} @ {t}, event #{cycle}"));
                deps.push(Dependency {
                    symbol: name.clone(),
                    at: Moment {
                        time: *t,
                        before: false,
                    },
                    why: format!("{edge} trigger, event #{cycle}"),
                    source: src.clone(),
                });
            }
            if let Some((v, src)) = x.assigned.remove(symbol) {
                notes.push(format!(
                    "active nonblocking assignment at {src}; inputs sampled before {t}"
                ));
                self.compare(symbol, at, v.value, notes)?;
                deps.extend(v.deps);
                return Ok(deps);
            }
            notes.push(format!(
                "hold at {t}: no active assignment; searching previous event"
            ));
            deps.extend(x.controls);
        }
        notes.push("missing prior assignment event: trace starts after the required history or register is uninitialized".into());
        Ok(deps)
    }
}
fn mask(width: u32) -> u64 {
    if width >= 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    }
}
fn signed(value: u64, width: u32) -> i64 {
    ((value << (64 - width)) as i64) >> (64 - width)
}
fn constant(text: &str, width: u32) -> Result<Option<u64>, String> {
    let text = text.replace('_', "");
    if text.to_ascii_lowercase().contains(['x', 'z', '?']) {
        return Ok(None);
    }
    if let Some((_, digits)) = text.split_once('\'') {
        let digits = digits.trim_start_matches('s');
        if digits == "0" {
            return Ok(Some(0));
        }
        if digits == "1" {
            return Ok(Some(mask(width)));
        }
        let (radix, digits) = match digits.as_bytes().first() {
            Some(b'b') => (2, &digits[1..]),
            Some(b'o') => (8, &digits[1..]),
            Some(b'h') => (16, &digits[1..]),
            Some(b'd') => (10, &digits[1..]),
            _ => return Err(format!("unsupported constant {text}")),
        };
        u64::from_str_radix(digits, radix)
            .map(|v| {
                Some(
                    if text.starts_with('-') {
                        v.wrapping_neg()
                    } else {
                        v
                    } & mask(width),
                )
            })
            .map_err(|e| e.to_string())
    } else {
        text.parse::<i128>()
            .map(|v| Some(v as u64 & mask(width)))
            .map_err(|e| format!("constant {text}: {e}"))
    }
}
fn binary(op: &str, a: u64, b: u64, ty: &Type) -> Result<u64, String> {
    let sa = signed(a, ty.width);
    let sb = signed(b, ty.width);
    Ok(match op {
        "Add" => a.wrapping_add(b),
        "Subtract" => a.wrapping_sub(b),
        "Multiply" => a.wrapping_mul(b),
        "Divide" | "Mod" if b == 0 => {
            return Err("unknown arithmetic result: division by zero".into())
        }
        "Divide" if ty.signed => sa.wrapping_div(sb) as u64,
        "Mod" if ty.signed => sa.wrapping_rem(sb) as u64,
        "Divide" => a / b,
        "Mod" => a % b,
        "BinaryAnd" => a & b,
        "BinaryOr" => a | b,
        "BinaryXor" => a ^ b,
        "BinaryXnor" => !(a ^ b),
        "LogicalAnd" => u64::from(a != 0 && b != 0),
        "LogicalOr" => u64::from(a != 0 || b != 0),
        "Equality" | "CaseEquality" => u64::from(a == b),
        "Inequality" | "CaseInequality" => u64::from(a != b),
        "LessThan" => u64::from(if ty.signed { sa < sb } else { a < b }),
        "LessThanEqual" => u64::from(if ty.signed { sa <= sb } else { a <= b }),
        "GreaterThan" => u64::from(if ty.signed { sa > sb } else { a > b }),
        "GreaterThanEqual" => u64::from(if ty.signed { sa >= sb } else { a >= b }),
        "LogicalShiftLeft" | "ArithmeticShiftLeft" => {
            if b >= 64 {
                0
            } else {
                a << b
            }
        }
        "LogicalShiftRight" => {
            if b >= 64 {
                0
            } else {
                a >> b
            }
        }
        "ArithmeticShiftRight" if ty.signed => (sa >> b.min(63)) as u64,
        "ArithmeticShiftRight" => {
            if b >= 64 {
                0
            } else {
                a >> b
            }
        }
        _ => return Err(format!("unsupported binary operator {op}")),
    })
}

fn statement_targets<'a>(s: &'a Statement, out: &mut BTreeSet<&'a str>) {
    match s {
        Statement::Assign { target, .. } => {
            out.insert(target);
        }
        Statement::Sequence { statements } => {
            for s in statements {
                statement_targets(s, out);
            }
        }
        Statement::If { yes, no, .. } => {
            statement_targets(yes, out);
            statement_targets(no, out);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slang_integer_spellings_preserve_signed_values() {
        assert_eq!(constant("-8'sd1", 8).unwrap(), Some(255));
        assert_eq!(
            constant("64'hffffffffffffffff", 64).unwrap(),
            Some(u64::MAX)
        );
        assert_eq!(constant("'1", 8).unwrap(), Some(255));
        assert_eq!(constant("8'b10xz", 8).unwrap(), None);
    }
}
