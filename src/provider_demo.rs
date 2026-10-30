use scramjet_rs::{
    Outcome, PROVIDER_DIAGNOSTIC_BYTES, ProviderFailure, ProviderHistory, ProviderInput,
    ProviderRequest, ProviderResponse, ProviderTool, ProviderUsage, Runtime,
};
use serde::Serialize;
use serde_json::json;
use std::{ffi::OsString, io::Write, path::PathBuf};
#[path = "provider_demo_capture.rs"]
pub(crate) mod capture;
#[path = "../tests/support/responses_fixture.rs"]
pub(crate) mod fixture;
#[path = "../tests/support/provider_server.rs"]
pub(crate) mod server;

pub fn execute(arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    let arguments: Vec<_> = arguments.collect();
    if arguments
        .first()
        .is_some_and(|v| v == "--validation-pair" || v == "--validation-observe")
    {
        let report = if arguments.len() == 2 && arguments[0] == "--validation-observe" {
            scramjet_rs::provider_validation_observe(&PathBuf::from(&arguments[1]))?
        } else if arguments.len() == 4
            && arguments[0] == "--validation-pair"
            && arguments[1] == "--live"
            && arguments[2] == "--capture"
        {
            scramjet_rs::provider_validation_live_pair(&PathBuf::from(&arguments[3]))?
        } else if arguments.len() == 6
            && arguments[0] == "--validation-pair"
            && arguments[1] == "--fixture"
            && arguments[2] == "--storage"
            && arguments[4] == "--capture"
        {
            scramjet_rs::provider_validation_fixture_pair(
                &PathBuf::from(&arguments[3]),
                &PathBuf::from(&arguments[5]),
            )?
        } else if arguments.len() == 6
            && arguments[0] == "--validation-observe"
            && arguments[1] == "--fixture"
            && arguments[2] == "--storage"
            && arguments[4] == "--capture"
        {
            scramjet_rs::provider_validation_fixture_observe(
                &PathBuf::from(&arguments[3]),
                &PathBuf::from(&arguments[5]),
            )?
        } else {
            return Err("requires --validation-pair --live --capture NEW_DIRECTORY, --validation-pair --fixture --storage OWNED_DIRECTORY --capture NEW_DIRECTORY, or --validation-observe DIRECTORY".into());
        };
        let mut stdout = std::io::stdout().lock();
        stdout
            .write_all(&report)
            .and_then(|()| stdout.write_all(b"\n"))
            .map_err(|_| "validation report write failed; no retry")?;
        let value: serde_json::Value =
            serde_json::from_slice(&report).map_err(|_| "validation report unavailable")?;
        if arguments[0] == "--validation-pair" && value["success"] != true {
            return Err("validation pair stopped; no retry".into());
        }
        return Ok(());
    }
    if arguments.len() == 2 && arguments[0] == "--replay" {
        let report = capture::replay(&PathBuf::from(&arguments[1]))?;
        let mut stdout = std::io::stdout().lock();
        stdout
            .write_all(&report)
            .and_then(|()| stdout.write_all(b"\n"))
            .map_err(|_| "replay report write failed")?;
        return Ok(());
    }
    let invalid = || {
        "requires --live or --fixture, then --storage PATH [--tool] [--retain-unknown-through N] [--capture NEW_DIRECTORY], or --replay DIRECTORY"
            .to_owned()
    };
    if arguments.len() < 3
        || (arguments[0] != "--live" && arguments[0] != "--fixture")
        || arguments[1] != "--storage"
    {
        return Err(invalid());
    }
    let storage = PathBuf::from(&arguments[2]);
    let live = arguments[0] == "--live";
    let mut tool = false;
    let mut retained = None;
    let mut capture_path = None;
    let mut tail = arguments[3..].iter();
    while let Some(option) = tail.next() {
        if option == "--tool" && !tool {
            tool = true;
        } else if option == "--retain-unknown-through" && retained.is_none() && live {
            retained = Some(
                tail.next()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.parse::<usize>().ok())
                    .ok_or_else(invalid)?,
            );
        } else if option == "--capture" && capture_path.is_none() {
            capture_path = Some(PathBuf::from(tail.next().ok_or_else(invalid)?));
        } else {
            return Err(invalid());
        }
    }
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "executor unavailable")?;
    executor.block_on(async {
        let prepared = if let Some(path) = capture_path {
            Some(
                tokio::task::spawn_blocking(move || capture::prepare(&path, live))
                    .await
                    .map_err(|_| "capture setup worker failed; no dispatch")??,
            )
        } else {
            None
        };
        if let Some(prepared) = prepared {
            run_captured(storage, tool, live, retained, None, Some(prepared)).await
        } else {
            run(storage, tool, live, retained, None).await
        }
    })
}
pub(crate) async fn run(
    storage: PathBuf,
    tool: bool,
    live: bool,
    retained: Option<usize>,
    fixture_events: Option<Vec<serde_json::Value>>,
) -> Result<(), String> {
    run_captured(storage, tool, live, retained, fixture_events, None).await
}
pub(crate) async fn run_captured(
    storage: PathBuf,
    tool: bool,
    live: bool,
    retained: Option<usize>,
    fixture_events: Option<Vec<serde_json::Value>>,
    prepared: Option<capture::Prepared>,
) -> Result<(), String> {
    let tools = if tool {
        vec![ProviderTool {
            name: "inert".into(),
            description: "Return an inert synthetic envelope; no effects execute.".into(),
            parameters: json!({"type":"object","properties":{"message":{"type":"string"}},"required":["message"],"additionalProperties":false}),
        }]
    } else {
        vec![]
    };
    let input = if tool {
        "Call inert exactly once with message synthetic-smoke. This is a transport test; no tool will execute."
    } else {
        "Reply with exactly synthetic-smoke. This is a transport test."
    };
    let request = ProviderRequest::new(&[ProviderInput::User(input.into())], &tools)
        .map_err(|_| "request refused")?;
    let (runtime, fixture_task) = if live {
        (
            Runtime::new_proxy_retaining(&storage, retained.unwrap_or(0))
                .await
                .map_err(|_| "Proxy readiness/history/ownership refused")?,
            None,
        )
    } else {
        let events = fixture_events.unwrap_or_else(|| {
            if tool {
                fixture::call_events("{\"message\":\"synthetic-smoke\"}", fixture::known_usage())
            } else {
                fixture::text_events("synthetic-smoke", fixture::known_usage())
            }
        });
        let (endpoint, task) = server::start(fixture::wire(&events, false)).await;
        (
            Runtime::new_provider(endpoint, &storage)
                .map_err(|_| "fixture history/ownership refused")?,
            Some(task),
        )
    };
    let prior = runtime
        .provider_history()
        .ok_or("missing provider history")?;
    if prior.has_unknown_after(retained.unwrap_or(0)) || prior.remaining() == 0 {
        runtime.shutdown().await;
        return Err(
            "prior remote/usage uncertainty or exhausted shared POST allowance; no dispatch".into(),
        );
    }
    let submitted = if prepared.is_some() {
        runtime
            .submit_provider_captured(request)
            .map(|(handle, receiver)| (handle, Some(receiver)))
    } else {
        runtime
            .submit_provider(request)
            .map(|handle| (handle, None))
    };
    let (mut handle, capture_receiver) = match submitted {
        Ok(pair) => pair,
        Err(_) => {
            runtime.shutdown().await;
            return Err("admission refused; no retry".into());
        }
    };
    let response = handle.result().await;
    drop(handle);
    let history = runtime
        .provider_history()
        .ok_or("missing provider history")?;
    let stats = runtime.shutdown().await;
    if let Some(task) = fixture_task {
        task.await.map_err(|_| "fixture failed")?;
    }
    let response = response.ok_or("missing local result; no retry")?;
    let report = encode_report(&response, &history, stats.active)?;
    if let (Some(prepared), Some(receiver)) = (prepared, capture_receiver) {
        let capture = receiver
            .receive()
            .await
            .ok_or("capture unavailable; no retry")?;
        let persisted_report = report.clone();
        tokio::task::spawn_blocking(move || capture::persist(prepared, capture, persisted_report))
            .await
            .map_err(|_| "capture persistence worker failed; no retry")??;
    }
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(&report)
        .and_then(|()| stdout.write_all(b"\n"))
        .map_err(|_| "sanitized report capture/write failed; no retry")?;
    writeln!(stdout,
        "provider-demo: consumed={} remaining={} unresolved_remote={} unknown_usage={} local_active={}",
        history.consumed(),
        history.remaining(),
        history.unresolved_remote(),
        history.unknown_usage(),
        stats.active
    ).map_err(|_| "console capture/write failed; no retry")?;
    writeln!(
        stdout,
        "provider-demo: outcome={:?} durable={} wire_bytes={} terminal={} usage={:?}",
        response.outcome,
        response.durable,
        response.wire_bytes,
        response
            .remote_terminal
            .as_ref()
            .map(|t| t.status.as_str())
            .unwrap_or("unknown"),
        response.usage
    )
    .map_err(|_| "console capture/write failed; no retry")?;
    if response.outcome != Outcome::Success
        || !response.durable
        || response.usage.as_ref().is_none_or(|u| !u.complete())
    {
        return Err("terminal/usage acceptance failed; stop without retry".into());
    }
    let envelope = response
        .envelope
        .ok_or("no executable completed envelope")?;
    let calls: Vec<_> = envelope
        .items
        .iter()
        .filter_map(|i| i.function_call())
        .collect();
    if tool {
        if calls.len() != 1 || calls[0].name != "inert" {
            return Err("inert tool envelope missing; no retry".into());
        }
        let arguments: serde_json::Value =
            serde_json::from_str(calls[0].arguments).map_err(|_| "invalid inert arguments")?;
        if arguments != json!({"message":"synthetic-smoke"}) {
            return Err("inert arguments differ; no retry".into());
        }
    } else if !calls.is_empty()
        || !envelope
            .items
            .iter()
            .filter_map(|i| i.text())
            .any(|text| text.trim() == "synthetic-smoke")
    {
        return Err("synthetic text envelope differs; no retry".into());
    }
    writeln!(
        stdout,
        "provider-demo: validated {} envelope; no tools executed",
        if tool { "function-call" } else { "text" }
    )
    .map_err(|_| "console capture/write failed; no retry")?;
    Ok(())
}

#[derive(Serialize)]
struct DiagnosticReport<'a> {
    report: &'static str,
    version: u8,
    outcome: &'static str,
    http_status: Option<u16>,
    durable: bool,
    wire_bytes: usize,
    consumed: usize,
    remaining: usize,
    unresolved_remote: usize,
    unknown_usage: usize,
    local_active: usize,
    terminal_observed: bool,
    usage: Option<&'a ProviderUsage>,
    diagnostics_complete: bool,
    failure: Option<&'a ProviderFailure>,
}
struct ReportWriter(Vec<u8>);
impl Write for ReportWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > PROVIDER_DIAGNOSTIC_BYTES - self.0.len() {
            return Err(std::io::Error::other("diagnostic report budget"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(crate) fn encode_report(
    response: &ProviderResponse,
    history: &ProviderHistory,
    active: usize,
) -> Result<Vec<u8>, String> {
    let outcome = match response.outcome {
        Outcome::Success => "Success",
        Outcome::HttpRejected(_) => "HttpRejected",
        Outcome::Framing => "Framing",
        Outcome::Unsupported => "Unsupported",
        Outcome::Incomplete => "Incomplete",
        Outcome::Transport => "Transport",
        Outcome::ResourceLimit => "ResourceLimit",
        Outcome::Cancelled => "Cancelled",
        Outcome::ConsumerLost => "ConsumerLost",
        Outcome::Deadline => "Deadline",
    };
    let mut report = DiagnosticReport {
        report: "provider-diagnostic",
        version: 1,
        outcome,
        http_status: if let Outcome::HttpRejected(status) = response.outcome {
            Some(status)
        } else {
            None
        },
        durable: response.durable,
        wire_bytes: response.wire_bytes,
        consumed: history.consumed(),
        remaining: history.remaining(),
        unresolved_remote: history.unresolved_remote(),
        unknown_usage: history.unknown_usage(),
        local_active: active,
        terminal_observed: response.remote_terminal.is_some(),
        usage: response.usage.as_ref(),
        diagnostics_complete: true,
        failure: response.failure.as_ref(),
    };
    let mut writer = ReportWriter(Vec::with_capacity(PROVIDER_DIAGNOSTIC_BYTES));
    if serde_json::to_writer(&mut writer, &report).is_err() {
        writer.0.clear();
        report.failure = None;
        report.diagnostics_complete = false;
        serde_json::to_writer(&mut writer, &report)
            .map_err(|_| "diagnostic report unavailable; no retry")?;
    }
    Ok(writer.0)
}

#[cfg(test)]
pub(crate) fn check_report_writer_budget() {
    let mut writer = ReportWriter(Vec::with_capacity(PROVIDER_DIAGNOSTIC_BYTES));
    writer
        .write_all(&vec![b'x'; PROVIDER_DIAGNOSTIC_BYTES - 1])
        .unwrap();
    assert!(writer.write_all(b"yy").is_err());
    assert_eq!(writer.0.len(), PROVIDER_DIAGNOSTIC_BYTES - 1);
    writer.write_all(b"z").unwrap();
    assert!(writer.write_all(b"x").is_err());
    assert_eq!(writer.0.len(), PROVIDER_DIAGNOSTIC_BYTES);
    assert_eq!(writer.0.capacity(), PROVIDER_DIAGNOSTIC_BYTES);
}
