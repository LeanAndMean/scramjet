use serde_json::{Value, json};

pub fn text_events(text: &str, usage: Value) -> Vec<Value> {
    let item = json!({"type":"message","id":"msg_1","role":"assistant","status":"completed",
        "content":[{"type":"output_text","text":text,"annotations":[]}]});
    vec![
        json!({"type":"response.created","response":{"id":"resp_1","status":"in_progress","output":[]}}),
        json!({"type":"response.in_progress","response":{"id":"resp_1","status":"in_progress","output":[]}}),
        json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg_1","role":"assistant","status":"in_progress","content":[]}}),
        json!({"type":"response.content_part.added","output_index":0,"item_id":"msg_1","content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}),
        json!({"type":"response.output_text.delta","output_index":0,"item_id":"msg_1","content_index":0,"delta":text}),
        json!({"type":"response.output_text.done","output_index":0,"item_id":"msg_1","content_index":0,"text":text}),
        json!({"type":"response.content_part.done","output_index":0,"item_id":"msg_1","content_index":0,"part":{"type":"output_text","text":text,"annotations":[]}}),
        json!({"type":"response.output_item.done","output_index":0,"item":item}),
        json!({"type":"response.completed","response":{"id":"resp_1","status":"completed","model":"gpt-6-luna","output":[item],"usage":usage}}),
    ]
}
pub fn call_events(arguments: &str, usage: Value) -> Vec<Value> {
    let item = json!({"type":"function_call","id":"fc_1","call_id":"call_1","name":"inert","status":"completed","arguments":arguments});
    vec![
        json!({"type":"response.created","response":{"id":"resp_1","status":"in_progress","output":[]}}),
        json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"inert","status":"in_progress","arguments":""}}),
        json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":"fc_1","delta":arguments}),
        json!({"type":"response.function_call_arguments.done","output_index":0,"item_id":"fc_1","arguments":arguments}),
        json!({"type":"response.output_item.done","output_index":0,"item":item}),
        json!({"type":"response.completed","response":{"id":"resp_1","status":"completed","model":"gpt-6-luna","output":[item],"usage":usage}}),
    ]
}
#[allow(dead_code)]
pub fn disagreement_events() -> Vec<Value> {
    let mut events = text_events("synthetic-private\u{001b}hé🙂", known_usage());
    let item = &mut events.last_mut().unwrap()["response"]["output"][0];
    item["phase"] = json!("final_answer");
    item["content"][0]["logprobs"] = json!([{"token":"synthetic-private"}]);
    item["synthetic-private\u{001b}key"] = json!({"nested":"synthetic-private"});
    events
}
pub fn known_usage() -> Value {
    json!({"input_tokens":10,"output_tokens":5,"total_tokens":15,
        "input_tokens_details":{"cached_tokens":3},"output_tokens_details":{"reasoning_tokens":2}})
}
pub fn wire(events: &[Value], crlf: bool) -> Vec<u8> {
    let newline = if crlf { "\r\n" } else { "\n" };
    let mut wire = String::new();
    for (sequence, event) in events.iter().enumerate() {
        let mut event = event.clone();
        event["sequence_number"] = json!(sequence);
        wire.push_str(&format!(
            "event: {}{newline}data: {}{newline}{newline}",
            event["type"].as_str().unwrap(),
            event
        ));
    }
    wire.into_bytes()
}
