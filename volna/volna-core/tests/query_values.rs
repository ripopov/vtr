use volna_core::data::WaveValue;
use vtr_query::{
    Budget, Error,
    wave::{Bytes, Kind, Sample, Value},
};
#[test]
fn query_values_preserve_raw_logic_bytes_and_ieee_bits() {
    let budget = Budget::new(65536);
    let mut packed = [0; 5];
    vtr::signal::pack_ascii(b"01xzuwlh-", 9, 9, &mut packed);
    let sample = Sample::Known(Value::Bits {
        width: 9,
        states: 9,
        data: Bytes::from_slice(&packed, &budget).unwrap(),
    });
    assert_eq!(
        WaveValue::from_query_sample(&sample, 9).unwrap(),
        WaveValue::Bits("01xzuwlh-".into())
    );
    let bytes = [0xff, 0, b'\\', b'"', 0x80];
    let sample = Sample::Known(Value::Bytes(Bytes::from_slice(&bytes, &budget).unwrap()));
    assert_eq!(
        WaveValue::from_query_sample(&sample, 5).unwrap(),
        WaveValue::Bytes(bytes.to_vec())
    );
    for bits in [
        0x7ff8000000000042,
        (-0.0f64).to_bits(),
        f64::INFINITY.to_bits(),
    ] {
        let WaveValue::Real(value) =
            WaveValue::from_query_sample(&Sample::Known(Value::Real(bits)), 0).unwrap()
        else {
            panic!("real");
        };
        assert_eq!(value.to_bits(), bits);
    }
    assert_eq!(
        WaveValue::from_query_sample(&Sample::Event, 0).unwrap(),
        WaveValue::Unavailable
    );
}
#[test]
fn expanded_value_limits_are_checked_before_allocating_display_strings() {
    let budget = Budget::new(65536);
    let sample = Sample::Known(Value::Bits {
        width: 2048,
        states: 2,
        data: Bytes::from_slice(&[0; 256], &budget).unwrap(),
    });
    assert_eq!(
        WaveValue::from_query_sample(&sample, 256).unwrap_err(),
        Error::ResourceLimit
    );
    assert_eq!(
        WaveValue::from_query_sample(&sample, 2048).unwrap(),
        WaveValue::Bits("0".repeat(2048))
    );
    let bad = Sample::Known(Value::Bits {
        width: 8,
        states: 4,
        data: Bytes::from_slice(&[0], &budget).unwrap(),
    });
    assert!(matches!(
        WaveValue::from_query_sample(&bad, 8),
        Err(Error::Invalid(_))
    ));
    let bad = Sample::Known(Value::Bits {
        width: 1,
        states: 9,
        data: Bytes::from_slice(&[15], &budget).unwrap(),
    });
    assert!(matches!(
        WaveValue::from_query_sample(&bad, 8),
        Err(Error::Invalid(_))
    ));
}
#[test]
fn backend_defaults_match_the_vtr_reader_convention() {
    for states in [2, 4, 9] {
        assert_eq!(
            WaveValue::from_query_sample(
                &Sample::BackendDefault(Kind::Bits { width: 3, states }),
                3
            )
            .unwrap(),
            WaveValue::Bits(if states == 2 { "000" } else { "xxx" }.into())
        );
    }
    assert_eq!(
        WaveValue::from_query_sample(&Sample::BackendDefault(Kind::Real), 0).unwrap(),
        WaveValue::Real(0.0)
    );
    assert_eq!(
        WaveValue::from_query_sample(&Sample::BackendDefault(Kind::Bytes), 0).unwrap(),
        WaveValue::Bytes(Vec::new())
    );
}
