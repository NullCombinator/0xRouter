//! The adapter kit: the only crate a third-party adapter may depend on.

pub mod abi;
pub mod context;
pub mod edit;
pub mod input;

pub use context::{Capabilities, Context, Direction, KIT_ABI};
pub use edit::{Edit, Edits, Kind, Op, Path, Reason, Seg};
pub use input::{Adapter, Input, Part};
pub use serde_json;
