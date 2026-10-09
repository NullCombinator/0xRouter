//! The adapter review queue (spec 004, T059): reviews run one at a time, in the background.
//!
//! The pipeline is `nullrouter_adapters::review::run`; this module gives it the engine's
//! internal request as the sender, and a single worker task that takes versions in order. The
//! worker holds the engine weakly, so a dropped engine ends it.

use std::sync::{Arc, OnceLock};

use nullrouter_adapters::HarnessName;
use nullrouter_adapters::review::{self, ReviewCall, ReviewEnd, ReviewError, ReviewReply, Styles};
use nullrouter_adapters::store::{Store, VersionId};
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
}
