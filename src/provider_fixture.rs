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
/// A reply stopped at its output limit, as a gateway's GPT routes send it: the message done as
/// `incomplete`, then `response.incomplete`.
pub fn cut_events(text: &str, usage: Value) -> Vec<Value> {
    let mut events = text_events(text, usage);
    events[7]["item"]["status"] = json!("incomplete");
    let terminal = &mut events[8];
    terminal["type"] = json!("response.incomplete");
    terminal["response"]["status"] = json!("incomplete");
    terminal["response"]["incomplete_details"] = json!({"reason":"max_output_tokens"});
    terminal["response"]["output"][0]["status"] = json!("incomplete");
    events
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
pub fn continuation_events(text: &str, arguments: &str, encrypted: &str) -> Vec<Value> {
    let call = call_events(arguments, known_usage());
    let message = text_events(text, known_usage());
    let mut events = vec![call[0].clone()];
    let mut output = Vec::new();
    for index in 0..2 {
        let id = format!("rs_{index}");
        let done = json!({"type":"reasoning","id":id,"status":"completed",
            "summary":[{"type":"summary_text","text":"consider hé🙂"}],"encrypted_content":encrypted});
        events.push(
            json!({"type":"response.output_item.added","output_index":index,
            "item":{"type":"reasoning","id":id,"summary":[]}}),
        );
        events.push(json!({"type":"response.reasoning_summary_part.added","output_index":index,"item_id":id,"summary_index":0,"part":{"type":"summary_text","text":""}}));
        events.push(json!({"type":"response.reasoning_summary_text.delta","output_index":index,"item_id":id,"summary_index":0,"delta":"consider hé🙂"}));
        events.push(json!({"type":"response.reasoning_summary_text.done","output_index":index,"item_id":id,"summary_index":0,"text":"consider hé🙂"}));
        events.push(json!({"type":"response.reasoning_summary_part.done","output_index":index,"item_id":id,"summary_index":0,"part":done["summary"][0]}));
        output.push(done);
    }
    for event in &call[1..5] {
        let mut event = event.clone();
        event["output_index"] = json!(2);
        events.push(event);
    }
    for event in &message[2..8] {
        let mut event = event.clone();
        event["output_index"] = json!(3);
        if event["type"] == "response.output_item.done" {
            event["item"]["phase"] = json!("final_answer");
        }
        events.push(event);
    }
    for index in [1, 0] {
        events.push(
            json!({"type":"response.output_item.done","output_index":index,"item":output[index]}),
        );
    }
    output.push(call[4]["item"].clone());
    let mut message = message[7]["item"].clone();
    message["phase"] = json!("final_answer");
    output.push(message);
    events.push(json!({"type":"response.completed","response":{"id":"resp_1","status":"completed","output":output,"usage":known_usage()}}));
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
