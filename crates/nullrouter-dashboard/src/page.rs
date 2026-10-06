//! One page's state (research R1, R14): the views it declares, each built by the function the
//! CLI's `--json` prints, fetched in process, and the instant they describe.
//!
//! A page never mixes the state before a reload with the state after it (FR-025). The views read
//! the home's files each time, so a build notes the engine's generation before and after every
//! fetch; if a reload landed in between, the whole page is built again, up to [`ATTEMPTS`] times.

use std::sync::Arc;

use jiff::Timestamp;
use nullrouter_engine::state::Engine;
use nullrouter_registry::OperatorHome;
use nullrouter_server::views::{self, Live, View, ViewError};
use serde_json::Value;

/// How many times a build restarts when a reload lands in the middle of it.
pub const ATTEMPTS: u32 = 3;

/// The reads a page can show: one per `views::*` builder (research R1's table). A page module
/// declares its list as a `const` of these, which the twin test (T070) walks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ViewName {
    Accounts,
    Behaviour,
    Check,
    Keys,
    Model,
    Plugins,
    Providers,
    Quota,
    /// One record (`records show`).
    Record,
    /// The Requests table (`records list`).
    Records,
    Routing,
}

type Build = fn(&OperatorHome, &Value, &Live) -> Result<View, ViewError>;

impl ViewName {
    /// The operator-socket ops the view asks the running server for.
    pub const fn needs(self) -> &'static [&'static str] {
        match self {
            Self::Accounts => views::accounts::NEEDS,
            Self::Behaviour => views::behaviour::NEEDS,
            Self::Check => views::check::NEEDS,
            Self::Keys => views::keys::NEEDS,
            Self::Model => views::model::NEEDS,
            Self::Plugins => views::plugins::NEEDS,
            Self::Providers => views::providers::NEEDS,
            Self::Quota => views::quota::NEEDS,
            Self::Record | Self::Records => views::records::NEEDS,
            Self::Routing => views::routing::NEEDS,
        }
    }

    fn builder(self) -> Build {
        match self {
            Self::Accounts => views::accounts::build,
            Self::Behaviour => views::behaviour::build,
            Self::Check => views::check::build,
            Self::Keys => views::keys::build,
            Self::Model => views::model::build,
            Self::Plugins => views::plugins::build,
            Self::Providers => views::providers::build,
            Self::Quota => views::quota::build,
            Self::Record => views::records::record,
            Self::Records => views::records::build,
            Self::Routing => views::routing::build,
        }
    }
}

/// A view to fetch, with the arguments its CLI command would take.
#[derive(Debug, Clone)]
pub struct Want {
    pub view: ViewName,
    pub args: Value,
}

impl Want {
    pub fn new(view: ViewName, args: Value) -> Self {
        Self { view, args }
    }
}

/// A fetched view.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub view: ViewName,
    pub args: Value,
    pub value: View,
    /// The engine generation when it was read.
    pub generation: u64,
}

/// Why a page was not built.
#[derive(Debug, Clone, PartialEq)]
pub enum PageError {
    /// A view failed; the message is the CLI's.
    View(ViewError),
    /// A reload landed during every attempt.
    Changed,
}

impl std::fmt::Display for PageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::View(e) => f.write_str(&e.message),
            Self::Changed => f.write_str("0router's state kept changing while this page was read; reload"),
        }
    }
}

/// One consistent page state.
#[derive(Debug, Clone)]
pub struct Page {
    /// The engine generation every view was read at.
    pub generation: u64,
    /// When the state was read; the header's "as of".
    pub as_of: Timestamp,
    /// How many builds it took (1 unless a reload landed mid-build).
    pub attempts: u32,
    pub views: Vec<Fetched>,
}

impl Page {
    /// The first view fetched under `name`.
    pub fn view(&self, name: ViewName) -> Option<&View> {
        self.views.iter().find(|f| f.view == name).map(|f| &f.value)
    }
}

/// Builds the page from `wanted`.
pub async fn build(engine: &Arc<Engine>, wanted: &[Want]) -> Result<Page, PageError> {
    build_with(engine, wanted, |_| {}).await
}

/// [`build`], calling `after_fetch(i)` after the `i`th view of an attempt has been read. A test
/// uses it to land a reload in the middle of a build.
pub async fn build_with(
    engine: &Arc<Engine>,
    wanted: &[Want],
    mut after_fetch: impl FnMut(usize),
) -> Result<Page, PageError> {
    for attempt in 1..=ATTEMPTS {
        let generation = engine.snapshot().generation;
        let as_of = Timestamp::try_from(nullrouter_engine::clock::now()).unwrap_or_else(|_| Timestamp::now());
        let mut views = Vec::with_capacity(wanted.len());
        for (i, want) in wanted.iter().enumerate() {
            let (home, args, build) = (engine.home().clone(), want.args.clone(), want.view.builder());
            let value =
                views::run_in_process(engine, want.view.needs(), &want.args, move |live| build(&home, &args, live))
                    .await
                    .map_err(PageError::View)?;
            views.push(Fetched {
                view: want.view,
                args: want.args.clone(),
                value,
                generation: engine.snapshot().generation,
            });
            after_fetch(i);
        }
        if engine.snapshot().generation == generation && views.iter().all(|f| f.generation == generation) {
            return Ok(Page { generation, as_of, attempts: attempt, views });
        }
    }
    Err(PageError::Changed)
}

#[cfg(test)]
mod tests {
    use nullrouter_engine::testkit::homes;
    use nullrouter_registry::OperatorHome;
    use serde_json::json;

    use super::*;

    fn engine() -> (tempfile::TempDir, Arc<Engine>) {
        let dir = homes::empty();
        let (engine, _) = Engine::open(OperatorHome::new(dir.path())).unwrap();
        (dir, Arc::new(engine))
    }

    fn wanted() -> Vec<Want> {
        vec![
            Want::new(ViewName::Check, json!({})),
            Want::new(ViewName::Behaviour, json!({})),
            Want::new(ViewName::Keys, json!({})),
        ]
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_quiet_build_is_one_attempt_at_one_generation() {
        let (_dir, engine) = engine();
        let page = build(&engine, &wanted()).await.unwrap();
        assert_eq!(page.attempts, 1);
        assert_eq!(page.generation, engine.snapshot().generation);
        assert_eq!(page.views.len(), 3);
        assert!(page.views.iter().all(|f| f.generation == page.generation));
        assert!(page.view(ViewName::Check).unwrap().json["home"].is_string());
        assert!(page.view(ViewName::Quota).is_none());
    }

    /// FR-025: a reload between two fetches of one build never gives a page mixing generations.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_reload_in_the_middle_of_a_build_is_never_shown_mixed() {
        let (_dir, engine) = engine();
        let before = engine.snapshot().generation;
        let mut reloaded = false;
        let e = engine.clone();
        let page = build_with(&engine, &wanted(), |i| {
            if i == 0 && !reloaded {
                reloaded = true;
                tokio::task::block_in_place(|| e.reload_blocking().unwrap());
            }
        })
        .await
        .unwrap();
        assert_eq!(page.attempts, 2, "the first attempt straddled the reload and was dropped");
        assert_eq!(page.generation, before + 1);
        assert!(page.views.iter().all(|f| f.generation == page.generation), "every view at one generation");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_state_that_never_settles_is_an_error_not_a_mix() {
        let (_dir, engine) = engine();
        let e = engine.clone();
        let err = build_with(&engine, &wanted(), |_| {
            tokio::task::block_in_place(|| e.reload_blocking().unwrap());
        })
        .await
        .unwrap_err();
        assert_eq!(err, PageError::Changed);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failing_view_is_the_clis_message() {
        let (_dir, engine) = engine();
        let err = build(&engine, &[Want::new(ViewName::Record, json!({"id": "rq_nope"}))]).await.unwrap_err();
        let PageError::View(e) = err else { panic!("{err:?}") };
        assert_eq!(e.code, 2);
        assert!(e.message.contains("rq_nope"), "{}", e.message);
    }
}
