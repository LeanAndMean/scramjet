use super::{Outcome, SyntheticUsage, Terminal};
use std::time::Duration;
use tokio::time::Instant;

const HOLD: Duration = Duration::from_millis(250);
const FRESH: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConcurrencyMode {
    #[default]
    Fixed,
    Adaptive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConcurrencyChange {
    Refusal,
    Proxy,
    Recovery,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ConcurrencySnapshot {
    pub mode: ConcurrencyMode,
    pub ceiling: usize,
    pub target: usize,
    pub generation: Option<u64>,
    pub recovery_samples: u8,
    pub refusals: Option<u64>,
    pub proxy_errors: Option<u64>,
    pub other_statuses: Option<u64>,
    pub deadlines: Option<u64>,
    pub transport_errors: Option<u64>,
    pub cancellations: Option<u64>,
    pub framing_errors: Option<u64>,
    pub incomplete_responses: Option<u64>,
    pub resource_failures: Option<u64>,
    pub consumer_losses: Option<u64>,
    pub ineligible_successes: Option<u64>,
    pub decreases: Option<u64>,
    pub increases: Option<u64>,
    pub last_change: Option<(Instant, ConcurrencyChange)>,
}

#[derive(Clone, Copy)]
pub(super) struct Witness {
    pub generation: Option<u64>,
    pub committed: Instant,
    pub demand: bool,
}

pub(super) struct Adaptive {
    pub snapshot: ConcurrencySnapshot,
    last_pressure: Option<Instant>,
    last_decrease: Option<Instant>,
    last_sample: Option<Instant>,
}

fn count(value: &mut Option<u64>) {
    *value = value.and_then(|n| n.checked_add(1));
}

impl Adaptive {
    pub fn new(mode: ConcurrencyMode, ceiling: usize) -> Self {
        Self {
            snapshot: ConcurrencySnapshot {
                mode,
                ceiling,
                target: ceiling,
                generation: Some(0),
                refusals: Some(0),
                proxy_errors: Some(0),
                other_statuses: Some(0),
                deadlines: Some(0),
                transport_errors: Some(0),
                cancellations: Some(0),
                framing_errors: Some(0),
                incomplete_responses: Some(0),
                resource_failures: Some(0),
                consumer_losses: Some(0),
                ineligible_successes: Some(0),
                decreases: Some(0),
                increases: Some(0),
                ..ConcurrencySnapshot::default()
            },
            last_pressure: None,
            last_decrease: None,
            last_sample: None,
        }
    }

    fn reset(&mut self) {
        self.snapshot.recovery_samples = 0;
        self.last_sample = None;
    }

    fn change(&mut self, target: usize, now: Instant, reason: ConcurrencyChange) {
        self.snapshot.target = target;
        self.snapshot.generation = self.snapshot.generation.and_then(|n| n.checked_add(1));
        self.snapshot.last_change = Some((now, reason));
        self.reset();
    }

    pub fn head(&mut self, status: u16, now: Instant) {
        let reason = match status {
            429 => {
                count(&mut self.snapshot.refusals);
                ConcurrencyChange::Refusal
            }
            502 | 504 => {
                count(&mut self.snapshot.proxy_errors);
                ConcurrencyChange::Proxy
            }
            _ => return,
        };
        self.last_pressure = Some(now);
        self.reset();
        if self.snapshot.mode == ConcurrencyMode::Adaptive
            && self.snapshot.target > 1
            && self
                .last_decrease
                .is_none_or(|t| now.saturating_duration_since(t) >= HOLD)
        {
            self.change(self.snapshot.target.div_ceil(2), now, reason);
            self.last_decrease = Some(now);
            count(&mut self.snapshot.decreases);
        }
    }

    pub fn finish(&mut self, terminal: &Terminal, witness: Option<Witness>, now: Instant) {
        if terminal.outcome != Outcome::Success {
            match terminal.outcome {
                Outcome::HttpRejected(429 | 502 | 504) => {}
                Outcome::HttpRejected(_) => count(&mut self.snapshot.other_statuses),
                Outcome::Deadline => count(&mut self.snapshot.deadlines),
                Outcome::Transport => count(&mut self.snapshot.transport_errors),
                Outcome::Cancelled => count(&mut self.snapshot.cancellations),
                Outcome::Framing => count(&mut self.snapshot.framing_errors),
                Outcome::Incomplete => count(&mut self.snapshot.incomplete_responses),
                Outcome::ResourceLimit => count(&mut self.snapshot.resource_failures),
                Outcome::ConsumerLost => count(&mut self.snapshot.consumer_losses),
                Outcome::Unsupported | Outcome::Success => {}
            }
            self.reset();
            return;
        }
        let eligible = matches!(terminal.synthetic_usage, SyntheticUsage::Known(_))
            && !terminal.output_backpressure
            && witness.is_some_and(|w| {
                w.demand
                    && w.generation.is_some()
                    && w.generation == self.snapshot.generation
                    && now.saturating_duration_since(w.committed) <= FRESH
            })
            && self
                .last_pressure
                .is_none_or(|t| now.saturating_duration_since(t) >= HOLD);
        if !eligible {
            count(&mut self.snapshot.ineligible_successes);
            self.reset();
            return;
        }
        if self.snapshot.mode == ConcurrencyMode::Fixed
            || self.snapshot.target == self.snapshot.ceiling
        {
            self.reset();
            return;
        }
        if self
            .last_sample
            .is_some_and(|t| now.saturating_duration_since(t) > FRESH)
        {
            self.reset();
        }
        self.last_sample = Some(now);
        self.snapshot.recovery_samples += 1;
        if self.snapshot.recovery_samples == 4 {
            self.change(self.snapshot.target + 1, now, ConcurrencyChange::Recovery);
            count(&mut self.snapshot.increases);
        }
    }
}
