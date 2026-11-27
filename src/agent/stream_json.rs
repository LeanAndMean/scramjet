//! A conversation's items as Claude Code's `-p --output-format stream-json` prints its
//! messages: one line per content block of a reply, and one per tool result.
use super::{FAILED, PROBE, PRODUCER, message_text};
use serde_json::{Value, json};

/// The line for `item`, newly added to the conversation of session `session_id`; `None` for a
/// prompt, a probe or anything else Claude Code does not print.
pub fn message(item: &Value, session_id: &str) -> Option<Value> {
    let assistant = |block: Value| {
        let mut message = json!({
            "type": "message",
            "role": "assistant",
            "model": item[PRODUCER],
            "content": [block],
        });
        if let Some(id) = item["id"].as_str() {
            message["id"] = id.into();
        }
        json!({
            "type": "assistant",
            "message": message,
            "parent_tool_use_id": null,
            "session_id": session_id,
        })
    };
    if item.get(PROBE).is_some() {
        return None;
    }
    match item["type"].as_str()? {
        "message" if item["role"] != "user" => {
            let text = message_text(item);
            (!text.is_empty()).then(|| assistant(json!({"type": "text", "text": text})))
        }
        "reasoning" => {
            let thinking: String = (item["summary"].as_array().into_iter().flatten())
                .filter_map(|part| part["text"].as_str())
                .collect();
            (!thinking.is_empty())
                .then(|| assistant(json!({"type": "thinking", "thinking": thinking})))
        }
        "function_call" => {
            let arguments = item["arguments"].as_str().unwrap_or_default();
            let input = serde_json::from_str(arguments).unwrap_or_else(|_| json!(arguments));
            Some(assistant(json!({
                "type": "tool_use",
                "id": item["call_id"],
                "name": item["name"],
                "input": input,
            })))
        }
        "function_call_output" => {
            let content = match &item["output"] {
                Value::String(text) => text.clone(),
                parts => (parts.as_array().into_iter().flatten())
                    .filter_map(|part| part["text"].as_str())
                    .collect(),
            };
            Some(json!({
                "type": "user",
                "message": {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": item["call_id"],
                        "content": content,
                        "is_error": item.get(FAILED).is_some(),
                    }],
                },
                "parent_tool_use_id": null,
                "session_id": session_id,
            }))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_and_results_print_as_claude_codes_messages() {
        let text = json!({"type":"message","role":"assistant","id":"m1",PRODUCER:"p/m",
            "content":[{"type":"output_text","text":"Hi."}]});
        let line = message(&text, "s").unwrap();
        assert_eq!(line["type"], "assistant");
        assert_eq!(line["session_id"], "s");
        assert_eq!(
            (&line["message"]["id"], &line["message"]["model"]),
            (&json!("m1"), &json!("p/m"))
        );
        assert_eq!(
            line["message"]["content"][0],
            json!({"type":"text","text":"Hi."})
        );
        let call = json!({"type":"function_call","call_id":"c1","name":"bash",
            "arguments":"{\"command\":\"ls\"}"});
        let line = message(&call, "s").unwrap();
        assert!(line["message"].get("id").is_none(), "{line}");
        let block = &line["message"]["content"][0];
        assert_eq!(
            *block,
            json!({"type":"tool_use","id":"c1","name":"bash","input":{"command":"ls"}})
        );
        let failed =
            json!({"type":"function_call_output","call_id":"c1","output":"no",FAILED:true});
        let line = message(&failed, "s").unwrap();
        assert_eq!(line["type"], "user");
        assert_eq!(
            line["message"]["content"][0],
            json!({"type":"tool_result","tool_use_id":"c1","content":"no","is_error":true})
        );
        let parts = json!({"type":"function_call_output","call_id":"c2",
            "output":[{"type":"input_text","text":"a"},{"type":"input_image","image_url":"data:"}]});
        let block = &message(&parts, "s").unwrap()["message"]["content"][0];
        assert_eq!(
            (&block["content"], &block["is_error"]),
            (&json!("a"), &json!(false))
        );
        let thought = json!({"type":"reasoning","summary":[{"type":"summary_text","text":"Hm."}]});
        let block = &message(&thought, "s").unwrap()["message"]["content"][0];
        assert_eq!(*block, json!({"type":"thinking","thinking":"Hm."}));
        // Prompts, probes and empty reasoning are not printed.
        for item in [
            json!({"role":"user","content":"hi"}),
            json!({"role":"user","content":"note",PROBE:true}),
            json!({"type":"reasoning","summary":[],"encrypted_content":"x"}),
        ] {
            assert_eq!(message(&item, "s"), None, "{item}");
        }
    }
}
