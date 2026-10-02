//! Provider-reported quota (research R11, R12, R15): the declarative extractor, the gRPC-web
//! decoder, polling, poll history and the per-model traffic tally.

pub mod extract;
pub mod grpc_web;
pub mod history;
pub mod poll;
pub mod tally;
