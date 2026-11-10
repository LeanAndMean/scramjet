use super::*;
use crate::provider_fixture as fixture;

fn parse(bytes: &[u8]) -> Result<ParsedResponse, Outcome> {
    let mut parser = ResponsesParser::new();
    for byte in bytes {
        parser.byte(*byte)?;
    }
    parser.complete()
}
fn complete(usage: &ProviderUsage) -> bool {
    usage.input_tokens.is_some() && usage.output_tokens.is_some() && usage.total_tokens.is_some()
}
fn message_text(item: &ProviderItem) -> Option<String> {
    (item.raw()["type"] == "message").then(|| {
        item.raw()["content"]
            .as_array()
            .unwrap()
            .iter()
            .map(|part| part["text"].as_str().unwrap())
            .collect()
    })
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
                assert!(complete(&parsed.usage.unwrap()));
                let envelope = parsed.envelope.unwrap();
                assert_eq!(
                    envelope.items[0].raw()["id"],
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
        (
            4,
            "item",
            json!({"type":"function_call","id":"fc_1","call_id":"call_foreign","name":"inert","status":"completed","arguments":"{}"}),
        ),
        (
            5,
            "response",
            json!({"id":"resp_1","status":"in_progress","output":[]}),
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
    // Without its `response.created`, its done copy or its terminal event, a call does not run;
    // as in Scramjet's package, the done copy is what runs, whatever came before it.
    let arguments = |events: &[Value]| {
        parse(&fixture::wire(events, false))
            .map(|parsed| parsed.envelope.unwrap().items[0].raw()["arguments"].clone())
    };
    for i in 0..good.len() {
        let mut events = good.clone();
        events.remove(i);
        match i {
            0 | 4 | 5 => assert!(arguments(&events).is_err(), "missing event {i}"),
            _ => assert_eq!(arguments(&events), Ok(json!("{}")), "missing event {i}"),
        }
        if i == 0 || i == good.len() - 1 {
            continue;
        }
        let mut events = good.clone();
        events.insert(i, events[i].clone());
        assert_eq!(arguments(&events), Ok(json!("{}")), "duplicate event {i}");
    }
    let mut events = good.clone();
    events[3]["arguments"] = json!("{\"x\":1}");
    assert_eq!(arguments(&events), Ok(json!("{}")));
    let bytes = fixture::wire(&good, false);
    for end in 0..bytes.len() {
        assert!(parse(&bytes[..end]).is_err());
    }
    for suffix in [b"x".as_slice(), b"\n", b"data: {\"type\":\"error\"}\n\n"] {
        let mut bytes = bytes.clone();
        bytes.extend_from_slice(suffix);
        assert!(parse(&bytes).unwrap().envelope.is_some());
    }
}
#[test]
fn events_naming_ids_of_their_own_are_for_the_item_under_way() {
    // As GitHub Copilot's Responses API streams: each event's ids are new.
    let mut events = fixture::call_events("{\"path\":\"a\"}", fixture::known_usage());
    for (i, event) in events.iter_mut().enumerate() {
        let own = format!("opaque-{i}");
        if event.get("item_id").is_some() {
            event["item_id"] = json!(own);
        }
        if event["response"].is_object() {
            event["response"]["id"] = json!(format!("resp-{own}"));
        }
        if event["item"].is_object() && i > 1 {
            event["item"]["id"] = json!(own);
        }
    }
    let parsed = parse(&fixture::wire(&events, false)).unwrap();
    let items = parsed.envelope.unwrap().items;
    assert_eq!(items.len(), 1);
    // The done event's copy is the one kept, as it came.
    assert_eq!(items[0].raw()["id"], events[4]["item"]["id"]);
    assert_eq!(items[0].raw()["arguments"], "{\"path\":\"a\"}");
    // A done item of another type than the one under way is still refused.
    let mut wrong = events.clone();
    wrong[4]["item"] = json!({"type":"message","id":"m","role":"assistant","status":"completed",
        "content":[]});
    assert!(parse(&fixture::wire(&wrong, false)).is_err());
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
    assert_eq!(envelope.items[0].raw()["id"], item_id);
    assert_eq!(envelope.items[0].raw()["call_id"], call_id);
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
fn final_function_calls_keep_optional_status_and_run_their_done_copys_arguments() {
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
        parsed.envelope.unwrap().items[0].raw()["arguments"],
        arguments
    );
    // The done copy's arguments run, whatever the added item began with.
    events[1]["item"]["arguments"] = json!("{\"message\":\"different\"}");
    assert_eq!(
        parse(&fixture::wire(&events, false))
            .unwrap()
            .envelope
            .unwrap()
            .items[0]
            .raw()["arguments"],
        arguments
    );
}
#[test]
fn opaque_ids_and_reported_models_use_aggregate_budgets_not_guessed_field_caps() {
    for character in ['x', '\"', '\\', '\u{0000}', 'é', '🙂'] {
        let id = character.to_string().repeat(512);
        let mut events = fixture::text_events("x", fixture::known_usage());
        events[0]["response"]["id"] = json!(id);
        events[1]["response"]["id"] = json!(id);
        events[8]["response"]["id"] = json!(id);
        assert!(parse(&fixture::wire(&events, false)).is_ok());
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
    assert_eq!(parsed.envelope.unwrap().items[0].raw()["call_id"], id);
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
    let mut metadata = json!({"opaque":true});
    for _ in 0..10 {
        metadata = json!({"nested":metadata});
    }
    for index in 0..32 {
        let part = json!({"type":"output_text","text":format!("x{index}"),"annotations":[{"type":"url_citation","url":"synthetic-reference"}],"opaque":metadata});
        events.push(json!({"type":"response.content_part.added","output_index":0,"item_id":"msg_1","content_index":index,"part":{"type":"output_text","text":""}}));
        events.push(json!({"type":"response.output_text.delta","output_index":0,"item_id":"msg_1","content_index":index,"delta":part["text"]}));
        events.push(json!({"type":"response.output_text.done","output_index":0,"item_id":"msg_1","content_index":index,"text":part["text"]}));
        events.push(json!({"type":"response.content_part.done","output_index":0,"item_id":"msg_1","content_index":index,"part":part}));
        parts.push(part);
    }
    let item = json!({"type":"message","id":"msg_1","role":"assistant","status":"completed","content":parts});
    events.push(json!({"type":"response.output_item.done","output_index":0,"item":item}));
    events.push(json!({"type":"response.completed","response":{"id":"resp_1","status":"completed","output":[item],"usage":fixture::known_usage()}}));
    for part in events.last_mut().unwrap()["response"]["output"][0]["content"]
        .as_array_mut()
        .unwrap()
    {
        part["annotations"] = json!([]);
        part["logprobs"] = Value::Null;
    }
    let parsed = parse(&fixture::wire(&events, false)).unwrap();
    let item = &parsed.envelope.unwrap().items[0];
    assert_eq!(
        message_text(item).unwrap(),
        (0..32).map(|index| format!("x{index}")).collect::<String>()
    );
    assert_eq!(item.wire["content"][31]["opaque"], metadata);
    let mut reordered = events.clone();
    reordered.last_mut().unwrap()["response"]["output"][0]["content"]
        .as_array_mut()
        .unwrap()
        .swap(0, 31);
    assert_eq!(
        parse(&fixture::wire(&reordered, false))
            .unwrap()
            .envelope
            .unwrap()
            .items[0]
            .wire,
        item.wire
    );
    let mut changed = events.clone();
    changed.last_mut().unwrap()["response"]["output"][0]["content"][31]["opaque"]["nested"] =
        json!("changed");
    assert_eq!(
        parse(&fixture::wire(&changed, false))
            .unwrap()
            .envelope
            .unwrap()
            .items[0]
            .wire,
        item.wire
    );
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
        message_text(&parsed.envelope.unwrap().items[1]).as_deref(),
        Some("hé🙂")
    );
    events.remove(events.len() - 2);
    let parsed = parse(&fixture::wire(&events, false)).unwrap();
    assert_eq!(parsed.envelope.unwrap().items[0].raw(), &finished);
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
    assert!(parse(&fixture::wire(&events, false)).is_ok());
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
        assert!(complete(&parsed.usage.unwrap()));
        wire.push_str(&format!(
            "data: [DONE]{newline}{newline}data: junk{newline}{newline}"
        ));
        assert!(parse(wire.as_bytes()).unwrap().envelope.is_some());
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
        assert!(complete(&parsed.usage.unwrap()));
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
        json!({"input_tokens":3,"input_tokens_details":{"cached_tokens":2,"cache_write_tokens":2}}),
        json!({"input_tokens_details":{"cache_write_tokens":0}}),
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
    assert!(!complete(&parsed.usage.unwrap()));
    let cache =
        json!({"input_tokens":5,"input_tokens_details":{"cached_tokens":2,"cache_write_tokens":3}});
    let parsed = parse(&fixture::wire(&fixture::text_events("x", cache), false)).unwrap();
    let usage = parsed.usage.unwrap();
    assert_eq!(
        (usage.cached_input_tokens, usage.cache_write_tokens),
        (Some(2), Some(3))
    );
}
#[test]
fn frame_limit_and_malformed_json_reject_without_clipping() {
    let mut parser = ResponsesParser::new();
    for _ in 0..PROVIDER_FRAME_BYTES {
        parser.byte(b'x').unwrap();
    }
    assert_eq!(parser.byte(b'x'), Err(Outcome::ResourceLimit));
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
    let items = parse(&fixture::wire(&events, false))
        .unwrap()
        .envelope
        .unwrap()
        .items;
    assert_eq!(items[0].raw()["encrypted_content"], "opaque-synthetic");
    assert_eq!(items[1].raw()["call_id"], "call_1");
    assert_eq!(items[2].raw()["id"], "msg_1");
}
#[test]
fn a_namespaced_call_is_unsupported_and_repeated_events_change_nothing() {
    let mut events = fixture::call_events("{}", Value::Null);
    events[1]["item"]["namespace"] = json!("foreign");
    events[4]["item"]["namespace"] = json!("foreign");
    events[5]["response"]["output"][0]["namespace"] = json!("foreign");
    assert!(parse(&fixture::wire(&events, false)).is_err());
    let text = fixture::text_events("x", Value::Null);
    // A repeated event changes nothing, as in Scramjet's package: deltas are only shown while
    // streaming, and the final text is the done copy's. Nothing after the terminal event is read.
    for i in 1..text.len() - 1 {
        let mut events = text.clone();
        events.insert(i, events[i].clone());
        let items = parse(&fixture::wire(&events, false))
            .unwrap()
            .envelope
            .unwrap()
            .items;
        assert_eq!(items.len(), 1, "repeated text event {i}");
        assert_eq!(message_text(&items[0]).as_deref(), Some("x"));
    }
    // Other events are ignored; an `error` event ends the stream.
    for kind in [
        "response.refusal.delta",
        "response.output_text.annotation.added",
        "response.unknown",
        "error",
    ] {
        let mut events = text.clone();
        events[4]["type"] = json!(kind);
        let parsed = parse(&fixture::wire(&events, false));
        assert_eq!(parsed.is_ok(), kind != "error", "{kind}");
    }
}
#[test]
fn items_are_found_by_id_and_a_text_may_arrive_only_in_its_done_event() {
    // The shape of a deepseek-v4.1-flash reply through a gateway on 2026-10-30.
    let message = json!({"type":"message","id":"msg_1","role":"assistant","status":"completed",
        "content":[{"type":"output_text","text":"2","annotations":[]}]});
    let reasoning = json!({"type":"reasoning","id":"rs_1","status":"completed",
        "summary":[{"type":"summary_text","text":"Two lines."}]});
    let events = [
        json!({"type":"response.created","response":{"id":"resp_1","status":"in_progress","output":[]}}),
        json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg_1","role":"assistant","status":"in_progress","content":[]}}),
        json!({"type":"response.content_part.added","output_index":0,"item_id":"msg_1","content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}),
        json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs_1","status":"in_progress","summary":[]}}),
        json!({"type":"response.reasoning_summary_part.added","output_index":0,"item_id":"rs_1","summary_index":0,"part":{"type":"summary_text","text":""}}),
        json!({"type":"response.reasoning_summary_text.delta","output_index":0,"item_id":"rs_1","summary_index":0,"delta":"Two lines."}),
        json!({"type":"response.reasoning_summary_text.done","output_index":0,"item_id":"rs_1","summary_index":0,"text":"Two lines."}),
        json!({"type":"response.reasoning_summary_part.done","output_index":0,"item_id":"rs_1","summary_index":0,"part":{"type":"summary_text","text":"Two lines."}}),
        json!({"type":"response.output_item.done","output_index":0,"item":reasoning}),
        json!({"type":"response.output_text.done","output_index":0,"item_id":"msg_1","content_index":0,"text":"2"}),
        json!({"type":"response.content_part.done","output_index":0,"item_id":"msg_1","content_index":0,"part":{"type":"output_text","text":"2","annotations":[]}}),
        json!({"type":"response.output_item.done","output_index":0,"item":message}),
        json!({"type":"response.completed","response":{"id":"resp_1","status":"completed","output":[reasoning, message]}}),
    ];
    let items = parse(&fixture::wire(&events, false))
        .unwrap()
        .envelope
        .unwrap()
        .items;
    let kinds: Vec<_> = items.iter().map(|item| &item.raw()["type"]).collect();
    assert_eq!(kinds, ["message", "reasoning"]);
    assert_eq!(message_text(&items[0]).as_deref(), Some("2"));
    // A repeated `output_item.added` changes nothing, as in Scramjet's package.
    let mut repeated = events.to_vec();
    repeated.insert(4, events[3].clone());
    let again = parse(&fixture::wire(&repeated, false))
        .unwrap()
        .envelope
        .unwrap()
        .items;
    assert_eq!(again, items);
}
#[test]
fn a_response_may_make_several_function_calls() {
    let first = fixture::call_events("{}", Value::Null);
    let text = serde_json::to_string(&first).unwrap();
    let second: Vec<Value> =
        serde_json::from_str(&text.replace("fc_1", "fc_2").replace("call_1", "call_2")).unwrap();
    let mut events = vec![first[0].clone()];
    events.extend(first[1..5].iter().cloned());
    events.extend(second[1..5].iter().cloned());
    let mut completed = first[5].clone();
    completed["response"]["output"] = json!([first[4]["item"], second[4]["item"]]);
    events.push(completed);
    let items = parse(&fixture::wire(&events, false))
        .unwrap()
        .envelope
        .unwrap()
        .items;
    let ids: Vec<_> = items.iter().map(|item| &item.raw()["call_id"]).collect();
    assert_eq!(ids, ["call_1", "call_2"]);
}

#[test]
fn compatibility_resource_and_malformed_failures_are_distinct() {
    let mut events = fixture::call_events("{}", Value::Null);
    events[4]["item"]["namespace"] = json!("foreign");
    assert!(matches!(
        parse(&fixture::wire(&events, false)),
        Err(Outcome::Unsupported)
    ));
    // A namespace that is not a text is dropped, as a gateway's normalizer drops Foundry's.
    for namespace in [json!(42), Value::Null, json!({"name":"tools"})] {
        events[1]["item"]["namespace"] = namespace.clone();
        events[4]["item"]["namespace"] = namespace;
        let items = parse(&fixture::wire(&events, false))
            .unwrap()
            .envelope
            .unwrap()
            .items;
        assert!(items[0].raw().get("namespace").is_none());
    }
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
fn canonical_done_replay_budget_includes_identity_and_wrapper_not_terminal_only() {
    let id = "r".repeat(1500);
    let done =
        json!({"type":"reasoning","id":"rs","summary":[],"encrypted_content":"x".repeat(15000)});
    let mut terminal = done.clone();
    terminal
        .as_object_mut()
        .unwrap()
        .remove("encrypted_content");
    let events = vec![
        json!({"type":"response.created","response":{"id":id,"status":"in_progress","output":[]}}),
        json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs","summary":[]}}),
        json!({"type":"response.output_item.done","output_index":0,"item":done}),
        json!({"type":"response.completed","response":{"id":id,"status":"completed","output":[terminal]}}),
    ];
    assert!(bounded_json(&done, PROVIDER_ENVELOPE_BYTES).is_ok());
    assert!(
        bounded_json(
            &json!({"response_id":id,"items":[terminal]}),
            PROVIDER_ENVELOPE_BYTES
        )
        .is_ok()
    );
    for crlf in [false, true] {
        assert!(matches!(
            parse(&fixture::wire(&events, crlf)),
            Err(Outcome::ResourceLimit)
        ));
    }
}

#[test]
fn done_items_are_authoritative_when_the_terminal_copy_differs() {
    // The deployed route sent a terminal reasoning copy whose ID and encrypted content differed
    // from its `output_item.done` event; Scramjet's clients replay the done copy.
    let mut events = fixture::continuation_events("hé🙂", "{}", "opaque-done");
    let done: Vec<Value> = (0..4)
        .map(|index| {
            events
                .iter()
                .find(|e| e["type"] == "response.output_item.done" && e["output_index"] == index)
                .unwrap()["item"]
                .clone()
        })
        .collect();
    let terminal = &mut events.last_mut().unwrap()["response"]["output"][0];
    terminal["id"] = json!("rs_terminal_rewritten_to_a_much_longer_gateway_identifier");
    terminal["encrypted_content"] = json!("opaque-terminal");
    let items = parse(&fixture::wire(&events, false))
        .unwrap()
        .envelope
        .unwrap()
        .items;
    for (item, done) in items.iter().zip(&done) {
        assert_eq!(item.raw(), done);
    }

    let mut events = fixture::text_events("x", fixture::known_usage());
    events.insert(
        5,
        json!({"type":"response.output_text.annotation.added","output_index":0,"item_id":"msg_1"}),
    );
    events.last_mut().unwrap()["response"]["output"] = json!([]);
    assert_eq!(
        message_text(
            &parse(&fixture::wire(&events, false))
                .unwrap()
                .envelope
                .unwrap()
                .items[0]
        )
        .as_deref(),
        Some("x")
    );

    let mut events = fixture::call_events("{}", Value::Null);
    events.remove(4);
    assert_eq!(
        parse(&fixture::wire(&events, false)).err(),
        Some(Outcome::Incomplete)
    );
}
#[test]
fn a_response_stopped_at_its_output_limit_keeps_its_finished_items_unless_it_made_a_call() {
    let parsed = parse(&fixture::wire(
        &fixture::cut_events("At the edge", fixture::known_usage()),
        false,
    ))
    .unwrap();
    assert_eq!(parsed.status, "incomplete");
    assert!(complete(&parsed.usage.unwrap()));
    let items = parsed.envelope.unwrap().items;
    assert_eq!(items.len(), 1);
    assert_eq!(message_text(&items[0]).as_deref(), Some("At the edge"));
    assert_eq!(items[0].raw()["status"], "incomplete");

    // A reasoning item still running when the reply stopped is left out.
    let mut events = fixture::cut_events("x", Value::Null);
    events.insert(
        7,
        json!({"type":"response.output_item.added","output_index":1,
            "item":{"type":"reasoning","id":"rs_1","summary":[]}}),
    );
    let items = parse(&fixture::wire(&events, false))
        .unwrap()
        .envelope
        .unwrap()
        .items;
    assert_eq!(items.len(), 1);

    let mut other = fixture::cut_events("x", Value::Null);
    other[8]["response"]["incomplete_details"]["reason"] = json!("content_filter");
    let parsed = parse(&fixture::wire(&other, false)).unwrap();
    assert!(parsed.envelope.is_none());
    assert_eq!(parsed.error.as_deref(), Some("content_filter"));

    let mut call = fixture::call_events("{}", Value::Null);
    let terminal = call.last_mut().unwrap();
    terminal["type"] = json!("response.incomplete");
    terminal["response"]["status"] = json!("incomplete");
    terminal["response"]["incomplete_details"] = json!({"reason":"max_output_tokens"});
    assert!(
        parse(&fixture::wire(&call, false))
            .unwrap()
            .envelope
            .is_none()
    );

    // A completed response holds no item that stopped short.
    let mut contradictory = fixture::text_events("x", Value::Null);
    contradictory[7]["item"]["status"] = json!("incomplete");
    assert!(parse(&fixture::wire(&contradictory, false)).is_err());
}
#[test]
fn progress_reports_deltas_and_started_calls_only_when_enabled() {
    let bytes = fixture::wire(&fixture::continuation_events("hé🙂", "{}", "opaque"), false);
    let mut parser = ResponsesParser::new();
    parser.progress = Some(Vec::new());
    let mut collected = Vec::new();
    for chunk in bytes.chunks(7) {
        for byte in chunk {
            parser.byte(*byte).unwrap();
        }
        collected.append(parser.progress.as_mut().unwrap());
    }
    let text: String = collected
        .iter()
        .filter_map(|p| match p {
            StreamProgress::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    let reasoning: String = collected
        .iter()
        .filter_map(|p| match p {
            StreamProgress::Reasoning(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "hé🙂");
    assert_eq!(reasoning, "consider hé🙂\n\nconsider hé🙂\n\n");
    let calls: Vec<_> = collected
        .iter()
        .enumerate()
        .filter(|(_, p)| matches!(p, StreamProgress::Call(_)))
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, &StreamProgress::Call("inert".into()));
    assert!(matches!(
        collected[calls[0].0 - 1],
        StreamProgress::Reasoning(_)
    ));
    assert!(matches!(collected[calls[0].0 + 1], StreamProgress::Text(_)));
    assert!(parser.complete().unwrap().envelope.is_some());

    let mut quiet = ResponsesParser::new();
    for byte in &bytes {
        quiet.byte(*byte).unwrap();
    }
    assert!(quiet.progress.is_none());
}

/// A response's events between its `response.created` and its `response.completed`.
fn stream(middle: &[Value], output: Value) -> Vec<Value> {
    let mut events = vec![
        json!({"type":"response.created","response":{"id":"resp_1","status":"in_progress","output":[]}}),
    ];
    events.extend_from_slice(middle);
    events.push(
        json!({"type":"response.completed","response":{"id":"resp_1","status":"completed",
        "output":output,"usage":fixture::known_usage()}}),
    );
    events
}
fn with_progress(events: &[Value]) -> (Result<ParsedResponse, Outcome>, String, String) {
    let mut parser = ResponsesParser::new();
    parser.progress = Some(Vec::new());
    let read = fixture::wire(events, false)
        .into_iter()
        .try_for_each(|byte| parser.byte(byte));
    let (mut text, mut reasoning) = (String::new(), String::new());
    for update in parser.progress.take().unwrap() {
        match update {
            StreamProgress::Text(t) => text.push_str(&t),
            StreamProgress::Reasoning(t) => reasoning.push_str(&t),
            StreamProgress::Call(_) => {}
        }
    }
    (read.and_then(|()| parser.complete()), text, reasoning)
}
fn summary(item: &ProviderItem) -> String {
    (item.raw()["summary"].as_array().unwrap().iter())
        .map(|part| part["text"].as_str().unwrap())
        .collect()
}

#[test]
fn foundry_reasoning_summaries_parse_without_their_added_events_or_ids() {
    // The shapes a gateway's normalizer repairs in GPT-5.6's
    // raw streams from Foundry, here around a reasoning item and a message as they end.
    let reasoning = json!({"type":"reasoning","id":"rs_1","status":"completed",
        "summary":[{"type":"summary_text","text":"Plan it."}],"encrypted_content":"opaque"});
    let added = json!({"type":"response.output_item.added","output_index":0,
        "item":{"type":"reasoning","id":"rs_1","status":"in_progress","summary":[]}});
    let event = |kind: &str, fields: Value| {
        let mut event = json!({"type":format!("response.reasoning_summary_{kind}")});
        event
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        event
    };
    let ids = json!({"item_id":"rs_1","output_index":0,"summary_index":0});
    let with = |fields: Value| {
        let mut all = ids.clone();
        all.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        all
    };
    let part = json!({"type":"summary_text","text":""});
    let done_part = json!({"type":"summary_text","text":"Plan it."});
    let part_added = event("part.added", with(json!({"part":part})));
    let delta = |text: &str| event("text.delta", with(json!({"delta":text})));
    let text_done = event("text.done", with(json!({"text":"Plan it."})));
    let part_done = event("part.done", with(json!({"part":done_part})));
    let item_done = json!({"type":"response.output_item.done","output_index":0,"item":reasoning});
    let message: Vec<Value> = fixture::text_events("ok", Value::Null)[2..8]
        .iter()
        .map(|event| {
            let mut event = event.clone();
            event["output_index"] = json!(1);
            event
        })
        .collect();
    let output = json!([
        reasoning,
        fixture::text_events("ok", Value::Null)[7]["item"]
    ]);
    let bare = |kind: &str| json!({"type":format!("response.reasoning_summary_{kind}")});
    let cases: Vec<(&str, Vec<Value>)> = vec![
        (
            "compliant",
            vec![
                added.clone(),
                part_added.clone(),
                delta("Plan"),
                delta(" it."),
                text_done.clone(),
                part_done.clone(),
                item_done.clone(),
            ],
        ),
        (
            "summary before its item is added, without its part added",
            vec![
                delta("Plan"),
                delta(" it."),
                text_done.clone(),
                part_done.clone(),
                item_done.clone(),
            ],
        ),
        (
            "a summary's final text without its part added",
            vec![
                added.clone(),
                text_done.clone(),
                part_done.clone(),
                item_done.clone(),
            ],
        ),
        (
            "a summary part added without its item id",
            vec![
                added.clone(),
                event(
                    "part.added",
                    json!({"output_index":0,"summary_index":0,"part":part}),
                ),
                delta("Plan it."),
                text_done.clone(),
                part_done.clone(),
                item_done.clone(),
            ],
        ),
        (
            "a summary's final text without its item id",
            vec![
                added.clone(),
                part_added.clone(),
                delta("Plan it."),
                event(
                    "text.done",
                    json!({"output_index":0,"summary_index":0,"text":"Plan it."}),
                ),
                part_done.clone(),
                item_done.clone(),
            ],
        ),
        (
            "bare done events",
            vec![
                added.clone(),
                part_added.clone(),
                delta("Plan"),
                delta(" it."),
                bare("text.done"),
                bare("part.done"),
                item_done.clone(),
            ],
        ),
        (
            "a summary part added before its item, whose id comes later",
            vec![
                event(
                    "part.added",
                    json!({"output_index":0,"summary_index":0,"part":part}),
                ),
                added.clone(),
                delta("Plan it."),
                bare("text.done"),
                bare("part.done"),
                item_done.clone(),
            ],
        ),
        (
            "a done reasoning item never added",
            vec![delta("Plan it."), item_done.clone()],
        ),
        (
            "no done event: the terminal copy",
            vec![
                added.clone(),
                part_added.clone(),
                delta("Plan it."),
                text_done.clone(),
                part_done.clone(),
            ],
        ),
    ];
    for (name, middle) in cases {
        let events = stream(&[&middle[..], &message[..]].concat(), output.clone());
        let (parsed, text, shown) = with_progress(&events);
        let items = parsed
            .unwrap_or_else(|e| panic!("{name}: {e:?}"))
            .envelope
            .unwrap()
            .items;
        assert_eq!(items.len(), 2, "{name}");
        assert_eq!(items[0].raw(), &reasoning, "{name}");
        assert_eq!(message_text(&items[1]).as_deref(), Some("ok"), "{name}");
        assert_eq!(text, "ok", "{name}");
        assert!(shown.starts_with("Plan it."), "{name}: {shown:?}");
    }

    // A bare done event with two summaries open goes to the last one, as a gateway's normalizer
    // cannot tell; only what is shown can differ, as each done copy is kept.
    let second = json!({"type":"reasoning","id":"rs_2","status":"completed",
        "summary":[{"type":"summary_text","text":"Then this."}],"encrypted_content":"opaque-2"});
    let mut added_second = added.clone();
    added_second["output_index"] = json!(1);
    added_second["item"]["id"] = json!("rs_2");
    let mut second_delta = delta("Then this.");
    second_delta["item_id"] = json!("rs_2");
    second_delta["output_index"] = json!(1);
    let mut second_done = item_done.clone();
    second_done["output_index"] = json!(1);
    second_done["item"] = second.clone();
    let events = stream(
        &[
            added.clone(),
            delta("Plan it."),
            added_second,
            second_delta,
            bare("text.done"),
            bare("part.done"),
            item_done.clone(),
            second_done,
        ],
        json!([reasoning, second]),
    );
    let items = parse(&fixture::wire(&events, false))
        .unwrap()
        .envelope
        .unwrap()
        .items;
    assert_eq!(summary(&items[0]), "Plan it.");
    assert_eq!(summary(&items[1]), "Then this.");

    // Reasoning with neither a done event nor a terminal copy is left out: a replay needs its
    // encrypted content. A message that streamed text without either fails, as does a call.
    let events = stream(
        &[
            &[added.clone(), part_added.clone(), delta("Plan it.")][..],
            &message[..],
        ]
        .concat(),
        json!([]),
    );
    let items = parse(&fixture::wire(&events, false))
        .unwrap()
        .envelope
        .unwrap()
        .items;
    assert_eq!(items.len(), 1);
    assert_eq!(message_text(&items[0]).as_deref(), Some("ok"));
    let events = stream(&message[..5], json!([]));
    assert_eq!(
        parse(&fixture::wire(&events, false)).err(),
        Some(Outcome::Incomplete)
    );
    // Nothing at all is an empty reply, as Scramjet's client takes it.
    let items = parse(&fixture::wire(&stream(&[], json!([])), false))
        .unwrap()
        .envelope
        .unwrap()
        .items;
    assert!(items.is_empty());
}

#[test]
fn calls_run_their_done_copy_without_an_arguments_done_event() {
    // Scramjet's package's live-shaped call: argument deltas, then the done copy.
    let call = json!({"type":"function_call","id":"fc_live","call_id":"call_live","name":"bash",
        "arguments":"{\"command\":\"ls\"}","status":"completed"});
    let mut added = call.clone();
    added["arguments"] = json!("");
    added["status"] = json!("in_progress");
    let added = json!({"type":"response.output_item.added","output_index":0,"item":added});
    let delta = |text: &str| {
        json!({"type":"response.function_call_arguments.delta","output_index":0,
            "item_id":"fc_live","delta":text})
    };
    let done =
        |item: &Value| json!({"type":"response.output_item.done","output_index":0,"item":item});
    let run = |middle: &[Value]| {
        parse(&fixture::wire(&stream(middle, json!([call])), false))
            .map(|parsed| parsed.envelope.unwrap().items[0].raw()["arguments"].clone())
    };
    let ls = json!("{\"command\":\"ls\"}");
    assert_eq!(
        run(&[
            added.clone(),
            delta("{\"command\":"),
            delta("\"ls\"}"),
            done(&call)
        ]),
        Ok(ls.clone())
    );
    // A done copy without arguments keeps the streamed ones; a done-only call without them
    // takes none, as in Scramjet's package.
    let mut empty = call.clone();
    empty["arguments"] = json!("");
    assert_eq!(
        run(&[added.clone(), delta("{\"command\":\"ls\"}"), done(&empty)]),
        Ok(ls.clone())
    );
    assert_eq!(run(&[done(&empty)]), Ok(json!("{}")));
    assert!(run(&[added.clone(), delta("{\"command\":"), done(&empty)]).is_err());
    // A call completed again runs once, unless it differs; two calls may not share a call id.
    assert_eq!(
        run(&[added.clone(), done(&call), done(&call)]),
        Ok(ls.clone())
    );
    let mut other = call.clone();
    other["arguments"] = json!("{\"command\":\"rm\"}");
    assert!(run(&[added.clone(), done(&call), done(&other)]).is_err());
    let mut twin = call.clone();
    twin["id"] = json!("fc_twin");
    let mut twin_done = done(&twin);
    twin_done["output_index"] = json!(1);
    assert!(run(&[done(&call), twin_done]).is_err());
    // A call whose arguments are cut short does not run.
    let events = stream(&[added, delta("{\"command\":")], json!([call]));
    assert_eq!(
        parse(&fixture::wire(&events, false)).err(),
        Some(Outcome::Incomplete)
    );
}

#[test]
fn benign_variations_scramjets_package_reads_are_read() {
    // From Scramjet's package's "tolerates benign ordering, duplicate, unknown and terminal
    // variations" test.
    let text = fixture::text_events("pong", fixture::known_usage());
    let reply = |events: &[Value]| {
        let (parsed, shown, _) = with_progress(events);
        let items = parsed
            .map_err(|e| format!("{e:?}"))?
            .envelope
            .unwrap()
            .items;
        Ok::<_, String>((
            items.iter().filter_map(message_text).collect::<String>(),
            shown,
        ))
    };
    let pong = Ok(("pong".to_owned(), "pong".to_owned()));
    let queued = json!({"type":"response.queued","response":{"id":"resp_1","status":"queued"}});
    assert_eq!(reply(&[&[queued][..], &text].concat()), pong);
    let search = json!({"type":"response.output_item.added","output_index":1,
        "item":{"type":"web_search_call","id":"ws_1","status":"in_progress"}});
    let search_done = json!({"type":"response.output_item.done","output_index":1,
        "item":{"type":"web_search_call","id":"ws_1","status":"completed"}});
    let mut events = text.clone();
    events.insert(2, search);
    events.insert(3, search_done);
    assert_eq!(reply(&events), pong);
    let mut events = text.clone();
    events.insert(
        4,
        json!({"type":"response.output_text.delta","output_index":0,
        "item_id":"msg_1","content_index":0,"delta":42}),
    );
    assert_eq!(reply(&events), pong);
    for fields in [
        &["content_index"][..],
        &["item_id", "output_index", "content_index"],
    ] {
        let mut events = text.clone();
        for event in &mut events[3..7] {
            for field in fields {
                event.as_object_mut().unwrap().remove(*field);
            }
        }
        assert_eq!(reply(&events), pong, "without {fields:?}");
    }
    let mut events = text.clone();
    events[7]["item"]["phase"] = json!("analysis");
    events[8]["response"]["output"][0]["phase"] = json!("analysis");
    let items = parse(&fixture::wire(&events, false))
        .unwrap()
        .envelope
        .unwrap()
        .items;
    assert_eq!(items[0].raw()["phase"], "analysis");
    // A message with no done event takes the terminal copy at its index, whatever its id.
    let mut events = text.clone();
    events.remove(7);
    events[7]["response"]["output"][0]["id"] = json!("msg_rewritten");
    assert_eq!(reply(&events), pong);
    // A text sent only in its done event is shown from there.
    let mut events = text.clone();
    events.remove(4);
    assert_eq!(reply(&events), pong);
}
