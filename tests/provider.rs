#[allow(dead_code)]
#[path = "../src/provider_demo.rs"]
mod demo;
use demo::fixture;
use scramjet_rs::{Outcome, ProviderInput, ProviderRequest, Runtime, reconstruct_provider_history};
use std::{io::Write, os::unix::fs::DirBuilderExt, path::PathBuf, process::Command};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn storage() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "scramjet-provider-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    path
}
#[test]
fn private_diagnostic_capture_and_fresh_read_do_not_replay() {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::process::Stdio;
    let directory = storage();
    let capture = directory.join("diagnostic.txt");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&capture)
        .unwrap();
    assert_eq!(
        std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    assert!(
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&capture)
            .is_err()
    );
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "provider_capture_case", "--nocapture"])
        .env("SCRAMJET_CAPTURE_STORAGE", &directory)
        .stdout(Stdio::from(file.try_clone().unwrap()))
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    file.sync_all().unwrap();
    drop(file);
    let before = std::fs::read(directory.join("provider-attempts.jsonl")).unwrap();
    let recovered = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "provider_capture_read", "--nocapture"])
        .env("SCRAMJET_CAPTURE_STORAGE", &directory)
        .output()
        .unwrap();
    assert!(
        recovered.status.success(),
        "{}",
        String::from_utf8_lossy(&recovered.stdout)
    );
    assert_eq!(
        before,
        std::fs::read(directory.join("provider-attempts.jsonl")).unwrap()
    );
    let bytes = std::fs::read(&capture).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("synthetic-private"));
    assert!(!bytes.contains(&0x1b));
    assert!(
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&capture)
            .is_err()
    );
    assert_eq!(bytes, std::fs::read(&capture).unwrap());

    let failed_directory = storage();
    let failed = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
        .args(["provider-demo", "--fixture", "--storage"])
        .arg(&failed_directory)
        .stdout(Stdio::from(
            std::fs::OpenOptions::new()
                .write(true)
                .open("/dev/full")
                .unwrap(),
        ))
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(
        String::from_utf8_lossy(&failed.stderr)
            .contains("sanitized report capture/write failed; no retry")
    );
    let ledger: String =
        std::fs::read_to_string(failed_directory.join("provider-attempts.jsonl")).unwrap();
    assert_eq!(ledger.lines().count(), 3);
    assert_eq!(ledger.lines().filter(|l| l.contains("Reserved")).count(), 1);
}
#[test]
fn provider_capture_case() {
    let Some(directory) = std::env::var_os("SCRAMJET_CAPTURE_STORAGE") else {
        return;
    };
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = executor.block_on(demo::run(
        PathBuf::from(directory),
        false,
        false,
        None,
        Some(fixture::disagreement_events()),
    ));
    let error = result.unwrap_err();
    assert_eq!(
        error,
        "terminal/usage acceptance failed; stop without retry"
    );
}
#[test]
fn provider_capture_read() {
    let Some(directory) = std::env::var_os("SCRAMJET_CAPTURE_STORAGE") else {
        return;
    };
    let bytes = std::fs::read(PathBuf::from(directory).join("diagnostic.txt")).unwrap();
    assert!(bytes.len() < 16384);
    let text = std::str::from_utf8(&bytes).unwrap();
    let line = text
        .lines()
        .find(|l| l.starts_with("{\"report\":"))
        .unwrap();
    assert!(line.len() <= scramjet_rs::PROVIDER_DIAGNOSTIC_BYTES);
    let report: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(report["version"], 1);
    assert_eq!(report["outcome"], "Unsupported");
    assert_eq!(report["durable"], true);
    assert_eq!(report["terminal_observed"], true);
    assert_eq!(report["usage"]["total_tokens"], 15);
    assert_eq!(report["diagnostics_complete"], true);
    assert_eq!(report["consumed"], 1);
    assert_eq!(report["unresolved_remote"], 0);
    assert_eq!(report["unknown_usage"], 0);
    assert_eq!(report["local_active"], 0);
    assert_eq!(report["failure"]["field"], "response.output.item");
    let item = &report["failure"]["final_difference"]["items"][0];
    assert_eq!(item["index"], 0);
    assert_eq!(item["fields"], (1 << 7) | (1 << 9));
    assert_eq!(item["done_kinds"][9], 0);
    assert_eq!(item["final_kinds"][9], 4);
    assert_eq!(item["other_fields"], true);
    assert_eq!(item["content"][2][0], 0);
    assert_eq!(item["content"][2][1], 8);
}
fn request() -> ProviderRequest {
    ProviderRequest::new(&[ProviderInput::User("Synthetic hello".into())], &[]).unwrap()
}
#[test]
fn provider_transport_cases() {
    for case in [
        "text",
        "call",
        "headers",
        "unsupported",
        "missing-usage",
        "failed-usage",
        "trailing",
        "missing-terminal",
        "bad-identity",
        "diagnostic",
        "wire-limit",
        "frame-limit",
        "output-limit",
        "401",
        "429",
        "502",
        "504",
        "cancel",
        "drop",
        "deadline",
        "cap",
        "corrupt",
        "recovery",
    ] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "provider_case", "--nocapture"])
            .env("SCRAMJET_PROVIDER_CASE", case)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{case}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if matches!(
            case,
            "recovery" | "corrupt" | "cap" | "cancel" | "drop" | "trailing"
        ) {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let line = stdout
                .lines()
                .find(|line| line.starts_with("RECOVERY|"))
                .unwrap();
            let fields: Vec<_> = line.split('|').collect();
            let path = PathBuf::from(fields[1]);
            let before = std::fs::read(path.join("provider-attempts.jsonl")).unwrap();
            let recovered = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "provider_recover", "--nocapture"])
                .env("SCRAMJET_PROVIDER_STORAGE", &path)
                .env("SCRAMJET_PROVIDER_ENDPOINT", fields[2])
                .env(
                    "SCRAMJET_PROVIDER_CORRUPT",
                    if case == "corrupt" { "yes" } else { "no" },
                )
                .env(
                    "SCRAMJET_PROVIDER_COUNT",
                    if case == "cap" { "4" } else { "1" },
                )
                .output()
                .unwrap();
            assert!(
                recovered.status.success(),
                "{case}: {} {}",
                String::from_utf8_lossy(&recovered.stdout),
                String::from_utf8_lossy(&recovered.stderr)
            );
            assert_eq!(
                before,
                std::fs::read(path.join("provider-attempts.jsonl")).unwrap()
            );
        }
    }
}
#[test]
fn provider_case() {
    let Ok(case) = std::env::var("SCRAMJET_PROVIDER_CASE") else {
        return;
    };
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    executor.block_on(async {
        let directory=storage();
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint=listener.local_addr().unwrap();
        let case_server=case.clone();
        let (ready_tx, mut ready_rx)=tokio::sync::mpsc::channel(4);
        let server=tokio::spawn(async move {
            let count=if case_server=="cap" {4} else {1};
            for _ in 0..count {
                let (mut socket,_)=listener.accept().await.unwrap();
                let mut request=Vec::new();
                let header_end=loop {
                    let mut byte=[0;1];socket.read_exact(&mut byte).await.unwrap();request.push(byte[0]);
                    if request.ends_with(b"\r\n\r\n") { break request.len(); }
                    assert!(request.len()<4096);
                };
                let head=std::str::from_utf8(&request).unwrap().to_ascii_lowercase();
                assert!(head.starts_with("post /v1/responses http/1.1\r\n"));
                assert!(head.contains("authorization: bearer managed-by-proxy\r\n"));
                assert!(head.contains("content-type: application/json\r\n"));
                assert!(!head.contains("x-fixture"));
                let length=head.lines().find_map(|line|line.strip_prefix("content-length: ")).unwrap().parse::<usize>().unwrap();
                assert!(length<=16384);
                request.resize(header_end+length,0);socket.read_exact(&mut request[header_end..]).await.unwrap();
                let body:serde_json::Value=serde_json::from_slice(&request[header_end..]).unwrap();
                assert_eq!(body["model"],"gpt-6-luna");assert_eq!(body["store"],false);assert_eq!(body["max_output_tokens"],1024);
                assert!(body.get("reasoning").is_none());
                if matches!(case_server.as_str(),"cancel"|"drop"|"deadline") {
                    ready_tx.send(()).await.unwrap();
                    let mut byte=[0;1];assert_eq!(socket.read(&mut byte).await.unwrap(),0);continue;
                }
                let status=case_server.parse::<u16>().unwrap_or(200);
                let usage=if matches!(case_server.as_str(),"missing-usage"|"cap") {serde_json::Value::Null} else {fixture::known_usage()};
                let mut events=if case_server=="call" {fixture::call_events("{}",usage)} else {fixture::text_events("hé🙂",usage)};
                if case_server=="failed-usage" {
                    events.truncate(1);events.push(serde_json::json!({"type":"response.failed","response":{"id":"resp_1","status":"failed","output":[],"usage":fixture::known_usage()}}));
                }
                if case_server=="missing-terminal" {events.pop();}
                if case_server=="bad-identity" {events[4]["item_id"]=serde_json::json!("foreign");}
                if case_server=="wire-limit" {
                    events.truncate(4);
                    for _ in 0..1200 {events.push(serde_json::json!({"type":"response.output_text.delta","output_index":0,"item_id":"msg_1","content_index":0,"delta":""}));}
                }
                if case_server=="output-limit" {
                    events=fixture::text_events(&"x".repeat(9000),serde_json::Value::Null);events.truncate(7);
                    events.push(serde_json::json!({"type":"response.content_part.added","output_index":0,"item_id":"msg_1","content_index":1,"part":{"type":"output_text","text":"","annotations":[]}}));
                    events.push(serde_json::json!({"type":"response.output_text.delta","output_index":0,"item_id":"msg_1","content_index":1,"delta":"x".repeat(9000)}));
                }
                if case_server=="unsupported" {events=fixture::call_events("{}",fixture::known_usage());events[1]["item"]["namespace"]=serde_json::json!("foreign");}
                if case_server=="diagnostic" {events=fixture::disagreement_events();}
                let mut wire=fixture::wire(&events,true);
                if case_server=="frame-limit" {wire=vec![b'x';16385];}
                if case_server=="trailing" {wire.extend_from_slice(b"garbage");}
                if status!=200 {wire=b"private error must not be displayed".to_vec();}
                let extra=if case_server=="headers" {(0..40).map(|i|format!("X-Metadata-{i}: synthetic\r\n")).collect::<String>()}else{String::new()};
                let head=format!("HTTP/1.1 {status} Result\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n{}{extra}\r\n",wire.len(),if status==429 {"Retry-After: 1\r\n"} else {""});
                socket.write_all(head.as_bytes()).await.unwrap();
                for chunk in wire.chunks(7) { if socket.write_all(chunk).await.is_err() {break;} tokio::task::yield_now().await; }
            }
        });
        let runtime=Runtime::new_provider(endpoint,&directory).unwrap();
        assert!(runtime.submit("fixture",scramjet_rs::SyntheticCost::new(1,1).unwrap()).is_err());
        let count=if case=="cap" {4} else {1};
        for _ in 0..count {
            let (mut handle,capture_receiver)=runtime.submit_provider_captured(request()).unwrap();
            let id=handle.id();
            if matches!(case.as_str(),"cancel"|"drop"|"deadline") {
                tokio::time::timeout(std::time::Duration::from_secs(5),ready_rx.recv()).await.unwrap().unwrap();
                if case=="drop" {drop(handle);while runtime.snapshot().active!=0 {tokio::task::yield_now().await;}let capture=capture_receiver.receive().await.unwrap();assert!(capture.local_joined);assert!(!capture.body_eof);assert!(capture.bytes.is_empty());assert_eq!(capture.id,id);break;}
                if case=="deadline" {tokio::time::pause();tokio::time::advance(std::time::Duration::from_secs(31)).await;}else{handle.cancel();}
            }
            let response=tokio::time::timeout(std::time::Duration::from_secs(5),handle.result()).await.unwrap().unwrap();
            assert_eq!(response.id,id);assert!(response.durable);
            assert!(Runtime::reconcile_fixture(id).is_err());assert!(Runtime::reconcile_usage_fixture(id,0).is_err());
            match case.as_str() {
                "text"|"call"|"headers"|"recovery"|"corrupt" => {assert_eq!(response.outcome,Outcome::Success);assert!(response.envelope.is_some());assert!(response.usage.unwrap().complete());},
                "missing-usage"|"cap" => {assert_eq!(response.outcome,Outcome::Success);assert!(response.envelope.is_some());assert!(response.usage.is_none());},
                "failed-usage" => {assert_ne!(response.outcome,Outcome::Success);assert!(response.envelope.is_none());assert!(response.usage.unwrap().complete());},
                "cancel" => {assert_eq!(response.outcome,Outcome::Cancelled);assert!(response.envelope.is_none());},
                "deadline" => {assert_eq!(response.outcome,Outcome::Deadline);assert!(response.envelope.is_none());},
                "trailing"|"missing-terminal"|"bad-identity" => {assert_ne!(response.outcome,Outcome::Success);assert!(response.envelope.is_none());assert!(response.remote_terminal.is_none());},
                "wire-limit"|"frame-limit"|"output-limit" => {assert_eq!(response.outcome,Outcome::ResourceLimit);assert!(response.envelope.is_none());},
                "unsupported" => {assert_eq!(response.outcome,Outcome::Unsupported);assert!(response.envelope.is_none());},
                "diagnostic" => {assert_eq!(response.outcome,Outcome::Unsupported);assert!(response.envelope.is_none());assert_eq!(response.remote_terminal.as_ref().unwrap().response_id,"resp_1");assert!(response.usage.as_ref().unwrap().complete());let difference=response.failure.unwrap().final_difference.unwrap();assert!(difference.items[0].unwrap().other_fields);assert_eq!(difference.items[0].unwrap().content.unwrap().first_part.unwrap().fields,8);},
                status => {assert_eq!(response.outcome,Outcome::HttpRejected(status.parse().unwrap()));assert!(response.envelope.is_none());assert!(response.remote_terminal.is_none());}
            }
            drop(handle);
            let capture=capture_receiver.receive().await.unwrap();
            assert!(capture.local_joined);assert_eq!(capture.id,id);assert!(capture.reservation.is_some());
            assert!(capture.bytes.len()<=scramjet_rs::PROVIDER_CAPTURE_BYTES);assert_eq!(capture.received_bytes,capture.bytes.len());assert!(capture.parser_fed_bytes<=capture.received_bytes);
            assert!(capture.received_bytes<=response.wire_bytes);
            if matches!(case.as_str(),"text"|"call"|"headers") {
                let events=if case=="call" {fixture::call_events("{}",fixture::known_usage())}else{fixture::text_events("hé🙂",fixture::known_usage())};
                assert_eq!(capture.bytes,fixture::wire(&events,true));assert!(capture.body_eof);assert!(capture.parser_accepted);
                assert_eq!(scramjet_rs::replay_provider_data(&capture.bytes).outcome,Outcome::Success);
            } else if case=="missing-terminal" {assert!(capture.body_eof);assert!(!capture.parser_accepted);}
            else if matches!(case.as_str(),"cancel"|"deadline") {assert!(!capture.body_eof);assert!(capture.bytes.is_empty());}
            else if matches!(case.as_str(),"diagnostic"|"trailing"|"bad-identity"|"frame-limit") {
                assert_eq!(capture.body_eof,matches!(case.as_str(),"trailing"|"diagnostic"));assert!(!capture.parser_accepted);assert_ne!(scramjet_rs::replay_provider_data(&capture.bytes).outcome,Outcome::Success);
            }
        }
        let history=runtime.provider_history().unwrap();
        assert_eq!(history.consumed(),count);
        if case=="cap" {assert_eq!(history.unknown_usage(),4);let mut handle=runtime.submit_provider(request()).unwrap();handle.cancel();assert_eq!(handle.result().await.unwrap().outcome,Outcome::Cancelled);drop(handle);}
        let stats=runtime.shutdown().await;
        assert_eq!(stats.active,0);assert!(stats.synthetic_budget.is_none());assert!(stats.synthetic_measured.is_none());
        if !matches!(case.as_str(),"bad-identity"|"trailing"|"missing-terminal") {server.await.unwrap();}else {server.abort();let _=server.await;}
        if case=="corrupt" {std::fs::OpenOptions::new().append(true).open(directory.join("provider-attempts.jsonl")).unwrap().write_all(b"{\"kind\":").unwrap();}
        println!("RECOVERY|{}|{}",directory.display(),endpoint);
    });
}
#[test]
fn correlated_two_post_continuation_and_pre_admission_refusal() {
    for crlf in ["lf", "crlf"] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "provider_continuation_case", "--nocapture"])
            .env("SCRAMJET_CONTINUATION_NEWLINE", crlf)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{crlf}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn provider_continuation_case() {
    let Ok(newline) = std::env::var("SCRAMJET_CONTINUATION_NEWLINE") else {
        return;
    };
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    executor.block_on(async {
        use scramjet_rs::{ProviderTool, PROVIDER_ENVELOPE_BYTES};
        use serde_json::json;
        let directory = storage();
        let user = "synthetic original context: hé🙂\\\"\n";
        let result = "synthetic result only, no effect executed: hé🙂\\\"\n";
        let tool = ProviderTool { name:"inert".into(), description:"synthetic data only".into(), parameters:json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false}) };
        let tools = vec![tool];
        let mut events = fixture::continuation_events("assistant hé🙂\\\"\n", "{\"text\":\"hé🙂\\n\"}", "opaque:/hé🙂\\\"\u{0000}");
        for item in events.last_mut().unwrap()["response"]["output"].as_array_mut().unwrap() {
            if item["type"] == "reasoning" { item["encrypted_content"] = serde_json::Value::Null; }
            if item["type"] == "function_call" { item.as_object_mut().unwrap().remove("status"); }
        }
        let canonical: Vec<_> = (0..4).map(|index| {
            if index == 3 { events.last().unwrap()["response"]["output"][index].clone() }
            else { events.iter().find(|event| event["type"] == "response.output_item.done" && event["output_index"] == index).unwrap()["item"].clone() }
        }).collect();
        let wire_tools = json!([{"type":"function","name":"inert","description":"synthetic data only","parameters":tools[0].parameters,"strict":true}]);
        let first = json!({"model":"gpt-6-luna","input":[{"role":"user","content":user}],"tools":wire_tools,"stream":true,"store":false,"include":["reasoning.encrypted_content"],"max_output_tokens":1024,"parallel_tool_calls":false});
        let mut second = first.clone();
        let input = second["input"].as_array_mut().unwrap();
        input.extend(canonical.clone());
        input.push(json!({"type":"function_call_output","call_id":"call_1","output":result}));
        let mut body = fixture::wire(&events, newline == "crlf");
        if newline == "crlf" { body.extend_from_slice(b"data: [DONE]\r\n\r\n"); }
        let final_body = fixture::wire(&fixture::text_events("final hé🙂", fixture::known_usage()), newline == "crlf");
        let (endpoint, server) = demo::server::start_sequence(vec![(Some(first),body),(Some(second),final_body)]).await;
        let runtime = Runtime::new_provider(endpoint, &directory).unwrap();
        let mut context = vec![ProviderInput::User(user.into())];
        let mut handle = runtime.submit_provider(ProviderRequest::new(&context, &tools).unwrap()).unwrap();
        let response = handle.result().await.unwrap();
        assert_eq!(response.outcome, Outcome::Success);
        assert!(response.durable && response.usage.as_ref().unwrap().complete());
        let envelope = response.envelope.unwrap();
        assert_eq!(envelope.items.len(),4);
        assert_eq!(envelope.items[2].function_call().unwrap().arguments,"{\"text\":\"hé🙂\\n\"}");
        context.extend(envelope.items.into_iter().map(ProviderInput::Item));
        context.push(ProviderInput::ToolResult{call_id:"call_1".into(),output:result.into()});
        drop(handle);
        let mut handle = runtime.submit_provider(ProviderRequest::new(&context, &tools).unwrap()).unwrap();
        let response = handle.result().await.unwrap();
        assert_eq!(response.outcome,Outcome::Success);
        assert_eq!(response.envelope.unwrap().items[0].text().as_deref(),Some("final hé🙂"));
        assert!(response.durable && response.usage.unwrap().complete());
        drop(handle);
        let before = std::fs::read(directory.join("provider-attempts.jsonl")).unwrap();
        let attempts = runtime.snapshot().attempts;
        let original = format!("{context:?}");
        let mut oversized = context.clone();
        oversized.push(ProviderInput::User("\u{0000}".repeat(PROVIDER_ENVELOPE_BYTES / 6)));
        assert!(matches!(ProviderRequest::new(&oversized,&tools),Err(Outcome::ResourceLimit)));
        assert_eq!(format!("{context:?}"),original);
        assert_eq!(runtime.snapshot().attempts,attempts);
        assert_eq!(std::fs::read(directory.join("provider-attempts.jsonl")).unwrap(),before);
        assert_eq!(runtime.provider_history().unwrap().consumed(),2);
        let stats = runtime.shutdown().await;
        assert_eq!(stats.active,0);
        assert_eq!(stats.unknown_usage,0);
        assert_eq!(stats.unresolved,0);
        server.await.unwrap();
    });
}

#[test]
fn provider_recover() {
    let Some(path) = std::env::var_os("SCRAMJET_PROVIDER_STORAGE") else {
        return;
    };
    let history = reconstruct_provider_history(
        &PathBuf::from(path),
        std::env::var("SCRAMJET_PROVIDER_ENDPOINT")
            .unwrap()
            .parse()
            .unwrap(),
    );
    if std::env::var("SCRAMJET_PROVIDER_CORRUPT").unwrap() == "yes" {
        assert!(history.is_err());
    } else {
        let history = history.unwrap();
        let count = std::env::var("SCRAMJET_PROVIDER_COUNT")
            .unwrap()
            .parse::<usize>()
            .unwrap();
        assert_eq!(history.consumed(), count);
        if count == 4 {
            let path = PathBuf::from(std::env::var_os("SCRAMJET_PROVIDER_STORAGE").unwrap());
            let endpoint = std::env::var("SCRAMJET_PROVIDER_ENDPOINT")
                .unwrap()
                .parse()
                .unwrap();
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let runtime = Runtime::new_provider(endpoint, &path).unwrap();
                    assert_eq!(runtime.snapshot().unknown_usage, 4);
                    let mut handle = runtime.submit_provider(request()).unwrap();
                    handle.cancel();
                    assert_eq!(handle.result().await.unwrap().outcome, Outcome::Cancelled);
                    drop(handle);
                    assert_eq!(runtime.snapshot().attempts, 0);
                    runtime.shutdown().await;
                });
        }
    }
}
