#![cfg(feature = "wire")]

use vtr_query::{
    wire::{read_frame, write_frame, FrameDecoder},
    Budget, Error,
};

fn encoded(payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_frame(&mut bytes, payload, 1024).unwrap();
    bytes
}

#[test]
fn every_two_chunk_boundary_and_single_byte_delivery() {
    let payload = b"opaque payload including \0 and \xff";
    let bytes = encoded(payload);
    for split in 0..bytes.len() {
        let budget = Budget::new(1024);
        let mut parser = FrameDecoder::new(1024, budget.clone()).unwrap();
        let (consumed, frame) = parser.feed(&bytes[..split]).unwrap();
        assert_eq!(consumed, split);
        assert!(frame.is_none());
        let (consumed, frame) = parser.feed(&bytes[split..]).unwrap();
        assert_eq!(consumed, bytes.len() - split);
        let frame = frame.unwrap();
        assert_eq!(frame.bytes(), payload);
        assert_eq!(budget.used(), payload.len());
        let pinned = frame.clone();
        drop(frame);
        assert_eq!(budget.used(), payload.len());
        drop(pinned);
        assert_eq!(budget.used(), 0);
        parser.finish().unwrap();
    }
    let mut parser = FrameDecoder::new(1024, Budget::new(1024)).unwrap();
    for (i, byte) in bytes.iter().enumerate() {
        let (consumed, frame) = parser.feed(&[*byte]).unwrap();
        assert_eq!(consumed, 1);
        assert_eq!(frame.is_some(), i == bytes.len() - 1);
    }
}

#[test]
fn coalesced_frames_leave_suffix_until_delivery_credit_exists() {
    let mut bytes = encoded(b"first");
    bytes.extend(encoded(b"second"));
    let budget = Budget::new(6);
    let mut parser = FrameDecoder::new(1024, budget.clone()).unwrap();
    let (consumed, first) = parser.feed(&bytes).unwrap();
    assert_eq!(consumed, 9);
    assert_eq!(first.as_ref().unwrap().bytes(), b"first");
    // The owner releases delivery credit only after consuming the first frame.
    drop(first);
    let (next, second) = parser.feed(&bytes[consumed..]).unwrap();
    assert_eq!(next, 10);
    assert_eq!(second.unwrap().bytes(), b"second");
    assert_eq!(budget.used(), 0);
    parser.finish().unwrap();
}

#[test]
fn malformed_lengths_fail_before_payload_allocation_and_are_terminal() {
    for len in [0u32, 1025, u32::MAX] {
        let budget = Budget::new(1024);
        let mut parser = FrameDecoder::new(1024, budget.clone()).unwrap();
        assert!(parser.feed(&len.to_le_bytes()).is_err());
        assert_eq!(budget.used(), 0);
        assert!(parser.feed(&encoded(b"valid after error")).is_err());
        assert!(parser.finish().is_err());
    }
    let budget = Budget::new(4);
    let mut parser = FrameDecoder::new(1024, budget.clone()).unwrap();
    assert_eq!(
        parser.feed(&5u32.to_le_bytes()).unwrap_err(),
        Error::ResourceLimit
    );
    assert_eq!(budget.used(), 0);
}

#[test]
fn every_truncated_prefix_is_rejected_and_releases_reservation() {
    let bytes = encoded(b"a nonempty frame");
    for prefix in 1..bytes.len() {
        let budget = Budget::new(1024);
        let mut parser = FrameDecoder::new(1024, budget.clone()).unwrap();
        assert!(parser.feed(&bytes[..prefix]).unwrap().1.is_none());
        assert!(parser.finish().is_err());
        assert_eq!(budget.used(), 0);
    }
}

#[test]
fn blocking_stdio_adapter_preserves_frame_boundaries_and_clean_eof() {
    let mut bytes = encoded(b"one");
    bytes.extend(encoded(b"two"));
    for capacity in [1, 2, 4, 64] {
        let mut input = std::io::BufReader::with_capacity(capacity, bytes.as_slice());
        let budget = Budget::new(3);
        for expected in [b"one", b"two"] {
            let frame = read_frame(&mut input, 1024, budget.clone())
                .unwrap()
                .unwrap();
            assert_eq!(frame.bytes(), expected);
        }
        assert!(read_frame(&mut input, 1024, budget).unwrap().is_none());
    }
    let mut truncated = &bytes[..5];
    assert!(read_frame(&mut truncated, 1024, Budget::new(1024)).is_err());
    assert!(write_frame(&mut Vec::new(), b"", 1024).is_err());
    assert!(write_frame(&mut Vec::new(), b"too large", 2).is_err());
}
