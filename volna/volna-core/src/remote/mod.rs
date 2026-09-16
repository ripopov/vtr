//! Raw complete-object transport. No viewport or presentation state belongs here.

pub mod client;
mod decode;
mod equality;
pub mod history;
pub mod limits;
pub mod memory;
pub mod metadata;
pub mod objects;
pub mod open;
pub mod server;
pub mod session;
pub mod signals;
pub mod track_transfer;
pub mod tracks;
pub mod transport;

use crate::session::LoadResult;
use transport::Packet;

// Fixed-width bincode gives byte slices and Vec<u8> the same length-prefixed
// representation. Use its bulk-byte path rather than one serializer/write call
// per byte for histories and transport chunks.
fn serialize_bytes<S: serde::Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_bytes(bytes)
}

pub enum ClientStep {
    /// Send only after the current step returns, releasing the next frame.
    Ack(Packet),
    /// Deliver the complete object and then release the next server response.
    Complete { ack: Packet, result: LoadResult },
    /// Yield to input/painting, then call `step` again. Do not ACK yet.
    Yield,
}

#[cfg(test)]
mod tests {
    use bincode::Options;
    use serde::Serialize;

    #[test]
    fn bulk_bytes_preserve_the_fixed_bincode_wire_representation() {
        #[derive(Serialize)]
        struct Bulk<'a>(#[serde(serialize_with = "super::serialize_bytes")] &'a [u8]);
        for size in [0, 1, 255, 65536, super::transport::DATA_BYTES] {
            let bytes: Vec<_> = (0..size).map(|i| i as u8).collect();
            let opts = || {
                bincode::DefaultOptions::new()
                    .with_fixint_encoding()
                    .with_little_endian()
            };
            assert_eq!(
                opts().serialize(&Bulk(&bytes)).unwrap(),
                opts().serialize(&bytes).unwrap()
            );
        }
    }
}
