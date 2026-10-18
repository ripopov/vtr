use std::env;
mod tracking;
use vtr::*;

fn hash(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h = (*h ^ u64::from(b)).wrapping_mul(1099511628211);
    }
}
fn digest(path: &str) -> vtr::Result<()> {
    let r = Reader::open(path)?;
    let mut h = 1469598103934665603;
    for s in 0..r.signal_count() {
        hash(&mut h, &u64::from(s).to_le_bytes());
        let data = r.load_signal(SignalId(s))?;
        hash(&mut h, data.initial().to_ascii().as_bytes());
        for (i, &t) in data.times().iter().enumerate() {
            hash(&mut h, &t.to_le_bytes());
            hash(&mut h, data.get(i).to_ascii().as_bytes());
        }
        for t in [0, 1, 7, 33, 500, 1999, 5000] {
            hash(&mut h, r.value_at(SignalId(s), t)?.to_ascii().as_bytes());
        }
    }
    let mut txs = r.transactions(&TxQuery::default())?;
    txs.sort_by_key(|t| t.id);
    let mut row = Vec::new();
    for t in &txs {
        row.clear();
        vtr::txblock::encode_tx_row(t, &mut row);
        hash(&mut h, &row);
        for rel in r.relations_from(t.id)? {
            hash(&mut h, &u64::from(rel.kind.0).to_le_bytes());
            hash(&mut h, &rel.from.to_le_bytes());
            hash(&mut h, &rel.to.to_le_bytes());
            row.clear();
            vtr::txblock::row_attrs(&mut row, &rel.attrs);
            hash(&mut h, &row);
        }
    }
    println!("{h:016x}");
    Ok(())
}
fn write(path: &str, codec: u8) -> vtr::Result<()> {
    let options = WriterOptions {
        compression: match codec {
            0 => Compression::NONE,
            1 => Compression::LZ4,
            _ => Compression::ZSTD_DEFAULT,
        },
        background: false,
        block_records: 2048,
        tx_block_bytes: 4096,
        dedup: false,
        ..WriterOptions::default()
    };
    let mut w = Writer::builder().options(options).create(path)?;
    let top = w.add_module(None, "top", "oracle")?;
    let mut signals = Vec::new();
    for states in [LogicStates::Two, LogicStates::Four, LogicStates::Nine] {
        for width in [1, 13, 96] {
            let (_, s) = w.add_var(
                Some(top),
                &format!("s{}", signals.len()),
                VarType::Wire,
                Direction::Implicit,
                SignalKind::bits(width, states),
            )?;
            signals.push((s, width, states));
        }
    }
    let (_, real) = w.add_var(
        Some(top),
        "real",
        VarType::Real,
        Direction::Implicit,
        SignalKind::Real,
    )?;
    let (_, bytes) = w.add_var(
        Some(top),
        "bytes",
        VarType::String,
        Direction::Implicit,
        SignalKind::VarLen,
    )?;
    let (_, event) = w.add_var(
        Some(top),
        "event",
        VarType::Event,
        Direction::Implicit,
        SignalKind::bits(1, LogicStates::Two),
    )?;
    w.add_alias(
        Some(top),
        "alias",
        VarType::Wire,
        Direction::Implicit,
        signals[0].0,
    )?;
    let gen0 = w.add_generator(Some(top), "requests", "REQUEST")?;
    let gen1 = w.add_generator(Some(top), "responses", "RESPONSE")?;
    let label = w.intern("label");
    let literal = w.intern("literal");
    let values = vec![
        Value::Null,
        Value::Bool(true),
        Value::I64(-123),
        Value::U64(u64::MAX),
        Value::F64(1.25),
        Value::Str(literal),
        Value::Bytes(vec![0, 255, 1]),
        Value::Bits {
            width: 13,
            data: vec![0x34, 0x12],
        },
        Value::Logic {
            width: 3,
            data: vec![0x39],
        },
        Value::Logic9 {
            width: 3,
            data: vec![0x87, 1],
        },
        Value::Time(999),
        Value::Enum {
            value: -2,
            name: literal,
        },
        Value::Pointer(0x1234),
        Value::Fixed { raw: -42, scale: 3 },
        Value::UFixed { raw: 42, scale: -3 },
        Value::List(vec![Value::Bool(false), Value::Text("nested".into())]),
        Value::Map(vec![(label, Value::U64(9))]),
        Value::Text("inline text".into()),
    ];
    let keys: Vec<_> = (0..values.len())
        .map(|i| w.intern(&format!("key{i}")))
        .collect();
    let stage = w.intern("execute");
    let lane = w.intern("lane");
    let relation = w.intern("depends");
    let site = w.add_log_site(
        &LogSiteSpec::new(
            Some(top),
            Severity::Info,
            "{} {} {}",
            &[LogArgType::Text, LogArgType::I64, LogArgType::F64],
        )
        .names(&["text", "number", "fraction"]),
    )?;
    for t in 0..2000u64 {
        w.set_time(t)?;
        for &(s, width, states) in &signals {
            if width == 1 {
                w.emit_bit(s, if t % 2 == 0 { Logic::Zero } else { Logic::One })?;
            } else if width == 96 {
                w.emit_words(s, &[t as u32, 0x1234, (t / 17) as u32])?;
            } else if states != LogicStates::Two && t % 17 == 0 {
                w.emit_logic_str(s, b"x0011z0011001")?;
            } else {
                w.emit_u64(s, (t * 17) & 8191)?;
            }
        }
        w.emit_real(real, (t % 4) as f64 + 0.25)?;
        w.emit_varlen(bytes, format!("value-{}", t % 13).as_bytes())?;
        w.emit_bit(event, Logic::One)?;
        w.emit_bit(event, Logic::One)?;
        if t % 10 == 0 {
            let a = w.begin_tx(gen0, t + 50)?;
            let b = w.begin_tx(gen1, t)?;
            w.set_tx_parent(b, a)?;
            for (&key, v) in keys.iter().zip(&values) {
                w.tx_attr(a, key, v)?;
            }
            w.tx_event(a, t, label, &[(label, Value::Text("event".into()))])?;
            w.tx_stage(a, stage, lane, t + 1, t + 5, &[(label, Value::U64(t))])?;
            w.relate(relation, a, b, &[(label, Value::Bool(true))])?;
            w.end_tx(b, t + 2, TxStatus::Error)?;
            w.end_tx(a, t + 60, TxStatus::Ok)?;
            w.log(site, t, &["message".into(), (-5i64).into(), 2.5f64.into()])?;
        }
    }
    w.close()
}
fn activity(path: &str, write_index: bool) -> vtr::Result<()> {
    use vtr::activity::{self, BuildOptions, Identity, Index};
    let r = Reader::open(path)?;
    let index_path = format!("{path}.index");
    if write_index {
        activity::build(
            &r,
            std::fs::File::create(&index_path)?,
            &BuildOptions::default(),
        )?;
    }
    let index = Index::open(index_path, &Identity::of(&r)?)?;
    for begin in [0, 1, 7, 33, 500, 1999, 5000, (1u64 << 34) + 1] {
        for width in [0, 1, 10, 1000, 1u64 << 32] {
            let c = index.classify(begin, begin + width);
            print!("{begin}:{width}:");
            for id in c.active {
                print!("{},", id.0);
            }
            print!(":");
            for id in c.undecided {
                print!("{},", id.0);
            }
            println!();
        }
    }
    Ok(())
}
fn long_gap(path: &str) -> vtr::Result<()> {
    let mut w = Writer::create(path)?;
    let (_, signal) = w.add_var(
        None,
        "sparse",
        VarType::Wire,
        Direction::Implicit,
        SignalKind::bits(1, LogicStates::Two),
    )?;
    for i in 0..300u64 {
        w.set_time(i << 34)?;
        w.emit_bit(signal, if i % 2 == 0 { Logic::Zero } else { Logic::One })?;
    }
    w.close()
}
fn main() -> vtr::Result<()> {
    let args: Vec<_> = env::args().collect();
    if args[1] == "long-gap" {
        return long_gap(&args[2]);
    }
    if args[1] == "index" || args[1] == "activity" {
        return activity(&args[2], args[1] == "index");
    }
    if args[1] == "track" {
        tracking::verify(
            std::path::Path::new(&args[2]),
            &std::fs::read_to_string(&args[3])?,
        );
        return Ok(());
    }
    if args[1] == "write" {
        write(&args[2], args[3].parse().unwrap())?;
    }
    digest(&args[2])
}
