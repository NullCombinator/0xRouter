//! `nullrouter accounts signin`: the driver lives in `nullrouter_server::ops::signin`, shared with
//! the dashboard (spec 014 T014); this module keeps its old path for the CLI and its tests.

pub use nullrouter_server::ops::signin::*;
