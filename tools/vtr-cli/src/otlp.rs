//! OpenTelemetry OTLP/JSON (`ExportTraceServiceRequest` / `TracesData`) -> VTR.
//!
//! * each `resource` -> scope node (`ScopeType::Resource`) with the resource attributes
//! * each instrumentation scope -> stream (kind `otel.scope`) under the resource scope
//! * span name -> generator; spans -> transactions (ns timestamps, timescale -9)
//! * `parentSpanId` -> transaction parent; links -> relations `otel.link` with attributes
//! * events -> transaction events; status -> `TxStatus`; kind -> `TxKind`
//! * ids, trace state, flags and dropped counts -> attributes `otel.*`

use serde_json::Value as J;
use std::collections::HashMap;
use vtr::{AttrPhase, FileType, NodeId, ScopeType, StrId, TxId, TxKind, TxStatus, Value, Writer};

fn any_value(w: &mut Writer, v: &J) -> Value {
    // OTLP JSON encodes AnyValue as {"stringValue": ..} etc.
    if let Some(o) = v.as_object() {
        if let Some(s) = o.get("stringValue").and_then(|x| x.as_str()) {
            return Value::Str(w.intern(s));
        }
        if let Some(b) = o.get("boolValue").and_then(|x| x.as_bool()) {
            return Value::Bool(b);
        }
        if let Some(i) = o.get("intValue") {
            if let Some(n) = i.as_i64() {
                return Value::I64(n);
            }
            if let Some(s) = i.as_str() {
                if let Ok(n) = s.parse::<i64>() {
                    return Value::I64(n);
                }
            }
        }
        if let Some(d) = o.get("doubleValue").and_then(|x| x.as_f64()) {
            return Value::F64(d);
        }
        if let Some(b) = o.get("bytesValue").and_then(|x| x.as_str()) {
            return Value::Bytes(base64_decode(b));
        }
        if let Some(a) = o.get("arrayValue").and_then(|x| x.get("values")).and_then(|x| x.as_array()) {
            return Value::List(a.iter().map(|x| any_value(w, x)).collect());
        }
        if let Some(kv) = o.get("kvlistValue").and_then(|x| x.get("values")).and_then(|x| x.as_array()) {
            return Value::Map(kv_list(w, kv));
        }
        if o.is_empty() {
            return Value::Null;
        }
    }
    // Plain JSON fallbacks.
    match v {
        J::Null => Value::Null,
        J::Bool(b) => Value::Bool(*b),
        J::Number(n) => n.as_i64().map(Value::I64).or_else(|| n.as_f64().map(Value::F64)).unwrap_or(Value::Null),
        J::String(s) => Value::Str(w.intern(s)),
        J::Array(a) => Value::List(a.iter().map(|x| any_value(w, x)).collect()),
        J::Object(_) => Value::Null,
    }
}

fn kv_list(w: &mut Writer, kv: &[J]) -> Vec<(StrId, Value)> {
    kv.iter()
        .filter_map(|e| {
            let k = e.get("key")?.as_str()?;
            let v = e.get("value").map(|v| any_value(w, v)).unwrap_or(Value::Null);
            Some((w.intern(k), v))
        })
        .collect()
}

fn base64_decode(s: &str) -> Vec<u8> {
    let table = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a') as u32 + 26,
            b'0'..=b'9' => (c - b'0') as u32 + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        })
    };
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0;
    for &c in s.as_bytes() {
        if let Some(v) = table(c) {
            acc = (acc << 6) | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
                acc &= (1 << bits) - 1;
            }
        }
    }
    out
}

fn hex_bytes(s: &str) -> Vec<u8> {
    (0..s.len() / 2).filter_map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()).collect()
}

fn id_bytes(s: &str) -> Vec<u8> {
    if s.len() == 32 || s.len() == 16 {
        if let Ok(v) = (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16)).collect::<Result<Vec<u8>, _>>() {
            return v;
        }
    }
    base64_decode(s)
}

fn u64_of(v: Option<&J>) -> u64 {
    match v {
        Some(J::Number(n)) => n.as_u64().unwrap_or(0),
        Some(J::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

struct Span {
    tx: TxId,
    end: u64,
    status: TxStatus,
    span_id: Vec<u8>,
    parent: Vec<u8>,
    links: Vec<(Vec<u8>, Vec<(StrId, Value)>)>,
}

pub fn convert_otlp_json(input: &str, w: &mut Writer) -> Result<(), Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(input)?;
    let root: J = serde_json::from_str(&text)?;
    w.set_timescale(-9)?;
    w.set_file_type(FileType::Software)?;
    let rs = root.get("resourceSpans").and_then(|x| x.as_array()).ok_or("no resourceSpans array")?;
    let k_trace = w.intern("otel.trace_id");
    let k_span = w.intern("otel.span_id");
    let k_state = w.intern("otel.trace_state");
    let k_flags = w.intern("otel.flags");
    let k_msg = w.intern("otel.status_message");
    let k_da = w.intern("otel.dropped_attributes_count");
    let k_de = w.intern("otel.dropped_events_count");
    let k_dl = w.intern("otel.dropped_links_count");
    let k_link = w.intern("otel.link");
    let mut spans: Vec<Span> = Vec::new();
    let mut by_id: HashMap<Vec<u8>, TxId> = HashMap::new();
    let mut gens: HashMap<(NodeId, String), NodeId> = HashMap::new();
    for (ri, r) in rs.iter().enumerate() {
        let res = w.begin_scope(&format!("resource{ri}"), ScopeType::Resource, "");
        if let Some(a) = r.get("resource").and_then(|x| x.get("attributes")).and_then(|x| x.as_array()) {
            for (k, v) in kv_list(w, a) {
                let ks = w.string(k).to_string();
                w.node_attr(res, &ks, v)?;
            }
        }
        if let Some(u) = r.get("schemaUrl").and_then(|x| x.as_str()) {
            let s = w.intern(u);
            w.node_attr(res, "otel.schema_url", Value::Str(s))?;
        }
        w.end_scope()?;
        for ss in r.get("scopeSpans").and_then(|x| x.as_array()).map(|a| a.as_slice()).unwrap_or(&[]) {
            let scope = ss.get("scope");
            let name = scope.and_then(|s| s.get("name")).and_then(|x| x.as_str()).unwrap_or("");
            let stream = w.add_stream(Some(res), name, "otel.scope");
            if let Some(v) = scope.and_then(|s| s.get("version")).and_then(|x| x.as_str()) {
                let s = w.intern(v);
                w.node_attr(stream, "otel.version", Value::Str(s))?;
            }
            if let Some(a) = scope.and_then(|s| s.get("attributes")).and_then(|x| x.as_array()) {
                for (k, v) in kv_list(w, a) {
                    let ks = w.string(k).to_string();
                    w.node_attr(stream, &ks, v)?;
                }
            }
            if let Some(u) = ss.get("schemaUrl").and_then(|x| x.as_str()) {
                let s = w.intern(u);
                w.node_attr(stream, "otel.schema_url", Value::Str(s))?;
            }
            let mut sp: Vec<&J> = ss.get("spans").and_then(|x| x.as_array()).map(|a| a.iter().collect()).unwrap_or_default();
            sp.sort_by_key(|s| u64_of(s.get("startTimeUnixNano")));
            for s in sp {
                let sname = s.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let gen = *gens.entry((stream, sname.clone())).or_insert_with(|| w.add_generator(stream, &sname));
                let start = u64_of(s.get("startTimeUnixNano"));
                let end = u64_of(s.get("endTimeUnixNano")).max(start);
                let tx = w.begin_tx(gen, start)?;
                let kind = match s.get("kind") {
                    Some(J::Number(n)) => TxKind::from_u8(n.as_u64().unwrap_or(0) as u8),
                    Some(J::String(k)) => match k.as_str() {
                        "SPAN_KIND_INTERNAL" => TxKind::Internal,
                        "SPAN_KIND_SERVER" => TxKind::Server,
                        "SPAN_KIND_CLIENT" => TxKind::Client,
                        "SPAN_KIND_PRODUCER" => TxKind::Producer,
                        "SPAN_KIND_CONSUMER" => TxKind::Consumer,
                        _ => TxKind::Unspecified,
                    },
                    _ => TxKind::Unspecified,
                };
                w.set_tx_kind(tx, kind)?;
                let trace_id = id_bytes(s.get("traceId").and_then(|x| x.as_str()).unwrap_or(""));
                let span_id = id_bytes(s.get("spanId").and_then(|x| x.as_str()).unwrap_or(""));
                let parent = id_bytes(s.get("parentSpanId").and_then(|x| x.as_str()).unwrap_or(""));
                w.tx_attr(tx, k_trace, AttrPhase::Begin, &Value::Bytes(trace_id))?;
                w.tx_attr(tx, k_span, AttrPhase::Begin, &Value::Bytes(span_id.clone()))?;
                if let Some(ts) = s.get("traceState").and_then(|x| x.as_str()) {
                    if !ts.is_empty() {
                        let v = w.intern(ts);
                        w.tx_attr(tx, k_state, AttrPhase::Begin, &Value::Str(v))?;
                    }
                }
                let flags = u64_of(s.get("flags"));
                if flags != 0 {
                    w.tx_attr(tx, k_flags, AttrPhase::Begin, &Value::U64(flags))?;
                }
                if let Some(a) = s.get("attributes").and_then(|x| x.as_array()) {
                    for (k, v) in kv_list(w, a) {
                        w.tx_attr(tx, k, AttrPhase::Record, &v)?;
                    }
                }
                for (key, field) in [(k_da, "droppedAttributesCount"), (k_de, "droppedEventsCount"), (k_dl, "droppedLinksCount")] {
                    let n = u64_of(s.get(field));
                    if n != 0 {
                        w.tx_attr(tx, key, AttrPhase::Record, &Value::U64(n))?;
                    }
                }
                for e in s.get("events").and_then(|x| x.as_array()).map(|a| a.as_slice()).unwrap_or(&[]) {
                    let t = u64_of(e.get("timeUnixNano"));
                    let n = e.get("name").and_then(|x| x.as_str()).unwrap_or("");
                    let ns = w.intern(n);
                    let mut attrs = e.get("attributes").and_then(|x| x.as_array()).map(|a| kv_list(w, a)).unwrap_or_default();
                    let d = u64_of(e.get("droppedAttributesCount"));
                    if d != 0 {
                        attrs.push((k_da, Value::U64(d)));
                    }
                    w.tx_event(tx, t, ns, &attrs)?;
                }
                let mut links = Vec::new();
                for l in s.get("links").and_then(|x| x.as_array()).map(|a| a.as_slice()).unwrap_or(&[]) {
                    let target = id_bytes(l.get("spanId").and_then(|x| x.as_str()).unwrap_or(""));
                    let mut attrs = l.get("attributes").and_then(|x| x.as_array()).map(|a| kv_list(w, a)).unwrap_or_default();
                    attrs.push((k_trace, Value::Bytes(id_bytes(l.get("traceId").and_then(|x| x.as_str()).unwrap_or("")))));
                    if let Some(ts) = l.get("traceState").and_then(|x| x.as_str()) {
                        if !ts.is_empty() {
                            let v = w.intern(ts);
                            attrs.push((k_state, Value::Str(v)));
                        }
                    }
                    let f = u64_of(l.get("flags"));
                    if f != 0 {
                        attrs.push((k_flags, Value::U64(f)));
                    }
                    links.push((target, attrs));
                }
                let (status, msg) = match s.get("status") {
                    Some(st) => {
                        let code = match st.get("code") {
                            Some(J::Number(n)) => n.as_u64().unwrap_or(0),
                            Some(J::String(c)) => match c.as_str() {
                                "STATUS_CODE_OK" => 1,
                                "STATUS_CODE_ERROR" => 2,
                                _ => 0,
                            },
                            _ => 0,
                        };
                        let status = match code {
                            1 => TxStatus::Ok,
                            2 => TxStatus::Error,
                            _ => TxStatus::Unset,
                        };
                        (status, st.get("message").and_then(|x| x.as_str()).map(|s| s.to_string()))
                    }
                    None => (TxStatus::Unset, None),
                };
                if let Some(m) = msg {
                    if !m.is_empty() {
                        let ms = w.intern(&m);
                        w.tx_attr(tx, k_msg, AttrPhase::End, &Value::Str(ms))?;
                    }
                }
                by_id.insert(span_id.clone(), tx);
                spans.push(Span { tx, end, status, span_id, parent, links });
            }
        }
    }
    let _ = hex_bytes;
    // Parents (all ids are known now), then ends in end-time order, then links.
    for s in &spans {
        if !s.parent.is_empty() {
            if let Some(&p) = by_id.get(&s.parent) {
                w.set_tx_parent(s.tx, p)?;
            }
        }
    }
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by_key(|&i| spans[i].end);
    for i in order {
        let s = &spans[i];
        w.end_tx(s.tx, s.end, s.status)?;
    }
    for s in &spans {
        for (target, attrs) in &s.links {
            match by_id.get(target) {
                Some(&t) => w.relate(k_link, s.tx, t, attrs)?,
                None => {
                    // Link to a span outside this file: keep the raw id.
                    let mut a = attrs.clone();
                    a.push((k_span, Value::Bytes(target.clone())));
                    w.relate(k_link, s.tx, 0, &a)?;
                }
            }
        }
    }
    let _ = &spans.iter().map(|s| &s.span_id).count();
    Ok(())
}
