use scramjet_rs::{CodingObservation, RequestId, Runtime};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot, watch},
    task::{JoinHandle, JoinSet},
};

#[cfg(test)]
pub const SESSION: &str = "coding-ui";
pub const BEFORE: &str = "before\n";
pub const AFTER: &str = "after\t\u{202e} e\u{301}\\n\n";
#[derive(Clone)]
pub struct Reply {
    pub text: String,
    pub done: bool,
    pub status: u16,
    pub usage: bool,
}
#[cfg(test)]
pub fn golden() -> Vec<Reply> {
    [
        serde_json::json!({"kind":"Read","op":1,"target":"sample.txt"}),
        serde_json::json!({"kind":"Replace","op":2,"target":"sample.txt","expected":BEFORE,"replacement":AFTER}),
        serde_json::json!({"kind":"Verify","op":3,"target":"sample.txt","expected":AFTER}),
        serde_json::json!({"kind":"Final","summary":"Exact disposable edit completed and verified."}),
    ].into_iter().map(|value| Reply { text: value.to_string(), done: true, status: 200, usage: true }).collect()
}
#[derive(Clone, Copy, Debug)]
pub struct Receipt {
    pub turn: u64,
    pub request_id: RequestId,
    pub cost: u32,
    pub joined: bool,
    pub consumed: bool,
}
#[derive(Clone, Debug)]
pub struct Report {
    pub session: Arc<str>,
    pub receipts: [Option<Receipt>; 8],
    pub error: Option<&'static str>,
    pub accepts: usize,
    pub prompt: Option<Arc<str>>,
    pub follow_prompt: Option<Arc<str>>,
    pub supervisor_joined: [bool; 8],
}
struct Shared {
    report: Mutex<Report>,
    replies: Vec<Reply>,
    consuming: bool,
    follow_up: bool,
    #[cfg(test)]
    padding: std::sync::atomic::AtomicUsize,
}
pub struct Fixture {
    pub address: SocketAddr,
    shared: Arc<Shared>,
    stop: watch::Sender<bool>,
    driver: JoinHandle<()>,
    checkpoint: mpsc::Sender<oneshot::Sender<Result<Report, &'static str>>>,
    #[cfg(test)]
    checkpoint_gate: Option<Arc<tokio::sync::Notify>>,
}
impl Fixture {
    pub async fn consuming_session(session: Arc<str>) -> std::io::Result<Self> {
        Self::start(Vec::new(), true, false, session).await
    }
    pub async fn replies_session(replies: Vec<Reply>, session: Arc<str>) -> std::io::Result<Self> {
        Self::start(replies, false, false, session).await
    }
    #[allow(dead_code)]
    pub async fn consuming_follow_up(session: Arc<str>) -> std::io::Result<Self> {
        Self::start(Vec::new(), true, true, session).await
    }
    #[allow(dead_code)]
    pub async fn consuming_follow_up_replies(
        session: Arc<str>,
        replies: Vec<Reply>,
    ) -> std::io::Result<Self> {
        Self::start(replies, true, true, session).await
    }
    #[cfg(test)]
    #[allow(dead_code)]
    pub fn pad_responses(&self, bytes: usize) {
        assert!(bytes <= 16384);
        self.shared
            .padding
            .store(bytes, std::sync::atomic::Ordering::Relaxed);
    }
    #[cfg(test)]
    #[allow(dead_code)]
    pub fn hold_checkpoint(&mut self) -> Arc<tokio::sync::Notify> {
        let gate = Arc::new(tokio::sync::Notify::new());
        self.checkpoint_gate = Some(gate.clone());
        gate
    }
    #[allow(dead_code)]
    pub async fn checkpoint(&self) -> Result<Report, &'static str> {
        #[cfg(test)]
        if let Some(gate) = &self.checkpoint_gate {
            gate.notified().await;
        }
        let (send, receive) = oneshot::channel();
        self.checkpoint
            .try_send(send)
            .map_err(|_| "fixture checkpoint unavailable")?;
        receive
            .await
            .map_err(|_| "fixture checkpoint acknowledgement lost")?
    }
    async fn start(
        replies: Vec<Reply>,
        consuming: bool,
        follow_up: bool,
        session: Arc<str>,
    ) -> std::io::Result<Self> {
        if replies.len() > 8 || replies.iter().any(|r| r.text.len() > 20000) {
            return Err(std::io::Error::other("fixture reply ceiling"));
        }
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let shared = Arc::new(Shared {
            report: Mutex::new(Report {
                session,
                receipts: [None; 8],
                error: None,
                accepts: 0,
                prompt: None,
                follow_prompt: None,
                supervisor_joined: [false; 8],
            }),
            replies,
            consuming,
            follow_up,
            #[cfg(test)]
            padding: std::sync::atomic::AtomicUsize::new(0),
        });
        let owner = shared.clone();
        let (stop, mut stopped) = watch::channel(false);
        let (checkpoint, mut checkpoints) =
            mpsc::channel::<oneshot::Sender<Result<Report, &'static str>>>(1);
        let driver = tokio::spawn(async move {
            let mut handlers = JoinSet::new();
            let mut pending_checkpoint = None;
            let mut checkpoint_used = false;
            let mut accepts = 0;
            loop {
                if handlers.is_empty()
                    && let Some(reply) = pending_checkpoint.take()
                {
                    let report = owner.report.lock().unwrap().clone();
                    let valid = !checkpoint_used
                        && report.accepts == 4
                        && report.error.is_none()
                        && report.supervisor_joined[..4].iter().all(|v| *v)
                        && report.receipts[..4].iter().all(Option::is_some)
                        && report.receipts[4..].iter().all(Option::is_none);
                    checkpoint_used = true;
                    let _ = oneshot::Sender::send(
                        reply,
                        if valid {
                            Ok(report)
                        } else {
                            Err("unsafe fixture checkpoint")
                        },
                    );
                }
                tokio::select! {
                    biased;
                    _ = stopped.changed() => break,
                    request = checkpoints.recv(), if pending_checkpoint.is_none() => {
                        if let Some(reply) = request { pending_checkpoint = Some(reply); }
                    },
                    joined = handlers.join_next(), if !handlers.is_empty() => {
                        record_join(&owner, joined);
                    },
                    socket = listener.accept(), if accepts < 8 && handlers.len() < 2 => {
                        let Ok((socket, _)) = socket else { owner.report.lock().unwrap().error = Some("fixture accept failed"); break; };
                        accepts += 1;
                        owner.report.lock().unwrap().accepts = accepts;
                        let shared = owner.clone();
                        let mut stop = stopped.clone();
                        handlers.spawn(async move {
                            tokio::select! {
                                result = serve(socket, shared.clone()) => result,
                                _ = stop.changed() => Ok(None),
                            }
                        });
                    }
                }
            }
            drop(listener);
            while let Some(joined) = handlers.join_next().await {
                record_join(&owner, Some(joined));
            }
        });
        Ok(Self {
            address,
            shared,
            stop,
            driver,
            checkpoint,
            #[cfg(test)]
            checkpoint_gate: None,
        })
    }
    pub async fn shutdown(self) -> Result<Report, &'static str> {
        self.stop.send_replace(true);
        drop(self.checkpoint);
        self.driver.await.map_err(|_| "fixture supervisor panic")?;
        Ok(self.shared.report.lock().unwrap().clone())
    }
}
fn record_join(
    shared: &Shared,
    joined: Option<Result<Result<Option<usize>, &'static str>, tokio::task::JoinError>>,
) {
    let mut report = shared.report.lock().unwrap();
    match joined {
        Some(Ok(Ok(Some(index)))) => report.supervisor_joined[index] = true,
        Some(Ok(Ok(None))) => {}
        Some(Ok(Err(error))) => report.error = Some(error),
        _ => report.error = Some("fixture handler panic"),
    }
}
struct Text<const N: usize>(String);
impl<'de, const N: usize> serde::Deserialize<'de> for Text<N> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<const N: usize>;
        impl<const N: usize> serde::de::Visitor<'_> for V<N> {
            type Value = Text<N>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("bounded fixture string")
            }
            fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<Text<N>, E> {
                if text.len() > N {
                    return Err(E::custom("fixture string ceiling"));
                }
                Ok(Text(text.to_owned()))
            }
        }
        d.deserialize_str(V::<N>)
    }
}
struct Prior {
    prompt: String,
    summary: String,
    target: String,
    operation: u64,
    verified: bool,
}
struct Context {
    session: String,
    turn: u64,
    prompt: String,
    previous: String,
    invocation: Option<u64>,
    prior: Option<Prior>,
}
macro_rules! fixture_object {
    ($name:ident { $($field:ident: $wire:ty => $convert:expr),+ } $(optional { $($optional:ident: $optional_wire:ty),+ })?) => {
        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                struct V;
                impl<'de> serde::de::Visitor<'de> for V {
                    type Value = $name;
                    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("fixture object") }
                    fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> Result<$name, M::Error> {
                        $(let mut $field = None;)+
                        $($(let mut $optional = None;)+)?
                        while let Some(key) = map.next_key::<Text<16>>()? {
                            match key.0.as_str() {
                                $(stringify!($field) => {
                                    if $field.is_some() { return Err(serde::de::Error::custom("duplicate fixture field")); }
                                    let value: $wire = map.next_value()?;
                                    $field = Some(($convert)(value));
                                },)+
                                $($(stringify!($optional) => {
                                    if $optional.is_some() { return Err(serde::de::Error::custom("duplicate fixture field")); }
                                    $optional = Some(map.next_value::<$optional_wire>()?);
                                },)+)?
                                _ => return Err(serde::de::Error::custom("unknown fixture field")),
                            }
                        }
                        Ok($name { $($field: $field.ok_or_else(|| serde::de::Error::missing_field(stringify!($field)))?,)+ $($($optional,)+)? })
                    }
                }
                d.deserialize_map(V)
            }
        }
    }
}
fixture_object!(Prior {
    prompt: Text<1024> => |v: Text<1024>| v.0,
    summary: Text<1024> => |v: Text<1024>| v.0,
    target: Text<64> => |v: Text<64>| v.0,
    operation: u64 => |v| v,
    verified: bool => |v| v
});
fixture_object!(Context {
    session: Text<64> => |v: Text<64>| v.0,
    turn: u64 => |v| v,
    prompt: Text<1024> => |v: Text<1024>| v.0,
    previous: Text<1024> => |v: Text<1024>| v.0
} optional { invocation: u64, prior: Prior });
struct Captured {
    shared: Arc<Shared>,
    turn: usize,
}
impl Drop for Captured {
    fn drop(&mut self) {
        self.shared.report.lock().unwrap().receipts[self.turn]
            .as_mut()
            .unwrap()
            .joined = true;
    }
}
async fn serve(mut socket: TcpStream, shared: Arc<Shared>) -> Result<Option<usize>, &'static str> {
    let mut request = Vec::with_capacity(8192);
    loop {
        let mut byte = [0];
        if socket
            .read(&mut byte)
            .await
            .map_err(|_| "fixture read failed")?
            == 0
        {
            return Ok(None);
        }
        if request.len() == 4096 {
            return Err("fixture header ceiling");
        }
        request.push(byte[0]);
        if request.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let header = std::str::from_utf8(&request).map_err(|_| "fixture header UTF8")?;
    let value = |name: &str| {
        header.lines().find_map(|line| {
            line.split_once(':')
                .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.trim())
        })
    };
    let len: usize = value("Content-Length")
        .and_then(|v| v.parse().ok())
        .ok_or("fixture content length")?;
    let id = RequestId(
        value("X-Fixture-Id")
            .and_then(|v| v.parse().ok())
            .ok_or("fixture request identity")?,
    );
    if len > 4096 {
        return Err("fixture input ceiling");
    }
    let start = request.len();
    request.resize(start + len, 0);
    if socket.read_exact(&mut request[start..]).await.is_err() {
        return Ok(None);
    }
    let context: Context =
        serde_json::from_slice(&request[start..]).map_err(|_| "fixture strict context JSON")?;
    let turn = context.turn;
    if turn == 0 || turn > 8 {
        return Err("fixture turn identity");
    }
    let session = shared.report.lock().unwrap().session.clone();
    if context.session != session.as_ref() {
        return Err("fixture foreign session");
    }
    let index = turn as usize - 1;
    #[cfg(test)]
    let mut consumed_reply;
    #[cfg(not(test))]
    let consumed_reply;
    let reply = if shared.consuming {
        let prompt = context.prompt.as_str();
        scramjet_rs::CodingAgent::validate_prompt(&session, prompt)
            .map_err(|_| "fixture invalid prompt")?;
        let mut report = shared.report.lock().unwrap();
        if turn <= 4 && (context.invocation.is_some() || context.prior.is_some()) {
            return Err("fixture foreign first context");
        }
        let local_turn = if turn > 4 {
            if !shared.follow_up || context.invocation != Some(2) {
                return Err("fixture consuming turn ceiling");
            }
            let prior = context
                .prior
                .as_ref()
                .ok_or("fixture missing prior context")?;
            if report.prompt.as_deref() != Some(prior.prompt.as_str())
                || prior.summary != "Submitted disposable edit completed and verified."
                || prior.target != "sample.txt"
                || prior.operation != 1
                || !prior.verified
                || !report.supervisor_joined[..4].iter().all(|v| *v)
            {
                return Err("fixture prior context mismatch");
            }
            if turn == 5 && report.follow_prompt.is_none() {
                report.follow_prompt = Some(Arc::from(prompt));
            } else if turn == 5 || report.follow_prompt.as_deref() != Some(prompt) {
                return Err("fixture follow-up prompt mismatch");
            }
            turn - 4
        } else {
            if turn == 1 && report.prompt.is_none() {
                report.prompt = Some(Arc::from(prompt));
            } else if turn == 1 || report.prompt.as_deref() != Some(prompt) {
                return Err("fixture prompt mismatch");
            }
            turn
        };
        if turn > 1 && report.receipts[index - 1].is_none() {
            return Err("fixture order mismatch");
        }
        let before = if turn > 4 {
            report.prompt.as_deref().unwrap()
        } else {
            BEFORE
        };
        let previous = match local_turn {
            1 => "",
            2 => before,
            3 => "replacement durably completed",
            4 => "exact content verified",
            _ => unreachable!(),
        };
        if context.previous != previous {
            return Err("fixture previous mismatch");
        }
        let value = match local_turn {
            1 => serde_json::json!({"kind":"Read","op":turn,"target":"sample.txt"}),
            2 => {
                serde_json::json!({"kind":"Replace","op":turn,"target":"sample.txt","expected":before,"replacement":prompt})
            }
            3 => {
                serde_json::json!({"kind":"Verify","op":turn,"target":"sample.txt","expected":prompt})
            }
            _ => {
                serde_json::json!({"kind":"Final","summary":"Submitted disposable edit completed and verified."})
            }
        };
        consumed_reply = if shared.follow_up && turn > 4 {
            shared.replies.get((turn - 5) as usize).cloned()
        } else {
            None
        }
        .unwrap_or_else(|| Reply {
            text: value.to_string(),
            done: true,
            status: 200,
            usage: true,
        });
        #[cfg(test)]
        {
            let padding = shared.padding.load(std::sync::atomic::Ordering::Relaxed);
            if padding > consumed_reply.text.len() {
                consumed_reply.text.extend(std::iter::repeat_n(
                    ' ',
                    padding - consumed_reply.text.len(),
                ));
            }
        }
        &consumed_reply
    } else {
        shared.replies.get(index).ok_or("fixture missing reply")?
    };
    {
        let mut report = shared.report.lock().unwrap();
        if report.receipts[index].is_some()
            || report.receipts.iter().flatten().any(|r| r.request_id == id)
        {
            return Err("fixture duplicate identity");
        }
        report.receipts[index] = Some(Receipt {
            turn,
            request_id: id,
            cost: 1,
            joined: false,
            consumed: shared.consuming,
        });
    }
    let _captured = Captured {
        shared: shared.clone(),
        turn: index,
    };
    let mut body = String::with_capacity(reply.text.len() + 512);
    let mut start = 0;
    while start < reply.text.len() {
        let mut end = (start + 900).min(reply.text.len());
        while !reply.text.is_char_boundary(end) {
            end -= 1;
        }
        body.push_str("data: ");
        body.push_str(&reply.text[start..end]);
        body.push_str("\n\n");
        start = end;
    }
    if reply.done {
        body.push_str("data: [DONE]\n\n");
    }
    let usage = if reply.usage {
        format!(
            "X-Fixture-Usage-Id: {}\r\nX-Fixture-Usage-Tokens: 1\r\n",
            id.0
        )
    } else {
        String::new()
    };
    let header = format!(
        "HTTP/1.1 {} fixture\r\n{usage}Content-Length: {}\r\nConnection: close\r\n\r\n",
        reply.status,
        body.len()
    );
    // A peer cancellation can make response writes fail; the joined captured request still attests synthetic cost.
    let _ = socket.write_all(header.as_bytes()).await;
    let _ = socket.write_all(body.as_bytes()).await;
    let _ = socket.shutdown().await;
    Ok(Some(index))
}
#[allow(dead_code)]
pub fn correlate_checkpoint(
    observation: &CodingObservation,
    report: &Report,
) -> Result<(), &'static str> {
    if observation.invocation != 1
        || !observation.run_ended
        || !observation.verified
        || observation.final_summary.is_none()
        || observation.effect != Some(scramjet_rs::OperationState::Completed)
        || observation.decision != Some(scramjet_rs::CodingDecision::Approve)
        || observation.error.is_some()
        || report.error.is_some()
        || report.accepts != 4
        || !report.supervisor_joined[..4].iter().all(|v| *v)
        || report.receipts[4..].iter().any(Option::is_some)
        || observation.attempts[4..]
            .iter()
            .any(|s| s.request_id.is_some())
        || observation
            .proposal
            .as_ref()
            .is_none_or(|p| p.intent.session != report.session.as_ref())
    {
        return Err("unsafe/foreign fixture checkpoint");
    }
    for (index, slot) in observation.attempts[..4].iter().enumerate() {
        let receipt = report.receipts[index].ok_or("missing checkpoint receipt")?;
        let id = slot.request_id.ok_or("missing checkpoint request")?;
        let accounting = slot.accounting.ok_or("missing checkpoint accounting")?;
        if !slot.recorded
            || receipt.request_id != id
            || receipt.turn != index as u64 + 1
            || !receipt.joined
            || !receipt.consumed
            || receipt.cost != 1
            || accounting.request_id != id
            || accounting.unknown_usage
            || accounting.unresolved_exposure
        {
            return Err("uncorrelated checkpoint receipt/obligation");
        }
    }
    Ok(())
}
pub fn reconcile(observation: &CodingObservation, report: &Report) -> Result<(), &'static str> {
    if report.error.is_some()
        || observation
            .proposal
            .as_ref()
            .is_some_and(|p| p.intent.session != report.session.as_ref())
    {
        return Err("fixture join/invocation failed");
    }
    for (index, slot) in observation.attempts.iter().enumerate() {
        if report.receipts[index].is_some()
            && (slot.request_id.is_none() || !report.supervisor_joined[index])
        {
            return Err("extra/unsupervised fixture receipt");
        }
        let Some(id) = slot.request_id else {
            continue;
        };
        let accounting = slot
            .accounting
            .filter(|a| a.request_id == id)
            .ok_or("runtime receipt unavailable")?;
        if let Some(receipt) = report.receipts[index]
            && (receipt.turn != index as u64 + 1
                || receipt.request_id != id
                || receipt.cost != 1
                || !receipt.joined
                || receipt.consumed != report.prompt.is_some())
        {
            return Err("foreign/unjoined fixture receipt");
        }
        if accounting.unknown_usage || accounting.unresolved_exposure {
            let receipt = report.receipts[index].ok_or("unattested runtime obligation")?;
            if accounting.unknown_usage {
                Runtime::reconcile_usage_fixture(receipt.request_id, receipt.cost)?;
            }
            if accounting.unresolved_exposure {
                Runtime::reconcile_fixture(receipt.request_id)?;
            }
        }
    }
    Ok(())
}
