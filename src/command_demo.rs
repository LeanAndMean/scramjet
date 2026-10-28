use crate::command_fixture as fixture;
use scramjet_rs::{
    CommandCompletion, CommandContinuation, CommandPhase, CommandPolicy, CommandReadResult,
    CommandSession,
};

pub fn run(automatic: bool, resume: bool) -> Result<(), Box<dyn std::error::Error>> {
    let roots = fixture::Roots::new(fixture::AUTOMATIC_CATALOG, b"same\n", b"same\n")?;
    let policy = if automatic {
        CommandPolicy::Automatic
    } else {
        CommandPolicy::Pause
    };
    let mut session = CommandSession::load(&roots.definitions, &roots.inputs, policy)?;
    session.discover("compare")?;
    let primary = session.begin("compare")?;
    session.execute(primary)?;
    let before = session.snapshot();
    if before.invocations[0].as_ref().unwrap().phase != CommandPhase::AwaitingCompletion
        || before.reads != 2
    {
        return Err("comparison did not await explicit completion".into());
    }
    println!(
        "command-demo: session={} invocation={} command=compare before_report={:?} reads={}",
        primary.session(),
        primary.ordinal(),
        before.invocations[0].as_ref().unwrap().phase,
        before.reads
    );
    session.report(
        primary,
        CommandCompletion::Completed,
        "Both captured inputs are byte-exact equal.",
        Some("inspect"),
    )?;
    if automatic || resume {
        let companion = if resume {
            let handle = session.paused_continuation().ok_or("no paused offer")?;
            if session.snapshot().reads != 2 {
                return Err("pause dispatched companion".into());
            }
            let id = session.resume(handle)?.ok_or("resume was not admitted")?;
            let admitted = session.snapshot();
            if admitted.reads != 2
                || admitted.invocations[1].as_ref().unwrap().phase != CommandPhase::Ready
            {
                return Err("resume performed execution".into());
            }
            println!(
                "command-demo: explicit_resume Ready reads=2; separate execution/report required"
            );
            session.execute(id)?;
            id
        } else {
            session.follow()?.ok_or("companion was not admitted")?
        };
        if session.snapshot().invocations[1].as_ref().unwrap().phase
            != CommandPhase::AwaitingCompletion
        {
            return Err("companion completion was fabricated".into());
        }
        session.report(
            companion,
            CommandCompletion::Completed,
            "Captured input is readable.",
            None,
        )?;
    }
    let final_state = session.finalize();
    match (&final_state.continuation, automatic || resume) {
        (CommandContinuation::Paused { source, target }, false)
            if *source == primary && target == "inspect" && final_state.reads == 2 => {}
        (CommandContinuation::Followed { source, target }, true)
            if *source == primary && target.ordinal() == 2 && final_state.reads == 3 => {}
        _ => return Err("incorrect continuation disposition".into()),
    }
    for fact in final_state.invocations.iter().flatten() {
        for step in fact.steps.iter().flatten() {
            match &step.result {
                CommandReadResult::Read(text) => println!(
                    "session={} invocation={} step={} target={:?} captured_bytes={} captured_ascii={}",
                    fact.id.session(),
                    fact.id.ordinal(),
                    step.ordinal,
                    step.target,
                    text.len(),
                    text.escape_default()
                ),
                CommandReadResult::Failed(error) => {
                    return Err(format!("unexpected read failure: {error:?}").into());
                }
            }
        }
        let report = fact.report.as_ref().ok_or("missing explicit report")?;
        if report.completion != CommandCompletion::Completed {
            return Err("command not completed".into());
        }
        println!(
            "session={} invocation={} explicit_report={:?} summary={:?}",
            fact.id.session(),
            fact.id.ordinal(),
            report.completion,
            report.summary
        );
    }
    println!(
        "command-demo: host={policy:?} continuation={:?} invocations={} reads={} retained_bytes={} settled={} local_final=0",
        final_state.continuation,
        final_state.invocations.iter().flatten().count(),
        final_state.reads,
        final_state.retained_bytes,
        final_state.settled
    );
    Ok(())
}
