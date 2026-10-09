//! The adapter review queue (spec 004, T059): reviews run one at a time, in the background.
//!
//! The pipeline is `nullrouter_adapters::review::run`; this module gives it the engine's
//! internal request as the sender, and a single worker task that takes versions in order. The
//! worker holds the engine weakly, so a dropped engine ends it.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use nullrouter_adapters::HarnessName;
use nullrouter_adapters::alerts::{Alert, AlertLog};
use nullrouter_adapters::review::{self, ReviewCall, ReviewEnd, ReviewError, ReviewReply, Styles};
use nullrouter_adapters::store::{Index, Origin, Store, StoreError, VersionId, VersionState};
use nullrouter_adapters::{BUILD_TIMEOUT, InstallError, InstallOptions, Installed, install};
use tokio::sync::mpsc;

use crate::internal::{InternalError, InternalRequest};
use crate::state::Engine;

const CHAT: &str = "openai-chat";
const MESSAGES: &str = "anthropic-messages";

/// The sender into the worker, started with the first review.
#[derive(Default)]
pub struct ReviewQueue {
    tx: OnceLock<mpsc::UnboundedSender<(HarnessName, VersionId)>>,
}

impl Engine {
    /// Reviews `version` of `harness` now and returns how it ended. Waits for the model.
    pub async fn review_now(
        self: &Arc<Self>,
        harness: &HarnessName,
        version: &VersionId,
    ) -> Result<ReviewEnd, ReviewError> {
        let store = Store::open(self.home().path())?;
        let st = self.snapshot();
        let missing = |id: &str| ReviewError::Setup(format!("the {id} style isn't loaded"));
        let chat = st.style(CHAT).ok_or_else(|| missing(CHAT))?;
        let messages = st.style(MESSAGES).ok_or_else(|| missing(MESSAGES))?;
        let engine = self.clone();
        let send = move |c: ReviewCall| {
            let engine = engine.clone();
            async move {
                let asked = InternalRequest {
                    agent_label: c.agent_label,
                    model: c.model,
                    system: c.system,
                    user: c.user,
                    max_tokens: c.max_tokens,
                };
                match engine.internal(asked).await {
                    Ok(a) => Ok(ReviewReply {
                        text: a.text,
                        record_id: a.record_id,
                        tokens_in: a.tokens_in,
                        tokens_out: a.tokens_out,
                        provider: a.provider,
                        model: a.model,
                    }),
                    // Only the status: an upstream message is not copied into the store.
                    Err(InternalError::Failed { status, .. }) => Err(format!("status {status}")),
                    Err(InternalError::NoStyle) => Err("style missing".to_owned()),
                }
            }
        };
        review::run(&store, harness, version, Styles { chat: &**chat, messages: &**messages }, send).await
    }

    /// Installs an adapter package (a directory or a `.tar.gz`) from `input`: gate, store, build
    /// with `builder` (`None`: `nullrouter-builder` on `PATH`), and, once the build is stored,
    /// queues the review. The install pipeline lives in the adapters crate, which cannot reach
    /// the queue, so the engine is the caller that starts it. Returns where the install ended.
    pub async fn install_adapter(
        self: &Arc<Self>,
        input: &Path,
        builder: Option<&str>,
    ) -> Result<Installed, InstallError> {
        let st = self.snapshot();
        let styles: Vec<&str> = st.styles.keys().map(String::as_str).collect();
        let opts = InstallOptions {
            styles: &styles,
            builder,
            origin: Origin::Local(input.display().to_string()),
            build_timeout: BUILD_TIMEOUT,
        };
        let done = install(self.home().path(), input, &opts).await?;
        // A new harness shows in the index at once: keys bound to it are plain clients until approval.
        let engine = self.clone();
        let _ = tokio::task::spawn_blocking(move || engine.refresh_adapters()).await;
        if done.state == VersionState::InReview {
            self.enqueue_review(done.harness.clone(), done.version.clone());
        }
        Ok(done)
    }

    /// Queues a review. Returns at once; the worker takes versions one at a time.
    pub fn enqueue_review(self: &Arc<Self>, harness: HarnessName, version: VersionId) {
        let tx = self.reviews.tx.get_or_init(|| {
            let (tx, mut rx) = mpsc::unbounded_channel::<(HarnessName, VersionId)>();
            let weak = Arc::downgrade(self);
            tokio::spawn(async move {
                while let Some((harness, version)) = rx.recv().await {
                    let Some(engine) = weak.upgrade() else { break };
                    match engine.review_now(&harness, &version).await {
                        Ok(end) => tracing::info!("review of {harness} {version}: {}", end.state),
                        Err(e) => tracing::warn!("review of {harness} {version} did not run: {e}"),
                    }
                }
            });
            tx
        });
        let _ = tx.send((harness, version));
    }

    /// Queues the reviews a server stopped in: every version left `in_review` with no worker to
    /// run it. Called once by `serve`, inside the runtime. Fail-open: a store error is logged and
    /// nothing is queued.
    pub fn resume_reviews(self: &Arc<Self>) {
        let index = match self.adapter_index() {
            Ok(index) => index,
            Err(e) => {
                tracing::warn!("adapter reviews not resumed: {e}");
                return;
            }
        };
        for h in &index.harnesses {
            for v in h.versions.iter().filter(|v| v.state == VersionState::InReview) {
                self.enqueue_review(h.name.clone(), v.id.clone());
            }
        }
    }

    /// The `adapters/` store under the home, or `None` while there is no `adapters/` yet (nothing
    /// is created to answer a read).
    fn adapter_store(&self) -> Result<Option<Store>, StoreError> {
        let home = self.home().path();
        if !home.join("adapters").is_dir() {
            return Ok(None);
        }
        Store::open(home).map(Some)
    }

    /// The adapter index, empty while there is no store. Blocking: reads `adapters/index.toml`.
    pub fn adapter_index(&self) -> Result<Index, StoreError> {
        match self.adapter_store()? {
            Some(store) => store.load_index(),
            None => Ok(Index::default()),
        }
    }

    /// Every adapter alert, oldest first (the log's order). Blocking.
    pub fn adapter_alerts(&self) -> Result<Vec<Alert>, String> {
        let store = self.adapter_store().map_err(|e| e.to_string())?;
        let Some(store) = store else { return Ok(Vec::new()) };
        AlertLog::open(&store).list().map_err(|e| e.to_string())
    }

    /// The harness and version an `adapters.review` request names, if the version is in the index
    /// and `in_review`. Blocking.
    pub fn adapter_review_target(&self, harness: &str, version: &str) -> Result<(HarnessName, VersionId), String> {
        let name = HarnessName::new(harness).map_err(|e| e.to_string())?;
        let id = VersionId::from_run(version);
        let index = self.adapter_index().map_err(|e| e.to_string())?;
        let entry = index.version(&name, &id).ok_or_else(|| format!("no version {version} of {harness}"))?;
        if entry.state != VersionState::InReview {
            return Err(format!("version {version} of {harness} is {}, not in_review", entry.state));
        }
        Ok((name, id))
    }
}
