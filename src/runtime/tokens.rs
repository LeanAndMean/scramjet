use super::*;

pub const MAX_SYNTHETIC_TOKENS: u32 = 65_536;
const MAX_DEBT: u32 = MAX_EXPOSURE as u32 * MAX_SYNTHETIC_TOKENS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntheticTokenBudget {
    pub(super) capacity: u32,
    quantum: u32,
    interval: Duration,
}
#[derive(Debug, PartialEq, Eq)]
pub struct InvalidSyntheticTokens;
impl SyntheticTokenBudget {
    pub fn new(
        capacity: u32,
        quantum: u32,
        interval: Duration,
    ) -> Result<Self, InvalidSyntheticTokens> {
        if !(1..=MAX_SYNTHETIC_TOKENS).contains(&capacity)
            || !(1..=capacity).contains(&quantum)
            || interval < Duration::from_millis(1)
            || interval > Duration::from_secs(86400)
        {
            return Err(InvalidSyntheticTokens);
        }
        Ok(Self {
            capacity,
            quantum,
            interval,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntheticCost {
    input: u32,
    output: u32,
}
impl SyntheticCost {
    pub fn new(input: u32, output: u32) -> Result<Self, InvalidSyntheticTokens> {
        if input > MAX_SYNTHETIC_TOKENS || output > MAX_SYNTHETIC_TOKENS || input + output == 0 {
            return Err(InvalidSyntheticTokens);
        }
        Ok(Self { input, output })
    }
    pub(super) fn total(self) -> u32 {
        self.input + self.output
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyntheticUsage {
    NotAttempted,
    Unknown,
    Known(u32),
}
impl SyntheticUsage {
    pub(crate) fn add_to(self, total: Option<u64>) -> Option<u64> {
        match self {
            Self::NotAttempted => total,
            Self::Unknown => None,
            Self::Known(value) => total.and_then(|n| n.checked_add(u64::from(value))),
        }
    }
}

pub(super) struct TokenBucket {
    pub(super) config: SyntheticTokenBudget,
    pub(super) credit: u32,
    pub(super) debt: u32,
    checkpoint: tokio::time::Instant,
    pub(super) charges: Option<u64>,
    pub(super) measured: Option<u64>,
}
impl TokenBucket {
    pub(super) fn new(config: SyntheticTokenBudget) -> Self {
        Self {
            config,
            credit: config.capacity,
            debt: 0,
            checkpoint: tokio::time::Instant::now(),
            charges: Some(0),
            measured: Some(0),
        }
    }
    pub(super) fn full(&self) -> bool {
        self.debt == 0 && self.credit == self.config.capacity
    }
    pub(super) fn refresh(&mut self, now: tokio::time::Instant) {
        let elapsed = now.saturating_duration_since(self.checkpoint);
        let ticks = elapsed.as_nanos() / self.config.interval.as_nanos();
        let missing = self.debt + self.config.capacity - self.credit;
        let needed = u128::from(missing).div_ceil(u128::from(self.config.quantum));
        if ticks >= needed {
            self.debt = 0;
            self.credit = self.config.capacity;
            self.checkpoint = now;
        } else {
            let earned = (ticks * u128::from(self.config.quantum)) as u32;
            let payment = earned.min(self.debt);
            self.debt -= payment;
            self.credit += earned - payment;
            let remainder = elapsed.as_nanos() % self.config.interval.as_nanos();
            self.checkpoint = now
                - Duration::new(
                    (remainder / 1_000_000_000) as u64,
                    (remainder % 1_000_000_000) as u32,
                );
        }
    }
    pub(super) fn wait(&self, now: tokio::time::Instant, cost: u32) -> Duration {
        let missing = self.debt + cost.saturating_sub(self.credit);
        let ticks = missing.div_ceil(self.config.quantum);
        (self.config.interval * ticks)
            .saturating_sub(now.saturating_duration_since(self.checkpoint))
    }
    pub(super) fn reserve(&mut self, cost: u32) {
        assert!(self.debt == 0 && self.credit >= cost);
        self.credit -= cost;
        self.charges = self.charges.and_then(|n| n.checked_add(u64::from(cost)));
    }
    pub(super) fn actual(&mut self, reserved: u32, actual: u32) -> bool {
        assert!(actual <= MAX_SYNTHETIC_TOKENS);
        self.refresh(tokio::time::Instant::now());
        self.measured = self.measured.and_then(|n| n.checked_add(u64::from(actual)));
        let excess = actual.saturating_sub(reserved);
        self.charges = self.charges.and_then(|n| n.checked_add(u64::from(excess)));
        let payment = excess.min(self.credit);
        self.credit -= payment;
        self.debt = self
            .debt
            .checked_add(excess - payment)
            .expect("bounded synthetic debt");
        assert!(self.debt <= MAX_DEBT);
        actual > reserved
    }
}
pub(super) struct UsageObligation {
    pub(super) id: RequestId,
    pub(super) reserved: u32,
    pub(super) terminal_unknown: bool,
}
