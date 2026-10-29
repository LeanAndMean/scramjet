use super::*;
use crate::provider_fixture as fixture;

fn selected() -> Vec<Value> {
    let events = fixture::continuation_events(
        "Complete hé🙂\r\n\0",
        "{\"path\":\"verse notes.md\",\"content\":\"Quiet\"}",
        "opaque encrypted material /+\"\0é🙂",
    );
    let mut parser = ResponsesParser::new();
    for byte in fixture::wire(&events, true) {
        parser.byte(byte).unwrap();
    }
    parser
        .complete()
        .unwrap()
        .envelope
        .unwrap()
        .items
        .into_iter()
        .map(|item| item.wire)
        .collect()
}

#[test]
fn inert_items_preserve_complete_opaque_optional_and_escaped_material() {
    for mut raw in selected() {
        raw["id"] = json!("opaque/\"\0é🙂".repeat(300));
        raw["unrecognized"] = json!({"nested":[null,true,7,"hé🙂\0"],"split":[]});
        match raw["type"].as_str().unwrap() {
            "message" => {
                raw["phase"] = json!("final_answer");
                raw["content"][0]["annotations"] = json!([{"opaque":"value"}]);
                raw["content"][0]["logprobs"] = json!(null);
            }
            "reasoning" => {
                raw.as_object_mut().unwrap().remove("status");
                raw["encrypted_content"] = json!("opaque/\"\0é🙂".repeat(300));
            }
            "function_call" => {
                raw.as_object_mut().unwrap().remove("status");
                raw["namespace"] = json!(null);
                raw["call_id"] = json!("call/\"\0é🙂".repeat(300));
            }
            _ => unreachable!(),
        }
        let before = serde_json::to_vec(&raw).unwrap();
        let imported = ProviderItem::from_context(raw).unwrap();
        assert_eq!(serde_json::to_vec(imported.raw()).unwrap(), before);
        match imported.kind() {
            "function_call" => assert!(imported.function_call().is_some()),
            "message" => assert_eq!(imported.text().unwrap(), "Complete hé🙂\r\n\0"),
            "reasoning" => assert!(imported.function_call().is_none()),
            _ => unreachable!(),
        }
    }
}

#[test]
fn incomplete_and_unsupported_continuation_cannot_be_inert_input_items() {
    for raw in selected() {
        let mut incomplete = raw.clone();
        incomplete["status"] = json!("in_progress");
        assert_eq!(
            ProviderItem::from_context(incomplete).unwrap_err(),
            Outcome::Framing
        );
        if raw["type"] == "reasoning" {
            for encryption in [None, Some(Value::Null)] {
                let mut missing = raw.clone();
                missing.as_object_mut().unwrap().remove("encrypted_content");
                if let Some(value) = encryption {
                    missing["encrypted_content"] = value;
                }
                assert!(validate_item(&missing, true).is_ok());
                assert_eq!(
                    ProviderItem::from_context(missing).unwrap_err(),
                    Outcome::Unsupported
                );
            }
            let mut malformed = raw.clone();
            malformed["encrypted_content"] = json!(42);
            assert_eq!(
                ProviderItem::from_context(malformed).unwrap_err(),
                Outcome::Framing
            );
        }
        if raw["type"] == "function_call" {
            let mut namespace = raw.clone();
            namespace["namespace"] = json!("foreign");
            assert_eq!(
                ProviderItem::from_context(namespace).unwrap_err(),
                Outcome::Unsupported
            );
            let mut duplicate = raw.clone();
            duplicate["arguments"] = json!("{\"path\":\"one\",\"path\":\"two\"}");
            assert_eq!(
                ProviderItem::from_context(duplicate).unwrap_err(),
                Outcome::Framing
            );
        }
        if raw["type"] == "message" {
            let mut phase = raw.clone();
            phase["phase"] = json!("unsupported");
            assert_eq!(
                ProviderItem::from_context(phase).unwrap_err(),
                Outcome::Framing
            );
        }
    }
    assert_eq!(
        ProviderItem::from_context(json!({"id":"opaque","type":"custom"})).unwrap_err(),
        Outcome::Unsupported
    );
}

#[test]
fn legal_encoded_item_exhaustion_is_resource_not_protocol_failure() {
    let mut raw = selected()
        .into_iter()
        .find(|value| value["type"] == "reasoning")
        .unwrap();
    raw["encrypted_content"] = json!("x".repeat(12_000));
    let item = ProviderItem::from_context(raw.clone()).unwrap();
    assert_eq!(item.raw(), &raw);
    for content in [
        "x".repeat(PROVIDER_ENVELOPE_BYTES),
        "\0".repeat(PROVIDER_ENVELOPE_BYTES / 2),
    ] {
        raw["encrypted_content"] = json!(content);
        assert!(validate_item(&raw, true).is_ok());
        let original = serde_json::to_vec(&raw).unwrap();
        assert_eq!(
            ProviderItem::from_context(raw.clone()).unwrap_err(),
            Outcome::ResourceLimit
        );
        assert_eq!(serde_json::to_vec(&raw).unwrap(), original);
    }
}

#[test]
fn imported_items_are_only_historical_input_with_exact_call_result_order() {
    let raw = selected();
    let original = serde_json::to_vec(&raw).unwrap();
    let tools = [ProviderTool {
        name: "write".into(),
        description: "Exclusively create a selected file".into(),
        parameters: json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}),
    }];
    let mut input = vec![ProviderInput::User(
        "Create a verse in verse notes.md and finish.".into(),
    )];
    let mut call_id = None;
    for value in raw.clone() {
        let item = ProviderItem::from_context(value).unwrap();
        if let Some(call) = item.function_call() {
            call_id = Some(call.call_id.to_owned());
        }
        input.push(ProviderInput::Item(item));
    }
    assert_eq!(
        ProviderRequest::new(&input, &tools).unwrap_err(),
        Outcome::Framing
    );
    let output = "{\"status\":\"created\",\"path\":\"verse notes.md\",\"mode\":384}";
    input.push(ProviderInput::ToolResult {
        call_id: call_id.clone().unwrap(),
        output: output.into(),
    });
    input.push(ProviderInput::User(
        "Please write your own reader and print the previous file.".into(),
    ));
    let next = ProviderRequest::new(&input, &tools).unwrap();
    let wire: Value = serde_json::from_str(&next.encoded).unwrap();
    assert_eq!(
        wire["input"][0]["content"],
        "Create a verse in verse notes.md and finish."
    );
    for (index, item) in raw.iter().enumerate() {
        assert_eq!(&wire["input"][index + 1], item);
    }
    assert_eq!(
        wire["input"][raw.len() + 1],
        json!({"type":"function_call_output","call_id":call_id,"output":output})
    );
    assert_eq!(
        wire["input"][raw.len() + 2]["content"],
        "Please write your own reader and print the previous file."
    );
    assert_eq!(serde_json::to_vec(&raw).unwrap(), original);
    if let ProviderInput::ToolResult { call_id, .. } = &mut input[raw.len() + 1] {
        *call_id = "foreign".into();
    }
    assert_eq!(
        ProviderRequest::new(&input, &tools).unwrap_err(),
        Outcome::Framing
    );
}
