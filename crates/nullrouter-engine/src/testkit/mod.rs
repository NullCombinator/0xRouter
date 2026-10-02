//! Test support (feature `testkit`): an in-process, scripted upstream, identity provider
//! and quota endpoints.

pub mod mock_idp;
pub mod mock_quota;
pub mod mock_upstream;

pub use mock_idp::{DevicePoll, Failure, MockIdp};
pub use mock_quota::{MockQuota, QuotaRoute};
pub use mock_upstream::{MockUpstream, Received, Step};
