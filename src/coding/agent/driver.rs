use super::*;
use crate::coding::haiku::agent::{HaikuAgent, HaikuRun};

pub(super) enum DriverMode<'a> {
    Fixture(&'a mut CodingAgent),
    Haiku(&'a mut HaikuAgent),
}
pub(super) enum DriverRun {
    Fixture(CodingRun),
    Haiku(HaikuRun),
}
enum Prepared {
    Fixture(String),
    Provider(crate::ProviderRequest),
}
enum Handle {
    Fixture(crate::RequestHandle),
    Provider(crate::ProviderHandle),
}
enum Completion {
    Fixture {
        id: RequestId,
        text: String,
        success: bool,
        usage: Option<crate::SyntheticUsage>,
        outcome: Option<Outcome>,
        accounting: Option<CodingAccounting>,
    },
    Provider(Box<crate::ProviderResponse>),
}
impl DriverMode<'_> {
    fn cancellation(&self) -> &EditCancellation {
        match self {
            Self::Fixture(agent) => &agent.cancellation,
            Self::Haiku(agent) => &agent.cancellation,
        }
    }
    fn expired(&self) -> bool {
        matches!(self, Self::Haiku(agent) if std::time::Instant::now() >= agent.deadline)
    }
    fn admitted(&self, turn: u64, id: RequestId) {
        match self {
            Self::Fixture(agent) => agent
                .shared
                .update(|snapshot| snapshot.attempts[(turn - 1) as usize].request_id = Some(id)),
            Self::Haiku(agent) => agent.admitted(id),
        }
    }
}
async fn fixture_request(
    agent: &CodingAgent,
    turn: u64,
    previous: &str,
) -> Result<Prepared, CodingError> {
    let text = if let Some(project) = &agent.project {
        serialize_project_context(&agent.session, turn, &project.evidence, previous)?
    } else if agent.invocation == 2 {
        serialize_follow_context(
            &agent.session,
            turn,
            agent.prior.as_ref().unwrap(),
            &agent.prompt,
            previous,
        )?
    } else {
        serialize_context(&agent.session, turn, &agent.prompt, previous)?
    };
    let (reply, receive) = oneshot::channel();
    agent
        .sender
        .as_ref()
        .unwrap()
        .try_send(Work::Request {
            turn,
            text: text.clone(),
            reply,
        })
        .map_err(|_| CodingError::Invalid("serialized worker unavailable"))?;
    receive
        .await
        .map_err(|_| CodingError::Invalid("worker lost request acknowledgement"))??;
    Ok(Prepared::Fixture(text))
}
fn submit(
    runtime: &Runtime,
    prepared: Prepared,
    deadline: Option<tokio::time::Instant>,
) -> Result<Handle, CodingError> {
    match prepared {
        Prepared::Fixture(text) => runtime
            .submit(&text, crate::SyntheticCost::new(1, 1).unwrap())
            .map(Handle::Fixture),
        Prepared::Provider(request) => runtime
            .submit_provider_until(request, deadline.unwrap())
            .map(Handle::Provider),
    }
    .map_err(|_| CodingError::Invalid("model admission refused"))
}
async fn complete(mut handle: Handle, mode: &DriverMode<'_>) -> Result<Completion, CodingError> {
    match &mut handle {
        Handle::Fixture(handle) => {
            let id = handle.id();
            let mut assembled = String::with_capacity(RESPONSE_BYTES);
            let mut rejected = false;
            loop {
                let delta = tokio::select! {
                    delta = handle.next_delta() => delta,
                    _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {
                        if mode.cancellation().is_cancelled() { rejected = true; handle.cancel(); }
                        continue;
                    }
                };
                let Some(delta) = delta else { break };
                if mode.cancellation().is_cancelled() {
                    rejected = true;
                    handle.cancel();
                }
                if delta.len() > RESPONSE_BYTES - assembled.len() {
                    rejected = true;
                    handle.cancel();
                } else if !rejected {
                    assembled.push_str(&delta);
                }
            }
            let terminal = handle.terminal().await;
            let duplicate = handle.terminal().await.is_some();
            let qualified = terminal.as_ref().filter(|t| t.id == id && !duplicate);
            let success = !rejected
                && !mode.cancellation().is_cancelled()
                && qualified.is_some_and(|t| t.outcome == Outcome::Success);
            let accounting = if qualified.is_some() {
                handle
                    .accounting_receipt()
                    .ok()
                    .map(|(unknown_usage, unresolved_exposure)| CodingAccounting {
                        request_id: id,
                        unknown_usage,
                        unresolved_exposure,
                    })
            } else {
                None
            };
            Ok(Completion::Fixture {
                id,
                text: assembled,
                success,
                usage: qualified.map(|t| t.synthetic_usage),
                outcome: qualified.map(|t| t.outcome.clone()),
                accounting,
            })
        }
        Handle::Provider(handle) => {
            let id = handle.id();
            let response = loop {
                if mode.cancellation().is_cancelled() || mode.expired() {
                    mode.cancellation().cancel();
                    handle.cancel();
                }
                let result = tokio::select! { result = handle.result() => result, _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => continue };
                break result.ok_or(CodingError::Invalid("provider local acknowledgement lost"))?;
            };
            if handle.result().await.is_some() || response.id != id {
                return Err(CodingError::Stale);
            }
            Ok(Completion::Provider(Box::new(response)))
        }
    }
}
pub(super) async fn drive(
    mut mode: DriverMode<'_>,
    runtime: &Runtime,
) -> Result<DriverRun, CodingError> {
    let (first, last) = match &mode {
        DriverMode::Fixture(agent) => (
            if agent.invocation == 2 { 5 } else { 1 },
            if agent.exact_replacement || (agent.follow_up && agent.invocation == 1) {
                4
            } else {
                TURNS
            },
        ),
        DriverMode::Haiku(_) => (1, 8),
    };
    let mut previous = String::new();
    let mut ids = Vec::with_capacity((last - first + 1) as usize);
    let mut usage = Some(0);
    for turn in first..=last {
        if mode.expired() {
            return Err(CodingError::Deadline);
        }
        if mode.cancellation().is_cancelled() {
            return Err(CodingError::Cancelled);
        }
        let prepared = match &mut mode {
            DriverMode::Fixture(agent) => fixture_request(agent, turn, &previous).await?,
            DriverMode::Haiku(agent) => Prepared::Provider(agent.request().await?),
        };
        if mode.expired() {
            return Err(CodingError::Deadline);
        }
        if mode.cancellation().is_cancelled() {
            return Err(CodingError::Cancelled);
        }
        let deadline = match &mode {
            DriverMode::Haiku(agent) => Some(tokio::time::Instant::from_std(agent.deadline)),
            DriverMode::Fixture(_) => None,
        };
        let handle = submit(runtime, prepared, deadline)?;
        let id = match &handle {
            Handle::Fixture(handle) => handle.id(),
            Handle::Provider(handle) => handle.id(),
        };
        ids.push(id);
        mode.admitted(turn, id);
        let completion = complete(handle, &mode).await?;
        match (&mut mode, completion) {
            (
                DriverMode::Fixture(agent),
                Completion::Fixture {
                    id,
                    text,
                    success,
                    usage: actual,
                    outcome,
                    accounting,
                },
            ) => {
                usage = actual.and_then(|actual| actual.add_to(usage));
                agent.shared.update(|snapshot| {
                    let slot = &mut snapshot.attempts[(turn - 1) as usize];
                    slot.outcome = outcome;
                    slot.accounting = accounting;
                });
                if !success && !agent.cancellation.is_cancelled() {
                    agent
                        .shared
                        .failure("model response failed; material acknowledgement pending");
                }
                let (reply, receive) = oneshot::channel();
                agent
                    .sender
                    .as_ref()
                    .unwrap()
                    .try_send(Work::Response {
                        turn,
                        id,
                        text,
                        success,
                        reply,
                    })
                    .map_err(|_| CodingError::Invalid("serialized worker unavailable"))?;
                let step = receive
                    .await
                    .map_err(|_| CodingError::Invalid("worker lost tool acknowledgement"))??;
                if let Some(edit_operation) = step.final_edit {
                    if agent.invocation == 1 {
                        agent.prior = step.prior;
                    }
                    return Ok(DriverRun::Fixture(CodingRun {
                        summary: step.output,
                        turns: turn,
                        request_ids: ids,
                        edit_operation,
                        usage_tokens: usage,
                    }));
                }
                previous = step.output;
            }
            (DriverMode::Haiku(agent), Completion::Provider(response)) => {
                if agent.response(id, response).await? {
                    return Ok(DriverRun::Haiku(HaikuRun {
                        turns: turn as usize,
                        request_ids: ids,
                        snapshot: agent.observer().snapshot(),
                    }));
                }
            }
            _ => unreachable!(),
        }
    }
    Err(CodingError::Invalid(
        "model turn ceiling without final result",
    ))
}
