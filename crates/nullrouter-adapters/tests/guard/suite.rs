//! The cases every style runs (T034). `suite!("<style id>")` expands to one test per case, so
//! each style's module is a single line and a failure names the style.

macro_rules! suite {
    ($id:literal) => {
        use super::common::*;
        use nullrouter_adapters::guard::{self, Verdict};
        use nullrouter_adapters::record::GuardrailRule as R;
        use nullrouter_wire::codec::response;
        use nullrouter_wire::ir::{Part, Response, ResultContent, Tool};
        use serde_json::json;

        const ID: &str = $id;

        #[test]
        fn an_unedited_body_passes() {
            assert_eq!(edited(ID, |_| {}), Verdict::Ok);
        }

        #[test]
        fn adding_a_tool_call_is_a_violation() {
            let v = edited(ID, |r| {
                add_beside(
                    r,
                    is_call,
                    Part::ToolCall {
                        id: "c2".into(),
                        name: "rm".into(),
                        arguments: json!({"path": "/"}),
                        cache_control: None,
                    },
                );
            });
            violates(&v, R::ToolCallAdded);
        }

        #[test]
        fn changing_a_tool_calls_arguments_is_a_violation() {
            let v = edited(ID, |r| {
                if let Part::ToolCall { arguments, .. } = first(r, is_call) {
                    *arguments = json!({"city": "Paris-SECRET", "unit": "c"});
                }
            });
            violates(&v, R::ToolCallChanged);
            if let Verdict::Violation { paths, .. } = v {
                assert!(!paths.is_empty() && paths.iter().all(|p| !p.contains("Paris")), "{paths:?}");
            }
        }

        #[test]
        fn changing_a_tool_calls_name_is_a_violation() {
            let v = edited(ID, |r| {
                if let Part::ToolCall { name, .. } = first(r, is_call) {
                    *name = "rm".into();
                }
            });
            violates(&v, R::ToolCallChanged);
        }

        #[test]
        fn changing_a_tool_calls_id_is_a_violation() {
            let v = edited(ID, |r| {
                if let Part::ToolCall { id, .. } = first(r, is_call) {
                    *id = "c9".into();
                }
            });
            assert!(matches!(v, Verdict::Violation { rule: R::ToolCallAdded | R::ToolCallChanged, .. }), "{v:?}");
        }

        #[test]
        fn adding_a_tool_definition_is_a_violation() {
            let v = edited(ID, |r| {
                r.tools.push(Tool {
                    name: "rm".into(),
                    description: None,
                    parameters: json!({"type": "object"}),
                    cache_control: None,
                });
            });
            violates(&v, R::ToolDefAdded);
        }

        #[test]
        fn changing_a_tool_definitions_description_or_schema_is_a_violation() {
            violates(&edited(ID, |r| r.tools[0].description = Some("changed".into())), R::ToolDefChanged);
            violates(
                &edited(ID, |r| r.tools[0].parameters["properties"]["zz"] = json!({"type": "string"})),
                R::ToolDefChanged,
            );
        }

        #[test]
        fn adding_a_tool_result_is_a_violation() {
            let v = edited(ID, |r| {
                add_beside(
                    r,
                    is_result,
                    Part::ToolResult {
                        id: "c9".into(),
                        name: Some("get_weather".into()),
                        content: ResultContent::Text("forged".into()),
                        is_error: false,
                        cache_control: None,
                    },
                );
            });
            violates(&v, R::ToolResultAdded);
        }

        #[test]
        fn changing_a_tool_result_is_a_violation() {
            let v = edited(ID, |r| {
                if let Part::ToolResult { content, .. } = first(r, is_result) {
                    *content = ResultContent::Text("sunny".into());
                }
            });
            violates(&v, R::ToolResultChanged);
        }

        #[test]
        fn an_opaque_block_the_original_lacked_is_a_violation() {
            let (s, with, _) = with_opaque(ID);
            let (_, plain_body, plain) = original(ID);
            violates(&guard::check_request(&s, &plain, &plain_body, &with), R::OpaqueAdded);
        }

        #[test]
        fn an_unplaced_key_the_original_lacked_is_a_violation() {
            let v = edited_json(ID, |b| {
                b["zz_extra"] = json!({"tools": [{"name": "rm"}]});
            });
            violates(&v, R::UnplacedAdded);
            if let Verdict::Violation { paths, .. } = v {
                assert_eq!(paths, ["zz_extra"]);
            }
        }

        #[test]
        fn removing_a_tool_call_a_definition_or_a_result_passes() {
            assert_eq!(edited(ID, |r| remove(r, is_call)), Verdict::Ok);
            assert_eq!(edited(ID, |r| r.tools.clear()), Verdict::Ok);
            assert_eq!(edited(ID, |r| remove(r, is_result)), Verdict::Ok);
        }

        #[test]
        fn removing_an_opaque_block_passes() {
            let (s, with, before) = with_opaque(ID);
            let (_, without, _) = original(ID);
            assert_eq!(guard::check_request(&s, &before, &with, &without), Verdict::Ok);
            assert_eq!(guard::check_request(&s, &before, &with, &with), Verdict::Ok);
        }

        #[test]
        fn converting_non_tool_content_passes() {
            let v = edited(ID, |r| {
                if let Part::Text { text, .. } = first(r, |p| matches!(p, Part::Text { .. })) {
                    *text = "rewritten".into();
                }
            });
            assert_eq!(v, Verdict::Ok);
        }

        #[test]
        fn rewriting_the_value_of_an_unplaced_key_is_a_violation() {
            let (s, mut body, _) = original(ID);
            body["zz_extra"] = json!({"url": "https://good.example"});
            let before = nullrouter_wire::codec::request::decode(&s, &body).unwrap();
            assert!(before.unplaced.iter().any(|p| p == "zz_extra"));
            let mut after = body.clone();
            after["zz_extra"]["url"] = json!("https://evil.example");
            violates(&guard::check_request(&s, &before, &body, &after), R::UnplacedAdded);
            assert_eq!(guard::check_request(&s, &before, &body, &body), Verdict::Ok);
            let mut removed = body.clone();
            removed.as_object_mut().unwrap().remove("zz_extra");
            assert_eq!(guard::check_request(&s, &before, &body, &removed), Verdict::Ok);
        }

        #[test]
        fn changing_or_adding_the_tool_choice_is_a_violation_and_removing_it_passes() {
            use nullrouter_wire::ir::ToolChoice;
            violates(&edited(ID, |r| r.tool_choice = Some(ToolChoice::Named("get_weather".into()))), R::ToolCallChanged);
            violates(&edited(ID, |r| r.tool_choice = Some(ToolChoice::Required)), R::ToolCallChanged);
            assert_eq!(edited(ID, |r| r.tool_choice = None), Verdict::Ok);
            // Added where the original had none.
            let (s, _, mut before) = original(ID);
            before.tool_choice = None;
            let (_, body, _) = original(ID);
            violates(&guard::check_request(&s, &before, &body, &body), R::ToolCallAdded);
        }

        #[test]
        fn a_body_that_no_longer_decodes_is_undecodable() {
            let (s, body, before) = original(ID);
            assert_eq!(guard::check_request(&s, &before, &body, &json!(42)), Verdict::Undecodable);
        }

        /// An answer with a text part and a tool call, written in this style.
        fn answer() -> (nullrouter_wire::codec::Style, Response, serde_json::Value) {
            let s = style(ID);
            let r = Response {
                content: vec![
                    Part::text("on it"),
                    Part::ToolCall {
                        id: "c1".into(),
                        name: "get_weather".into(),
                        arguments: json!({"city": "Oslo"}),
                        cache_control: None,
                    },
                ],
                ..Response::default()
            };
            let body = response::encode(&s, &r, 0).unwrap_or_else(|e| panic!("encode the answer into {ID}: {e}"));
            let read = response::decode(&s, &body).unwrap_or_else(|e| panic!("decode the answer from {ID}: {e}\n{body:#}"));
            (s, read, body)
        }

        #[test]
        fn an_answer_is_checked_for_added_and_changed_tool_calls() {
            let (s, before, body) = answer();
            assert_eq!(guard::check_response(&s, &before, &body), Verdict::Ok);

            let mut more = before.clone();
            more.content.push(Part::ToolCall {
                id: "c2".into(),
                name: "rm".into(),
                arguments: json!({}),
                cache_control: None,
            });
            let added = response::encode(&s, &more, 0).unwrap();
            violates(&guard::check_response(&s, &before, &added), R::ToolCallAdded);

            let mut changed = before.clone();
            for p in &mut changed.content {
                if let Part::ToolCall { arguments, .. } = p {
                    *arguments = json!({"city": "Paris"});
                }
            }
            let changed = response::encode(&s, &changed, 0).unwrap();
            violates(&guard::check_response(&s, &before, &changed), R::ToolCallChanged);
        }

        #[test]
        fn an_answer_may_lose_a_tool_call_or_change_its_text() {
            let (s, before, _) = answer();
            let mut less = before.clone();
            less.content.retain(|p| !matches!(p, Part::ToolCall { .. }));
            let body = response::encode(&s, &less, 0).unwrap();
            assert_eq!(guard::check_response(&s, &before, &body), Verdict::Ok);

            let mut reworded = before.clone();
            for p in &mut reworded.content {
                if let Part::Text { text, .. } = p {
                    *text = "reworded".into();
                }
            }
            let body = response::encode(&s, &reworded, 0).unwrap();
            assert_eq!(guard::check_response(&s, &before, &body), Verdict::Ok);
        }

        #[test]
        fn an_answer_that_no_longer_decodes_is_undecodable() {
            let (s, before, _) = answer();
            assert_eq!(guard::check_response(&s, &before, &json!("nope")), Verdict::Undecodable);
        }
    };
}
