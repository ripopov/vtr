//! Transactions with attributes, pipeline stages, and dependency relations.

use vtr::{Reader, ScopeType, TxQuery, TxStatus, Value, Writer};

fn main() -> vtr::Result<()> {
    let temporary = tempfile::tempdir()?;
    let path = std::env::args_os().nth(1).map_or_else(
        || temporary.path().join("transactions.vtr"),
        std::path::PathBuf::from,
    );
    let mut w = Writer::create(&path)?;
    let core = w.add_scope(None, "cpu", ScopeType::Core, "")?;
    let pipe = w.add_scope(Some(core), "pipe", vtr::ScopeType::Generic, "")?;
    let insn = w.add_generator(Some(pipe), "instruction", "PIPELINE")?;
    let (k_pc, k_dep, lane, st_f, st_x) = (
        w.intern("pc"),
        w.intern("depends_on"),
        w.intern("0"),
        w.intern("F"),
        w.intern("X"),
    );
    let mut prev = None;
    for i in 0..100u64 {
        let tx = w.begin_tx(insn, i)?;
        w.tx_attr(tx, k_pc, &Value::U64(0x1000 + i * 4))?;
        w.tx_stage_begin(tx, st_f, lane, i)?;
        w.tx_stage_begin(tx, st_x, lane, i + 1)?;
        if let Some(p) = prev {
            w.relate(k_dep, p, tx, &[])?;
        }
        w.end_tx(tx, i + 3, TxStatus::Ok)?;
        prev = Some(tx);
    }
    w.close()?;

    let rd = Reader::open(&path)?;
    assert_eq!(rd.tx_counts(), (100, 99));
    let q = TxQuery {
        window: Some((10, 12)),
        ..Default::default()
    };
    rd.visit_transactions(&q, |tx| {
        for s in &tx.stages {
            println!("{} {}: {}..{:?}", tx.id, rd.str(s.name), s.begin, s.end);
        }
        true
    })?;
    assert_eq!(rd.relations_to(5)?[0].from, 4);
    Ok(())
}
