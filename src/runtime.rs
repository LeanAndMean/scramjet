use std::{
    collections::VecDeque,
    net::SocketAddr,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{Notify, mpsc, watch},
    task::{JoinHandle, JoinSet},
};

mod adaptive;
use adaptive::{Adaptive, Witness};
pub use adaptive::{ConcurrencyChange, ConcurrencyMode, ConcurrencySnapshot};
pub(crate) mod provider_history;
use crate::http::provider::ProviderResponse;
use crate::http::responses::{PROVIDER_ENVELOPE_BYTES, ProviderRequest};
use provider_history::ProviderLedger;
pub use provider_history::{
    PROVIDER_POST_BUDGET, ProviderAttemptFact, ProviderHistory, ProviderTerminalEvidence,
    reconstruct_provider_history,
};
mod tokens;
pub use tokens::{
    InvalidSyntheticTokens, MAX_SYNTHETIC_TOKENS, SyntheticCost, SyntheticTokenBudget,
    SyntheticUsage,
};
use tokens::{TokenBucket, UsageObligation};

pub const MAX_INPUT: usize = 4096;
pub const MAX_RECORD: usize = 1024;
pub const MAX_WIRE: usize = 131_072;
pub const MAX_BODY: usize = 65_536;
pub const MAX_HANDLES: usize = 12;
pub const MAX_PENDING: usize = 8;
pub const DELTA_CAPACITY: usize = 8;
const MAX_EXPOSURE: usize = 4;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static PROCESS_OWNER: Mutex<Option<Arc<Shared>>> = Mutex::new(None);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Success,
    HttpRejected(u16),
    Framing,
    Unsupported,
    Incomplete,
    Transport,
    ResourceLimit,
    Cancelled,
    ConsumerLost,
    Deadline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryWait {
    Delay(Duration),
    Unrepresentable,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RetryAfter {
    pub wait: Option<RetryWait>,
    pub missing: bool,
    pub duplicate: bool,
    pub valid: u8,
    pub invalid: u8,
    pub unsupported: u8,
}

#[derive(Clone, Debug)]
pub struct Terminal {
    pub id: RequestId,
    pub outcome: Outcome,
    pub body_bytes: usize,
    pub wire_bytes: usize,
    pub text_bytes: usize,
    pub retry_after: Option<RetryAfter>,
    pub synthetic_usage: SyntheticUsage,
    pub dispatch_to_head: Option<Duration>,
    pub commitment_to_terminal: Option<Duration>,
    pub output_backpressure: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestRate {
    burst: u8,
    interval: Duration,
}

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidRequestRate;

impl RequestRate {
    pub fn new(burst: u8, interval: Duration) -> Result<Self, InvalidRequestRate> {
        if !(1..=4).contains(&burst)
            || interval < Duration::from_millis(1)
            || interval > Duration::from_secs(86400)
        {
            return Err(InvalidRequestRate);
        }
        Ok(Self { burst, interval })
    }
}

struct Bucket {
    rate: RequestRate,
    available: u8,
    checkpoint: tokio::time::Instant,
}

impl Bucket {
    fn new(rate: RequestRate) -> Self {
        Self {
            rate,
            available: rate.burst,
            checkpoint: tokio::time::Instant::now(),
        }
    }

    fn refresh(&mut self, now: tokio::time::Instant) {
        let elapsed = now.saturating_duration_since(self.checkpoint);
        let earned = elapsed.as_nanos() / self.rate.interval.as_nanos();
        let missing = self.rate.burst - self.available;
        if earned >= u128::from(missing) {
            self.available = self.rate.burst;
            self.checkpoint = now;
        } else {
            self.available += earned as u8;
            let remainder = elapsed.as_nanos() % self.rate.interval.as_nanos();
            self.checkpoint = now
                - Duration::new(
                    (remainder / 1_000_000_000) as u64,
                    (remainder % 1_000_000_000) as u32,
                );
        }
    }

    fn wait(&self, now: tokio::time::Instant, tokens: u8) -> Duration {
        if tokens <= self.available {
            return Duration::ZERO;
        }
        (self.rate.interval * u32::from(tokens - self.available))
            .saturating_sub(now.saturating_duration_since(self.checkpoint))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct RuntimeBusy;

#[derive(Debug, PartialEq, Eq)]
pub enum SubmitError {
    InputTooLarge,
    CostTooLarge,
    Full,
    Shutdown,
    WrongRoute,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Snapshot {
    pub concurrency: ConcurrencySnapshot,
    pub active: usize,
    pub pending: usize,
    pub handles: usize,
    pub input_bytes: usize,
    pub delta_bytes: usize,
    pub delta_items: usize,
    pub potential: usize,
    pub unresolved: usize,
    pub active_peak: usize,
    pub pending_peak: usize,
    pub handles_peak: usize,
    pub input_peak: usize,
    pub delta_bytes_peak: usize,
    pub delta_items_peak: usize,
    pub attempts: usize,
    pub settled: usize,
    pub commitments: usize,
    pub request_rate: Option<RequestRate>,
    pub synthetic_budget: Option<SyntheticTokenBudget>,
    pub token_credit: u32,
    pub token_debt: u32,
    pub usage_obligations: usize,
    pub unknown_usage: usize,
    pub usage_peak: usize,
    pub exposure_peak: usize,
    pub synthetic_measured: Option<u64>,
    pub synthetic_charges: Option<u64>,
    pub rate_available: u8,
    pub rate_next_token: Duration,
    pub rate_full_refill: Duration,
    pub last_commitment: Option<tokio::time::Instant>,
    pub last_committed_id: Option<RequestId>,
    pub cooldown_remaining: Duration,
    pub cooldown_indefinite: bool,
    pub cooldown_signals: usize,
    pub retry_after_missing: usize,
    pub retry_after_duplicate: usize,
    pub retry_after_invalid: usize,
    pub retry_after_unsupported: usize,
}

pub struct Runtime {
    driver: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
}

pub struct RequestHandle {
    id: RequestId,
    deltas: Option<mpsc::Receiver<String>>,
    terminal: Option<mpsc::Receiver<Terminal>>,
    shared: Arc<Shared>,
}

pub struct ProviderHandle {
    handle: RequestHandle,
}
impl ProviderHandle {
    pub fn id(&self) -> RequestId {
        self.handle.id()
    }
    pub fn cancel(&self) {
        self.handle.cancel();
    }
    pub async fn result(&mut self) -> Option<ProviderResponse> {
        let terminal = self.handle.terminal().await?;
        let mut state = self.handle.shared.state.lock().unwrap();
        let slot = state.slots.iter_mut().find(|s| s.id == terminal.id)?;
        Some(slot.provider.take().unwrap_or(ProviderResponse {
            id: terminal.id,
            reservation: None,
            failure: None,
            outcome: terminal.outcome,
            envelope: None,
            usage: None,
            remote_terminal: None,
            durable: true,
            wire_bytes: terminal.wire_bytes,
        }))
    }
}

#[derive(Clone, Copy)]
enum AdmissionCost {
    Fixture(SyntheticCost),
    Provider,
}
impl AdmissionCost {
    fn total(self) -> u32 {
        match self {
            Self::Fixture(cost) => cost.total(),
            Self::Provider => 0,
        }
    }
}
impl From<SyntheticCost> for AdmissionCost {
    fn from(cost: SyntheticCost) -> Self {
        Self::Fixture(cost)
    }
}

struct Slot {
    provider: Option<ProviderResponse>,
    capture: Option<tokio::sync::oneshot::Sender<crate::ProviderCapture>>,
    id: RequestId,
    cost: AdmissionCost,
    phase: Phase,
    input: Option<String>,
    input_len: usize,
    output: Option<mpsc::Sender<String>>,
    terminal: Option<mpsc::Sender<Terminal>>,
    admitted: Option<tokio::time::Instant>,
    consumer_deadline: Option<tokio::time::Instant>,
    commitment_order: Option<usize>,
    witness: Option<Witness>,
    head_observed: bool,
    cancel: watch::Sender<Option<Outcome>>,
    held: bool,
    receivers_dropped: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Starting,
    Running,
    Pending,
    Complete,
}
struct Exposure {
    id: RequestId,
    dispatched: bool,
    unresolved: bool,
}
struct State {
    provider: Option<Arc<Mutex<ProviderLedger>>>,
    retained_unknown_through: Option<usize>,
    provider_dispatch: Option<tokio::time::Instant>,
    slots: Vec<Slot>,
    pending: VecDeque<RequestId>,
    exposure: Vec<Exposure>,
    configured_limit: usize,
    adaptive: Adaptive,
    shutdown: bool,
    stopped: bool,
    stats: Snapshot,
    cooldown: Option<(RetryWait, tokio::time::Instant)>,
    bucket: Bucket,
    tokens: TokenBucket,
    usage: Vec<UsageObligation>,
}

pub(crate) struct Shared {
    state: Mutex<State>,
    wake: Notify,
    #[cfg(test)]
    pub(crate) probe: Mutex<Option<Probe>>,
    #[cfg(test)]
    drop_probe: Mutex<Option<DropProbe>>,
}

#[cfg(test)]
struct DropProbe {
    reached: std::sync::mpsc::SyncSender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Checkpoint {
    BeforeTransport,
    ProviderBeforeDispatch,
    ProviderBeforeAcknowledgement,
    ConnectPending,
    ResponseHeaders,
    OutputBackpressure,
    BodyComplete,
    Settled,
}

#[cfg(test)]
pub(crate) struct Probe {
    pub(crate) checkpoint: Checkpoint,
    pub(crate) reached: mpsc::Sender<()>,
    pub(crate) release: Arc<Notify>,
}

impl Runtime {
    pub fn new(
        endpoint: SocketAddr,
        active_limit: usize,
        rate: RequestRate,
        budget: SyntheticTokenBudget,
        mode: ConcurrencyMode,
    ) -> Result<Self, RuntimeBusy> {
        Self::construct(endpoint, active_limit, rate, budget, mode, None, None)
    }

    pub async fn new_proxy(storage: &std::path::Path) -> std::io::Result<Self> {
        Self::new_proxy_retaining(storage, 0).await
    }

    pub async fn new_proxy_retaining(
        storage: &std::path::Path,
        through: usize,
    ) -> std::io::Result<Self> {
        let endpoint = crate::http::provider_metadata::check().await?;
        Self::provider_with_policy(endpoint, storage, Some(through))
    }

    pub fn new_provider(endpoint: SocketAddr, storage: &std::path::Path) -> std::io::Result<Self> {
        Self::provider_with_policy(endpoint, storage, None)
    }

    fn provider_with_policy(
        endpoint: SocketAddr,
        storage: &std::path::Path,
        retained: Option<usize>,
    ) -> std::io::Result<Self> {
        let ledger = ProviderLedger::open(storage, endpoint)?;
        let next = {
            let ledger = ledger.lock().unwrap();
            let history = ledger.history_ref();
            if retained.is_some_and(|through| {
                through > history.consumed() || history.has_unknown_after(through)
            }) {
                return Err(std::io::Error::other(
                    "provider uncertainty exceeds explicit retained boundary",
                ));
            }
            history
                .attempts
                .iter()
                .map(|a| a.request_id)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(|| std::io::Error::other("provider request identity exhausted"))?
        };
        NEXT_ID.fetch_max(next, Ordering::Relaxed);
        Self::construct(
            endpoint,
            1,
            RequestRate::new(1, Duration::from_secs(1)).unwrap(),
            SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
            ConcurrencyMode::Fixed,
            Some(ledger),
            retained,
        )
        .map_err(|_| std::io::Error::other("runtime owner busy"))
    }

    pub(crate) fn coding_provider(
        &self,
        endpoint: SocketAddr,
    ) -> std::io::Result<Arc<Mutex<ProviderLedger>>> {
        let state = self
            .shared
            .state
            .lock()
            .map_err(|_| std::io::Error::other("runtime accounting poisoned"))?;
        let ledger = state
            .provider
            .as_ref()
            .ok_or_else(|| std::io::Error::other("provider runtime required"))?;
        if state.retained_unknown_through.is_some()
            || state.shutdown
            || state.stopped
            || !state.slots.is_empty()
            || !state.pending.is_empty()
            || ledger
                .lock()
                .map_err(|_| std::io::Error::other("provider accounting poisoned"))?
                .binding()?
                .endpoint
                != endpoint
        {
            return Err(std::io::Error::other("current provider owner unavailable"));
        }
        Ok(ledger.clone())
    }

    pub fn provider_history(&self) -> Option<ProviderHistory> {
        self.shared
            .state
            .lock()
            .unwrap()
            .provider
            .as_ref()
            .map(|ledger| ledger.lock().unwrap().history())
    }

    pub fn try_provider_history(&self) -> Result<Option<ProviderHistory>, &'static str> {
        use std::sync::TryLockError;
        let state = match self.shared.state.try_lock() {
            Ok(state) => state,
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Poisoned(_)) => return Err("runtime accounting state poisoned"),
        };
        let Some(provider) = &state.provider else {
            return Ok(None);
        };
        let ledger = match provider.try_lock() {
            Ok(ledger) => ledger,
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Poisoned(_)) => return Err("provider accounting ledger poisoned"),
        };
        Ok(Some(ledger.history()))
    }

    fn construct(
        endpoint: SocketAddr,
        active_limit: usize,
        rate: RequestRate,
        budget: SyntheticTokenBudget,
        mode: ConcurrencyMode,
        provider: Option<Arc<Mutex<ProviderLedger>>>,
        retained_unknown_through: Option<usize>,
    ) -> Result<Self, RuntimeBusy> {
        assert!(endpoint.ip().is_loopback(), "endpoint must be loopback");
        assert!((1..=4).contains(&active_limit));
        let mut owner = PROCESS_OWNER.lock().unwrap();
        if let Some(shared) = owner.as_ref() {
            let mut state = shared.state.lock().unwrap();
            state.refresh_timing();
            if !state.tokens.full()
                || !state.usage.is_empty()
                || state.bucket.available != state.bucket.rate.burst
                || state.cooldown.is_some()
                || !state.stopped
                || !state.slots.is_empty()
                || !state.exposure.is_empty()
                || Arc::strong_count(shared) != 1
            {
                return Err(RuntimeBusy);
            }
        }
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                provider,
                retained_unknown_through,
                provider_dispatch: None,
                slots: Vec::with_capacity(MAX_HANDLES),
                pending: VecDeque::with_capacity(MAX_PENDING),
                exposure: Vec::with_capacity(MAX_EXPOSURE),
                configured_limit: active_limit,
                adaptive: Adaptive::new(mode, active_limit),
                shutdown: false,
                stopped: false,
                stats: Snapshot::default(),
                cooldown: None,
                bucket: Bucket::new(rate),
                tokens: TokenBucket::new(budget),
                usage: Vec::with_capacity(MAX_EXPOSURE),
            }),
            wake: Notify::new(),
            #[cfg(test)]
            probe: Mutex::new(None),
            #[cfg(test)]
            drop_probe: Mutex::new(None),
        });
        {
            let mut state = shared.state.lock().unwrap();
            state.refresh_timing();
            state.refresh();
        }
        let driver = tokio::spawn(schedule(shared.clone(), endpoint, active_limit));
        *owner = Some(shared.clone());
        Ok(Self {
            shared,
            driver: Some(driver),
        })
    }

    pub(crate) fn ownership(&self) -> Arc<Shared> {
        self.shared.clone()
    }

    pub fn process_snapshot() -> Snapshot {
        PROCESS_OWNER
            .lock()
            .unwrap()
            .as_ref()
            .map(|shared| {
                let mut state = shared.state.lock().unwrap();
                state.refresh_timing();
                state.stats
            })
            .unwrap_or_default()
    }

    pub fn submit(&self, input: &str, cost: SyntheticCost) -> Result<RequestHandle, SubmitError> {
        if self.shared.state.lock().unwrap().provider.is_some() {
            return Err(SubmitError::WrongRoute);
        }
        self.submit_input(input, AdmissionCost::Fixture(cost), MAX_INPUT, None, None)
    }

    pub fn submit_provider(&self, request: ProviderRequest) -> Result<ProviderHandle, SubmitError> {
        if self.shared.state.lock().unwrap().provider.is_none() {
            return Err(SubmitError::WrongRoute);
        }
        self.submit_input(
            &request.encoded,
            AdmissionCost::Provider,
            PROVIDER_ENVELOPE_BYTES,
            None,
            None,
        )
        .map(|handle| ProviderHandle { handle })
    }

    pub fn submit_provider_captured(
        &self,
        request: ProviderRequest,
    ) -> Result<(ProviderHandle, crate::ProviderCaptureReceiver), SubmitError> {
        if self.shared.state.lock().unwrap().provider.is_none() {
            return Err(SubmitError::WrongRoute);
        }
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let handle = self.submit_input(
            &request.encoded,
            AdmissionCost::Provider,
            PROVIDER_ENVELOPE_BYTES,
            Some(sender),
            None,
        )?;
        Ok((
            ProviderHandle { handle },
            crate::ProviderCaptureReceiver(receiver),
        ))
    }

    pub(crate) fn submit_provider_until(
        &self,
        request: ProviderRequest,
        deadline: tokio::time::Instant,
    ) -> Result<ProviderHandle, SubmitError> {
        if self.shared.state.lock().unwrap().provider.is_none() {
            return Err(SubmitError::WrongRoute);
        }
        self.submit_input(
            &request.encoded,
            AdmissionCost::Provider,
            PROVIDER_ENVELOPE_BYTES,
            None,
            Some(deadline),
        )
        .map(|handle| ProviderHandle { handle })
    }

    fn submit_input(
        &self,
        input: &str,
        cost: AdmissionCost,
        max_input: usize,
        capture: Option<tokio::sync::oneshot::Sender<crate::ProviderCapture>>,
        consumer_deadline: Option<tokio::time::Instant>,
    ) -> Result<RequestHandle, SubmitError> {
        if input.len() > max_input {
            return Err(SubmitError::InputTooLarge);
        }
        let mut state = self.shared.state.lock().unwrap();
        if cost.total() > state.tokens.config.capacity {
            return Err(SubmitError::CostTooLarge);
        }
        if state.shutdown {
            return Err(SubmitError::Shutdown);
        }
        let can_start = state.pending.is_empty() && state.can_start(cost);
        if state.slots.len() == MAX_HANDLES || (!can_start && state.pending.len() == MAX_PENDING) {
            return Err(SubmitError::Full);
        }
        let id = RequestId(
            NEXT_ID
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                .expect("request identity exhausted"),
        );
        let (output, deltas) = mpsc::channel(DELTA_CAPACITY);
        let (terminal_tx, terminal) = mpsc::channel(1);
        let (cancel, _) = watch::channel(None);
        let commitment = if can_start {
            Some(state.start(id, cost))
        } else {
            state.pending.push_back(id);
            None
        };
        let phase = if commitment.is_some() {
            Phase::Starting
        } else {
            Phase::Pending
        };
        let generation = state.adaptive.snapshot.generation;
        state.slots.push(Slot {
            provider: None,
            capture,
            id,
            cost,
            phase,
            input: Some(input.to_owned()),
            input_len: input.len(),
            output: Some(output),
            terminal: Some(terminal_tx),
            admitted: commitment.map(|(_, time)| time),
            consumer_deadline,
            commitment_order: commitment.map(|(order, _)| order),
            witness: commitment.map(|(_, time)| Witness {
                generation,
                committed: time,
                demand: false,
            }),
            head_observed: false,
            cancel,
            held: true,
            receivers_dropped: false,
        });
        state.stats.input_bytes += input.len();
        state.refresh();
        drop(state);
        self.shared.wake.notify_one();
        Ok(RequestHandle {
            id,
            shared: self.shared.clone(),
            deltas: Some(deltas),
            terminal: Some(terminal),
        })
    }

    pub(crate) fn active_limit(&self) -> usize {
        self.shared.state.lock().unwrap().configured_limit
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut state = self.shared.state.lock().unwrap();
        state.refresh_timing();
        state.stats
    }

    // The fixture owner must attest synthetic settlement; socket closure is not that evidence.
    pub fn reconcile_fixture(id: RequestId) -> Result<(), &'static str> {
        let owner = PROCESS_OWNER.lock().unwrap();
        let shared = owner.as_ref().ok_or("no fixture runtime")?;
        let mut state = shared.state.lock().unwrap();
        if state.provider.is_some() {
            return Err("provider obligations cannot be fixture-attested");
        }
        let Some(index) = state
            .exposure
            .iter()
            .position(|e| e.id == id && e.unresolved)
        else {
            return Err("unknown, unsettled, or already reconciled fixture request");
        };
        state.exposure.remove(index);
        state.refresh();
        drop(state);
        shared.wake.notify_one();
        Ok(())
    }

    // Final synthetic usage attestation is independent of remote exposure settlement.
    pub fn reconcile_usage_fixture(id: RequestId, actual: u32) -> Result<(), &'static str> {
        if actual > MAX_SYNTHETIC_TOKENS {
            return Err("invalid synthetic usage");
        }
        let owner = PROCESS_OWNER.lock().unwrap();
        let shared = owner.as_ref().ok_or("no fixture runtime")?;
        let mut state = shared.state.lock().unwrap();
        if state.provider.is_some() {
            return Err("provider obligations cannot be fixture-attested");
        }
        let index = state
            .usage
            .iter()
            .position(|u| u.id == id && u.terminal_unknown)
            .ok_or("unknown, active, or already reconciled usage")?;
        let usage = state.usage.remove(index);
        state.tokens.actual(usage.reserved, actual);
        state.refresh_timing();
        state.refresh();
        drop(state);
        shared.wake.notify_one();
        Ok(())
    }

    pub async fn shutdown(self) -> Snapshot {
        self.shutdown_checked()
            .await
            .expect("runtime supervisor failed")
    }

    pub(crate) async fn shutdown_checked(mut self) -> Result<Snapshot, &'static str> {
        self.stop();
        self.driver
            .take()
            .unwrap()
            .await
            .map_err(|_| "runtime supervisor failed")?;
        Ok(self.snapshot())
    }

    fn stop(&self) {
        let mut state = self.shared.state.lock().unwrap();
        state.shutdown = true;
        for slot in &state.slots {
            slot.cancel.send_replace(Some(Outcome::Cancelled));
        }
        drop(state);
        self.shared.wake.notify_one();
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop();
    }
}

impl RequestHandle {
    pub fn id(&self) -> RequestId {
        self.id
    }
    /// Admission order and monotonic charge time, retained only for this handle's lifetime.
    pub fn commitment(&self) -> Option<(usize, tokio::time::Instant)> {
        let state = self.shared.state.lock().unwrap();
        let slot = state.slots.iter().find(|s| s.id == self.id).unwrap();
        slot.commitment_order.zip(slot.admitted)
    }
    pub(crate) fn accounting_receipt(&self) -> Result<(bool, bool), &'static str> {
        let state = self
            .shared
            .state
            .lock()
            .map_err(|_| "runtime accounting poisoned")?;
        if !state
            .slots
            .iter()
            .any(|slot| slot.id == self.id && slot.phase == Phase::Complete)
        {
            return Err("request not complete");
        }
        Ok((
            state
                .usage
                .iter()
                .any(|usage| usage.id == self.id && usage.terminal_unknown),
            state
                .exposure
                .iter()
                .any(|exposure| exposure.id == self.id && exposure.unresolved),
        ))
    }
    pub fn cancel(&self) {
        let state = self.shared.state.lock().unwrap();
        if let Some(slot) = state.slots.iter().find(|s| s.id == self.id) {
            slot.cancel.send_if_modified(|reason| {
                if reason.is_none() {
                    *reason = Some(Outcome::Cancelled);
                    true
                } else {
                    false
                }
            });
        }
        drop(state);
        self.shared.wake.notify_one();
    }
    pub async fn next_delta(&mut self) -> Option<String> {
        std::future::poll_fn(|cx| {
            let mut state = self.shared.state.lock().unwrap();
            self.deltas.as_mut().unwrap().poll_recv(cx).map(|delta| {
                if let Some(text) = &delta {
                    state.stats.delta_items -= 1;
                    state.stats.delta_bytes -= text.len();
                }
                delta
            })
        })
        .await
    }
    pub async fn terminal(&mut self) -> Option<Terminal> {
        self.terminal.as_mut().unwrap().recv().await
    }
}

impl Drop for RequestHandle {
    fn drop(&mut self) {
        let deltas = self.deltas.as_mut().unwrap();
        deltas.close();
        let mut state = self.shared.state.lock().unwrap();
        while let Ok(delta) = deltas.try_recv() {
            state.stats.delta_bytes -= delta.len();
            state.stats.delta_items -= 1;
        }
        let slot = state.slots.iter_mut().find(|s| s.id == self.id).unwrap();
        slot.held = false;
        slot.cancel.send_replace(Some(Outcome::ConsumerLost));
        drop(state);
        self.shared.wake.notify_one();
        #[cfg(test)]
        if let Some(probe) = self.shared.drop_probe.lock().unwrap().take() {
            probe.reached.send(()).unwrap();
            probe.release.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        drop(self.deltas.take());
        drop(self.terminal.take());
        let mut state = self.shared.state.lock().unwrap();
        let index = state.slots.iter().position(|s| s.id == self.id).unwrap();
        state.slots[index].receivers_dropped = true;
        if state.slots[index].phase == Phase::Complete {
            state.slots.remove(index);
        }
        state.refresh();
        drop(state);
        self.shared.wake.notify_one();
    }
}

impl State {
    fn refresh_cooldown(&mut self) {
        let now = tokio::time::Instant::now();
        if let Some((RetryWait::Delay(remaining), checkpoint)) = &mut self.cooldown {
            *remaining = remaining.saturating_sub(now.saturating_duration_since(*checkpoint));
            *checkpoint = now;
            if remaining.is_zero() {
                self.cooldown = None;
            }
        }
        self.stats.cooldown_remaining = match self.cooldown {
            Some((RetryWait::Delay(remaining), _)) => remaining,
            _ => Duration::ZERO,
        };
        self.stats.cooldown_indefinite =
            matches!(self.cooldown, Some((RetryWait::Unrepresentable, _)));
    }

    fn refresh_timing(&mut self) {
        self.refresh_cooldown();
        let now = tokio::time::Instant::now();
        self.bucket.refresh(now);
        self.tokens.refresh(now);
        self.stats.concurrency = self.adaptive.snapshot;
        self.stats.synthetic_budget = self.provider.is_none().then_some(self.tokens.config);
        self.stats.token_credit = if self.provider.is_none() {
            self.tokens.credit
        } else {
            0
        };
        self.stats.token_debt = if self.provider.is_none() {
            self.tokens.debt
        } else {
            0
        };
        self.stats.synthetic_measured = self
            .provider
            .is_none()
            .then_some(self.tokens.measured)
            .flatten();
        self.stats.synthetic_charges = self
            .provider
            .is_none()
            .then_some(self.tokens.charges)
            .flatten();
        self.stats.request_rate = Some(self.bucket.rate);
        self.stats.rate_available = self.bucket.available;
        self.stats.rate_next_token = self.bucket.wait(now, 1);
        self.stats.rate_full_refill = self.bucket.wait(now, self.bucket.rate.burst);
    }

    fn timing_wait(&mut self) -> Option<Duration> {
        self.refresh_timing();
        if self.stats.active >= self.adaptive.snapshot.target
            || self.exposure.len() == MAX_EXPOSURE
            || self.usage.len() == MAX_EXPOSURE
        {
            return None;
        }
        match self.cooldown {
            Some((RetryWait::Unrepresentable, _)) => None,
            _ => {
                let wait = self
                    .stats
                    .cooldown_remaining
                    .max(self.stats.rate_next_token)
                    .max(self.pending.front().map_or(Duration::ZERO, |id| {
                        let cost = self.slots.iter().find(|s| s.id == *id).unwrap().cost;
                        self.tokens.wait(tokio::time::Instant::now(), cost.total())
                    }));
                (!wait.is_zero()).then_some(wait)
            }
        }
    }

    fn can_start(&mut self, cost: impl Into<AdmissionCost>) -> bool {
        let cost = cost.into();
        self.refresh_timing();
        if let Some(ledger) = &self.provider {
            let Ok(ledger) = ledger.try_lock() else {
                return false;
            };
            if !ledger.eligible()
                || self
                    .retained_unknown_through
                    .is_some_and(|through| ledger.history_ref().has_unknown_after(through))
            {
                return false;
            }
        }
        self.bucket.available > 0
            && self.tokens.debt == 0
            && self.tokens.credit >= cost.total()
            && self.usage.len() < MAX_EXPOSURE
            && self.cooldown.is_none()
            && self.stats.active < self.adaptive.snapshot.target
            && self.exposure.len() < MAX_EXPOSURE
    }

    fn feedback(&mut self, feedback: RetryAfter, observed: tokio::time::Instant) {
        self.refresh_cooldown();
        self.stats.retry_after_missing = self
            .stats
            .retry_after_missing
            .saturating_add(usize::from(feedback.missing));
        self.stats.retry_after_duplicate = self
            .stats
            .retry_after_duplicate
            .saturating_add(usize::from(feedback.duplicate));
        self.stats.retry_after_invalid = self
            .stats
            .retry_after_invalid
            .saturating_add(usize::from(feedback.invalid));
        self.stats.retry_after_unsupported = self
            .stats
            .retry_after_unsupported
            .saturating_add(usize::from(feedback.unsupported));
        if let Some(wait) = feedback.wait {
            self.stats.cooldown_signals = self.stats.cooldown_signals.saturating_add(1);
            let wait = match wait {
                RetryWait::Delay(delay) => RetryWait::Delay(delay.saturating_sub(
                    tokio::time::Instant::now().saturating_duration_since(observed),
                )),
                wait => wait,
            };
            let wait = match (self.cooldown, wait) {
                (Some((RetryWait::Unrepresentable, _)), _) | (_, RetryWait::Unrepresentable) => {
                    RetryWait::Unrepresentable
                }
                (Some((RetryWait::Delay(a), _)), RetryWait::Delay(b)) => RetryWait::Delay(a.max(b)),
                (None, wait) => wait,
            };
            self.cooldown = Some((wait, tokio::time::Instant::now()));
            self.refresh_cooldown();
        }
    }

    fn demand_eligible(&self) -> bool {
        self.stats.active == self.adaptive.snapshot.target
            && self.pending.front().is_some_and(|next| {
                let cost = self.slots.iter().find(|s| s.id == *next).unwrap().cost;
                self.bucket.available > 0
                    && self.tokens.debt == 0
                    && self.tokens.credit >= cost.total()
                    && self.cooldown.is_none()
                    && self.exposure.len() < MAX_EXPOSURE
                    && self.usage.len() < MAX_EXPOSURE
            })
    }

    fn start(&mut self, id: RequestId, cost: AdmissionCost) -> (usize, tokio::time::Instant) {
        if self.provider.is_none() {
            self.tokens.reserve(cost.total());
        }
        self.usage.push(UsageObligation {
            id,
            reserved: cost.total(),
            terminal_unknown: false,
        });
        self.bucket.available -= 1;
        self.stats.last_commitment = Some(tokio::time::Instant::now());
        self.stats.last_committed_id = Some(id);
        self.stats.commitments += 1;
        self.stats.active += 1;
        self.exposure.push(Exposure {
            id,
            dispatched: false,
            unresolved: false,
        });
        (self.stats.commitments, self.stats.last_commitment.unwrap())
    }
    fn refresh(&mut self) {
        self.stats.pending = self.pending.len();
        self.stats.handles = self.slots.len();
        if let Some(ledger) = &self.provider {
            if let Ok(ledger) = ledger.try_lock() {
                let history = ledger.history_ref();
                let slots = &self.slots;
                let past = |attempt: &&ProviderAttemptFact| {
                    !slots.iter().any(|slot| {
                        slot.id.0 == attempt.request_id
                            && matches!(slot.phase, Phase::Starting | Phase::Running)
                    })
                };
                self.stats.unresolved = history
                    .attempts
                    .iter()
                    .filter(past)
                    .filter(|a| a.terminal.is_none())
                    .count();
                self.stats.unknown_usage = history
                    .attempts
                    .iter()
                    .filter(past)
                    .filter(|a| a.usage.as_ref().is_none_or(|u| !u.complete()))
                    .count();
                self.stats.usage_obligations = self.stats.unknown_usage + self.stats.active;
                self.stats.potential = self.stats.active;
                self.stats.usage_peak = self.stats.usage_peak.max(self.stats.usage_obligations);
                self.stats.exposure_peak = self
                    .stats
                    .exposure_peak
                    .max(self.stats.unresolved + self.stats.potential);
            }
        } else {
            self.stats.usage_obligations = self.usage.len();
            self.stats.usage_peak = self.stats.usage_peak.max(self.usage.len());
            self.stats.exposure_peak = self.stats.exposure_peak.max(self.exposure.len());
            self.stats.unknown_usage = self.usage.iter().filter(|u| u.terminal_unknown).count();
            self.stats.unresolved = self.exposure.iter().filter(|e| e.unresolved).count();
            self.stats.potential = self.exposure.len() - self.stats.unresolved;
        }
        self.stats.active_peak = self.stats.active_peak.max(self.stats.active);
        self.stats.pending_peak = self.stats.pending_peak.max(self.stats.pending);
        self.stats.handles_peak = self.stats.handles_peak.max(self.stats.handles);
        self.stats.input_peak = self.stats.input_peak.max(self.stats.input_bytes);
        self.stats.delta_bytes_peak = self.stats.delta_bytes_peak.max(self.stats.delta_bytes);
        self.stats.delta_items_peak = self.stats.delta_items_peak.max(self.stats.delta_items);
    }
    fn finish(&mut self, mut terminal: Terminal, observed: bool) {
        if let Some(index) = self.usage.iter().position(|u| u.id == terminal.id) {
            let dispatched = self
                .exposure
                .iter()
                .find(|e| e.id == terminal.id)
                .unwrap()
                .dispatched;
            if !dispatched {
                terminal.synthetic_usage = SyntheticUsage::NotAttempted;
            }
            if self.provider.is_some() {
                self.usage.remove(index);
            } else {
                match terminal.synthetic_usage {
                    SyntheticUsage::Known(actual) => {
                        let usage = self.usage.remove(index);
                        if self.tokens.actual(usage.reserved, actual)
                            && terminal.outcome == Outcome::Success
                        {
                            terminal.outcome = Outcome::ResourceLimit;
                        }
                    }
                    SyntheticUsage::NotAttempted => {
                        self.usage.remove(index);
                    }
                    SyntheticUsage::Unknown => self.usage[index].terminal_unknown = true,
                }
            }
        }
        let index = self.slots.iter().position(|s| s.id == terminal.id).unwrap();
        terminal.commitment_to_terminal = self.slots[index]
            .admitted
            .map(|t| tokio::time::Instant::now().saturating_duration_since(t));
        self.adaptive.finish(
            &terminal,
            self.slots[index].witness,
            tokio::time::Instant::now(),
        );
        let slot = &mut self.slots[index];
        if let Some(provider) = &mut slot.provider
            && let Some(reason) = slot.cancel.borrow().clone()
        {
            provider.outcome = reason.clone();
            provider.envelope = None;
            terminal.outcome = reason;
        }
        let was_pending = slot.phase == Phase::Pending;
        self.stats.input_bytes -= slot.input_len;
        slot.input = None;
        slot.input_len = 0;
        slot.phase = Phase::Complete;
        slot.capture.take();
        let _ = slot.terminal.take().unwrap().try_send(terminal.clone());
        slot.output.take();
        if slot.receivers_dropped {
            self.slots.remove(index);
        }
        if was_pending {
            self.pending.retain(|id| *id != terminal.id);
        } else {
            self.stats.active -= 1;
            let e = self
                .exposure
                .iter()
                .position(|e| e.id == terminal.id)
                .unwrap();
            if observed || !self.exposure[e].dispatched {
                self.exposure.remove(e);
            } else {
                self.exposure[e].unresolved = true;
            }
        }
        self.stats.settled += 1;
        self.refresh_timing();
        self.refresh();
    }
}

impl Shared {
    pub(crate) fn head(
        &self,
        id: RequestId,
        status: u16,
        feedback: Option<RetryAfter>,
        observed: tokio::time::Instant,
    ) {
        let mut state = self.state.lock().unwrap();
        let slot = state.slots.iter_mut().find(|s| s.id == id).unwrap();
        if !slot.head_observed {
            slot.head_observed = true;
            state.adaptive.head(status, tokio::time::Instant::now());
            if let Some(feedback) = feedback {
                state.feedback(feedback, observed);
            }
            state.refresh_timing();
        }
        drop(state);
        self.wake.notify_one();
    }

    #[cfg(test)]
    pub(crate) fn feedback(&self, feedback: RetryAfter, observed: tokio::time::Instant) {
        self.state.lock().unwrap().feedback(feedback, observed);
        self.wake.notify_one();
    }

    #[cfg(test)]
    pub(crate) async fn checkpoint(&self, checkpoint: Checkpoint) {
        let probe = {
            let mut slot = self.probe.lock().unwrap();
            if slot
                .as_ref()
                .is_some_and(|probe| probe.checkpoint == checkpoint)
            {
                slot.take()
            } else {
                None
            }
        };
        if let Some(probe) = probe {
            probe.reached.try_send(()).unwrap();
            probe.release.notified().await;
        }
    }

    pub(crate) fn provider_dispatch_delay(&self) -> Duration {
        let mut state = self.state.lock().unwrap();
        let now = tokio::time::Instant::now();
        let delay = state.provider_dispatch.map_or(Duration::ZERO, |last| {
            Duration::from_secs(1).saturating_sub(now.saturating_duration_since(last))
        });
        if delay.is_zero() {
            state.provider_dispatch = Some(now);
        }
        delay
    }

    pub(crate) fn dispatched(&self, id: RequestId) {
        let mut state = self.state.lock().unwrap();
        state
            .exposure
            .iter_mut()
            .find(|e| e.id == id)
            .unwrap()
            .dispatched = true;
        state.stats.attempts += 1;
    }
    pub(crate) fn emit(
        &self,
        id: RequestId,
        permit: mpsc::Permit<'_, String>,
        text: String,
    ) -> Result<(), Outcome> {
        let mut state = self.state.lock().unwrap();
        if !state.slots.iter().find(|s| s.id == id).unwrap().held {
            return Err(Outcome::ConsumerLost);
        }
        state.stats.delta_items += 1;
        state.stats.delta_bytes += text.len();
        permit.send(text);
        state.refresh();
        Ok(())
    }
}

async fn schedule(shared: Arc<Shared>, endpoint: SocketAddr, _active_limit: usize) {
    let mut tasks = JoinSet::new();
    loop {
        let notified = shared.wake.notified();
        let expiry;
        {
            let mut state = shared.state.lock().unwrap();
            let cancelled: Vec<_> = state
                .slots
                .iter()
                .filter(|s| s.phase == Phase::Pending)
                .filter_map(|s| s.cancel.borrow().clone().map(|reason| (s.id, reason)))
                .collect();
            for (id, outcome) in cancelled {
                state.finish(
                    Terminal {
                        id,
                        outcome,
                        body_bytes: 0,
                        wire_bytes: 0,
                        text_bytes: 0,
                        retry_after: None,
                        synthetic_usage: SyntheticUsage::NotAttempted,
                        dispatch_to_head: None,
                        commitment_to_terminal: None,
                        output_backpressure: false,
                    },
                    false,
                );
            }
            while !state.shutdown {
                let Some(&id) = state.pending.front() else {
                    break;
                };
                let cost = state.slots.iter().find(|s| s.id == id).unwrap().cost;
                if !state.can_start(cost) {
                    break;
                }
                state.pending.pop_front();
                let (order, time) = state.start(id, cost);
                let demand = state.demand_eligible();
                let witness = Witness {
                    generation: state.adaptive.snapshot.generation,
                    committed: time,
                    demand,
                };
                let slot = state.slots.iter_mut().find(|s| s.id == id).unwrap();
                slot.phase = Phase::Starting;
                slot.admitted = Some(time);
                slot.commitment_order = Some(order);
                slot.witness = Some(witness);
            }
            let provider = state.provider.clone();
            for slot in &mut state.slots {
                if slot.phase != Phase::Starting {
                    continue;
                }
                slot.phase = Phase::Running;
                let input = slot.input.take().unwrap();
                let id = slot.id;
                let output = slot.output.as_ref().unwrap().clone();
                let attempt_deadline = slot.admitted.unwrap()
                    + Duration::from_secs(if provider.is_some() { 30 } else { 5 });
                let deadline = slot
                    .consumer_deadline
                    .map_or(attempt_deadline, |deadline| deadline.min(attempt_deadline));
                let cancel = slot.cancel.subscribe();
                let shared = shared.clone();
                let provider = provider.clone();
                let capture = slot.capture.take();
                tasks.spawn(async move {
                    #[cfg(test)]
                    shared.checkpoint(Checkpoint::BeforeTransport).await;
                    if let Some(ledger) = provider {
                        crate::http::provider::attempt(
                            endpoint, id, input, cancel, &shared, deadline, ledger, capture,
                        )
                        .await
                    } else {
                        let (terminal, observed) = crate::http::attempt(
                            endpoint, id, input, output, cancel, &shared, deadline,
                        )
                        .await;
                        (terminal, observed, None)
                    }
                });
            }
            state.refresh();
            expiry = if !state.shutdown && !state.pending.is_empty() {
                state.timing_wait().and_then(|remaining| {
                    tokio::time::Instant::now()
                        .checked_add(remaining.min(Duration::from_secs(86400)))
                })
            } else {
                None
            };
            if state.shutdown
                && state.stats.active == 0
                && state.pending.is_empty()
                && tasks.is_empty()
            {
                break;
            }
        }
        let expiry_wait = async {
            if let Some(expiry) = expiry {
                tokio::time::sleep_until(expiry).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        tokio::select! {
            _ = notified => {},
            _ = expiry_wait => {},
            joined = tasks.join_next(), if !tasks.is_empty() => {
                let (terminal, observed, provider) = joined.unwrap().expect("request task failed");
                let mut state = shared.state.lock().unwrap();
                state.slots.iter_mut().find(|s| s.id == terminal.id).unwrap().provider = provider;
                state.finish(terminal, observed);
            }
        }
    }
    drop(tasks);
    shared.state.lock().unwrap().stopped = true;
}

#[cfg(test)]
mod adaptive_tests;
#[cfg(test)]
mod cooldown_tests;
#[cfg(test)]
mod pacing_tests;
#[cfg(all(test, target_os = "linux"))]
mod provider_observation_tests;
#[cfg(test)]
mod provider_tests;
#[cfg(test)]
mod token_tests;

#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod support;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) static TEST_OWNER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    pub(crate) fn abort_supervisor(runtime: &Runtime) {
        runtime.driver.as_ref().unwrap().abort();
    }

    pub(super) fn probe(
        runtime: &Runtime,
        checkpoint: Checkpoint,
    ) -> (mpsc::Receiver<()>, Arc<Notify>) {
        let (reached, receiver) = mpsc::channel(1);
        let release = Arc::new(Notify::new());
        *runtime.shared.probe.lock().unwrap() = Some(Probe {
            checkpoint,
            reached,
            release: release.clone(),
        });
        (receiver, release)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn exact_handle_receipt_keeps_usage_and_exposure_independent() {
        let _owner = TEST_OWNER.lock().await;
        for unknown in [false, true] {
            for unresolved in [false, true] {
                tokio::time::sleep(Duration::from_millis(5)).await;
                let runtime = Runtime::new(
                    "127.0.0.1:9".parse().unwrap(),
                    2,
                    RequestRate::new(4, Duration::from_millis(1)).unwrap(),
                    SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
                    ConcurrencyMode::Fixed,
                )
                .unwrap();
                let mut handle = runtime
                    .submit("receipt", SyntheticCost::new(1, 1).unwrap())
                    .unwrap();
                assert!(handle.accounting_receipt().is_err());
                let id = handle.id();
                {
                    let mut state = runtime.shared.state.lock().unwrap();
                    state.pending.retain(|pending| *pending != id);
                    state
                        .slots
                        .iter_mut()
                        .find(|slot| slot.id == id)
                        .unwrap()
                        .phase = Phase::Running;
                    state
                        .exposure
                        .iter_mut()
                        .find(|exposure| exposure.id == id)
                        .unwrap()
                        .dispatched = true;
                    state.finish(
                        Terminal {
                            id,
                            outcome: Outcome::Success,
                            text_bytes: 0,
                            body_bytes: 0,
                            wire_bytes: 0,
                            retry_after: None,
                            synthetic_usage: if unknown {
                                SyntheticUsage::Unknown
                            } else {
                                SyntheticUsage::Known(1)
                            },
                            dispatch_to_head: None,
                            commitment_to_terminal: None,
                            output_backpressure: false,
                        },
                        !unresolved,
                    );
                }
                assert_eq!(handle.terminal().await.unwrap().id, id);
                assert!(handle.terminal().await.is_none());
                assert_eq!(handle.accounting_receipt().unwrap(), (unknown, unresolved));
                if unknown {
                    Runtime::reconcile_usage_fixture(id, 1).unwrap();
                }
                if unresolved {
                    Runtime::reconcile_fixture(id).unwrap();
                }
                assert_eq!(handle.accounting_receipt().unwrap(), (false, false));
                drop(handle);
                let snapshot = runtime.shutdown().await;
                assert_eq!(
                    (
                        snapshot.handles,
                        snapshot.unresolved,
                        snapshot.unknown_usage
                    ),
                    (0, 0, 0)
                );
            }
        }
        let refill = Runtime::process_snapshot()
            .rate_full_refill
            .max(Duration::from_millis(1));
        tokio::time::sleep(refill).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn completion_during_handle_drop_keeps_lease_until_receivers_are_destroyed() {
        let _owner = TEST_OWNER.lock().await;
        let fixture = support::Fixture::new().await;
        let runtime = Runtime::new(
            fixture.address,
            4,
            crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
            crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
                .unwrap(),
            crate::ConcurrencyMode::Fixed,
        )
        .unwrap();
        let mut handles: Vec<_> = (0..12)
            .map(|_| {
                runtime
                    .submit("slow", crate::SyntheticCost::new(1, 1).unwrap())
                    .unwrap()
            })
            .collect();
        tokio::time::timeout(Duration::from_secs(3), async {
            while runtime.snapshot().delta_items != 32 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let ids: Vec<_> = handles.iter().map(RequestHandle::id).collect();
        let handle = handles.remove(0);
        for handle in &handles {
            handle.cancel();
        }
        let (reached_tx, reached) = std::sync::mpsc::sync_channel(1);
        let (release, release_rx) = std::sync::mpsc::sync_channel(1);
        *runtime.shared.drop_probe.lock().unwrap() = Some(DropProbe {
            reached: reached_tx,
            release: release_rx,
        });
        let dropping = std::thread::spawn(move || drop(handle));
        let settled = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match reached.try_recv() {
                    Ok(()) => break,
                    Err(std::sync::mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                    Err(error) => panic!("drop checkpoint disconnected: {error}"),
                }
            }
            while runtime.snapshot().settled != 12 {
                tokio::task::yield_now().await;
            }
        })
        .await;
        let state = runtime.snapshot();
        let submitted = runtime.submit("ok", crate::SyntheticCost::new(1, 1).unwrap());
        release.send(()).unwrap();
        dropping.join().unwrap();
        settled.unwrap();
        assert!(matches!(submitted, Err(SubmitError::Full)));
        assert_eq!(
            (
                state.handles,
                state.active,
                state.pending,
                state.input_bytes
            ),
            (12, 0, 0, 0)
        );
        assert_eq!(
            (state.delta_items, state.delta_bytes, state.unresolved),
            (24, 24_576, 4)
        );
        assert_eq!(runtime.snapshot().handles, 11);
        let mut next = runtime
            .submit("ok", crate::SyntheticCost::new(1, 1).unwrap())
            .unwrap();
        next.cancel();
        assert_eq!(next.terminal().await.unwrap().outcome, Outcome::Cancelled);
        assert!(next.terminal().await.is_none());
        drop(next);
        for handle in &mut handles {
            assert_eq!(handle.terminal().await.unwrap().outcome, Outcome::Cancelled);
            assert!(handle.terminal().await.is_none());
        }
        for id in &ids[..4] {
            fixture.wait_settled(*id).await;
            Runtime::reconcile_fixture(*id).unwrap();
        }
        assert_eq!(fixture.started().len(), 4);
        drop(handles);
        let state = runtime.shutdown().await;
        assert_eq!(
            (
                state.handles,
                state.active,
                state.delta_bytes,
                state.unresolved
            ),
            (0, 0, 0, 0)
        );
        fixture.shutdown().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancellation_after_pending_connect_poll_never_dispatches() {
        let _owner = TEST_OWNER.lock().await;
        let fixture = support::Fixture::new().await;
        let runtime = Runtime::new(
            fixture.address,
            4,
            crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
            crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
                .unwrap(),
            crate::ConcurrencyMode::Fixed,
        )
        .unwrap();
        let (mut reached, _release) = probe(&runtime, Checkpoint::ConnectPending);
        let mut handle = runtime
            .submit("ok", crate::SyntheticCost::new(1, 1).unwrap())
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), reached.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(runtime.snapshot().attempts, 0);
        handle.cancel();
        assert_eq!(handle.terminal().await.unwrap().outcome, Outcome::Cancelled);
        assert!(fixture.started().is_empty());
        drop(handle);
        assert_eq!(runtime.shutdown().await.unresolved, 0);
        fixture.shutdown().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancellation_before_observation_wins_even_with_complete_http_body() {
        let _owner = TEST_OWNER.lock().await;
        let fixture = support::Fixture::new().await;
        let runtime = Runtime::new(
            fixture.address,
            4,
            crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
            crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
                .unwrap(),
            crate::ConcurrencyMode::Fixed,
        )
        .unwrap();
        let (mut reached, _release) = probe(&runtime, Checkpoint::BodyComplete);
        let mut handle = runtime
            .submit("ok", crate::SyntheticCost::new(1, 1).unwrap())
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), reached.recv())
            .await
            .unwrap()
            .unwrap();
        handle.cancel();
        let terminal = handle.terminal().await.unwrap();
        assert_eq!(terminal.outcome, Outcome::Cancelled);
        assert_eq!(terminal.synthetic_usage, SyntheticUsage::Unknown);
        assert!(handle.terminal().await.is_none());
        assert_eq!(runtime.snapshot().unknown_usage, 1);
        assert_eq!(runtime.snapshot().unresolved, 1);
        fixture.wait_settled(handle.id()).await;
        Runtime::reconcile_fixture(handle.id()).unwrap();
        drop(handle);
        assert_eq!(runtime.shutdown().await.unresolved, 0);
        fixture.shutdown().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn protocol_observation_wins_cancel_during_teardown_exactly_once() {
        let _owner = TEST_OWNER.lock().await;
        for scenario in ["hold", "reject"] {
            let fixture = support::Fixture::new().await;
            let runtime = Runtime::new(
                fixture.address,
                4,
                crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                crate::SyntheticTokenBudget::new(
                    65_536,
                    65_536,
                    std::time::Duration::from_millis(1),
                )
                .unwrap(),
                crate::ConcurrencyMode::Fixed,
            )
            .unwrap();
            let (mut reached, release) = probe(&runtime, Checkpoint::Settled);
            let mut handle = runtime
                .submit(scenario, crate::SyntheticCost::new(1, 1).unwrap())
                .unwrap();
            fixture.wait_started(1).await;
            fixture.release(1);
            tokio::time::timeout(Duration::from_secs(2), reached.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(runtime.snapshot().active, 1);
            handle.cancel();
            release.notify_one();
            let terminal = handle.terminal().await.unwrap();
            assert_eq!(
                terminal.outcome,
                if scenario == "reject" {
                    Outcome::HttpRejected(429)
                } else {
                    Outcome::Success
                }
            );
            assert!(handle.terminal().await.is_none());
            fixture.wait_settled(handle.id()).await;
            assert_eq!(runtime.snapshot().unresolved, 0);
            assert_eq!(runtime.snapshot().settled, 1);
            assert!(fixture.peak() <= 1);
            assert!(fixture.fd_peak() > 0);
            while handle.next_delta().await.is_some() {}
            drop(handle);
            runtime.shutdown().await;
            fixture.shutdown().await;
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn expired_admission_deadline_cannot_dispatch_ready_work() {
        let _owner = TEST_OWNER.lock().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let runtime = Runtime::new(
            listener.local_addr().unwrap(),
            4,
            crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
            crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
                .unwrap(),
            crate::ConcurrencyMode::Fixed,
        )
        .unwrap();
        let mut handle = runtime
            .submit("ok", crate::SyntheticCost::new(1, 1).unwrap())
            .unwrap();
        runtime.shared.state.lock().unwrap().slots[0].admitted =
            Some(tokio::time::Instant::now() - Duration::from_secs(6));
        assert_eq!(handle.terminal().await.unwrap().outcome, Outcome::Deadline);
        assert_eq!(runtime.snapshot().attempts, 0);
        drop(handle);
        assert_eq!(runtime.shutdown().await.unresolved, 0);
    }
}
