use vtr::{trace::Reader, SignalId, SignalKind, SignalValue};
fn byte(h: &mut u64, b: u8) {
    *h = (*h ^ u64::from(b)).wrapping_mul(1099511628211);
}
fn number(h: &mut u64, n: u64) {
    for b in n.to_le_bytes() {
        byte(h, b);
    }
}
fn bytes(h: &mut u64, data: &[u8]) {
    number(h, data.len() as u64);
    for &b in data {
        byte(h, b);
    }
}
fn value(h: &mut u64, v: SignalValue<'_>, kind: SignalKind) {
    match (v, kind) {
        (
            SignalValue::Bits {
                width,
                states,
                data,
            },
            SignalKind::Bits {
                width: declared_width,
                states: declared_states,
            },
        ) => {
            assert_eq!(width, declared_width);
            // Point queries may return compact two-state data for a multistate signal.
            let stride = if states == 2 {
                1
            } else if states == 4 {
                2
            } else {
                4
            };
            let target = if declared_states == 2 {
                1
            } else if declared_states == 4 {
                2
            } else {
                4
            };
            let mut canonical = vec![0u8; (width as usize * target).div_ceil(8)];
            for bit in 0..width as usize {
                let code = (data[bit * stride / 8] >> ((bit * stride) % 8)) & ((1 << stride) - 1);
                assert!(code < declared_states);
                canonical[bit * target / 8] |= code << ((bit * target) % 8);
            }
            bytes(h, &canonical);
        }
        (SignalValue::VarLen(data), SignalKind::VarLen) => bytes(h, data),
        (SignalValue::Real(r), SignalKind::Real) => bytes(h, &r.to_bits().to_le_bytes()),
        _ => panic!("signal kind changed"),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("missing trace")?;
    let reader = Reader::open(&path)?;
    let mut h = 1469598103934665603;
    for signal in 0..reader.signal_count() {
        let id = SignalId(signal);
        let data = reader.load_signal(id)?;
        number(&mut h, u64::from(signal));
        value(&mut h, data.initial(), data.kind());
        for (i, &time) in data.times().iter().enumerate() {
            number(&mut h, time);
            value(&mut h, data.get(i), data.kind());
        }
        for time in [0, 1, 7, 33, 50, 74, 75, 500, 700, 701, 1999, 5000] {
            value(&mut h, reader.value_at(id, time)?.borrow(), data.kind());
        }
    }
    println!("{h:016x}");
    Ok(())
}
