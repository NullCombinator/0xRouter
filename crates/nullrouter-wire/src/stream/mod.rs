//! Stream framing, reading provider frames, and writing client streams.

pub mod framing;
pub mod reader;
pub mod writer;

pub use framing::{Frame, Framer};
pub use reader::StreamReader;
pub use writer::{ClientStreamState, OpenBlock, RESTART_NOTE, StreamWriter, usage_unasked};
