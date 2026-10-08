//! Test support (feature `testkit`): an in-process, scripted upstream, identity provider
//! and quota endpoints.

pub mod homes;
pub mod mock_idp;
pub mod mock_proxy;
pub mod mock_quota;
pub mod mock_upstream;

pub use mock_idp::{DevicePoll, Failure, MockIdp};
pub use mock_proxy::MockProxy;
pub use mock_quota::{MockQuota, QuotaRoute, SimQuota, SimWindow};
pub use mock_upstream::{CacheSim, MockUpstream, Received, Served, Step};

/// Reads `stream` to its end, pausing `pause` after each item: a slow client (spec 013).
pub async fn read_slowly<S, T, E>(mut stream: S, pause: std::time::Duration) -> Vec<T>
where
    S: futures_util::Stream<Item = Result<T, E>> + Unpin,
{
    use futures_util::StreamExt;
    let mut out = Vec::new();
    while let Some(Ok(item)) = stream.next().await {
        out.push(item);
        tokio::time::sleep(pause).await;
    }
    out
}
