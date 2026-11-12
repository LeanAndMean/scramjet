use super::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::Arc;

type Answer = dyn Fn(&str, &str, &Value) -> (u16, Value) + Send + Sync;

/// A server answering each request, by method, path and JSON body, with `answer`'s status and
/// JSON; its address.
fn serve(answer: impl Fn(&str, &str, &Value) -> (u16, Value) + Send + Sync + 'static) -> String {
    let answer: Arc<Answer> = Arc::new(answer);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            let mut length = 0;
            let mut authorization = Value::Null;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim().is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap();
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("authorization")
                {
                    authorization = value.trim().into();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let mut body = serde_json::from_slice(&body).unwrap_or(Value::Null);
            // A GET has no body: the scripted answer sees its authorization there.
            if body.is_null() {
                body = json!({"authorization": authorization});
            }
            let mut words = request.split(' ');
            let (method, path) = (words.next().unwrap(), words.next().unwrap());
            let (status, json) = answer(method, path, &body);
            let json = json.to_string();
            let reply = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{json}",
                json.len()
            );
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    format!("http://{address}")
}

fn not_found() -> (u16, Value) {
    (404, json!({"error": "not found"}))
}

/// Ollama 0.40's answers, trimmed: a chat model with tools, loaded; one without tools; an
/// embedder.
fn ollama(_: &str, path: &str, _: &Value) -> (u16, Value) {
    match path {
        "/v1/models" => (
            200,
            json!({"object": "list", "data": [
            {"id": "qwen3:1.7b", "owned_by": "library"},
            {"id": "gemma:2b", "owned_by": "library"},
            {"id": "nomic-embed-text:latest", "owned_by": "library"}]}),
        ),
        "/api/version" => (200, json!({"version": "0.40.16"})),
        "/api/tags" => (
            200,
            json!({"models": [
            {"name": "qwen3:1.7b", "model": "qwen3:1.7b", "details": {"context_length": 40960},
                "capabilities": ["completion", "tools", "thinking"]},
            {"name": "gemma:2b", "model": "gemma:2b", "details": {"context_length": 8192},
                "capabilities": ["completion", "vision"]},
            {"name": "nomic-embed-text:latest", "model": "nomic-embed-text:latest",
                "details": {"context_length": 2048}, "capabilities": ["embedding"]}]}),
        ),
        "/api/ps" => (
            200,
            json!({"models": [
            {"name": "qwen3:1.7b", "model": "qwen3:1.7b", "context_length": 32768}]}),
        ),
        _ => not_found(),
    }
}

#[test]
fn ollama_loads_a_model_to_say_its_window() {
    let generated = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = generated.clone();
    let url = serve(move |method, path, body| match (method, path) {
        ("POST", "/api/generate") => {
            seen.lock().unwrap().push(body.clone());
            (
                200,
                json!({"model": body["model"], "done": true, "done_reason": "load"}),
            )
        }
        ("GET", "/api/ps") if !seen.lock().unwrap().is_empty() => (
            200,
            json!({"models": [{"name": "gemma:2b", "model": "gemma:2b", "context_length": 4096}]}),
        ),
        _ => ollama(method, path, body),
    });
    let go = AtomicBool::new(false);
    assert_eq!(window_now(&url, None, "qwen3:1.7b", &go), Some(32768));
    assert!(
        generated.lock().unwrap().is_empty(),
        "a loaded model is not loaded again"
    );
    assert_eq!(window_now(&url, None, "gemma:2b", &go), Some(4096));
    let sent = generated.lock().unwrap().clone();
    assert_eq!(sent, [json!({"model": "gemma:2b", "stream": false})]);

    let slow = serve(|method, path, body| match path {
        "/api/generate" => {
            std::thread::sleep(Duration::from_secs(5));
            (200, json!({"done": true}))
        }
        _ => ollama(method, path, body),
    });
    let started = std::time::Instant::now();
    assert_eq!(
        window_now(&slow, None, "gemma:2b", &AtomicBool::new(true)),
        None
    );
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "a cancelled load is not waited for"
    );
}

#[test]
fn an_address_is_taken_as_typed_without_v1() {
    assert_eq!(
        address("localhost:11434").unwrap(),
        "http://localhost:11434"
    );
    assert_eq!(
        address(" http://10.0.0.2:8000/v1/ ").unwrap(),
        "http://10.0.0.2:8000"
    );
    assert_eq!(
        address("https://gpu.example/vllm").unwrap(),
        "https://gpu.example/vllm"
    );
    assert!(address("").is_err());
    assert!(address("ftp://x").is_err());
}

#[test]
fn ollama_is_told_apart_and_its_models_described() {
    let url = serve(ollama);
    let server = probe(&url, None).unwrap();
    assert_eq!(server.runtime, Runtime::Ollama);
    assert_eq!(server.version.as_deref(), Some("0.40.16"));
    let qwen = &server.models[0];
    assert_eq!(
        (qwen.id.as_str(), qwen.window, qwen.trained, qwen.reasoning),
        ("qwen3:1.7b", Some(32768), Some(40960), true)
    );
    assert_eq!((&qwen.unfit, qwen.switch), (&None, Some(Switch::Effort)));
    let gemma = &server.models[1];
    assert_eq!(gemma.switch, None);
    // Not loaded: Ollama has not chosen its window yet.
    assert_eq!((gemma.window, gemma.images), (None, true));
    assert_eq!(
        gemma.unfit.as_deref(),
        Some("Ollama says its template takes no tools")
    );
    assert_eq!(
        server.models[2].unfit.as_deref(),
        Some("Ollama says it does not chat")
    );
}

#[test]
fn llama_cpp_and_vllm_say_their_windows() {
    let llama = serve(|_, path, _| match path {
        "/v1/models" => (
            200,
            json!({"data": [{"id": "qwen3-1.7b", "owned_by": "llamacpp",
            "meta": {"n_ctx": 16384, "n_ctx_train": 40960}}]}),
        ),
        "/props" => (
            200,
            json!({"build_info": "b12282-aa75ff323",
            "default_generation_settings": {"n_ctx": 16384},
            "chat_template_caps": {"supports_tool_calls": false}, "modalities": {"vision": false}}),
        ),
        _ => not_found(),
    });
    let server = probe(&llama, None).unwrap();
    assert_eq!(server.runtime, Runtime::LlamaCpp);
    assert_eq!(server.version.as_deref(), Some("b12282-aa75ff323"));
    let model = &server.models[0];
    assert_eq!((model.window, model.trained), (Some(16384), Some(40960)));
    assert_eq!(
        model.unfit.as_deref(),
        Some("its chat template takes no tools")
    );
    assert!(!model.effort_in_template);
    // gpt-oss's template, as b12282 describes it.
    let gpt_oss = serve(|_, path, _| match path {
        "/v1/models" => (
            200,
            json!({"data": [{"id": "gpt-oss-20b", "owned_by": "llamacpp"}]}),
        ),
        "/props" => (
            200,
            json!({"build_info": "b12282-aa75ff323",
            "chat_template_caps": {"supports_tool_calls": true, "supports_reasoning_effort": true,
                "reasoning_effort_levels": ["minimal", "low", "medium", "high", "xhigh", "max"]}}),
        ),
        _ => not_found(),
    });
    assert!(probe(&gpt_oss, None).unwrap().models[0].effort_in_template);

    let vllm = serve(|_, path, _| match path {
        "/v1/models" => (
            200,
            json!({"data": [{"id": "Qwen/Qwen3-0.6B", "owned_by": "vllm",
            "max_model_len": 40960}]}),
        ),
        "/version" => (200, json!({"version": "0.13.0"})),
        _ => not_found(),
    });
    let server = probe(&vllm, None).unwrap();
    assert_eq!(server.runtime, Runtime::Vllm);
    assert_eq!(server.models[0].window, Some(40960));
    assert!(!server.models[0].effort_in_template);
    // Served under another name, gpt-oss is told by its weights' path.
    let gpt_oss = serve(|_, path, _| match path {
        "/v1/models" => (
            200,
            json!({"data": [{"id": "gpt", "owned_by": "vllm", "max_model_len": 32768,
            "root": "/models/OpenAI/GPT-OSS-20B"}]}),
        ),
        _ => not_found(),
    });
    assert!(probe(&gpt_oss, None).unwrap().models[0].effort_in_template);

    let other = serve(|_, path, _| match path {
        "/v1/models" => (200, json!({"data": [{"id": "m"}]})),
        _ => not_found(),
    });
    let server = probe(&other, None).unwrap();
    assert_eq!(
        (server.runtime, server.models[0].window),
        (Runtime::Other, None)
    );
}

/// LM Studio's model lists (llmster 0.0.27, trimmed): its chat model loaded on the first
/// request with an 8192-token window, an embedder, both answering `/v1/models` alike.
fn lm_studio(loaded: bool) -> impl Fn(&str, &str, &Value) -> (u16, Value) {
    move |_, path, body| {
        let instances = match loaded {
            true => json!([{"id": "qwen3-1.7b", "config": {"context_length": 8192}}]),
            false => json!([]),
        };
        match path {
            "/v1/models" => (
                200,
                json!({"object": "list", "data": [
                {"id": "qwen3-1.7b", "owned_by": "organization_owner"},
                {"id": "text-embedding-nomic-embed-text-v1.5", "owned_by": "organization_owner"}]}),
            ),
            "/api/version" => (
                200,
                json!({"error": "Unexpected endpoint or method. (GET /api/version)"}),
            ),
            "/api/v1/models" => (
                200,
                json!({"models": [
                {"type": "llm", "key": "qwen3-1.7b", "loaded_instances": instances,
                    "max_context_length": 40960, "capabilities": {"vision": false,
                    "trained_for_tool_use": true,
                    "reasoning": {"allowed_options": ["off", "on"], "default": "on"}}},
                {"type": "embedding", "key": "text-embedding-nomic-embed-text-v1.5",
                    "loaded_instances": [], "max_context_length": 2048},
                {"type": "llm", "key": "openai/gpt-oss-20b", "loaded_instances": [],
                    "max_context_length": 131072, "capabilities": {"vision": false,
                    "trained_for_tool_use": true,
                    "reasoning": {"allowed_options": ["low", "medium", "high"],
                    "default": "low"}}}]}),
            ),
            // gpt-oss reasons at every effort, and called the tool without reasoning first.
            "/v1/chat/completions" if body["model"] == "openai/gpt-oss-20b" => {
                let message = match body["tools"].is_array() {
                    true => json!({"content": "", "reasoning": "",
                        "tool_calls": [{"function": {"name": CHECK_TOOL,
                        "arguments": "{\"word\":\"scramjet\"}"}}]}),
                    false => json!({"content": "391", "reasoning": "Just multiply."}),
                };
                (200, json!({"choices": [{"message": message}]}))
            }
            "/v1/chat/completions" => {
                let message = match body["reasoning_effort"] == "none" {
                    true => json!({"content": "391"}),
                    false => json!({"content": "", "reasoning_content": "Call it.",
                        "tool_calls": [{"function": {"name": CHECK_TOOL,
                        "arguments": "{\"word\":\"scramjet\"}"}}]}),
                };
                (200, json!({"choices": [{"message": message}]}))
            }
            _ => not_found(),
        }
    }
}

#[test]
fn lm_studio_is_told_apart_and_its_window_read_once_loaded() {
    let url = serve(lm_studio(false));
    let server = probe(&url, None).unwrap();
    assert_eq!((server.runtime, server.version), (Runtime::LmStudio, None));
    let chat = &server.models[0];
    assert_eq!(
        (chat.window, chat.trained, chat.reasoning),
        (None, Some(40960), true)
    );
    assert_eq!(chat.unfit, None);
    let embedder = server.models[1].unfit.as_deref();
    assert_eq!(embedder, Some("LM Studio says it does not chat"));

    let loaded = serve(lm_studio(true));
    assert_eq!(
        window_now(&loaded, None, "qwen3-1.7b", &AtomicBool::new(false)),
        Some(8192)
    );
    let server = probe(&loaded, None).unwrap();
    let checked = check_tools(&server, "qwen3-1.7b", None).unwrap();
    assert_eq!(checked.window, Some(8192));
    assert_eq!(
        (checked.thinks, checked.switch),
        (true, Some(Switch::Effort))
    );
    let checked = check_tools(&server, "openai/gpt-oss-20b", None).unwrap();
    assert_eq!(
        (checked.thinks, checked.switch),
        (true, Some(Switch::EffortNoOff))
    );

    // Before 0.4.0, only `/api/v0/models`.
    let v0 = serve(|_, path, _| match path {
        "/v1/models" => (200, json!({"data": [{"id": "qwen2-vl-7b-instruct"}]})),
        "/api/v0/models" => (
            200,
            json!({"object": "list", "data": [
            {"id": "qwen2-vl-7b-instruct", "type": "vlm", "state": "loaded",
                "max_context_length": 32768, "loaded_context_length": 4096},
            {"id": "nomic", "type": "embeddings", "state": "not-loaded"}]}),
        ),
        _ => not_found(),
    });
    let server = probe(&v0, None).unwrap();
    assert_eq!(server.runtime, Runtime::LmStudio);
    let vision = &server.models[0];
    assert_eq!(
        (vision.window, vision.trained, vision.images),
        (Some(4096), Some(32768), true)
    );
    assert!(server.models[1].unfit.is_some());
}

#[test]
fn each_failing_step_is_named() {
    let keyed = serve(|_, _, _| (401, json!({"error": "no key"})));
    assert_eq!(probe(&keyed, None).unwrap_err().step, Step::Key);
    let empty = serve(|_, path, _| match path {
        "/v1/models" => (200, json!({"data": []})),
        _ => not_found(),
    });
    let failed = probe(&empty, None).unwrap_err();
    assert_eq!(failed.step, Step::NoModels);
    assert_eq!(
        failed.to_string(),
        format!("no models: OpenAI-compatible server at {empty} lists none")
    );
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    assert_eq!(probe(&url, None).unwrap_err().step, Step::NotReachable);
    let web = serve(|_, _, _| not_found());
    let failed = probe(&web, None).unwrap_err();
    assert!(
        failed.detail.contains("not an OpenAI-compatible server"),
        "{failed}"
    );
}

#[test]
fn a_key_given_goes_as_a_bearer_token() {
    let url = serve(|_, path, body| match path {
        _ if body["authorization"] != "Bearer secret" => (401, json!({"error": "no key"})),
        "/v1/models" => (200, json!({"data": [{"id": "m", "owned_by": "vllm"}]})),
        _ => not_found(),
    });
    assert_eq!(probe(&url, None).unwrap_err().step, Step::Key);
    assert!(probe(&url, Some("secret")).is_ok());
}

fn server(url: &str, runtime: Runtime) -> Server {
    Server {
        runtime,
        url: url.to_owned(),
        version: None,
        models: vec![LocalModel::named("m")],
    }
}

#[test]
fn the_tool_check_passes_on_a_call_and_names_what_else_happened() {
    let url = serve(|method, path, body| match (method, path) {
        ("POST", "/v1/chat/completions") => {
            assert_eq!(body["tools"][0]["function"]["name"], CHECK_TOOL);
            assert_eq!(body["messages"][0]["content"], CHECK_PROMPT);
            let message = match body["model"].as_str().unwrap() {
                "caller" => json!({"role": "assistant", "content": "", "tool_calls": [
                    {"id": "c", "type": "function",
                        "function": {"name": CHECK_TOOL, "arguments": "{\"word\":\"scramjet\"}"}}]}),
                "broken" => json!({"role": "assistant", "tool_calls": [
                    {"function": {"name": CHECK_TOOL, "arguments": "{word"}}]}),
                "talker" => json!({"role": "assistant", "content": "I would echo scramjet."}),
                "refused" => {
                    return (
                        400,
                        json!({"error": {"message":
                    "\"auto\" tool choice requires --enable-auto-tool-choice and \
                     --tool-call-parser to be set"}}),
                    );
                }
                _ => return (500, json!({"error": "model failed to load"})),
            };
            (200, json!({"choices": [{"message": message}]}))
        }
        ("GET", "/api/ps") => (
            200,
            json!({"models": [{"model": "caller", "context_length": 8192}]}),
        ),
        _ => not_found(),
    });
    let ollama = server(&url, Runtime::Ollama);
    let checked = check_tools(&ollama, "caller", None).unwrap();
    assert_eq!(checked.arguments, "{\"word\":\"scramjet\"}");
    // Ollama says the window once the model is loaded.
    assert_eq!(checked.window, Some(8192));
    let vllm = server(&url, Runtime::Vllm);
    assert_eq!(check_tools(&vllm, "caller", None).unwrap().window, None);
    let broken = check_tools(&vllm, "broken", None).unwrap_err();
    assert_eq!(broken.step, Step::NoToolCalls);
    assert!(broken.detail.contains("not JSON"), "{broken}");
    let llama = server(&url, Runtime::LlamaCpp);
    let talker = check_tools(&llama, "talker", None).unwrap_err();
    assert_eq!(talker.step, Step::NoToolCalls);
    assert!(
        talker.detail.contains("“I would echo scramjet.”") && talker.detail.contains("--jinja")
    );
    let refused = check_tools(&vllm, "refused", None).unwrap_err();
    assert_eq!(refused.step, Step::NoToolCalls);
    assert!(
        refused.detail.contains("HTTP 400") && refused.detail.contains("vllm serve"),
        "{refused}"
    );
    let failed = check_tools(&vllm, "missing", None).unwrap_err();
    assert_eq!(
        failed,
        problem(Step::Loading, "HTTP 500: model failed to load")
    );
}

#[test]
fn thinking_is_seen_apart_or_between_tags() {
    let seen = |message: Value| thinking(&message);
    assert_eq!(
        seen(json!({"content": "391", "reasoning_content": "17 × 23…"})),
        (true, false)
    );
    assert_eq!(
        seen(json!({"content": "", "reasoning": " "})),
        (false, false)
    );
    assert_eq!(
        seen(json!({"content": "<think>\n17 × 23…\n</think>\n\n391"})),
        (false, true)
    );
    // A template that opens the tag in the prompt.
    assert_eq!(
        seen(json!({"content": "17 × 23…</think>391"})),
        (false, true)
    );
    // Cut off while thinking.
    assert_eq!(seen(json!({"content": "<think>17 × 23"})), (false, true));
    // Thinking turned off leaves an empty block.
    assert_eq!(
        seen(json!({"content": "<think>\n\n</think>\n\n391"})),
        (false, false)
    );
    assert_eq!(seen(json!({"content": "391"})), (false, false));
}

#[test]
fn the_check_finds_whether_thinking_turns_off() {
    let url = serve(|_, path, body| {
        assert_eq!(path, "/v1/chat/completions");
        let off = body["chat_template_kwargs"]["enable_thinking"] == false;
        let call = json!([{"function": {"name": CHECK_TOOL, "arguments": "{}"}}]);
        let message = match (body["model"].as_str().unwrap(), off) {
            ("switched" | "stubborn", false) => {
                json!({"content": "", "reasoning_content": "Call it.", "tool_calls": call})
            }
            ("switched", true) => json!({"content": "391"}),
            ("stubborn", true) => json!({"content": "391", "reasoning_content": "17 × 23…"}),
            ("tagged", false) => json!({"content": "<think>Call it.</think>", "tool_calls": call}),
            ("tagged", true) => json!({"content": "<think>\n\n</think>\n\n391"}),
            ("plain", false) => json!({"content": "", "tool_calls": call}),
            (model, off) => panic!("{model} asked again (thinking off: {off})"),
        };
        (200, json!({"choices": [{"message": message}]}))
    });
    let vllm = server(&url, Runtime::Vllm);
    let switched = check_tools(&vllm, "switched", None).unwrap();
    assert_eq!(
        (switched.thinks, switched.switch, switched.thinking_as_text),
        (true, Some(Switch::ChatTemplate), None)
    );
    let stubborn = check_tools(&vllm, "stubborn", None).unwrap();
    assert_eq!((stubborn.thinks, stubborn.switch), (true, None));
    let tagged = check_tools(&vllm, "tagged", None).unwrap();
    assert_eq!(tagged.switch, Some(Switch::ChatTemplate));
    assert!(
        tagged
            .thinking_as_text
            .unwrap()
            .contains("--reasoning-parser")
    );
    let plain = check_tools(&vllm, "plain", None).unwrap();
    assert_eq!((plain.thinks, plain.switch), (false, None));
}

#[test]
fn the_check_asks_a_llama_cpp_template_that_takes_effort_whether_none_stops_it() {
    let url = serve(|_, _, body| {
        let template_off = body["chat_template_kwargs"]["enable_thinking"] == false;
        let none = body["reasoning_effort"] == "none";
        let call = json!([{"function": {"name": CHECK_TOOL, "arguments": "{}"}}]);
        let message = match (body["model"].as_str().unwrap(), template_off, none) {
            (_, false, false) => {
                json!({"content": "", "reasoning_content": "Call it.", "tool_calls": call})
            }
            ("gpt-oss" | "stops-at-none", true, _) => {
                json!({"content": "3", "reasoning_content": "Count them."})
            }
            ("gpt-oss", _, true) => json!({"content": "3", "reasoning_content": "Count them."}),
            ("stops-at-none", _, true) => json!({"content": "3"}),
            (model, ..) => panic!("{model} asked again: {body}"),
        };
        (200, json!({"choices": [{"message": message}]}))
    });
    let takes_effort = |id: &str| LocalModel {
        effort_in_template: true,
        ..LocalModel::named(id)
    };
    let llama = Server {
        models: vec![takes_effort("gpt-oss"), takes_effort("stops-at-none")],
        ..server(&url, Runtime::LlamaCpp)
    };
    let switch = |id: &str| check_tools(&llama, id, None).unwrap().switch;
    assert_eq!(switch("gpt-oss"), Some(Switch::EffortNoOff));
    assert_eq!(switch("stops-at-none"), Some(Switch::Effort));
}

#[test]
fn the_check_gives_gpt_oss_on_vllm_three_levels_without_asking_for_none() {
    // vLLM 0.11.0 ignores `enable_thinking` for gpt-oss and refuses `none` with HTTP 400.
    let url = serve(|_, _, body| {
        if body["reasoning_effort"] == "none" {
            return (
                400,
                json!({"error": {"message": "Input should be 'low', 'medium' or 'high'"}}),
            );
        }
        let call = json!([{"function": {"name": CHECK_TOOL, "arguments": "{}"}}]);
        let message = match body["chat_template_kwargs"].is_null() {
            true => json!({"content": "", "reasoning_content": "Call it.", "tool_calls": call}),
            false => json!({"content": "3", "reasoning_content": "Count them."}),
        };
        (200, json!({"choices": [{"message": message}]}))
    });
    let vllm = Server {
        models: vec![LocalModel {
            effort_in_template: true,
            ..LocalModel::named("gpt")
        }],
        ..server(&url, Runtime::Vllm)
    };
    let checked = check_tools(&vllm, "gpt", None).unwrap();
    assert_eq!(
        (checked.thinks, checked.switch),
        (true, Some(Switch::EffortLowToHigh))
    );
}

#[test]
fn the_check_asks_an_ollama_model_that_thinks_whether_none_stops_it() {
    let url = serve(|_, path, body| {
        if path == "/api/ps" {
            return (200, json!({"models": []}));
        }
        let off = body["reasoning_effort"] == "none";
        let call = json!([{"function": {"name": CHECK_TOOL, "arguments": "{}"}}]);
        let message = match (body["model"].as_str().unwrap(), off) {
            (_, false) => json!({"content": "", "reasoning": "Call it.", "tool_calls": call}),
            ("qwen3:1.7b", true) => json!({"content": "391"}),
            ("gpt-oss:20b", true) => json!({"content": "391", "reasoning": "17 × 23…"}),
            (model, true) => panic!("{model} asked whether none stops it"),
        };
        (200, json!({"choices": [{"message": message}]}))
    });
    let thinker = |id: &str| LocalModel {
        reasoning: true,
        ..LocalModel::named(id)
    };
    let ollama = Server {
        models: vec![
            thinker("qwen3:1.7b"),
            thinker("gpt-oss:20b"),
            LocalModel::named("gemma3"),
        ],
        ..server(&url, Runtime::Ollama)
    };
    let checked = |id: &str| {
        let checked = check_tools(&ollama, id, None).unwrap();
        (checked.thinks, checked.switch)
    };
    assert_eq!(checked("qwen3:1.7b"), (true, Some(Switch::Effort)));
    assert_eq!(checked("gpt-oss:20b"), (true, Some(Switch::EffortNoOff)));
    assert_eq!(checked("gemma3"), (false, None));
}

#[test]
fn a_saved_switch_sends_the_thinking_level() {
    let scratch = scratch("switch");
    let file = scratch.0.join("models.json");
    let ollama = server("http://localhost:11434", Runtime::Ollama);
    let thinking = |id: &str, switch| LocalModel {
        reasoning: true,
        switch,
        ..model(id, Some(40960))
    };
    let effort = thinking("qwen3:1.7b", Some(Switch::Effort));
    save(&file, "ollama", &ollama, &effort).unwrap();
    let vllm = server("http://localhost:8000", Runtime::Vllm);
    save(
        &file,
        "vllm",
        &vllm,
        &thinking("Qwen3", Some(Switch::ChatTemplate)),
    )
    .unwrap();
    let stubborn = thinking("gpt-oss:20b", Some(Switch::EffortNoOff));
    save(&file, "ollama", &ollama, &stubborn).unwrap();
    // One that thinks whatever it is asked is offered no levels.
    save(&file, "vllm", &vllm, &thinking("R1", None)).unwrap();
    let three = thinking("gpt-oss-20b", Some(Switch::EffortLowToHigh));
    save(&file, "vllm", &vllm, &three).unwrap();
    let providers = super::super::Providers::read(vec![file.clone()], None);
    assert!(
        providers.problems().is_empty(),
        "{:?}",
        providers.problems()
    );
    let sent = |name: &str, effort: &str| {
        let info = &providers.find(name).unwrap().info;
        let request = json!({"model": "m", "input": [], "reasoning": {"effort": effort}});
        let body = super::super::completions::request(&request, info);
        (
            body["reasoning_effort"].clone(),
            body["chat_template_kwargs"].clone(),
        )
    };
    assert_eq!(
        sent("ollama/qwen3:1.7b", "none"),
        (json!("none"), Value::Null)
    );
    assert_eq!(
        sent("ollama/qwen3:1.7b", "high"),
        (json!("high"), Value::Null)
    );
    // Off is not offered: the agent fits it to the lowest level there is.
    let gpt_oss = &providers.find("ollama/gpt-oss:20b").unwrap().info;
    assert!(gpt_oss.reasoning && gpt_oss.fit("none") == "minimal");
    assert_eq!(
        sent("ollama/gpt-oss:20b", "high"),
        (json!("high"), Value::Null)
    );
    let off = sent("vllm/Qwen3", "none");
    assert_eq!(
        (off.0, off.1["enable_thinking"].clone()),
        (Value::Null, json!(false))
    );
    assert_eq!(sent("vllm/Qwen3", "medium").1["enable_thinking"], true);
    assert_eq!(sent("vllm/R1", "high"), (Value::Null, Value::Null));
    assert!(!providers.find("vllm/R1").unwrap().info.reasoning);
    let three = &providers.find("vllm/gpt-oss-20b").unwrap().info;
    assert_eq!(three.levels, ["low", "medium", "high"]);
    assert_eq!(three.fit("none"), "low");
    assert_eq!(sent("vllm/gpt-oss-20b", "low"), (json!("low"), Value::Null));
}

struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> Scratch {
    let path = std::env::temp_dir().join(format!("scramjet-local-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    Scratch(path)
}

fn model(id: &str, window: Option<u64>) -> LocalModel {
    LocalModel {
        window,
        ..LocalModel::named(id)
    }
}

#[test]
fn saving_keeps_the_files_order_and_puts_models_in_place() {
    let scratch = scratch("save");
    let file = scratch.0.join("config/models.json");
    // The user's file, in an order serde_json's maps would sort.
    let theirs = "{\n  \"providers\": {\n    \"zeta\": {\"models\": [{\"id\": \"z\"}], \"baseUrl\": \
                  \"http://z/v1\", \"api\": \"openai-responses\"}\n  },\n  \"credentialFiles\": []\n}";
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, theirs).unwrap();
    let ollama = server("http://localhost:11434", Runtime::Ollama);
    assert_eq!(name_for(&file, &ollama).unwrap(), "ollama");
    save(&file, "ollama", &ollama, &model("qwen3:1.7b", Some(32768))).unwrap();
    save(&file, "ollama", &ollama, &model("gemma:2b", None)).unwrap();
    save(&file, "ollama", &ollama, &model("qwen3:1.7b", Some(40960))).unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    let at = |needle: &str| text.find(needle).unwrap();
    assert!(at("\"zeta\"") < at("\"ollama\"") && at("\"ollama\"") < at("credentialFiles"));
    assert!(
        at("\"models\": [") < at("\"baseUrl\": \"http://z/v1\""),
        "{text}"
    );
    let document: Value = serde_json::from_str(&text).unwrap();
    let entry = &document["providers"]["ollama"];
    assert_eq!(entry["baseUrl"], "http://localhost:11434/v1");
    assert_eq!(
        (entry["api"].as_str(), entry["runtime"].as_str()),
        (Some("openai-completions"), Some("ollama"))
    );
    assert!(entry.get("apiKey").is_none());
    let ids: Vec<&Value> = entry["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| &m["id"])
        .collect();
    assert_eq!(ids, ["qwen3:1.7b", "gemma:2b"]);
    assert_eq!(entry["models"][0]["contextWindow"], 40960);
    assert!(entry["models"][1].get("contextWindow").is_none());

    // The same address keeps its name; another of the runtime takes its port.
    assert_eq!(name_for(&file, &ollama).unwrap(), "ollama");
    let other = server("http://10.0.0.2:11434", Runtime::Ollama);
    assert_eq!(name_for(&file, &other).unwrap(), "ollama-11434");
    let names: Vec<String> = saved(&file)
        .unwrap()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names, ["ollama"]);

    // The providers read it as any models.json: no key, the window as saved.
    let providers = super::super::Providers::read(vec![file.clone()], None);
    assert!(
        providers.problems().is_empty(),
        "{:?}",
        providers.problems()
    );
    let qwen = providers.find("ollama/qwen3:1.7b").unwrap();
    assert_eq!(qwen.info.window, Some(40960));

    remove_model(&file, "ollama", "gemma:2b").unwrap();
    assert_eq!(
        saved(&file).unwrap()[0].1["models"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    remove_model(&file, "ollama", "qwen3:1.7b").unwrap();
    assert!(saved(&file).unwrap().is_empty());
    save(&file, "ollama", &ollama, &model("m", None)).unwrap();
    remove(&file, "ollama").unwrap();
    let left: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(left["providers"].as_object().unwrap().len(), 1);
}

#[test]
fn a_file_that_is_not_json_is_left_alone() {
    let scratch = scratch("broken");
    let file = scratch.0.join("models.json");
    std::fs::write(&file, "{ not json").unwrap();
    let ollama = server("http://localhost:11434", Runtime::Ollama);
    assert!(save(&file, "ollama", &ollama, &model("m", None)).is_err());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ not json");
}

#[test]
fn the_default_gateway_is_read_from_the_routes() {
    let routes = "Iface\tDestination\tGateway \tFlags\n\
                  eth0\t00F0FF0A\t00000000\t0001\n\
                  eth0\t00000000\t0100F0AC\t0003\n";
    assert_eq!(
        default_gateway(routes),
        Some(std::net::Ipv4Addr::new(172, 240, 0, 1))
    );
    assert_eq!(default_gateway("Iface\tDestination\tGateway\n"), None);
}
