use super::responses;
use scramjet_rs::{HAIKU_COMMAND, HAIKU_SCRIPT, HaikuProcessResult, haiku_prompt, haiku_tools};
use serde_json::{Value, json};
fn rename(value: &mut Value, name: &str) {
    match value {
        Value::Object(map) => {
            if map.get("name").is_some_and(|value| value == "inert") {
                map.insert("name".into(), json!(name));
            }
            for value in map.values_mut() {
                rename(value, name);
            }
        }
        Value::Array(values) => {
            for value in values {
                rename(value, name);
            }
        }
        _ => {}
    }
}
pub fn sequence(poem: &str, extra_reads: usize, reasoning: &str) -> Vec<(Option<Value>, Vec<u8>)> {
    let mut steps = vec![
        (
            "write",
            json!({"path":"haiku.md","content":poem}),
            json!({"status":"created","path":"haiku.md","mode":384}).to_string(),
        ),
        (
            "read",
            json!({"path":"haiku.md"}),
            json!({"status":"read","path":"haiku.md","content":poem}).to_string(),
        ),
        (
            "write",
            json!({"path":"print_haiku.py","content":HAIKU_SCRIPT}),
            json!({"status":"created","path":"print_haiku.py","mode":384}).to_string(),
        ),
    ];
    for index in 0..extra_reads {
        let (path, content) = if index % 2 == 0 {
            ("print_haiku.py", HAIKU_SCRIPT)
        } else {
            ("haiku.md", poem)
        };
        steps.push((
            "read",
            json!({"path":path}),
            json!({"status":"read","path":path,"content":content}).to_string(),
        ));
    }
    steps.push((
        "bash",
        json!({"command":HAIKU_COMMAND}),
        serde_json::to_string(&HaikuProcessResult {
            exit_code: Some(0),
            signal: None,
            stdout: poem.into(),
            stderr: String::new(),
            stopped: None,
            launcher_reaped: true,
            namespace_dead: true,
        })
        .unwrap(),
    ));
    steps.push(("", json!({}), String::new()));
    let tools: Vec<_> = haiku_tools().into_iter().map(|tool| json!({"type":"function","name":tool.name,"description":tool.description,"parameters":tool.parameters,"strict":true})).collect();
    let mut input = vec![json!({"role":"user","content":haiku_prompt()})];
    let mut sequence = Vec::new();
    for (index, (name, args, output)) in steps.into_iter().enumerate() {
        let request = json!({"model":"gpt-6-luna","input":input,"tools":tools,"stream":true,"store":false,"include":["reasoning.encrypted_content"],"max_output_tokens":1024,"parallel_tool_calls":false});
        let mut events = if name.is_empty() {
            responses::text_events("haiku journey complete", responses::known_usage())
        } else {
            responses::continuation_events("working hé🙂", &args.to_string(), reasoning)
        };
        for event in &mut events {
            rename(event, name);
        }
        responses::qualify_events(&mut events, index + 1);
        input.extend(
            events.last().unwrap()["response"]["output"]
                .as_array()
                .unwrap()
                .iter()
                .cloned(),
        );
        if !name.is_empty() {
            input.push(json!({"type":"function_call_output","call_id":format!("call_1.{}", index + 1),"output":output}));
        }
        sequence.push((Some(request), responses::wire(&events, index % 2 == 0)));
    }
    sequence
}
