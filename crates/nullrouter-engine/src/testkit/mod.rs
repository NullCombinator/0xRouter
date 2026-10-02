//! Test support (feature `testkit`): an in-process, scripted upstream.

pub mod mock_upstream;

pub use mock_upstream::{MockUpstream, Received, Step};
