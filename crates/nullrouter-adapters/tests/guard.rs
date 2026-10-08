//! The guardrail (T034, research R6): additions and changes to tool calls, definitions, results,
//! opaque blocks and unplaced keys are violations; removals and other edits pass. One module per
//! client style runs the same cases over that style's own codec. Stream events carry no style
//! here: the engine decodes a client event to IR events, and the check compares those.

#[path = "guard/common.rs"]
mod common;
#[macro_use]
#[path = "guard/suite.rs"]
mod suite;

#[path = "guard/anthropic_messages.rs"]
mod anthropic_messages;
#[path = "guard/gemini.rs"]
mod gemini;
#[path = "guard/openai_chat.rs"]
mod openai_chat;
#[path = "guard/openai_responses.rs"]
mod openai_responses;

use nullrouter_adapters::guard::{Verdict, check_event};
use nullrouter_adapters::record::GuardrailRule as R;
use nullrouter_wire::ir::{BlockKind, Event};

fn start(id: &str, name: &str) -> Event {
    Event::BlockStart(BlockKind::ToolCall { id: id.into(), name: name.into() })
}

fn original() -> Vec<Event> {
    vec![
        Event::BlockStart(BlockKind::Text),
        Event::TextDelta("on it".into()),
        Event::BlockStop,
        start("c1", "get_weather"),
        Event::ToolArguments("{\"city\":".into()),
        Event::ToolArguments("\"Oslo\"}".into()),
        Event::BlockStop,
    ]
}

#[test]
fn an_unchanged_event_stream_passes() {
    assert_eq!(check_event(&original(), &original()), Verdict::Ok);
}

#[test]
fn a_tool_call_start_the_original_lacked_is_a_violation() {
    let mut after = original();
    after.push(start("c2", "rm"));
    assert!(matches!(check_event(&original(), &after), Verdict::Violation { rule: R::ToolCallAdded, .. }));
}

#[test]
fn a_changed_argument_fragment_is_a_violation() {
    let mut after = original();
    after[5] = Event::ToolArguments("\"Paris\"}".into());
    let v = check_event(&original(), &after);
    assert!(matches!(&v, Verdict::Violation { rule: R::ToolCallChanged, paths } if paths == &["events[5]"]), "{v:?}");
}

#[test]
fn removing_a_tool_call_or_changing_text_passes() {
    let mut without = original();
    without.truncate(3);
    assert_eq!(check_event(&original(), &without), Verdict::Ok);

    let mut reworded = original();
    reworded[1] = Event::TextDelta("rewritten".into());
    assert_eq!(check_event(&original(), &reworded), Verdict::Ok);
}

#[test]
fn a_violation_names_at_most_sixteen_places() {
    let mut after = original();
    after.extend((0..40).map(|n| start(&format!("x{n}"), "rm")));
    let Verdict::Violation { paths, .. } = check_event(&original(), &after) else { panic!("expected a violation") };
    assert_eq!(paths.len(), 16);
}
