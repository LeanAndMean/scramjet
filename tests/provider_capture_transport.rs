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
struct Removed(std::path::PathBuf);
impl Drop for Removed {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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
        let directory=std::env::temp_dir().join(format!("s414-transport-{}",std::process::id()));
        // A directory left by an earlier process with the same id would make create fail.
        let _=std::fs::remove_dir_all(&directory);
        std::fs::DirBuilder::new().mode(0o700).create(&directory).unwrap();
        let _cleanup=Removed(directory.clone());
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let endpoint=listener.local_addr().unwrap();
        let events=if case=="mid-chunk" {demo::fixture::failed_terminal_events()}else{demo::fixture::text_events("hé🙂",demo::fixture::known_usage())};
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
        else if case=="mid-chunk" {assert_eq!(capture.bytes,expected);assert_eq!(capture.parser_fed_bytes,capture.received_bytes);assert_eq!(capture.parser_fed_bytes,expected.len());assert!(capture.body_eof);assert_eq!(response.outcome,Outcome::Incomplete);assert_eq!(response.remote_terminal.as_ref().unwrap().status,"failed");assert!(response.usage.as_ref().unwrap().complete());}
        else if case=="trailers" {assert_eq!(capture.bytes,expected);assert!(!capture.body_eof);assert!(!capture.parser_accepted);assert_eq!(response.outcome,Outcome::Framing);assert!(!String::from_utf8_lossy(&capture.bytes).contains("X-Untrusted"));}
        else if matches!(case.as_str(),"interleaved"|"split-lf") {assert_eq!(capture.bytes,expected);assert!(capture.body_eof);assert!(capture.parser_accepted);assert_eq!(scramjet_rs::replay_provider_data(&capture.bytes).outcome,Outcome::Success);}
        else if case=="wire-exact" {assert_eq!(response.wire_bytes,scramjet_rs::PROVIDER_CAPTURE_BYTES);assert_eq!(capture.bytes,expected);assert_eq!(response.outcome,Outcome::Success);assert!(capture.body_eof);}
        else {assert_eq!(response.outcome,Outcome::ResourceLimit);assert!(!capture.body_eof);assert!(capture.received_bytes<=scramjet_rs::PROVIDER_CAPTURE_BYTES);assert_eq!(capture.bytes,expected[..capture.received_bytes]);}
        server.await.unwrap();
    });
}
