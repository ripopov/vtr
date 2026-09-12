//! Module-local RTL connectivity. Children are opaque, navigable blocks.
use crate::{Database, Debugger, Expr, ExprOp, Instance, Moment, Process, Statement};
use serde_json::Value;
#[cfg(feature = "layout")]
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

#[derive(Clone, Debug)]
pub struct Pin {
    pub name: String,
    pub symbol: Option<String>,
    pub output: bool,
}
#[derive(Clone, Debug)]
pub struct Block {
    pub id: String,
    pub title: String,
    pub detail: String,
    /// Declaration or expression location for navigation, separate from labels.
    pub source: Option<crate::Source>,
    /// Instance path to open when drilling into a child module.
    pub child: Option<String>,
    pub pins: Vec<Pin>,
}
#[derive(Clone, Debug)]
pub struct Wire {
    pub source: (usize, usize),
    pub target: (usize, usize),
}
/// A reusable structural view. Changing time does not rebuild or relayout it.
#[derive(Clone, Debug)]
pub struct Netlist {
    pub instance: String,
    pub blocks: Vec<Block>,
    pub wires: Vec<Wire>,
}
#[derive(Default)]
struct Members<'a> {
    children: Vec<&'a Instance>,
    symbols: Vec<&'a str>,
    processes: Vec<&'a Process>,
    connections: Vec<&'a crate::Connection>,
}
/// Builds ownership indexes once; a view visits only the selected module and its ports.
pub struct NetlistIndex<'a> {
    db: &'a Database,
    instances: BTreeMap<&'a str, &'a Instance>,
    members: BTreeMap<&'a str, Members<'a>>,
}
impl<'a> NetlistIndex<'a> {
    pub fn new(db: &'a Database) -> Result<Self, String> {
        let mut instances = BTreeMap::new();
        let mut members: BTreeMap<&str, Members> = BTreeMap::new();
        for i in &db.instances {
            if instances.insert(i.path.as_str(), i).is_some() {
                return Err(format!("duplicate VDB instance: {}", i.path));
            }
            members.entry(&i.path).or_default();
        }
        for i in &db.instances {
            if let Some(p) = &i.parent {
                members
                    .get_mut(p.as_str())
                    .ok_or_else(|| format!("unknown parent: {p}"))?
                    .children
                    .push(i);
            }
            let mut seen = BTreeSet::new();
            for p in &i.ports {
                let s = db
                    .symbols
                    .get(&p.symbol)
                    .ok_or_else(|| format!("unknown port symbol: {}", p.symbol))?;
                if s.owner != i.path
                    || !seen.insert(&p.name)
                    || !matches!(p.direction.as_str(), "In" | "Out" | "InOut" | "Ref")
                {
                    return Err(format!("invalid port: {}.{}", i.path, p.name));
                }
            }
            let mut ancestors = BTreeSet::new();
            let mut current = Some(i);
            while let Some(node) = current {
                if !ancestors.insert(&node.path) {
                    return Err("cyclic VDB hierarchy".into());
                }
                current = node
                    .parent
                    .as_ref()
                    .and_then(|p| instances.get(p.as_str()).copied());
            }
        }
        for (path, s) in &db.symbols {
            members
                .get_mut(s.owner.as_str())
                .ok_or_else(|| format!("unknown symbol owner: {}", s.owner))?
                .symbols
                .push(path);
        }
        let mut ids = BTreeSet::new();
        for p in &db.processes {
            for s in p.reads.iter().chain(&p.targets) {
                if !db.symbols.contains_key(s) {
                    return Err(format!("unknown process symbol: {s}"));
                }
            }
            if !ids.insert(p.id) {
                return Err(format!("duplicate process: {}", p.id));
            }
            if !matches!(p.origin.as_str(), "rtl" | "connection") {
                return Err(format!("unknown process origin: {}", p.origin));
            }
            let m = members
                .get_mut(p.owner.as_str())
                .ok_or_else(|| format!("unknown process owner: {}", p.owner))?;
            if p.origin == "rtl" {
                m.processes.push(p);
            }
        }
        for c in &db.connections {
            let i = instances
                .get(c.instance.as_str())
                .ok_or_else(|| format!("unknown connection instance: {}", c.instance))?;
            if !i
                .ports
                .iter()
                .any(|p| p.symbol == c.port && p.direction == c.direction)
            {
                return Err(format!("invalid connection port: {}", c.port));
            }
            if let Some(parent) = &i.parent {
                members
                    .get_mut(parent.as_str())
                    .unwrap()
                    .connections
                    .push(c);
            }
        }
        Ok(Self {
            db,
            instances,
            members,
        })
    }
    pub fn module(&self, path: &str) -> Result<Netlist, String> {
        let instance = self
            .instances
            .get(path)
            .ok_or_else(|| format!("unknown module instance: {path}"))?;
        let m = &self.members[path];
        let mut b = Builder {
            db: self.db,
            graph: Netlist {
                instance: path.into(),
                blocks: vec![],
                wires: vec![],
            },
            signals: BTreeMap::new(),
        };
        for s in &m.symbols {
            if self.db.symbols[*s].value.is_none() {
                b.signal(s);
            }
        }
        // Annotate external module boundaries, including unconnected ports.
        for port in &instance.ports {
            let id = b.signal(&port.symbol);
            b.graph.blocks[id].detail = format!(
                "{} port · {} bits",
                port.direction, self.db.symbols[&port.symbol].ty.width
            );
        }
        let mut children = BTreeMap::new();
        for i in &m.children {
            let pins = i
                .ports
                .iter()
                .map(|p| Pin {
                    name: if matches!(p.direction.as_str(), "InOut" | "Ref") {
                        format!("↔ {}", p.name)
                    } else {
                        p.name.clone()
                    },
                    symbol: Some(p.symbol.clone()),
                    output: p.direction == "Out",
                })
                .collect();
            let id = b.block(
                format!("{} : {}", relative(&i.path, path), i.definition),
                format!("module · {}", i.source),
                Some(i.path.clone()),
                pins,
            );
            b.graph.blocks[id].source = Some(i.source.clone());
            children.insert(i.path.as_str(), id);
        }
        for c in &m.connections {
            let id = children[c.instance.as_str()];
            let pin = b.graph.blocks[id]
                .pins
                .iter()
                .position(|p| p.symbol.as_deref() == Some(&c.port))
                .unwrap();
            if c.direction == "Out" {
                match &c.expression.op {
                    ExprOp::NamedValue { symbol } | ExprOp::HierarchicalValue { symbol } => {
                        let dest = b.signal(symbol);
                        b.wire((id, pin), (dest, 0));
                    }
                    _ => {
                        let dest = b.block(
                            "unsupported output connection".into(),
                            format!("{}", c.source),
                            None,
                            vec![pin_named("in", false)],
                        );
                        b.graph.blocks[dest].source = Some(c.source.clone());
                        b.wire((id, pin), (dest, 0));
                    }
                }
            } else {
                let source = b.expression(&c.expression);
                b.wire(source, (id, pin));
                if c.direction != "In" {
                    b.graph.blocks[id]
                        .detail
                        .push_str(" · bidirectional/ref connection (not directional logic)");
                }
            }
        }
        for p in &m.processes {
            if p.mode == "comb" {
                if let Statement::Assign { target, value, .. } = &p.body {
                    let source = b.expression(value);
                    let dest = b.signal(target);
                    b.wire(source, (dest, 0));
                    continue;
                }
            }
            let refs: BTreeSet<&str> = p.reads.iter().map(String::as_str).collect();
            let mut pins: Vec<_> = refs
                .iter()
                .map(|s| Pin {
                    name: relative(s, path).into(),
                    symbol: Some((*s).into()),
                    output: false,
                })
                .collect();
            pins.extend(p.targets.iter().map(|s| Pin {
                name: relative(s, path).into(),
                symbol: Some(s.clone()),
                output: true,
            }));
            let title = match p.mode.as_str() {
                "seq" => "clocked process",
                "comb" => "combinational process",
                _ => "unsupported process",
            };
            let mut detail = format!("{} · {}", p.id, p.source);
            for e in &p.events {
                write!(detail, " · {} {}", e.edge, expr_label(&e.expr)).unwrap();
            }
            if let Statement::Unsupported { reason, .. } = &p.body {
                write!(detail, " · {reason}").unwrap();
            }
            let id = b.block(title.into(), detail, None, pins);
            b.graph.blocks[id].source = Some(p.source.clone());
            for (k, s) in refs.iter().enumerate() {
                let src = b.signal(s);
                b.wire((src, 1), (id, k));
            }
            for (k, s) in p.targets.iter().enumerate() {
                let dst = b.signal(s);
                b.wire((id, refs.len() + k), (dst, 0));
            }
        }
        Ok(b.graph)
    }
}
fn relative<'a>(s: &'a str, parent: &str) -> &'a str {
    s.strip_prefix(parent)
        .and_then(|s| s.strip_prefix('.'))
        .unwrap_or(s)
}
fn pin_named(name: &str, output: bool) -> Pin {
    Pin {
        name: name.into(),
        symbol: None,
        output,
    }
}
struct Builder<'a> {
    db: &'a Database,
    graph: Netlist,
    signals: BTreeMap<String, usize>,
}
impl Builder<'_> {
    fn block(
        &mut self,
        title: String,
        detail: String,
        child: Option<String>,
        pins: Vec<Pin>,
    ) -> usize {
        let id = self.graph.blocks.len();
        self.graph.blocks.push(Block {
            id: format!("n{id}"),
            title,
            detail,
            source: None,
            child,
            pins,
        });
        id
    }
    fn signal(&mut self, s: &str) -> usize {
        if let Some(id) = self.signals.get(s) {
            return *id;
        }
        let detail = match self.db.symbols.get(s) {
            Some(v) => format!(
                "{} · {} bits",
                if v.owner == self.graph.instance {
                    "signal"
                } else {
                    "external reference"
                },
                v.ty.width
            ),
            None => "unknown signal".into(),
        };
        let id = self.block(
            relative(s, &self.graph.instance).into(),
            detail,
            None,
            vec![
                Pin {
                    name: "in".into(),
                    symbol: None,
                    output: false,
                },
                Pin {
                    name: "value".into(),
                    symbol: Some(s.into()),
                    output: true,
                },
            ],
        );
        self.graph.blocks[id].source = self.db.symbols.get(s).map(|symbol| symbol.source.clone());
        self.signals.insert(s.into(), id);
        id
    }
    fn wire(&mut self, source: (usize, usize), target: (usize, usize)) {
        self.graph.wires.push(Wire { source, target });
    }
    fn expression(&mut self, e: &Expr) -> (usize, usize) {
        let mut args: Vec<(&str, &Expr)> = vec![];
        let label = match &e.op {
            ExprOp::NamedValue { symbol } | ExprOp::HierarchicalValue { symbol } => {
                return (self.signal(symbol), 1)
            }
            ExprOp::Constant { value } => format!("constant {value}"),
            ExprOp::BinaryOp { op, left, right } => {
                args.extend([("a", left.as_ref()), ("b", right.as_ref())]);
                op.clone()
            }
            ExprOp::UnaryOp { op, operand } => {
                args.push(("in", operand));
                op.clone()
            }
            ExprOp::Conversion { operand } => {
                args.push(("in", operand));
                format!("cast {} bits", e.ty.width)
            }
            ExprOp::ConditionalOp { cond, yes, no } => {
                args.extend([
                    ("select", cond.as_ref()),
                    ("1", yes.as_ref()),
                    ("0", no.as_ref()),
                ]);
                "mux".into()
            }
            ExprOp::Replication { count, operand } => {
                args.push(("in", operand));
                format!("repeat {count}")
            }
            ExprOp::RangeSelect {
                value,
                left,
                right,
                selection,
                ..
            } => {
                args.extend([
                    ("value", value.as_ref()),
                    ("left", left.as_ref()),
                    ("right", right.as_ref()),
                ]);
                format!("slice {selection}")
            }
            ExprOp::Concatenation { operands } => {
                for o in operands {
                    args.push(("part", o));
                }
                "concat".into()
            }
            ExprOp::ElementSelect {
                value, selector, ..
            } => {
                args.extend([("value", value.as_ref()), ("index", selector.as_ref())]);
                "select bit".into()
            }
            ExprOp::Unsupported { reason, reads } => {
                let mut pins: Vec<_> = reads
                    .iter()
                    .map(|s| Pin {
                        name: relative(s, &self.graph.instance).into(),
                        symbol: Some(s.clone()),
                        output: false,
                    })
                    .collect();
                pins.push(pin_named("out", true));
                let id = self.block(
                    format!("unsupported: {reason}"),
                    format!("{}", e.source),
                    None,
                    pins,
                );
                self.graph.blocks[id].source = Some(e.source.clone());
                for (k, s) in reads.iter().enumerate() {
                    let src = self.signal(s);
                    self.wire((src, 1), (id, k));
                }
                return (id, reads.len());
            }
        };
        let mut pins: Vec<_> = args
            .iter()
            .enumerate()
            .map(|(i, (name, _))| pin_named(&format!("{name}[{i}]"), false))
            .collect();
        pins.push(pin_named("out", true));
        let out = args.len();
        let id = self.block(
            label,
            format!("{} bits · {}", e.ty.width, e.source),
            None,
            pins,
        );
        self.graph.blocks[id].source = Some(e.source.clone());
        for (k, (_, arg)) in args.into_iter().enumerate() {
            let source = self.expression(arg);
            self.wire(source, (id, k));
        }
        (id, out)
    }
}
fn expr_label(e: &Expr) -> String {
    match &e.op {
        ExprOp::NamedValue { symbol } | ExprOp::HierarchicalValue { symbol } => symbol.clone(),
        _ => "expression".into(),
    }
}
/// ELK positions retained independently of timestamp annotations.
pub struct LaidOutNetlist {
    pub netlist: Netlist,
    pub geometry: Value,
}
impl Netlist {
    #[cfg(feature = "layout")]
    pub fn layout(self) -> Result<LaidOutNetlist, String> {
        let nodes:Vec<_>=self.blocks.iter().map(|b| {
            let width=380.0_f64.max(b.title.chars().count() as f64*8.0+24.0);
            let height=70.0+b.pins.iter().filter(|p|p.output).count().max(b.pins.iter().filter(|p|!p.output).count()) as f64*38.0;
            let mut sides=[0,0];
            let ports:Vec<_>=b.pins.iter().enumerate().map(|(i,p)| {
                let side=usize::from(p.output); let y=65.0+sides[side] as f64*38.0; sides[side]+=1;
                json!({"id":format!("{}p{i}",b.id),"x":if p.output {width}else{0.0},"y":y,"width":0,"height":0,"layoutOptions":{"elk.port.side":if p.output{"EAST"}else{"WEST"}}})
            }).collect();
            json!({"id":b.id,"width":width,"height":height,"ports":ports,"layoutOptions":{"elk.portConstraints":"FIXED_POS"}})
        }).collect();
        let edges:Vec<_>=self.wires.iter().enumerate().map(|(i,w)| json!({"id":format!("e{i}"),"sources":[format!("n{}p{}",w.source.0,w.source.1)],"targets":[format!("n{}p{}",w.target.0,w.target.1)]})).collect();
        let graph = json!({"id":"root","layoutOptions":{"elk.algorithm":"layered","elk.direction":"RIGHT","elk.edgeRouting":"ORTHOGONAL","elk.spacing.nodeNode":"50","elk.layered.spacing.nodeNodeBetweenLayers":"90","elk.randomSeed":"1"},"children":nodes,"edges":edges});
        let mut geometry = elkrs::create_elk()
            .layout_json(&graph.to_string())
            .map_err(|e| format!("ELK layout: {e}"))?;
        // ELK omits empty collections when serializing its typed graph.
        let object = geometry
            .as_object_mut()
            .ok_or("ELK returned a non-object")?;
        object.entry("children").or_insert(json!([]));
        object.entry("edges").or_insert(json!([]));
        for node in object["children"]
            .as_array_mut()
            .ok_or("invalid ELK children")?
        {
            node.as_object_mut()
                .ok_or("invalid ELK node")?
                .entry("ports")
                .or_insert(json!([]));
        }
        Ok(LaidOutNetlist {
            netlist: self,
            geometry,
        })
    }
}
fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn short(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    } else {
        s.into()
    }
}
fn number(v: &Value, k: &str) -> Result<f64, String> {
    v[k].as_f64()
        .filter(|n| n.is_finite())
        .ok_or_else(|| format!("invalid ELK coordinate: {k}"))
}
impl LaidOutNetlist {
    /// Values are recorded, settled samples. Missing data is visible and never coerced to zero.
    pub fn svg(&self, debug: &mut Debugger<'_>, time: u64) -> Result<String, String> {
        let mut values = BTreeMap::new();
        for b in &self.netlist.blocks {
            for p in &b.pins {
                if let Some(s) = &p.symbol {
                    values.entry(s.clone()).or_insert_with(|| {
                        debug.sample(
                            s,
                            Moment {
                                time,
                                before: false,
                            },
                        )
                    });
                }
            }
        }
        let w = (number(&self.geometry, "width")? + 40.0)
            .max(self.netlist.instance.chars().count() as f64 * 9.0 + 420.0);
        let h = (number(&self.geometry, "height")? + 80.0).max(130.0);
        let mut svg=format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.1}\" height=\"{h:.1}\" viewBox=\"0 0 {w:.1} {h:.1}\">\n<title>{} @ {time}</title>\n<rect width=\"100%\" height=\"100%\" fill=\"#f8fafc\"/>\n<style>text {{ font-family: monospace; font-size: 12px; fill: #172b4d; }} .title {{ font-size: 15px; font-weight: bold; }} .detail {{ fill: #52637a; font-size: 10px; }} .missing {{ fill: #b42318; }} .value {{ fill: #075985; }}</style>\n<text x=\"20\" y=\"26\" class=\"title\">{} @ {time} (1e{} s ticks)</text>\n<g transform=\"translate(20 60)\">\n",xml(&self.netlist.instance),xml(&self.netlist.instance),debug.reader.meta().timescale);
        if self.netlist.blocks.is_empty() {
            svg.push_str("<text x=\"0\" y=\"20\">No local logic or ports.</text>\n");
        }
        if let Some(edges) = self.geometry["edges"].as_array() {
            for e in edges {
                if let Some(sections) = e["sections"].as_array() {
                    for section in sections {
                        let mut points = vec![&section["startPoint"]];
                        if let Some(bends) = section["bendPoints"].as_array() {
                            points.extend(bends);
                        }
                        points.push(&section["endPoint"]);
                        let coords: Result<Vec<_>, String> = points
                            .iter()
                            .map(|p| Ok(format!("{:.1},{:.1}", number(p, "x")?, number(p, "y")?)))
                            .collect();
                        writeln!(svg,"<polyline data-edge=\"{}\" points=\"{}\" fill=\"none\" stroke=\"#64748b\" stroke-width=\"1.5\"/>",xml(e["id"].as_str().unwrap_or("")),coords?.join(" ")).unwrap();
                    }
                }
            }
        }
        let nodes: BTreeMap<_, _> = self.geometry["children"]
            .as_array()
            .ok_or("ELK missing nodes")?
            .iter()
            .filter_map(|n| n["id"].as_str().map(|id| (id, n)))
            .collect();
        for b in &self.netlist.blocks {
            let n = nodes.get(b.id.as_str()).ok_or("ELK missing block")?;
            let x = number(n, "x")?;
            let y = number(n, "y")?;
            let width = number(n, "width")?;
            let height = number(n, "height")?;
            writeln!(svg,"<g data-node=\"{}\"{}><title>{}</title><rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{width:.1}\" height=\"{height:.1}\" rx=\"6\" fill=\"{}\" stroke=\"#334155\"/>",b.id,b.child.as_ref().map(|c|format!(" data-instance=\"{}\"",xml(c))).unwrap_or_default(),xml(&b.detail),if b.child.is_some(){"#e0f2fe"}else{"#ffffff"}).unwrap();
            writeln!(svg,"<text x=\"{:.1}\" y=\"{:.1}\" class=\"title\">{}</text><text x=\"{:.1}\" y=\"{:.1}\" class=\"detail\">{}</text>",x+12.0,y+22.0,xml(&b.title),x+12.0,y+40.0,xml(&short(&b.detail,((width-24.0)/6.0) as usize))).unwrap();
            for (i, p) in b.pins.iter().enumerate() {
                let port = n["ports"]
                    .as_array()
                    .and_then(|ps| ps.iter().find(|v| v["id"] == format!("{}p{i}", b.id)))
                    .ok_or("ELK missing pin")?;
                let px = x + number(port, "x")?;
                let py = y + number(port, "y")?;
                let (value, tip, class) = match p.symbol.as_ref().map(|s| (&values[s], s)) {
                    Some((Ok(v), s)) => (short(v, 20), format!("{s} = {v}"), "value"),
                    Some((Err(e), s)) => ("unavailable".into(), format!("{s}: {e}"), "missing"),
                    None => (String::new(), p.name.clone(), "detail"),
                };
                writeln!(svg,"<g data-pin=\"{}p{i}\"><title>{}</title><circle cx=\"{px:.1}\" cy=\"{py:.1}\" r=\"3\" fill=\"#334155\"/><text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"{}\">{}</text><text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"{}\" class=\"{class}\">{}</text></g>",b.id,xml(&tip),px+if p.output{-8.0}else{8.0},py-4.0,if p.output{"end"}else{"start"},xml(&short(&p.name,20)),px+if p.output{-8.0}else{8.0},py+11.0,if p.output{"end"}else{"start"},xml(&value)).unwrap();
            }
            svg.push_str("</g>\n");
        }
        svg.push_str("</g>\n</svg>\n");
        Ok(svg)
    }
}
