use super::*;
use crate::provider_fixture as fixture;
use crate::{PROVIDER_ITEM_FIELDS, PROVIDER_PART_FIELDS, ProviderJsonKind};

fn rejection(events: &[Value], crlf: bool) -> ProviderFailure {
    let mut parser = ResponsesParser::new();
    assert_eq!(
        fixture::wire(events, crlf)
            .into_iter()
            .find_map(|b| parser.byte(b).err()),
        Some(Outcome::Framing)
    );
    assert!(parser.terminal.is_none());
    let failure = parser.failure.clone();
    assert!(parser.complete().is_err());
    let report = serde_json::to_string(&failure).unwrap();
    assert!(!report.contains("synthetic-private"));
    assert!(!report.contains("msg_1"));
    assert!(!report.contains("\u{001b}"));
    failure
}
#[test]
fn final_differences_preserve_presence_kinds_and_fixed_field_masks() {
    for crlf in [false, true] {
        for (index, name) in PROVIDER_ITEM_FIELDS.iter().enumerate() {
            for value in [
                Value::Null,
                json!(42),
                json!("synthetic-private\u{001b}hé🙂"),
                json!({"synthetic-private":true}),
            ] {
                let mut events = fixture::text_events("synthetic-private", fixture::known_usage());
                events.last_mut().unwrap()["response"]["output"][0][*name] = value.clone();
                let difference = rejection(&events, crlf).final_difference.unwrap();
                let item = difference.items[0].unwrap();
                assert_eq!(item.fields, 1 << index, "{name}");
                assert_eq!(item.index, 0);
                assert_eq!(
                    item.final_kinds[index],
                    match value {
                        Value::Null => ProviderJsonKind::Null,
                        Value::Number(_) => ProviderJsonKind::Number,
                        Value::String(_) => ProviderJsonKind::String,
                        _ => ProviderJsonKind::Object,
                    }
                );
            }
        }
        for (index, name) in PROVIDER_PART_FIELDS.iter().enumerate() {
            let mut events = fixture::text_events("synthetic-private", fixture::known_usage());
            events.last_mut().unwrap()["response"]["output"][0]["content"][0][*name] =
                json!({"synthetic-private":null});
            let item = rejection(&events, crlf).final_difference.unwrap().items[0].unwrap();
            let part = item.content.unwrap().first_part.unwrap();
            assert_eq!(part.index, 0);
            assert_eq!(part.fields, 1 << index);
            assert!(!part.additional_parts);
        }
        let failure = rejection(&fixture::disagreement_events(), crlf);
        let item = failure.final_difference.unwrap().items[0].unwrap();
        assert!(item.other_fields);
        assert_eq!(item.done_kinds[9], ProviderJsonKind::Missing);
        assert_eq!(item.final_kinds[9], ProviderJsonKind::String);
        assert_eq!(item.content.unwrap().first_part.unwrap().fields, 8);
    }
}
#[test]
fn final_differences_cover_missing_done_counts_shapes_and_metadata_replay() {
    for crlf in [false, true] {
        let mut events = fixture::text_events("hé🙂", fixture::known_usage());
        events.remove(7);
        let item = rejection(&events, crlf).final_difference.unwrap().items[0].unwrap();
        assert_eq!(item.done_kind, ProviderJsonKind::Missing);
        assert_eq!(item.final_kind, ProviderJsonKind::Object);
        for value in [
            Value::Null,
            json!({}),
            json!("synthetic-private"),
            json!([]),
            json!([null, null]),
        ] {
            let mut events = fixture::text_events("x", fixture::known_usage());
            events.last_mut().unwrap()["response"]["output"] = value;
            let diff = rejection(&events, crlf).final_difference.unwrap();
            assert_eq!(diff.done_count, 1);
            assert_eq!(
                diff.final_count,
                events.last().unwrap()["response"]["output"]
                    .as_array()
                    .map(Vec::len)
            );
        }
        let mut events = fixture::call_events("{}", fixture::known_usage());
        events.last_mut().unwrap()["response"]["output"][0]
            .as_object_mut()
            .unwrap()
            .remove("status");
        let item = rejection(&events, crlf).final_difference.unwrap().items[0].unwrap();
        assert_eq!(item.done_kinds[3], ProviderJsonKind::String);
        assert_eq!(item.final_kinds[3], ProviderJsonKind::Missing);
        let mut events = fixture::text_events("hé🙂", fixture::known_usage());
        for index in [7, 8] {
            let item = if index == 7 {
                &mut events[index]["item"]
            } else {
                &mut events[index]["response"]["output"][0]
            };
            item["phase"] = json!("final_answer");
            item["content"][0]["annotations"] =
                json!([{"type":"url_citation","url":"synthetic-private","title":"hé🙂"}]);
            item["content"][0]["logprobs"] = Value::Null;
        }
        let parsed = parse(&fixture::wire(&events, crlf)).unwrap();
        let wire = parsed.envelope.unwrap().items[0].wire.clone();
        assert_eq!(wire["phase"], "final_answer");
        assert_eq!(
            wire["content"][0]["annotations"][0]["url"],
            "synthetic-private"
        );
        assert!(wire["content"][0]["logprobs"].is_null());
    }
}
#[test]
fn fixed_differences_bound_nested_and_many_part_evidence_without_raw_payloads() {
    let mut nested = json!("synthetic-private");
    for _ in 0..100 {
        nested = json!({"synthetic-private":nested});
    }
    let done = json!({"id":"synthetic-private", "content":vec![json!({"type":"output_text","text":"synthetic-private","annotations":[]}); 256], "summary":[], "phase":null});
    let mut final_item = done.clone();
    for part in final_item["content"].as_array_mut().unwrap() {
        part["text"] = json!("different-private");
        part["annotations"] = nested.clone();
        part["synthetic-private\u{001b}"] = json!(true);
    }
    let mut diff = ProviderFinalDifference::new(8, Some(&json!([])));
    for index in 0..8 {
        diff.item(index, Some(&done), Some(&final_item));
    }
    for item in diff.items.iter().flatten() {
        let part = item.content.unwrap().first_part.unwrap();
        assert!(part.additional_parts && part.other_fields);
        assert_eq!(part.fields, 6);
    }
    let serialized = serde_json::to_string(&diff).unwrap();
    assert!(!serialized.contains("private"));
    assert!(serialized.len() < crate::PROVIDER_DIAGNOSTIC_BYTES);
    assert!(std::mem::size_of::<ProviderFailure>() < 4096);
}

fn parse(bytes: &[u8]) -> Result<ParsedResponse, Outcome> {
    let mut parser = ResponsesParser::new();
    for byte in bytes {
        parser.byte(*byte)?;
    }
    parser.complete()
}
#[test]
fn text_and_call_roundtrip_every_split_and_newline() {
    for crlf in [false, true] {
        for events in [
            fixture::text_events("hé🙂", fixture::known_usage()),
            fixture::call_events("{\"text\":\"hé🙂\"}", fixture::known_usage()),
        ] {
            let bytes = fixture::wire(&events, crlf);
            for split in 0..=bytes.len() {
                let mut parser = ResponsesParser::new();
                for chunk in [&bytes[..split], &bytes[split..]] {
                    for byte in chunk {
                        parser.byte(*byte).unwrap();
                    }
                }
                let parsed = parser.complete().unwrap();
                assert_eq!(parsed.response_id, "resp_1");
                assert!(parsed.usage.unwrap().complete());
                let envelope = parsed.envelope.unwrap();
                let mut input = vec![ProviderInput::User("synthetic".into())];
                for item in envelope.items {
                    let call = item.function_call().map(|c| c.call_id.to_owned());
                    input.push(ProviderInput::Item(item));
                    if let Some(call_id) = call {
                        input.push(ProviderInput::ToolResult {
                            call_id,
                            output: "read back".into(),
                        });
                    }
                }
                let request = ProviderRequest::new(&input, &[]).unwrap();
                let value = strict_json(request.encoded.as_bytes()).unwrap();
                assert_eq!(value["model"], PROVIDER_MODEL);
                assert_eq!(value["max_output_tokens"], 1024);
                assert_eq!(value["store"], false);
                assert_eq!(
                    value["input"][1]["id"],
                    if events.len() == 9 { "msg_1" } else { "fc_1" }
                );
            }
        }
    }
}
#[test]
fn rejects_identity_order_terminal_and_strict_argument_mutations() {
    let good = fixture::call_events("{}", fixture::known_usage());
    for (event, field, value) in [
        (0, "type", json!("response.in_progress")),
        (
            0,
            "response",
            json!({"id":"","status":"in_progress","output":[]}),
        ),
        (1, "output_index", json!(1)),
        (2, "item_id", json!("foreign")),
        (3, "arguments", json!("{\"x\":1}")),
        (
            4,
            "item",
            json!({"type":"function_call","id":"fc_foreign","call_id":"call_1","name":"inert","status":"completed","arguments":"{}"}),
        ),
        (
            5,
            "response",
            json!({"id":"resp_foreign","status":"completed","output":[]}),
        ),
    ] {
        let mut events = good.clone();
        events[event][field] = value;
        assert!(
            parse(&fixture::wire(&events, false)).is_err(),
            "event {event} field {field}"
        );
    }
    for arguments in [
        "{\"x\":1,\"x\":2}",
        "{\"nested\":{\"x\":1,\"x\":2}}",
        "{} trailing",
        "[]",
        "null",
        "{\"x\":NaN}",
        "{\"x\":1e999}",
    ] {
        assert!(
            parse(&fixture::wire(
                &fixture::call_events(arguments, Value::Null),
                false
            ))
            .is_err(),
            "{arguments}"
        );
    }
    for i in 0..good.len() {
        let mut events = good.clone();
        events.remove(i);
        assert!(
            parse(&fixture::wire(&events, false)).is_err(),
            "missing event {i}"
        );
        let mut events = good.clone();
        events.insert(i, events[i].clone());
        assert!(
            parse(&fixture::wire(&events, false)).is_err(),
            "duplicate event {i}"
        );
    }
    let bytes = fixture::wire(&good, false);
    for end in 0..bytes.len() {
        assert!(parse(&bytes[..end]).is_err());
    }
    for suffix in [b"x".as_slice(), b"\n", b"data: {\"type\":\"error\"}\n\n"] {
        let mut bytes = bytes.clone();
        bytes.extend_from_slice(suffix);
        assert!(parse(&bytes).is_err());
    }
}
#[test]
fn bounded_opaque_ids_are_correlated_and_replayed_without_character_normalization() {
    let mut events = fixture::call_events("{}", Value::Null);
    let response_id = "resp:/+==.opaque";
    let item_id = "fc:/+==.opaque";
    let call_id = "call:/+==.opaque";
    events[0]["response"]["id"] = json!(response_id);
    for i in [1, 4] {
        events[i]["item"]["id"] = json!(item_id);
        events[i]["item"]["call_id"] = json!(call_id);
    }
    for i in [2, 3] {
        events[i]["item_id"] = json!(item_id);
    }
    events[5]["response"]["id"] = json!(response_id);
    events[5]["response"]["output"][0]["id"] = json!(item_id);
    events[5]["response"]["output"][0]["call_id"] = json!(call_id);
    let parsed = parse(&fixture::wire(&events, false)).unwrap();
    let envelope = parsed.envelope.unwrap();
    assert_eq!(envelope.response_id, response_id);
    let request = ProviderRequest::new(
        &[
            ProviderInput::Item(envelope.items[0].clone()),
            ProviderInput::ToolResult {
                call_id: call_id.into(),
                output: "observed".into(),
            },
        ],
        &[],
    )
    .unwrap();
    assert_eq!(
        strict_json(request.encoded.as_bytes()).unwrap()["input"][0]["id"],
        item_id
    );
    assert_eq!(
        strict_json(request.encoded.as_bytes()).unwrap()["input"][1]["call_id"],
        call_id
    );
    assert!(identity(&"x".repeat(4096)));
    assert!(identity(&"\u{0000}".repeat(1024)));
    assert!(!identity(""));
    for (value, problem, bytes) in [
        (Value::Null, ProviderIdentityProblem::NotString, None),
        (json!(""), ProviderIdentityProblem::Empty, Some(0)),
    ] {
        let mut created = events[0].clone();
        created["response"]["id"] = value;
        let mut parser = ResponsesParser::new();
        assert!(
            fixture::wire(&[created], false)
                .into_iter()
                .any(|byte| parser.byte(byte).is_err())
        );
        assert_eq!(parser.failure.identity_problem, Some(problem));
        assert_eq!(parser.failure.identity_bytes, bytes);
    }
}
#[test]
fn final_function_calls_keep_optional_status_and_initial_argument_evidence() {
    let arguments = "{\"message\":\"synthetic\"}";
    let mut events = fixture::call_events(arguments, fixture::known_usage());
    for i in [1, 4] {
        events[i]["item"].as_object_mut().unwrap().remove("status");
    }
    events[5]["response"]["output"][0]
        .as_object_mut()
        .unwrap()
        .remove("status");
    events[1]["item"]["arguments"] = json!(arguments);
    events.remove(2);
    let parsed = parse(&fixture::wire(&events, false)).unwrap();
    assert_eq!(
        parsed.envelope.unwrap().items[0]
            .function_call()
            .unwrap()
            .arguments,
        arguments
    );
    events[1]["item"]["arguments"] = json!("{\"message\":\"different\"}");
    assert!(parse(&fixture::wire(&events, false)).is_err());
}
#[test]
fn opaque_ids_and_reported_models_use_aggregate_budgets_not_guessed_field_caps() {
    for character in ['x', '\"', '\\', '\u{0000}', 'é', '🙂'] {
        let id = character.to_string().repeat(512);
        let mut events = fixture::text_events("x", fixture::known_usage());
        events[0]["response"]["id"] = json!(id);
        events[1]["response"]["id"] = json!(id);
        events[8]["response"]["id"] = json!(id);
        let parsed = parse(&fixture::wire(&events, false)).unwrap();
        assert_eq!(parsed.response_id, id);
    }
    let mut events = fixture::call_events("{}", fixture::known_usage());
    let id = "opaque:/+==".repeat(384);
    let model = "engine/é.".repeat(128);
    for i in [0, 5] {
        events[i]["response"]["id"] = json!(id);
        events[i]["response"]["model"] = json!(model);
    }
    for i in [1, 4] {
        events[i]["item"]["id"] = json!(id);
        events[i]["item"]["call_id"] = json!(id);
    }
    for i in [2, 3] {
        events[i]["item_id"] = json!(id);
    }
    events[5]["response"]["output"][0] = events[4]["item"].clone();
    let parsed = parse(&fixture::wire(&events, false)).unwrap();
    assert_eq!(parsed.reported_model.as_deref(), Some(model.as_str()));
    let request = ProviderRequest::new(
        &[
            ProviderInput::Item(parsed.envelope.unwrap().items[0].clone()),
            ProviderInput::ToolResult {
                call_id: id.clone(),
                output: "observed".into(),
            },
        ],
        &[],
    )
    .unwrap();
    assert_eq!(
        strict_json(request.encoded.as_bytes()).unwrap()["input"][1]["call_id"],
        id
    );
    let mut events = fixture::text_events("x", Value::Null);
    events[0]["response"]["id"] = json!("x".repeat(PROVIDER_FRAME_BYTES));
    assert!(matches!(
        parse(&fixture::wire(&events, false)),
        Err(Outcome::ResourceLimit)
    ));
}

#[test]
fn byte_bounded_large_part_lists_preserve_metadata_without_saving_small_allocations() {
    let mut events = fixture::text_events("x", fixture::known_usage());
    events.truncate(3);
    let mut parts = Vec::new();
    for index in 0..32 {
        let part = json!({"type":"output_text","text":"x","annotations":[{"type":"url_citation","url":"synthetic-reference"}]});
        events.push(json!({"type":"response.content_part.added","output_index":0,"item_id":"msg_1","content_index":index,"part":{"type":"output_text","text":""}}));
        events.push(json!({"type":"response.output_text.delta","output_index":0,"item_id":"msg_1","content_index":index,"delta":"x"}));
        events.push(json!({"type":"response.output_text.done","output_index":0,"item_id":"msg_1","content_index":index,"text":"x"}));
        events.push(json!({"type":"response.content_part.done","output_index":0,"item_id":"msg_1","content_index":index,"part":part}));
        parts.push(part);
    }
    let item = json!({"type":"message","id":"msg_1","role":"assistant","status":"completed","content":parts});
    events.push(json!({"type":"response.output_item.done","output_index":0,"item":item}));
    events.push(json!({"type":"response.completed","response":{"id":"resp_1","status":"completed","output":[item],"usage":fixture::known_usage()}}));
    let parsed = parse(&fixture::wire(&events, false)).unwrap();
    let item = &parsed.envelope.unwrap().items[0];
    assert_eq!(item.text().unwrap(), "x".repeat(32));
    assert_eq!(
        item.wire["content"][31]["annotations"][0]["url"],
        "synthetic-reference"
    );
}

#[test]
fn valid_deep_json_is_a_work_budget_refusal_not_malformed_protocol() {
    let valid = format!("{}0{}", "[".repeat(100), "]".repeat(100));
    let value = strict_json(valid.as_bytes()).unwrap();
    assert!(bounded_json(&value, PROVIDER_ENVELOPE_BYTES).is_ok());
    let deeper = format!("{}0{}", "[".repeat(256), "]".repeat(256));
    assert_eq!(strict_json(deeper.as_bytes()), Err(Outcome::ResourceLimit));
    assert_eq!(strict_json(b"[broken]"), Err(Outcome::Framing));
}
#[test]
fn representative_complete_parent_context_with_185_byte_ids_fits_without_clipping() {
    let poem = "\u{0000}".repeat(512);
    let script = "from pathlib import Path\nprint(Path('haiku.md').read_text(), end='')\n";
    let tools:Vec<_>=["write","read","bash"].into_iter().map(|name|ProviderTool {name:name.into(),description:"bounded synthetic tool".into(),
        parameters: match name {
            "write" => json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}),
            "read" => json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}),
            _ => json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"],"additionalProperties":false}),
        }}).collect();
    let steps = [
        (
            "write",
            json!({"path":"haiku.md","content":poem}),
            "created".to_owned(),
        ),
        ("read", json!({"path":"haiku.md"}), poem.clone()),
        (
            "write",
            json!({"path":"print_haiku.py","content":script}),
            "created".into(),
        ),
        (
            "bash",
            json!({"command":"/usr/bin/python3 -I print_haiku.py"}),
            poem.clone(),
        ),
    ];
    let mut input = vec![ProviderInput::User(
        "create/read a haiku, create a Python reader, execute via fixed Bash".into(),
    )];
    for (ordinal, (name, arguments, output)) in steps.into_iter().enumerate() {
        let mut events = fixture::call_events(&arguments.to_string(), fixture::known_usage());
        let response_id = format!("resp.{ordinal}{}", "x".repeat(179));
        let item_id = format!("item.{ordinal}{}", "x".repeat(179));
        let call_id = format!("call.{ordinal}{}", "x".repeat(179));
        events[0]["response"]["id"] = json!(response_id);
        events[5]["response"]["id"] = json!(response_id);
        for i in [1, 4] {
            events[i]["item"]["id"] = json!(item_id);
            events[i]["item"]["call_id"] = json!(call_id);
            events[i]["item"]["name"] = json!(name);
        }
        for i in [2, 3] {
            events[i]["item_id"] = json!(item_id);
        }
        events[5]["response"]["output"][0] = events[4]["item"].clone();
        let envelope = parse(&fixture::wire(&events, false))
            .unwrap()
            .envelope
            .unwrap();
        input.push(ProviderInput::Item(envelope.items[0].clone()));
        input.push(ProviderInput::ToolResult { call_id, output });
    }
    let request = ProviderRequest::new(&input, &tools).unwrap();
    println!(
        "representative-parent-185-id-request={}",
        request.encoded_bytes()
    );
    assert!(request.encoded_bytes() <= PROVIDER_ENVELOPE_BYTES);
    let wire = strict_json(request.encoded.as_bytes()).unwrap();
    assert_eq!(wire["input"][4]["output"], poem);
    assert_eq!(wire["input"][8]["output"], poem);
}
#[test]
fn interleaved_items_and_initial_text_remain_provisional_until_all_final_items_match() {
    let text = fixture::text_events("hé🙂", fixture::known_usage());
    let added = json!({"id":"rs_1","type":"reasoning","summary":[],"status":"in_progress"});
    let finished = json!({"id":"rs_1","type":"reasoning","summary":[],"status":"completed","encrypted_content":"opaque"});
    let mut events = vec![
        text[0].clone(),
        json!({"type":"response.output_item.added","output_index":0,"item":added}),
    ];
    for event in &text[2..text.len() - 1] {
        let mut event = event.clone();
        event["output_index"] = json!(1);
        if event["type"] == "response.content_part.added" {
            event["part"]["text"] = json!("hé🙂");
        }
        if event["type"] != "response.output_text.delta" {
            events.push(event);
        }
    }
    events.push(json!({"type":"response.output_item.done","output_index":0,"item":finished}));
    events.push(json!({"type":"response.completed","response":{"id":"resp_1","status":"completed","output":[finished,text[7]["item"]],"usage":fixture::known_usage()}}));
    let parsed = parse(&fixture::wire(&events, false)).unwrap();
    assert_eq!(
        parsed.envelope.unwrap().items[1].text().as_deref(),
        Some("hé🙂")
    );
    events.remove(events.len() - 2);
    assert!(parse(&fixture::wire(&events, false)).is_err());
}
#[test]
fn diagnostics_report_only_fixed_stage_event_and_field_labels() {
    let mut event = fixture::text_events("x", Value::Null)[0].clone();
    event["response"].as_object_mut().unwrap().remove("status");
    let mut parser = ResponsesParser::new();
    let bytes = fixture::wire(&[event], false);
    assert!(bytes.into_iter().any(|byte| parser.byte(byte).is_err()));
    assert_eq!(parser.failure.stage, ProviderFailureStage::CreatedResponse);
    assert_eq!(parser.failure.event, Some("response.created"));
    assert_eq!(parser.failure.field, Some("response.status"));
    assert_eq!(
        ProviderFailure::event("untrusted\u{001b}[remote-error"),
        "unsupported event"
    );
}
#[test]
fn response_metadata_is_consistent_without_assuming_request_model_echo() {
    let mut events = fixture::text_events("x", fixture::known_usage());
    for i in [0, 1, 8] {
        events[i]["response"]["model"] = json!("qualified-deployment-model");
    }
    let parsed = parse(&fixture::wire(&events, false)).unwrap();
    assert_eq!(
        parsed.reported_model.as_deref(),
        Some("qualified-deployment-model")
    );
    let mut contradictory = events.clone();
    contradictory[8]["response"]["model"] = json!("different");
    assert!(parse(&fixture::wire(&contradictory, false)).is_err());
    for key in ["error", "incomplete_details"] {
        let mut contradictory = events.clone();
        contradictory[8]["response"][key] = json!({"reason":"synthetic"});
        assert!(parse(&fixture::wire(&contradictory, false)).is_err());
    }
}
#[test]
fn final_item_diagnostics_distinguish_predicates_without_exposing_remote_values() {
    for (key, value, expected) in [
        (
            "error",
            json!({"message":"synthetic\u{001b}[remote"}),
            "response.error",
        ),
        (
            "incomplete_details",
            json!({"reason":"synthetic"}),
            "response.incomplete_details",
        ),
        ("output", Value::Null, "response.output"),
        ("output", json!([]), "response.output.count"),
        ("output", json!([{"id":"foreign"}]), "response.output.item"),
    ] {
        let mut events = fixture::text_events("x", fixture::known_usage());
        events.last_mut().unwrap()["response"][key] = value;
        let mut parser = ResponsesParser::new();
        let rejected = fixture::wire(&events, false)
            .into_iter()
            .find_map(|byte| parser.byte(byte).err());
        assert_eq!(rejected, Some(Outcome::Framing));
        assert_eq!(parser.failure.stage, ProviderFailureStage::FinalItems);
        assert_eq!(parser.failure.event, Some("response.completed"));
        assert_eq!(parser.failure.field, Some(expected));
        assert_eq!(parser.failure.identity_problem, None);
        assert_eq!(parser.failure.identity_bytes, None);
        assert!(parser.terminal.is_none());
        assert!(parser.complete().is_err());
    }
}
#[test]
fn standard_sse_metadata_multiline_data_and_optional_done_do_not_mint_authority() {
    for newline in ["\n", "\r\n"] {
        let mut wire = String::new();
        for (sequence, event) in fixture::text_events("hé🙂", fixture::known_usage())
            .into_iter()
            .enumerate()
        {
            let mut event = event;
            event["sequence_number"] = json!(sequence);
            wire.push_str(&format!(
                ": heartbeat{newline}id: ignored{newline}retry: 1{newline}event:{}{newline}",
                event["type"].as_str().unwrap()
            ));
            for line in serde_json::to_string_pretty(&event).unwrap().lines() {
                wire.push_str(&format!("data:{line}{newline}"));
            }
            wire.push_str(newline);
        }
        wire.push_str(&format!(
            ": end heartbeat{newline}{newline}data: [DONE]{newline}{newline}"
        ));
        let parsed = parse(wire.as_bytes()).unwrap();
        assert!(parsed.envelope.is_some());
        assert!(parsed.usage.unwrap().complete());
        wire.push_str(&format!("data: [DONE]{newline}{newline}"));
        assert!(parse(wire.as_bytes()).is_err());
    }
    assert!(parse(b"data: [DONE]\n\n").is_err());
    assert!(parse(b": heartbeat\n\n").is_err());
}
#[test]
fn failures_preserve_usage_without_executable_items() {
    for status in ["failed", "incomplete"] {
        let events = vec![
            fixture::text_events("x", Value::Null)[0].clone(),
            json!({"type":format!("response.{status}"),"response":{"id":"resp_1","status":status,"output":[],"usage":fixture::known_usage()}}),
        ];
        let parsed = parse(&fixture::wire(&events, false)).unwrap();
        assert!(parsed.envelope.is_none());
        assert!(parsed.usage.unwrap().complete());
        assert_eq!(parsed.status, status);
    }
    let parsed = parse(&fixture::wire(
        &fixture::text_events("x", Value::Null),
        false,
    ))
    .unwrap();
    assert!(parsed.envelope.is_some());
    assert!(parsed.usage.is_none());
    for usage in [
        json!({"input_tokens":1,"output_tokens":2,"total_tokens":4}),
        json!({"input_tokens":1,"input_tokens_details":{"cached_tokens":2}}),
        json!({"output_tokens":1,"output_tokens_details":{"reasoning_tokens":2}}),
        json!({"input_tokens":-1}),
        json!({"input_tokens":1.5}),
        json!({"input_tokens_details":{"cached_tokens":0}}),
    ] {
        assert!(parse(&fixture::wire(&fixture::text_events("x", usage), false)).is_err());
    }
    let parsed = parse(&fixture::wire(
        &fixture::text_events("x", json!({"input_tokens":3})),
        false,
    ))
    .unwrap();
    assert!(!parsed.usage.unwrap().complete());
}
#[test]
fn request_and_frame_limits_reject_without_clipping() {
    let mut parser = ResponsesParser::new();
    for _ in 0..PROVIDER_FRAME_BYTES {
        parser.byte(b'x').unwrap();
    }
    assert_eq!(parser.byte(b'x'), Err(Outcome::ResourceLimit));
    assert!(
        ProviderRequest::new(
            &[ProviderInput::User("x".repeat(PROVIDER_ENVELOPE_BYTES))],
            &[]
        )
        .is_err()
    );
    assert!(
        ProviderRequest::new(
            &[ProviderInput::User(
                "\u{0000}".repeat(PROVIDER_ENVELOPE_BYTES / 6)
            )],
            &[]
        )
        .is_err()
    );
    let request =
        ProviderRequest::new(&[ProviderInput::User("\u{0000}".repeat(2000))], &[]).unwrap();
    assert!(request.encoded_bytes() <= PROVIDER_ENVELOPE_BYTES);
    assert_eq!(
        strict_json(request.encoded.as_bytes()).unwrap()["input"][0]["content"]
            .as_str()
            .unwrap()
            .len(),
        2000
    );
    assert!(strict_json(b"{\"type\":1,\"type\":2}").is_err());
    assert!(strict_json(b"{\"a\":{\"x\":1,\"x\":2}}").is_err());
    assert!(parse(b"data: \xff\n\n").is_err());
    assert!(parse(b"data: {}\ndata: {}\n\n").is_err());
}
#[test]
fn opaque_reasoning_and_complete_mixed_output_survive_replay() {
    let reasoning_added = json!({"type":"reasoning","id":"rs_1","summary":[]});
    let reasoning_done =
        json!({"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"opaque-synthetic"});
    let call = fixture::call_events("{}", fixture::known_usage());
    let text = fixture::text_events("after call", fixture::known_usage());
    let mut events = vec![
        call[0].clone(),
        json!({"type":"response.output_item.added","output_index":0,"item":reasoning_added}),
        json!({"type":"response.output_item.done","output_index":0,"item":reasoning_done}),
    ];
    for event in &call[1..call.len() - 1] {
        let mut event = event.clone();
        event["output_index"] = json!(1);
        events.push(event);
    }
    for event in &text[2..text.len() - 1] {
        let mut event = event.clone();
        event["output_index"] = json!(2);
        events.push(event);
    }
    events.push(json!({"type":"response.completed","response":{"id":"resp_1","status":"completed",
        "output":[reasoning_done,call[4]["item"].clone(),text[7]["item"].clone()],"usage":fixture::known_usage()}}));
    let parsed = parse(&fixture::wire(&events, false)).unwrap();
    let mut input = vec![ProviderInput::User("synthetic".into())];
    input.extend(
        parsed
            .envelope
            .unwrap()
            .items
            .into_iter()
            .map(ProviderInput::Item),
    );
    input.push(ProviderInput::ToolResult {
        call_id: "call_1".into(),
        output: "observed".into(),
    });
    let request = ProviderRequest::new(&input, &[]).unwrap();
    let wire = strict_json(request.encoded.as_bytes()).unwrap();
    assert_eq!(wire["input"][1]["encrypted_content"], "opaque-synthetic");
    assert_eq!(wire["input"][2]["call_id"], "call_1");
    assert_eq!(wire["input"][3]["id"], "msg_1");
    assert_eq!(wire["input"][4]["output"], "observed");
}
#[test]
fn unsupported_namespace_and_part_or_item_repetition_are_non_executable() {
    let mut events = fixture::call_events("{}", Value::Null);
    events[1]["item"]["namespace"] = json!("foreign");
    events[4]["item"]["namespace"] = json!("foreign");
    events[5]["response"]["output"][0]["namespace"] = json!("foreign");
    assert!(parse(&fixture::wire(&events, false)).is_err());
    let text = fixture::text_events("x", Value::Null);
    for i in 0..text.len() {
        let mut events = text.clone();
        events.insert(i, events[i].clone());
        assert!(
            parse(&fixture::wire(&events, false)).is_err(),
            "repeated text event {i}"
        );
    }
    for kind in [
        "response.refusal.delta",
        "error",
        "response.output_text.annotation.added",
        "response.unknown",
    ] {
        let mut events = text.clone();
        events[4]["type"] = json!(kind);
        assert!(parse(&fixture::wire(&events, false)).is_err());
    }
}
#[test]
fn compatibility_resource_and_malformed_failures_are_distinct() {
    let mut events = fixture::call_events("{}", Value::Null);
    events[1]["item"]["namespace"] = json!("foreign");
    assert!(matches!(
        parse(&fixture::wire(&events, false)),
        Err(Outcome::Unsupported)
    ));
    events[1]["item"]["namespace"] = json!(42);
    assert!(matches!(
        parse(&fixture::wire(&events, false)),
        Err(Outcome::Framing)
    ));
    let mut events = vec![fixture::text_events("x", Value::Null)[0].clone()];
    for index in 0..=PROVIDER_OUTPUT_ITEMS {
        let item = json!({"id":format!("rs_{index}"),"type":"reasoning","summary":[]});
        events.push(json!({"type":"response.output_item.added","output_index":index,"item":item}));
        events.push(json!({"type":"response.output_item.done","output_index":index,"item":item}));
    }
    assert!(matches!(
        parse(&fixture::wire(&events, false)),
        Err(Outcome::ResourceLimit)
    ));
}
#[test]
fn replay_requires_actual_call_result_identity() {
    let envelope = parse(&fixture::wire(
        &fixture::call_events("{}", Value::Null),
        false,
    ))
    .unwrap()
    .envelope
    .unwrap();
    let item = envelope.items[0].clone();
    assert!(ProviderRequest::new(&[ProviderInput::Item(item.clone())], &[]).is_err());
    assert!(
        ProviderRequest::new(
            &[
                ProviderInput::Item(item.clone()),
                ProviderInput::ToolResult {
                    call_id: "foreign".into(),
                    output: "x".into()
                }
            ],
            &[]
        )
        .is_err()
    );
    assert!(
        ProviderRequest::new(
            &[ProviderInput::ToolResult {
                call_id: "call_1".into(),
                output: "x".into()
            }],
            &[]
        )
        .is_err()
    );
    assert!(
        ProviderRequest::new(
            &[
                ProviderInput::Item(item.clone()),
                ProviderInput::ToolResult {
                    call_id: "call_1".into(),
                    output: "x".into()
                },
                ProviderInput::Item(item)
            ],
            &[]
        )
        .is_err()
    );
}
