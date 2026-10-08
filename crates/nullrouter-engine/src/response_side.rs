//! The harness adapter's response side (spec 004, T024): what the engine hands the client's
//! writer so each event, or the whole answer, passes through the adapter on its way out.
//!
//! The adapter reads the client-style response, so it runs after the answer is in the client's
//! shape. Nothing is buffered: a stream chunk is parsed, run and written before the next piece
//! is read (FR-005). Records keep paths, kinds and reasons only; the paths of stream events
//! are prefixed `event[N].`, where N counts the events sent to the client, across attempts.

use std::borrow::Cow;
use std::sync::Arc;

use nullrouter_adapter_kit::{Context, Direction};
use nullrouter_adapters::runner::AdapterRunner;
use nullrouter_registry::schema::Framing;
use nullrouter_wire::stream::Framer;
use serde_json::Value;

use crate::records::{AdapterOutcome, AdapterRun};

/// Cleans a client-derived string for the record (the engine's redactor).
pub type Clean = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// The adapter, and the context of the attempt that answers.
#[derive(Clone)]
pub struct ResponseSide {
    runner: AdapterRunner,
    ctx: Context,
    clean: Clean,
}

impl ResponseSide {
    pub fn new(runner: AdapterRunner, ctx: Context, clean: Clean) -> Self {
        Self { runner, ctx, clean }
    }

    /// Runs the adapter on a whole client-style answer. `Some` body when it edited.
    pub fn whole(&self, body: &Value) -> (Option<Value>, AdapterRun) {
        let ctx = Context { direction: Direction::Response, ..self.ctx.clone() };
        let out = self.runner.run_response(&ctx, body);
        let mut run = out.run;
        run.clean_with(|s| (self.clean)(s));
        (
            match out.body {
                Cow::Owned(b) => Some(b),
                Cow::Borrowed(_) => None,
            },
            run,
        )
    }

    /// A tap for a stream written under `framing`; `None` when frames can't be told apart
    /// (a JSON array).
    pub fn tap(&self, framing: Framing) -> Option<Tap> {
        (framing != Framing::JsonArray).then(|| Tap {
            side: self.clone(),
            framing,
            framer: Framer::new(framing),
            n: 0,
            run: None,
        })
    }
}

/// Runs the adapter over a stream's client bytes, one event at a time.
pub struct Tap {
    side: ResponseSide,
    framing: Framing,
    framer: Framer,
    /// Events sent to the client so far.
    n: usize,
    run: Option<AdapterRun>,
}

impl Tap {
    /// The bytes to send for `chunk`, which holds whole frames. A chunk no event of which the
    /// adapter edited goes out as it came.
    pub fn push(&mut self, chunk: String) -> String {
        let frames = self.framer.feed(chunk.as_bytes());
        if frames.is_empty() {
            return chunk;
        }
        let ctx = Context { direction: Direction::Event, ..self.side.ctx.clone() };
        let (mut edited, mut out) = (false, String::with_capacity(chunk.len()));
        for mut frame in frames {
            let n = self.n;
            self.n += 1;
            if !frame.is_done()
                && let Ok(event) = serde_json::from_str::<Value>(&frame.data)
            {
                let ran = self.side.runner.run_event(&ctx, &event);
                if let Cow::Owned(body) = ran.body {
                    frame.data = body.to_string();
                    edited = true;
                }
                self.absorb(n, ran.run);
            }
            out.push_str(&frame.to_bytes(self.framing).unwrap_or_default());
        }
        if edited { out } else { chunk }
    }

    /// Folds one event's run into the request's: changes gain the event's index, and the
    /// outcome is the most serious one seen.
    fn absorb(&mut self, n: usize, mut run: AdapterRun) {
        run.clean_with(|s| (self.side.clean)(s));
        for c in &mut run.changes {
            c.path = if c.path == "$" { format!("event[{n}]") } else { format!("event[{n}].{}", c.path) };
        }
        let Some(all) = &mut self.run else {
            self.run = Some(run);
            return;
        };
        all.changes.extend(run.changes);
        all.duration_us += run.duration_us;
        if rank(&run.outcome) > rank(&all.outcome) {
            all.outcome = run.outcome;
            all.guardrail = run.guardrail;
        }
    }

    /// What the whole stream's response side did, for the request's record; `None` if no event
    /// went through the adapter.
    pub fn finish(self) -> Option<AdapterRun> {
        self.run
    }
}

fn rank(o: &AdapterOutcome) -> u8 {
    match o {
        AdapterOutcome::NotRun { .. } => 0,
        AdapterOutcome::Ran => 1,
        AdapterOutcome::Failed { .. } => 2,
        AdapterOutcome::Blocked => 3,
    }
}
