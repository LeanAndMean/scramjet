use crate::project_fixture::Fixture;
use scramjet_rs::*;
use std::{ffi::OsString, path::PathBuf, time::Duration};

struct Options {
    project: Option<PathBuf>,
    storage: PathBuf,
    session: String,
    requests: Vec<String>,
    approve: bool,
    verify: bool,
    history: bool,
}
fn options(arguments: impl Iterator<Item = OsString>) -> Result<Options, String> {
    let mut arguments = arguments.peekable();
    let (mut project, mut storage, mut session, mut request, mut second) =
        (None, None, None, None, None);
    let (mut approve, mut verify, mut history) = (false, false, false);
    let mut count = 0;
    while let Some(argument) = arguments.next() {
        count += 1;
        if count > 12 {
            return Err("project-demo argument count ceiling".into());
        }
        let name = argument
            .to_str()
            .ok_or("project-demo option must be UTF-8")?;
        match name {
            "--approve-edit" if !approve => approve = true,
            "--verify-edited" if !verify => verify = true,
            "--history" if !history => history = true,
            "--project" | "--storage" | "--session" | "--request" | "--second-request" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| format!("missing value for {name}"))?;
                count += 1;
                if count > 12 || value.len() > 4096 {
                    return Err("project-demo argument byte/count ceiling".into());
                }
                match name {
                    "--project" if project.is_none() => project = Some(PathBuf::from(value)),
                    "--storage" if storage.is_none() => storage = Some(PathBuf::from(value)),
                    "--session" if session.is_none() => {
                        session = Some(value.into_string().map_err(|_| "session must be UTF-8")?)
                    }
                    "--request" if request.is_none() => {
                        request = Some(value.into_string().map_err(|_| "request must be UTF-8")?)
                    }
                    "--second-request" if second.is_none() => {
                        second = Some(value.into_string().map_err(|_| "request must be UTF-8")?)
                    }
                    _ => return Err(format!("duplicate option {name}")),
                }
            }
            _ => return Err(format!("unsupported/duplicate project-demo option {name}")),
        }
    }
    let storage = storage.ok_or("--storage is required")?;
    let session = session.unwrap_or_else(|| "project-demo".into());
    if session.is_empty()
        || session.len() > 64
        || !session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err("invalid bounded --session".into());
    }
    let mut requests = Vec::with_capacity(2);
    if history {
        if project.is_some() || request.is_some() || second.is_some() || approve || verify {
            return Err("--history accepts only --storage and --session".into());
        }
    } else {
        if project.is_none() {
            return Err("--project is required".into());
        }
        let request = request.ok_or("--request is required")?;
        ProjectRequest::parse(&request).map_err(|e| e.to_string())?;
        requests.push(request);
        if let Some(second) = second {
            if !approve || !verify {
                return Err("--second-request requires --approve-edit and --verify-edited".into());
            }
            ProjectRequest::parse(&second).map_err(|e| e.to_string())?;
            requests.push(second);
        }
    }
    Ok(Options {
        project,
        storage,
        session,
        requests,
        approve,
        verify,
        history,
    })
}
pub fn execute(arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    let options = options(arguments)?;
    if options.history {
        let history = reconstruct_project_history(&options.storage, &options.session)
            .map_err(|e| e.to_string())?;
        println!(
            "project-history: read-only records={} closure_recorded={}; no runtime, approval, continuation or replay",
            history.records.len(),
            history.closure_recorded
        );
        for record in history.records {
            println!(
                "{}",
                serde_json::to_string(record.as_ref()).map_err(|e| e.to_string())?
            );
        }
        for (index, coding) in history.coding.iter().enumerate() {
            if let Some(coding) = coding {
                println!(
                    "coding-history journey={} ending={:?} effects={:?}",
                    index + 1,
                    coding.ending(),
                    coding.operations()
                );
            }
        }
        return Ok(());
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?
        .block_on(run(options))
}
async fn journey(
    session: &mut ProjectSession,
    request: &str,
    approve: bool,
    verify: bool,
) -> Result<bool, String> {
    let observer = session.observer();
    let control = session.control();
    let decide = async {
        loop {
            let snapshot = observer.snapshot();
            if let Some(id) = snapshot.current
                && let Some(coding) = &snapshot.journeys[id.journey() as usize - 1].coding
            {
                if let Some(proposal) = &coding.proposal {
                    println!(
                        "exact proposal: {}",
                        serde_json::to_string(&proposal.intent).map_err(|e| e.to_string())?
                    );
                    control
                        .decide(
                            proposal,
                            if approve {
                                CodingDecision::Approve
                            } else {
                                CodingDecision::Deny
                            },
                        )
                        .map_err(|e| e.to_string())?;
                    return Ok::<_, String>(());
                }
                if coding.run_ended {
                    return Ok(());
                }
            }
            if snapshot.error.is_some() || snapshot.cancelled {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    };
    let (receipt, decision) = tokio::join!(session.submit(request), decide);
    decision?;
    let mut receipt = receipt.map_err(|e| e.to_string())?;
    let snapshot = session.observer().snapshot();
    let id = snapshot.current.unwrap();
    let coding = snapshot
        .records
        .iter()
        .rev()
        .find_map(|r| match r.as_ref() {
            ProjectRecord::Coding(c) if c.journey == id.journey() => Some(c),
            _ => None,
        })
        .unwrap();
    let complete = coding.completion == "Completed";
    let disposition = if complete {
        CommandCompletion::Completed
    } else if coding.completion == "Failed" {
        CommandCompletion::Failed
    } else {
        CommandCompletion::Incomplete
    };
    let summary = coding
        .summary
        .as_deref()
        .unwrap_or("source did not establish success")
        .to_owned();
    let continuation = session
        .report_source(&mut receipt, disposition, &summary)
        .await
        .map_err(|e| e.to_string())?;
    println!(
        "source report: {:?}; paused={}; effect={}",
        disposition,
        continuation.is_some(),
        coding.effect
    );
    if let Some(continuation) = continuation {
        if verify {
            let ready = session
                .resume(continuation)
                .await
                .map_err(|e| e.to_string())?;
            println!("verify_edited Ready admitted; no target read performed by admission");
            let mut receipt = session.verify(ready).await.map_err(|e| e.to_string())?;
            let snapshot = session.observer().snapshot();
            let matched = snapshot
                .records
                .iter()
                .rev()
                .find_map(|r| match r.as_ref() {
                    ProjectRecord::VerificationRead(read) => Some(read.matched),
                    _ => None,
                })
                .unwrap();
            let completion = if matched {
                CommandCompletion::Completed
            } else {
                CommandCompletion::Failed
            };
            session
                .report_verification(
                    &mut receipt,
                    completion,
                    if matched {
                        "full edited target identity/content matched"
                    } else {
                        "edited target verification failed"
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
            println!("verification report: {:?}", completion);
            return Ok(matched);
        }
        println!("paused verification not invoked: --verify-edited omitted");
    }
    Ok(false)
}
async fn run(options: Options) -> Result<(), String> {
    let fixture = Fixture::start(crate::project_fixture::Mode::Golden).await;
    let runtime = match Runtime::new(
        fixture.endpoint,
        2,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
        ConcurrencyMode::Fixed,
    ) {
        Ok(runtime) => runtime,
        Err(_) => {
            let _ = fixture.shutdown().await;
            return Err("runtime owner unavailable".into());
        }
    };
    let (mut session, settlement) = ProjectSession::start(
        runtime,
        options.project.as_ref().unwrap(),
        &options.storage,
        &options.session,
    );
    settlement.release_host();
    let result = async {
        for (index, request) in options.requests.iter().enumerate() {
            let verified = journey(&mut session, request, options.approve, options.verify).await?;
            if !verified && index + 1 < options.requests.len() {
                return Err("next request dependency was not verified".into());
            }
        }
        Ok::<_, String>(())
    }
    .await;
    let finish = settlement.finish().await;
    let snapshot = session.observer().snapshot();
    let fixture = fixture.shutdown().await;
    let finish = finish.map_err(|e| e.to_string())?;
    fixture.attest(&snapshot).map_err(str::to_owned)?;
    let reconciled = Runtime::process_snapshot();
    println!(
        "project settlement: logical={} readers={} coding={} storage={} runtime={} history={} attempts={} fixture_requests={} fixture_peak={} fixture_joined={} unknown_before_fixture={} exposure_before_fixture={} unknown_after_fixture={} exposure_after_fixture={}",
        finish.logical_settled,
        finish.readers_joined,
        finish.coding_joined,
        finish.storage_joined,
        finish.runtime_joined,
        finish.history_acknowledged,
        finish.runtime.attempts,
        fixture.requests.len(),
        fixture.peak,
        fixture.joined,
        finish.runtime.unknown_usage,
        finish.runtime.unresolved,
        reconciled.unknown_usage,
        reconciled.unresolved
    );
    result?;
    if let Some(error) = finish.error {
        return Err(error.to_string());
    }
    if !fixture.joined
        || reconciled.handles != 0
        || reconciled.active != 0
        || reconciled.pending != 0
        || reconciled.unresolved != 0
        || reconciled.unknown_usage != 0
    {
        return Err("fixture/local settlement incomplete".into());
    }
    let history = reconstruct_project_history(&options.storage, &options.session)
        .map_err(|e| e.to_string())?;
    println!(
        "retained project/history: records={} project-preserved=true lock-entry-preserved=true no replay",
        history.records.len()
    );
    Ok(())
}
