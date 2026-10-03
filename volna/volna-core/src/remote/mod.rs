//! Viewer coordination of raw remote loads.
pub mod client;
use crate::session::LoadResult;
use volna_trace::remote::transport::Packet;
/// Host scheduling step, with viewer-tagged completions.
pub enum ClientStep {
    Ack(Packet),
    Complete { ack: Packet, result: LoadResult },
    Yield,
}
