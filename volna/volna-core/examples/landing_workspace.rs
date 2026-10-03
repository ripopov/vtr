//! Writes the Volna workspace of the landing-page demo
//! (docs/volna-landing.html) beside `landing.vtr`: waves with groups, a folded
//! group, transaction lanes, analog plots, a clock row and clock rulers; a
//! pipeline; a pinned Transaction panel; a table; named markers. The layout
//! centres on the interrupt: its handler's uncached read goes to DRAM.
//!
//! Times and the pinned record come from the trace itself, so regenerating the
//! trace only needs this rerun. The result is restored into a headless `App`
//! (every row, track and clock must resolve without a notice) and written in
//! the canonical form `Workspace::capture` produces.
//!
//! cargo run -p volna-core --example landing_workspace -- volna/volna/examples/landing.vtr

use std::path::PathBuf;

use serde_json::{Value, json};
use volna_core::App;
use volna_core::workspace::Workspace;
use volna_trace::data::transactions::{AttributeValue, Transaction};
use volna_trace::session::OpenSpec;

fn label(t: &Transaction) -> Option<&str> {
    t.attributes
        .iter()
        .find_map(|a| match (&a.key[..], &a.value) {
            ("vtr.label", AttributeValue::Text(text)) => Some(text.as_str()),
            _ => None,
        })
}

fn main() -> anyhow::Result<()> {
    let trace = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: landing_workspace TRACE.vtr"),
    )
    .canonicalize()?;
    let name = trace.file_name().unwrap().to_string_lossy().into_owned();
    let output = trace.with_file_name(format!("{name}.volna.json"));
    let trace_uri = url::Url::from_file_path(&trace).unwrap().to_string();
    let location = url::Url::from_file_path(&output).unwrap().to_string();

    let session = OpenSpec::Path(trace.clone()).open()?;
    let info = session.info().clone();
    let (first, last) = info.time_range;
    let pipeline = session
        .tracks()
        .iter()
        .find(|t| t.path.join(".") == "soc.cpu0.pipeline")
        .expect("soc.cpu0.pipeline")
        .id;
    let loaded = session.load_track(pipeline)?;
    // Pipeline rows are the stream's records in (begin, end, id) order.
    let mut rows: Vec<&Transaction> = loaded
        .generators
        .iter()
        .flat_map(|g| g.transactions())
        .collect();
    rows.sort_by_key(|t| (t.begin, t.end, t.id.0));
    let handler = rows
        .iter()
        .position(|t| label(t).is_some_and(|l| l.ends_with("csrrw sp, mscratch, sp")))
        .expect("the interrupt handler ran");
    let read = rows[handler..]
        .iter()
        .find(|t| label(t).is_some_and(|l| l.ends_with("lw   t1, 64(gp)")))
        .expect("the handler reads the device");
    let irq = rows[handler].begin - 2; // the event is one core cycle before the handler's fetch
    let memory = read
        .stages
        .iter()
        .find(|s| s.name == "M")
        .expect("the device read has a memory stage");
    let memory_end = memory.end.expect("the device read completed");
    let (view_start, view_end) = (irq as f64 - 40.0, memory_end as f64 + 50.0);
    // Halfway through the refill, on a rising core clock edge like `memory.begin`.
    let cursor = memory.begin + (memory_end - memory.begin) / 4 * 2;

    let signal = |path: &str, format: &str| json!({"type": "signal", "signal": path.split('.').collect::<Vec<_>>(), "format": format});
    let plot = |path: &str, height: u8, draw: &str| {
        json!({"type": "signal", "signal": path.split('.').collect::<Vec<_>>(), "format": "real",
               "height": height, "analog": {"draw": draw, "range": "trace"}})
    };
    let lane =
        |path: &str| json!({"type": "lane", "generator": path.split('.').collect::<Vec<_>>()});
    let link = json!({"viewport": true, "cursor": true});
    let waves = json!({
        "id": 1, "kind": "waves", "version": 6, "title": null, "link": link,
        "scroll_y": 0.0, "columns": {"names": 150.0, "values": 110.0},
        "rows": [
            {"type": "group", "name": "cpu0", "rows": [
                signal("soc.cpu0.phase", "text"),
                signal("soc.cpu0.pc", "hex"),
                signal("soc.cpu0.irq", "bit"),
                signal("soc.cpu0.stall", "bit"),
                signal("soc.cpu0.flush", "bit"),
                signal("soc.cpu0.retired", "udec"),
            ]},
            lane("soc.l2.bus.read"),
            lane("soc.l2.bus.write"),
            lane("soc.dram.bus.burst"),
            {"type": "group", "name": "l2 port", "collapsed": true, "rows": [
                signal("soc.l2.req_valid", "bit"),
                signal("soc.l2.req_write", "bit"),
                signal("soc.l2.req_addr", "hex"),
                signal("soc.l2.resp_valid", "bit"),
                signal("soc.l2.resp_data", "hex"),
            ]},
            {"type": "group", "name": "perf", "rows": [
                plot("soc.perf.ipc", 3, "linear"),
                plot("soc.perf.l2_bandwidth_gbps", 3, "step"),
                plot("soc.perf.temperature_c", 2, "linear"),
            ]},
            {"type": "signal", "signal": ["soc", "l2", "mshr_used"], "format": "udec",
             "height": 2, "analog": {"draw": "step", "range": "trace"}},
            {"type": "clock", "clock": "soc.dram.dram_clk"},
            signal("soc.reset_n", "bit"),
        ],
        "selected": [],
        "clocks": {"rulers": [["A", "soc.clk"], ["A", "soc.dram.dram_clk"]],
                   "selected": ["A", "soc.clk"]},
    });
    let pipeline_panel = json!({
        "follow": "following", "id": 2, "kind": "pipeline", "version": 1, "title": null,
        "track": ["soc", "cpu0", "pipeline"], "link": link,
        "rows": {"top": handler as f64 - 7.0, "row_px": 18.0}, "label_width": 150.0,
    });
    let transaction = json!({
        "id": 3, "kind": "transaction", "version": 1, "title": null,
        "pinned": {"track": ["soc", "cpu0", "pipeline", "instruction"], "id": read.id.0},
        "collapsed": [], "radix": {"address": "hex", "pc": "hex"},
    });
    let table = json!({
        "id": 4, "kind": "table", "version": 2, "title": null,
        "source": {"type": "generator", "path": ["soc", "l2", "bus", "read"]},
        "columns": ["label", "begin", "duration", "status", "attributes"], "link": link,
    });
    let workspace = json!({
        "format": volna_core::workspace::FORMAT,
        "version": volna_core::workspace::VERSION,
        "traces": [{"letter": "A", "path": name, "name": name, "timescale": info.timescale,
                    "time_range": [first, last], "design_id": info.design_id}],
        "timescale": info.timescale,
        "layout": {"split": "horizontal", "sizes": [0.69, 0.31], "children": [
            {"split": "vertical", "sizes": [0.56, 0.44], "children": [
                {"tabs": [1], "active": 1}, {"tabs": [2], "active": 2}]},
            {"split": "vertical", "sizes": [0.62, 0.38], "children": [
                {"tabs": [3], "active": 3}, {"tabs": [4], "active": 4}]},
        ]},
        "focused": 2,
        "panels": [waves, pipeline_panel, transaction, table],
        "shared": {
            "viewport": {"start": view_start, "end": view_end},
            "cursor": cursor,
            "markers": [
                {"id": 1, "time": irq, "label": "irq"},
                {"id": 2, "time": memory_end, "label": "refill done"},
            ],
        },
        "sidebar": {"visible": true, "width": 190.0, "scopes_fraction": 0.42,
                    "selected_scope": ["A", ["soc", "cpu0"]],
                    "expanded": [["A", ["soc"]], ["A", ["soc", "cpu0"]], ["A", ["soc", "l2"]]],
                    "filter": ""},
    });

    // Restore it exactly as a host would, then save the canonical capture.
    let mut app = App::new();
    app.open_resource(OpenSpec::Path(trace.clone()), trace_uri.clone());
    volna_core::testing::complete_open(&mut app, session.clone());
    let plan =
        Workspace::parse(&serde_json::to_vec(&workspace)?)?.prepare(&app, &trace_uri, &location)?;
    anyhow::ensure!(
        plan.report().notices.is_empty(),
        "restore notices: {:?}",
        plan.report().notices
    );
    plan.commit(&mut app)?;
    let captured = Workspace::capture(&app, |_| Ok(Some(name.clone())), None)?;
    // Layout fractions are f32: compare with the written file read back the same way.
    let written: Value =
        serde_json::from_slice(&Workspace::parse(&serde_json::to_vec(&workspace)?)?.to_bytes()?)?;
    let value: Value = serde_json::from_slice(&captured.to_bytes()?)?;
    anyhow::ensure!(
        value == written,
        "the capture differs from the written workspace:\n{value:#}"
    );
    std::fs::write(&output, captured.to_bytes()?)?;
    println!(
        "{}: irq at {irq}, handler row {handler}, pinned record {} ({}..{}), cursor {cursor}",
        output.display(),
        read.id.0,
        read.begin,
        read.end
    );
    Ok(())
}
