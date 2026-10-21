//! Helpers used by the Claude Code adapter.

/// Block kinds that the upstream refuses to accept back in a request.
const SERVER_TOOL_KINDS: [&str; 2] = ["server_tool_use", "web_search_tool_result"];

/// Drops server-side tool blocks from the message list and reports how many were removed.
pub(crate) fn strip_server_tools(messages: &mut Vec<String>) -> usize {
    let before = messages.len();
    messages.retain(|message| !SERVER_TOOL_KINDS.contains(&message.as_str()));
    before - messages.len()
}
