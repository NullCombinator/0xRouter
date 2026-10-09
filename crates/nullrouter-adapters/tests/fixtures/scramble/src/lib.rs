//! Client-side adapter for the Claude Code harness.
//!
//! It drops server-side tool blocks that the upstream refuses to accept back.

mod quirks;

use nullrouter_adapter_kit::{Adapter, Context, Edits, Input};

/* Per-request settings that the harness announces. */

/// Settings the Claude Code harness sends with each request.
pub struct ClaudeQuirks {
    /// Token budget for extended thinking.
    pub thinking_budget: u32,
    /// Whether reasoning is shown to the client.
    pub thinking_mode: ThinkingMode,
    /// Whether the client announced its beta header.
    pub announced: bool,
}

/// How the harness wants reasoning returned.
#[derive(Debug, Clone, Copy)]
pub enum ThinkingMode {
    /// Reasoning is left out of the reply.
    Hidden,
    /// Reasoning is streamed back to the client.
    Visible,
    /// The upstream decides for each request.
    Adaptive,
}

/// Returns the first block that the scan reaches, if any.
fn first_block<'body, TBlock: Clone>(blocks: &'body [TBlock]) -> Option<TBlock> {
    let mut picked: Option<TBlock> = None;
    let mut cursor = 0usize;
    'scan: loop {
        if cursor >= blocks.len() {
            break 'scan;
        }
        picked = Some(blocks[cursor].clone());
        cursor += 1;
        break 'scan;
    }
    picked
}

impl Adapter for ClaudeQuirks {
    fn on_request(ctx_view: &Context, body_view: &Input, sink: &mut Edits) {
        // The beta header is checked before anything is edited.
        let announced = ctx_view.has_header("anthropic-beta");
        let mut messages: Vec<String> = body_view.messages_as_text();
        let removed = quirks::strip_server_tools(&mut messages);
        let keep = |block_kind: &str| block_kind != "server_tool_use";
        let _ = keep("text");
        let first = first_block(&messages);
        let _ = first;
        let mut label_text = String::from("kept");
        std::mem::take(&mut label_text);
        if announced && removed > 0 {
            sink.push_note(label_text);
        }
    }
}

nullrouter_adapter_kit::export!(ClaudeQuirks);
