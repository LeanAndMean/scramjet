#[allow(dead_code)]
#[path = "../src/provider_demo.rs"]
mod demo;
use scramjet_rs::{Outcome, ProviderInput, ProviderRequest, Runtime};
use std::{os::unix::fs::DirBuilderExt, process::Command};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[test]
fn received_data_lifetime_and_framing_cases() {
    for case in [
        "cancel-prefix",
        "drop-prefix",
        "deadline-prefix",
        "terminal-prefix",
        "trailers",
        "mid-chunk",
        "wire-exact",
        "wire-over",
        "interleaved",
        "split-lf",
    ] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "transport_child", "--nocapture"])
            .env("S414_TRANSPORT_CASE", case)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{case}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
#[test]
fn prospective_refusal_http_boundaries() {
    for case in [
        "opaque-lf",
        "opaque-crlf",
        "missing-encryption",
        "terminal-encryption",
        "missing-usage",
        "partial-usage",
        "duplicate",
        "trailing-event",
        "partial-frame",
        "marker-data",
        "second-marker",
        "trailers",
        "truncated",
        "cancel",
        "deadline",
        "frame-limit",
        "wire-limit",
    ] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "refusal_transport_child", "--nocapture"])
            .env("S414_REFUSAL_CASE", case)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{case}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn refusal_transport_child() {
    let Ok(case) = std::env::var("S414_REFUSAL_CASE") else {
        return;
    };
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        let directory=std::env::temp_dir().join(format!("s414-refusal-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));std::fs::DirBuilder::new().mode(0o700).create(&directory).unwrap();
        let mut events=demo::fixture::continuation_events("hé🙂\\\"\n","{\"message\":\"hé🙂\"}","opaque-hé🙂");
        events[0]["response"]["model"]=serde_json::json!("gpt-6-luna");
        events.last_mut().unwrap()["response"]["model"]=serde_json::json!("gpt-6-luna");
        events.last_mut().unwrap()["response"]["output"][0]["unknown"]=serde_json::json!({"opaque":true});
        if matches!(case.as_str(),"missing-encryption"|"terminal-encryption") {
            events.iter_mut().find(|e|e["type"]=="response.output_item.done" && e["output_index"]==0).unwrap()["item"].as_object_mut().unwrap().remove("encrypted_content");
            if case=="missing-encryption" {events.last_mut().unwrap()["response"]["output"][0]["encrypted_content"]=serde_json::Value::Null;}
        }
        if case=="missing-usage" {events.last_mut().unwrap()["response"].as_object_mut().unwrap().remove("usage");}
        if case=="partial-usage" {events.last_mut().unwrap()["response"]["usage"]=serde_json::json!({"input_tokens":10});}
        if case=="duplicate" {let mut duplicate=events.last().unwrap().clone();duplicate["response"]["usage"]=serde_json::json!({"input_tokens":20});events.push(duplicate);}
        let mut body=demo::fixture::wire(&events,case=="opaque-crlf");
        match case.as_str() {
            "opaque-crlf"=>body.extend_from_slice(b": legal\r\n\r\ndata: [DONE]\r\n\r\n"),
            "opaque-lf"=>body.extend_from_slice(b": legal\n\n"),
            "trailing-event"=>body.extend_from_slice(b"data: {\"type\":\"response.completed\"}\n\n"),
            "partial-frame"=>body.extend_from_slice(b": unfinished"),
            "marker-data"=>body.extend_from_slice(b"data: [DONE]\n\ndata: {}\n\n"),
            "second-marker"=>body.extend_from_slice(b"data: [DONE]\n\ndata: [DONE]\n\n"),
            "frame-limit"=>body.extend(std::iter::repeat_n(b':',16385)),
            "wire-limit"=>{while body.len()<132000 {body.extend_from_slice(b": bounded comment padding padding padding\n\n");}},
            _=>{}
        }
        let expected=body.clone();
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let endpoint=listener.local_addr().unwrap();
        let server_case=case.clone();let (ready_tx,ready_rx)=tokio::sync::oneshot::channel();
        let server=tokio::spawn(async move {
            let (mut socket,_)=listener.accept().await.unwrap();let mut head=Vec::new();
            while !head.ends_with(b"\r\n\r\n") {let mut b=[0];socket.read_exact(&mut b).await.unwrap();head.push(b[0]);assert!(head.len()<4096);}
            let head=std::str::from_utf8(&head).unwrap().to_ascii_lowercase();let length:usize=head.lines().find_map(|l|l.strip_prefix("content-length: ")).unwrap().parse().unwrap();socket.read_exact(&mut vec![0;length]).await.unwrap();
            let prefix=matches!(server_case.as_str(),"cancel"|"deadline");
            if server_case=="trailers" {
                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await.unwrap();
                socket.write_all(format!("{:x}\r\n",body.len()).as_bytes()).await.unwrap();socket.write_all(&body).await.unwrap();socket.write_all(b"\r\n0\r\nX-Untrusted: value\r\n\r\n").await.unwrap();
            } else {
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()+usize::from(prefix||server_case=="truncated")).as_bytes()).await.unwrap();
                for chunk in body.chunks(if server_case.starts_with("opaque") {1}else{127}) {if socket.write_all(chunk).await.is_err() {break;}tokio::task::yield_now().await;}
            }
            let _=ready_tx.send(());
            if prefix {let mut b=[0];assert_eq!(socket.read(&mut b).await.unwrap(),0);}
            assert!(tokio::time::timeout(std::time::Duration::from_millis(100),listener.accept()).await.is_err());
        });
        let runtime=Runtime::new_provider(endpoint,&directory).unwrap();let (mut handle,receiver)=runtime.submit_provider_captured(ProviderRequest::new(&[ProviderInput::User("synthetic".into())],&[]).unwrap()).unwrap();
        if matches!(case.as_str(),"cancel"|"deadline") {
            ready_rx.await.unwrap();tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            if case=="cancel" {handle.cancel();} else {tokio::time::pause();tokio::time::advance(std::time::Duration::from_secs(31)).await;tokio::time::resume();}
        }
        let response=handle.result().await.unwrap();drop(handle);let capture=receiver.receive().await.unwrap();
        assert!(response.envelope.is_none());assert!(response.durable);assert!(capture.local_joined);assert!(!capture.parser_accepted);
        let positive=matches!(case.as_str(),"opaque-lf"|"opaque-crlf"|"missing-encryption"|"terminal-encryption"|"missing-usage"|"partial-usage");
        if positive {
            assert_eq!(response.outcome,Outcome::Unsupported);assert!(capture.body_eof);assert_eq!(capture.bytes,expected);assert_eq!(capture.parser_fed_bytes,expected.len());
            let terminal=response.remote_terminal.as_ref().unwrap();assert_eq!(terminal.response_id,"resp_1");assert_eq!(terminal.reported_model.as_deref(),Some("gpt-6-luna"));assert_eq!(terminal.status,"completed");
            let replay=scramjet_rs::replay_provider_data(&capture.bytes);assert_eq!(replay.outcome,Outcome::Unsupported);assert!(!replay.parser_accepted);assert_eq!(serde_json::to_value(replay.failure).unwrap(),serde_json::to_value(&response.failure).unwrap());
            if case=="missing-usage" {assert!(response.usage.is_none());}else if case=="partial-usage" {assert_eq!(response.usage.as_ref().unwrap().input_tokens,Some(10));assert!(!response.usage.as_ref().unwrap().complete());}else {assert!(response.usage.as_ref().unwrap().complete());}
        } else {
            let outcome=match case.as_str() {"partial-frame"|"truncated"=>Outcome::Incomplete,"cancel"=>Outcome::Cancelled,"deadline"=>Outcome::Deadline,"frame-limit"|"wire-limit"=>Outcome::ResourceLimit,_=>Outcome::Framing};
            assert_eq!(response.outcome,outcome);assert!(response.remote_terminal.is_none());assert!(response.usage.is_none());
        }
        let history=runtime.provider_history().unwrap();assert_eq!(history.consumed(),1);assert_eq!(history.unresolved_remote(),usize::from(!positive));assert_eq!(history.unknown_usage(),usize::from(!positive||matches!(case.as_str(),"missing-usage"|"partial-usage")));assert_eq!(history.attempts[0].ordinal,response.reservation.unwrap());
        let stats=runtime.shutdown().await;assert_eq!(stats.active,0);server.await.unwrap();
    });
}

#[test]
fn transport_child() {
    let Ok(case) = std::env::var("S414_TRANSPORT_CASE") else {
        return;
    };
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    executor.block_on(async {
        let directory=std::env::temp_dir().join(format!("s414-transport-{}",std::process::id()));std::fs::DirBuilder::new().mode(0o700).create(&directory).unwrap();
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let endpoint=listener.local_addr().unwrap();
        let events=if case=="mid-chunk" {demo::fixture::disagreement_events()}else{demo::fixture::text_events("hé🙂",demo::fixture::known_usage())};
        let events=if case=="interleaved" {
            let call=demo::fixture::call_events("{\"message\":\"hé🙂\\\\\\\"\"}",demo::fixture::known_usage());
            let text=demo::fixture::text_events("hé🙂\\\"\n",demo::fixture::known_usage());
            let reason=serde_json::json!({"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"opaque-hé🙂"});
            let mut mixed=vec![call[0].clone()];
            let mut text:Vec<_>=text[2..8].to_vec();for event in &mut text {event["output_index"]=serde_json::json!(1);}
            mixed.push(call[1].clone());mixed.push(text[0].clone());
            mixed.push(serde_json::json!({"type":"response.output_item.added","output_index":2,"item":{"type":"reasoning","id":"rs_1","summary":[]}}));
            mixed.push(call[2].clone());mixed.extend(text[1..4].iter().cloned());mixed.push(call[3].clone());mixed.push(text[4].clone());mixed.push(call[4].clone());mixed.push(text[5].clone());
            mixed.push(serde_json::json!({"type":"response.output_item.done","output_index":2,"item":reason}));
            mixed.push(serde_json::json!({"type":"response.completed","response":{"id":"resp_1","status":"completed","output":[call[4]["item"].clone(),text[5]["item"].clone(),reason],"usage":demo::fixture::known_usage()}}));mixed
        }else{events};
        let mut body=demo::fixture::wire(&events,case!="split-lf");
        let prefix=case.ends_with("prefix");let wire_bound=case.starts_with("wire-");
        if case=="mid-chunk" {body.extend_from_slice(&[b'x';100]);}
        if prefix && case!="terminal-prefix" {body=demo::fixture::wire(&events[..4],true);}
        let head=|size|format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n");
        if wire_bound {
            let target=scramjet_rs::PROVIDER_CAPTURE_BYTES+usize::from(case=="wire-over");
            let mut length=target-100;while length+head(length).len()!=target {length=target-head(length).len();}
            let mut remaining=length-body.len();let mut padded=Vec::with_capacity(length);
            while remaining>0 {let size=remaining.min(16384);let size=if remaining>size && remaining-size<3 {size-3}else{size};padded.push(b':');padded.extend(std::iter::repeat_n(b'x',size-3));padded.extend_from_slice(b"\n\n");remaining-=size;}
            padded.extend_from_slice(&body);body=padded;
        }
        let expected=body.clone();let server_case=case.clone();let (ready_tx,ready_rx)=tokio::sync::oneshot::channel();
        let server=tokio::spawn(async move {
            let (mut socket,_)=listener.accept().await.unwrap();let mut request=Vec::new();while !request.ends_with(b"\r\n\r\n") {let mut byte=[0];socket.read_exact(&mut byte).await.unwrap();request.push(byte[0]);assert!(request.len()<4096);}
            let text=std::str::from_utf8(&request).unwrap().to_ascii_lowercase();let length=text.lines().find_map(|l|l.strip_prefix("content-length: ")).unwrap().parse::<usize>().unwrap();socket.read_exact(&mut vec![0;length]).await.unwrap();
            if server_case=="trailers" {socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await.unwrap();socket.write_all(format!("{:x}\r\n",body.len()).as_bytes()).await.unwrap();socket.write_all(&body).await.unwrap();socket.write_all(b"\r\n0\r\nX-Untrusted: synthetic-private\r\n\r\n").await.unwrap();}
            else {socket.write_all(head(body.len()+usize::from(prefix)*20).as_bytes()).await.unwrap();
                if matches!(server_case.as_str(),"interleaved"|"split-lf") {for byte in &body {socket.write_all(&[*byte]).await.unwrap();tokio::task::yield_now().await;}}
                else {let _=socket.write_all(&body).await;}}
            let _=ready_tx.send(());
            if prefix {let mut byte=[0];assert_eq!(socket.read(&mut byte).await.unwrap(),0);}
        });
        let runtime=Runtime::new_provider(endpoint,&directory).unwrap();let (mut handle,receiver)=runtime.submit_provider_captured(ProviderRequest::new(&[ProviderInput::User("synthetic".into())],&[]).unwrap()).unwrap();let id=handle.id();
        ready_rx.await.unwrap();
        if prefix {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            if case=="drop-prefix" {drop(handle);let stats=runtime.shutdown().await;assert_eq!(stats.active,0);let capture=receiver.receive().await.unwrap();assert_eq!(capture.bytes,expected);assert!(capture.local_joined);assert!(!capture.body_eof);assert!(!capture.parser_accepted);server.await.unwrap();return;}
            if case=="deadline-prefix" {tokio::time::pause();tokio::time::advance(std::time::Duration::from_secs(31)).await;}else{handle.cancel();}
        }
        let response=handle.result().await.unwrap();drop(handle);let stats=runtime.shutdown().await;assert_eq!(stats.active,0);let capture=receiver.receive().await.unwrap();assert_eq!(capture.id,id);assert!(capture.local_joined);
        if prefix {assert_eq!(capture.bytes,expected);assert!(!capture.body_eof);assert!(!capture.parser_accepted);assert!(response.remote_terminal.is_none());assert!(response.usage.is_none());assert!(response.envelope.is_none());assert_eq!(response.outcome,if case=="deadline-prefix" {Outcome::Deadline}else{Outcome::Cancelled});}
        else if case=="mid-chunk" {assert_eq!(capture.bytes,expected);assert_eq!(capture.parser_fed_bytes,capture.received_bytes);assert_eq!(capture.parser_fed_bytes,expected.len());assert!(capture.body_eof);assert_eq!(response.outcome,Outcome::Incomplete);assert!(response.remote_terminal.is_none());assert!(response.usage.is_none());}
        else if case=="trailers" {assert_eq!(capture.bytes,expected);assert!(!capture.body_eof);assert!(!capture.parser_accepted);assert_eq!(response.outcome,Outcome::Framing);assert!(!String::from_utf8_lossy(&capture.bytes).contains("X-Untrusted"));}
        else if matches!(case.as_str(),"interleaved"|"split-lf") {assert_eq!(capture.bytes,expected);assert!(capture.body_eof);assert!(capture.parser_accepted);assert_eq!(scramjet_rs::replay_provider_data(&capture.bytes).outcome,Outcome::Success);}
        else if case=="wire-exact" {assert_eq!(response.wire_bytes,scramjet_rs::PROVIDER_CAPTURE_BYTES);assert_eq!(capture.bytes,expected);assert_eq!(response.outcome,Outcome::Success);assert!(capture.body_eof);}
        else {assert_eq!(response.outcome,Outcome::ResourceLimit);assert!(!capture.body_eof);assert!(capture.received_bytes<=scramjet_rs::PROVIDER_CAPTURE_BYTES);assert_eq!(capture.bytes,expected[..capture.received_bytes]);}
        server.await.unwrap();
    });
}
