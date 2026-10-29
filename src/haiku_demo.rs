use scramjet_rs::{HaikuAgent, HaikuSession, Runtime, reconstruct_haiku_history};
use std::{ffi::OsString, io::Write, path::PathBuf};
#[path = "../tests/support/haiku_fixture.rs"]
mod fixture;
use crate::provider_demo::{fixture as responses, server};

async fn report(text: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || std::io::stdout().lock().write_all(text.as_bytes()))
        .await
        .map_err(|_| "report owner failed")?
        .map_err(|_| "haiku report write failed; no retry".to_owned())
}

pub fn execute(arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    let arguments: Vec<_> = arguments.take(10).collect();
    if arguments.len() > 9 {
        return Err("too many haiku options".into());
    }
    let mut fixture_mode = false;
    let mut history = false;
    let mut approve = false;
    let mut project = None;
    let mut storage = None;
    let mut session = None;
    let mut args = arguments.iter();
    while let Some(option) = args.next() {
        if option == "--fixture" && !fixture_mode {
            fixture_mode = true;
        } else if option == "--history" && !history {
            history = true;
        } else if option == "--approve-effects" && !approve {
            approve = true;
        } else if option == "--project" && project.is_none() {
            project = Some(PathBuf::from(args.next().ok_or("missing project path")?));
        } else if option == "--storage" && storage.is_none() {
            storage = Some(PathBuf::from(args.next().ok_or("missing storage path")?));
        } else if option == "--session" && session.is_none() {
            session = Some(
                args.next()
                    .and_then(|value| value.to_str())
                    .ok_or("missing session")?
                    .to_owned(),
            );
        } else {
            return Err("requires --fixture --project PATH --storage PATH [--approve-effects] [--session ID], or --history --storage PATH [--session ID]; live mode is not eligible".into());
        }
    }
    let storage = storage.ok_or("missing owned storage path")?;
    let session_name = session.unwrap_or_else(|| "haiku-demo".into());
    if history {
        if fixture_mode || project.is_some() || approve {
            return Err("history is read-only, not execution or approval".into());
        }
        let history = reconstruct_haiku_history(&storage, &session_name)
            .map_err(|error| error.to_string())?;
        let mut stdout = std::io::stdout().lock();
        for entry in &history.entries {
            let bytes = serde_json::to_vec(entry.record.as_ref())
                .map_err(|_| "history serialization failed")?;
            stdout
                .write_all(&bytes)
                .and_then(|()| stdout.write_all(b"\n"))
                .map_err(|_| "history report write failed")?;
        }
        writeln!(
            stdout,
            "haiku-demo: history_read_only=true ending_seen={} ending_acknowledged={}",
            history.ending_seen(),
            history.finished()
        )
        .map_err(|_| "history summary write failed")?;
        return Ok(());
    }
    if !fixture_mode {
        return Err("only explicitly selected owned fixture execution is eligible".into());
    }
    let project = project.ok_or("missing owned disposable project")?;
    let session = HaikuSession::create(&project, &storage, &session_name)
        .map_err(|error| error.to_string())?;
    let provider_path = storage.join(format!("{session_name}-provider"));
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&provider_path)
        .map_err(|error| error.to_string())?;
    let agent = HaikuAgent::start(session).map_err(|error| error.to_string())?;
    let observer = agent.observer();
    let control = agent.control();
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "executor unavailable")?;
    executor.block_on(async move {
        let poem =
            "# Haiku\nRust unfolds softly\nStreams answer in quiet files\nA small verse returns\n";
        let (endpoint, server) =
            server::start_sequence(fixture::sequence(poem, 0, "opaque fixture reasoning")).await;
        let runtime = match tokio::task::spawn_blocking(move || {
            Runtime::new_provider(endpoint, &provider_path)
        })
        .await
        .map_err(|_| "provider ledger owner failed")?
        {
            Ok(runtime) => runtime,
            Err(error) => {
                server.abort();
                let _ = server.await;
                return Err(error.to_string());
            }
        };
        let decisions = tokio::spawn(async move {
            let result = async {
                loop {
                    let snapshot = observer.snapshot();
                    if let Some(proposal) = &snapshot.proposal {
                        let bytes = serde_json::to_vec(proposal.intent())
                            .map_err(|_| "proposal serialization failed")?;
                        let printed = tokio::task::spawn_blocking(move || {
                            let mut stdout = std::io::stdout().lock();
                            stdout
                                .write_all(&bytes)
                                .and_then(|()| stdout.write_all(b"\n"))
                        })
                        .await
                        .map_err(|_| "proposal display owner failed")?;
                        if printed.is_err() {
                            control.cancel();
                            return Err("proposal display failed; no approval".to_owned());
                        }
                        control
                            .decide(proposal, approve)
                            .map_err(|error| error.to_string())?;
                    }
                    if snapshot.run_ended {
                        return Ok::<(), String>(());
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            }
            .await;
            if result.is_err() {
                control.cancel();
            }
            result
        });
        let result = agent.run(&runtime).await;
        let decision = decisions.await.map_err(|_| "approval driver failed");
        let history = runtime
            .provider_history()
            .ok_or("provider accounting unavailable")?;
        let local = runtime.shutdown().await;
        if result.is_ok() {
            server.await.map_err(|_| "request-aware fixture failed")?;
        } else {
            server.abort();
            let _ = server.await;
        }
        decision??;
        report(format!(
            "haiku-demo: consumed={} unknown_usage={} unresolved_remote={} local_active={}\n",
            history.consumed(),
            history.unknown_usage(),
            history.unresolved_remote(),
            local.active
        ))
        .await?;
        let run = result.map_err(|error| error.to_string())?;
        if !run.snapshot.worker_joined
            || !run.snapshot.worker_terminated
            || !run.snapshot.history.finished()
            || local.active != 0
        {
            return Err("haiku settlement incomplete".into());
        }
        report(format!(
            "haiku-demo: finished turns={} worker_joined=true namespace_dead=true\n",
            run.turns
        ))
        .await?;
        Ok(())
    })
}
