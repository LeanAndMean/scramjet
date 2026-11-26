//! `/loop`, as Claude Code's: a prompt sent again on an interval while the session waits for
//! the user. It is due at once when set, then an interval after each run; one due during a
//! turn runs once the turn ends, never twice. As Claude Code's recurring tasks, it ends after
//! `LIFETIME`, firing once more first.
use super::Agent;
use std::time::{Duration, Instant};

/// How long a loop lasts, as Claude Code's `recurringMaxAgeMs`.
pub const LIFETIME: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Clone, Debug)]
pub struct Loop {
    pub prompt: String,
    pub every: Duration,
    /// How many times it was sent.
    pub runs: usize,
    next: Instant,
    ends: Instant,
}

impl Loop {
    /// How long until it is due, from `now`.
    pub fn due_in(&self, now: Instant) -> Duration {
        self.next.saturating_duration_since(now)
    }
}

/// A due loop's prompt, and whether this run is its last.
#[derive(Debug, PartialEq)]
pub struct Due {
    pub prompt: String,
    pub last: bool,
}

impl Agent {
    /// Starts sending `prompt` every `every`, the first time at once, in place of any loop.
    pub fn start_loop(&mut self, prompt: &str, every: Duration, now: Instant) {
        self.recurring = Some(Loop {
            prompt: prompt.to_owned(),
            every,
            runs: 0,
            next: now,
            ends: now + LIFETIME,
        });
    }

    pub fn stop_loop(&mut self) -> Option<Loop> {
        self.recurring.take()
    }

    pub fn looping(&self) -> Option<&Loop> {
        self.recurring.as_ref()
    }

    /// The loop's prompt if it is due at `now`, counted as sent; the next run is an interval
    /// later. After its last run the loop ends.
    pub fn loop_due(&mut self, now: Instant) -> Option<Due> {
        let looping = self
            .recurring
            .as_mut()
            .filter(|looping| now >= looping.next)?;
        looping.runs += 1;
        looping.next = now + looping.every;
        let last = looping.next >= looping.ends;
        let prompt = looping.prompt.clone();
        if last {
            self.recurring = None;
        }
        Some(Due { prompt, last })
    }
}

/// `every` as `/loop` takes it, larger units first: `5m`, `1h 30m`.
pub fn interval(every: Duration) -> String {
    let mut left = every.as_secs();
    let mut parts = Vec::new();
    for (unit, seconds) in [("d", 86400), ("h", 3600), ("m", 60), ("s", 1)] {
        if left >= seconds {
            parts.push(format!("{}{unit}", left / seconds));
            left %= seconds;
        }
    }
    match parts.is_empty() {
        true => "0s".to_owned(),
        false => parts.join(" "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::agent;

    #[test]
    fn a_loop_runs_at_once_then_each_interval_after_a_run_and_ends_after_its_lifetime() {
        let (mut agent, _scratch) = agent("127.0.0.1:9".parse().unwrap(), "loop");
        let start = Instant::now();
        let minute = Duration::from_secs(60);
        agent.start_loop("check the deploy", 5 * minute, start);
        let due = agent.loop_due(start);
        assert_eq!(
            due,
            Some(Due {
                prompt: "check the deploy".to_owned(),
                last: false
            })
        );
        assert_eq!(agent.loop_due(start + 4 * minute), None);
        assert_eq!(agent.looping().unwrap().due_in(start + 4 * minute), minute);
        // Due during a long turn, it runs once when the turn ends, and the next is an interval
        // after that run.
        let late = start + 12 * minute;
        assert!(agent.loop_due(late).is_some());
        assert_eq!(agent.loop_due(late + 4 * minute), None);
        assert!(agent.loop_due(late + 5 * minute).is_some());
        assert_eq!(agent.looping().unwrap().runs, 3);
        let near_end = start + LIFETIME - minute;
        assert_eq!(agent.loop_due(near_end).map(|due| due.last), Some(true));
        assert!(agent.looping().is_none());
        assert_eq!(agent.loop_due(near_end + 10 * minute), None);
    }
}
