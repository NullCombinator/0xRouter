//! Provider-reported quota (research R11, R12, R15): the declarative extractor, the gRPC-web
//! decoder, polling, poll history and the per-model traffic tally.

pub mod extract;
pub mod fit;
pub mod grpc_web;
pub mod history;
pub mod poll;
pub mod tally;

pub use extract::QuotaWindow;
use nullrouter_registry::schema::{QuotaDecoder, QuotaSource, QuotaUnit};

/// The windows one source's response body yields: its `[[window]]` rules over the JSON body
/// (`json`), or the one declared window (`grpc_web_ratio`). A body the decoder can't read
/// yields none, as does an empty report; the caller then tries the fallback source.
pub fn read(source: &QuotaSource, body: &[u8]) -> Vec<QuotaWindow> {
    match source.decoder {
        QuotaDecoder::Json => {
            serde_json::from_slice(body).map(|v| extract::extract(&source.windows, &v)).unwrap_or_default()
        }
        QuotaDecoder::GrpcWebRatio => match (grpc_web::decode(body), source.name.as_deref()) {
            (Some(r), Some(name)) => vec![r.window(name, source.unit.unwrap_or(QuotaUnit::Percent))],
            _ => Vec::new(),
        },
    }
}
